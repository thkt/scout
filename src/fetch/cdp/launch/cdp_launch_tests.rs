use super::*;

/// [T-F043]
#[test]
fn t009_launch_args_contain_security_flags() {
    let args = build_launch_args(0);
    for flag in [
        "--disable-webrtc",
        "--disable-background-networking",
        "--disable-features=DnsOverHttps",
        "--disable-domain-reliability",
        "--no-pings",
    ] {
        assert!(
            args.iter().any(|a| a == flag),
            "missing security flag: {flag}"
        );
    }
}

/// [T-201-8] proxy flags route every chromium TCP egress through the SSRF proxy.
#[test]
fn t201_8_launch_args_contain_ssrf_proxy_flags() {
    let args = build_launch_args(54321);
    assert!(
        args.iter()
            .any(|a| a == "--proxy-server=socks5://127.0.0.1:54321"),
        "missing SOCKS5 proxy-server flag with port"
    );
    for flag in ["--proxy-bypass-list=<-loopback>", "--disable-quic"] {
        assert!(args.iter().any(|a| a == flag), "missing proxy flag: {flag}");
    }
}
