# Technology Stack — scout

## 言語とツールチェーン

| 項目            | 値                                           | 所在                                                          |
| --------------- | -------------------------------------------- | ------------------------------------------------------------- |
| 言語            | Rust、edition 2024                           | `Cargo.toml` の `[package]`                                   |
| MSRV            | `rust-version = "1.97.1"`                    | 同上                                                          |
| ライセンス      | MIT                                          | 同上                                                          |
| package version | 2.6.0                                        | 同上                                                          |
| ビルドシステム  | Cargo (単一 crate、workspace ではない)       | `Cargo.toml` に `[workspace]` セクション無し                  |
| テストランナー  | `cargo-nextest`                              | `.config/nextest.toml` に `default` と `ci` の 2 プロファイル |
| カバレッジ      | `cargo llvm-cov` + `diff-cover`              | `.github/workflows/ci.yml` の coverage job                    |
| 依存ポリシー    | `cargo deny`、`cargo audit`、`cargo machete` | `deny.toml`、`.github/workflows/ci.yml` の security job       |

リリースプロファイルは `[profile.release]` に `opt-level = 3`、`lto = true`、`codegen-units = 1`、`strip = true`。

MSRV は renovate が自動追跡する。追跡の仕組みと、その datasource が crates.io ではなく Docker Hub である理由は `dependencies.md` の `## 依存の自動更新` が持つ。

**`.config/nextest.toml` の `ci` プロファイルは `retries` を設定していない。** `final-status-level = "flaky"` は現状 1 度も発火しない。根拠は `code-quality-assessment.md` の `### retries は 0 で、final-status-level = "flaky" は現状発火しない` が持つ。

## feature フラグ

**構成の実質的な軸は `js-rendering` である。既定では無効。** `Cargo.toml` の `[features]` にある `js-rendering = ["chromiumoxide", "nix", "tempfile"]` が optional 依存 3 件を有効化し、headless Chromium による JS レンダリング経路 (`src/fetch/cdp*`) をコンパイルする。

この feature は 4 箇所に同時に現れ、どれか 1 つを直すときは残りも見る必要がある。**参照は行番号ではなくシンボル名で指す (DR-0028)** — 行番号はコードが動いた時点で別の宣言を指し、ずれたことが読者に見えない。

| 現れる場所                                                                   | 内容                                                                                                                                                                                                                                                                                                  |
| ---------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml` の `[features]` と `optional = true` を付けた依存 3 件          | `chromiumoxide`、`nix`、`tempfile`                                                                                                                                                                                                                                                                    |
| 3 ファイル 6 箇所の `#[cfg_attr(not(feature = "js-rendering"), allow(...))]` | `allow(dead_code)` が `src/fetch/cdp.rs` の `mod proxy` 宣言・`enum BrowserError`・`impl From<BrowserError> for FetchError` と、`src/fetch/cdp/launch.rs` の `check_browser_request`・`resolve_browser_binary_from`。`allow(unused_imports)` が `src/fetch/cdp/proxy.rs` の `spawn_ssrf_proxy` 再輸出 |
| `#[ignore = "requires chromium"]` が付く 1 テスト                            | `src/fetch/cdp/cdp_integration_tests.rs` の `t005_t006_cdp_renders_and_removes_profile_dir`                                                                                                                                                                                                           |
| CI の独立した 2 step                                                         | `cargo check --features js-rendering`、`cargo nextest run --features js-rendering --run-ignored all --profile ci`                                                                                                                                                                                     |

## ライブラリ

`Cargo.toml` は範囲 (`"4"`、`"0.13"` など) で宣言する。**下表の version は `git show HEAD:Cargo.lock` が実際に解決した値である。** 作業ツリーの `Cargo.lock` は未コミットの変更を持つが、**動いているのは推移的依存 14 件だけで、下表の直接依存は 1 件も違わない** (`dependencies.md` の `## 作業ツリーと HEAD の差`)。

依存の件数・ライセンス方針・version 分裂は `dependencies.md` が持つ。ここは「何をどの目的で使っているか」だけを持つ。

### 本番依存

| name                 | version           | purpose                                                                                                                                           |
| -------------------- | ----------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- |
| `clap`               | 4.6.6             | CLI パーサ (derive)。`Cli` / `Command` / 各 `*Params` を生成し、`--help` の `after_help` を持つ                                                   |
| `reqwest`            | 0.13.4            | HTTP クライアント。features は `json`, `gzip`, `brotli`, `deflate`, `zstd`。`ClientBuilder::dns_resolver` が SSRF connect 時 guard の注入点になる |
| `tokio`              | 1.53.1            | 非同期ランタイム。features を `full` にせず列挙する                                                                                               |
| `dom_smoothie`       | 0.18.0            | Readability 実装。本文抽出                                                                                                                        |
| `htmd`               | 0.5.5             | HTML から Markdown への変換                                                                                                                       |
| `markup5ever_rcdom`  | 0.38.0+unofficial | htmd が再エクスポートしない `NodeData` を読むため。htmd と同じ crate に解決させる目的で pin                                                       |
| `serde`              | 1.0.229           | JSON envelope と API 応答のシリアライズ / デシリアライズ                                                                                          |
| `serde_json`         | 1.0.151           | 同上                                                                                                                                              |
| `thiserror`          | 2.0.20            | 各バックエンドのエラー enum の `Display` 導出                                                                                                     |
| `url`                | 2.5.8             | URL 解析。`ValidatedUrl` の内部型                                                                                                                 |
| `futures`            | 0.3.34            | `stream::buffer_unordered`。`research` の並列取得 (並列度 5) と Slack `users.info` の並列解決                                                     |
| `base64`             | 0.23.1            | GitHub Contents / Blob API の本文復号                                                                                                             |
| `globset`            | 0.4.20            | `repo-tree --pattern` のグロブ照合                                                                                                                |
| `percent-encoding`   | 2.3.2             | GitHub API のパスセグメントのエンコード                                                                                                           |
| `tracing`            | 0.1.44            | 構造化ログ。stderr 固定                                                                                                                           |
| `tracing-subscriber` | 0.3.23            | `EnvFilter` に `scout=info` を最後に足すので `RUST_LOG` で消せない (`src/lib.rs` の `init_tracing`)                                               |
| `encoding_rs`        | 0.8.35            | ラベル指定 / BOM 由来のデコード。`src/charset.rs` が信頼できる検出とみなす 8 エンコーディングの判定にも使う                                       |
| `chardetng`          | 1.0.0             | 文字コード自動判定 (DR-0013)                                                                                                                      |
| `fastrand`           | 2.5.0             | バックオフのジッタ。`src/rng.rs` の `Rng` trait の本番実装                                                                                        |
| `httpdate`           | 1.0.3             | `src/retry.rs` が読む `Retry-After` の HTTP-date 形式                                                                                             |

