# API Documentation — scout

## 外部契約は CLI 表面にある

crate としての公開面は `pub async fn run() -> ExitCode` の 1 つだけである。`Cargo.toml` の `unreachable_pub = "deny"` がこれを機械的に固定するため、**scout の外部契約は Rust API ではなく CLI の表面**、すなわちサブコマンド・フラグ・終了コード・stdout の形式・環境変数にある。

契約の記述と検証は 3 層に分かれる。

| 層   | 所在                                                                                                                                        | 役割                                       |
| ---- | ------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| 決定 | `docs/decisions/` の DR-0002 / 0003 / 0010 / 0016 / 0017 / 0019 / 0020                                                                      | 契約そのものを決める                       |
| 実装 | `src/lib.rs` の `Cli` と `after_help`、`src/tools/params.rs` の `enum Command`、`src/envelope.rs`                                           | 実際の表面                                 |
| pin  | `tests/exit_code_contract.rs`、`tests/cli_integration.rs`、`tests/output_injection.rs`、および各 DR の Confirmation 節が名指しするテスト ID | 契約が壊れたらビルドではなくテストが落ちる |

## Provided — CLI サブコマンド

`src/tools/params.rs` の `enum Command` が 6 variant を定義し、`src/tools.rs` の `run()` が 6 分岐でディスパッチする。

| CLI 表記        | variant        | 受け取るもの                                                                                                      |
| --------------- | -------------- | ----------------------------------------------------------------------------------------------------------------- |
| `search`        | `Search`       | 検索クエリ                                                                                                        |
| `fetch`         | `Fetch`        | URL。`src/tools/query.rs` が `parse_slack_url` で Slack permalink を判定し、一致すれば `fetch_slack` へ振り分ける |
| `research`      | `Research`     | 検索クエリ。内部で `search` と `fetch` を合成する                                                                 |
| `repo-tree`     | `RepoTree`     | owner / repo / ref とグロブパターン                                                                               |
| `repo-read`     | `RepoRead`     | owner / repo / path                                                                                               |
| `repo-overview` | `RepoOverview` | owner / repo                                                                                                      |

引数は stdin からも取れる。`src/tools.rs` の `StdinResolver` が 3 状態 (引数あり/stdin から読む/どちらも無い) を持つ。

## Provided — グローバルフラグと終了コード

グローバルフラグは `src/lib.rs` の `struct Cli` にある。

| フラグ      | 意味                                                                       |
| ----------- | -------------------------------------------------------------------------- |
| `--json`    | 1 行 JSON envelope で出力する (`global = true` なので全サブコマンドに効く) |
| `--version` | バージョンを出し、`AGENT_HELP_HINT` を tracing 経由で stderr へ出す        |
| `--help`    | 終了コード表・環境変数・チューニング範囲を全部載せた `after_help` を出す   |

終了コードは `src/envelope.rs` の `ErrorCode::exit_code` と `src/signals.rs` の `InterruptSignal::exit_code` が決める。**`ErrorCode` は 10 variant で、成功の 0 を足した 11 値が `after_help` の表に載る** — 0/64/65/66/70/74/75/104/124/130/143。sysexits.h と GNU coreutils と POSIX の 128+signo という 3 系統を統合した判断が DR-0002 と DR-0017 に記録されている。

**シグナル側の 2 値は POSIX 規約の導出である。** `src/signals.rs` の `enum InterruptSignal` は `Sigint` と `Sigterm` の 2 variant を持ち、`exit_code()` が 128 + signal number で 130 と 143 を返す。`Sigterm` は `#[cfg(unix)]` 配下にあり、非 unix ビルドには存在しない。導出の規約が doc コメントに書かれている。

シグナル経路には猶予がある。`src/lib.rs` の `drive()` が signal race を扱い、`SHUTDOWN_DRAIN_TIMEOUT` が 7 秒である。

## Provided — JSON envelope

`--json` を付けたときの stdout は必ず 1 行で、成功と失敗で形が分かれる (DR-0010)。型は `src/envelope.rs` の `SuccessEnvelope`/`ErrorEnvelope`/`ErrorPayload`。

| 側   | フィールド                                                                              | 省略条件                                                      |
| ---- | --------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| 成功 | `data`、`degraded`、`notes`、`degraded_reasons`                                         | `degraded_reasons` は `skip_serializing_if = "Vec::is_empty"` |
| 失敗 | `error.code`、`error.message`、`error.next_step`、`error.candidates`、`error.retryable` | `next_step` と `candidates` は空なら省略                      |

