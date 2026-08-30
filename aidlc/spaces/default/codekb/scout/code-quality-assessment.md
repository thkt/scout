# Code Quality Assessment — scout

**すべての数値は測定範囲とセットで読むこと。** 同じ対象でもパターンを変えると値が変わるものが複数あり、範囲を落として引用すると別の数になる。この節の数値は commit `ef2fbc9`/version 2.6.0/測定日 2026-08-30 のものである (`reverse-engineering-timestamp.md` の `## 測定基準`)。

## テスト

| 指標               | 値             | 測定範囲                                                                                                                                                                                                                                                 |
| ------------------ | -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| テスト属性の宣言数 | 851            | `src/` + `tests/`、行頭アンカー付きの属性行。内訳は `#[test]` 646、`#[tokio::test]` 191、`#[tokio::test(start_paused = true)]` 13、`#[tokio::test(flavor = "multi_thread")]` 1。**実行結果ではない**                                                     |
| テスト ID          | 806 (重複なし) | 同じ範囲。`src/test_support.rs` の `extract_bracketed_test_ids` と同じ規則 — `[T-` の後に ASCII 英数字とハイフンが 1 文字以上続き `]` で閉じるもの。より狭い `[T-[A-Z]+[0-9]+]` では 778 に落ち、`T-ER001a` / `T-GH011a` のような接尾辞付き 6 個を落とす |
| `#[ignore]`        | 1              | `src/fetch/cdp/cdp_integration_tests.rs` の `t005_t006_cdp_renders_and_removes_profile_dir` に付く `#[ignore = "requires chromium"]`                                                                                                                     |
| 重複採番           | 0              | 出現数も 806 で `sort -u` 後と一致する。リポジトリ自身のテスト `test_support::scan_test_id_violations` も検出する                                                                                                                                        |

**この 3 つは先行ストアの値と一致する。** `src/` が 1 バイトも動いていないので、測り方が正しかった数値は持ち越せる。

テストファイルの配置 (テスト専用 47 本/inline `mod tests` 19 箇所/`tests/` 4 本) と、その 3 つの数を混同しないための注意は `code-structure.md` の `## ファイル分類` が持つ。

ファイル単位で測った内訳が 2 本ある。

| ファイル                 | テスト属性 | 測定範囲と注意                                                                                                                                                                                                   |
| ------------------------ | ---------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/fetch/converter.rs` | 79         | inline `mod tests` ブロック 2,146 行。すべて `#[test]` で `#[tokio::test]` は 0。テスト ID も 79 個で重複なし                                                                                                    |
| `src/tools/config.rs`    | 20         | inline `mod tests` ブロック 282 行。行頭アンカー付きの `^\s*#\[test\]$` で測定。テスト ID も 20 個で重複なし。うち 3 本が `#[tracing_test::traced_test]`、1 本が `#[cfg(feature = "js-rendering")]` (`T-CFG025`) |

`src/tools/config.rs` の `traced_test` を素の `grep 'traced_test'` で数えると 4 hit になるが、1 件は属性ではなく doc コメントが `traced_test` に言及している行である。**属性を数えるなら行頭アンカーを付けること。**

**skip をゼロに寄せる 2 つの仕掛けがある。**

- `#[ignore]` は 1 本だけで、CI は `--run-ignored all` で必ず走らせる。chromium 不在のランナーは skip ではなく fail する
- CI が job 外の `env:` で `SCOUT_NETWORK_TESTS: "1"` を立てる。loopback bind に失敗したテストはローカルでは skip するが CI では失敗する。理由が `ci.yml` のコメントにある —「nextest は成功テストの stderr を隠すので、skip したまま緑になる」。外部前提ごとの skip 方針は DR-0024

時間の制御は `tokio` の `test-util` (`start_paused`) で行い、タイムアウトとバックオフのテストが実時間を待たない。HTTP モックは `wiremock`、ログの assertion は `tracing-test` の `logs_contain`。

### `retries` は 0 で、`final-status-level = "flaky"` は現状発火しない

**確定した。** `.config/nextest.toml` の `[profile.ci]` は `fail-fast = false`、`slow-timeout`、`final-status-level = "flaky"` を持つが、**再試行そのものは 1 度も起きない。**

