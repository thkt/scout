# Developer Code Scan — scout

**Collaborator:** aidlc-developer-agent

対象リポジトリ: `/Users/thkt/GitHub/cli/scout` (repo qualifier なし)
基準 commit: `c8460b5`/package version `2.6.0` (`Cargo.toml`)
測定日: 2026-08-29

## Developer Code Scan Results

### Scan Coverage

- **Analyzed deeply**:
  - `Cargo.toml` — 依存宣言、feature、`[lints.rust]`/`[lints.clippy]`、`[profile.release]` を全行
  - `Cargo.lock` — `[[package]] name = "scout"` の直接依存 26 件と、それぞれの解決済み version
  - `clippy.toml`/`deny.toml`/`renovate.json`/`.config/nextest.toml`/`.gitignore` — 全行
  - `.github/workflows/ci.yml` — 全行 (3 job、27 step)
  - `src/main.rs`/`src/lib.rs` — 全行 (CLI 定義、`after_help` の終了コード表、signal drive、envelope 分岐)
  - `src/tools.rs`/`src/tools/query.rs`/`src/tools/config.rs`/`src/tools/repo.rs`/`src/tools/errors.rs` — 全行。6 サブコマンドのハンドラ層
  - `src/tools/builder.rs` — 1-300 行 (303 行中)。`ScoutBuilder` の注入 seam と `build_default_clients`
  - `src/fetch.rs` 1-310/`src/fetch/ssrf.rs` 1-300/`src/fetch/download.rs` 全行/`src/fetch/cdp.rs` 全行 — fetch パイプラインと SSRF 多層防御
  - `src/github.rs` 全行/`src/github/helpers.rs` 全行/`src/github/encoding.rs` 全行/`src/github/errors.rs` 全行
  - `src/slack.rs` 全行/`src/slack/client.rs` 1-400 (620 行中)/`src/slack/url.rs` 全行/`src/slack/mention.rs` 全行/`src/slack/format.rs` の実装 130 行
  - `src/brave/client.rs` 全行/`src/search/engine.rs` の実装全行/`src/search/lang.rs` 全行
  - `src/classify.rs`/`src/retry.rs`/`src/body_limit.rs`/`src/redacted.rs`/`src/charset.rs`/`src/signals.rs`/`src/search.rs`/`src/brave.rs` — 全行
  - `src/envelope.rs` 1-180 (261 行中) — `DegradedReason`、`Degradation`、`CommandOutput`、`ErrorCode`
  - `docs/decisions/README.md` (DR 索引 28 件) と DR 本文 8 本: 0001, 0002, 0003, 0010 は先頭から Decision Outcome まで、0011, 0012, 0014, 0023 は Decision Outcome 節
  - `docs/audit/2026-08-11-rust-code-assessment.md` — A 節から G 節まで
  - `.claude/rules/CONVENTIONS.md`/`.claude/rules/CORRECTIONS.md` — 全行

- **Skimmed only**:
  - 実装ファイルのうち部分読み: `src/fetch/converter.rs` (関数シグネチャ一覧 + `to_fetch_result`/`format_with_frontmatter`/`pre_handler` 周辺約 130 行、実装 985 行中)、`src/markdown.rs` 1-200 (671 行中)、`src/yaml.rs` 1-120 (403 行中)、`src/fetch/extractor.rs` 1-120 (433 行中)、`src/github/format.rs` 1-140 (284 行中)、`src/tools/params.rs` 1-200 (540 行中)、`src/token_source.rs` 1-110 (223 行中)、`src/test_support.rs` 1-90 (900 行中)、`src/tools/typo.rs` 1-60、`src/brave/types.rs` 1-80、`src/fetch/cdp/proxy.rs` 1-90 (182 行中)、`src/fetch.rs` 310-462、`src/fetch/ssrf.rs` 300-370
  - 一度も開いていない実装ファイル 4 本: `src/fetch/cdp/launch.rs` (289 行)、`src/fetch/cdp/proxy/transport.rs` (114 行)、`src/github/types.rs` (251 行)、`src/tools/test_helpers.rs` (60 行)。役割は呼び出し側の `use` と doc コメントから確定したが、本文は未読
  - テスト専用ファイル 45 本 (11,538 行) と、実装ファイル 26 本に inline で載る `mod tests`: module doc とテスト ID の分布は読んだが、個々の assertion は未読
  - `tests/` の 4 ファイル (1,833 行): module doc と先頭 50-60 行のみ
  - `.github/workflows/release.yml` (job/step 名とマトリクスのみ)、`.github/workflows/zizmor.yml`、`.github/workflows/label-from-issue.yml`、`.github/ISSUE_TEMPLATE/`、`.github/zizmor.yml`、`.github/advanced-issue-labeler.yml`
  - `README.md` 先頭 120 行、`README.ja.md` は未読
  - DR 本文 20 本 (0004-0009, 0013, 0015-0022, 0024-0028) はタイトルと status のみ、`docs/audit/` の他 13 ファイルは未読