`src/lib.rs` の描画経路は 4 本ある — `render_json_success`/`render_json_error`/`bare_error_line`/`write_failure_line`。最後の 2 本は envelope の描画自体が失敗しうる場面のためにある。

### 劣化通知 (`DegradedReason`、14 値)

部分失敗を **文字列パースなしに** 検出させるための機械可読な理由コードである (DR-0003)。`src/envelope.rs` の `enum DegradedReason` が 14 variant を持ち、JSON へは `SCREAMING_SNAKE_CASE` で出る。`research` が 1 本の URL 取得に失敗しても run 全体は成功として返り、理由がこの配列に載る (`architecture.md` の `## Interaction Diagrams` 参照)。

**cap のヒットも劣化として呼び出し側へ運ばれる。** Slack 経路には 4 つの cap がある — `SLACK_REPLIES_LIMIT`、`SLACK_USERS_CONCURRENCY`、`SLACK_MAX_REPLY_PAGES`、`SLACK_MAX_USER_LOOKUPS`。ヒットは `src/slack/client.rs` の `SlackFetchOutcome` が持つ 3 つの bool (`thread_truncated`/`users_capped`/`lookups_failed`) で運ばれる。`String` を返すだけでは cap が見えないという設計理由が同構造体の doc コメントにある。`lookups_failed` と `users_capped` を分ける理由も同じ場所にあり、名前を持たずに返った lookup は呼び出し側に retry するものが無いので失敗に数えない。**この 3 bool が `DegradedReason` のどの variant へ写るかは、この CodeKB では追っていない。**

## Provided — 本文の上限

出力に載るバイト数には 3 層の上限がある。**共有する cap と単一バックエンド専用の cap は置き場が分かれる規約になっている** — `src/body_limit.rs` の module doc が「2 バックエンド以上が共有する cap はここへ、1 つだけのものはそのバックエンドに残す」と定める。

| 上限                     | 値    | 所在                   | 掛かる対象                                              |
| ------------------------ | ----- | ---------------------- | ------------------------------------------------------- |
| `MAX_API_RESPONSE_BYTES` | 1 MiB | `src/body_limit.rs`    | 4 バックエンドが共有する API 応答本文                   |
| `MAX_PAGE_BYTES`         | 4,500 | `src/search/engine.rs` | `research` の 1 ページあたりの本文                      |
| `MAX_FIELD_BYTES`        | 450   | `src/yaml.rs`          | frontmatter の 1 フィールド。上の 1/10 として導出される |

**cap は decode 後のバイトに掛かる。** `read_body_capped` の doc コメントが、圧縮応答では `content_length()` が `None` になり事前検査が無効化して chunk ループだけが生きること、その代償 (展開後が大きい正当なページを弾く) までを書く。

上限なしの読み出しは lint が禁じる。`clippy.toml` の `disallowed-methods` が `reqwest::Response::{text, bytes, json}` を deny し、各 `reason` に代替関数名 (`read_body_capped` はペイロード用、`read_body_snippet` は診断用) を書く。

## Provided — 環境変数

15 種。`--help` の `after_help` が既定値と許容範囲を全部載せ、`root_help_lists_scout_tuning_env_vars` (T-H010) が内容を pin する。検証とタイムアウト階層の決定は DR-0019。

| 分類             | 変数                                                                                                                                                                                        | 既定 / 範囲                                                                                                                                                                 |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 認証 (4)         | `BRAVE_SEARCH_API_KEY`、`GITHUB_TOKEN`、`GH_TOKEN`、`SLACK_TOKEN`                                                                                                                           | GitHub は `GITHUB_TOKEN` → `GH_TOKEN` → `gh auth token` の順に解決する (DR-0018)                                                                                            |
| チューニング (5) | `SCOUT_FETCH_TIMEOUT_SECS` (95)、`SCOUT_RESEARCH_TIMEOUT_SECS` (45)、`SCOUT_SLACK_TIMEOUT_SECS` (60)、`SCOUT_GITHUB_TIMEOUT_SECS` (180)、`SCOUT_MAX_RETRIES` (`retry::DEFAULT_MAX_RETRIES`) | タイムアウトは 1-600、リトライは 0-10                                                                                                                                       |
| egress (4)       | `HTTPS_PROXY`、`https_proxy`、`HTTP_PROXY`、`http_proxy`                                                                                                                                    | 設定されると名前解決由来の SSRF 防御を proxy の egress control へ委譲する (DR-0023)                                                                                         |
| テスト (1)       | `SCOUT_NETWORK_TESTS`                                                                                                                                                                       | CI が `"1"` を立て、loopback bind に失敗したテストを skip ではなく fail させる                                                                                              |
| ログ (1)         | `RUST_LOG`                                                                                                                                                                                  | src に literal を持たない (`EnvFilter::from_default_env` 経由)。`src/lib.rs` の `init_tracing` が `scout=info` を最後に足すので、`RUST_LOG` では scout 自身のログを消せない |

