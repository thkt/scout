# Code Structure — scout

## ディレクトリ構成

アプリケーションソース面は `src/` 95 ファイル (27,359 行)、`tests/` 4 ファイル (1,833 行)、リポジトリ直下の設定 6 ファイル、`.github/`、`docs/decisions/`、`docs/audit/` である。

```
scout/
+-- src/                    95 files 27359 lines - implementation plus test-only siblings
|   +-- main.rs             6 lines - calls run
|   +-- lib.rs              CLI surface - clap Cli / tracing / signals / JSON dispatch
|   +-- tools.rs  tools/    command dispatch / Scout DI / params / config / errors
|   +-- fetch.rs  fetch/    url validate / download / extract / convert / cdp
|   +-- github.rs github/   REST v3 client / encoding / format / wire types
|   +-- slack.rs  slack/    Web API client / url / mention / format
|   +-- brave/              search client and response types
|   +-- search/             research orchestration and language handling
|   +-- classify.rs envelope.rs retry.rs body_limit.rs markdown.rs yaml.rs
|   +-- redacted.rs clock.rs rng.rs token_source.rs charset.rs signals.rs
|   +-- test_support.rs     cfg test only - test ID scanner and helpers
+-- tests/                  4 files 1833 lines - integration level
+-- docs/decisions/         28 accepted Decision Records plus README index
+-- docs/audit/             14 audit documents
+-- .github/workflows/      ci.yml release.yml zizmor.yml label-from-issue.yml
+-- Cargo.toml clippy.toml deny.toml renovate.json .config/nextest.toml
```

<!-- Text fallback: src holds one flat entry pair (main.rs and lib.rs) plus six backend module trees (tools, fetch, github, slack, brave, search) and twelve cross-cutting leaf files; tests holds four integration files; docs carries the decision records and audit documents; the repository root carries the six build and policy configs. -->

`aidlc/`・`.claude/`・`target/`・`workspace/` はこのツリーに含めない。除外の理由と根拠は `reverse-engineering-timestamp.md` の `## アプリケーションソース外として除外した領域` が持つ。

## ファイル分類

`src/` のファイルは 2 種に分かれ、`tests/` が 3 種目になる。テストの置き場が 3 通りあることがこのリポジトリの特徴で、混同すると数え間違いが起きる。

| 種別               | 数             | 説明                                                                                                                 |
| ------------------ | -------------- | -------------------------------------------------------------------------------------------------------------------- |
| 実装ファイル       | 48 (14,861 行) | 本体。うち 18 本は末尾に inline の `#[cfg(test)] mod tests { … }` ブロックを持つ                                     |
| テスト専用ファイル | 47 (12,498 行) | `src/` 配下に置かれ、`#[cfg(test)] mod <name>;` 形式の宣言 47 本から参照される兄弟ファイル                           |
| 統合テスト         | 4 (1,833 行)   | `tests/` 配下。`cli_integration.rs` 447、`exit_code_contract.rs` 271、`output_injection.rs` 784、`common/mod.rs` 331 |

**この分類は先行ストアの「実装 50 / テスト専用 45 (11,538 行)」を上書きしたものである。** `src/` は先行ストアの基準 commit から 1 バイトも動いていないので、これはコードの変化ではなく数え方の誤りである。**差の内訳が算術で確定する** — 11,538 + 900 (`src/test_support.rs`) + 60 (`src/tools/test_helpers.rs`) = 12,498 で、測定値と 1 行も違わない。先行ストアはこの 2 本を実装ファイル側に数えていた。どちらも `#[cfg(test)]` の下にしか存在しない。

- `src/test_support.rs` — `src/lib.rs` の `#[cfg(test)] mod test_support;` 配下
- `src/tools/test_helpers.rs` — `src/tools.rs` の `#[cfg(test)] mod test_helpers;` 配下

**先行ストアは自分自身と食い違っていた。** 同じファイルのディレクトリ図が `src/test_support.rs     cfg test only` と書き、サイズ分布表も `#[cfg(test)] 専用` と書いていた。分類表だけがこの 2 本を実装側に置いていた。

判定方法: `#[cfg(test)] mod <name>;` と `#[cfg(all(test, feature = "js-rendering"))] mod <name>;` の宣言を `src/` 全 95 ファイルから抽出し、宣言元ファイルの兄弟パスへ解決した。`src/lib.rs` は crate root なので兄弟ディレクトリは `src/` になる (`src/lib/` ではない)。前者が 44 件、後者が 3 件で合わせて 47 件。95 − 47 = 48。行数は 12,498 + 14,861 = 27,359 で `src/` 全体の実測と一致する。

