---
status: "accepted"
date: 2026-07-21
decision-makers: thkt (project owner)
---

# Proxy Egress Delegation for Fetch

## Context and Problem Statement

ADR-0012 は reqwest 経路 (`fetch_http`) の DNS rebind を connect 時 IP guard (`SsrfResolver`) で塞いだ。この guard は connector が dial する実 IP を `is_private_ip` で検証し private を reject する。scout 自身が host を解決し dial する Direct 運用ではこれが成立する。

しかし scout を forward proxy 配下で動かす運用 (egress gateway, sidecar proxy, 企業 proxy) では、この構成が成立しない。proxy 経由では scout は host を解決も dial もせず、proxy が解決・dial する。connect 時 guard は scout が実際に dial する先である proxy の address (loopback / private な proxy 自身) を private と判定し、全 fetch をブロックしてしまう。同様に `ssrf_check` の DNS 事前チェック (`resolver.lookup`) は、scout が到達しない address を解決・検証することになり、proxy 側でしか解決できない host を誤って弾く。

標準の proxy 環境変数 (`HTTPS_PROXY` / `HTTP_PROXY` とその小文字形) を設定した運用で fetch を成立させつつ、OUTCOME Constraint「全 fetch 経路で SSRF 防御を必須とする」を保つ方針が未記録だった。本 ADR はこの reqwest 経路の proxied-egress carve-out を扱う。CDP/chromium 経路 (ADR-0021) は独立の loopback SOCKS5 proxy を持ち、外部 proxy env の影響を受けない (後述)。

## Decision Drivers

- forward proxy 配下 (egress gateway / sidecar) で scout を動かす運用を成立させる
- proxy 運用では egress control (宛先許可リスト, IMDS ブロック, 名前解決ポリシー) は proxy 層の責務であり、scout が名前解決を二重に行うと proxy でしか解決できない host を誤ブロックする
- caller 供給 URL が literal private/loopback を直指定する SSRF は proxy の有無に依らず塞ぎ続ける
- reqwest 0.13 は `Proxy::all` で全 scheme を forward proxy へ流す default API を提供する

## Considered Options

- 方式 P: proxy env 検出時に literal 検査 (scheme allowlist + literal private/loopback/suffix reject) のみ scout 側で維持し、名前解決に基づく防御 (DNS 事前チェックと connect 時 IP guard) を proxy の egress control へ委譲する (採用)
- 方式 Q: proxy 経由でも scout の DNS 事前チェック・connect 時 guard を維持する
- 方式 R: proxy 非対応。proxy env が設定された環境では fetch を無効化する

## Decision Outcome

Chosen option: 方式 P。`detect_egress_mode` (src/fetch/ssrf.rs) が env map から proxy URL を検出し `EgressMode::{Direct, Proxied(url)}` を返す。`ScoutBuilder::from_env` が一度検出し (src/tools/builder.rs)、`build_default_clients` が mode に応じて `fetch_http` を組む。Proxied では `Proxy::all(url)` で全 request を forward proxy へ流し、ADR-0012 の `SsrfResolver` connect 時 guard を外す (guard は scout が dial する先である loopback/private な proxy address 自体を private と判定しブロックするため)。detect した mode は `Scout.egress` に持ち、`fetch` が `FetchOptions.egress` として `fetch_page` へ渡す。

防御の分担 (defense split):

- literal 検査は scout 側で全 mode 共通に効く。`validate_url_sync` (scheme allowlist + literal private/loopback IP / blocked-suffix reject) は URL 自体を対象に mode に依らず走り、`download` の redirect loop が各 hop で `ssrf_check` を同 mode で再適用するため、literal reject は全 hop で維持される。
- 名前解決に基づく防御は mode で分かれる。Direct では scout の DNS 事前チェック + connect 時 IP guard (ADR-0012) が DNS rebind を含めて塞ぐ。Proxied では scout はこれらを行わず (`ssrf_check` が `resolver.lookup` を skip し、connect 時 guard を外す)、名前解決に基づく防御 (DNS rebind を含む) を proxy の egress control へ委譲する。

Proxied を選ぶのは proxy env を明示設定した運用者であり、その運用者は proxy 側 egress policy が名前解決由来の SSRF (rebind, public→private を返す DNS) を塞ぐ責務を負う。scout は名前解決を proxy と二重に行わないことで、proxy でしか解決できない正当 host の誤ブロックを避ける。

方式 Q は scout が dial する proxy address 自体を connect 時 guard がブロックし fetch が全滅する、かつ proxy でしか解決できない host を DNS 事前チェックが誤ブロックするため却下。方式 R は forward proxy 配下という現実的な運用を放棄し OUTCOME に反するため却下。

