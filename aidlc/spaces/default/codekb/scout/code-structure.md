# Code Structure — scout

## ディレクトリ構成

アプリケーションソース面は `src/` 95 ファイル、`tests/` 4 ファイル、リポジトリ直下の設定 6 ファイル、`.github/`、`docs/decisions/`、`docs/audit/` である。

```
scout/
+-- src/                    95 files - implementation plus test-only siblings
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

`src/lib.rs` の `mod` 宣言は 20 本 (`grep -c '^mod ' src/lib.rs` で測定)。うち `test_support` は `#[cfg(test)]` 配下にあり、リリースビルドには入らない。

`aidlc/`・`.claude/`・`target/`・`workspace/` はこのツリーに含めない。除外の理由と根拠は `reverse-engineering-timestamp.md` の `## アプリケーションソース外として除外した領域` が持つ。

## ファイル分類

`src/` のファイルは 3 種に分かれる。テストの置き場が 2 通りあることがこのリポジトリの特徴で、混同すると数え間違いが起きる。

| 種別               | 数             | 説明                                                                                                                 |
| ------------------ | -------------- | -------------------------------------------------------------------------------------------------------------------- |
| 実装ファイル       | 50             | 本体。うち 19 本は末尾に `#[cfg(test)] mod tests { … }` ブロックを持つ                                               |
| テスト専用ファイル | 45 (11,538 行) | `src/` 配下に置かれ、実装ファイル側の `mod tests;` 宣言 7 本から参照される兄弟ファイル                               |
| 統合テスト         | 4 (1,833 行)   | `tests/` 配下。`cli_integration.rs` 447、`exit_code_contract.rs` 271、`output_injection.rs` 784、`common/mod.rs` 331 |

**inline `mod tests` ブロックは 19 箇所であって 26 ではない。** 26 は `mod tests` という文字列を含むファイル数で、そのうち 7 本は兄弟ファイルを指す `mod tests;` 宣言である。`docs/audit/2026-08-11-rust-code-assessment.md` の実測値表がこの 2 形を「26 ファイルが inline `mod tests` を含み」と一括りにしているため、監査文書を無批判に引くと 26 が伝播する。CodeKB では 19 と 7 に割って記録する。

テスト専用ファイルへ切り出すか inline に残すかの基準は行数ではない。`.claude/rules/CONVENTIONS.md` が「1 ファイルのテストが 2 つ以上の関心を持ったら分ける。ファイル名が中身を言い表せなくなった時点で、関心が 2 つ以上ある」と定める。実例が `src/fetch/cdp/launch/` で、browser_binary/browser_request/cdp_launch/ws_url_parse の 4 関心に 4 ファイルが対応する。`src/fetch/cdp/launch.rs` はこの 4 本を `#[cfg(test)] mod <name>_tests;` の並びで宣言しており、**実装ファイル自体は 1 本のまま残している。** 分割の単位はテスト側にだけ入っている。

## モジュール宣言と可視性

**到達できない `pub` は 0 である。** `Cargo.toml` の `[lints.rust]` にある `unreachable_pub = "deny"` がビルドで落とす。crate 外へ出るのは `pub async fn run() -> ExitCode` の 1 つだけで、これが「契約面は Rust API ではなく CLI 表面」という設計を機械的に保証している (`architecture.md` の `## アーキテクチャスタイル` 参照)。

内側では可視性が段階的に使い分けられる。この 2 回の走査で読んだ範囲での実例:

| 形                          | 実例                                                    |
| --------------------------- | ------------------------------------------------------- |
| `pub(crate)`                | crate 全体へ出す型と関数                                |
| `pub(super)`                | 親モジュールにだけ出す補助                              |
| `pub(in crate::fetch::cdp)` | `src/fetch/cdp/proxy.rs` の `spawn_ssrf_proxy` の再輸出 |

**可視性の分布を素朴な grep で数えると落ちる。** `src/fetch/converter.rs` の実装 1-985 行で「crate 外または親モジュールへ出ている項目」は 10 個 (`pub(crate) fn` 6 + `pub(super) fn` 1 + `pub(crate) const` 2 + `pub(crate) struct` 1) だが、列 0 に錨を打った `^pub\(` は 4 しか返さない。`impl FetchResult` の 6 メソッドはインデントされているためである。