| 確認したもの                                           | 結果                                                                                                                                                           |
| ------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `.config/nextest.toml` 全 10 行                        | `[profile.default]` にも `[profile.ci]` にも `retries` の行は無い                                                                                              |
| `.github/workflows/ci.yml` の `cargo nextest run` 2 本 | どちらも `--retries` を渡さない。`--profile ci` のみ                                                                                                           |
| `NEXTEST_RETRIES` と `.cargo/`                         | どちらも存在しない                                                                                                                                             |
| `cargo-nextest` バイナリに埋め込まれた既定設定         | `retries = 0` と、その直下の `flaky-result = "pass"`。同じブロックの冒頭に `# This is the default config used by nextest. It is embedded in the binary` とある |

したがってどのテストも再試行されず、flaky 行は出ない。**この設定は「今は効かないが、`retries` を入れた日に効く」ものとして読むのが正しい。**

**残る限界が 1 つある。** 既定値を読んだのはローカルの `cargo-nextest 0.9.143` である。CI は `taiki-e/install-action` で入れるのでバージョンが違いうる。ただし `.config/nextest.toml` と CI の両方に上書きが無いという事実はバージョンに依らない。

## カバレッジゲート

**絶対値ではなく差分カバレッジを課す。** PR イベントでのみ走る。

```
cargo llvm-cov --features js-rendering --lcov --output-path lcov.info -- --include-ignored
diff-cover lcov.info --compare-branch=origin/main \
  --exclude '*/fetch/cdp/proxy/transport.rs' --fail-under=95
```

`-- --include-ignored` は DR-0024 の Decision Outcome が名指しで要求するフラグである。これを欠くと `#[ignore]` 付きの経路が `lcov.info` から静かに消え、判定対象から外れる。

除外は `src/fetch/cdp/proxy/transport.rs` の 1 本だけである。**除外理由が 2 箇所に書かれている** — `ci.yml` のコメントと `transport.rs` の module doc の両方に、accept の EMFILE/ENFILE/ECONNABORTED、10 秒の dial ブラックホール、途中リセットという実ソケット障害でしか通らないエラーアームだと記録される。SOCKS5 プロトコル層 (`src/fetch/cdp/proxy.rs`) は除外せずゲートに乗せる。

**`main` へ直接 push した変更はこのゲートを受けない。** `coverage` job だけが `if: github.event_name == 'pull_request'` を持つ。

## Lint と静的検査

**clippy の deny 数は 2 つのセクションに分かれる。合算した数を書くなら、その旨を添えること。**

| セクション                       | 数  | 内容                                                                                                                                                                                                                                                                                          |
| -------------------------------- | --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml` の `[lints.clippy]` | 13  | `absolute_paths`、`cast_possible_truncation`、`cast_precision_loss`、`redundant_closure_for_method_calls`、`filter_map_next`、`flat_map_option`、`manual_filter_map`、`manual_find_map`、`wildcard_imports`、`enum_glob_use`、`str_to_string`、`needless_pass_by_value`、`disallowed_methods` |
| `Cargo.toml` の `[lints.rust]`   | 2   | `unsafe_code = "forbid"`、`unreachable_pub = "deny"`                                                                                                                                                                                                                                          |

`clippy.toml` の `disallowed-methods` が `reqwest::Response::{text, bytes, json}` を禁じ、各 `reason` に代替関数名 (`body_limit::read_body_capped`/`read_body_snippet`) を書く。テストではなく lint で守る判断の理由がファイル冒頭のコメントにある —「どちらの caller も cap を観測可能にしない (診断文を後段でさらに切るので、`text()` を戻しても全テストが通る)。だからビルドを落とす」。

**この deny リストに構造を検査するものは 1 つも無い。** モジュール間の依存の向き、循環の有無、層をまたぐ import はいずれも対象外である (下の `## 層の向きに検査点が無い`)。

**lint 抑制は 15 個。この数は `cfg_attr` 経由と inner attribute を含むパターンでの値である。**

測定パターン: `grep -rnE '#!?\[(cfg_attr\(.*)?(allow|expect)\(' src tests --include='*.rs'`

| 形                                                       | 数                                                       |
| -------------------------------------------------------- | -------------------------------------------------------- |
| `#[expect(...)]`                                         | 8                                                        |
| `#[cfg_attr(not(feature = "js-rendering"), allow(...))]` | 6                                                        |
| `#![allow(dead_code)]` (inner attribute)                 | 1 (`tests/common/mod.rs` の module 冒頭 inner attribute) |

