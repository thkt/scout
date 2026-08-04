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

Chosen option: Option A。出力境界の中和を `src/markdown.rs` と `src/fetch/converter.rs` の少数の `pub(crate)` 関数に集約し、全 backend がそれを経由する。URL は http/https のみ clickable link にし、他 scheme は不活性な `text (url)` へ流す。markdown メタ文字は escape し改行は空白へ畳む。YAML 値は backslash escape し、本文行頭の `---`/`...` は `***` へ書き換える。`<script>`/`<style>` 等 active HTML の除去は Readability (dom_smoothie) と html2md (fast_html2md) に委譲し、scout は変換後の markdown/YAML 層を担う。未知 scheme・難読化 (大小文字、先頭空白)・制御文字は素通しせず fail-closed で中和する。

Option B は policy 管理コストが高く per-site ルールが脆いため却下。エージェント consumer は全取得元に対し予測可能な保証を要する。Option C は防御を下流 (時に人間の端末) に押し付け誤りやすいため却下。

### Consequences

- Good, because 未知 scheme・難読化 URL・制御文字を素通しせず fail-closed で中和し、`javascript:` URL が clickable link になることを防ぐ
- Good, because fetch/search/Slack/GitHub が同じ中和関数を経由し、新 backend も同じ防御を継承する
- Good, because HTML 層の script 除去は変換ライブラリに委譲し、scout は markdown/YAML 層の保証に集中する
- Good, because escape 系関数は clean input で借用を返し common path でゼロアロケーション
- Bad, because `\0\n\r\t` 以外の制御文字 (ESC, BEL) は素通しし、人間が端末で読む場合に terminal 描画へ影響しうる (主 consumer はエージェントのため受容)
- Bad, because 本文 markdown は意図的に rendered のまま渡すため、markdown を命令として naive に読むエージェント実装は見出し本文を誤解しうる (見出しレベル shift と JSON envelope 構造で緩和)
- Bad, because HTML 層の script 除去は fast_html2md に依存し scout は再検証しないため、ライブラリが退行すると実行可能 markup が漏れうる

### Confirmation

中和点ごとに専用テストが存在する。markdown 層は `src/markdown.rs` の `[T-MD001..T-MD018]` が escape/改行畳み/scheme allowlist/難読化 fail-closed/見出し shift を網羅する。YAML 層は `src/yaml.rs` の `[T-FC003..T-FC007]` が値 escape と document marker 書き換えを、`src/fetch/converter.rs` の `[T-FC008]` が frontmatter 注入防止を網羅する。search 層は `src/search/engine/tests.rs` の `[T-SE010]` が source URL の `javascript:` scheme を不活性 text として出すことを assert する。新しい出力経路を足す際は、これらの境界関数を経由しているかをテストで確認する。

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

| 関数                      | 場所                    | 中和内容                                                                                 |
| ------------------------- | ----------------------- | ---------------------------------------------------------------------------------------- |
| `md_link`                 | src/markdown.rs:26-40   | http/https のみ clickable、他 scheme は不活性 text、空白/制御文字 URL を拒否             |
| `escape_md_inline`        | src/markdown.rs:44-57   | `\|` `[]()` escape、改行を空白へ畳む                                                     |
| `escape_md_link`          | src/markdown.rs:6-19    | link target の `[]()` escape、改行畳み                                                   |
| `sanitize_heading`        | src/markdown.rs:64-71   | 見出し内改行を空白へ                                                                     |
| `shift_headings`          | src/markdown.rs:109-140 | ページ見出しを深い level へ下げ scout 構造との衝突を防ぐ (code fence 内・非 ATX は skip) |
| `escape_yaml`             | src/yaml.rs:57-79       | `\` `"` `\n\r\t` を escape、`\0` を除去                                                  |
| `neutralize_yaml_markers` | src/yaml.rs:15-34       | 行頭 `---`/`...` を `***` へ書き換え、indent/inline は不変                               |

### HTML 層の分担

`html2md::rewrite_html` (src/fetch/converter.rs) と Readability (dom_smoothie) が `<script>`/`<style>` 等を除去する。scout 側は変換後テキストに対し上表の markdown/YAML 保証を上乗せする二層構造で、HTML パース自体は再実装しない。

### 参照

- `src/markdown.rs` (markdown 層中和 + テスト T-MD001..018)
- `src/yaml.rs` (frontmatter YAML 無害化 leaf + テスト T-FC003..007, T-FC012)
- `src/fetch/converter.rs` (frontmatter 組み立て + テスト T-FC001, T-FC002, T-FC008)
- `src/search/engine.rs` + `src/search/engine/tests.rs:T-SE010` (search 出力中和)
- `src/slack.rs:395-435` (`format_slack_output` が共有 leaf `src/yaml.rs` の `write_yaml_str`/`neutralize_yaml_markers` を再利用)
- `docs/audit/2026-06-24-020601-adr-gaps.md` (本 ADR の根拠 audit、候補 #2)