- **アプリケーションソース外として除外した領域** (省略ではなく、明示的な除外):
  - `.claude/` — `.gitignore` の 5 行目で除外されるハーネス領域。現時点では AI-DLC フレームワーク本体 (skills、agents、tools、hooks、sensors、knowledge) が入る。scout の実行バイナリには 1 行も入らない
  - `aidlc/` — AI-DLC のワークスペース (memory/codekb/intents/audit)。同じくアプリケーションソースではない
  - `docs/*` — `.gitignore` が `docs/*` を除外し `!docs/decisions/` と `!docs/audit/` だけを戻す。`git ls-files docs/` で確認すると追跡されているのは `docs/audit/` 14 ファイルと `docs/decisions/` 29 ファイル (DR 28 本 + README) だけで、ignore 規則より前に commit された取りこぼしは無い。両方ともこのスキャンの一次ソースとして扱った
  - `target/`、`workspace/`、`.codegraph/`、`.yomu/`、`.playwright-cli/`、`.venv/`、`*.profraw` — ビルド成果物とローカルツールのキャッシュ
  - これらを除いたアプリケーションソース面は `src/` (95 ファイル)、`tests/` (4 ファイル)、リポジトリ直下の設定 6 ファイル、`.github/`、`docs/decisions/`、`docs/audit/` である

- **`kind: full` を名乗れるかについての所見**: モジュールの網羅という意味では全域を覆っている。6 サブコマンド、4 バックエンド、fetch パイプライン、注入 seam、エラー分類、ビルド/CI/lint/依存ポリシーはいずれも一次ソースを読んで確定した。一方で「リポジトリ全体を deep に読んだ」とは言えない。実装 50 ファイルのうち 46 本を開き、うち 30 本は全行、16 本は実装部分の一部にとどまる。テスト 11,538 行 + inline `mod tests` は分布のみで中身を読んでいない。**したがって Scope of Analysis ブロックは `kind: partial` を立ててほしい。** `kind: full` は `analyzed.paths` に `./` を要求し、次回以降のどの intent もこのリポジトリを「検証済みの全域カバー」として読む。上に挙げた 4 本の未読ファイルとテスト 11,538 行がその主張の裏に入ることになる。pre-scan snapshot の `["./"]` は検証済みカバレッジの上限を定めるものなので、その内側に収まる `src/` 単位の列挙は snapshot 違反にならない。

`analyzed.paths` にはこの走査が deep に読んだ次を推す。

```
src/lib.rs
src/main.rs
src/classify.rs
src/envelope.rs
src/retry.rs
src/body_limit.rs
src/redacted.rs
src/charset.rs
src/signals.rs
src/search/
src/brave/
src/slack/
src/tools/
src/fetch/
src/github/
Cargo.toml
clippy.toml
deny.toml
renovate.json
.config/nextest.toml
.github/workflows/ci.yml
docs/decisions/
docs/audit/
```

`shallow.paths` にはテスト専用ファイル 45 本、`tests/`、DR 本文 20 本、`.github/workflows/` の残り 3 本を置く。

ディレクトリ単位の記載について 1 点補足する。`src/fetch/` と `src/github/` と `src/tools/` は配下に未読ファイルを含む (`fetch/cdp/launch.rs`、`fetch/cdp/proxy/transport.rs`、`github/types.rs`、`tools/test_helpers.rs`)。`src/search/`・`src/brave/`・`src/slack/` にはその漏れがない。ディレクトリ表記を採るなら、この 4 本を `shallow.paths` にも重ねて明記してほしい。粒度を揃えたいなら、上記 3 ディレクトリだけファイル単位へ落とす形でもよい。どちらを採ったかをブロックに書き残すこと。

### Packages Found

Cargo workspace ではなく **単一 crate** である。`Cargo.toml` に `[workspace]` セクションは無く、`Cargo.lock` の `[[package]] name = "scout"` は 1 件。したがって「パッケージ一覧」は 1 行で尽きる。実質的な構成の単位は crate 内のモジュールと、1 つの feature フラグにある。

| package | type                                                                     | language                                | purpose                                                                                                                                                    |
| ------- | ------------------------------------------------------------------------ | --------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `scout` | binary + library crate (`src/main.rs` は `scout::run()` を呼ぶ 6 行のみ) | Rust edition 2024 / rust-version 1.97.1 | Web 検索 (Brave)、ページ取得と Markdown 化、GitHub リポジトリ探索、Slack permalink 取得を 1 本の CLI に束ねる。AI エージェントを一次コンシューマに想定する |

**構成の実質的な軸は `js-rendering` feature** である。既定では無効で、有効にすると `chromiumoxide`/`nix`/`tempfile` の 3 crate が入り、headless Chromium による JS レンダリング経路 (`src/fetch/cdp*`) がコンパイルされる。この feature は次の 4 箇所に同時に現れる。

- `Cargo.toml` の `[features] js-rendering = ["chromiumoxide", "nix", "tempfile"]`
- `#[cfg_attr(not(feature = "js-rendering"), allow(...))]` が 3 ファイルに 6 箇所。`src/fetch/cdp.rs` に 3、`src/fetch/cdp/launch.rs` に 2 (いずれも `allow(dead_code)`)、`src/fetch/cdp/proxy.rs:36` に 1 (`allow(unused_imports)`)
- `#[ignore = "requires chromium"]` が付く 1 テスト (`src/fetch/cdp/cdp_integration_tests.rs:74`)
- CI の独立した 2 step (`cargo check --features js-rendering`、`cargo nextest run --features js-rendering --run-ignored all`)

