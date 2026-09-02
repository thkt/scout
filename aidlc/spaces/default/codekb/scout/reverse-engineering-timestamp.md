# Reverse Engineering Timestamp — scout

## 測定基準

この CodeKB の 9 アーティファクトが載せる数値と記述は、すべて下の基準で測ったものである。数値を引用するときはこの基準を一緒に運ぶこと。

| 項目            | 値                                                     |
| --------------- | ------------------------------------------------------ |
| 対象リポジトリ  | `/Users/thkt/GitHub/cli/scout` (repo qualifier なし)   |
| 基準 commit     | `ef2fbc9` (`ef2fbc93b8936163de6b19986a5e4624fc3f7200`) |
| package version | 2.6.0 (`Cargo.toml`)                                   |
| ブランチ        | `chore/aidlc-v2-install`                               |
| 測定日          | 2026-08-30                                             |

**作業ツリーは HEAD と 1 ファイル食い違う。** `Cargo.lock` に未コミットの変更があり、推移的依存 14 パッケージの version と checksum が動いている。依存について述べる数値はすべて `git show HEAD:Cargo.lock` 側の値であり、その差の内訳は `dependencies.md` の `## 作業ツリーと HEAD の差` が持つ。

引用される外部文書の基準は別である。`docs/audit/2026-08-11-rust-code-assessment.md` は **v2.5.0 / commit `c0499fd` / 測定日 2026-08-17** 基準で、監査文書自身も D 節と F 節を 2026-08-11 時点の記録として内部で分けている。基準差の一覧は `code-quality-assessment.md` の `## 前回監査との基準差` が持つ。

## この走査は全面再走査である

**先行するストア (基準 commit `c8460b5`、測定日 2026-08-29) は STALE と判定され、9 アーティファクトを丸ごと置き換えた。** この `## Scope of Analysis` ブロックもこの走査だけから組んでいる。先行ストアの記述を引き継いだ箇所はあるが、引き継ぐ前に測り直している。

置き換えの動機はコードの変化ではない。`git diff --name-status c8460b5..HEAD` をアプリケーションソース面へ当てると、動いたのは 2 ファイルだけである。

| ファイル                                    | 変更       | 中身                                                                                                                                   |
| ------------------------------------------- | ---------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `.github/workflows/ci.yml`                  | M (2 行)   | `taiki-e/install-action` の SHA を v2.85.11 (`7f4eb899…`) から v2.86.3 (`5b4d68e2…`) へ。`cargo-nextest` と `cargo-llvm-cov` の 2 step |
| `docs/audit/2026-08-28-aidlc-v2-install.md` | M (+79 行) | AI-DLC 導入記録。Rust ソースではない                                                                                                   |

**`src/` は 1 バイトも動いていない。** したがってストアが `src/` について載せていた数値のうち、測り方が正しいものはこの HEAD でもそのまま成り立つ。この走査が覆した 3 件は、コードの変化ではなくストア側の数え方の誤りである。覆した内容は `code-structure.md` の `## ファイル分類` と `## モジュール宣言と可視性`、および `architecture.md` の `## モジュール依存の実形` が持つ。

## 読みの深さは 2 段階に分かれる

`analyzed.paths` の 35 パスは均質ではない。**35 を平らに並べると、次回の走査は 21 パスもこの commit で深く読まれたものとして扱う。** 段階を分けて記録する。

| 段階 | 件数 | 内容                                                                                                                                                                                              |
| ---- | ---- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | 14   | この走査で端から端まで読んだファイル。先行ストアが未読のまま抱えていた分                                                                                                                          |
| 2    | 21   | 先行ストアの `analyzed.paths`。`git diff c8460b5..HEAD` がこの 21 パスに対して `.github/workflows/ci.yml` の 2 行だけを返したので、内容が無変更であることを確認したうえでストアの記述を測り直した |

**段階 1 の 14 ファイル**: `src/markdown.rs`、`src/yaml.rs`、`src/search/engine.rs`、`src/token_source.rs`、`src/retry.rs`、`src/redacted.rs`、`src/body_limit.rs`、`src/signals.rs`、`src/charset.rs`、`src/rng.rs`、`src/clock.rs`、`src/search.rs`、`.github/workflows/zizmor.yml`、`.github/zizmor.yml`。

