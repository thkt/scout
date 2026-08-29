# Reverse Engineering Timestamp — scout

## 測定基準

この CodeKB の 9 アーティファクトが載せる数値と記述は、すべて下の基準で測ったものである。数値を引用するときはこの基準を一緒に運ぶこと。

| 項目            | 値                                                     |
| --------------- | ------------------------------------------------------ |
| 対象リポジトリ  | `/Users/thkt/GitHub/cli/scout` (repo qualifier なし)   |
| 基準 commit     | `c8460b5` (`c8460b59c04b785e7e8378b37bc80504bad2d743`) |
| package version | 2.6.0 (`Cargo.toml`)                                   |
| ブランチ        | `chore/aidlc-v2-install`                               |
| 測定日          | 2026-08-29                                             |

引用される外部文書の基準は別である。`docs/audit/2026-08-11-rust-code-assessment.md` は **v2.5.0 / commit `c0499fd` / 測定日 2026-08-17** 基準で、監査文書自身も D 節と F 節を 2026-08-11 時点の記録として内部で分けている。基準差の一覧は `code-quality-assessment.md` の `## 前回監査との基準差` が持つ。

## 2 回の走査

このストアは 2 回の走査を統合したものである。**2 回とも同じ commit `c8460b5` / version 2.6.0 / 測定日 2026-08-29 で測っているため、この 2 つの間には基準差が無い。** 数値を突き合わせるときに基準の変換は要らない。

| 走査                 | 種別                                                                                                                     | この走査が読んだもの                                                                    | pre-scan snapshot                                                                  |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| attempt 1 (初回)     | 全域スキャン (`codekb-scope-diff` が NO_STORE を返した)。9 アーティファクトの骨格を作った                                | 17 パス                                                                                 | `["./"]` / fingerprint `git:37f523eea65c4d730fe8b818c9a4e9aed6cd7618`              |
| attempt 2 (この走査) | **CURRENT なストアへの focused merge。** 既存の 17 パスは再走査せず、ストアが未読のまま抱えていた 4 ファイルだけを開いた | `src/fetch/converter.rs`、`src/slack/client.rs`、`src/tools/config.rs`、`renovate.json` | fingerprint `git:498e1629fc5c29351ee88564fe0d3c66c16f5cfe`。paths フィールドそのものは読んでいない |

focused merge なので、attempt 2 が触っていない節の記述は attempt 1 のまま残る。**attempt 2 が上書きした箇所は、各アーティファクトの本文に「上書き」と明示してある。**

### fingerprint は path 集合から導かれる git tree である

attempt 2 の走査担当は「`git rev-parse HEAD` (`c8460b5…`) と dispatch が渡した snapshot の fingerprint (`git:498e1629…`) は別の導出であり、突き合わせは担当範囲外」と記録した。**この統合時に突き合わせた結果、2 つは別の導出ではなく別のオブジェクト種別だった。**

測ったのは次の 3 点である。

| 測定                                                           | 結果                                                                |
| -------------------------------------------------------------- | ------------------------------------------------------------------- |
| `codekb-scope-diff --mint` を attempt 1 の 17 パスに対して実行 | `7f475813277b800717150f7dd8eeced0c2024e65` — ストアの記録値と一致   |
| 同 attempt 2 の 21 パスに対して実行                            | `498e1629fc5c29351ee88564fe0d3c66c16f5cfe` — snapshot の値と一致    |
| `git cat-file -t` を上の 2 つと `37f523ee…` に対して実行       | 3 つとも `tree`。HEAD の tree は `26edf83c…` で、どれとも一致しない |

fingerprint は commit ではなく、`analyzed.paths` の集合から組んだ内容アドレス付きの tree である。commit hash と比べれば当然食い違う。**走査担当が見た「不一致」は、tree と commit という別種のオブジェクトを比べたことによる。** mint は path の並び順に依らず、同じ集合なら同じ値を返す (上の 2 回はどちらもストアの記録と別順で渡して一致した)。

## アプリケーションソース外として除外した領域

省略ではなく明示的な除外である。**除外の理由は「gitignore されているから」ではなく「アプリケーションソースではないから」** で、この区別は誤解を招きやすいので根拠を添えて記録する。

| 領域                                      | 除外の根拠                                                                                                                                    | gitignore の実態                                                                                                                                                                                                                                                  |
| ----------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `.claude/`                                | AI-DLC フレームワーク本体 (skills、agents、tools、hooks、sensors、knowledge) が入り、scout の実行バイナリには 1 行も入らない                  | `.gitignore` の `.claude/` エントリが除外。`git ls-files .claude/` は 0 件                                                                                                                                                                                                            |
| `docs/*`                                  | 追跡対象は `docs/audit/` 14 ファイルと `docs/decisions/` 29 ファイル (DR 28 本 + README) のみで、両方ともこのスキャンの一次ソースとして扱った | `.gitignore` の `docs/*` エントリが除外し、`!docs/decisions/` と `!docs/audit/` だけを戻す                                                                                                                                                                                          |
| `aidlc/`                                  | AI-DLC のワークスペースでありアプリケーションソースではない                                                                                   | **ディレクトリ全体は gitignore されていない。** `git ls-files aidlc/` は 8 件を返し、`git check-ignore aidlc/spaces/default/memory/org.md` は何も返さない。除外されるのは `.gitignore` が個別に挙げるカーソル / ランタイム / スクラッチのパスだけである |
| `target/`、`workspace/`、`*.profraw` ほか | ビルド成果物とローカルキャッシュ                                                                                                              | 各パターンで除外                                                                                                                                                                                                                                                  |

