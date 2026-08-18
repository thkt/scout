---
status: "accepted"
date: 2026-05-13
decision-makers: thkt (project owner)
---

# SSRF Defense Architecture and fetch.rs Module Structure

## Context and Problem Statement

scout は Web fetch + GitHub repo exploration CLI で、user 入力 URL を取得する責務がある。SSRF 攻撃を防ぐため fetch 経路は複数の防御層 (DNS pre-check, redirect 制御, headless browser subrequest 制御) を持つ。

`Scout` 構造体は 2 つの `reqwest::Client` を保持: `http` (auto-redirect、API 用) と `fetch_http` (`Policy::none()`、user URL 用)。field コメントは現状を記述するが、未来 contributor 向けの rule (どちらを使うべきか) は明示されておらず、型でも強制されない。

`fetch.rs` (1456 行) は `js-rendering` feature を `#[cfg]` で plain HTTP path と同居させており、`#[cfg_attr(...)] allow(dead_code)` の散在が構造的 smell を生んでいる。両者ともに「現状の判断」を明文化する必要がある。

## Decision Drivers

- SSRF contract 違反は silent security incident に直結 (型で守られない不変条件)
- 個人 OSS scale で型強制 (Newtype) と code review 依存のコスト比較が必要
- `fetch.rs` の module split は thin-extract heuristic の locality を損なう可能性

## Considered Options

- Option A: Newtype 化 + module split (full enforcement)
- Option B: ADR で contract 明文化 + 現状構造維持 (lightweight)
- Option C: コメント拡充のみ (no ADR)

## Decision Outcome

Chosen option: Option B, because 個人 OSS scale の review 負荷とコード変更コストのバランスで、ADR が「未来 contributor 向け rule」を提供しつつ実装変更を回避できる。Newtype 化と split は trigger 条件 (incident or 規模超過) まで保留する。

### Consequences

- Good, because 新規 command 追加時の SSRF 配慮 path が明文化された
- Good, because fetch.rs の現状構造を意図ある選択として記録、split trigger を数値化
- Bad, because 型強制ではないので contract 違反は code review にのみ依存
- Bad, because `fetch.rs` の `allow(dead_code)` smell は残る

### Confirmation

新規 command PR で `self.http(...)` 呼び出し箇所に対し reviewer が URL source (user 入力か信頼済みか) を確認する。CI で `fetch.rs` の行数を check し、2000 行超過時に warning (将来追加検討)。

## Pros and Cons of the Options

### Option A: Newtype 化 + module split

`SsrfSafeClient` newtype 導入と `fetch.rs` を `fetch/{download,heuristic,browser}.rs` に split。

- Good, because contract 違反が compile error として検出される
- Good, because module boundary が明確になり、`#[cfg]` smell が消える
- Bad, because 既存 6 command のリファクタリング cost が大きい
- Bad, because fallback heuristic と orchestrator が別 module になり、code review 時の文脈分断

### Option B: ADR 明文化 + 現状維持 (採用)

ADR で SSRF contract と fetch.rs 構造判断を記録、Newtype/split は trigger 条件で再評価。

- Good, because 実装変更ゼロで未来 contributor への rule を提供
- Good, because trigger 条件 (incident or 規模超過) で再評価できる
- Bad, because 型強制ではないので review 依存
- Bad, because `allow(dead_code)` smell は残る

### Option C: コメント拡充のみ

field コメントに contract を追記、ADR は作らない。

- Good, because 最小コスト
- Bad, because rule の根拠 (なぜ Newtype を選ばないか) が記録されない
- Bad, because contract がコメントに散在し、未来 contributor が全体像を把握しにくい

## More Information

### Implementation Guidelines

| Client              | 用途                                             | Redirect Policy                                       |
| ------------------- | ------------------------------------------------ | ----------------------------------------------------- |
| `Scout::http`       | Brave Search API / GitHub API / Slack API        | `limited(5)` (reqwest 既定)                           |
| `Scout::fetch_http` | user 入力 URL を扱う全 fetch 経路                | `Policy::none()` + 手動 redirect + per-hop SSRF check |

新規 command 追加時のルール:

- user 入力 URL を含む経路は MUST `fetch_http` を使う
- 信頼済みエンドポイントなら `http` でよい

### Reassessment Triggers