最小可視性の決め方 (手順と注意点) は `docs/audit/2026-08-11-rust-code-assessment.md` の B-6 が持つ。

## コードパターンと規約

規約の本体はいずれも一次ソース側にあり、`.claude/rules/CONVENTIONS.md` はその索引として働く。以下は「どのパターンがどこで決まっているか」の対応表である。

| パターン                           | 一次ソース                                           | 内容の要点                                                                                                                                                                        |
| ---------------------------------- | ---------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| テスト ID の採番と prefix 設計     | `src/test_support.rs` の crate doc                   | `[T-<PREFIX><NNN>]` 形式。prefix は「テスト対象」を指すので 1 prefix が複数ファイルに跨る。番号は prefix 内で一意でファイル単位ではない。引用時はブラケットを外して定義と区別する |
| 重複採番の検出                     | `src/test_support.rs` の `scan_test_id_violations`   | リポジトリ自身のテストが違反を落とす                                                                                                                                              |
| 上限なし read の禁止               | `clippy.toml` の `disallowed-methods`                | `reqwest::Response::{text, bytes, json}` を禁じ、各 `reason` に代替関数名 (`body_limit::read_body_capped` / `read_body_snippet`) を書く                                           |
| DR からコードを指す参照の形        | DR-0028                                              | 行番号ではなくシンボル名で指す                                                                                                                                                    |
| コメントは英語で書く               | `.github/workflows/ci.yml` の Comment language check | 対象はコメント、doc comment、テスト名、assertion message。例外はバイト列の注釈で、注釈自体は英語・引用部分だけ原語                                                                |
| テストは関心ごとにファイルを分ける | `.claude/rules/CONVENTIONS.md`                       | 行数は基準にしない                                                                                                                                                                |

### doc コメントが却下を残す

このリポジトリの doc コメントは「何をするか」ではなく **「なぜこの値か」「なぜこの順序か」「何を却下したか」** を書く。却下を測った数値がコメントに残るのが特徴である。2 回の走査で読んで確認した実例:

- `src/fetch/cdp/launch.rs` の `resolve_browser_binary` — `OnceLock` キャッシュを外した理由 (テスト分離を壊し、プロセス生存期間で結果を固定した。`which` 数回の 1-5ms は後続の約 2 秒の chromium レンダリングに対して無視できる)
- `src/fetch/cdp/launch.rs` の `PGROUP_SIGTERM_GRACE` — 50ms という値の根拠
- `src/github/types.rs` の `ContentsPayload::Directory` — `Vec<IgnoredAny>` でなく素の `IgnoredAny` にしたときに起きたこと
- `src/fetch.rs` の `SPA_ROOT_IDS` — 単引用符形式を「見落としではなく」外した理由
- `src/lib.rs` の `init_tracing` — `expect` を fallback にしない理由
- `clippy.toml` 冒頭 — テストではなく lint で守る判断の理由 (「どちらの caller も cap を観測可能にしない。だからビルドを落とす」)
- `src/tools/config.rs` の `DEFAULT_GITHUB_TIMEOUT_SECS` — 180 秒を選んだ根拠、それが retry 予算 (約 279 秒) の下に収まる理由、切り捨てられるケース (約 186 秒) までを数値で書く
- `src/tools/config.rs` の `from_env_with` — env 読み取りを注入可能にした理由 (`unsafe { std::env::set_var(...) }` が `unsafe_code = "forbid"` で使えない)
- `src/tools/config.rs` の `read_env_raw` — `VarError::NotUnicode` を「未設定」に潰さず `UsageError` へ落とす理由 (デフォルトへのフォールスルーを避ける)
- `src/slack/client.rs` の `SlackFetchOutcome` — `String` 返しでは cap のヒットが呼び出し側に見えないという設計理由。`lookups_failed` と `users_capped` を分ける理由も同じ doc コメントにある (「名前を持たずに返った lookup は失敗に数えない。呼び出し側に retry するものが無い」)
- `src/slack/client.rs` の `USER_TOKEN_PREFIX` — bot/app-level/workflow の各トークンを弾く理由 (bot トークンはアプリが追加されたチャンネルしか見えない) と、それを検証した日付 (2026-06)