**`#[allow` だけを探す素朴な grep は 0 件を返す。** allow 系 7 個は 6 個が `cfg_attr` 経由、1 個が inner attribute なので、1 つも拾わない。6 件の `cfg_attr` allow は `src/fetch/cdp.rs` に 3、`src/fetch/cdp/launch.rs` に 2、`src/fetch/cdp/proxy.rs` に 1 で、いずれも `js-rendering` 無効時の dead_code/unused_imports を黙らせるためのものである。`#[expect(...)]` には必ず `reason` が付く。

**本番経路の panic**: `unsafe_code = "forbid"` に加え、`expect` は 2 箇所で理由コメント付きであることを確認した (`code-structure.md` の `## コードパターンと規約` 参照)。

## CI/CD

`.github/workflows/` に 4 本。`ci.yml` は push (main) と PR で走り、**3 job・27 step** (test 12、coverage 6、security 9)。

| job        | timeout | 実行するもの                                                                                                                                                                                                                                                               |
| ---------- | ------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `test`     | 15 分   | `cargo check` × 2 (通常 / `--features js-rendering`)、`cargo nextest run --profile ci` × 2 (通常 / `--features js-rendering --run-ignored all`)、`cargo clippy --all-targets -- -D warnings` × 2 (通常 / `--all-features`)、`cargo fmt -- --check`、Comment language check |
| `coverage` | 20 分   | PR のみ。`fetch-depth: 0` で checkout → `cargo llvm-cov` → `diff-cover --fail-under=95`                                                                                                                                                                                    |
| `security` | 10 分   | `cargo deny check`、`cargo audit`、`cargo machete --with-metadata`                                                                                                                                                                                                         |

`cargo machete` に `--with-metadata` を付ける理由が step コメントにある — package 名と lib 名が違う crate を未使用と誤報するため。

**supply chain 側の締め方が徹底している。** 全 action が SHA pin、`ci.yml` の 3 job が `persist-credentials: false` と最小 `permissions: contents: read` を持ち、`concurrency` で同一 ref の実行をキャンセルする。

### Comment language check

コメントを英語で統一する規約を CI が機械的に落とす。判定式は各行から引用断片 (`"..."` と 1 文字の `'.'`) を外してから日本語文字クラスを当てる形で、**件数の固定値を使わない** ため、Shift_JIS バイト列の注釈のような例外が増えても落ちない。

### zizmor の finding は CI job を落とさない

**確定した。** `.github/workflows/zizmor.yml` は `main` への push と PR で `zizmorcore/zizmor-action` を SHA pin で走らせるが、**検出した finding が job の終了コードを非ゼロにすることは無い。** 5 段の連鎖を 1 つずつ確認した。

| 段  | 確認したもの                                      | 結果                                                                                                                                                                         |
| --- | ------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | `.github/workflows/zizmor.yml` の action 呼び出し | `with:` ブロックを持たない = 全 input が既定値                                                                                                                               |
| 2   | 同 SHA の `action.yml`                            | `advanced-security` は `required: false` / `default: "true"`                                                                                                                 |
| 3   | 同 SHA の `action.sh`                             | `advanced-security` が `"true"` のとき引数へ `--format=sarif` を足す                                                                                                         |
| 4   | `zizmorcore/zizmor` の `docs/usage.md`            | 「If you use `--format=sarif`, `zizmor` will **not** use exit codes 11 and above.」exit code 表の 0 行も「Successful audit; no findings to report (or SARIF mode enabled).」 |
| 5   | 同 `action.sh` の終了処理                         | zizmor の終了コードをそのまま返す                                                                                                                                            |

**強制点は SARIF のアップロード先である。** `action.yml` の 2 番目の step が `github/codeql-action/upload-sarif` を走らせ、`category: zizmor` で code scanning alert として登録する。リポジトリ内の裏付けもある — `.github/workflows/zizmor.yml` の job が `permissions: security-events: write` を持つ。job を赤くするだけなら要らない権限である。

**正確な言い方は「finding は job を落とさない」であって「zizmor は job を落とさない」ではない。** exit code 1 (audit 中のエラー) と 2 (引数解析の失敗) は SARIF モードでも抑制されないので、zizmor 自身が壊れれば job は落ちる。

**残る限界が 1 つある。** `docs/usage.md` は `zizmorcore/zizmor` の既定ブランチから取得したもので、action が入れる zizmor 本体は `version: latest` の既定に従う。この文書と同一バージョンである保証は無い。

`.github/zizmor.yml` は rule ignore を 3 件持つ — `artipacked` (`release.yml` の `update-homebrew` job の checkout が `HOMEBREW_TAP_TOKEN` を保持する必要があるため)、`cache-poisoning`、`superfluous-actions`。**3 件とも理由コメント付きである。**

