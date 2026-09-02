# Developer Code Scan — scout (full rescan, attempt 3)

**Collaborator:** aidlc-developer-agent

## Developer Code Scan Results

この走査は **FULL RESCAN** である。既存 CodeKB (`aidlc/spaces/default/codekb/scout/`、9 artifact) は STALE と判定されており、architect は 9 artifact を丸ごと置き換える。`## Scope of Analysis` ブロックもこの走査だけから組む。

測定基準は次のとおり。すべての数値をこの基準と一緒に運ぶこと。

| 項目                     | 値                                                                                    |
| ------------------------ | ------------------------------------------------------------------------------------- |
| 対象リポジトリ           | `/Users/thkt/GitHub/cli/scout`                                                        |
| 基準 commit              | `ef2fbc9` (`ef2fbc93b8936163de6b19986a5e4624fc3f7200`)                                |
| package version          | 2.6.0 (`Cargo.toml`)                                                                  |
| ブランチ                 | `chore/aidlc-v2-install`                                                              |
| 測定日                   | 2026-08-30                                                                            |
| pre-scan snapshot        | paths `["./"]` / source fingerprint `git:9ab12e551ead0c2b36a3b3b83634c9783030fd2a`   |
| 既存ストアの基準 commit  | `c8460b5` (`c8460b59c04b785e7e8378b37bc80504bad2d743`)、測定日 2026-08-29             |

**作業ツリーは HEAD と 1 ファイル食い違う。** `Cargo.lock` に未コミットの変更があり、14 パッケージ (`cc`、`chacha20`、`combine`、`cpufeatures`、`crc32fast`、`either`、`h2`、`icu_provider`、`log`、`rustls-webpki`、`syn`、`which`、`zerovec`、`zerovec-derive`) の version/checksum 行 28 本が動いている。`[[package]] name = "scout"` の version は 2.6.0 のまま、`Cargo.toml` は無変更なので直接依存は動いていない。この走査が依存について述べる数値は `git show HEAD:Cargo.lock` 側の値である。

### ストアの基準からの差分は 2 ファイルだけである

`git diff --name-status c8460b5..HEAD` をアプリケーションソース面 (`src/ tests/ Cargo.toml Cargo.lock clippy.toml deny.toml renovate.json .config/ .github/ docs/decisions/ docs/audit/`) に当てた結果。

| ファイル                                   | 変更     | 中身                                                                                                                                       |
| ------------------------------------------ | -------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| `.github/workflows/ci.yml`                 | M (2 行) | `taiki-e/install-action` の SHA を `7f4eb899022d8fe70b20c4f3de697aa85c309026` (v2.85.11) から `5b4d68e2e660441203ab128a23676f1e4faf1532` (v2.86.3) へ。48 行目 (`tool: cargo-nextest`) と 103 行目 (`tool: cargo-llvm-cov`) の 2 箇所 |
| `docs/audit/2026-08-28-aidlc-v2-install.md` | M (+79 行) | AI-DLC 導入記録。Rust ソースではない。`docs/audit/` はストアでも `shallow` 扱いだった                                                    |

**`src/` は 1 バイトも動いていない。** したがって `src/` について既存ストアが載せる数値は、測り方が正しければこの HEAD でもそのまま成り立つ。以下で数値を覆した 1 件は、コードの変化ではなく **ストア側の数え方の誤り** である。

### Scan Coverage

読みの深さが 2 段階に分かれる。両方を `analyzed` に入れるので、段階を明示する。

**段階 1 — この attempt で端から端まで読んだ 14 ファイル** (ストアが未読のまま抱えていた分)

| ファイル                       | 行数 | 実装 / テストの内訳                                    |
| ------------------------------ | ---- | ------------------------------------------------------ |
| `src/markdown.rs`              | 671  | 実装 1-280、inline `mod tests` 282-671                 |
| `src/yaml.rs`                  | 403  | 実装 1-196、inline `mod tests` 198-403 (テスト 15 本)  |
| `src/search/engine.rs`         | 245  | 全 245 行が実装。テストは兄弟 `src/search/engine/tests.rs` |
| `src/token_source.rs`          | 223  | 実装 1-103、inline `mod tests` 105-223                 |
| `src/retry.rs`                 | 160  | 全 160 行が実装。テストは兄弟 `src/retry/tests.rs`     |
| `src/redacted.rs`              | 128  | 実装 1-58、inline `mod tests` 60-128                   |
| `src/body_limit.rs`            | 100  | 全 100 行が実装。テストは兄弟 `src/body_limit/tests.rs` |
| `src/signals.rs`               | 99   | 実装 1-68、inline `mod tests` 70-99                    |
| `src/charset.rs`               | 86   | 実装 1-20、inline `mod tests` 22-86                    |
| `src/rng.rs`                   | 80   | 実装 1-46、inline `mod tests` 48-80                    |
| `src/clock.rs`                 | 51   | 実装 1-32、inline `mod tests` 34-51                    |
| `src/search.rs`                | 6    | `mod` 宣言と `pub(crate) use lang::Lang;` のみ         |
| `.github/workflows/zizmor.yml` | 30   | —                                                      |
| `.github/zizmor.yml`           | 15   | rule ignore 3 件。ストアのどちらのリストにも無かった   |

**段階 2 — ストアの 21 パスの再検証。** `git diff c8460b5..HEAD` がこの 21 パスに対して `.github/workflows/ci.yml` の 2 行だけを返したので、残る 20 パスの内容は無変更である。この attempt で改めて全行を開いたのは `.github/workflows/ci.yml`、`.config/nextest.toml`、`src/main.rs` の 3 本、および `src/lib.rs` の `mod` 宣言部 (1-20 行) と crate 直下の項目 (22 行の `USER_AGENT`、30-34 行の再輸出) である。残りはストアの記述を測り直す形で当たった (下の「上書きした主張」を見る)。

**アプリケーションソース外として除外した領域。** 省略ではなく明示的な除外である。

