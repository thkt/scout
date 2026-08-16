---
status: "accepted"
date: 2026-06-24
decision-makers: thkt (project owner)
---

# Output-Injection Defense for AI-Agent Consumers

## Context and Problem Statement

scout の主 consumer は AI エージェントで、fetch/search/GitHub/Slack の結果を直接 context に取り込んで判断や次のアクションに使う。取得元は信頼できない Web ページや外部 message であり、本文に active markup や構造マーカーを仕込むことでエージェントの解釈を歪められる。代表的な注入面は次の 4 つである。

1. URL scheme 注入: markdown link に `javascript:`/`data:` を埋め、クリックや naive parser で実行を誘う
2. YAML 構造注入: frontmatter を付ける出力で、本文の行頭 `---`/`...` が新しい YAML document として解釈され、偽の frontmatter を差し込める
3. markdown メタ文字注入: `|` `[]()`・改行で table/link/見出し構造を壊し、本文を scout 自身の構造に偽装する
4. 制御文字注入: null byte や改行で parser を壊す

scout はこれらの中和を出力境界に実装しているが、方針 (どこで何を中和するか、strip か escape か、HTML 層との分担) が ADR として記録されていない。

## Decision Drivers

- エージェントは人間のレビューを介さず scout 出力を読むため、注入は silent に効く
- fetch/search/Slack/GitHub の全 backend で一貫した保証が要る (per-site 例外は脆い)
- HTML→markdown 変換の script 除去は変換ライブラリの責務で、scout は markdown/YAML 層の保証を上乗せする

## Considered Options

- Option A: scheme allowlist + メタ文字 escape を出力境界の共通関数に集約し fail-closed で中和する (採用)
- Option B: ドメインごとに strip/escape ルールを変える per-site policy
- Option C: 中和を consumer (エージェント側 parser) に委ね、scout は素通しする

## Decision Outcome

Chosen option: Option A。出力境界の中和を `src/markdown.rs` と `src/fetch/converter.rs` の少数の `pub(crate)` 関数に集約し、全 backend がそれを経由する。URL は http/https のみ clickable link にし、他 scheme は不活性な `text (url)` へ流す。markdown メタ文字は escape し改行は空白へ畳む。YAML 値は backslash escape する。

本文行頭の `---`/`...` の書き換えは、fetch と Slack で経路を分ける。GitHub の README は fetch と同じフェンス追跡側に付く。fetch は `neutralize_yaml_markers_outside_fences` (src/yaml.rs) を経由し、`fence_marker` (src/markdown.rs) でフェンスの開始・継続・終了を追跡する。閉じたフェンスの内側にあるマーカー行は取得元ページのコードブロックの一部として原文のまま返す。フェンスが本文の終わりまで閉じない場合は、開いたと判定したフェンス自体を信用せず、本文全体を fence 非対応の `neutralize_yaml_markers` に通した結果へ fail-closed で切り替え、フェンス以降を無中和のまま残さない。Slack は `neutralize_yaml_markers` を直接経由し、フェンスの内外を区別せず全行を書き換える。Slack の message 本文はほぼ生のまま leaf に渡るため、fetch と同じフェンス追跡を持ち込むと、攻撃者が閉じないフェンスを 1 行打つだけで以降の本文が無中和になる経路を開く。fetch 側の忠実性向上のためだけに Slack 側の注入防御を緩める変更はしない。