## ドキュメント

このリポジトリの文書量と密度は、CodeKB が書き足すべきものが無いと言える水準にある。

| 種類                       | 量                                           | 特徴                                                                                                                                                                                                                                                                    |
| -------------------------- | -------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Decision Record            | 28 本、全て `status: "accepted"`             | MADR v4。Context / Decision Drivers / Considered Options / Decision Outcome / Consequences / Confirmation を揃え、**Confirmation 節が決定を pin しているテスト ID を名指しする** (DR-0012 なら `T-F072`、Spec `T-201-1` / `T-201-4`)。索引は `docs/decisions/README.md` |
| 実装コードから DR への参照 | 143 箇所 (`src` のみ)、`tests/` を足すと 158 | 測定は `grep -rhoE '(ADR\|DR)-[0-9]{4}'`。参照の形はシンボル名で指す (DR-0028)                                                                                                                                                                                          |
| README                     | `README.md` 358 行 / `README.ja.md` 347 行   | 2 言語。この CodeKB のどの走査でも未読                                                                                                                                                                                                                                  |
| 監査文書                   | `docs/audit/` に 14 本                       | 中心は `2026-08-11-rust-code-assessment.md`                                                                                                                                                                                                                             |
| doc コメント               | 密度が高い                                   | 定数には「なぜこの値か」、`match` のアームには「なぜこの順序か」。**却下した選択肢と、その却下を測った数値がコメントに残る**。実例は `code-structure.md` の `## コードパターンと規約`                                                                                   |

### DR-0012 は文書とコードのドリフトではない

DR-0012 のタイトルは `Connect-time IP Guard for SSRF DNS Rebinding, with CDP Path Asymmetry` で終わり、Consequences には CDP/chromium 経路に rebind 穴が残ると書く行がある。**この行を未解決の課題として扱うのは誤読である。**

- issue #201 は **CLOSED / stateReason COMPLETED / closedAt 2026-06-16** である
- DR-0012 は本文の後に `## Addendum: CDP 経路の carve-out 解消 (issue #201, 2026-06-16)` を持ち、loopback SOCKS5 proxy 方式で穴を塞いだこと、OUTCOME Constraint の carve-out 文言も置換済みであることを記録する
- コード側も一致する。`build_launch_args` の 3 フラグと `handle_conn` の fail-closed が実装にあたる (`architecture.md` の `## Interaction Diagrams` 参照)

**Consequences の箇条書きは MADR の慣行どおり決定時点の帰結を記録した行であり、Addendum がそれを解消する。** 箇条書きだけを読むと未解決に見えるが、二次的な要約が一次ソースの追記に追随しなかっただけである。この誤読はこの CodeKB の先行資料で実際に起きた。

## 層の向きに検査点が無い

**判断が要る所見である。決着した所見として扱わないこと。**

crate 内の依存グラフには本番の辺が 56 本あり、そのうち `yaml → search` の 1 辺が 2 本の循環を閉じている (`architecture.md` の `## モジュール依存の実形`)。**この辺自体は問題ではない。** 両側の doc コメントが派生の理由を数値付きで書いており、同じページ予算を上限を決める側と切る側が共有するという意図された設計である。

問題はその意図に検査点が無いことにある。

| 確認したもの                           | 結果                             |
| -------------------------------------- | -------------------------------- |
| `Cargo.toml` の `[lints.clippy]` 13 件 | 構造や依存の向きを見るものは無い |
| `Cargo.toml` の `[lints.rust]` 2 件    | 同上                             |
| `clippy.toml` の `disallowed-methods`  | メソッド呼び出しのみを対象とする |
| `.github/workflows/` の 4 本           | 循環検査や層検査の step は無い   |

**2 本目の同種の辺が入っても、機械は何も言わない。** 先行ストアが「循環は無い」と書けてしまったのは、まさにこの辺が誰の目にも見えていなかったためである。

**チームのメモリ層はこの事実を既に持っていた。** `aidlc/spaces/default/memory/team.md` の `## Code Style` は「「循環が無い」「横断リーフからバックエンドへの import も無い」という形の主張はここには書かない」と定め、その理由として `src/yaml.rs` が `MAX_PAGE_BYTES` を import することを名指ししている。**CodeKB 側だけがそれと矛盾する主張を 9 箇所に載せていた。**

この意図をどこで守るかは人が決めることである。候補は 2 つあり、どちらもこの CodeKB は推していない。