| 領域                                            | 除外の根拠                                                                                                     |
| ----------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `.claude/`                                      | AI-DLC フレームワーク本体 (skills / agents / tools / hooks / sensors / knowledge)。scout のバイナリに 1 行も入らない |
| `aidlc/`                                        | AI-DLC のワークスペース (memory / codekb / intents / audit)。アプリケーションソースではない                      |
| `docs/*` (`docs/decisions/`・`docs/audit/` を除く) | `.gitignore` が `docs/*` を除外し `!docs/decisions/` `!docs/audit/` だけを戻す。戻る 2 つはこの走査の一次ソース |
| `target/`、`workspace/`、`*.profraw`            | ビルド成果物とローカルキャッシュ                                                                                  |

除いた残りのアプリケーションソース面は `src/` 95 ファイル (27,359 行)、`tests/` 4 ファイル (1,833 行)、リポジトリ直下の設定 6 ファイル、`.github/`、`docs/decisions/` 29 ファイル、`docs/audit/` 14 ファイルである。

**`kind` は `partial` である。** `analyzed.paths` に `./` は入れられない。`src/` の 95 ファイルのうち `analyzed.paths` に載るのは 25 ファイルで、実装 48 本・テスト専用 47 本の大半は依然として未読である。`analyzed.paths` をディレクトリ単位で書けない事情もストアと同じで、`src/fetch/`・`src/github/`・`src/tools/`・`src/slack/`・`src/brave/`・`src/search/` はいずれも配下に未読ファイルを含む。同じ理由で `shallow.paths` もファイル単位に保つ。

#### architect へ渡す 2 リスト

`analyzed.paths` — **35 件**。ストアの 21 に、この走査が読んだ 14 を足したもの。

```
.config/nextest.toml
.github/workflows/ci.yml
.github/workflows/zizmor.yml
.github/zizmor.yml
Cargo.toml
clippy.toml
deny.toml
docs/decisions/0012-connect-time-ip-guard-for-ssrf-dns-rebinding.md
docs/decisions/README.md
renovate.json
src/body_limit.rs
src/charset.rs
src/classify.rs
src/clock.rs
src/envelope.rs
src/fetch.rs
src/fetch/cdp/launch.rs
src/fetch/cdp/proxy/transport.rs
src/fetch/converter.rs
src/github/types.rs
src/lib.rs
src/main.rs
src/markdown.rs
src/redacted.rs
src/retry.rs
src/rng.rs
src/search.rs
src/search/engine.rs
src/signals.rs
src/slack/client.rs
src/token_source.rs
src/tools.rs
src/tools/config.rs
src/tools/test_helpers.rs
src/yaml.rs
```

`shallow.paths` — **31 件**。ストアの shallow 44 から、上へ格上げした 13 件を引いたもの (`.github/zizmor.yml` はストアのどちらのリストにも無かった新規パスなので、この引き算には入らない)。

```
.github/workflows/label-from-issue.yml
.github/workflows/release.yml
README.ja.md
README.md
docs/audit/
src/brave.rs
src/brave/client.rs
src/brave/types.rs
src/fetch/cdp.rs
src/fetch/cdp/proxy.rs
src/fetch/download.rs
src/fetch/extractor.rs
src/fetch/ssrf.rs
src/github.rs
src/github/encoding.rs
src/github/errors.rs
src/github/format.rs
src/github/helpers.rs
src/search/lang.rs
src/slack.rs
src/slack/format.rs
src/slack/mention.rs
src/slack/url.rs
src/test_support.rs
src/tools/builder.rs
src/tools/errors.rs
src/tools/params.rs
src/tools/query.rs
src/tools/repo.rs
src/tools/typo.rs
tests/
```

算術: 21 + 14 = 35、44 − 13 = 31。リストは手写しではなく、ストアの `## Scope of Analysis` ブロックを `awk` で抽出し `sort -u` / `comm -23` / `comm -12` で導出した。2 リストが重ならないこと、65 + 1 = 66 パスすべてが作業ツリー上に実在することを機械で確認済み。

**この 2 リストに現れないが証拠として引いたものが 4 つある。** `src/lib.rs` 以外の `src/` 全 95 ファイル (`use crate::` の全数走査の対象)、`src/tools/builder.rs:80,94`、`src/fetch/converter.rs:990`、`src/slack/client/http_tests.rs:10`。いずれも「1 行を引いた」以上の読みはしていないので格上げしない。加えて外部リポジトリ 3 ファイル (`zizmorcore/zizmor-action` の `action.yml` と `action.sh`、`zizmorcore/zizmor` の `docs/usage.md`) を `gh api` で読んだ。scout のツリー外なのでどちらのリストにも入らない。

---

## モジュール依存の実形 — ストアの主張を 3 箇所で上書きする

**この節がこの走査の主目的である。** ストアは「循環は無い」「横断リーフ側からバックエンドへの import も無い」と書く。**どちらも成り立たない。**

### 測定方法とその限界

`src/` の全 95 ファイルから `use crate::…;` 文を全数抽出し (複数行の brace group を含めて 166 文)、各 import を「ファイルが属するトップレベルモジュール → import 先のトップレベルモジュール」の辺に落とした。同一モジュール内の import は辺にしない。各辺を **本番 / テスト専用** に分けた。テスト専用の判定は次の 2 条件のいずれかである。

- そのファイルが `#[cfg(test)] mod <name>;` または `#[cfg(all(test, feature = "js-rendering"))] mod <name>;` で宣言された兄弟テストファイルである
- その import 文が inline の `#[cfg(test)] mod <name> { … }` ブロックの brace 範囲の内側にある

**この測定が覆わない範囲を先に述べる。** 3 つある。

