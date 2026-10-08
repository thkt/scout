---
status: "proposed"
date: 2026-10-08
---

# 明示的な非HTML text/XML を一次ソース本文として返す

## Context and Problem Statement

[Issue #491](https://github.com/thkt/scout/issues/491) は、受理済みの非HTML text/XMLについて、通常/raw・Markdown/JSONでタグ・属性・文字参照のリテラル・コード・改行を保持する要求を合意している。基準版は `a46fb3ea0c2469b599339136824cd8f690301002`（2026-10-08）。Issueのproxy観測では、XMLのタグと属性、Markdownのコードと改行がHTML変換で失われた。基準版の `fetch_page`（`src/fetch.rs`）でも、`OtherText` がHTML抽出へ進む分岐を確認できる。

[DR-0029](0029-explicit-plain-text-fetch.md) は同じ基準版でproposedであり、実装済みのtext/plain経路と、その媒体だけに限った方式選択を記録している。本記録は後続の媒体拡張について実装方式をレビューへ提示する。元DRの本文と過去の確認記録は変更しない。

## Considered Options

- 受理済み非HTML媒体を既存の `plain_text_result` に直接渡す。
- 媒体ごとにXML解析やMarkdown変換を追加する。

## Decision Outcome

直接渡す方式を実装する。`check_content_type`（`src/fetch/download.rs`）の受理範囲・分類は維持し、`fetch_page` は `PlainText` と `OtherText` をHTML抽出・変換から分離する。対象は `text/html` を除く `text/*`、`application/xml`、`application/xhtml+xml` を除く `application/*+xml`。媒体ごとの解析は一次ソースのリテラルを再解釈し、追加の変換・保守を必要とするため採用しない。

HTML・XHTML・媒体不明は従来の抽出と自動JS判定を使う。明示的な `--js` は媒体にかかわらず既存の強制レンダリング要求を優先する。application/json等の受理範囲は拡張しない。

`fetch_page` では非HTMLかつ `!opts.js` の場合に早期returnするため、後段の自動JS判定で同じ媒体分類を再検査しない。明示的なJS要求では `need_js` がtrueとなり、薄い抽出のfallbackは既存の `!need_js` 条件で抑止される。`!opts.raw` 条件、ブラウザー実行後の再抽出と失敗処理は維持する。

### Consequences

非HTML本文はHTMLメタデータを抽出せず空のフロントマターを付け、Readability失敗とは扱わない。`FetchResult::with_heading_offset`（`src/fetch/converter.rs`）のplain text分岐を共有し、researchに組み込む際も見出しを変換しない。HTTP charsetのデコードと不確実性は既存の `download` から受け渡す（[DR-0013](0013-charset-detection-and-decode-policy.md)）。XML宣言のencodingを新たなデコード入力にはしない。

`format_body`（`src/fetch/converter.rs`）による初期YAML中和はMarkdown/JSONの両形式で共有する。`format_fetch_output`（`src/tools.rs`）が適用する既定の100,000バイトの出力上限と、`truncate_and_reneutralize`（`src/yaml.rs`）による切断後の再中和はMarkdown出力だけに適用する（[DR-0014](0014-output-injection-defense-for-agent-consumers.md)）。`Scout::fetch`（`src/tools/query.rs`）は出力切断前の `FetchResult` をJSON用にシリアライズし、`CommandOutput::into_envelope`（`src/envelope.rs`）がそのdataを使うため、JSONの `data.markdown` は既存のレスポンス取得上限内の切断前本文を含む。この形式ごとの既存動作は変更しない。非HTMLの `<script>` 等は一次ソースのリテラルとして保持する。全バイトの無条件な一致や、消費者がMarkdownを表示する際の生HTML解釈まで保証するものではない。HTMLのactive markup除去と段落改行の契約は変更しない（[DR-0027](0027-fetch-body-line-break-handling.md)）。SSRF・redaction・JSONスキーマ・終了コードも既存経路を使う。

### Confirmation

`tests/fetch_media_type.rs` のT-C049にXML・Markdown・text/xml・RSSの原文一致を統合し、通常/raw・Markdown/JSON、空のメタデータ、文字参照・属性・コード・末尾空行と自動JS抑止を確認する。T-C050はHTML/XHTMLの対照、T-C051は非HTML媒体のYAML中和を確認する。旧T-C050の非HTML変換期待値は新要求と矛盾するため除き、自動JS抑止の検出条件はT-C049へ移す。分類・拒否・charset、見出しオフセット、切断後のYAML防御は既存検証を再利用する（DR-0029のConfirmation参照）。

基準版 `a46fb3ea0c2469b599339136824cd8f690301002` の隔離コピーへ回帰テストの差分だけを適用し、`SCOUT_NETWORK_TESTS=1 cargo nextest run --profile ci -E 'binary(fetch_media_type) & test(non_html_source_survives_normal_raw_markdown_and_json)'` を実行した。XMLのタグ・属性・文字参照・改行が変換された出力により原文一致assertionが失敗した（1 failed、終了100）。修正後の検証は設定済みdefault/all-featuresのcheckで行う。実Webの全XML/Markdownサービス、実行時間や不安定さの改善は未確認。

2026-10-08のホストcheckでは、修正後の原文保持テストがdefault/all-featuresの両構成で成功した。ただし、その版のブラウザー後始末テストは媒体不明の応答を使っており、非HTML媒体と明示的な `--js` の競合条件は未検証だった。独立評価の指摘を受け、`tests/exit_code_contract/browser_cleanup.rs` の共通fixtureを明示的な `application/xml` 応答へ変更した。js-rendering有効のUnix環境でT-BGC005/T-BGC006がHTTP取得後のブラウザー起動、タイムアウト時の終了124、SIGINT/SIGTERM時の終了130/143、所有プロセスとprofileの後始末を確認する。原文保持分岐から `!opts.js` を落とす回帰は、ブラウザー起動待ちのassertionで失敗し、終了0も許容しない。テスト数とCLI起動数は増やさず、媒体不明と後始末を組み合わせた旧fixtureの条件は失う。媒体不明の従来経路は既存検証を残す。このfixture変更後の実行結果はホストcheckと独立評価で確認する必要があり、以前の成功を流用しない。

後続のホストcheck-2は、XML fixtureへ変更した版（独立評価の対象ID `678a7e0324b31e9caf0c1c26318be3bd8c046a6383a6cea31efb526b7f779f78`）を実行した。defaultは850件、all-features・ignoredを含む構成は870件が成功し、後者にはT-BGC005/T-BGC006が含まれる。独立評価ではR1-1の検出条件を再確認した一方、後段の不要な媒体guard（R2-1）と、出力上限の形式別条件の説明不足（R2-2）を指摘した。今回、guardを局所削除し、英日READMEと本記録を実際の出力経路へ合わせた。この変更後の全体checkと独立再評価は未完了であり、check-2を変更後の成功や受入として扱わない。基準版RedはXMLの失敗を示すが、後続MarkdownケースのRedや実装に先行したTDD順序の証拠ではない。fake browserによる起動・中断・後始末の保証は、実Chromiumでの非HTML＋明示的JSの正常描画を保証しない。

### Reassessment Triggers

媒体不明の推測、受理範囲、XML宣言のデコード、または `--js` の優先順位を変える要求が来た場合は、別途合意範囲と出力契約を確認する。
