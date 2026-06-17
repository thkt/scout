//! SOCKS5 SSRF proxy tests. All offline: rejection cases close before any dial,
//! and the one validation-pass case (T-201-3) exercises `all_ips_public`
//! directly so no real upstream connection is attempted (full-tunnel success is
//! covered by the chromium e2e, T-201-7).

use std::net::IpAddr;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::yield_now;
use tracing_test::traced_test;

use super::transport::dial_and_tunnel;
use super::{all_ips_public, handle_conn, spawn_ssrf_proxy};
use crate::fetch::ssrf::{DnsResolver, StaticDnsResolver};

/// SOCKS5 no-auth greeting: VER=5, one method (no-auth).
const GREETING: [u8; 3] = [0x05, 0x01, 0x00];

/// Build a SOCKS5 CONNECT request for a domain target.
fn connect_domain(domain: &str, port: u16) -> Vec<u8> {
    let mut req = vec![
        0x05,
        CMD_CONNECT,
        0x00,
        0x03,
        u8::try_from(domain.len()).unwrap(),
    ];
    req.extend_from_slice(domain.as_bytes());
    req.extend_from_slice(&port.to_be_bytes());
    req
}

const CMD_CONNECT: u8 = 0x01;
const CMD_BIND: u8 = 0x02;

/// Run `handle_conn` against a loopback client that writes `client_bytes`, then
/// return everything the proxy wrote back (it closes after a rejection reply, so
/// `read_to_end` terminates).
async fn run_proxy(resolver: Arc<dyn DnsResolver>, client_bytes: Vec<u8>) -> Vec<u8> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = handle_conn(stream, resolver.as_ref()).await;
    });

    let mut client = TcpStream::connect(addr).await.unwrap();
    client.write_all(&client_bytes).await.unwrap();
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).await.unwrap();
    server.await.unwrap();
    reply
}

/// REP byte sits at offset 3: [method-reply(2)] then [VER, REP, ..].
fn rep_code(reply: &[u8]) -> u8 {
    assert!(reply.len() >= 4, "reply too short: {reply:?}");
    reply[3]
}

/// T-201-1: CONNECT to a domain that resolves to the IMDS link-local address is
/// rejected with REP 0x02 and logs the blocked address. This is the DNS-rebind
/// regression (acceptance criterion 1): a public pre-flight cannot save a
/// connect-time private resolution because the proxy validates the dial IP.
#[tokio::test]
#[traced_test]
async fn t201_1_rebind_to_imds_replies_not_allowed_and_logs() {
    let resolver = Arc::new(StaticDnsResolver::single("169.254.169.254"));
    let mut bytes = GREETING.to_vec();
    bytes.extend(connect_domain("evil.test", 443));

    let reply = run_proxy(resolver, bytes).await;

    assert_eq!(
        rep_code(&reply),
        0x02,
        "private resolution must reply NOT_ALLOWED"
    );
    assert!(
        logs_contain("blocked connect to private IP"),
        "block reason must be logged"
    );
}

/// T-201-2: a literal loopback IPv4 target (ATYP 0x01) is validated too, not just
/// domains. REP 0x02.
#[tokio::test]
async fn t201_2_ipv4_literal_loopback_replies_not_allowed() {
    // Resolver is unused for IP literals; supply a public IP to prove the
    // literal itself (127.0.0.1) is what gets rejected.
    let resolver = Arc::new(StaticDnsResolver::single("93.184.216.34"));
    let mut bytes = GREETING.to_vec();
    // CONNECT 127.0.0.1:80, ATYP=IPv4.
    bytes.extend_from_slice(&[0x05, CMD_CONNECT, 0x00, 0x01, 127, 0, 0, 1, 0, 80]);

    let reply = run_proxy(resolver, bytes).await;

    assert_eq!(rep_code(&reply), 0x02);
}

/// T-201-3: a domain resolving to a single public IP passes validation: no block
/// reply, no block log. Exercises `all_ips_public` directly so no real dial is
/// attempted (the equivalence class "all public" maps to "allowed").
#[tokio::test]
#[traced_test]
async fn t201_3_public_resolution_passes_validation() {
    let public: IpAddr = "93.184.216.34".parse().unwrap();

    assert!(all_ips_public("example.test", &[public]));
    assert!(
        !logs_contain("blocked connect to private IP"),
        "a public IP must not log a block"
    );
}

