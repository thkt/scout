use super::*;
use crate::clock::FixedClock;
use crate::envelope::ErrorCode;
use crate::fetch::{FailingDnsResolver, StaticDnsResolver};
use crate::rng::SeededRng;
use crate::test_support::try_spawn_mock_server;
use crate::token_source::StaticTokenSource;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// [T-SB001] `ScoutBuilder::with_clock` で渡した `Arc` が `Scout.clock` まで
/// 届く injection slot の最小証明。end-to-end な plumbing 確認は T-SB004。
#[test]
fn scout_builder_with_clock_routes_arc_into_scout() {
    let injected: Arc<dyn Clock> = Arc::new(FixedClock(42));
    let scout = ScoutBuilder::for_test()
        .with_clock(injected.clone())
        .build();
    assert!(
        Arc::ptr_eq(&scout.clock, &injected),
        "with_clock must install the supplied Arc into Scout.clock"
    );
}

/// [T-SB002] `ScoutBuilder::with_rng` で渡した `Arc` が `Scout.rng` まで
/// 届く injection slot の最小証明。
#[test]
fn scout_builder_with_rng_routes_arc_into_scout() {
    let injected: Arc<dyn Rng> = Arc::new(SeededRng::new(7));
    let scout = ScoutBuilder::for_test().with_rng(injected.clone()).build();
    assert!(
        Arc::ptr_eq(&scout.rng, &injected),
        "with_rng must install the supplied Arc into Scout.rng"
    );
}

/// [T-SB003] `ScoutBuilder::with_token_source` で渡した `Arc` が
/// `Scout.token_source` まで届く injection slot の最小証明。
#[test]
fn scout_builder_with_token_source_routes_arc_into_scout() {
    let injected: Arc<dyn TokenSource> = Arc::new(StaticTokenSource(None));
    let scout = ScoutBuilder::for_test()
        .with_token_source(injected.clone())
        .build();
    assert!(
        Arc::ptr_eq(&scout.token_source, &injected),
        "with_token_source must install the supplied Arc into Scout.token_source"
    );
}

/// [T-DNS001] `ScoutBuilder::with_dns` で渡した `Arc<dyn DnsResolver>` が
/// `Scout.dns` slot に届き、かつ `Scout::fetch` の SSRF 経路で実際に
/// consult されることを end-to-end で確認する。
///
/// 注入した `StaticDnsResolver(10.0.0.1)` が `https://example.com` の
/// DNS lookup を override すれば、`ssrf_check` の private-IP 判定が
/// `FetchError::InternalHost` を即座に返す。default の `TokioDnsResolver`
/// なら `example.com` は public IP を返すため、この assert は
/// injection が wire できていない場合に必ず落ちる。
#[tokio::test]
async fn scout_builder_with_dns_blocks_fetch_via_injected_private_ip() {
    let injected: Arc<dyn DnsResolver> = Arc::new(StaticDnsResolver::single("10.0.0.1"));
    let scout = ScoutBuilder::for_test().with_dns(injected.clone()).build();

    assert!(
        Arc::ptr_eq(&scout.dns, &injected),
        "with_dns must install the supplied Arc into Scout.dns"
    );

    let result = scout
        .fetch(FetchParams {
            url: Some("https://example.com/page".into()),
            js: false,
            raw: false,
        })
        .await;
    let err = result.expect_err("injected private IP must trip SSRF check");
    assert_eq!(
        err.error_kind(),
        ErrorCode::DataError,
        "SSRF InternalHost maps to DataError (sysexits EX_DATAERR)"
    );
    assert!(
        err.message().contains("internal/private"),
        "error message must surface the SSRF cause, got: {}",
        err.message()
    );
}