`<script>`/`<style>` 等 active HTML は Readability (dom_smoothie) と変換層の 2 箇所で除去する。変換層の除去は Readability を迂回する `--raw` にも及ぶ (#403)。未知 scheme・難読化 (大小文字、先頭空白)・制御文字は素通しせず fail-closed で中和する。

Option B は policy 管理コストが高く per-site ルールが脆いため却下。エージェント consumer は全取得元に対し予測可能な保証を要する。Option C は防御を下流 (時に人間の端末) に押し付け誤りやすいため却下。

### Consequences

- Good, because 未知 scheme・難読化 URL・制御文字を素通しせず fail-closed で中和し、`javascript:` URL が clickable link になることを防ぐ
- Good, because fetch/search/Slack/GitHub が同じ中和関数を経由し、新 backend も同じ防御を継承する。GitHub は repo-overview の README を `format_readme_section` (src/github/format.rs) 経由で `neutralize_yaml_markers_outside_fences` に通し、fetch と同じフェンス追跡型の経路を辿る。フェンス追跡のように経路が分かれる箇所は、分ける理由 (フェンス追跡型の fetch・GitHub README は取得元の忠実な再現、直接型の Slack は生に近い攻撃者制御入力) を関数選択の差として明示する
- Good, because HTML 層の script 除去は変換ライブラリに委譲し、scout は markdown/YAML 層の保証に集中する
- Good, because escape 系関数は clean input で借用を返し common path でゼロアロケーション
- Good, because fetch はフェンスが本文末尾まで閉じない場合、開いたと判定したフェンス自体を信用せず本文全体を書き換える fail-closed に倒すため、フェンス構文の偽装による中和回避を許さない
- Bad, because Slack はフェンス追跡を持たないため、Slack 上のコードブロックであっても `---`/`...` を含む行は `***` に書き換わり、fetch と異なり原文と一致しない。message 本文がほぼ生で共有 leaf に渡る Slack でフェンス追跡を緩めると、攻撃者が閉じないフェンスを打つだけで以降が無中和になるため、意図して見送る
- Bad, because フェンス追跡を入れても、想定する consumer (フェンスを解さない naive な multi-document YAML reader) に対する穴自体は残る。フェンス対応は fetch と GitHub README の忠実性回帰を防ぐ追加中和であって、想定 consumer への根本対策ではない。scout 自身が出す区切りのうち research 出力は `***` へ替えて閉じたが (#405)、Slack の reply 区切りは `---` のまま残る。`***` は CommonMark 上は `---` と同じ thematic break で、YAML の document marker ではない
- Bad, because `\0\n\r\t` 以外の制御文字 (ESC, BEL) は素通しし、人間が端末で読む場合に terminal 描画へ影響しうる (主 consumer はエージェントのため受容)
- Bad, because 本文 markdown は意図的に rendered のまま渡すため、markdown を命令として naive に読むエージェント実装は見出し本文を誤解しうる (見出しレベル shift と JSON envelope 構造で緩和)
- Bad, because HTML 層の script 除去が dom_smoothie 単独への依存のままなら、ライブラリが退行したとき script の中身が本文へ漏れる。`--raw` は Readability 自体を通らないため、この経路では現に漏れていた。変換層の `suppressed_handler` (`src/fetch/converter.rs`) が script/style/noscript/textarea/iframe/title と SVG 名前空間の desc を Readability の成否と独立に除去する二重化により、dom_smoothie が退行しても変換層側の除去が残る。`--raw` の `content_html` も同じ変換層を経由するため、この経路でも漏れない (#403)

### Confirmation

中和点ごとに専用テストが存在する。markdown 層は `src/markdown.rs` の `[T-MD001..T-MD035]` が escape/改行畳み/scheme allowlist/難読化 fail-closed/見出し shift/フェンス判定 (`fence_marker`) を網羅する。YAML 層は `src/yaml.rs` の `[T-FC003..T-FC007]` が値 escape と document marker 書き換えを、`src/fetch/converter.rs` の `[T-FC008]` が frontmatter 注入防止を網羅する。fetch と Slack のフェンス扱いの分岐は、`src/yaml.rs` の `[T-FC030..T-FC033]` がフェンス内保存と閉じないフェンスの fail-closed 切り替えを leaf 単体で、`tests/output_injection.rs` の `[T-C032, T-C041]` が fetch 出力での同じ挙動を実際の変換経路越しに、`src/slack/format/format_tests.rs` の `[T-SK088]` が Slack 出力ではフェンスの内側も書き換えることをそれぞれ pin する。search 層は `src/search/engine/tests.rs` の `[T-SE010]` が source URL の `javascript:` scheme を不活性 text として出すことを assert する。HTML 層は `src/fetch/converter.rs` の `[T-FC084, T-FC085]` が `suppressed_handler` の 7 タグの中身が本文へ出ないことを、`[T-FC086]` が `--raw` の経路で同じことを pin する。除去が本文を巻き添えにしないことは、`[T-FC089, T-FC090]` が自己終了した raw-text タグの後続本文が残ることを、`[T-FC091]` が SVG 名前空間の外の `<desc>` のテキストが残ることを pin する。書き換えの側が新しい漏れを作らないことは、`[T-FC092]` が JS ソースの中に書かれた `<script … />` を書き換えないことで pin する。GitHub の README 経路は `src/github/format/overview_tests.rs` の `[T-GF044, T-GF045]` がフェンス外のマーカーの書き換えを打ち切りの有無の両方で、`[T-GF046]` が閉じないフェンスでの fail-closed を pin する。新しい出力経路を足す際は、これらの境界関数を経由しているかをテストで確認する。

## Pros and Cons of the Options

### Option A: 共通境界関数 + scheme allowlist + escape、fail-closed (採用)

中和を少数の `pub(crate)` 関数に集約し全 backend が経由する。

- Good, because 一貫した保証と単一の監査点 (5 関数) を与える
- Good, because common path でゼロアロケーション
- Bad, because 制御文字の網羅は 5 つの特殊ケースに限られる

### Option B: per-site policy

ドメインごとに strip/escape を変える。

- Good, because 特定サイトの正当な markup を許せる
- Bad, because ルール管理コストが高く脆い
- Bad, because エージェントが必要とする across-source の予測可能性を壊す

### Option C: consumer 委譲

scout は素通しし parser 側の安全性に依存する。

- Good, because 実装コストゼロ
- Bad, because 防御を下流 (人間端末を含む) に押し付け誤りやすい
- Bad, because scout の責務 (安全で曖昧でない出力) を放棄する

## More Information

### 中和点 (一次ソース)

場所はファイルまでとし、行番号は書かない。行番号はこの表の外の変更で古くなり、古いことが誰にも見えない。関数名は `ugrep -F 'fn <name>'` で一意に引ける。

| 関数                                     | 場所                 | 中和内容                                                                                                                                                                                      |
| ---------------------------------------- | -------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `md_link`                                | src/markdown.rs      | http/https のみ clickable、他 scheme は不活性 text、空白/制御文字 URL を拒否                                                                                                                  |
| `escape_md_inline`                       | src/markdown.rs      | `\|` `[]()` escape、改行を空白へ畳む                                                                                                                                                          |
| `escape_md_link`                         | src/markdown.rs      | link target の `[]()` escape、改行畳み                                                                                                                                                        |
| `sanitize_heading`                       | src/markdown.rs      | 見出し内改行を空白へ                                                                                                                                                                          |
| `shift_headings`                         | src/markdown.rs      | ページ見出しを深い level へ下げ scout 構造との衝突を防ぐ (code fence 内・非 ATX は skip)                                                                                                      |
| `fence_marker`                           | src/markdown.rs      | フェンスの開始/継続/終了判定 (CommonMark §4.5、run 長比較)。`shift_headings` と `neutralize_yaml_markers_outside_fences` が共有                                                               |
| `escape_yaml`                            | src/yaml.rs          | `\` `"` `\n\r\t` を escape、`\0` を除去                                                                                                                                                       |
| `neutralize_yaml_markers`                | src/yaml.rs          | 行頭 `---`/`...` を `***` へ書き換え、indent/inline は不変。Slack が直接経由し、fetch と GitHub README はフェンスが閉じないときの fail-closed 経路としてのみ経由する                          |
| `neutralize_yaml_markers_outside_fences` | src/yaml.rs          | fetch と GitHub README 専用。`fence_marker` でフェンスを追跡し、閉じたフェンス内側のマーカーは原文のまま保持。本文末尾までフェンスが閉じない場合は `neutralize_yaml_markers` へ丸ごと委譲する |
| `format_readme_section`                  | src/github/format.rs | GitHub repo-overview の README を `shift_headings` で見出しシフトし `neutralize_yaml_markers_outside_fences` へ通す。フェンス追跡は fetch と共有する                                          |

### HTML 層の分担

Readability (dom_smoothie) は抽出時に `<script>`/`<style>` 等を DOM から落とす。ただし除去を担うのはこの層だけではない。変換層 (`src/fetch/converter.rs` の `markdown_converter` に登録された `suppressed_handler`) が `script`/`style`/`noscript`/`textarea`/`iframe`/`title` と SVG 名前空間の `desc` に対し、子要素を辿らず空の `HandlerResult` を返すことで、同じ 7 タグを Readability の成否と独立に除去する。scout 側は変換後テキストに対し上表の markdown/YAML 保証を上乗せする二層構造で HTML パース自体は再実装しないが、active HTML の除去そのものは Readability と変換層の二重チェックになっている。

`desc` だけが名前空間で絞られるのは、SVG の外の `<desc>` はブラウザが本文として描画するためである。htmd のハンドラ振り分けはローカルタグ名だけを見るので、絞らなければ HTML の `<desc>` やその名前のカスタム要素の可視テキストまで消える。`title` を絞らないのは逆の理由で、SVG の外の `<title>` もブラウザは本文に描画しない (タブに出す)。frontmatter の title は `src/fetch/extractor.rs` の `make_raw` が `extract_title_from_html` で別途読むため、この除去では失われない。

変換層の除去は、自己終了記法の raw-text タグを開始タグと終了タグの対へ書き換える前処理 (`close_self_closed_raw_text_tags`) と対で成立する。`check_content_type` (`src/fetch/download.rs`) は `application/xhtml+xml` を受理するが、htmd は受理した本文を HTML として解析する。HTML の tokenizer は raw-text タグの自己終了フラグを無視して raw-text 状態へ入るため、XHTML 式に書かれた `<script src="app.js" />` は以降の本文すべてを自分の Text 子として飲み込む。前処理が無ければ `suppressed_handler` がその本文ごと落とす。この書き換えが変えるのは解析構造だけで、書き換え後の要素は中身が空のまま除去される。scout が XHTML を解析できるようになるわけではない。

`--raw` は `extract_raw` が Readability を迂回するため、Readability 側の除去は効かない。しかし `--raw` の `content_html` も同じ変換層 (`markdown_converter`) を経由するため、`suppressed_handler` による除去は Readability の有無に関係なく成立する。除去する層を足すか既知の限界とするかの判断は、変換層に独立した除去を足すことで決着した (#403)。

`template` の中身は `suppressed_handler` の対象外である。html5ever は `<template>` の子孫を通常の `children` とは別の `template_contents` に格納し、htmd の DOM walk は `children` しか辿らないため、ハンドラを登録しなくても中身は最初から本文に出ない。HTML コメントも対象外である。`markdown_converter` が使う `Pure` 翻訳モードでは htmd はコメントノードを一切出力せず (`Faithful` モードのみ `<!--...-->` を復元する)、除去を追加する必要がない。いずれも本文へ漏れる経路自体が無いため、対象タグに加える意味がない。

`suppressed_handler` はタグ名だけで判定し属性を見ないため、`<script type="application/ld+json">` のような非実行の構造化データも実行可能な JS と区別されず本文から消える。除去範囲を広げた副作用であり、構造化データの保全は scope 外の限界として残る。

### 参照

- `src/markdown.rs` (markdown 層中和 + テスト T-MD001..018、フェンス判定 `fence_marker` + テスト T-MD032..035)
- `src/yaml.rs` (frontmatter YAML 無害化 leaf + テスト T-FC003..007, T-FC012、フェンス外限定の `neutralize_yaml_markers_outside_fences` + テスト T-FC030..033)
- `src/fetch/converter.rs` (frontmatter 組み立て + テスト T-FC001, T-FC002, T-FC008。`format_with_frontmatter` は `neutralize_yaml_markers_outside_fences` を経由する)
- `tests/output_injection.rs:T-C032, T-C041` (fetch 出力での閉じたフェンス保存/閉じないフェンスの fail-closed 切り替えを、実際の変換経路越しに固定する統合テスト)
- `src/search/engine.rs` + `src/search/engine/tests.rs:T-SE010` (search 出力中和)
- `src/slack/format.rs` (`format_slack_output` が共有 leaf `src/yaml.rs` の `write_yaml_str`/`neutralize_yaml_markers` を再利用。フェンス非対応のまま直接経由することを `src/slack/format/format_tests.rs:T-SK088` が pin する)
- `docs/audit/2026-06-24-020601-adr-gaps.md` (本 ADR の根拠 audit、候補 #2)