### optional 依存 (`js-rendering` 時のみ)

| name            | version | purpose                                                    |
| --------------- | ------- | ---------------------------------------------------------- |
| `chromiumoxide` | 0.9.1   | CDP クライアント                                           |
| `nix`           | 0.31.3  | プロセスグループへのシグナル送出 (`features = ["signal"]`) |
| `tempfile`      | 3.27.0  | chromium の `--user-data-dir`                              |

### dev 依存

| name                  | version | purpose                                                                           |
| --------------------- | ------- | --------------------------------------------------------------------------------- |
| `wiremock`            | 0.6.5   | HTTP モックサーバ                                                                 |
| `tracing-test`        | 0.2.6   | ログ出力の assertion (`logs_contain`)。「ログに出ていない」の assertion にも使う  |
| `flate2`              | 1.1.9   | 圧縮応答のテスト fixture 生成                                                     |
| `tokio` (`test-util`) | 1.53.1  | `start_paused` による仮想時間。タイムアウトとバックオフのテストが実時間を待たない |

### 横断リーフが直接使う crate

**12 の横断リーフを全行読んだうえで、各ファイルが直接触る外部 crate を対応させた。** 上の表と重複するが、リーフを 1 本触るときにどの crate の挙動へ踏み込むかが分かる形にしてある。

| ファイル              | 実装部が直接使う crate         | 何のために                                                                          |
| --------------------- | ------------------------------ | ------------------------------------------------------------------------------------- |
| `src/retry.rs`        | `reqwest`、`httpdate`、`tokio`、`tracing` | `is_connect`/`is_timeout`/`is_decode` 判定、`Retry-After` の 2 形式、`sleep`     |
| `src/envelope.rs`     | `serde`、`serde_json`          | envelope の serde 属性と `to_json_line`                                              |
| `src/signals.rs`      | `tokio`、`tracing`             | signal 受信と `watch` による graceful drain                                          |
| `src/token_source.rs` | `tokio`、`tracing`             | `gh auth token` の subprocess と `timeout`                                           |
| `src/body_limit.rs`   | `reqwest`                      | `Response::chunk()` による cap 付き読み出し                                          |
| `src/classify.rs`     | `reqwest`                      | `reqwest::Error` から `Classification` への写像                                      |
| `src/charset.rs`      | `encoding_rs`                  | ラベルと BOM からの `Encoding` 解決。**自動判定の `chardetng` はここには無い** — 呼ぶのは `src/fetch/download.rs`、`src/github/encoding.rs`、`src/github/helpers.rs`、`src/tools/params.rs` の側である |
| `src/rng.rs`          | `fastrand`                     | backoff のジッタ                                                                     |
| `src/clock.rs`        | なし (`std::time` のみ)        | **壁時計の抽象であって sleep の抽象ではない。** `now_secs()` 1 メソッドで、仮想時間を進めるのは `tokio` の `start_paused` 側の役目 |
| `src/redacted.rs`     | なし                           | `std::fmt::Debug` のみ。`Display`/`Serialize` を意図的に持たない                     |
| `src/markdown.rs`     | なし                           | 文字列処理のみ                                                                       |
| `src/yaml.rs`         | なし                           | 文字列処理のみ                                                                       |

測定範囲は各ファイルの実装部 (inline `#[cfg(test)] mod tests` ブロックを除いた範囲) で、`<crate>::` の形のパス参照を数えている。

## 技術選択の判断が残っている場所

選定そのものと却下した代替は Decision Record にある。ここは対応だけを持つ。

| 選択                                                                           | DR               |
| ------------------------------------------------------------------------------ | ---------------- |
| 検索バックエンドを Gemini Grounding から Brave Search API へ切り替えた         | 0005             |
| Brave クライアントの初期化を factory と `Result` ベースの https 検査へ統一した | 0007             |
| テスト seam を `Arc<dyn Trait>` フィールドと `ScoutBuilder` で作った           | 0008, 0009       |
| 文字コードの判定とデコード方針                                                 | 0013             |
| `<pre>` / `<br>` と改行の扱い (htmd の出力に対する上書き)                      | 0025, 0026, 0027 |

`markup5ever_rcdom` の pin は上の表に無い — 選定ではなく htmd への追随であり、判断の記録は `Cargo.toml` のコメントと `renovate.json` の側にある。**この 2 つは相互に相手を名指ししており、値も整合している。** renovate 側の 4 規則と、解除手順が `Cargo.toml` 側にしか無いことは `dependencies.md` の `## 依存の自動更新` が持つ。
