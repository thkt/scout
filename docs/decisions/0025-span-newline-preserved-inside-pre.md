---
status: "accepted"
date: 2026-08-14
decision-makers: thkt (project owner)
---

# Span Newline Preserved Inside `<pre>`

## Context and Problem Statement

#373 で fast_html2md から htmd へ乗り換えた結果、行ごとに `<span>` を使うシンタックスハイライトのページでコードブロック全体が 1 行に潰れる退行が生じた (#384)。原因は htmd 自身の 2 箇所にある。`span` ハンドラが未登録のとき通る高速経路 (`htmd-0.5.5/src/dom_walker.rs` の `walk_node` の span 高速経路) が、span の walk 済み内容の先頭と末尾の `\n` を無条件に剥がす。この strip に `is_pre` の除外が無く、`<pre>` の中で行ごとに置かれた span の末尾改行も剥がれるため、隣接するテキストと同じ行へ合流する。同じ欠落は組み込みの `htmd-0.5.5/src/element_handler/span.rs` の `span_handler` にもあり、こちらは高速経路より前から存在する。0.5.4 でも同じ strip があるため、htmd のバージョンを下げても直らない。

`https://squidfunk.github.io/mkdocs-material/reference/code-blocks/` など実在する 6 ページ 123 ブロックで検証すると 50 ブロックが行数を落とす。取得したコードがそのまま実行できないだけでなく、コメント行が次のコマンドを飲み込む実害がある (issue #384 本文)。上流のリリースを待たずに、scout 側で退行を止める必要があった。

## Decision Drivers

- 上流 htmd のリリースを待たずに #384 の退行を止める
- 挙動差分を `<pre>` 配下の span に限定し、`<pre>` の外の span (inline code 内など) は htmd 標準の処理へそのまま委譲する
- htmd の fork や恒久的なパッチを避け、上流が直った時点で単純に消せる形にする

## Considered Options

- 方式 A: scout 側で `span` ハンドラを登録し、`<pre>` の祖先を持つ span の内容を `walk_children` でそのまま通す (採用)
- 方式 B: htmd を fork し、`dom_walker.rs` と `element_handler/span.rs` の両方に `is_pre` 除外を足したパッチ版を使う
- 方式 C: htmd へ上流 PR を送り、リリースされるまで退行を許容する
- 方式 D: `converter.rs` の後処理として、崩れた行を正規表現などで復元する

## Decision Outcome

Chosen option: 方式 A。`src/fetch/converter.rs` の `pre_handler` と同じ `add_handler` 登録の形で `span_handler` を追加し、`<pre>` 祖先を持つ span だけ `Handlers::fallback` を経由せず `walk_children` の結果をそのまま返す。それ以外の span は `Handlers::fallback` へ委譲し、htmd 組み込みの `span_handler` (element_handler/span.rs) に処理を渡す。

決め手は、`add_handler(vec!["span"], span_handler)` を足すだけで htmd 側の登録ハンドラ数が 1 を超え、`htmd-0.5.5/src/dom_walker.rs` の `walk_node` の高速経路自体が全 span で無効になり、`element_handler/mod.rs` の通常ディスパッチ (`find_handler`) に切り替わる点である。この副作用により、最後に登録した `span_handler` (scout 側) が最初に呼ばれる形になり、fork も上流リリース待ちも要らずに退行を止められた。

祖先判定は `<pre>` のみとし、htmd 自身の `is_inside_pre` (`<code>` 祖先も含む、`htmd-0.5.5/src/element_handler/mod.rs` の `is_inside_pre`) より意図的に狭くした。`<pre>` の外にある inline `<code>` 内の span は `Handlers::fallback` を経て htmd 組み込みの span ハンドラへ渡る。そこで `content.trim_matches('\n')` (`htmd-0.5.5/src/element_handler/span.rs` の `span_handler`) が span 内容の両端の改行を落とすため、`handle_preformatted_code` の改行→空白折り畳み (`htmd-0.5.5/src/element_handler/code.rs` の `handle_preformatted_code`) には届かず、前後の行が区切りなしで連結する (T-FC054 で pin)。改行が span の中ではなく `<code>` 自身のテキストノードにある場合は span ハンドラを通らず、従来どおり空白へ畳まれる。math span (`class="math math-inline"`) の除外は写していない。組み込みの math 分岐は属性と単一 Text 子の両方を要求するため、要素の子を持つ span 構造では元々マッチしない。

表セルの中の `<pre>` は対象外になる。`<td>` / `<th>` を祖先に持つ `<pre>` は `pre_handler` の `has_table_cell_ancestor` 分岐で打ち切られ、子要素ごとのディスパッチが走らないため `span_handler` に到達しない。その分岐は `text_content` で DOM を直接読み、結果を `normalize_cell_content` が `\n` ごと空白へ畳む。`[T-FC025]` がこの形を固定する。表の行が割れないことを優先した判断で、DR-0027 が同じ限界を契約の側から書いている。

### Consequences

- Good, because #384 の退行が上流のリリースを待たずに止まり、6 ページ 123 ブロックの行数落ちが解消する (行ごとの検証は issue #384 本文の実測。本 DR の実装確認範囲はユニットテストと目視観察に限る)
- Good, because `pre_handler` と同じ登録パターンを踏襲し、`converter.rs` 内の実装が一貫する
- Bad, because htmd の内部実装 (`dom_walker.rs` の登録ハンドラ数ゲート、`element_handler/mod.rs` のディスパッチ順) に依存した回避策であり、htmd 側の実装変更で無言のまま壊れうる
- Bad, because 判定が `<pre>` 祖先のみで `is_inside_pre` より狭いため、`<pre>` の外にある inline `<code>` 内の span は改行が両端とも剥がれて空白も残らず、pre 内 span と挙動が分かれる。これは #384 以前から続く htmd 標準の挙動で、span の登録を外した状態でも同一の出力を実測した

### Confirmation

T-FC052〜T-FC055 (`src/fetch/converter.rs` の `mod tests`) が、pre 内 span の末尾改行の保持、行ごとの span が別行を保つこと、pre 外の inline code 内 span で改行が剥がれること、隣接 span が要素の子を持つ形でも改行が残ることを assert する。`cargo nextest run --profile ci` が緑であることを確認する。

## Pros and Cons of the Options

### 方式 A

`span` ハンドラを登録し、`<pre>` 祖先を持つ内容だけそのまま通す。

- Good, because 実装が `pre_handler` と同じ 1 関数追加で済み、即座に効く
- Good, because 上流が直った後は本ハンドラを削除するだけで元に戻せる
- Bad, because htmd の「登録ハンドラ数が 1 のときだけ高速経路が動く」という非公開の内部条件に依存する

### 方式 B

htmd を fork し、`dom_walker.rs` と `element_handler/span.rs` の両方に `is_pre` 除外を足す。

- Good, because 修正が htmd 本体に入り、scout 側の回避コードが要らない
- Bad, because fork の追従コストが継続的に発生し、htmd の以降のリリースを取り込むたびに patch を当て直す必要がある

### 方式 C

htmd へ上流 PR を送り、リリースされるまで退行を許容する。

- Good, because 恒久的な修正が上流に入り、scout 側のコードは増えない
- Bad, because リリース待ちの期間、priority:high の退行 (#384) が本番の出力品質を落とし続ける

### 方式 D

`converter.rs` の後処理として、崩れた行を正規表現などで復元する。

- Good, because htmd の内部実装に依存しない
- Bad, because 一度合流した行の境界は情報として失われており、正規表現での復元は誤判定を生みやすい。6 機構 (line_spans, hl_lines, class="line"/"cl", boring, 属性なし token span) それぞれで境界の見分け方が異なる

## More Information

### Trade-offs

htmd の内部実装 (登録ハンドラ数ゲート、ディスパッチ順) に依存する回避策を採る代わりに、fork の保守コストと上流リリース待ちの期間を避けた。この回避策は htmd のマイナーバージョンアップでも黙って壊れうるため、`cargo update` 後は T-FC052〜T-FC055 の green を確認する。

### Upstream status

上流 letmutex/htmd へは報告していない。2026-08-14 時点で同種の報告も無く、issue と PR を全件走査して該当が 0 件だった。open issue は `#27 escape_if_needed` と `#3 Compare with Pandoc` の 2 件で、どちらも改行の strip とは別の主題である。closed の `#14 Better handling code block` は `<pre class="language-rs">` からの言語解決の要望で、これも別の主題である。

htmd の main (v0.5.5 と同じ commit、2026-07-27) に両方の欠落が残る。`htmd-0.5.5/src/element_handler/span.rs` の `span_handler` の `content.trim_matches('\n')` には `is_pre` の分岐が無い。`dom_walker.rs` の高速経路は `is_pre` を再計算した直後に `trim_start_matches('\n')` と `trim_end_matches('\n')` を無条件で適用し、その 2 行下の `append_normalized_content(output, content, is_pre)` にだけ `is_pre` を渡す。上流側の修正は strip を `if !is_pre` で囲む形になる。

### Reassessment Triggers

- `dom_walker.rs` の strip と `element_handler/span.rs` の両方が上流で直る。両方の修正が入った htmd バージョンへ上げた時点で `span_handler` と `has_pre_ancestor` を削除し、`add_handler(vec!["span"], ...)` の登録も外す
- htmd がハンドラ登録数のゲートや `find_handler` のディスパッチ順を変更し、本ハンドラの前提 (最後に登録したハンドラが最初に呼ばれる) が崩れる。回避策の実装を作り直すか、方式 B・C へ切り替える
- `<pre>` の外にある inline `<code>` 内 span で改行が消える挙動について要望が来る。祖先判定を `is_inside_pre` 相当に広げるか判断する。なお `htmd-0.5.5/src/element_handler/span.rs` の `span_handler` の strip だけが上流で直ると、この経路の改行は `handle_preformatted_code` へ届くようになり、消える挙動から空白へ畳む挙動へ変わる

Related to issue #384.
