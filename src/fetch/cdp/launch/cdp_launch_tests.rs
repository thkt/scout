use super::*;

/// [T-F043] t009_launch_args_contain_security_flags
#[test]
fn t009_launch_args_contain_security_flags() {
    let args = build_launch_args();
    for flag in [
        "--disable-webrtc",
        "--disable-background-networking",
        "--disable-features=DnsOverHttps",
        "--disable-domain-reliability",
        "--no-pings",
    ] {
        assert!(args.contains(&flag), "missing security flag: {flag}");
    }
}