- **Decision Record にする** — 派生定数の共有を意図として記録し、Confirmation 節で pin するテストを名指しする。既存 28 本と同じ形になるが、DR は CI が検査しない (DR-0028 の参照形式にも検査 job が無いのと同じ状況)
- **lint か CI job にする** — 循環の不在、あるいは「例外は 1 辺だけ」を機械が落とす形にする。既存の deny リストは構造を見ないので、新しい仕組みを 1 つ足すことになる

## 技術的負債

**一般的な負債マーカーは実質存在しない。**

| 指標                       | 結果                                                                                                                                                                                                                                                                            |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| TODO / FIXME / HACK / XXX  | 実質 0 件。`grep -rnE '(TODO\|FIXME\|HACK\|XXX)' src tests --include='*.rs'` は 5 hit を返すが、5 件とも負債マーカーではない (`src/markdown.rs` の `shift_headings` の doc コメントが `# TODO` のようなコメント行を例示、残り 4 件は Slack テストの fixture ユーザー ID `UXXX`) |
| ハードコードされた資格情報 | 検出なし。秘密は `src/redacted.rs` の `Redacted` 型に封じ込められる。`spawn_gh` の失敗時 stderr がログへ出ないことを `T-TOK004` が assert する                                                                                                                                  |
| 未使用依存                 | CI の `cargo machete --with-metadata` が毎回検査する                                                                                                                                                                                                                            |

以下は **文書化されたうえで未着手の判断** であり、放置ではない。いずれも再検討の条件が数値または着手条件として残っている。**この走査は新しい負債を見つけていない。**

| 項目                                                                                                           | 現状                                                                                                                                                                     | 再検討の条件                                                                                                                                                                                    |
| -------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/fetch/converter.rs` の 3,131 行 (実装 985 + テスト 2,146)                                                 | このリポジトリ自身の分割規約から外れる唯一の実装ファイル。テストは 9 関心・26 連続区間に散っている。**切り出す単位の判断は依然として未着手だが、判断に要る材料は揃った** | 監査文書 E-4。判断の材料は下の `### E-4` が持つ                                                                                                                                                 |
| `with_clock` / `with_rng` の 4 重複 (`github.rs` / `brave/client.rs` / `slack/client.rs` / `tools/builder.rs`) | 共通化 (ClientCommon 化、DRY-02) は実測のうえ棄却済み                                                                                                                    | 着手条件は「新 DR の起草」。**その着手条件が closed issue #310 の Backlog candidates の中にしか無い** (監査文書 E-2)。#310 が CLOSED であることは確認したが、引き継ぐ open issue の有無は未確認 |
| `src/tools/config.rs` の `surface_overrides` の 5 連 if                                                        | **決着。現状維持で正しい** (下の `### E-1`)                                                                                                                              | 監査文書 E-1 の閾値「フィールドが 8-10 個」は 3 通りのどの数え方でも未達 (すべて 5)                                                                                                             |
| `src/slack/client.rs` の `api_get_once` の二重パース                                                           | **決着。現状維持で正しい。ただし棄却の根拠は監査文書の論拠より強いものがコードにある** (下の `### E-3`)                                                                  | 監査文書 E-3。畳めば失うのは `SlackError::Api` の 6 分岐と retry である                                                                                                                         |

### E-1 — `surface_overrides` の 5 連 if は閾値に届かない

**決着した。** 監査文書は「フィールドが 8-10 個に増えたら宣言的マクロを検討する」という再検討の閾値を数値で残す。「フィールド」がどの名詞を指すかで値が変わりうるので 3 通りとも測った。**3 つとも 5 で一致したため、閾値の判定に曖昧さは残らない。**

| 数える対象                           | 値  | 測定範囲                                                                              |
| ------------------------------------ | --- | ------------------------------------------------------------------------------------- |
| `surface_overrides` の `if` アーム   | 5   | `src/tools/config.rs` の `surface_overrides` 関数本体                                 |
| `RuntimeConfig` 構造体のフィールド   | 5   | `fetch_timeout`、`research_timeout`、`slack_timeout`、`github_timeout`、`max_retries` |
| このファイルが読む `SCOUT_*` env var | 5   | `ENV_*` 定数 5 本。すべて `from_env_with` から参照され、全部が surface される         |

**マクロで畳むコストは監査文書が見積もったより高い。** 監査文書が書いていない事実が 2 つあり、どちらもマクロ化を難しくする方向に働く。