1. **`src/lib.rs` 発の辺は入らない。** `lib.rs` は crate root なので `use envelope::{…}` `use signals::{…}` `use tools::{…}` と `crate::` 接頭辞なしで書く (`src/lib.rs:30-34`)。手で足すと `(lib.rs) → envelope`、`(lib.rs) → signals`、`(lib.rs) → tools` の 3 辺になる。**`signals` はこの 3 辺以外にどこからも参照されないので、`use crate::` だけを見るグラフには 1 度も現れない。**
2. **`use` を経由しないインラインのパス参照は入らない。** `grep -rn 'crate::' src/ | grep -v 'use crate::'` は 34 行を返す。内訳は `pub(in crate::slack)` などの可視性修飾子 26 行、doc コメント / 通常コメント内の参照 6 行、実コード 2 行である。実コード 2 行は `src/tools/builder.rs:80` と `:94` の `.user_agent(crate::USER_AGENT)` で、`src/lib.rs:22` の crate 直下 const を指す。よって足りない本番の辺は `tools → (crate root の USER_AGENT)` 1 本だけである。
3. **`crate::` 直下の再輸出は 1 件を別枠にした。** `use crate::[A-Z]` に当たるのは `src/tools/repo_lazy_tests.rs:3` の `use crate::ErrorCode;` 1 件のみ。`src/lib.rs:30` の非公開 `use envelope::{…, ErrorCode, …}` を経由して解決するので、実体は `tools → envelope` のテスト専用辺である。

### 本番の辺 — 17 ノード・56 辺

出次数のあるモジュールは 10 個で、残り 7 個は crate 内への出辺を 1 本も持たない終端である。

| 起点           | 本番の import 先                                                                                                  | 出次数 |
| -------------- | ----------------------------------------------------------------------------------------------------------------- | ------ |
| `tools`        | `brave` `classify` `clock` `envelope` `fetch` `github` `markdown` `retry` `rng` `search` `slack` `token_source` `yaml` | 13     |
| `github`       | `body_limit` `charset` `classify` `clock` `envelope` `markdown` `redacted` `retry` `rng` `token_source` `yaml`     | 11     |
| `slack`        | `body_limit` `classify` `clock` `envelope` `redacted` `retry` `rng` `yaml`                                         | 8      |
| `brave`        | `body_limit` `classify` `clock` `envelope` `redacted` `retry` `rng`                                                | 7      |
| `fetch`        | `body_limit` `charset` `classify` `envelope` `markdown` `yaml`                                                     | 6      |
| `search`       | `brave` `fetch` `markdown` `yaml`                                                                                  | 4      |
| `classify`     | `envelope` `retry`                                                                                                 | 2      |
| `retry`        | `clock` `rng`                                                                                                      | 2      |
| `yaml`         | `markdown` **`search`**                                                                                            | 2      |
| `token_source` | `redacted`                                                                                                         | 1      |

crate 内への出辺を持たない 7 モジュール: `body_limit`、`charset`、`clock`、`envelope`、`markdown`、`redacted`、`rng`。`signals` も出辺を持たないが、上の限界 1 のとおり入辺の側がこの表に現れない。

### 循環は 2 本ある。どちらも 1 本の辺を通る

単純閉路を全列挙した結果は次の 2 本だけである。

```
search -> yaml -> search
fetch  -> yaml -> search -> fetch
```

**`yaml → search` の 1 辺を除くと、残りの 55 辺は非巡回になる。** これが architect に渡すべき層の規則の形である — 「循環が無い」ではなく「文書化された派生定数の辺 1 本を除いて非巡回である」。

その 1 辺は `src/yaml.rs:9` の

```rust
use crate::search::engine::MAX_PAGE_BYTES;
```

**両側に理由の doc コメントがある。意図された参照であって事故ではない。**

- `src/search/engine.rs:20-22` — `/// `pub(crate)` because `yaml::MAX_FIELD_BYTES` derives the per-field frontmatter cap from this same page budget.` に続いて `pub(crate) const MAX_PAGE_BYTES: usize = 4_500;`
- `src/yaml.rs:130-137` — `const MAX_FIELD_BYTES: usize = MAX_PAGE_BYTES / 10;` の doc コメントが、4,500 の 1/10 を選んだ算術 (title / author / date の 3 フィールドで 3/10、`escape_yaml` が最悪で倍にするので 6/10、残りが body の取り分) を数値で書く

逆向きの `search → yaml` は `src/search/engine.rs:18` の `use crate::yaml::truncate_and_reneutralize;` で、`format_fetched_pages` (`src/search/engine.rs:200`) が `truncate_and_reneutralize(&content, MAX_PAGE_BYTES)` を呼ぶ。**同じ 1 つの予算値を、上限を決める側と切る側の両方が参照している。** 3 ノードの閉路 `fetch → yaml → search → fetch` も同じ辺を通る (`fetch → yaml` は `src/fetch/converter.rs:13` の `use crate::yaml::{neutralize_yaml_markers_outside_fences, write_yaml_str};`、`search → fetch` は `src/search/engine.rs:13-15` の 3 本)。

### テスト専用の逆向き辺が 2 本ある

本番のグラフには現れず、`cfg(test)` の下にだけ立つ辺で、どちらも層の向きに逆らう。

| 辺                | 出どころ                              | 内容                                                                                      |
| ----------------- | ------------------------------------- | ----------------------------------------------------------------------------------------- |
| `slack → tools`   | `src/slack/client/http_tests.rs:10`   | `use crate::tools::ScoutError;`。ハンドラ層へ逆流する。`src/slack/client.rs:619-620` の `#[cfg(test)] mod http_tests;` 配下 |
| `fetch → search`  | `src/fetch/converter.rs:990`          | `use crate::search::engine::MAX_PAGE_BYTES;`。バックエンド間の辺。`src/fetch/converter.rs:986` から始まる inline `#[cfg(test)] mod tests` の内側 |

`fetch → search` は本番の辺ではない。`src/fetch/converter.rs` の実装部 (1-985 行) が持つ `use crate::` は 12 行目の `markdown` と 13 行目の `yaml` の 2 本だけである。

このほか `test_support` へ向かうテスト専用辺が 8 モジュールから 22 本ある (`body_limit` 2、`brave` 2、`fetch` 2、`github` 2、`retry` 1、`search` 1、`slack` 3、`tools` 9)。`src/test_support.rs` は `src/lib.rs:16-17` の `#[cfg(test)] mod test_support;` 配下なので、リリースビルドには入らない。

### ストアが直すべき箇所は 9 つある

**`architecture.md:55` はなぜ誤ったかを自分で書いている。** 「この一方向性の測定範囲は `src/tools.rs` と `src/fetch.rs` の `use crate::…` を読んだ範囲に限る」と添えてある。**その 2 ファイルには実際に反証が無い。** 誤りは測定にではなく、2 ファイルで測った結果を crate 全体の主張として書いたことにある。同じ形の誤りを繰り返さないため、この走査の主張には `src/` 全 95 ファイルという測定範囲を付けてある。