/// T-201-4: when resolution returns a mix of public and private IPs, the whole
/// connection is rejected (fail-closed). REP 0x02, no dial.
#[tokio::test]
async fn t201_4_mixed_public_and_private_fails_closed() {
    let resolver = Arc::new(StaticDnsResolver(vec![
        "93.184.216.34".parse().unwrap(),
        "169.254.169.254".parse().unwrap(),
    ]));
    let mut bytes = GREETING.to_vec();
    bytes.extend(connect_domain("evil.test", 443));

    let reply = run_proxy(resolver, bytes).await;

    assert_eq!(rep_code(&reply), 0x02, "one private IP must reject the set");
}

/// T-201-5: a non-CONNECT command (BIND) is refused with REP 0x07.
#[tokio::test]
async fn t201_5_bind_command_replies_cmd_not_supported() {
    let resolver = Arc::new(StaticDnsResolver::single("93.184.216.34"));
    let mut bytes = GREETING.to_vec();
    // BIND request header only. The server rejects on CMD before parsing the
    // address, so DST.ADDR/DST.PORT are omitted: sending trailing bytes the
    // server never reads would make it close with unread data, which macOS
    // signals as a RST that would corrupt the client's `read_to_end`. Real
    // chromium only ever sends CONNECT, where the full request is consumed
    // before any reply.
    bytes.extend_from_slice(&[0x05, CMD_BIND, 0x00, 0x01]);

    let reply = run_proxy(resolver, bytes).await;

    assert_eq!(rep_code(&reply), 0x07);
}

/// T-201-6: a non-SOCKS5 greeting (VER 0x04) closes the connection without a
/// reply and without panicking.
#[tokio::test]
async fn t201_6_non_socks5_greeting_closes_silently() {
    let resolver = Arc::new(StaticDnsResolver::single("93.184.216.34"));
    // Exactly the 2-byte greeting head: VER=0x04 (SOCKS4) plus NMETHODS. The
    // server rejects on the version byte and reads nothing further, so no
    // trailing bytes are sent (an unread tail would trigger a macOS RST).
    let bytes = vec![0x04, 0x01];

    let reply = run_proxy(resolver, bytes).await;

    assert!(
        reply.is_empty(),
        "no reply expected for non-SOCKS5 greeting"
    );
}

/// T-201-9: a wrong version byte in the request stage (after a valid greeting)
/// closes the connection without a SOCKS5 reply. The greeting's method reply
/// ([0x05, 0x00]) is the only thing written back.
#[tokio::test]
async fn t201_9_request_stage_version_mismatch_closes_after_method_reply() {
    let resolver = Arc::new(StaticDnsResolver::single("93.184.216.34"));
    let mut bytes = GREETING.to_vec();
    // Request header with VER=0x04. The server consumes the 4-byte header, sees
    // the bad version, and returns without reading an address, so no trailing
    // bytes are sent (an unread tail would trigger a macOS RST).
    bytes.extend_from_slice(&[0x04, CMD_CONNECT, 0x00, 0x01]);

    let reply = run_proxy(resolver, bytes).await;

    assert_eq!(
        reply,
        vec![0x05, 0x00],
        "only the greeting method reply, no request reply"
    );
}

/// T-201-10: an IPv6 literal target (ATYP 0x04) is validated like IPv4. The
/// loopback address ::1 is private, so REP 0x02.
#[tokio::test]
async fn t201_10_ipv6_literal_loopback_replies_not_allowed() {
    let resolver = Arc::new(StaticDnsResolver::single("93.184.216.34"));
    let mut bytes = GREETING.to_vec();
    // CONNECT [::1]:80, ATYP=IPv6 (16-byte address + 2-byte port).
    bytes.extend_from_slice(&[0x05, CMD_CONNECT, 0x00, 0x04]);
    bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
    bytes.extend_from_slice(&80u16.to_be_bytes());

    let reply = run_proxy(resolver, bytes).await;

    assert_eq!(rep_code(&reply), 0x02);
}

/// T-201-11: an unsupported address type (ATYP 0x05) is refused with REP 0x08.
#[tokio::test]
async fn t201_11_unknown_atyp_replies_addr_not_supported() {
    let resolver = Arc::new(StaticDnsResolver::single("93.184.216.34"));
    let mut bytes = GREETING.to_vec();
    // Request header only: the server rejects on the unknown ATYP before reading
    // any address, so trailing bytes are omitted (unread tail = macOS RST).
    bytes.extend_from_slice(&[0x05, CMD_CONNECT, 0x00, 0x05]);

    let reply = run_proxy(resolver, bytes).await;

    assert_eq!(rep_code(&reply), 0x08);
}