crate 内のモジュール構成 (`src/lib.rs` の `mod` 宣言 20 本、および各モジュール直下のサブモジュール):

| module     | 責務                                                                                                                                      | 主なファイル                                                                                                                                                            |
| ---------- | ----------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tools`    | CLI ハンドラ層。`Command` enum のディスパッチ、stdin フォールバック、`Scout` の DI、`ScoutError`                                          | `tools.rs`, `params.rs`, `query.rs`, `repo.rs`, `builder.rs`, `config.rs`, `errors.rs`, `typo.rs`                                                                       |
| `fetch`    | URL 検証 → DNS 事前検査 → ダウンロード → redirect ごとの再検査 → 本文抽出 → Markdown 変換                                                 | `fetch.rs`, `ssrf.rs`, `download.rs`, `extractor.rs`, `converter.rs`, `cdp*`                                                                                            |
| `github`   | GitHub REST v3 クライアント、パス / ref 検証、base64 とエンコーディング復号、Markdown 整形                                                | `github.rs`, `helpers.rs`, `encoding.rs`, `errors.rs`, `format.rs`, `types.rs`                                                                                          |
| `slack`    | Slack Web API クライアント、permalink 解析、mention 置換、YAML frontmatter 出力                                                           | `slack.rs`, `client.rs`, `url.rs`, `mention.rs`, `format.rs`                                                                                                            |
| `brave`    | Brave Search API クライアントと応答型                                                                                                     | `brave/client.rs`, `brave/types.rs`                                                                                                                                     |
| `search`   | `research` の並列取得オーケストレーションとレポート整形                                                                                   | `search/engine.rs`, `search/lang.rs`                                                                                                                                    |
| 横断リーフ | エラー分類、終了コード / JSON envelope、リトライ、本文上限、Markdown / YAML 中和、秘密の型封じ込め、時計 / 乱数 / トークン / DNS の注入点 | `classify.rs`, `envelope.rs`, `retry.rs`, `body_limit.rs`, `markdown.rs`, `yaml.rs`, `redacted.rs`, `clock.rs`, `rng.rs`, `token_source.rs`, `charset.rs`, `signals.rs` |

### Build System

- **Type**: Cargo (Rust edition 2024、`rust-version = "1.97.1"`)
- **Config Files**: `Cargo.toml`、`Cargo.lock`、`clippy.toml`、`deny.toml`、`renovate.json`、`.config/nextest.toml`、`.github/workflows/ci.yml`、`.github/workflows/release.yml`
- **Build Dependencies**: crate 間の依存関係は 1 crate なので存在しない。crate 内のモジュール依存は `use crate::…` から抽出した。参照数の上位は `envelope` 18、`fetch` 12、`slack` 9、`clock` 9、`rng` 8、`retry` 8、`brave::client` 8、`token_source` 7、`github` 7。依存の向きは `main` → `lib` → `tools` → 各バックエンド (`fetch`/`github`/`slack`/`brave`/`search`) → 横断リーフ (`envelope`/`classify`/`retry`/`body_limit`/`markdown`/`yaml`/`redacted`/`clock`/`rng`) の一方向で、循環は見当たらない。`body_limit.rs` の module doc が「2 つ以上のバックエンドが共有する上限だけをここに置く」という配置規則を明文で持ち、バックエンド固有の上限 (`MAX_GITHUB_RESPONSE_BYTES`、`MAX_RESPONSE_BYTES`) は各バックエンド側に残している
- **リリースビルド**: `[profile.release]` は `opt-level = 3`、`lto = true`、`codegen-units = 1`、`strip = true`
- **配布**: `release.yml` が 4 ターゲット (`x86_64-apple-darwin`、`aarch64-apple-darwin`、`x86_64-unknown-linux-gnu`、`aarch64-unknown-linux-gnu`) をクロスビルドし、GitHub Release への添付と Homebrew tap (`thkt/homebrew-tap`) の Formula 更新まで行う
- **依存の自動更新**: `renovate.json` が `thkt/renovate-config` を継承し、加えて 3 つの規則を持つ。(1) `Cargo.toml` の `rust-version` を regex custom manager で MSRV として追跡、(2) `htmd` と `markup5ever_rcdom` を 1 つの PR にまとめる、(3) `markup5ever_rcdom` を `allowedVersions: "<0.39"` で `htmd 0.5.5` が要求する 0.38 系に固定する。(2) と (3) が両方必要な理由 ("group 規則だけでは片方の crate 単独 PR が開いてコンパイルできない") が `description` に書かれている

### APIs Discovered

**Provided (scout が外部へ差し出す契約)** — crate としての公開面は `pub async fn run() -> ExitCode` の 1 つだけである。`Cargo.toml` の `unreachable_pub = "deny"` がこれを機械的に固定しており、実際コード中の可視性はほぼすべて `pub(crate)`/`pub(super)`/`pub(in crate::slack)` である。つまり外部契約は Rust API ではなく **CLI の表面** にある。

| API type             | location                                                                                      | 内容                                                                                                                                                                                                                                                                                                                                                                           |
| -------------------- | --------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| CLI サブコマンド (6) | `src/tools/params.rs` の `enum Command`                                                       | `search`、`fetch`、`research`、`repo-tree`、`repo-read`、`repo-overview`                                                                                                                                                                                                                                                                                                       |
| グローバルフラグ     | `src/lib.rs` の `struct Cli`                                                                  | `--json` (1 行 JSON envelope 出力)、`--version`、`--help`                                                                                                                                                                                                                                                                                                                      |
| 終了コード表 (10 値) | `src/envelope.rs` の `ErrorCode::exit_code`、`src/signals.rs` の `InterruptSignal::exit_code` | 0 / 64 / 65 / 66 / 70 / 74 / 75 / 104 / 124 / 130 / 143。sysexits.h + GNU coreutils + POSIX 128+signo の 3 系統を統合 (DR-0002, DR-0017)                                                                                                                                                                                                                                       |
| JSON envelope        | `src/envelope.rs` の `SuccessEnvelope` / `ErrorEnvelope`                                      | 成功側は `data` / `degraded` / `notes` / `degraded_reasons` (空なら省略)。失敗側は `error.{code, message, next_step, candidates, retryable}` (DR-0010)                                                                                                                                                                                                                         |
| 劣化通知 (14 値)     | `src/envelope.rs` の `enum DegradedReason`                                                    | `SCREAMING_SNAKE_CASE` で JSON に出る。部分失敗を文字列パースなしに検出させる (DR-0003)                                                                                                                                                                                                                                                                                        |
| 環境変数             | `src/tools/config.rs`、`src/brave/client.rs`、`src/slack/client.rs`、`src/token_source.rs`    | 認証: `BRAVE_SEARCH_API_KEY`、`GITHUB_TOKEN` / `GH_TOKEN` / `gh auth token`、`SLACK_TOKEN`。チューニング: `SCOUT_FETCH_TIMEOUT_SECS` (95, 1-600)、`SCOUT_RESEARCH_TIMEOUT_SECS` (45)、`SCOUT_SLACK_TIMEOUT_SECS` (60)、`SCOUT_GITHUB_TIMEOUT_SECS` (180)、`SCOUT_MAX_RETRIES` (2, 0-10)。egress: `HTTPS_PROXY` / `https_proxy` / `HTTP_PROXY` / `http_proxy`。ログ: `RUST_LOG` |
| `--help` 本文        | `src/lib.rs` の `after_help`                                                                  | 終了コード表、環境変数、チューニング範囲を全部載せ、`root_help_contains_exit_codes_and_environment` と `root_help_lists_scout_tuning_env_vars` の 2 テストが内容を pin する。`--version` 実行時に `AGENT_HELP_HINT` を stderr へ出し、コーディングエージェントを `--help` へ誘導する                                                                                           |

**Consumed (scout が外部へ出す要求)**:

| 相手                     | location                                                      | エンドポイント / メソッド                                                                                                                                                                                                                                                                                                                     |
| ------------------------ | ------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Brave Web Search API     | `src/brave/client.rs` の `API_BASE`                           | `https://api.search.brave.com/res/v1/web/search` の 1 本。パラメータは `q` と任意の `search_lang` のみで、`count` / `offset` / `safesearch` は既定に任せる旨がコメントにある                                                                                                                                                                  |
| GitHub REST API v3       | `src/github.rs` の `API_BASE` = `https://api.github.com`      | 8 本: `/repos/{o}/{r}`、`/repos/{o}/{r}/git/trees/{ref}?recursive=1`、`/repos/{o}/{r}/contents/{path}`、`/repos/{o}/{r}/git/blobs/{sha}`、`/repos/{o}/{r}/readme`、`/repos/{o}/{r}/issues`、`/repos/{o}/{r}/pulls`、`/repos/{o}/{r}/releases`。全部 GET。ヘッダは `Accept: application/vnd.github+json` と `X-GitHub-Api-Version: 2022-11-28` |
| Slack Web API            | `src/slack/client.rs` の `API_BASE` = `https://slack.com/api` | 4 メソッド: `conversations.info`、`conversations.replies`、`conversations.history`、`users.info`。全部 GET                                                                                                                                                                                                                                    |
| Chrome DevTools Protocol | `src/fetch/cdp.rs`                                            | `js-rendering` feature 時のみ。ローカル chromium を pgroup で起動し WebSocket 経由で `Fetch.enable` / `Fetch.requestPaused` / `Page.navigate` を使う                                                                                                                                                                                          |
| 任意の HTTP(S) ページ    | `src/fetch/download.rs`                                       | `fetch` / `research` が取りに行く先。ユーザー入力 URL なので SSRF 防御の対象                                                                                                                                                                                                                                                                  |
| `gh` CLI サブプロセス    | `src/token_source.rs` の `spawn_gh`                           | `gh auth token`。5 秒でタイムアウト。stderr はトークンを含みうるので破棄し、終了コードだけを報告する                                                                                                                                                                                                                                          |