`grep -n '循環\|一方向\|逆向きの import\|no leaf\|バックエンドへの import' aidlc/spaces/default/codekb/scout/*.md` で全数列挙した。同じ誤りが 3 ファイル・9 箇所に写っている。**1 箇所だけ直すと残り 8 箇所が古いまま残る。**

| 所在                          | 現在の文言                                                                                                       | 実形との食い違い                                                             |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------ |
| `architecture.md:7`           | 「レイヤは 4 段で、依存の向きは一方向である。」                                                                 | 一方向ではない。`yaml → search` が 4 段目の内側で逆流する                    |
| `architecture.md:53`          | text fallback の `no leaf imports a backend`                                                                     | `yaml` が `search` を import する                                             |
| `architecture.md:55`          | 「**循環は無い。** … 横断リーフ側からバックエンドへの import も無い。」                                        | 循環が 2 本ある。リーフ → バックエンドの import が 1 本ある                  |
| `dependencies.md:56`          | 「向きは一方向で、循環は無い。」                                                                                 | 同上                                                                          |
| `dependencies.md:67`          | text fallback の `no leaf imports a backend and no backend imports tools`                                        | 前半は偽。後半は本番では真だが、テストでは `slack → tools` が立つ            |
| `dependencies.md:69`          | 「`src/tools.rs` は … 逆向きの import を持たない。`search` だけが例外的に他のバックエンド 2 つを合成する。」   | 前半は本番では真、テストでは偽。後半はバックエンド間の辺としては真だが、リーフ → バックエンドの辺 `yaml → search` を取りこぼす |
| `component-inventory.md:15`   | 「依存の向きは `main` から横断リーフへの一方向で、循環は無い。」                                                | 同上                                                                          |
| `component-inventory.md:27`   | `## tools` の 依存 の「逆向きの import は持たない。」                                                           | 本番では真。テストでは `src/slack/client/http_tests.rs:10` が `crate::tools::ScoutError` を import する |
| `component-inventory.md:112`  | `## 横断リーフ` の 依存 の「バックエンドへの import は持たない。この向きが循環の不在を支えている。」          | 偽                                                                            |

`component-inventory.md` の Mermaid 図そのもの (`LEAF` ノードから他ノードへの矢印が無い) と `dependencies.md` の図も、本番グラフの `yaml → search` を表現していない。

### `component-inventory.md` の `**依存**` 欄は 7 つとも実形と合わない

**文だけでなくデータが合っていない。** 7 コンポーネントの `**依存**` 欄を本番の辺と突き合わせた結果、**7 つすべてに欠落か過剰がある。** 個別に直すのではなく、上の本番辺の表から 7 欄を組み直すのが正しい。

| コンポーネント | ストアの `**依存**`                                                                       | 欠けている本番の辺                        | ストアが挙げるが**存在しない**辺                     |
| -------------- | ------------------------------------------------------------------------------------------- | ----------------------------------------- | ---------------------------------------------------- |
| `tools`        | `brave::client` `clock` `envelope` `fetch` `github` `markdown` `rng` `slack` `token_source` `yaml` (10) | `classify` `retry` `search`               | —                                                    |
| `fetch`        | `classify` `envelope` `retry` `body_limit` `markdown` `charset` (6)                          | `yaml`                                    | **`retry`**                                          |
| `github`       | `classify` `retry` `body_limit` `redacted` `token_source` `charset` `markdown` (7)           | `clock` `envelope` `rng` `yaml`           | —                                                    |
| `slack`        | `classify` `retry` `body_limit` `redacted` `yaml` `markdown` (6)                             | `clock` `envelope` `rng`                  | **`markdown`**                                       |
| `brave`        | `classify` `retry` `body_limit` `redacted` (4)                                               | `clock` `envelope` `rng`                  | —                                                    |
| `search`       | `brave` `fetch` `envelope` (3)                                                               | `markdown` `yaml`                         | **`envelope`**                                       |
| 横断リーフ     | 「バックエンドへの import は持たない」                                                       | `yaml → search`                           | —                                                    |

存在しない 3 辺は個別に確認した。`grep -rn 'use crate::retry' src/fetch.rs src/fetch/`、`grep -rn 'use crate::markdown' src/slack.rs src/slack/`、`grep -rn 'use crate::envelope' src/search.rs src/search/` はいずれも 0 件を返す (テストファイルを含めても `slack` の `envelope` が 1 件出るだけで、`markdown` は 0 件)。

### `analyzed.components` の 7 語を動かすなら両方を同時に動かす

`re-artifacts.md` は rerun guard が `analyzed.components` と `component-inventory.md` の `##` 見出しをリテラル比較すると定める。**この走査の発見は `横断リーフ` という見出し語に圧力をかける** — `yaml` がバックエンドを import する以上、この 7 つ目は「リーフ」という語が示すほど綺麗な層ではない。

architect がこの見出しを実形に合わせて改名するなら、`reverse-engineering-timestamp.md` の `analyzed.components` も同じ文字列へ同時に変えること。**片方だけ変えると、次回の `codekb-scope-diff` はこの走査のカバレッジを引き当てられない。** 改名するかしないかはどちらでもよいが、両方か、どちらもか、の 2 択である。

---

### Packages Found

- `scout` — binary + library — Rust edition 2024 / `rust-version = "1.97.1"` (`Cargo.toml:4-5`) — CLI。`Cargo.toml` に `[workspace]` セクションは無く、`git show HEAD:Cargo.lock` の `[[package]] name = "scout"` は 1 件。単一 crate・単一デプロイ単位

### Build System

- **Type**: Cargo
- **Config Files**: `Cargo.toml`、`Cargo.lock`、`clippy.toml`、`deny.toml`、`renovate.json`、`.config/nextest.toml`。`rustfmt.toml` は存在しない (rustfmt 既定設定)
- **Build Dependencies**: crate 内の依存は上の「モジュール依存の実形」が持つ。外部依存は直接 23 本 (`[dependencies]`) + dev 4 本 (`[dev-dependencies]`)、`Cargo.lock` に解決済みパッケージ 311 件 (HEAD 版)
- **Feature**: `js-rendering` (既定 off)。CDP/chromium 経路をコンパイルに含める

