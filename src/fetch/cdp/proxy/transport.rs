//! OS-dependent transport for the SOCKS5 proxy: the listener accept loop, the
//! upstream dial, and the bidirectional byte tunnel.
//!
//! T-201-13 covers the failure reply for a refused loopback dial and T-F088 the
//! fallback to a later address with a tunnel over loopback. T-F089 to T-F091
//! cover how the dial budget is shared, with a stand-in dialer and a paused
//! clock. Accept faults and mid-stream tunnel resets require other real socket
//! faults and are not covered. This layer is excluded from the global diff
//! coverage gate; the parent protocol layer remains on it.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout};
use tracing::{debug, warn};

use super::{REP_GENERAL_FAILURE, REP_SUCCESS, handle_conn, send_reply};
use crate::fetch::ssrf::DnsResolver;

/// Cap the upstream dial so black-holed (validated) public IPs return
/// `REP_GENERAL_FAILURE` promptly instead of hanging on the OS default (~75s).
/// The budget covers the whole address list.
const UPSTREAM_DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// Shortest wait one address gets while budget remains. A lost SYN is
/// retransmitted after about 1s, so an even split across many records must not
/// drop below a wait that survives one retransmission.
const MIN_DIAL_ATTEMPT: Duration = Duration::from_secs(2);

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

/// Try each address in order and return the first connection. Each attempt
/// waits for an even share of the time left, so time an early address did not
/// use (an immediate refusal) passes to the later ones.
///
/// Generic over the dialer so the budget can be tested without sockets.
pub(super) async fn dial_first_reachable<T, D, Fut>(dial_addrs: &[SocketAddr], dial: D) -> Option<T>
where
    D: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = io::Result<T>>,
{
    let deadline = Instant::now() + UPSTREAM_DIAL_TIMEOUT;
    for (index, &addr) in dial_addrs.iter().enumerate() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let addresses_left = u32::try_from(dial_addrs.len() - index).unwrap_or(u32::MAX);
        let attempt = (remaining / addresses_left)
            .max(MIN_DIAL_ATTEMPT)
            .min(remaining);
        match timeout(attempt, dial(addr)).await {
            Ok(Ok(stream)) => return Some(stream),
            Ok(Err(e)) => debug!(%addr, error = %e, "SOCKS5 upstream dial failed"),
            Err(_) => debug!(%addr, "SOCKS5 upstream dial timed out"),
        }
    }
    None
}

/// Dial the already-validated upstream addresses in order and tunnel bytes both
/// ways through the first that connects. Split from `handle_conn` so the
/// dial-failure reply path can be exercised without the connect-time validation
/// gate: callers must validate every entry of `dial_addrs` first.
///
/// Generic over the client stream so a refused-dial reply can be asserted with a
/// loopback `TcpStream` pair.
pub(super) async fn dial_and_tunnel<S>(client: &mut S, dial_addrs: &[SocketAddr]) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(mut upstream) = dial_first_reachable(dial_addrs, TcpStream::connect).await else {
        return send_reply(client, REP_GENERAL_FAILURE).await;
    };
    send_reply(client, REP_SUCCESS).await?;

    // Tunnel. A closed client socket (chromium reaped) ends the copy; that is the
    // normal teardown, not an error worth surfacing above debug.
    if let Err(e) = copy_bidirectional(client, &mut upstream).await {
        debug!(error = %e, "SOCKS5 tunnel closed with error");
    }
    Ok(())
}