### 3 つの数を混同しないこと

テストの置き場をめぐる数が 3 つあり、どれも別のものを数えている。

| 数  | 数える対象                                           | 注意                                                                                                                               |
| --- | ---------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| 47  | `#[cfg(test)] mod <name>;` の兄弟ファイル宣言 (全形) | テスト専用ファイル 47 本と 1 対 1 で対応する                                                                                       |
| 7   | そのうち名前がちょうど `tests` である宣言            | 残る 40 本は `mod <name>_tests;` の形。**先行ストアはこの 7 を「テスト専用 45 本を参照する宣言」と書いており、対応が付かなかった** |
| 19  | inline の `#[cfg(test)] mod tests { … }` ブロック    | うち 18 個は実装ファイル、1 個は `src/test_support.rs` (テスト専用ファイル) にある                                                 |

**19 は 26 ではない。** 26 は `mod tests` という文字列を含むファイル数で、そのうち 7 本は兄弟ファイルを指す宣言である。`docs/audit/2026-08-11-rust-code-assessment.md` の実測値表がこの 2 形を「26 ファイルが inline `mod tests` を含み」と一括りにしているため、監査文書を無批判に引くと 26 が伝播する。

**`src/lib.rs` の inline ブロックは 19 個の内数である。** 別枠に数えると 20 になる。

テスト専用ファイルへ切り出すか inline に残すかの基準は行数ではない。`.claude/rules/CONVENTIONS.md` が「1 ファイルのテストが 2 つ以上の関心を持ったら分ける。ファイル名が中身を言い表せなくなった時点で、関心が 2 つ以上ある」と定める。実例が `src/fetch/cdp/launch/` で、browser_binary/browser_request/cdp_launch/ws_url_parse の 4 関心に 4 ファイルが対応する。`src/fetch/cdp/launch.rs` はこの 4 本を `#[cfg(test)] mod <name>_tests;` の並びで宣言しており、**実装ファイル自体は 1 本のまま残している。** 分割の単位はテスト側にだけ入っている。

## モジュール宣言と可視性

**`src/lib.rs` のトップレベル `mod` 宣言は 19 本で、うち 18 本がリリースビルドに入る。**

| 数える対象                                     | 値  |
| ---------------------------------------------- | --- |
| トップレベルの `mod <name>;` 宣言              | 19  |
| うちリリースビルドに入るもの                   | 18  |
| ファイル末尾の inline `#[cfg(test)] mod tests` | 1   |

**先行ストアの「20 本」は測定パターンの取りこぼしである。** `grep -c '^mod ' src/lib.rs` は 20 を返すが、20 本目は `src/lib.rs` 末尾の inline `#[cfg(test)] mod tests { … }` ブロックの開始行である。`^mod ` というパターンは宣言 (`;` 終端) とブロック (`{` 開始) を区別しない。リリースビルドに入るのが 18 本なのは `test_support` が `#[cfg(test)]` 配下にあるためで、先行ストアはこれを 19 本と書いていた。

**すべて非公開である点は変わらない。** 19 宣言のいずれにも `pub` は付かない。

**到達できない `pub` は 0 である。** `Cargo.toml` の `[lints.rust]` にある `unreachable_pub = "deny"` がビルドで落とす。crate 外へ出るのは `pub async fn run() -> ExitCode` の 1 つだけで、これが「契約面は Rust API ではなく CLI 表面」という設計を機械的に保証している (`architecture.md` の `## アーキテクチャスタイル` 参照)。

内側では可視性が段階的に使い分けられる。読んだ範囲での実例:

| 形                          | 実例                                                    |
| --------------------------- | ------------------------------------------------------- |
| `pub(crate)`                | crate 全体へ出す型と関数                                |
| `pub(super)`                | 親モジュールにだけ出す補助                              |
| `pub(in crate::fetch::cdp)` | `src/fetch/cdp/proxy.rs` の `spawn_ssrf_proxy` の再輸出 |

**`pub(crate)` を選んだ理由が doc コメントに残る例がある。** `src/search/engine.rs` の `MAX_PAGE_BYTES` は「`yaml::MAX_FIELD_BYTES` が同じページ予算からフィールド上限を導くため」と書く。この 1 行が crate 内の唯一の循環辺の由来でもある (`architecture.md` の `## モジュール依存の実形`)。

**可視性の分布を素朴な grep で数えると落ちる。** `src/fetch/converter.rs` の実装 985 行で「crate 外または親モジュールへ出ている項目」は 10 個 (`pub(crate) fn` 6 + `pub(super) fn` 1 + `pub(crate) const` 2 + `pub(crate) struct` 1) だが、列 0 に錨を打った `^pub\(` は 4 しか返さない。`impl FetchResult` の 6 メソッドはインデントされているためである。