### APIs Discovered

- **CLI** — `src/lib.rs` の `clap` `Cli` — 6 サブコマンド (`src/tools/params.rs:31-44` の `enum Command` に `Search` / `Fetch` / `Research` / `RepoTree` / `RepoRead` / `RepoOverview` の 6 variant)。crate の外部 Rust API は `pub async fn run() -> ExitCode` の 1 本だけで、`Cargo.toml` の `unreachable_pub = "deny"` と `src/lib.rs:1-20` の `mod` 宣言 19 本がすべて非公開であることがそれを保つ
- **外向き HTTP クライアント** — `github` (REST v3)、`slack` (Web API 4 メソッド)、`brave` (Search API 1 エンドポイント)、`fetch` (任意の URL)。要求はすべて GET
- **JSON envelope** — `src/envelope.rs` の `SuccessEnvelope` / `ErrorEnvelope` / `ErrorPayload` (DR-0010)

### Frameworks & Libraries

この attempt で新たに読んだ 14 ファイルが直接使う外部 crate だけを挙げる。全一覧はストアの `technology-stack.md` が持つ (基準 `c8460b5`、`Cargo.toml` は無変更なので有効)。

| crate           | 使う場所                            | 用途                                                                       |
| --------------- | ----------------------------------- | -------------------------------------------------------------------------- |
| `reqwest`       | `src/body_limit.rs`、`src/retry.rs` | `Response::chunk()` による cap 付き読み出し、`is_connect`/`is_timeout` 判定 |
| `tokio`         | `src/signals.rs`、`src/token_source.rs`、`src/retry.rs`、`src/search/engine.rs` | signal、subprocess、`sleep`、`timeout`、`watch`   |
| `futures`       | `src/search/engine.rs`              | `stream::buffer_unordered(5)` による並列取得                               |
| `encoding_rs`   | `src/charset.rs`                    | 信頼できる検出とみなす 8 エンコーディングの判定                            |
| `fastrand`      | `src/rng.rs`                        | backoff のジッタ                                                           |
| `httpdate`      | `src/retry.rs`                      | `Retry-After` の HTTP-date 形式                                            |
| `tracing`       | 上記多数                            | `warn!` 構造化ログ                                                         |
| `serde`         | `src/search/engine.rs`              | `ResearchReport` / `FailedUrl` の `Serialize`                              |
| `tracing-test`  | `src/token_source.rs` の tests      | `logs_contain` による「ログに出ていない」の assertion                      |

### Test Coverage

- **Test Directories**: `tests/` (4 ファイル 1,833 行)、および `src/` 配下のテスト専用兄弟ファイル 47 本 (12,498 行)、実装ファイル内の inline `#[cfg(test)] mod tests { … }` ブロック 19 箇所
- **Test Frameworks**: `cargo-nextest` (ランナー)、`wiremock` (HTTP モック)、`tracing-test` (ログ assertion)、`tokio` の `test-util` (`start_paused` で時間を進める)
- **Coverage Config**: present。`cargo-llvm-cov` + `diff-cover --fail-under=95`。**PR イベントでのみ走る** (`coverage` job の `if: github.event_name == 'pull_request'`)

HEAD で測り直した量。すべて `src/` + `tests/` が測定範囲。

| 指標                      | 値             | 測定方法                                                                                                                    |
| ------------------------- | -------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| テスト ID (重複なし)      | 806            | `[T-…]` を抽出しカンマで分割、`sort -u` を通した値。出現数も 806 で、**同じ ID が 2 度書かれた箇所は 1 つも無い**            |
| テスト属性の宣言数        | 851            | `#[test]` 646、`#[tokio::test]` 191、`#[tokio::test(start_paused = true)]` 13、`#[tokio::test(flavor = "multi_thread")]` 1 |
| `#[ignore]`               | 1              | `src/fetch/cdp/cdp_integration_tests.rs:74` の `#[ignore = "requires chromium"]`。70 行目の同語は doc コメント             |

3 つともストアの値と一致する。

**`project.md` の DR-0014 の mandate が名指しする YAML 層の pin を実物で確認した。** 引用は `src/yaml.rs` の `[T-FC003..T-FC007, T-FC030..T-FC033]` の 9 本で、198-403 行の `#[cfg(test)] mod tests` に 9 本すべてが実在する。同ブロックのテストは全 15 本 (`#[test]` 15、ブラケット付き ID 15) で、mandate が挙げない 6 本 (`T-FC012`、`T-FC013`、`T-FC100`〜`T-FC103`) が cap と escape の順序を押さえる。**`T-FC013` は「ADR-0014 が述べていない 2 点」を意図的に pin していると doc コメントに書く** — ESC などの C0 制御文字は escape 対象に入らないので値が borrow されたまま出ること、および出力された scalar が YAML 1.2 の c-printable から外れるバイトを運ぶので厳格なパーサが拒否すること。ADR を後から変えるとき、この 2 つの挙動が黙って変わらないための仕掛けである。

### Code Quality Indicators

- **Linting**: `Cargo.toml` の `[lints.clippy]` 13 件と `[lints.rust]` 2 件、`clippy.toml` の `disallowed-methods`。CI は通常 feature と `--all-features` の 2 回、`cargo clippy --all-targets -- -D warnings` を走らせる
- **Formatting**: `cargo fmt -- --check` (rustfmt 既定設定)
- **CI/CD**: `.github/workflows/` に 4 本。`ci.yml` は 3 job・27 step (`test` 12、`coverage` 6、`security` 9)。3 job とも `permissions: contents: read` と `persist-credentials: false` を持つ。job 外の `env:` が `SCOUT_NETWORK_TESTS: "1"` を立てる
- **Documentation**: `README.md` / `README.ja.md` の 2 言語、`docs/decisions/` に DR 28 本 (全て `status: "accepted"`) + README 索引、`docs/audit/` に 14 本。doc コメントは「何をするか」ではなく「なぜこの値か」「何を却下したか」を書く