段階 2 のうち、この走査で改めて全行を開いたのは `.github/workflows/ci.yml`、`.config/nextest.toml`、`src/main.rs` の 3 本と、`src/lib.rs` の `mod` 宣言部および crate 直下の項目である。残りはストアの記述を測り直す形で当たった。

**この 2 リストに現れないが証拠として引いたものがある。** `src/lib.rs` 以外の `src/` 全 95 ファイル (`use crate::` の全数走査の対象)、`src/tools/builder.rs`、`src/fetch/converter.rs` の inline テストブロック、`src/slack/client/http_tests.rs` の 4 つで、いずれも該当する 1 行を引いた以上の読みはしていないので格上げしない。加えて外部リポジトリ 3 ファイル (`zizmorcore/zizmor-action` の `action.yml` と `action.sh`、`zizmorcore/zizmor` の `docs/usage.md`) を `gh api` で読んだ。scout のツリー外なのでどちらのリストにも入らない。

## fingerprint の由来

**`fingerprint` はこの走査の 35 パス集合に対して新しく mint した値である。** コマンドは次のとおりで、出力をそのまま `## Scope of Analysis` に置いた。

```
bun .claude/tools/aidlc-utility.ts codekb-scope-diff --repo scout --mint --paths <35 パスをカンマ区切りで>
```

**`--paths` はカンマ区切りである。** 空白区切りで渡すと先頭 1 パスだけを読み、残り 34 を黙って捨てた値が返る。この走査は最初その形で mint し、35 パス集合とは無関係な値を得た。`--json` を付けると `paths` 配列が echo されるので、**渡した数だけ載っているかを毎回確認すること。**

**この走査に無関係な 2 つの値と取り違えないこと。** pre-scan snapshot が渡した `git:9ab12e551ead0c2b36a3b3b83634c9783030fd2a` は paths `["./"]` に対する source fingerprint であり、path 集合の mint ではない。先行ストアが載せる `498e1629fc5c29351ee88564fe0d3c66c16f5cfe` は旧 21 パスに対する mint である。

**その旧 21 パスを今 mint し直すと `498e1629…` ではなく `d771a2a2d80f3c5689e789d4dbad2375439edc5d` が返る。** 同じ path 集合でも中身が変われば値が動く — この 21 パスには `.github/workflows/ci.yml` が含まれ、そのファイルが `c8460b5` から 2 行動いている。**fingerprint はパスの一覧ではなく内容を指しており、ストアが STALE と判定される機械的な理由がこれである。**

mint が返すのは `analyzed.paths` の集合から組んだ内容アドレス付きの git tree であって、commit ではない。**commit hash と比べれば当然食い違う。** 先行ストアはこの取り違えの経緯に 1 節を費やしており、同じ混同を繰り返さないために由来をここに 1 行残す。mint は path の並び順に依らず、同じ集合なら同じ値を返す (35 パスを逆順で渡して同値を確認した)。

## アプリケーションソース外として除外した領域

省略ではなく明示的な除外である。**除外の理由は「gitignore されているから」ではなく「アプリケーションソースではないから」** で、この区別は誤解を招きやすいので根拠を添えて記録する。

| 領域                                      | 除外の根拠                                                                                                                                | gitignore の実態                                                                                                                             |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `.claude/`                                | AI-DLC フレームワーク本体 (skills、agents、tools、hooks、sensors、knowledge) が入り、scout の実行バイナリには 1 行も入らない              | `.gitignore` の `.claude/` エントリが除外。`git ls-files .claude/` は 0 件                                                                   |
| `docs/*`                                  | 追跡対象は `docs/audit/` 14 ファイルと `docs/decisions/` 29 ファイル (DR 28 本 + README) のみで、両方ともこの走査の一次ソースとして扱った | `.gitignore` の `docs/*` エントリが除外し、`!docs/decisions/` と `!docs/audit/` だけを戻す                                                   |
| `aidlc/`                                  | AI-DLC のワークスペースでありアプリケーションソースではない                                                                               | **ディレクトリ全体は gitignore されていない。** 除外されるのは `.gitignore` が個別に挙げるカーソル / ランタイム / スクラッチのパスだけである |
| `target/`、`workspace/`、`*.profraw` ほか | ビルド成果物とローカルキャッシュ                                                                                                          | 各パターンで除外                                                                                                                             |

