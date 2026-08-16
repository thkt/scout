//! Loopback SOCKS5 proxy that re-validates connect addresses for the CDP
//! (chromium) fetch path, closing the DNS-rebind SSRF gap that
//! `check_browser_request`'s resolve-time pre-flight cannot reach.
//!
//! chromium resolves DNS itself when it dials, so scout cannot inject a
//! `Resolve` the way it does for the reqwest path (ADR-0012 method Y').
//! Instead scout launches chromium with the proxy flags (`--proxy-server`,
//! `--proxy-bypass-list=<-loopback>`, `--disable-quic`), forcing every TCP
//! egress through this proxy. The proxy resolves each CONNECT target once and
//! dials only IPs that pass `is_private_ip`, fail-closed: one private IP rejects
//! the whole connection (mirrors `SsrfResolver`). This relocates method Y' to
//! the proxy layer without a shared host-to-IP pin map (which ADR-0012 rejected
//! as method X).
//!
//! This file is the SOCKS5 protocol layer: greeting/request parsing, fail-closed
//! IP validation, and reply encoding. It is generic over the client byte stream
//! (`AsyncRead + AsyncWrite`) and owns no sockets, so every branch is covered by
//! offline unit tests (T-201-*). The OS-dependent transport — the listener accept
//! loop, the upstream dial, and the byte tunnel, whose error arms fire only under
//! real socket faults — lives in `transport`.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;
use tracing::debug;

use crate::fetch::ssrf::{DnsResolver, first_blocked_ip};

#[cfg(test)]
mod proxy_tests;
mod transport;

#[cfg_attr(not(feature = "js-rendering"), allow(unused_imports))]
pub(in crate::fetch::cdp) use transport::spawn_ssrf_proxy;

/// Cap the SOCKS5 handshake/request read so a client that connects but stalls
/// mid-greeting (sends nothing, or only part of the request) cannot pin its
/// handler task on `read_exact` indefinitely. chromium completes the handshake
/// in microseconds; this only bounds a misbehaving same-host peer (the proxy
/// listens on `127.0.0.1`). Mirrors `UPSTREAM_DIAL_TIMEOUT`'s 10s.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

// SOCKS5 wire constants (RFC 1928).
const SOCKS5_VERSION: u8 = 0x05;
const METHOD_NO_AUTH: u8 = 0x00;
const CMD_CONNECT: u8 = 0x01;
const ATYP_IPV4: u8 = 0x01;
const ATYP_DOMAIN: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;

// SOCKS5 reply codes (RFC 1928 § 6).
const REP_SUCCESS: u8 = 0x00;
const REP_GENERAL_FAILURE: u8 = 0x01;
const REP_NOT_ALLOWED: u8 = 0x02;
const REP_HOST_UNREACHABLE: u8 = 0x04;
const REP_CMD_NOT_SUPPORTED: u8 = 0x07;
const REP_ADDR_NOT_SUPPORTED: u8 = 0x08;

/// CONNECT target before resolution.
enum Target {
    Ip(IpAddr),
    Domain(String),
}