**この attempt が読んだ 14 ファイルは、その doc コメント文化の追加実例を 6 つ出す。** ストアの `code-structure.md` の `### doc コメントが却下を残す` に足せる。

| 所在                                              | 却下または理由の内容                                                                                                       |
| ------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| `src/body_limit.rs:1-9` (module doc)              | 置き場の規則そのもの — 2 バックエンド以上が共有する cap はここ、1 つだけのものはそのバックエンドに残す。実例を 3 つ名指しする |
| `src/body_limit.rs:63-72`                         | cap が **decode 後** のバイトに掛かること、圧縮時は `content_length()` が `None` になり事前検査が無効化して chunk ループだけが生きること、その代償 (展開後が大きい正当なページを弾く) |
| `src/retry.rs:16-19`                              | `MAX_RETRY_AFTER_SECS = 300` を選んだ理由 (端末で待つ人間の忍耐)                                                            |
| `src/retry.rs:36-40`                              | `is_transient_decode` が source chain を歩く理由 — reqwest 0.13 は hyper の `UnexpectedEof` を `is_decode() == true` で出すので、bool だけでは serde のスキーマ不一致と区別できない |
| `src/rng.rs:24-27`                                | `SeededRng` に `Mutex` を使う理由と、`.clone()` にしたときに起きたこと (毎回同じ値が返る)                                  |
| `src/markdown.rs:84-91`                           | 切る位置を行境界にする理由 — 行中で切ると `-------` が `---` になり、後続の note がそれを終端して本文に無い区切り線が生まれる |

**lint 抑制は 15 個。** 測定パターンは `grep -rnE '#!?\[(cfg_attr\(.*)?(allow|expect)\(' src tests --include='*.rs'`。内訳は `#[expect(...)]` 8、`#[cfg_attr(not(feature = "js-rendering"), allow(...))]` 6、`#![allow(dead_code)]` 1 (`tests/common/mod.rs:17`)。**`src/` に素の `#[allow(` は 1 件も無い。** 6 件の `cfg_attr` allow は `src/fetch/cdp.rs` に 3、`src/fetch/cdp/launch.rs` に 2、`src/fetch/cdp/proxy.rs` に 1 で、いずれも `js-rendering` 無効時の dead_code / unused_imports を黙らせるためのもの。3 つともストアの値と一致する。

### Technical Debt Signals

**一般的な負債マーカーは実質 0 である。** `grep -rnE '(TODO|FIXME|HACK|XXX)' src/ tests/ --include='*.rs'` は 5 hit を返すが、5 件とも負債マーカーではない。1 件は `src/markdown.rs:220` の doc コメントが `# TODO` というコメント行を fence 内の例として挙げているもの、4 件は Slack テストの fixture ユーザー ID `UXXX` (`src/slack/mention/mention_tests.rs:111,112`、`src/slack/format/resolve_messages_tests.rs:59,64`)。

未着手の判断として残るのは 2 件で、どちらもストアが既に記録している。この走査は新しい負債を見つけていない。

- `src/fetch/converter.rs` の 3,131 行 — 切り出す単位の判断が未着手 (監査文書 E-4)
- `with_clock` / `with_rng` の 4 重複 — 共通化は実測のうえ棄却済みで、再着手の条件が closed issue #310 の中にしか無い (監査文書 E-2)

**この走査が新しく足す観察が 1 つある。** `yaml → search` の循環辺は負債ではなく、両側に理由が書かれた意図的な設計である。ただし **その意図を守る仕掛けは何も無い。** `Cargo.toml` の lint にも CI にも循環や層の向きを検査するものは無く、この辺が 2 本目に増えても機械は何も言わない。ストアが「循環は無い」と書けてしまったのは、この辺が誰にも見えていなかったからである。

---

## 上書きした主張

### 1. 循環と、リーフからバックエンドへの import

上の「モジュール依存の実形」が全体を持つ。証拠は `src/yaml.rs:9`、`src/search/engine.rs:18` と、両側の doc コメント (`src/search/engine.rs:20-22`、`src/yaml.rs:130-137`)。直す箇所は 3 つ。

### 2. `src/` のファイル分類は 50 / 45 ではなく 48 / 47

ストアの `code-structure.md` の `## ファイル分類` は「実装ファイル 50 / テスト専用ファイル 45 (11,538 行)」と書く。**HEAD で数え直すと 48 / 47 (12,498 行) である。**

`src/` は `c8460b5` から 1 バイトも変わっていないので、これはコードの変化ではなくストアの数え方の誤りである。**差の内訳が算術で確定する。**

| 値                                    | 行数   |
| ------------------------------------- | ------ |
| ストアのテスト専用 45 本              | 11,538 |
| `src/test_support.rs`                 | 900    |
| `src/tools/test_helpers.rs`           | 60     |
| 合計                                  | 12,498 |

**12,498 は測定値と 1 行も違わない。** つまりストアは `src/test_support.rs` と `src/tools/test_helpers.rs` の 2 本を実装ファイル側に数えている。どちらも `#[cfg(test)]` の下にしか存在しない。

- `src/test_support.rs` — `src/lib.rs:16-17` の `#[cfg(test)] mod test_support;` 配下
- `src/tools/test_helpers.rs` — `src/tools.rs` の `#[cfg(test)] mod test_helpers;` 配下

**ストアは自分自身と食い違っている。** 同じ `code-structure.md` のディレクトリ図が `src/test_support.rs     cfg test only - test ID scanner and helpers` と書き、`## サイズ分布` の表も `src/test_support.rs | 900 | #[cfg(test)] 専用` と書く。分類表だけがこの 2 本を実装側に置いている。

判定方法: `#[cfg(test)] mod <name>;` と `#[cfg(all(test, feature = "js-rendering"))] mod <name>;` の宣言を `src/` 全 95 ファイルから抽出し、宣言元ファイルの兄弟パスへ解決した。`src/lib.rs` は crate root なので兄弟ディレクトリは `src/` になる点に注意する (`src/lib/` ではない)。前者が 44 件、後者が 3 件 (`src/fetch/cdp/cdp_integration_tests.rs`、`src/fetch/cdp/launch/cdp_launch_tests.rs`、`src/fetch/cdp/launch/ws_url_parse_tests.rs`) で、合わせて 47 件。95 − 47 = 48。行数は 12,498 + 14,861 = 27,359 で、`src/` 全体の実測と一致する。