/// [T-DNS002] `FailingDnsResolver` を inject すると `Scout::fetch` が
/// `FetchError::DnsResolution` 由来の `ScoutError` を返すことを確認する。
/// resolver の失敗パスが SSRF 経路に正しく伝播することを保証する。
#[tokio::test]
async fn scout_builder_with_dns_propagates_resolver_failure() {
    let injected: Arc<dyn DnsResolver> =
        Arc::new(FailingDnsResolver("simulated DNS failure".into()));
    let scout = ScoutBuilder::for_test().with_dns(injected).build();

    let result = scout
        .fetch(FetchParams {
            url: Some("https://example.com/page".into()),
            js: false,
            raw: false,
        })
        .await;
    let err = result.expect_err("injected resolver failure must surface as error");
    assert!(
        err.message().contains("DNS resolution failed"),
        "error message must surface the DNS failure cause, got: {}",
        err.message()
    );
}

/// [T-SB004] `with_clock` で inject した `FixedClock` が `Scout::github()`
/// 経由で初期化される `GitHubClient` まで届くことを end-to-end で確認する。
/// `Arc::ptr_eq` 単体テスト (T-SB001) では `github()` の plumbing バグ
/// (例: clone 忘れ、async move への束縛漏れ) を catch できないので、
/// wiremock 越しに `secs_until_ratelimit_reset` の算出値を assert する。
///
/// reset = 1600, clock = 1000 → retry_after = 600 が `MAX_RETRY_AFTER_SECS`
/// (300) を超えるため `is_retriable = false` で retry loop はスキップ。
/// `start_paused` を併用すると wiremock の TCP listener も止まり connect が
/// timeout するので、retry を走らせない算術にする方が安定する。
#[tokio::test]
async fn scout_builder_clock_reaches_github_client_via_seam() {
    let Some(server) = try_spawn_mock_server("tools::scout_builder_seam").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo"))
        .respond_with(
            ResponseTemplate::new(403)
                .append_header("x-ratelimit-remaining", "0")
                .append_header("x-ratelimit-reset", "1600")
                .set_body_json(serde_json::json!({"message": "rate limit exceeded"})),
        )
        .mount(&server)
        .await;

    let scout = ScoutBuilder::for_test()
        .with_clock(Arc::new(FixedClock(1000)))
        .with_github_endpoint(&server.uri())
        .build();

    let result = scout.github().await.get_repo("owner", "repo").await;
    assert!(
        matches!(
            result,
            Err(github::GitHubError::RateLimited {
                retry_after: Some(600)
            })
        ),
        "expected retry_after = 600 (reset 1600 - clock 1000), got: {result:?}"
    );
}

/// [T-SB005] `with_slack_endpoint` で inject した wiremock endpoint が
/// `fetch`(slack permalink) → `fetch_slack` → `slack()` の production 経路で
/// 構築される `SlackClient` まで届くことを end-to-end で確認する。`slack()` の
/// `OnceCell` を build() で pre-set するため `SLACK_TOKEN` 未設定でも注入
/// クライアントが使われ、`conversations.history` に到達して本文を取得できる。
/// 注入が wire できていなければ `from_env` が `TokenNotSet` を返し落ちる
/// (issue #191)。
#[tokio::test]
async fn scout_builder_slack_endpoint_reaches_fetch_slack_via_seam() {
    let Some(server) = try_spawn_mock_server("tools::scout_builder_slack_seam").await else {
        return;
    };
    Mock::given(method("GET"))
        .and(path("/conversations.history"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "messages": [
                {"type": "message", "text": "hello from wiremock", "ts": "1773819598.273499"}
            ]
        })))
        .mount(&server)
        .await;

    let scout = ScoutBuilder::for_test()
        .with_slack_endpoint(&server.uri())
        .build();

    let result = scout
        .fetch(FetchParams {
            url: Some("https://team.slack.com/archives/C123/p1773819598273499".into()),
            js: false,
            raw: false,
        })
        .await;
    let output =
        result.expect("injected slack endpoint must serve fetch_slack without SLACK_TOKEN");
    assert!(
        output.markdown().contains("hello from wiremock"),
        "fetch output must carry the wiremock message body, got: {}",
        output.markdown()
    );
}