1. **`info!` のキー名が構造体のフィールド名と一致しない。** `Duration` を `as_secs()` で `u64` に落とすため、5 個中 4 個に接尾辞 `_secs` が付く (`fetch_timeout` → `fetch_timeout_secs` など)。`max_retries` だけが一致する。構造体フィールドを機械的に走査するマクロやループでは、この 4 個のキー名を導出できない
2. **5 個の `if` は「ほぼ同形」であって同形ではない。** 4 個は `Duration` の `as_secs()` を `u64` の定数と比べるが、`max_retries` の 1 個だけは `u32` の比較で、比較相手の `DEFAULT_MAX_RETRIES` がこのファイルではなく `crate::retry` にある

**挙動は 3 本のテストが pin する。** いずれも `tracing_test::traced_test` と `logs_contain` の組み合わせで、`T-CFG-LOG001` (上書きされたフィールドだけが INFO を出し構造化値を運ぶ)、`T-CFG-LOG002` (全フィールドがデフォルトなら 1 件も出ない)、`T-CFG-LOG003` (`github_timeout` でも同じ形が成り立つ) の 3 本である。**`T-CFG-LOG002` が効いている** — ループやマクロへ畳んだときに「差分なしでも出す」退化が起きたら、無音経路を押さえているこのテストが落とす。

### E-3 — `api_get_once` の棄却理由は監査文書より強いものがコードにある

**決着した。現状維持は正しい。ただし監査文書が挙げた論拠をそのまま引かないこと。**

二重パースの実体は `src/slack/client.rs` の `api_get_once` にある。本文を `serde_json::Value` へ 1 回、`ok` フィールドを見てから目的型 `T` へもう 1 回パースする。どちらの失敗も `SlackError::Decode` に落ちる。本文の上限は `src/body_limit.rs` の `MAX_API_RESPONSE_BYTES` (1 MiB) で、超過は `read_body_capped` が `Decode` に落とす。

**畳んだときに失われるのはエラーのラベルではなく 6 分岐である。** `src/slack.rs` の `classify()` で、`SlackError::Api` は error 文字列によって 6 つの `ErrorCode` へ分かれる。

| 分岐先 `ErrorCode` | 代表的な error 文字列                                             |
| ------------------ | ----------------------------------------------------------------- |
| `UsageError`       | `invalid_auth`、`missing_scope`、`not_authed` ほか計 14 文字列    |
| `DataError`        | `invalid_arguments`                                               |
| `NotFound`         | `channel_not_found`、`message_not_found`、`thread_not_found` ほか |
| `TempFailure`      | `internal_error`、`service_unavailable`、`invalid_cursor` ほか    |
| `Internal`         | `invalid_arg_name`、`deprecated_endpoint`、`method_deprecated`    |
| `Unknown`          | 表にない文字列 (ADR-0011 の retreat slot)                         |

対して `SlackError::Decode` は `classify()` で `ErrorCode::Internal` 一択、exit 70、retry なしである。**`internal_error` が retry される経路は実測で pin されている** — `T-SK077` が「`internal_error` は 1 回 retry され、2 回目の成功が返る」を assert し、その doc コメントが「`is_retriable` は `classify().kind` から導かねばならない」と書く。畳めばこの retry が消える。

**一方で、監査文書の論拠を直接 pin するテストは無い。** 監査文書の棄却理由は「`ok: false` かつ目的型に合わない本文が `Api` ではなく `Decode` に落ちる」というケースに立つが、そのケースを走らせるテストは 1 本も無い。測定範囲は兄弟テストファイル 2 本 (`src/slack/client/http_tests.rs`、`src/slack/client/constructor_tests.rs`) の全体で、`api_get`/`api_get_once` を直接呼ぶ 13 箇所すべてが目的型に `DummyBody` を指定している。`DummyBody` は serde が既定で未知フィールドを無視するため、`{"ok": false}` も `{"ok": false, "error": "…"}` も deserialize に成功する。

さらに、本番の目的型 3 種 (`ChannelBody`、`UserBody`、`MessagesBody`) と `DummyBody` はすべて全フィールドが `Option` か `#[serde(default)]` である。**したがってキー欠落では目的型の deserialize は失敗しない。** 監査文書が想定する分類変化を実際に起こすには、キーが存在して型が合わない本文が要る。

**この 2 点は矛盾しない。** 棄却の判断は正しく、その判断の論拠として監査文書が挙げた具体ケースは現在の型形状では発生しにくい、というのが実測の姿である。**CodeKB や DR にこの棄却を書くときは、監査文書の文言ではなく `T-SK077` と `classify()` の 6 分岐を根拠として引くこと。**