**`analyzed.paths` に副作用が 1 つある。** ストアの 21 パスに含まれる `src/tools/test_helpers.rs` は、実装ファイルではなくテスト専用ファイルである。architect が「深く読んだ実装ファイル」を数えるときはこれを除く。

### 3. `src/lib.rs` の `mod` 宣言は 20 本ではなく 19 本

ストアの `code-structure.md` は「`src/lib.rs` の `mod` 宣言は 20 本 (`grep -c '^mod ' src/lib.rs` で測定)」と書く。**そのコマンドは 20 を返すが、20 本目はモジュール宣言ではない。**

`grep -nE '^\s*(pub|pub\(crate\))? *mod ' src/lib.rs` が返す 20 行のうち、1-20 行目にある 19 行が `mod <name>;` のトップレベル宣言で、20 行目にあたるのは **323 行目の `mod tests {`** — ファイル末尾の inline `#[cfg(test)] mod tests` ブロックの開始行である。`^mod ` というパターンは宣言 (`;` 終端) とブロック (`{` 開始) を区別しないので、両方に当たる。

正しい値は次のとおり。

| 数える対象                                          | 値 |
| --------------------------------------------------- | -- |
| トップレベルの `mod <name>;` 宣言 (1-20 行目)      | 19 |
| うちリリースビルドに入るもの                        | 18 |
| inline `#[cfg(test)] mod tests { … }` ブロック      | 1  |

`test_support` が `#[cfg(test)]` 配下 (16-17 行目) なので、リリースビルドに入るのは 18 本である。ストアはこの数を 19 本と書いている。

**すべて非公開である点は変わらない。** 20 行のいずれにも `pub` は付かず、crate 外へ出るのは `pub async fn run() -> ExitCode` の 1 本だけである。

---

## 決着した 2 件

どちらも自分で検証した。ディスパッチの記述を引き写していない。

### zizmor の finding は CI job を落とさない

**確定。** 5 段の連鎖を 1 つずつ確認した。

| 段 | 確認したもの                                                                                    | 結果                                                                                                                   |
| -- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| 1  | `.github/workflows/zizmor.yml:29-30`                                                            | `uses: zizmorcore/zizmor-action@3dc1ecc9bcb9e94e9b2c709687979e1298497054 # v0.6.2`。**`with:` ブロックが無い** = 全 input が既定値 |
| 2  | 同 SHA の `action.yml` (`gh api` で取得)                                                         | `advanced-security` は `required: false` / `default: "true"`                                                           |
| 3  | 同 SHA の `action.sh:58-60`                                                                     | `[[ "${GHA_ZIZMOR_ADVANCED_SECURITY}" == "true" ]]` のとき `arguments+=("--format=sarif")`                             |
| 4  | `zizmorcore/zizmor` の `docs/usage.md:597-598`                                                  | 「If you use `--format=sarif`, `zizmor` will **not** use exit codes 11 and above.」exit code 表の 0 行も「Successful audit; no findings to report (or SARIF mode enabled).」 |
| 5  | 同 `action.sh:114,122`                                                                          | `exitcode="${PIPESTATUS[0]}"` … `exit "${exitcode}"`。zizmor の終了コードをそのまま返す                               |

強制点は SARIF のアップロード先である。`action.yml` の 2 番目の step が `github/codeql-action/upload-sarif@7188fc363630916deb702c7fdcf4e481b751f97a # v4.37.1` を `if: ${{ inputs.advanced-security == 'true' }}` で走らせ、`category: zizmor` で code scanning alert として登録する。**リポジトリ内の裏付けもある** — `.github/workflows/zizmor.yml:19-22` の job が `permissions: security-events: write` を持つ。job を赤くするだけなら要らない権限である。

**正確な言い方は「finding は job を落とさない」であって「zizmor は job を落とさない」ではない。** exit code 1 (audit 中のエラー) と 2 (引数解析の失敗) は SARIF モードでも抑制されないので、zizmor 自身が壊れれば job は落ちる。

範囲の限界を 1 つ残す。`docs/usage.md` は `zizmorcore/zizmor` の既定ブランチから取得したもので、action が入れる zizmor 本体のバージョンは `version: latest` の既定に従うため、この文書と同一バージョンである保証は無い。

### `cargo-nextest` の `retries` 既定は 0 で、`final-status-level = "flaky"` は現状発火しない

**確定。** リポジトリ側と nextest 本体側の両方を確認した。

| 確認したもの                                       | 結果                                                                                            |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `.config/nextest.toml` 全 10 行                    | `[profile.default]` にも `[profile.ci]` にも `retries` の行は無い (`grep -n retries` が exit 1) |
| `.github/workflows/ci.yml` の `cargo nextest run` 2 本 (53 行、58 行) | どちらも `--retries` を渡さない。`--profile ci` のみ                                |
| `NEXTEST_RETRIES` 環境変数と `.cargo/`             | `git grep -nI 'NEXTEST_RETRIES' -- ':!aidlc/'` は exit 1 (0 件)。`.cargo/` ディレクトリも存在しない |
| `cargo-nextest 0.9.143` バイナリに埋め込まれた既定設定 | `strings /opt/homebrew/bin/cargo-nextest \| grep -B3 '^retries = '` が `retries = 0` と、その直下の `flaky-result = "pass"` を返す。同じ埋め込みブロックの冒頭に `# This is the default config used by nextest. It is embedded in the binary` とある |

したがってどのテストも再試行されず、`final-status-level = "flaky"` の flaky 行は出ない。**この設定は「今は効かないが、`retries` を入れた日に効く」ものとして読むのが正しい。**

範囲の限界を 1 つ残す。既定値を読んだのはローカルの `cargo-nextest 0.9.143` である。CI は `taiki-e/install-action` で入れるので、バージョンが違えば既定値も違いうる。ただし `retries = 0` は nextest の既定として長く安定しており、`.config/nextest.toml` と CI の両方に上書きが無いという事実はバージョンに依らない。