既定値の定数は `src/tools/config.rs` の `DEFAULT_*_SECS` にあり、許容範囲は同ファイルの `TIMEOUT_MIN_SECS`/`TIMEOUT_MAX_SECS`/`RETRIES_CAP` が持つ。

**`src/retry.rs` が持つ 2 つの上限は環境変数で動かせない。** `INITIAL_BACKOFF_MS` (1000) と `MAX_RETRY_AFTER_SECS` (300) はどちらもコンパイル時定数で、CLI からも env からも上書きできない。後者はサーバが返した `Retry-After` の上限で、超えた分は待たずに fail fast する。上の `## Provided — 本文の上限` が挙げる 3 つの cap と Slack 経路の 4 cap も同じく env では動かないが、この 2 つとは別の節が持つ。

**未設定と「設定されているが読めない」を分ける。** `read_env_raw` は `VarError::NotUnicode` を未設定に潰さず `UsageError` へ落とす。デフォルトへ黙って落ちないためで、その意図が doc コメントにある。env 読み取り自体は `from_env_with` が関数を取る形で注入可能になっており、その理由も doc コメントにある — `unsafe { std::env::set_var(...) }` が `unsafe_code = "forbid"` で使えないためである。

**タイムアウトの階層は散文ではなくテストが値として押さえている。** `T-CFG021` が `github_timeout` を `HTTP_TIMEOUT` と `CANDIDATE_FETCH_TIMEOUT` より大きいこと、`T-CFG026` が `research_timeout` を `REQUEST_TIMEOUT + FETCH_TIMEOUT` より大きいこと、`T-CFG025` (`js-rendering` 時のみ) が `fetch_timeout` を `CDP_TIMEOUT` より大きいことを assert する。**内側の定数を縮める変更を外側から落とす仕掛けである。** 階層そのものの決定は DR-0019。

既定から上書きされた値は `surface_overrides` が INFO で 1 フィールドずつ出す。差分の無いフィールドは 1 行も出さず、その無音経路を `T-CFG-LOG002` が押さえる (`code-quality-assessment.md` の `### E-1`)。

## Consumed — 外部 API

**scout が外部へ出す要求は全部 GET である。** 書き込み系のエンドポイントを 1 本も使わない。

| 相手                     | 定義箇所                                                      | エンドポイント / メソッド                                                                                                                                                       |
| ------------------------ | ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Brave Web Search API     | `src/brave/client.rs` の `API_BASE`                           | `https://api.search.brave.com/res/v1/web/search` の 1 本                                                                                                                        |
| GitHub REST API v3       | `src/github.rs` の `API_BASE` (`https://api.github.com`)      | 8 本 (下表)                                                                                                                                                                     |
| Slack Web API            | `src/slack/client.rs` の `API_BASE` (`https://slack.com/api`) | `conversations.info`、`users.info`、`conversations.replies`、`conversations.history` の 4 メソッド                                                                              |
| Chrome DevTools Protocol | `src/fetch/cdp.rs`、`src/fetch/cdp/launch.rs`                 | `js-rendering` feature 時のみ。`Fetch.enable` / `Fetch.requestPaused` / `Page.navigate`                                                                                         |
| 任意の HTTP(S) ページ    | `src/fetch/download.rs`                                       | `fetch` と `research` の取得先。ユーザー入力 URL なので SSRF 防御の対象                                                                                                         |
| `gh` CLI サブプロセス    | `src/token_source.rs` の `spawn_gh`                           | `gh auth token`。`TOKEN_RESOLVE_TIMEOUT` (5 秒) でタイムアウト。stderr はトークンを含みうるので破棄し終了コードだけ報告する (T-TOK004 が「stderr がログへ出ないこと」を assert) |

### GitHub REST エンドポイント 8 本

`src/github.rs` の `API_BASE` に対する相対パス。全て GET。振る舞い上限は DR-0004、出力スキーマと README のバイト上限は DR-0016 が定める。