/// Serve one SOCKS5 client (CONNECT only). Returns `Ok(())` for both successful
/// tunnels and handled policy rejections (a reply was sent, then the connection
/// closed); returns `Err` only on a transport/protocol read-write failure.
///
/// Generic over the client stream so the full parse/validate/reply path can be
/// driven by offline unit tests; the accept loop instantiates it with a
/// `TcpStream`.
async fn handle_conn<S>(mut stream: S, resolver: &dyn DnsResolver) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // 1-2. Read and parse the greeting + CONNECT request under a single deadline.
    //      Every `read_exact` here reads client bytes, so a peer that connects but
    //      stalls cannot pin the task; on timeout the future drops (freeing the
    //      borrow of `stream`) and the connection closes with no reply. The dial
    //      and tunnel that follow have their own bounds and legitimately run long,
    //      so they stay outside this timeout.
    let (target, port) = match timeout(HANDSHAKE_TIMEOUT, read_request(&mut stream)).await {
        Ok(Ok(Some(parsed))) => parsed,
        Ok(Ok(None)) => return Ok(()), // dropped or replied mid-parse
        Ok(Err(e)) => return Err(e),
        Err(_elapsed) => {
            debug!("SOCKS5 handshake timed out");
            return Ok(());
        }
    };

    // 3. Resolve the target to the IPs that will actually be dialed.
    let (host_label, ips): (String, Vec<IpAddr>) = match target {
        Target::Ip(ip) => (ip.to_string(), vec![ip]),
        Target::Domain(domain) => match resolver.lookup(&domain, port).await {
            Ok(ips) if !ips.is_empty() => (domain, ips),
            Ok(_) | Err(_) => return send_reply(&mut stream, REP_HOST_UNREACHABLE).await,
        },
    };

    // 4. Validate fail-closed before any dial.
    if first_blocked_ip("proxy", &host_label, &ips).is_some() {
        return send_reply(&mut stream, REP_NOT_ALLOWED).await;
    }

    // 5. Dial the validated IP and tunnel. Every IP in `ips` already passed the
    //    private-IP check, so there is no unvalidated happy-eyeballs fallback.
    let dial_addr = SocketAddr::new(ips[0], port);
    transport::dial_and_tunnel(&mut stream, dial_addr).await
}

/// Read the SOCKS5 greeting and CONNECT request, returning the parsed target and
/// port. `Ok(None)` means the handshake was terminated in-band (a non-SOCKS5
/// version dropped without reply, or a mid-parse rejection reply was sent), so
/// the caller has nothing further to do. Split out of `handle_conn` so the whole
/// client-read span sits behind one `HANDSHAKE_TIMEOUT`.
async fn read_request<S>(stream: &mut S) -> io::Result<Option<(Target, u16)>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Greeting: [VER, NMETHODS, METHODS..]. Reply no-auth.
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).await?;
    if head[0] != SOCKS5_VERSION {
        return Ok(None); // not SOCKS5: drop without reply
    }
    let mut methods = vec![0u8; usize::from(head[1])];
    stream.read_exact(&mut methods).await?;
    stream.write_all(&[SOCKS5_VERSION, METHOD_NO_AUTH]).await?;

    // Request: [VER, CMD, RSV, ATYP, DST.ADDR, DST.PORT(2 BE)].
    let mut req = [0u8; 4];
    stream.read_exact(&mut req).await?;
    if req[0] != SOCKS5_VERSION {
        return Ok(None);
    }
    if req[1] != CMD_CONNECT {
        send_reply(stream, REP_CMD_NOT_SUPPORTED).await?;
        return Ok(None);
    }
    let target = match req[3] {
        ATYP_IPV4 => {
            let mut a = [0u8; 4];
            stream.read_exact(&mut a).await?;
            Target::Ip(IpAddr::from(a))
        }
        ATYP_IPV6 => {
            let mut a = [0u8; 16];
            stream.read_exact(&mut a).await?;
            Target::Ip(IpAddr::from(a))
        }
        ATYP_DOMAIN => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut name = vec![0u8; usize::from(len[0])];
            stream.read_exact(&mut name).await?;
            Target::Domain(String::from_utf8_lossy(&name).into_owned())
        }
        _ => {
            send_reply(stream, REP_ADDR_NOT_SUPPORTED).await?;
            return Ok(None);
        }
    };
    let mut port_buf = [0u8; 2];
    stream.read_exact(&mut port_buf).await?;
    let port = u16::from_be_bytes(port_buf);
    Ok(Some((target, port)))
}

/// Send a SOCKS5 reply with `BND.ADDR = 0.0.0.0:0` (RFC 1928 § 6). The bound
/// address is irrelevant for a CONNECT proxy on loopback, so a fixed all-zero
/// IPv4 address is returned for every reply code.
async fn send_reply<S>(stream: &mut S, rep: u8) -> io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let reply = [SOCKS5_VERSION, rep, 0x00, ATYP_IPV4, 0, 0, 0, 0, 0, 0];
    stream.write_all(&reply).await
}