---

## Handoff Summary

- **Intent-relevant finding**: **`src/yaml.rs:9` の `use crate::search::engine::MAX_PAGE_BYTES;` が本番モジュールグラフの唯一の循環辺で、これ 1 本が 2 本の閉路 (`search ↔ yaml`、`fetch → yaml → search → fetch`) を閉じている。** この辺を除けば残る 55 辺は非巡回になる。両側に派生の理由が doc コメントで書かれている (`src/search/engine.rs:20-22`、`src/yaml.rs:130-137`) ので、直すべきはコードではなくストアの記述である。層の規則の正しい言い方は「循環が無い」ではなく「文書化された派生定数の辺 1 本を除いて非巡回であり、その向きを検査する仕掛けは無い」。

- **Risks / follow-up**:
  1. **同じ誤った主張が 3 ファイル・9 箇所にある。** `architecture.md:7,53,55`、`dependencies.md:56,67,69`、`component-inventory.md:15,27,112`。全数列挙のコマンドと食い違いの内訳は本文の「ストアが直すべき箇所は 9 つある」が持つ。`architecture.md` と `dependencies.md` の Mermaid 図自体 (`LEAF` から出る矢印が無い) も実形と食い違う。**1 箇所だけ直すと残り 8 箇所が古いまま残る。**
  2. **`component-inventory.md` の `**依存**` 欄は 7 つとも実形と合わない。文だけでなくデータが壊れている。** 欠落は `tools` 3・`fetch` 1・`github` 4・`slack` 3・`brave` 3・`search` 2・横断リーフ 1。加えて **存在しない辺を 3 つ挙げている** — `fetch → retry`、`slack → markdown`、`search → envelope` はいずれも `src/` に 1 件も無い。個別に直すのではなく、本文の本番辺の表 (10 起点・56 辺) から 7 欄を組み直すこと。`## tools` の「逆向きの import は持たない」は本番限定の主張として書き直す (テストでは `slack → tools` が立つ)。
  3. **`code-structure.md` の `## ファイル分類` の 50 / 45 を 48 / 47 (12,498 行) へ直す。** ストアが `src/test_support.rs` (900) と `src/tools/test_helpers.rs` (60) を実装側に数えている。差 960 行が算術で一致する。同じファイルのディレクトリ図とサイズ分布表は既に「`#[cfg(test)]` 専用」と書いており、ストアは自分と食い違っている。
  4. **`analyzed.paths` の `src/tools/test_helpers.rs` はテスト専用ファイルである。** 「深く読んだ実装ファイル」を数えるときは除く。
  5. **`code-structure.md` の「`src/lib.rs` の `mod` 宣言は 20 本」を 19 本へ、「リリースビルドに入るのは 19 本」を 18 本へ直す。** 20 本目として数えられているのは `src/lib.rs:323` の inline `#[cfg(test)] mod tests {` ブロックで、`^mod ` というパターンが宣言 (`;`) とブロック (`{`) を区別しないために入り込んでいる。
  6. **`code-quality-assessment.md` の zizmor と nextest の未確認事項を落とせる。** 両方この走査で決着した。ただし zizmor 側は「finding は job を落とさない」であって「zizmor は job を落とさない」ではない (exit 1 / 2 は落ちる)。nextest 側は「今は効かないが `retries` を入れた日に効く設定」と書く。
  7. **測定の限界を 3 つ引き継ぐこと。** (a) `use crate::` を見るグラフには `src/lib.rs` 発の 3 辺 (`envelope` / `signals` / `tools`) が入らず、`signals` は 1 度も現れない。(b) `use` を経由しない実コードのパス参照は `src/tools/builder.rs:80,94` の `crate::USER_AGENT` 2 行だけで、辺 `tools → (crate root)` 1 本を足す。(c) 本番 55 辺の非巡回性はトップレベルモジュール粒度での話であり、モジュール内部のファイル間循環は測っていない。
  8. **`Cargo.lock` に未コミットの変更がある。** 14 パッケージの transitive な version/checksum が動いている。`dependencies.md` を書くときは `git show HEAD:Cargo.lock` を読むか、作業ツリーとの差を明記すること。
  9. **ストアの基準からの差分は `.github/workflows/ci.yml` の 2 行だけである。** `taiki-e/install-action` を v2.85.11 (`7f4eb899…`) から v2.86.3 (`5b4d68e2…`) へ、48 行目と 103 行目の 2 箇所。`src/` は 1 バイトも動いていないので、`src/` について測り方が正しい既存の数値はそのまま持ち越せる。
  10. **`analyzed.components` の 7 語と `component-inventory.md` の `##` 見出しは同時にしか動かせない。** この走査の発見は `横断リーフ` という語に圧力をかける。改名するなら両方、しないなら両方。片方だけ変えると次回の `codekb-scope-diff` がこの走査のカバレッジを引き当てられない。
  11. **`fingerprint` はこの 35 パス集合に対して新しく mint すること。** snapshot が渡した `git:9ab12e551ead0c2b36a3b3b83634c9783030fd2a` は *source* fingerprint であって path 集合の mint ではなく、ストアの `498e1629…` は旧 21 パスに対する mint である。**どちらもこの走査の値ではない。** ストアの `reverse-engineering-timestamp.md` は、前回の走査担当が tree と commit を比べて「不一致」と誤読した経緯に 1 節を費やしている。同じ混同を繰り返さないため、mint した値の由来 (35 パスに対する `codekb-scope-diff --mint`) を 1 行添えること。
  12. **読みの 2 段階を `reverse-engineering-timestamp.md` へ持ち込むこと。** 35 パスのうち 14 はこの attempt で端から端まで読み、残る 21 は `git diff c8460b5..HEAD` が無変更を示したうえでストアの記述を測り直したものである。35 を平らに並べると、次回の走査は 21 パスも `ef2fbc9` で深く読まれたものとして扱う。35 を `analyzed` に入れること自体は正当だが (ディスパッチがその方法を指示し、`src/` はバイト単位で同一)、段階の注記は intent 内のこの文書ではなく耐久側の artifact に載らなければ次回に届かない。
