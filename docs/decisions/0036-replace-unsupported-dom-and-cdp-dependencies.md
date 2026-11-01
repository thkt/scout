---
status: "accepted"
date: 2026-10-25
decision-makers: thkt (project owner)
---

# HTML DOMとCDP接続の依存を置き換える

## Context and Problem Statement

HTML変換は `htmd` と `markup5ever_rcdom` に依存していた。`markup5ever_rcdom` はテスト用のDOMであり、本番向けDOMとしてのサポートを提供しないと公開説明に明記されている。バージョンを上げても、このサポート方針は変わらない。

JSレンダリングの `chromiumoxide` はWebSocketと暗号処理の依存を経由して `generic-array 0.14.7` を取り込んでいた。`generic-array` の開発自体は続いているが、0.14系は公式に非推奨となっている。`crypto-common 0.1.7` による完全一致の指定があるため、scout側で0.14.9や1系へ上げることはできなかった。

利用者から、両方の代替を進め、CDP接続の置き換えも今回の変更に含める指示を受けた。

## Decision Drivers

- 非推奨の配列クレートと、本番利用をサポートしないDOMへの依存を外す。
- 表、コード、リンク、本文の抑制規則を維持する。
- CDPのSSRF検査、タイムアウト、signal処理、ブラウザの終了処理を維持する。
- 外部データは受信時に型を確定し、移行に不要な並行処理を増やさない。

## Considered Options

1. 現行の依存を維持して上流の更新を待つ。
2. `htmd` と `chromiumoxide` をforkし、依存だけを変更する。
3. DOMを `dom_query` へ移し、必要なMarkdown出力とCDP操作をscoutで実装する。

## Decision Outcome

方式3を採る。HTML解析には `dom_query 0.29` と `html5ever 0.40.1` を使う。`src/fetch/converter/html.rs` の `convert` が従来どおり `scripting_enabled = true` で解析する。`dom_query::Document::from` はこの設定がfalseになるため使わない。

`dom_query` の組み込みMarkdown出力を使う試作では、既存77テスト中31件で出力が変わった。表セルの位置、コードの空白やフェンス、リンク先の括弧に影響したため、そのまま採用しない。変換は `src/fetch/converter/html.rs` と `src/fetch/converter/html/table.rs` に移し、既存の出力仕様を実装する。旧 `htmd 0.5.6` 経路から取得した86入力の出力をfixtureとして残す。

CDP接続は `tokio-tungstenite 0.30` を使う。`src/fetch/cdp/connection.rs` の `Connection` は単一ページの作成、sessionの接続、navigation、HTML取得、browser closeに必要な操作だけを扱う。コマンド応答を待つ間に `Fetch.requestPaused` を処理し、既存の `check_browser_request` へURLを渡す。ページのload完了は `frameId` と `loaderId` の組で照合する。

既存のSOCKS5 proxyによる接続時IP検査と、`ChromiumProcess` によるprocess groupの停止・profile削除は維持する。WebSocketハンドラとinterceptor用の追加タスクは使わない。受信メッセージを型へ変換し、interceptorのコマンドが失敗した場合はnavigationにも失敗を返す。

HTML変換はI/Oエラーを返すAPIを使わなくなったため、到達不能になった `MarkdownConversion` と、その分類だけを検査するT-FC018を削除する。CLIの引数、出力形式、他のエラー分類は変更しない。

### Consequences

`generic-array`、`markup5ever_rcdom`、`htmd`、`chromiumoxide`、`async-tungstenite` が依存グラフから外れる。WebSocket側の暗号処理は `hybrid-array` を使う経路になる。Renovateの旧 `htmd` / `markup5ever_rcdom` 用ルールも削除する。

Markdown出力とCDPの限定的な操作はscoutで保守する。`htmd` の全オプションや、汎用ブラウザ操作のAPIを実装するものではない。

直接依存の最新公開版への追随を、バージョン重複の解消より優先する。重複回避のために一度追加したRenovateの `allowedVersions` は削除する。`dom_smoothie` / `dom_query` / `html5ever` の更新は同じPRで確認する。

`dom_smoothie 0.19.0` は変換側と同じ `dom_query 0.29` を使うため、DOM系の依存は1系列にそろう。抽出処理と変換処理の間はHTML文字列で受け渡し、DOM型は混ぜない。

`core-foundation` と `syn` も、現在の公開版の上流依存が異なる系列を要求するため併存する。`hyper-util 0.1.21` は `system-configuration 0.7` 経由で `core-foundation 0.9` を使い、TLS検証側は0.10を使う。複数のマクロクレートは `syn 2` を要求し、他の現行クレートは3を使う。重複を消すための機能削除や独自forkは行わず、上流の公開版更新に追随する。

### Confirmation

HTML変換の既存テストに加え、`src/fetch/converter/html/tests.rs` のT-HMD001からT-HMD006で、基本書式、リスト、引用、数式、抑制対象、旧実装との出力比較を確認する。86件の一致は、任意のHTMLで全出力が一致することの証明ではない。

CDPの模擬通信は `src/fetch/cdp/connection/tests.rs` のT-CDPW001からT-CDPW005で確認する。実ブラウザによるHTML取得とprofile削除は、既存のT-F051 / T-F057を実行する。通常構成と全機能のテスト、fmt、clippy、依存チェックを実行し、結果を変更報告に記録する。

## More Information

- [generic-arrayの移行案内](https://github.com/fizyk20/generic-array#upgrading-from-014-or-using-with-hybrid-array-04)
- [markup5ever_rcdomのサポート方針](https://docs.rs/crate/markup5ever_rcdom/0.39.0%2Bunofficial)
- [dom_query](https://github.com/niklak/dom_query)
- [tokio-tungstenite](https://github.com/snapview/tokio-tungstenite)

DR-0014のHTML本文の抑制規則、DR-0025のpre内の改行、DR-0026のDOMからのコード本文取得、DR-0027の改行仕様を引き継ぐ。これらの文書にある `htmd` のAPIや内部処理の説明は、移行前の実装記録となる。