| Trigger                                                  | アクション                            |
| -------------------------------------------------------- | ------------------------------------- |
| SSRF contract 違反 incident 発生                         | `SsrfSafeClient` newtype 化を即検討   |
| 新規 command 追加で計 9 以上                             | Review 漏れリスク上昇、Newtype 化検討 |
| `src/fetch/` の 1 実装ファイルが 1000 行超                | そのファイルの split 検討             |
| `#[cfg(feature = "js-rendering")]` 累積行数 > plain path | Module split 検討                     |

### 参照ファイル

ADR 制定時の行参照は `fetch.rs` 分割 (commit `a7a7a4f`、後述 Addendum (2026-06-24): Decision Outcome ドリフト参照) で移動したため、現行位置に更新済み。

- `src/tools.rs` の `Scout` の `http` / `fetch_http` field 定義、`src/tools/builder.rs` の `build_default_clients` (`fetch_http` に `Policy::none()`)
- `src/fetch/download.rs` の `download` (per-hop SSRF check を伴う manual redirect 経路)
- `src/fetch/cdp.rs` の `cdp_navigate` の `EventRequestPaused` listener (protocol 上は `Fetch.RequestPaused`)、`src/fetch/cdp/launch.rs` の `check_browser_request` (subrequest 判定)
- `docs/audit/2026-05-13-undocumented-decisions.md` (本 ADR の根拠 audit)

## Addendum (2026-06-24): blocklist 構成と CDP subrequest scheme の列挙

ADR ギャップ監査 (`docs/audit/2026-06-24-020601-adr-gaps.md`、downgrade 候補 20/21) で、本 ADR の SSRF 境界を構成する 2 つの具体テーブル (blocked-host の合成と browser subrequest の scheme 判定) が docstring とテストにのみ pin され ADR 化されていないと判定された。ADR-0012 の Addendum 方針に倣い、決定本文は変えず以下を一次ソースの列挙として追記する。判定ロジックは `src/fetch/ssrf.rs` と `src/fetch/cdp/launch.rs` が真実源で、本節はその転記である。

### blocked-host 合成 (downgrade 21、一次ソース `src/fetch/ssrf.rs` の `validate_url_sync` / `is_blocked_host` / `is_private_ip`)

`validate_url_sync` は scheme を `http`/`https` に限定し (それ以外は `InvalidScheme`)、`is_blocked_host` で host を判定する。`is_blocked_host` は host 種別ごとに分岐し、IP は `is_private_ip` へ委譲する。host が無い URL は `None` 分岐で fail closed (block)。

| host 種別       | block 条件                                                                                                           |
| --------------- | -------------------------------------------------------------------------------------------------------------------- |
| Domain (suffix) | `localhost` / `.localhost` / `.local` / `.internal` / `.arpa` (ASCII 小文字化して比較)                               |
| host 無し       | `None` は常に block (fail closed)                                                                                    |
| IPv4            | loopback / private / link-local / unspecified / `0.0.0.0/8` (先頭 octet 0) / broadcast / CGN `100.64.0.0/10`         |
| IPv6            | loopback / unspecified / link-local `fe80::/10` / unique-local `fc00::/7` / IPv4 埋め込み (mapped + compat) の再判定 |

IPv6 の IPv4 埋め込みは `to_ipv4` (`to_ipv4_mapped` ではない) で unwrap し、IPv4-mapped (`::ffff:a.b.c.d`) と IPv4-compatible (`::a.b.c.d`、例 `::7f00:1` = `::127.0.0.1`) の双方を再帰的に IPv4 判定へ通す。CGN は `is_cgn` が先頭 octet 100 かつ 2 番目 octet 64..=127 で判定する。`is_private_ip` は connect-time の `SsrfResolver` (ADR-0012) も同じ関数を共用するため、pre-flight と connect 時で blocklist が一致する。

### CDP browser subrequest の scheme 判定 (downgrade 20、一次ソース `src/fetch/cdp/launch.rs` の `check_browser_request`)

`js-rendering` 経路で chromium が `Fetch.RequestPaused` を発火するたび、`check_browser_request` が subrequest URL の scheme で許可/遮断を決める。SOCKS5 proxy (ADR-0021) が TCP egress を縛るのと別に、scheme 単位の allowlist をここで適用する。