| パス                                                                               | 用途                             |
| ---------------------------------------------------------------------------------- | -------------------------------- |
| `/repos/{owner}/{repo}`                                                            | リポジトリメタ                   |
| `/repos/{owner}/{repo}/git/trees/{ref_}?recursive=1`                               | `repo-tree` のツリー取得         |
| `/repos/{owner}/{repo}/contents/{encoded}{query}`                                  | `repo-read` のファイル取得       |
| `/repos/{owner}/{repo}/git/blobs/{sha}`                                            | Contents が返さない大きさの blob |
| `/repos/{owner}/{repo}/readme`                                                     | `repo-overview` の README        |
| `/repos/{owner}/{repo}/issues?state=open&sort=updated&direction=desc&per_page={n}` | `repo-overview` の issue         |
| `/repos/{owner}/{repo}/pulls?...`                                                  | `repo-overview` の PR            |
| `/repos/{owner}/{repo}/releases?per_page={n}`                                      | `repo-overview` の release       |

issue 応答から PR を除くフィルタは `src/github/types.rs` の `real_issues` にある。GitHub API は `/issues` に PR も混ぜて返すためである。ワイヤ型は同ファイルに 13 個 (struct 11 + enum 2: `EntryType`、`ContentsPayload`)、欠損配列の受けは `null_as_empty_vec`。

### Slack Web API メソッド 4 本

いずれも GET。user token の prefix 検証を構築時に行う判断が DR-0022 にある。`users.info` の解決は `futures` の `stream::buffer_unordered` で並列化され、mention 置換は `src/slack/mention.rs` が担う。

### トークン解決の順序

`src/token_source.rs` の `GhCliSource` が `GITHUB_TOKEN` → `GH_TOKEN` → `gh auth token` の順に試す。**解決した値は `Redacted` にくるまれて返る** — trait のシグネチャ自体が `Option<Redacted>` を返す形なので、生の `String` が呼び出し側へ出ることがない。テストが subprocess を回避できるよう、`TokenSource` は object-safe な trait として定義され `Arc<dyn TokenSource>` で注入される (DR-0008)。

## 契約を pin しているもの

契約は散文ではなくテストと lint で固定されている。壊したときにどこが落ちるかの対応は以下のとおり。

| 契約                                            | 落ちる場所                                                                                                              |
| ----------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| 終了コードの値と対応                            | `tests/exit_code_contract.rs` (271 行)                                                                                  |
| CLI の入出力全般                                | `tests/cli_integration.rs` (447 行)                                                                                     |
| エージェント向け出力の注入防御                  | `tests/output_injection.rs` (784 行)。決定は DR-0014                                                                    |
| `--help` 本文が終了コードと環境変数を載せること | `root_help_contains_exit_codes_and_environment` (T-H000)、`root_help_lists_scout_tuning_env_vars` (T-H010)              |
| 上限なし body 読み出しの禁止                    | `clippy.toml` の `disallowed-methods` (テストではなく lint。理由はファイル冒頭のコメント)                               |
| 秘密が出力経路へ届かないこと                    | 型で落ちる。`Redacted` が `Display` と `Serialize` を持たないので、`{}` や serde へ渡すとコンパイルが通らない (DR-0015) |

各 DR の Confirmation 節が、その決定を守っているテスト ID を名指しする。DR-0012 なら `T-F072` と Spec `T-201-1`/`T-201-4`。テスト ID の規約は `src/test_support.rs` の crate doc にある (`code-structure.md` の `## コードパターンと規約` 参照)。

**YAML 層の pin を実物で確認した。** `project.md` の DR-0014 の mandate が名指しする `src/yaml.rs` の 9 本 (`T-FC003`〜`T-FC007`、`T-FC030`〜`T-FC033`) は、inline `#[cfg(test)] mod tests` に 9 本すべて実在する。同ブロックのテストは全 15 本で、mandate が挙げない 6 本 (`T-FC012`、`T-FC013`、`T-FC100`〜`T-FC103`) が cap と escape の順序を押さえる。**`T-FC013` は「ADR-0014 が述べていない 2 点」を意図的に pin していると doc コメントに書く** — ESC などの C0 制御文字は escape 対象に入らないので値が borrow されたまま出ること、および出力された scalar が YAML 1.2 の c-printable から外れるバイトを運ぶので厳格なパーサが拒否すること。ADR を後から変えるとき、この 2 つの挙動が黙って変わらないための仕掛けである。
