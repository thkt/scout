---
status: "proposed"
date: 2026-10-08
---

# 明示的な text/plain を HTML として解釈しない

## Context and Problem Statement

[Issue #479](https://github.com/thkt/scout/issues/479) の合意範囲は、通常/raw・Markdown/JSONで text/plain の型引数・リテラル・改行を保持し、明示的な非HTMLにHTMLの薄さを理由とするJSレンダリングを行わないことである。基準は v2.6.1 / `388db1f13c741e04a0babe5d8a1b9ce06242ad4d`。実装担当に方式選択が委ねられており、本記録はその選択をレビューへ提示する。acceptedな既存DRの本文は変更しない。

## Considered Options

- デコード後の plain text を出力境界へ直接渡す。
- HTMLエスケープして `<pre>` を組み立て、既存のHTML変換へ渡す。

## Decision Outcome

直接渡す方式を実装する。`download`（`src/fetch/download.rs`）は最終応答の媒体種別を保持する。`fetch_page`（`src/fetch.rs`）は明示的な text/plain を抽出・HTML変換から分離し、`plain_text_result`（`src/fetch/converter.rs`）へ渡す。通常/rawの区別はこの本文には適用しない。HTMLとしてタグや文字参照を解釈せず、Markdown構文も変換しない。`FetchResult::with_heading_offset`（`src/fetch/converter.rs`）が本文種別を使って見出し変換を分岐し、fetch・researchの出力境界でも見出し風リテラルと末尾空行を保持する。HTML由来の見出しは従来どおり変換する。`<pre>` 方式はHTMLの空白・エンティティ処理とフェンス整形に依存するため見送る。

自動JS判定は text/html・application/xhtml+xml、または媒体種別不明の場合だけ行う。明示的な `--js` は既存の強制レンダリング要求として維持する。他の受理済み text/XML 媒体の変換方式は拡張しない。

### Consequences

plain text はHTMLメタデータを持たず、空のフロントマターを付ける。Readability失敗として扱わない。[DR-0013](0013-charset-detection-and-decode-policy.md) のデコードと不確実性、[DR-0010](0010-scout-local-json-envelope-contract.md) のJSON形、既定出力上限を維持する。取得前・各リダイレクトのSSRF検証も既存経路を使う。

[DR-0014](0014-output-injection-defense-for-agent-consumers.md) のHTML除去はHTML変換経路で維持する。plain text の `<script>` 等は実行可能HTMLとして解釈せず、一次ソースのリテラルとして保持する。新経路も同じ `format_body` を経由し、フェンス外または未閉鎖フェンス中のYAMLマーカーを中和する。したがって本文の全バイトが無条件に原文と一致する契約ではない。Markdownとして表示する消費者による生HTMLの解釈まで保証するものでもない。

[DR-0027](0027-fetch-body-line-break-handling.md) のHTML段落の改行契約は変更しない。明示的なplain textの改行保持を、そのHTML契約の例外として一般化しない。

### Confirmation

`tests/fetch_media_type.rs` は既存のmock proxyを使い、SSRFを緩めず実CLIの取得から出力までを通す。短いplain textの通常/raw・Markdown/JSON、媒体種別の大小文字とパラメーター、JSらしいリテラル、setext風の行・行頭 `#`・末尾空行、HTML/XHTML変換対照、他の非HTML媒体の自動JS抑止、YAML中和を確認する。分類の既存テストは受理に加えて経路分類を照合する。T-TS039（`src/tools/query_tests.rs`）とT-SE021（`src/search/engine/tests.rs`）は出力境界での本文保持を確認する。`truncate_and_reneutralize`（`src/yaml.rs`）は中和済み入力が切断されない場合は借用結果を直接返し、切断時だけフェンスを再検査する。T-FC105（`src/yaml.rs`）は未切断の未閉鎖フェンスで不要なコピーを防ぎ、既存のT-FC087（`src/tools/query_tests.rs`）とT-FC088（`src/search/engine/tests.rs`）は切断後のYAML防御を維持する。

### Reassessment Triggers

他媒体の忠実な出力を要求する場合、媒体種別不明の推測方式を変える場合、または `--js` と明示的な非HTMLの優先順位を変える場合は、別途範囲と出力契約を確認する。