/// T-201-12: a domain that resolves to zero IPs is refused with REP 0x04
/// (host unreachable), never dialed.
#[tokio::test]
async fn t201_12_empty_resolution_replies_host_unreachable() {
    let resolver = Arc::new(StaticDnsResolver(vec![]));
    let mut bytes = GREETING.to_vec();
    bytes.extend(connect_domain("empty.test", 443));

    let reply = run_proxy(resolver, bytes).await;

    assert_eq!(rep_code(&reply), 0x04);
}

/// T-201-13: when the validated upstream refuses the connection, the client gets
/// REP 0x01 (general failure). Calls `dial_and_tunnel` directly with a closed
/// loopback port — the validation gate is bypassed deliberately so a refused dial
/// can be forced without a reachable public host.
#[tokio::test]
async fn t201_13_upstream_dial_refused_replies_general_failure() {
    // Reserve then drop a loopback listener: the port is now closed, so a connect
    // to it is refused immediately (ECONNREFUSED), deterministically and fast.
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dial_addr = closed.local_addr().unwrap();
    drop(closed);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = dial_and_tunnel(&mut stream, dial_addr).await;
    });

    let mut client = TcpStream::connect(addr).await.unwrap();
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).await.unwrap();
    server.await.unwrap();

    assert_eq!(
        reply[1], 0x01,
        "refused dial must reply REP_GENERAL_FAILURE"
    );
}

/// T-201-14: a client that connects and drops before sending the greeting makes
/// `handle_conn` return an error, which the accept loop logs without killing the
/// proxy. Exercises the full `spawn_ssrf_proxy` accept path.
#[tokio::test]
#[traced_test]
async fn t201_14_abrupt_client_disconnect_is_logged_not_fatal() {
    let resolver = Arc::new(StaticDnsResolver::single("93.184.216.34"));
    let (_tx, rx) = watch::channel(false);
    let (port, task) = spawn_ssrf_proxy(resolver, rx).await.unwrap();

    // Connect then drop immediately: the server's first `read_exact` hits EOF.
    let client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    drop(client);

    // Poll the log instead of a fixed sleep so the spawned handler has run.
    for _ in 0..100 {
        if logs_contain("SOCKS5 proxy connection ended with error") {
            break;
        }
        yield_now().await;
    }
    task.abort();

    assert!(
        logs_contain("SOCKS5 proxy connection ended with error"),
        "an abrupt disconnect must be logged at the accept loop"
    );
}

/// T-201-15: a client that connects but sends no SOCKS5 bytes is closed after
/// `HANDSHAKE_TIMEOUT` with no reply, rather than pinning its handler on the
/// first `read_exact` forever. Driven over an in-memory `duplex` under a paused
/// clock so the timeout fires in virtual time with no real wait and no socket
/// reactor I/O (the module is generic over the stream precisely for this).
#[tokio::test(start_paused = true)]
#[traced_test]
async fn t201_15_no_handshake_bytes_times_out_and_closes() {
    let resolver = Arc::new(StaticDnsResolver::single("93.184.216.34"));
    let (mut client, server) = duplex(64);

    // Send nothing: handle_conn hangs on the greeting read until the deadline.
    let result = handle_conn(server, resolver.as_ref()).await;

    assert!(
        result.is_ok(),
        "handshake timeout closes cleanly, not as Err"
    );
    assert!(
        logs_contain("SOCKS5 handshake timed out"),
        "the timeout path must log its reason"
    );
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).await.unwrap();
    assert!(reply.is_empty(), "no reply on handshake timeout");
}

/// T-201-16: a client that sends only the greeting and then stalls before the
/// request is closed after `HANDSHAKE_TIMEOUT`. The deadline spans both read
/// stages, so the stall on the request `read_exact` (a different hang point than
/// T-201-15) is bounded too; only the greeting's method reply was written back.
#[tokio::test(start_paused = true)]
#[traced_test]
async fn t201_16_partial_handshake_times_out_after_method_reply() {
    let resolver = Arc::new(StaticDnsResolver::single("93.184.216.34"));
    let (mut client, server) = duplex(64);

    // Greeting only: handle_conn replies no-auth, then hangs on the request read.
    client.write_all(&GREETING).await.unwrap();
    let result = handle_conn(server, resolver.as_ref()).await;

    assert!(
        result.is_ok(),
        "handshake timeout closes cleanly, not as Err"
    );
    assert!(logs_contain("SOCKS5 handshake timed out"));
    let mut reply = Vec::new();
    client.read_to_end(&mut reply).await.unwrap();
    assert_eq!(
        reply,
        vec![0x05, 0x00],
        "only the greeting method reply precedes the timeout"
    );
}