### Frameworks & Libraries

`Cargo.toml` は範囲 (`"4"`、`"0.13"` など) で宣言する。下表の version は `Cargo.lock` が実際に解決した値である。`Cargo.lock` は作業ツリーで modified 状態 (推移的依存 36 件のパッチ更新差分) だったので、読んだのは作業ツリー側の内容である。

| name                             | version           | purpose                                                                                                                                                                           |
| -------------------------------- | ----------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `clap`                           | 4.6.6             | CLI パーサ (derive)。`Cli` / `Command` / 各 `*Params` を生成し、`--help` の `after_help` も持つ                                                                                   |
| `reqwest`                        | 0.13.4            | HTTP クライアント。features は `json`, `gzip`, `brotli`, `deflate`, `zstd`。`ClientBuilder::dns_resolver` が SSRF connect 時 guard の注入点になる                                 |
| `tokio`                          | 1.53.1            | 非同期ランタイム。features を `full` にせず 10 個を列挙し、「リストにない feature を使う call site を足したらコンパイルが落ちる、それが欲しい信号だ」とコメントで理由を書いている |
| `dom_smoothie`                   | 0.18.0            | Readability 実装。本文抽出                                                                                                                                                        |
| `htmd`                           | 0.5.5             | HTML → Markdown 変換。`pre` / `span` / `table` / `a` と抑制タグ 7 種にカスタムハンドラを登録する                                                                                  |
| `markup5ever_rcdom`              | 0.38.0+unofficial | htmd が再エクスポートしない `NodeData` を読むため。htmd と同じ crate に解決させる目的で pin                                                                                       |
| `serde` / `serde_json`           | 1.0.229 / 1.0.151 | JSON envelope と API 応答のシリアライズ / デシリアライズ                                                                                                                          |
| `thiserror`                      | 2.0.20            | 6 つのエラー enum (`FetchError`, `GitHubError`, `SlackError`, `BraveError`, `BrowserError`, `CdpInterceptError`) の `Display` 導出                                                |
| `url`                            | 2.5.8             | URL 解析。`ValidatedUrl` の内部型                                                                                                                                                 |
| `futures`                        | 0.3.34            | `stream::buffer_unordered`。`research` の並列取得 (上限 5) と Slack `users.info` の並列解決 (上限 5)                                                                              |
| `base64`                         | 0.23.1            | GitHub Contents / Blob API の本文復号。lock には transitive な 0.22.1 も並ぶ                                                                                                      |
| `globset`                        | 0.4.20            | `repo-tree --pattern` のグロブ照合                                                                                                                                                |
| `percent-encoding`               | 2.3.2             | GitHub API のパスセグメントのエンコード                                                                                                                                           |
| `tracing` / `tracing-subscriber` | 0.1.44 / 0.3.23   | 構造化ログ。stderr 固定、`scout=info` を最後に pin するので `RUST_LOG` で消せない                                                                                                 |
| `encoding_rs`                    | 0.8.35            | ラベル指定 / BOM 由来のデコード                                                                                                                                                   |
| `chardetng`                      | 1.0.0             | 文字コード自動判定。多バイト 8 種のみ信頼する gate 付き (DR-0013)                                                                                                                 |
| `fastrand`                       | 2.5.0             | バックオフのジッタ。`Rng` trait の本番実装                                                                                                                                        |
| `httpdate`                       | 1.0.3             | `Retry-After` の HTTP-date 形式の解釈 (RFC 9110 §10.2.4)                                                                                                                          |
| `chromiumoxide`                  | 0.9.1             | CDP クライアント。`js-rendering` 時のみ                                                                                                                                           |
| `nix`                            | 0.31.3            | プロセスグループへのシグナル送出。`js-rendering` 時のみ                                                                                                                           |
| `tempfile`                       | 3.27.0            | chromium の `--user-data-dir`。`js-rendering` 時のみ                                                                                                                              |
| `wiremock` (dev)                 | 0.6.5             | HTTP モックサーバ。3 バックエンドのクライアントテストが使う                                                                                                                       |
| `tracing-test` (dev)             | 0.2.6             | ログ出力の assertion (`logs_contain`)                                                                                                                                             |
| `flate2` (dev)                   | 1.1.9             | 圧縮応答のテスト fixture 生成                                                                                                                                                     |
| `tokio` (dev, `test-util`)       | 1.53.1            | `start_paused` による仮想時間。タイムアウトとバックオフのテストが実時間を待たない                                                                                                 |