### Consequences

- Good, because forward proxy 配下で fetch が成立する (guard が proxy address を誤ブロックしない)
- Good, because literal SSRF 検査は全 mode で不変 (caller URL の private/loopback 直指定は Proxied でも全 hop で reject)
- Good, because 名前解決防御を proxy の egress control に一本化し、二重解決による正当 host の誤ブロックを避ける
- Neutral, because Direct (default、proxy env 未設定) は ADR-0012 の connect 時 guard を維持し挙動不変
- Bad, because Proxied では名前解決由来の SSRF 防御が proxy 依存になり、proxy の egress policy が緩いと DNS rebind が通りうる (mode を選ぶ運用者の責務であり、OUTCOME Constraint にこの carve-out を明記する)

### Confirmation

`src/fetch/ssrf/egress_tests.rs` の[T-001..T-004]が `detect_egress_mode` の env→mode 写像を pin する (`HTTPS_PROXY` 優先、大文字優先、無ければ `Direct`)。`src/tools/builder_tests.rs` が Proxied で `fetch_http` が proxy 経由になり guard を持たないことを、`src/fetch/ssrf/tests.rs` と `src/fetch/fetch_page_tests.rs` が Proxied で literal reject を維持しつつ DNS 事前チェックを skip することを pin する。reqwest 更新時は `Proxy::all` が全 scheme を forward し、proxy env 検出精度が本 ADR の前提どおりかを再検証する。

## Pros and Cons of the Options

### 方式 P: literal は scout、名前解決防御は proxy へ委譲 (採用)

proxy env 検出時、literal 検査を維持し名前解決防御を proxy の egress control に委ねる。

- Good, because proxy 配下運用が成立し、literal SSRF 検査は全 mode で不変
- Good, because 名前解決を二重化せず正当 host の誤ブロックを避ける
- Bad, because Proxied の rebind 防御が proxy の egress policy 依存になる

### 方式 Q: proxy 経由でも scout の名前解決防御を維持

Proxied でも DNS 事前チェックと connect 時 guard を残す。

- Good, because scout 側で rebind 防御が完結する
- Bad, because guard が dial 先の proxy address を private と判定し fetch が全滅する
- Bad, because proxy でしか解決できない host を DNS 事前チェックが誤ブロックする

### 方式 R: proxy 非対応

proxy env が設定された環境では fetch を無効化する。

- Good, because 実装が単純で SSRF 境界の判断が Direct のみになる
- Bad, because forward proxy 配下という現実的な運用を放棄し OUTCOME に反する

## More Information

### CDP/chromium 経路との関係

CDP 経路 (ADR-0021) は chromium を独立の loopback SOCKS5 proxy 経由で起動し、`check_browser_request` (src/fetch/cdp/launch.rs) は scout 自身のプロセス内で走る subrequest allowlist check のため `EgressMode::Direct` を明示的に渡す (scout が直接解決するため DNS 事前チェックが効く)。外部 proxy env (`HTTPS_PROXY` 等) は scout の reqwest `fetch_http` のみを Proxied にし、CDP 経路のフラグや loopback proxy は変えない。本 ADR は reqwest 経路に限定し、CDP 経路の egress 制御は ADR-0021 が引き続き扱う。

### reqwest 0.13.4 一次ソース確認

- `Proxy::all<U: IntoProxy>(proxy_scheme: U) -> Result<Proxy>`: 全 `http`/`https` traffic を渡した URL の proxy へ流す (<https://docs.rs/reqwest/0.13.4/reqwest/struct.Proxy.html> で確認)。
- proxy 環境変数の precedence (大文字/小文字の優先順) は上記ページに記載が無く、`detect_egress_mode` が採る「`HTTPS_PROXY` 優先、大文字優先、first match」の case 順は egress_tests の四シナリオが権威スペックであり、reqwest 側 precedence との一致は unverified (src/fetch/ssrf.rs の doc comment と同じ扱い)。

### 参照

- ADR-0001 (SSRF contract), ADR-0012 (Direct 経路の connect 時 IP guard。本 ADR が Proxied carve-out を足し、0012 に Addendum を追記した), ADR-0021 (CDP 経路の loopback SOCKS5 proxy と launch フラグ)
- issue / branch: `fix/ssrf-proxy-env`
- `src/fetch/ssrf.rs` (`EgressMode`, `detect_egress_mode`, `ssrf_check` の mode gate)、`src/tools/builder.rs` (`build_default_clients` の mode 分岐)