最小可視性の決め方 (手順と注意点) は `docs/audit/2026-08-11-rust-code-assessment.md` の B-6 が持つ。

## コードパターンと規約

規約の本体はいずれも一次ソース側にあり、`.claude/rules/CONVENTIONS.md` はその索引として働く。以下は「どのパターンがどこで決まっているか」の対応表である。

| パターン                           | 一次ソース                                           | 内容の要点                                                                                                                                                                        |
| ---------------------------------- | ---------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| テスト ID の採番と prefix 設計     | `src/test_support.rs` の crate doc                   | `[T-<PREFIX><NNN>]` 形式。prefix は「テスト対象」を指すので 1 prefix が複数ファイルに跨る。番号は prefix 内で一意でファイル単位ではない。引用時はブラケットを外して定義と区別する |
| 重複採番の検出                     | `src/test_support.rs` の `scan_test_id_violations`   | リポジトリ自身のテストが違反を落とす                                                                                                                                              |
| 上限なし read の禁止               | `clippy.toml` の `disallowed-methods`                | `reqwest::Response::{text, bytes, json}` を禁じ、各 `reason` に代替関数名 (`body_limit::read_body_capped` / `read_body_snippet`) を書く                                           |
| 共有する定数とヘルパーの置き場     | `src/body_limit.rs` の module doc                    | 2 バックエンド以上が共有する cap はここへ、1 バックエンド専用の cap はそのバックエンドに残す。実例を 3 つ名指しする                                                               |
| DR からコードを指す参照の形        | DR-0028                                              | 行番号ではなくシンボル名で指す                                                                                                                                                    |
| コメントは英語で書く               | `.github/workflows/ci.yml` の Comment language check | 対象はコメント、doc comment、テスト名、assertion message。例外はバイト列の注釈で、注釈自体は英語・引用部分だけ原語                                                                |
| テストは関心ごとにファイルを分ける | `.claude/rules/CONVENTIONS.md`                       | 行数は基準にしない                                                                                                                                                                |

### doc コメントが却下を残す

このリポジトリの doc コメントは「何をするか」ではなく **「なぜこの値か」「なぜこの順序か」「何を却下したか」** を書く。却下を測った数値がコメントに残るのが特徴である。読んで確認した実例:

- `src/fetch/cdp/launch.rs` の `resolve_browser_binary` — `OnceLock` キャッシュを外した理由 (テスト分離を壊し、プロセス生存期間で結果を固定した。`which` 数回の 1-5ms は後続の約 2 秒の chromium レンダリングに対して無視できる)
- `src/fetch/cdp/launch.rs` の `PGROUP_SIGTERM_GRACE` — 50ms という値の根拠
- `src/github/types.rs` の `ContentsPayload::Directory` — `Vec<IgnoredAny>` でなく素の `IgnoredAny` にしたときに起きたこと
- `src/fetch.rs` の `SPA_ROOT_IDS` — 単引用符形式を「見落としではなく」外した理由
- `src/lib.rs` の `init_tracing` — `expect` を fallback にしない理由
- `clippy.toml` 冒頭 — テストではなく lint で守る判断の理由 (「どちらの caller も cap を観測可能にしない。だからビルドを落とす」)
- `src/tools/config.rs` の `DEFAULT_GITHUB_TIMEOUT_SECS` — 180 秒を選んだ根拠、それが retry 予算 (約 279 秒) の下に収まる理由、切り捨てられるケース (約 186 秒) までを数値で書く
- `src/tools/config.rs` の `from_env_with` — env 読み取りを注入可能にした理由 (`unsafe { std::env::set_var(...) }` が `unsafe_code = "forbid"` で使えない)
- `src/tools/config.rs` の `read_env_raw` — `VarError::NotUnicode` を「未設定」に潰さず `UsageError` へ落とす理由
- `src/slack/client.rs` の `SlackFetchOutcome` — `String` 返しでは cap のヒットが呼び出し側に見えないという設計理由
- `src/slack/client.rs` の `USER_TOKEN_PREFIX` — bot/app-level/workflow の各トークンを弾く理由と、それを検証した日付 (2026-06)

**この走査が読んだ 12 の横断リーフから、同じ文化の実例が 6 つ増えた。**

