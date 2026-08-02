//! OS-dependent transport for the SOCKS5 proxy: the listener accept loop, the
//! upstream dial, and the bidirectional byte tunnel.
//!
//! The error arms here fire only under real socket faults — an `accept` failure
//! (EMFILE/ENFILE/ECONNABORTED), an upstream dial that black-holes past the
//! timeout, or a tunnel copy that resets mid-stream. None can be forced by an
//! offline unit test without flaky timing or a socket-injection mock, so this
//! file is held to its own coverage rather than the global diff gate. The
//! testable SOCKS5 protocol logic lives in the parent module.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tracing::{debug, warn};

use super::{REP_GENERAL_FAILURE, REP_SUCCESS, handle_conn, send_reply};
use crate::fetch::ssrf::DnsResolver;

/// Cap an upstream dial so a black-holed (validated) public IP returns
/// `REP_GENERAL_FAILURE` promptly instead of hanging on the OS default (~75s).
const UPSTREAM_DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Backoff after a transient `accept` error so a sustained fault (e.g. EMFILE)
/// cannot busy-spin the loop while file descriptors stay exhausted.
const ACCEPT_RETRY_BACKOFF: Duration = Duration::from_millis(50);

/// Bind a loopback SOCKS5 proxy on `127.0.0.1:0` and return its port plus the
/// accept-loop task.
///
/// The listener is moved into the accept task rather than closed-and-reopened,
/// so no other process can grab the port between `local_addr()` and the first
/// accept (a TOCTOU that would let chromium connect somewhere scout never
/// validated). The task ends when `cancel` flips to `true` (SIGINT path) or when
/// the caller aborts it after reaping chromium.
pub(in crate::fetch::cdp) async fn spawn_ssrf_proxy(
    resolver: Arc<dyn DnsResolver>,
    cancel: watch::Receiver<bool>,
) -> io::Result<(u16, JoinHandle<()>)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let task = tokio::spawn(accept_loop(listener, resolver, cancel));
    Ok((port, task))
}

async fn accept_loop(
    listener: TcpListener,
    resolver: Arc<dyn DnsResolver>,
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        let accepted = tokio::select! {
            biased;
            // `wait_for` is sticky: a flag already `true` at subscribe time
            // (SIGINT arrived earlier) resolves immediately and ends the loop.
            _ = cancel.wait_for(|&c| c) => break,
            accepted = listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, _peer)) => {
                let resolver = Arc::clone(&resolver);
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(stream, resolver.as_ref()).await {
                        debug!(error = %e, "SOCKS5 proxy connection ended with error");
                    }
                });
            }
            Err(e) => {
                // Transient accept failures (EMFILE/ENFILE/ECONNABORTED) must not
                // kill the proxy for the whole session: a dead proxy means every
                // later chromium subrequest fails closed.
                warn!(error = %e, "SOCKS5 proxy accept failed; retrying");
                sleep(ACCEPT_RETRY_BACKOFF).await;
            }
        }
    }
}

/// Dial the already-validated upstream and tunnel bytes both ways. Split from
/// `handle_conn` so the dial-failure reply path can be exercised without the
/// connect-time validation gate: callers must validate `dial_addr` first.
///
/// Generic over the client stream so a refused-dial reply can be asserted with a
/// loopback `TcpStream` pair while the success/tunnel path stays covered by the
/// chromium e2e (T-201-7).
pub(super) async fn dial_and_tunnel<S>(client: &mut S, dial_addr: SocketAddr) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut upstream = match timeout(UPSTREAM_DIAL_TIMEOUT, TcpStream::connect(dial_addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            debug!(error = %e, "SOCKS5 upstream dial failed");
            return send_reply(client, REP_GENERAL_FAILURE).await;
        }
        Err(_) => {
            debug!(addr = %dial_addr, "SOCKS5 upstream dial timed out");
            return send_reply(client, REP_GENERAL_FAILURE).await;
        }
    };
    send_reply(client, REP_SUCCESS).await?;

    // Tunnel. A closed client socket (chromium reaped) ends the copy; that is the
    // normal teardown, not an error worth surfacing above debug.
    if let Err(e) = copy_bidirectional(client, &mut upstream).await {
        debug!(error = %e, "SOCKS5 tunnel closed with error");
    }
    Ok(())
}