### E-4 — `converter.rs` を割るなら、素直に出せるのは 3 塊だけ

**「6 群」は上書きした。** 9 関心と 26 区間の内訳、およびその 2 つが機械測定ではなく分類であることは `code-structure.md` の `## サイズ分布` が持つ。この節は切り出しの判断材料だけを持つ。

**そのまま切り出せる連続区間は 3 つある。** 塊の長さ (連続区間の本数) と関心の総本数は別の数なので、両方を並べて書く。

| 関心     | 総本数 | 最長の連続区間 | 区間の外に散る分                              |
| -------- | ------ | -------------- | --------------------------------------------- |
| ANCHOR   | 10     | 9              | `T-FC026` が冒頭に 1 本                       |
| CELLCODE | 9      | 7              | `T-FC025` が冒頭、`T-FC098` が末尾に 1 本ずつ |
| SUPPRESS | 8      | 7              | `T-FC097` が末尾に 1 本                       |

**残る `PRE` 24 本と `TABLE` 16 本は各 4 区間に散っており、切り出す前に並べ替えが要る。** 連なりを割る側のテストは `T-FC082`、`T-FC055`、`T-FC043`、`T-FC046` である。

**テストだけの分割は可視性の変更が 0 件で済む。** 子モジュールは親の private 項目へ `super::` で到達できるためである。79 本のうち private を直接触るのは 3 本 (計 5 呼び出し) だけで、いずれも子モジュールから届く。

| テスト    | 触る private 項目                                |
| --------- | ------------------------------------------------ |
| `T-FC008` | `format_with_frontmatter`                        |
| `T-FC068` | `pre_handler` / `span_handler` / `table_handler` |
| `T-FC090` | `close_self_closed_raw_text_tags` (3 呼び出し)   |

残る 76 本はすべて `pub(super) fn to_fetch_result` 経由なので、そもそも private に触らない。

**実装側の分割はコストが 2 桁違うので、テスト分割とは別の判断として扱う。** 実装 985 行には素の private `fn` が 38 個あり、割るならその多くを `pub(super)` や `pub(in …)` へ広げることになる。さらに 6 個のヘルパーが関心をまたいで共有されており、切り出した先へ素直には付いていかない。

| 共有ヘルパー               | 呼び出し箇所 | またぐ関心                                                                         |
| -------------------------- | ------------ | ---------------------------------------------------------------------------------- |
| `element_tag`              | 8            | PRE / TABLE / SUPPRESS / CELLCODE                                                  |
| `trim_document_whitespace` | 3            | TABLE / ANCHOR                                                                     |
| `is_suppressed_element`    | 2            | SUPPRESS / CELLCODE                                                                |
| `push_text_content`        | 2            | CELLCODE。うち 1 つは自身への再帰なので、外から呼ぶのは `text_content` の 1 つだけ |
| `has_ancestor_matching`    | 2            | PRE (`has_pre_ancestor`) / CELLCODE (`has_table_cell_ancestor`)                    |
| `get_parent`               | 2            | 上の `has_ancestor_matching` 経由                                                  |

呼び出し箇所は **実装 985 行から定義行を除いた範囲での出現数** であって行数ではない。関心の結合が実在することは `T-FC095` が踏んでいる — テーブルセルの `<pre>` が `<script>` の中身を落とすことを assert し、その doc コメントが「セルの `<pre>` は walked text ではなく自分の部分木を読むので、同じ抑制を自分で適用しなければならない」と書く。

**前例はテスト側だけを割る形である。** `src/fetch/cdp/launch.rs` は実装 1 本のまま `#[cfg(test)] mod <name>_tests;` を 4 本並べ、テストだけを 4 関心へ割っている。`converter.rs` に同じ形を適用するなら実装 985 行は 1 本のまま残り、テスト 2,146 行が関心ごとの `*_tests.rs` になる。`BR-WS` 5 本と `CONTAINER-li` 2 本は scout 側の実装を持たず htmd 組み込みの挙動を pin する回帰群なので、「依存ライブラリの契約」として 1 本にまとめるのが関心の切り方に沿う。

**監査文書の「切り出す単位を先に決めないとファイルが 6 本に割れる」は、実測では 9 本である。**

### 作業ツリーの状態

どちらも commit されるものではない。