| 所在                                      | 却下または理由の内容                                                                                                                                       |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/body_limit.rs` の module doc         | 置き場の規則そのもの — 2 バックエンド以上が共有する cap はここ、1 つだけのものはそのバックエンドに残す。実例を 3 つ名指しする                              |
| `src/body_limit.rs` の `read_body_capped` | cap が **decode 後** のバイトに掛かること、圧縮時は `content_length()` が `None` になり事前検査が無効化して chunk ループだけが生きること、その代償         |
| `src/retry.rs` の `MAX_RETRY_AFTER_SECS`  | 300 を選んだ理由 (端末で待つ人間の忍耐)                                                                                                                    |
| `src/retry.rs` の `is_transient_decode`   | source chain を歩く理由 — reqwest 0.13 は hyper の `UnexpectedEof` を `is_decode() == true` で出すので、bool だけでは serde のスキーマ不一致と区別できない |
| `src/rng.rs` の `SeededRng`               | `Mutex` を使う理由と、`.clone()` にしたときに起きたこと (毎回同じ値が返る)                                                                                 |
| `src/markdown.rs` の切り詰め位置          | 行境界で切る理由 — 行中で切ると `-------` が `---` になり、後続の note がそれを終端して本文に無い区切り線が生まれる                                        |

`#[expect(...)]` には必ず `reason` が付く。抑制の総数と測定範囲は `code-quality-assessment.md` が持つ。

### 本番経路の panic

`unsafe_code = "forbid"`。`expect` は 2 箇所で確認され、両方に理由コメントがある — `src/envelope.rs` の `to_json_line` の `expect("envelope is Serialize")` と、`src/lib.rs` の `"scout=info".parse().expect("static directive is valid")`。

## サイズ分布

行数は分割の基準ではないが、規約から外れた 1 本を特定するには使える。**内訳の列が空の行は「テストを含まない」という主張ではなく「内訳を測っていない」という意味である。** 内訳を測った行は 2 値が総行数へ足し合わさる — 属性行 `#[cfg(test)]` はテスト側に、その手前の空行は実装側に数えている。

| ファイル                 | 行数  | 内訳                                                                                                                                                                                                                                                                                                 |
| ------------------------ | ----- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/fetch/converter.rs` | 3,131 | 実装 985 + `#[cfg(test)] mod tests` 2,146                                                                                                                                                                                                                                                            |
| `src/test_support.rs`    | 900   | `#[cfg(test)]` 専用。inline `mod tests` を末尾に持つ                                                                                                                                                                                                                                                 |
| `src/markdown.rs`        | 671   | 実装 281 + `#[cfg(test)] mod tests` 390                                                                                                                                                                                                                                                              |
| `src/lib.rs`             | 639   | 末尾に inline `#[cfg(test)] mod tests` を持つ                                                                                                                                                                                                                                                        |
| `src/slack/client.rs`    | 620   | **全 620 行が実装。** inline `mod tests` を持たない。テストは兄弟 2 ファイル計 1,152 行 (`src/slack/client/constructor_tests.rs` 96、`src/slack/client/http_tests.rs` 1,056) にあり、`src/slack/client.rs` 末尾の `#[cfg(test)] mod constructor_tests;` と `#[cfg(test)] mod http_tests;` が参照する |
| `src/tools/repo.rs`      | 548   | 未測定                                                                                                                                                                                                                                                                                               |
| `src/tools/params.rs`    | 540   | 未測定                                                                                                                                                                                                                                                                                               |
| `src/tools/config.rs`    | 477   | 実装 195 + `#[cfg(test)] mod tests` 282                                                                                                                                                                                                                                                              |
| `src/fetch.rs`           | 462   | 未測定                                                                                                                                                                                                                                                                                               |
| `src/yaml.rs`            | 403   | 実装 197 + `#[cfg(test)] mod tests` 206 (テスト 15 本)                                                                                                                                                                                                                                               |
| `src/search/engine.rs`   | 245   | **全 245 行が実装。** テストは兄弟の `src/search/engine/tests.rs`                                                                                                                                                                                                                                    |
| `src/token_source.rs`    | 223   | 実装 104 + `#[cfg(test)] mod tests` 119                                                                                                                                                                                                                                                              |
| `src/retry.rs`           | 160   | **全 160 行が実装。** テストは兄弟の `src/retry/tests.rs`                                                                                                                                                                                                                                            |
| `src/body_limit.rs`      | 100   | **全 100 行が実装。** テストは兄弟の `src/body_limit/tests.rs`                                                                                                                                                                                                                                       |
| `src/search.rs`          | 6     | `mod` 宣言と `pub(crate) use lang::Lang;` のみ                                                                                                                                                                                                                                                       |
| `src/main.rs`            | 6     | `scout::run()` を呼ぶだけ                                                                                                                                                                                                                                                                            |