`#[expect(...)]` には必ず `reason` が付く。抑制の総数と測定範囲は `code-quality-assessment.md` が持つ。

### 本番経路の panic

`unsafe_code = "forbid"`。`expect` は 2 箇所で確認され、両方に理由コメントがある — `src/envelope.rs` の `to_json_line` の `expect("envelope is Serialize")` と、`src/lib.rs` の `"scout=info".parse().expect("static directive is valid")`。

## サイズ分布

行数は分割の基準ではないが、規約から外れた 1 本を特定するには使える。**内訳の列が空の行は「テストを含まない」という主張ではなく「内訳を測っていない」という意味である。** 誤読を避けるため、走査で内訳を確定した行にはその旨を書く。

| ファイル                 | 行数  | 内訳                                                                                                                                                                                                                                                                                                 |
| ------------------------ | ----- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/fetch/converter.rs` | 3,131 | 実装 985 (1-985) + `#[cfg(test)] mod tests` 2,146 (986-3,131)                                                                                                                                                                                                                                        |
| `src/test_support.rs`    | 900   | `#[cfg(test)]` 専用                                                                                                                                                                                                                                                                                  |
| `src/markdown.rs`        | 671   | 未測定                                                                                                                                                                                                                                                                                               |
| `src/lib.rs`             | 639   | 未測定                                                                                                                                                                                                                                                                                               |
| `src/slack/client.rs`    | 620   | **全 620 行が実装。** inline `mod tests` を持たない。テストは兄弟 2 ファイル計 1,152 行 (`src/slack/client/constructor_tests.rs` 96、`src/slack/client/http_tests.rs` 1,056) にあり、`src/slack/client.rs` 末尾の `#[cfg(test)] mod constructor_tests;` と `#[cfg(test)] mod http_tests;` が参照する |
| `src/tools/repo.rs`      | 548   | 未測定                                                                                                                                                                                                                                                                                               |
| `src/tools/params.rs`    | 540   | 未測定                                                                                                                                                                                                                                                                                               |
| `src/tools/config.rs`    | 477   | 実装 195 (1-195) + `#[cfg(test)] mod tests` 282 (196-477)                                                                                                                                                                                                                                            |
| `src/fetch.rs`           | 462   | 未測定                                                                                                                                                                                                                                                                                               |
| `src/main.rs`            | 6     | `scout::run()` を呼ぶだけ                                                                                                                                                                                                                                                                            |

### `src/fetch/converter.rs` の関心は 6 群ではなく 9 つに割れる

**上書き。** 先行資料 (`docs/audit/2026-08-11-rust-code-assessment.md` の E-4、および `.claude/rules/CONVENTIONS.md` の「テストは関心ごとにファイルを分ける」節) は、このファイルのテスト ID の並びを「表/pre とフェンス/リンクとアンカー/script と style の抑制/frontmatter/リストの 6 群」と書く。attempt 2 が 986-3,131 行を開いて 79 本すべてを割り当てた結果、**この 6 群という枠は 2 点で成り立たない。**

数値の性質を分けて記録する。**79 という本数と 79 という ID 数は機械測定である。9 関心と 26 区間は走査担当による分類であり、測定ではない。**

| 値               | 性質     | 測定範囲                                                                                                                                                                                              |
| ---------------- | -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| テスト属性 79    | 機械測定 | 986-3,131 行。すべて `#[test]` で `#[tokio::test]` は 0                                                                                                                                               |
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

**このファイルは追加された順に積まれており、関心順ではない。** ファイル順が ID 番号順と一致しない箇所がある (`T-FC083` → `T-FC082` → `T-FC020`、`T-FC068` → `T-FC067`、`T-FC091` → `T-FC078`)。その結果、冒頭の 8 本と末尾の 5 本が「1 関心 1 本ずつ」の散らばりになり、中央部だけが関心ごとの塊になる。切り出しの見通しは `code-quality-assessment.md` の `## 技術的負債` が持つ。