- **`Cargo.lock` が modified 状態** — 推移的依存 **14 crate** のパッチ版更新差分が未 commit。**`git diff --numstat` が返す 36 は変更行数であって crate 数ではない** (version 14 行 + checksum 14 行 + 依存参照 8 行)。この文書と `dependencies.md` の依存数値は `git show HEAD:Cargo.lock` 側の値である
- **リポジトリ直下の `*.profraw`** — カバレッジ計測の残骸。`.gitignore` の `*.profraw` で除外されるので commit はされない

## 前回監査との基準差

`docs/audit/2026-08-11-rust-code-assessment.md` を引用するときは、必ず基準の差を一緒に運ぶこと。

|         | 監査文書   | この CodeKB |
| ------- | ---------- | ----------- |
| version | v2.5.0     | v2.6.0      |
| commit  | `c0499fd`  | `ef2fbc9`   |
| 測定日  | 2026-08-17 | 2026-08-30  |

監査文書自身も内部で基準日を分けており、「D 節と F 節は 2026-08-11 時点の調査記録なので、当時の記述のまま残す」と明記する。

実測が動いた 4 項目。左が監査文書の値、右がこの CodeKB の測り直しである。**左の値は監査文書の記述の引き写しであり、こちらでは測っていない。**

| 指標                     | 監査文書 (v2.5.0) | この CodeKB (v2.6.0) |
| ------------------------ | ----------------- | -------------------- |
| lint 抑制                | 19                | 15                   |
| テスト ID                | 774               | 806                  |
| Decision Record          | 27                | 28                   |
| `src/fetch/converter.rs` | 3,177 行          | 3,131 行             |

**監査文書の inline `mod tests` の数え方には注意が必要である。** 実測値表が `mod tests {` ブロックと `mod tests;` 兄弟宣言を「26 ファイルが inline `mod tests` を含み」と一括りにしているため、無批判に引くと 26 が伝播する。正しくは inline ブロック 19 と、名前がちょうど `tests` の兄弟宣言 7 に割れる (`code-structure.md` の `## ファイル分類`)。

テスト実行結果についても同様で、監査文書は v2.5.0 時点の `cargo nextest run --all-features` を 854 passed (4 leaky)/1 skipped と記録する。この CodeKB の 851 は属性行の宣言数なので、この 2 つは直接比較できない。

**監査文書の E-4 を引く二次資料が他にもある。** `.claude/rules/CONVENTIONS.md` の「テストは関心ごとにファイルを分ける」節が、`src/fetch/converter.rs` を v2.5.0 基準 (実装 1,016 + テスト 2,161 = 3,177 行) で記述し、テスト ID の並びを 6 群と書いている。**この CodeKB はその 6 群を 9 関心へ上書きしたので、両者は現在食い違っている。** どちらを直すかは CodeKB の外の判断である。

## 未確認の項目

以下はこの CodeKB の走査で確認できなかった。**確定した所見へ格上げしないこと。** 再走査でファイルを開けばいずれも決着する。

| 項目                                                               | 未確認の理由                                                                                                                                                                                                                  |
| ------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `renovate.json` の **実効設定**                                    | ローカルに書かれた 4 規則は確定した (`dependencies.md` の `## 依存の自動更新`)。ただし `"extends": ["github>thkt/renovate-config"]` が指す共有プリセットは取得していないので、renovate が最終的に適用する設定は決まっていない |
| `release.yml` の 4 クロスビルドターゲットと Homebrew tap 更新      | ファイル未読。step 名レベルの確認にとどまる                                                                                                                                                                                   |
| `with_clock` / `with_rng` の再検討条件を引き継ぐ open issue の有無 | #310 の CLOSED は確認したが、後継 issue の探索をしていない。監査文書 E-2 は「再検討するなら本文書か新 issue へ移す必要がある」と書いており、**この CodeKB がその移送先になりうる**                                            |
| `label-from-issue.yml`、`.github/ISSUE_TEMPLATE/`                  | ファイル未読                                                                                                                                                                                                                  |
| `README.md` / `README.ja.md` の内容                                | ファイル未読                                                                                                                                                                                                                  |
| DR 本文 27 本 (0012 以外) の中身                                   | 索引 (`docs/decisions/README.md`) のみ読み。この CodeKB の DR 参照はタイトルと番号の対応に限る                                                                                                                                |
| モジュール内部のファイル間循環                                     | 依存グラフはトップレベルモジュール粒度で測っており、`src/fetch/` の中や `src/tools/` の中の循環は測っていない                                                                                                                 |