依存総数は `Cargo.lock` の `name =` 行で 311 件 (推移含む)。同名で version が割れている crate は 11 件 (`base64`, `core-foundation`, `cpufeatures`, `getrandom`, `html5ever`, `markup5ever`, `r-efi`, `rand_core`, `rand`, `syn`, `windows-sys`)。`deny.toml` は `multiple-versions = "warn"` なのでこれは警告どまりで、`html5ever`/`markup5ever` の重複については `Cargo.toml` に「揃えても警告が 2 つ消えるだけでコードは 2 セットのまま (issue #379 で実測)」という不追跡の判断が書かれている。

### Test Coverage

- **Test Directories**: `tests/` (統合テスト 4 ファイル/1,833 行)、および `src/` 配下のテスト専用ファイル 45 本 (11,538 行) と実装ファイル 26 本に載る inline `mod tests`
- **Test Frameworks**: Rust 標準の `#[test]`/`#[tokio::test]`。ランナーは `cargo-nextest` (`.config/nextest.toml` に `default` と `ci` の 2 プロファイル)。HTTP モックは `wiremock`、ログ assertion は `tracing-test`、時間の制御は `tokio` の `test-util` (`start_paused`)
- **Coverage Config**: 存在する。`cargo-llvm-cov` が lcov を出し、`diff-cover` が `--fail-under=95` で **差分カバレッジ** を課す (絶対値ではない)。PR イベントでのみ走る。除外は `*/fetch/cdp/proxy/transport.rs` の 1 本だけで、除外理由 (accept EMFILE、10 秒の dial ブラックホール、途中リセットという実ソケット障害でしか通らないエラーアーム) が yml のコメントにある。SOCKS5 プロトコル層 (`proxy.rs`) は除外せずゲートに乗せている
- **テスト数**: `#[test]`/`#[tokio::test]` の属性行を `src` と `tests` で数えて 852。これは属性の出現数であって実行結果ではない。参考として `docs/audit/2026-08-11-rust-code-assessment.md` は v2.5.0 時点で `cargo nextest run --all-features` の実行結果を 854 passed/1 skipped と記録している
- **テスト ID**: `[T-<PREFIX><NNN>]` 形式が 806 個 (`grep -o` の後 `sort -u` を通した重複除去後の数。前回監査時点は 774)。規約そのものが `src/test_support.rs` の crate doc に書かれており、prefix は「テスト対象」を指すので 1 prefix が複数ファイルに跨ること、番号は prefix 内で一意でファイル単位ではないこと、引用時はブラケットを外して定義と区別することまで決めてある。重複採番はリポジトリ自身のテスト (`test_support::scan_test_id_violations`) が検出する
- **`#[ignore]`**: 1 本のみ (`src/fetch/cdp/cdp_integration_tests.rs:74`、`#[ignore = "requires chromium"]`)。CI は `--run-ignored all` で必ず走らせるので、chromium 不在のランナーは skip ではなく fail する
- **ネットワークテストの skip 防止**: CI が `env: SCOUT_NETWORK_TESTS: "1"` を立てる。loopback bind に失敗したテストはローカルでは skip するが CI では panic する。理由 ("nextest は成功テストの stderr を隠すので、skip したまま緑になる事故が起きる") が yml のコメントに書かれている
- **統合テストの分担**: `cli_integration.rs` (プロセス起動レベルの `--help`/`--version`/終了コード)、`exit_code_contract.rs` (モック proxy 経由で HTTP status → 終了コード → JSON `error.code` を端から端まで pin。proxy への接続カウンタが 1 以上であることも assert し、SSRF 事前チェックや DNS 失敗で偶然同じコードが出た偽陽性を排除する)、`output_injection.rs` (`neutralize_yaml_markers_outside_fences` を `scout fetch` 越しに pin)

### Code Quality Indicators

- **Linting**:
  - `Cargo.toml` の `[lints.clippy]` に **deny 13 個** (absolute_paths, cast_possible_truncation, cast_precision_loss, redundant_closure_for_method_calls, filter_map_next, flat_map_option, manual_filter_map, manual_find_map, wildcard_imports, enum_glob_use, str_to_string, needless_pass_by_value, disallowed_methods)。`[lints.rust]` は別セクションで `unsafe_code = "forbid"` と `unreachable_pub = "deny"` の 2 個
  - `clippy.toml` の `disallowed-methods` が `reqwest::Response::{text, bytes, json}` を禁じ、`reason` に代替関数名 (`body_limit::read_body_capped`/`read_body_snippet`) を書く。理由がコメントに残っている: 上限を守っていることが出力から検証できない (診断文を後段でさらに切るので 64KiB 読んでも 20MB 読んでも出力が同じ) ので、テストではなく lint で守る判断
  - lint 抑制は **15 個**。パターンは `#!?\[(cfg_attr\(.*)?(allow|expect)\(`。内訳は `#[expect(...)]` 8、`#[cfg_attr(not(feature = "js-rendering"), allow(...))]` 6、`#![allow(dead_code)]` 1 (`tests/common/mod.rs:17`)。前回監査 (2026-08-17、v2.5.0) は同じパターンで 19 個で、差分は `src/fetch/converter.rs` にあった `#[allow(clippy::needless_pass_by_value)]` 5 箇所が `#[expect(...)]` 1 箇所に減っていること。`#[allow]` から `#[expect]` への移行は、外部 crate (htmd) が署名を変えて制約が消えた日に警告が出る側へ寄せる変更である
- **CI/CD**: `.github/workflows/` に 4 本。`ci.yml` が push (main) と PR で走り、3 job・27 step (test 12、coverage 6、security 9):
  - `test` job: `cargo check` × 2 (通常/`--features js-rendering`)、`cargo nextest run --profile ci` × 2 (通常/`--features js-rendering --run-ignored all`)、`cargo clippy --all-targets -- -D warnings` × 2 (通常/`--all-features`)、`cargo fmt -- --check`、そして Comment language check
  - `coverage` job (PR のみ): `cargo llvm-cov --features js-rendering --lcov -- --include-ignored` → `diff-cover --fail-under=95`
  - `security` job: `cargo deny check`、`cargo audit`、`cargo machete --with-metadata` (`--with-metadata` を付ける理由が step のコメントにある: package 名と lib 名が違う crate を未使用と誤報するため)
  - 全 action が SHA pin、`persist-credentials: false`、`zizmor.yml` が workflow 自体を lint する
- **依存ポリシー**: `deny.toml` がライセンス許可リスト (OSI permissive + MPL-2.0 + CDLA-Permissive-2.0 + BSL-1.0 + Unicode-3.0) を明示し、strong copyleft は列挙しないことで拒否する。`unknown-registry = "deny"`、`unknown-git = "deny"`、`allow-registry` は crates.io のみ
- **Documentation**:
  - `README.md`/`README.ja.md` の 2 言語。README は「問題 → scout なし → scout あり」の順で、使うべき場面と使うべきでない場面を並べる構成
  - Decision Record **28 本、全て `status: "accepted"`**。`docs/decisions/README.md` が番号/タイトル/status/日付の索引を持つ。MADR v4 形式で Context/Decision Drivers/Considered Options/Decision Outcome/Consequences/Confirmation を揃え、**Confirmation 節が決定を pin しているテスト ID を名指しする**
  - 実装コードから DR/ADR への参照が **143 箇所** (`src/` に対して `grep -o 'ADR-[0-9]{4}|DR-[0-9]{4}'`)。`tests/` を足すと 158
  - doc コメントの密度が高い。定数には「なぜこの値か」、`match` のアームには「なぜこの順序か」、`#[expect]` には `reason` が付く。特徴的なのは **却下した選択肢と、その却下を測った数値がコメントに残る** こと。例: `Cargo.toml` の `markup5ever_rcdom` pin、`src/tools/config.rs` の `DEFAULT_GITHUB_TIMEOUT_SECS` (180 秒がリトライ総予算 ~279 秒を下回るよう選んだ根拠とトレードオフ)、`src/retry.rs` の 300 秒上限 (issue #185 の実測)
  - コメント言語は英語で統一。`.claude/rules/CONVENTIONS.md` が規約を持ち、CI の Comment language check が違反行を落とす。判定は各行から引用断片を外してから日本語文字クラスを当てるので、例外 (Shift_JIS バイト列の注釈など) が増えても落ちない
- **可視性**: `unreachable_pub = "deny"` により到達できない `pub` は 0。前回監査の記録では最終分布が `pub(crate)` 241/`pub(super)` 134/private 21 で、一括置換ではなくコンパイラに最小可視性を答えさせる手順 (監査文書 B-6) で決めた
- **本番経路の panic**: `unsafe_code = "forbid"`。前回監査は本番経路の `unwrap()`/`expect()` を 2 箇所 (`lib.rs` の静的 directive、`envelope.rs` の infallible Serialize) と記録し、両方に理由コメントがある

### Technical Debt Signals

一般的な負債マーカーはほぼ検出されない。以下は測った結果である。

- **TODO / FIXME / HACK / XXX: 実質 0 件**。`grep -rnE '(TODO|FIXME|HACK|XXX)' src tests --include='*.rs'` は 5 hit を返すが、5 件とも負債マーカーではない。`src/markdown.rs:220` は doc コメントが「`# TODO` のようなコメント行」を例示している箇所、残り 4 件は Slack テストの fixture ユーザー ID `UXXX` (`slack/format/resolve_messages_tests.rs:59,64`、`slack/mention/mention_tests.rs:111,112`)
- **ハードコードされた資格情報: 検出なし**。秘密は `src/redacted.rs` の `Redacted` 型に封じ込められ、`Debug` は `[REDACTED]` を出し、空白のみの値は構築時に `None` になる。テスト用の literal (`"test-key"`、`"xoxp-test"`) は `#[cfg(test)]` の中にある
- **未使用依存**: CI の `cargo machete --with-metadata` が毎回検査する。ローカルでは未実行

以下は文書化された、現時点で未着手の判断である。

- **`src/fetch/converter.rs` が 3,131 行** — このリポジトリ自身の分割規約から外れる唯一の実装ファイル。内訳は実装 985 行 (1-985) と `#[cfg(test)] mod tests` 2,146 行 (986-3,131)。実装ファイルの中で 2 番目に大きい `src/test_support.rs` (900 行) の 3.5 倍。`.claude/rules/CONVENTIONS.md` は「1 ファイルのテストが 2 つ以上の関心を持ったら分ける」と定め、この 1 本はテスト ID の並びが表/pre とフェンス/リンクとアンカー/script と style の抑制/frontmatter/リストの 6 群に分かれる。監査文書 E-4 が 2026-08-17 に 3,177 行として計上し、判断は未着手のまま。増分の出所は issue #373 (fast_html2md → htmd の置き換え) の回帰テスト
- **`with_clock` / `with_rng` の 4 重複** — `github.rs`/`brave/client.rs`/`slack/client.rs`/`tools/builder.rs` に同形のメソッドが 8 個ある。共通化 (ClientCommon 化、DRY-02) は実測のうえ棄却済みで、再検討の着手条件は「新 DR の起草」。**その着手条件が closed issue #310 の Backlog candidates の中にしか無く、引き継ぐ open issue が存在しない** (監査文書 E-2)。条件付きの判断が閉じた issue に閉じ込められている状態
- **`tools/config.rs` の `surface_overrides` の 5 連 if** — フィールド名が `tracing` の構造化ログのキーなので、ループに畳むとキーが文字列になり静的性が失われる。現状維持の判断で、監査文書 E-1 が「フィールドが 8-10 個に増えたら宣言的マクロを検討する境界」と再検討の閾値を数値で残している
- **`slack/client.rs` の `api_get_once` の二重パース** — `serde_json::Value` へ 1 回、`from_value` で目的型へもう 1 回。`ok: false` を目的型の deserialize より先に判定するために必要な形。`#[serde(flatten)]` で畳むとエラー分類が `Api` から `Decode` へ変わる。現状維持 (監査文書 E-3)
- **CDP 経路の SSRF 非対称** — DR-0012 が明示的に carve-out として記録している。reqwest 経路は connect 時 IP guard で DNS rebind を塞ぐが、chromium は自分で名前解決するので同じ guard が効かない。代替として `src/fetch/cdp/proxy.rs` の loopback SOCKS5 proxy が CONNECT 先を解決して private IP を弾く。DR-0012 は「rebind 穴は残る、issue #201 で追跡」と書き、`proxy.rs` の module doc は「これは方式 Y' を proxy 層へ移設したもの」と書く。文書間で追跡先の状態が食い違う可能性があるので、#201 の現状は未確認
- **Proxied egress での防御委譲** — DR-0023 の決定により、proxy env が設定された経路では名前解決由来の SSRF 防御 (rebind を含む) を proxy の egress control へ委譲する。literal な private/loopback の拒否は全経路で維持される。DR-0023 は Consequences に「proxy の egress policy が緩いと DNS rebind が通りうる」と Bad として明記し、OUTCOME Constraint への carve-out 記載を求めている
- **リポジトリ直下に 78 個の `*.profraw`** — カバレッジ計測の残骸。`.gitignore` の `*.profraw` で除外されているので commit はされないが、作業ツリーには残っている
- **`Cargo.lock` が modified 状態** — 推移的依存 36 件のパッチ版更新差分が未 commit。ブランチは `chore/aidlc-v2-install`

## Handoff Summary

- **Intent-relevant finding**: **このリポジトリは既に、CodeKB が作ろうとしている情報を自前で持っている。** 具体的には、28 本の accepted Decision Record (MADR v4、Confirmation 節がそれを守るテスト ID を名指しする)、実装コードから DR への 143 箇所の参照、806 個の `[T-XXX]` テスト ID による決定とテストの双方向 pin、そして 2026-08-11/17 に実装 50 ファイルを 1 本ずつ読んで書かれた測定付き監査文書 (`docs/audit/2026-08-11-rust-code-assessment.md`) である。したがって 9 本の CodeKB アーティファクトの仕事は、これらを **索引して指すこと** であって、内容を書き写すことではない。書き写すと同じ知識の 2 つ目のコピーができ、`.claude/rules/CONVENTIONS.md` が「一次ソースを写した時点で 2 箇所が食い違う」と明示的に禁じている状態に入る。CONVENTIONS.md 自身がこの方針の実例で、規約の索引だけを持ち本体は一次ソース (`src/test_support.rs` の crate doc、`Cargo.toml` の lints、`clippy.toml` の reason、`docs/decisions/`) に置いている。

- **Risks / follow-up**:
  1. **Scope of Analysis の `kind` 判断**: 上の `### Scan Coverage` に書いたとおり、この走査はモジュールを網羅したが全行を読んではいない (実装 50 本中 46 本を開き、30 本が全行、テスト 11,538 行は分布のみ)。`kind: full` + `./` を立てると次回以降の intent がこのリポジトリを検証済み全域として扱う。**`kind: partial` を立ててほしい。** `analyzed.paths` / `shallow.paths` に入れる具体的なパス一覧と、ディレクトリ単位で書く場合に重ねて明記すべき未読 4 本を `### Scan Coverage` の末尾に置いた。
  2. **未読の実装ファイル 4 本**: `src/fetch/cdp/launch.rs` (289 行、chromium の起動と pgroup 管理、`check_browser_request`)、`src/fetch/cdp/proxy/transport.rs` (114 行、カバレッジゲートから唯一除外されている)、`src/github/types.rs` (251 行、GitHub API のワイヤ型)、`src/tools/test_helpers.rs` (60 行)。component-inventory を書く際、この 4 本の記述は呼び出し側と doc コメントからの推定であって本文の確認ではない。
  3. **`src/fetch/converter.rs` の 3,131 行** (監査文書 E-4、未着手)。このリポジトリ自身の分割規約から外れる唯一の実装ファイルで、切り出す単位の判断が保留されている。code-quality-assessment に載せる価値がある。
  4. **`with_clock` / `with_rng` の再検討条件が closed issue #310 にしかない** (監査文書 E-2)。条件付きの判断が閉じた issue に閉じ込められており、監査文書自身が「再検討するなら本文書か新 issue へ移す必要がある」と書いている。CodeKB に載せるとその移送先になりうる。
  5. **前回監査からの実測差分**: lint 抑制 19 → 15 (`converter.rs` の `#[allow]` 5 → `#[expect]` 1)、テスト ID 774 → 806、DR 27 → 28、`converter.rs` 3,177 → 3,131 行。監査文書は v2.5.0/commit `c0499fd` 基準、この走査は v2.6.0/commit `c8460b5` 基準。監査文書を引用する際はこの基準差を保持してほしい。
  6. **数え方の明記**: `Cargo.toml` の deny は `[lints.clippy]` 13 個と `[lints.rust]` 2 個で、合算すると 15 になる。テスト数 852 は属性行の出現数であって実行結果ではない。lint 抑制 15 は `cfg_attr` 経由と inner attribute を含むパターンで数えたもので、`#[allow` だけの grep は 6 件を取りこぼす。CodeKB へ転記する際はこの測定範囲ごと運んでほしい。
