---
status: "accepted"
date: 2026-08-14
decision-makers: thkt (project owner)
---

# Pre Body Rebuilt from DOM, Not Reverse-Escaped from htmd's Walked Text

## Context and Problem Statement

`pre_handler` (`src/fetch/converter.rs`) が `<code>` 子を持たない `<pre>` をフェンスする際、旧実装は htmd の走査後テキスト (`Handlers::walk_children` の戻り値) から先頭 1 箇所のバックスラッシュを剥がしていた。剥がす理由は、htmd 自身の `escape_pre_text_if_needed` (`htmd-0.5.5/src/dom_walker.rs` の `walk_node` の `is_pre` テキスト分岐と `escape_pre_text_if_needed`) が `<pre>` 直下のテキストノードごとに、先頭が `` ` `` または `~` のとき常にバックスラッシュを前置するためで、この前置とフェンス文字の衝突を避ける処理は元から必要だった。

旧実装 (`opens_with_escaped_fence_char`) はこの前置を、走査後文字列の先頭 1 文字だけを見て逆変換していた。しかし `escape_pre_text_if_needed` はテキストノードごとに独立して効くのに対し、旧実装は `<pre>` の**最初の Text 子**だけを読み、その判定を**走査後文字列全体の先頭**に適用していた。この 2 つの位置は一致しない場合がある。2 番目以降の Text 子が `` ` `` から始まる場合や、最初の Text 子より前に Element 子がある場合、その Element 子自身の変換結果が走査後文字列の先頭を占め、剥がすべきバックスラッシュは文字列の途中に埋もれる。要素の変換結果の長さは呼び出し前に分からないため、位置を推測して逆変換する方式はこの時点で一般化できない (issue #376)。

実測 20 ページ中 4 ページ (fzf README, Wikipedia の Bash 記事, ss64.com, GNU bash manual) で実際に出力が壊れた。nushell の演算子 `=~` が `=\~` に化けるなど、AI エージェントが一次ソースとして読むコードブロックに原文へ存在しない文字が混入した。

`<pre>` の中身を DOM の子ノードから直接組み直す案 (Option A) は、htmd の `Handlers::walk_children` (`htmd-0.5.5/src/element_handler/mod.rs` の `Handlers::walk_children`) が持つ副作用のうち、隣接する同タグ同属性のインライン兄弟のマージ (`htmd-0.5.5/src/dom_walker.rs` の `can_combine`) を失う。このマージが先に走っていないと、同じタグ・同じ属性の `<span>` が複数の子ノードのまま `raw_pre_content` のループへ渡り、`<span>line1\n</span><span>line2\n</span>` のような構造で各行の改行が別ノードの境界に分断される (T-FC037 が pin)。

## Decision Drivers

- OUTCOME: 一次ソース本文をそのまま AI エージェントの context に届ける。原文に無い `\` がコードブロックへ混ざるのはこの Behavior に正面から反する
- htmd API の制約: scout が触れるのは `add_handler` と `Handlers` トレイト (`fallback`/`handle`/`walk_children`/`options`) だけで、`dom_walker::walk_children` のような crate 内部関数は呼べない
- 既存契約を壊さない: T-FC020 (`<pre><code>` を二重にフェンスしない) と T-FC028 (span だけの `<pre>` もフェンスする) は DOM 形状で分岐する契約で、この分岐は維持する

## Considered Options

- 方式 A: `<pre>` の中身を DOM の子ノードから組み直す。Text 子はそのまま push し、Element 子だけ `Handlers::handle` で変換する。組み直しの前に `Handlers::walk_children` を 1 回呼び、戻り値の文字列は使わず、htmd 自身の隣接兄弟マージという副作用だけを先に走らせる (採用)
- 方式 B: 走査後文字列からエスケープの位置を復元する判定を一般化する
- 方式 C: htmd を fork し、`<pre>` 直下では `escape_pre_text_if_needed` 自体を無効化する
- 方式 D: `raw_pre_content` の子ループで、Element 子ごとに `Handlers::walk_children` を呼ぶ

## Decision Outcome

Chosen option: 方式 A。`pre_handler` の `<code>` 子を持たない分岐で `raw_pre_content` を呼び、Text 子の `contents` を DOM から直接 push し、Element 子だけ `Handlers::handle` に渡す。呼び出しの直前に `handlers.walk_children(element.node)` を 1 回だけ呼び、その戻り値の文字列 (`result.content`) は使わずに捨てる。

htmd の出力を位置で逆変換しない。`escape_pre_text_if_needed` が前置するバックスラッシュは、走査後の文字列上のどこに現れるか呼び出し前には分からない (要素の変換結果の長さが不定なため)。位置を推測して剥がす代わりに、エスケープが入る前の Text ノードの `contents` を DOM から直接読む。これにより走査後の文字列に対する逆変換そのものが不要になり、`opens_with_escaped_fence_char` は削除できる (方式 B は変換結果の長さを事前に知る手段が無く、この時点で不成立)。方式 C は htmd 全体の fork と追従コストを要し、DR-0025 で同じトレードオフを一度採らなかった判断と揃える。

兄弟マージのために走査を 1 回捨てる。`raw_pre_content` は DOM の `node.children` を直接ループするため、htmd の `Handlers::walk_children` が内部で行う隣接兄弟マージ (`can_combine`, `attrs1 == attrs2` を含む複数条件がすべて揃う同タグ・同属性のインライン要素だけが対象) を経由しない。このマージは `node.children` という `RefCell` 自体を書き換える副作用であり、`Handlers::walk_children` を 1 回呼ぶだけで、その戻り値を使わなくても `node.children` へ反映される。したがって `pre_handler` は `raw_pre_content` を呼ぶ前に `walk_children` を 1 回呼び、htmd 標準のマージ済み DOM の上で `raw_pre_content` を走らせる。方式 D (子ごとに `walk_children` を呼ぶ) はこの兄弟マージの前提が崩れたうえ、`Handlers::walk_children` は子の中身しか歩かないため `<br>` のような子要素自身のハンドラ出力が失われ、不成立。

`<td>` / `<th>` を祖先に持つ `<pre>` はこの分岐へ入らない。`pre_handler` は `<code>` 子の有無を見る前に `has_table_cell_ancestor` を判定し、`text_content` で DOM を読んで `inline_code_span` で包む経路へ抜ける (commit `a4acf72`、本 DR の受理より後)。この経路も DOM を直接読むので本 DR の決定 (走査後の文字列を逆変換しない) は保たれるが、`raw_pre_content` は通らない。下の Reassessment Triggers が `can_combine` の条件変化を挙げるとき、この分岐も同じ前提に依存している点で対象に含まれる。

### Consequences

- Good, because 走査後文字列に対する位置推測が要らなくなり、#376 が挙げた「2 番目以降の Text 子」「先頭 Text 子より前に Element 子がある」の 2 形がどちらも構造的に発生しなくなる。実測 4/20 ページの `\` 混入が解消する
- Good, because `opens_with_escaped_fence_char` と、それが必要とした「htmd の走査後テキストと DOM を突き合わせて逆変換する」仕組み自体が消え、`pre_handler` の非 code 分岐がテキストと要素を素直に走査するだけの形になる
- Bad, because `Handlers::walk_children` が持つ副作用のうち利用するのは隣接兄弟マージのみで、残る 2 つ (`append_normalized_content` の改行 2 本上限、ブロック子直前の行末空白削り) は暗黙に失われる。改行上限は `push_element_content` として明示的に再実装した (T-FC038) が、行末空白削りは再実装していない。影響は `<pre>a <div>b</div></pre>` の 1 形のみで、変わる向きは htmd 標準の削りより原文寄り (削られていた空白が残る) であり、退行ではない
- Bad, because 同タグ・異属性、または `<a>` を含む一部のインライン兄弟マージ (`can_combine` が対象外とする形) は依然マージされないため、その境界での改行結合は #384 と同じ形で残る。この既知の残存欠陥は既に issue #384 として起票済みで、本 DR の変更範囲外
- Bad, because `walk_children` を戻り値を使わない目的だけで呼ぶ構造は、その意図がコメントに書かれていないと「消せる無駄な呼び出し」に見える。呼び出し箇所にその理由を明記した

### Confirmation

T-FC034〜T-FC040 (`src/fetch/converter.rs` の `mod tests`) が、2 番目以降の Text 子・先頭 Text 子より前の Element 子・入れ子 `<pre>` のいずれでもバックスラッシュが原文どおり残ること、同タグ同属性の隣接 span で改行が保たれること、隣接ブロック子境界の改行が 2 個までに収まること、`<br>` が行末空白 2 個と改行として残ること、未登録タグの子のテキストがエスケープされないことを assert する。`cargo nextest run --profile ci` が緑であることを確認する。

## Pros and Cons of the Options

### 方式 A

`<pre>` の中身を DOM の子ノードから組み直し、組み直しの前に `walk_children` を 1 回だけ呼んで兄弟マージを先に走らせる。

- Good, because 位置推測が不要になり、要素の変換結果の長さに依存しない
- Good, because htmd の標準マージを流用でき、隣接 span の改行保持を自前で再実装しなくて済む
- Bad, because `walk_children` を戻り値を捨てる目的で呼ぶ構造そのものが、htmd の内部副作用 (`node.children` という `RefCell` の書き換え) に依存する

### 方式 B

走査後文字列からエスケープの位置を復元する判定を一般化する。

- Good, because DOM の直接読み取りを増やさず、既存の `pre_handler` の骨格を保てる
- Bad, because 要素の変換結果の長さは呼び出し前に分からないため、走査後文字列上のどこにエスケープが現れるか原理的に復元できない

### 方式 C

htmd を fork し、`<pre>` 直下では `escape_pre_text_if_needed` 自体を無効化する。

- Good, because scout 側の回避コードが `pre_handler` に一切要らなくなる
- Bad, because fork の追従コストが継続的に発生し、htmd の以降のリリースを取り込むたびに patch を当て直す必要がある (DR-0025 で一度不採用にした理由と同じ)

### 方式 D

`raw_pre_content` の子ループで、Element 子ごとに `Handlers::walk_children` を呼ぶ。

- Good, because `pre_handler` の冒頭で `walk_children` を 1 回呼ぶ必要が無くなる
- Bad, because `Handlers::walk_children` は子の中身しか歩かないため、`<br>` のように子要素自身がハンドラで生成する出力 (行末空白 2 個と改行) が失われる

## More Information

### Trade-offs

`Handlers::walk_children` を戻り値ではなく副作用のためだけに呼ぶ構造を採ることで、隣接兄弟マージという htmd 標準の仕組みを再実装せずに済ませた。この構造は htmd の非公開の内部動作 (`node.children` を書き換える副作用が公開 API の戻り値と独立して観測できること) に依存しており、htmd がマージのタイミングや対象条件を変更した場合、`raw_pre_content` が読む DOM の形が黙って変わりうる。`cargo update` 後は T-FC034〜T-FC040 の green を確認する。

### Reassessment Triggers

- htmd が `can_combine` の対象条件 (同タグ・同属性、`<a>` を除くインライン要素) を変更する、または隣接兄弟マージ自体を `Handlers::walk_children` から切り離す。`raw_pre_content` 呼び出し前の `walk_children` 呼び出しが前提とする副作用が崩れるため、実装を作り直すか方式 C・D へ切り替える
- `<pre>a <div>b</div></pre>` のようなブロック子直前の行末空白差異について要望が来る。`append_normalized_content` のもう 1 つの正規化 (行末空白削り) も明示的に再実装するか判断する
- 属性が異なる隣接 span の境界で改行が落ちる #384 の残存欠陥が別の実害を生む。`can_combine` の対象外を scout 側で拾う独自マージを足すか判断する

Related to issue #376.
