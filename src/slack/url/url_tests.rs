use super::*;

/// [T-SK005] parse_slack_url accepts a standard archive permalink
#[test]
fn parse_standard_url() {
    let url = "https://myteam.slack.com/archives/C0656BJSFL7/p1773819598273499";
    let parsed = parse_slack_url(url).unwrap();
    assert_eq!(parsed.workspace, "myteam");
    assert_eq!(parsed.channel, "C0656BJSFL7");
    assert_eq!(parsed.ts, "1773819598.273499");
    assert!(parsed.thread_ts.is_none());
}

/// [T-SK006]
#[test]
fn parse_parent_permalink_with_thread_ts() {
    let url = "https://team.slack.com/archives/C123/p1234567890123456?thread_ts=1234567890.123456&cid=C123";
    let parsed = parse_slack_url(url).unwrap();
    assert_eq!(parsed.channel, "C123");
    assert_eq!(parsed.ts, "1234567890.123456");
    assert_eq!(parsed.thread_ts.as_deref(), Some("1234567890.123456"));
}

/// [T-SK007]
#[test]
fn parse_reply_permalink_has_different_ts_and_thread_ts() {
    let url = "https://team.slack.com/archives/C123/p1111111111222222?thread_ts=1234567890.123456&cid=C123";
    let parsed = parse_slack_url(url).unwrap();
    assert_eq!(parsed.ts, "1111111111.222222");
    assert_eq!(parsed.thread_ts.as_deref(), Some("1234567890.123456"));
}

/// [T-SK012] parse_slack_url rejects URLs outside *.slack.com
#[test]
fn parse_rejects_non_slack_url() {
    assert!(parse_slack_url("https://example.com/page").is_none());
}

/// [T-SK013]
#[test]
fn parse_rejects_non_archives_path() {
    assert!(parse_slack_url("https://team.slack.com/messages/C123/p111111222222333").is_none());
}

/// [T-SK014] parse_slack_url rejects timestamps shorter than micro-precision
#[test]
fn parse_rejects_short_timestamp() {
    assert!(parse_slack_url("https://team.slack.com/archives/C123/p12345").is_none());
}