これらを除いたアプリケーションソース面は `src/` 95 ファイル (27,359 行)、`tests/` 4 ファイル (1,833 行)、リポジトリ直下の設定 6 ファイル、`.github/`、`docs/decisions/`、`docs/audit/` である。

## `kind: partial` の根拠

**`kind: full` は `analyzed.paths` に `./` を要求し、以後どの intent もこのリポジトリを「検証済みの全域カバー」として扱う。この走査を足してもその主張は裏付かない。**

モジュール網羅という意味では全域に触れているが、深く読んだ範囲は `src/` の一部にとどまる。**35 パスのうち `src/` 配下は 25 ファイルで、そのうち `src/tools/test_helpers.rs` はテスト専用ファイルである。つまり深く読んだ実装ファイルは 48 本中 24 本である。** テスト専用ファイル 47 本 (12,498 行) の大半も依然として未読で、`kind: full` を立てるとその未読分がその主張の裏に入る。

**`analyzed.paths` はディレクトリ単位では書けない。** `src/fetch/`・`src/github/`・`src/tools/`・`src/slack/`・`src/brave/`・`src/search/` はいずれも配下に未読ファイルを含む。同じ理由で `shallow.paths` もファイル単位に保つ — ディレクトリ表記にすると `analyzed.paths` のファイルを内側に含み、同じパスが両側に立つ。66 パスすべてが作業ツリー上に実在し、2 つのリストは重ならない。

このブロックに現れないが未読の範囲が 2 つある。どちらもディレクトリ表記が `analyzed.paths` と衝突するため、パスではなく散文で記録する。

1. `src/` 配下のテスト専用ファイル 47 本 (12,498 行) — テスト ID と分布は測ったが assertion 本文は大半が未読。**`src/fetch/converter.rs` の inline `mod tests` 2,146 行は読んだが、この 47 本には含まれない** (inline ブロックであって兄弟ファイルではない)
2. `docs/decisions/` の DR 本文 27 本 — 0012 のみ全行を読み、README は索引として読んだ

`analyzed.components` の 7 語は `component-inventory.md` の `##` 見出しと文字単位で一致させてある。rerun guard がこの 2 つをリテラル比較するため、片方だけを言い換えるとこの走査のカバレッジが引き当てられなくなる。**この走査は `横断リーフ` という語を残す判断をした。** 判断の理由は `component-inventory.md` の `## 横断リーフ` が持つ。

## Scope of Analysis

```yaml
scope_version: 1
kind: partial
intent: codekb-scout
fingerprint: 5259dd37a7da16216a521540472a01f83697be06
analyzed:
  paths:
    - .config/nextest.toml
    - .github/workflows/ci.yml
    - .github/workflows/zizmor.yml
    - .github/zizmor.yml
    - Cargo.toml
    - clippy.toml
    - deny.toml
    - docs/decisions/0012-connect-time-ip-guard-for-ssrf-dns-rebinding.md
    - docs/decisions/README.md
    - renovate.json
    - src/body_limit.rs
    - src/charset.rs
    - src/classify.rs
    - src/clock.rs
    - src/envelope.rs
    - src/fetch.rs
    - src/fetch/cdp/launch.rs
    - src/fetch/cdp/proxy/transport.rs
    - src/fetch/converter.rs
    - src/github/types.rs
    - src/lib.rs
    - src/main.rs
    - src/markdown.rs
    - src/redacted.rs
    - src/retry.rs
    - src/rng.rs
    - src/search.rs
    - src/search/engine.rs
    - src/signals.rs
    - src/slack/client.rs
    - src/token_source.rs
    - src/tools.rs
    - src/tools/config.rs
    - src/tools/test_helpers.rs
    - src/yaml.rs
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
    - README.ja.md
    - README.md
    - docs/audit/
    - src/brave.rs
    - src/brave/client.rs
    - src/brave/types.rs
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
    - src/search/lang.rs
    - src/slack.rs
    - src/slack/format.rs
    - src/slack/mention.rs
    - src/slack/url.rs
    - src/test_support.rs
    - src/tools/builder.rs
    - src/tools/errors.rs
    - src/tools/params.rs
    - src/tools/query.rs
    - src/tools/repo.rs
    - src/tools/typo.rs
    - tests/
```