これらを除いたアプリケーションソース面は `src/` 95 ファイル、`tests/` 4 ファイル、リポジトリ直下の設定 6 ファイル、`.github/`、`docs/decisions/`、`docs/audit/` である。

## `kind: partial` の根拠

**`kind: full` は `analyzed.paths` に `./` を要求し、以後どの intent もこのリポジトリを「検証済みの全域カバー」として扱う。2 回の走査を足してもその主張は裏付かない。**

モジュール網羅という意味では全域に触れているが、`analyzed.paths` の 21 パスのうち `src/` 配下は 13 ファイルであり、`code-structure.md` の `## ファイル分類` が数える実装 50 本・テスト専用 45 本 (11,538 行) の大半は依然として未読である。`kind: full` を立てると、その未読分がその主張の裏に入る。

**`analyzed.paths` はディレクトリ単位では書けない。** `src/fetch/`・`src/github/`・`src/tools/`・`src/slack/`・`src/brave/`・`src/search/` はいずれも配下に未読ファイルを含む。同じ理由で `shallow.paths` もファイル単位にしてある — ディレクトリ表記にすると `analyzed.paths` のファイルを内側に含み、同じパスが両側に立つ。65 パスすべてが作業ツリー上に実在し、2 つのリストは重ならない (attempt 2 は 4 パスを `shallow` から `analyzed` へ移しただけなので、合計 65 は attempt 1 から変わらない)。

このブロックに現れないが未読の範囲が 2 つある。どちらもディレクトリ表記が `analyzed.paths` と衝突するため、パスではなく散文で記録する。

1. `src/` 配下のテスト専用ファイル 45 本 (11,538 行) — テスト ID と分布は測ったが assertion 本文は未読。**`src/fetch/converter.rs` の inline `mod tests` 2,146 行は attempt 2 で全行を読んだが、この 45 本には含まれない** (inline ブロックであって兄弟ファイルではない)
2. `docs/decisions/` の DR 本文 27 本 — 0012 のみ全行を読み、README は索引として読んだ

**このブロックに現れないファイルを attempt 2 が証拠として引いている。** `src/slack/client/http_tests.rs` (1,056 行)、`src/slack/client/constructor_tests.rs` (96 行)、`src/slack.rs` の 3 本で、いずれも `analyzed.paths` の 21 にも `shallow.paths` の 44 にも無い。`shallow.paths` が `src/` のテスト専用兄弟ファイル 45 本を列挙していないためである。attempt 2 はこれらを **証拠の引用元** として扱い、格上げしていない。次回の snapshot 設計で扱いを決める必要がある。

`analyzed.components` の 7 語は `component-inventory.md` の `##` 見出しと文字単位で一致させてある。rerun guard がこの 2 つをリテラル比較するため、片方だけを言い換えるとこの走査のカバレッジが引き当てられなくなる。attempt 2 は既存の 7 コンポーネントの内側を深掘りしただけなので、この 7 語は attempt 1 から変わらない。

## Scope of Analysis

```yaml
scope_version: 1
kind: partial
intent: codekb-scout
fingerprint: 498e1629fc5c29351ee88564fe0d3c66c16f5cfe
analyzed:
  paths:
    - .config/nextest.toml
    - .github/workflows/ci.yml
    - Cargo.toml
    - clippy.toml
    - deny.toml
    - docs/decisions/0012-connect-time-ip-guard-for-ssrf-dns-rebinding.md
    - docs/decisions/README.md
    - renovate.json
    - src/classify.rs
    - src/envelope.rs
    - src/fetch.rs
    - src/fetch/cdp/launch.rs
    - src/fetch/cdp/proxy/transport.rs
    - src/fetch/converter.rs
    - src/github/types.rs
    - src/lib.rs
    - src/main.rs
    - src/slack/client.rs
    - src/tools.rs
    - src/tools/config.rs
    - src/tools/test_helpers.rs
  components:
    - tools
    - fetch
    - github
    - slack
    - brave
    - search
    - 横断リーフ
shallow:
  paths:
    - .github/workflows/label-from-issue.yml
    - .github/workflows/release.yml
    - .github/workflows/zizmor.yml
    - README.ja.md
    - README.md
    - docs/audit/
    - src/body_limit.rs
    - src/brave.rs
    - src/brave/client.rs
    - src/brave/types.rs
    - src/charset.rs
    - src/clock.rs
    - src/fetch/cdp.rs
    - src/fetch/cdp/proxy.rs
    - src/fetch/download.rs
    - src/fetch/extractor.rs
    - src/fetch/ssrf.rs
    - src/github.rs
    - src/github/encoding.rs
    - src/github/errors.rs
    - src/github/format.rs
    - src/github/helpers.rs
    - src/markdown.rs
    - src/redacted.rs
    - src/retry.rs
    - src/rng.rs
    - src/search.rs
    - src/search/engine.rs
    - src/search/lang.rs
    - src/signals.rs
    - src/slack.rs
    - src/slack/format.rs
    - src/slack/mention.rs
    - src/slack/url.rs
    - src/test_support.rs
    - src/token_source.rs
    - src/tools/builder.rs
    - src/tools/errors.rs
    - src/tools/params.rs
    - src/tools/query.rs
    - src/tools/repo.rs
    - src/tools/typo.rs
    - src/yaml.rs
    - tests/
```