| scheme                                       | 扱い                                                          |
| -------------------------------------------- | ------------------------------------------------------------- |
| `http` / `https`                             | `ssrf_check` に直接通す (blocklist + connect-time guard)      |
| `ws` / `wss`                                 | `http` / `https` に書き換えてから `ssrf_check` (内部到達防止) |
| `data:` / `about:` / `chrome:` / `blob:`     | 外部 egress の無い合成 scheme として SSRF check 無しで許可    |
| 上記以外 (`file:` / `ftp:` / `gopher:` ほか) | 分類不能として warn + block (catch-all で fail closed)        |

`ws`/`wss` を http(s) へ写してから検査するのは、WebSocket が内部サービスへ到達しうるため SSRF allowlist を同じ blocklist で適用するため。テストは `[T-F047]` が scheme ごとの allow/block を pin する。

## Addendum (2026-06-24): Decision Outcome ドリフト (module split 実施済み + URL 軸型強制の追加)

ADR ギャップ監査 (`docs/audit/2026-06-24-020601-adr-gaps.md`、DRIFT 側流し #260) で、本 ADR の Decision Outcome「Newtype 化と split は trigger 条件 (incident or 規模超過) まで保留する」が現状コードと乖離していると判定された。決定本文 (Option B 採用) は当時の lightweight な判断として保持し、保留としていた 2 点が trigger 発火ではない要因で先行実施された事実を以下に記録する。

### module split は可読性要因で実施済み (trigger 未発火)

`fetch.rs` は commit `a7a7a4f` (`refactor(fetch): split fetch.rs below 400 lines`) で `src/fetch/{ssrf,cdp,download,converter,extractor}.rs` 等へ分割され、本体は ADR 制定時の 1456 行から 400 行へ縮小した。Reassessment Triggers の「`fetch.rs` 行数 > 2000」「`#[cfg(feature = "js-rendering")]` 累積行数 > plain path」のいずれも発火していない。分割の動機は trigger 条件ではなく可読性で、commit message (`split fetch.rs below 400 lines`) が本体行数の削減を主目的として明示する。さらに `cdp` サブツリーは testability/coverage invariant に従い `cdp/{proxy,transport}.rs` へ 4-way split されている (`docs/audit/2026-06-24-020601-adr-gaps.md` finding #11、stream-generic ロジックを offline 100% でテストし OS-I/O を transport tier に隔離して diff-coverage gate から除外)。Option B の「現状構造維持」は incident や規模超過ではなく可読性主導の選択で上書きされたと読み替える。当時懸念した「fallback heuristic と orchestrator の文脈分断」は、`thin-extract` heuristic が `src/fetch/extractor.rs` に locality を保って収まることで実害化しなかった。

### URL 軸の型強制 (`ValidatedUrl`) が追加済み

commit `871da8f` (`fix(security): enforce SSRF via ValidatedUrl + redact URL logs (issue #100)`) で `ValidatedUrl` newtype (`src/fetch/ssrf.rs` の `ValidatedUrl`) が導入された。これは async な `ssrf_check` (前述 Addendum の sync 段 `validate_url_sync` を内包し、続けて connect-time の IP guard を適用する gate) のみが構築でき、downstream (`download`/`reqwest::Client::get`) が `&ValidatedUrl` を受け取ることで「全 fetch path が SSRF check を通過した URL のみを扱う」ことを型で強制する。

この型強制は Option A が想定した `SsrfSafeClient` とは別軸である。Option A の newtype は client 選択 (`http` か `fetch_http` か) を型で縛る client 単位の抽象で、これは現状も未導入である。`ValidatedUrl` は URL 単位の検証済みマーカーで、enforcement の軸が異なる。

軸は異なるが、本 ADR が Option B の Consequences として記録した「Bad, because 型強制ではないので contract 違反は code review にのみ依存」は部分的に不正確になった。現状の正確な切り分けは次のとおり。

| SSRF contract の構成要素                             | 現状の enforcement                                           |
| ---------------------------------------------------- | ------------------------------------------------------------ |
| user URL が SSRF check を通過したか                  | `ValidatedUrl` 型で強制 (`ssrf_check` 以外で構築不能)        |
| user URL 経路で `fetch_http` (`Policy::none()`) 選択 | 型では未強制、Implementation Guidelines + code review に依存 |

監査 finding #13 はこの `ValidatedUrl` を Option B 違反 (DRIFT) として/adrift へ流したが、上記のとおり Option A の client 軸 newtype 採用ではなく別軸の URL 軸型強制であるため、status は accepted を維持する。本節はその「なぜ Option A 採用ではないか」を pin し、後続の/adrift・/census が同じ判定を再フラグするのを防ぐ。