### `src/fetch/converter.rs` の関心は 6 群ではなく 9 つに割れる

**上書き。** 先行資料 (`docs/audit/2026-08-11-rust-code-assessment.md` の E-4、および `.claude/rules/CONVENTIONS.md` の「テストは関心ごとにファイルを分ける」節) は、このファイルのテスト ID の並びを「表/pre とフェンス/リンクとアンカー/script と style の抑制/frontmatter/リストの 6 群」と書く。テスト 79 本すべてを割り当てた結果、**この 6 群という枠は 2 点で成り立たない。**

数値の性質を分けて記録する。**79 という本数と 79 という ID 数は機械測定である。9 関心と 26 区間は分類であり、測定ではない。**

| 値               | 性質     | 測定範囲                                                                                                                                                                                              |
| ---------------- | -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| テスト属性 79    | 機械測定 | inline `mod tests` ブロック 2,146 行。すべて `#[test]` で `#[tokio::test]` は 0                                                                                                                       |
| テスト ID 79     | 機械測定 | 同上。`sort -u` 後の値なので重複は無い                                                                                                                                                                |
| 関心 9 / 区間 26 | 分類     | 「各テストの doc コメントと assertion がどの実装関数を狙っているか」で 79 本を 1 本ずつ手で割り当て、その割り当て列をファイル順に走らせて切り替わり回数を数えたもの。割り当てが変われば区間数も変わる |

**成り立たない点 1 — 「リスト」は関心ではない。** 該当は `T-FC024` と `T-FC046` の 2 本だけで、どちらもリスト変換そのものを見ていない。前者は `<li>` の中の `<pre>` がリストマーカーの下にインデントされたまま残るか、後者は `<li>` の中の `<br>` が htmd の `list_item_handler` のインデント処理で末尾 2 スペースを失うかを見る。**どちらもリスト以外の構造がリストという容器の中でどう振る舞うかのテスト**であり、切り出せる単位を構成しない。

**成り立たない点 2 — 6 群が名指ししていない関心が 2 つあり、どちらも「リスト」より大きい。** テーブルセル内の `<pre>` を inline code span へ落とす関心が 9 本、`<br>` と空白の畳み込みが 5 本ある。

この 2 点は上の分類の粒度に依存しない。テスト本体を読めば、どちらも直接確かめられる。

9 関心の内訳と、それぞれが狙う実装は次のとおり。

| 関心         | 本数 | 狙う実装 (`src/fetch/converter.rs` 内)                                                                                                                                                               |
| ------------ | ---- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| PRE          | 24   | `pre_handler`、`has_code_child`、`raw_pre_content`、`push_element_content`、`span_handler`、`has_pre_ancestor`                                                                                       |
| TABLE        | 16   | `table_handler`、`row_children`、`is_row`、`row_is_all_header_cells`、`extract_data_row`、`extract_row_cells`、`normalize_cell_content`、`format_table_row`、`format_separator_row`                  |
| ANCHOR       | 10   | `a_handler`、`anchor_href`、`anchor_attr`、`strip_link_title`、`process_title_like_htmd`、`split_trailing_document_whitespace`                                                                       |
| CELLCODE     | 9    | `pre_handler` のセル分岐、`has_table_cell_ancestor`、`inline_code_span`、`text_content`、`push_text_content`                                                                                         |
| SUPPRESS     | 8    | `suppressed_handler`、`SUPPRESSED_TAGS`、`is_suppressed_element`、`element_namespace`、`RAW_TEXT_TAGS`、`close_self_closed_raw_text_tags`、`raw_text_tag_at`、`end_tag_at_or_after`、`start_tag_end` |
| BR-WS        | 5    | scout 側の実装は無い。htmd 組み込みの `br_handler` と `compress_whitespace` の挙動を pin する回帰群                                                                                                  |
| FRONTMATTER  | 4    | `format_with_frontmatter`                                                                                                                                                                            |
| CONTAINER-li | 2    | scout 側の実装は無い。htmd の `list_item_handler` の挙動を pin する                                                                                                                                  |
| RESULT       | 1    | `to_fetch_result` と `FetchResult`                                                                                                                                                                   |

**このファイルは追加された順に積まれており、関心順ではない。** ファイル順が ID 番号順と一致しない箇所がある (`T-FC083` → `T-FC082` → `T-FC020`、`T-FC068` → `T-FC067`、`T-FC091` → `T-FC078`)。その結果、冒頭の 8 本と末尾の 5 本が「1 関心 1 本ずつ」の散らばりになり、中央部だけが関心ごとの塊になる。切り出しの見通しは `code-quality-assessment.md` の `### E-4` が持つ。
