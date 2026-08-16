---
status: "accepted"
date: 2026-05-15
decision-makers: thkt (project owner)
---

# Switch Search Backend from Gemini Grounding to Brave Search API

> **Implementation Status:** Completed 2026-05-16 on branch `feat/brave-migration-v2`. The Confirmation checklist below and References to `src/gemini/*`, `src/search/bilingual.rs`, `Lang::apply_to_query` reflect the pre-migration state at the time this ADR was authored; those files and APIs no longer exist in the codebase.

## Context and Problem Statement

`.claude/OUTCOME.md` は scout を「AI エージェントが一次ソースを直接読むためのツール」と定義し、Non-goals に「LLM による要約や統合判断を scout 側で代替すること」を明示する。

しかし現在の実装は Gemini API の Google Search grounding を使用しており、OUTCOME と以下の点で矛盾する:

1. `src/gemini/grounding.rs:27` が抽出する URL は Google の redirect URL (`vertexaisearch.cloud.google.com/grounding-api-redirect/...`) でラップされ、真のソース URL が AI エージェントに届かない
2. `src/search/engine.rs:185` の `format_search_results` は Gemini が生成した grounded answer (中間 LLM 要約) を `Search Result` セクションとして出力する

どの検索バックエンドに切替えれば OUTCOME に整合するか。

## Decision Drivers

* OUTCOME 整合: 真の URL を返し、中間 LLM 要約を含まないこと
* 持続的無料運用: 個人 dev が月額課金なしで永続利用できること
* 構造的安定性: 第三者サービスのスクレイプ依存ではなく自立した index
* AI エージェント用途実績: Claude / 類似 agent ecosystem での採用事例

## Considered Options

* Brave Search API (Web Search エンドポイント)
* Google Custom Search JSON API
* Vertex AI Search (Grounding with Google Search)
* DuckDuckGo Instant Answer API
* SerpAPI
* Tavily
* Exa

## Decision Outcome

Chosen option: **Brave Search API (Web Search エンドポイント)**。OUTCOME 整合・継続無料枠・独立 index・Claude MCP 公式採用実績の 4 点が同時に満たされる唯一の候補。

### Consequences

* Good, because Web Search エンドポイントは真の URL を返し、grounded answer を含まない (OUTCOME 完全整合)
* Good, because `$5/月 (~1000q)` の継続無料クレジットで永続的に無料運用可能
* Good, because 30B+ pages の独立 index と SEO スパム削減ポリシーで「一次ソース」志向と一致
* Good, because Claude MCP 公式採用実績、scout の用途 (AI エージェント向け CLI) と一致
* Bad, because env var rename / 出力フォーマット変更 / コマンド意味変更を伴う **breaking change** (v1.1.2 → v2.0.0)
* Bad, because 既存ユーザは `BRAVE_SEARCH_API_KEY` 再設定が必要
* Bad, because Google index 規模より小さく、ニッチな技術ドキュメントで取りこぼし可能性
* Bad, because Brave サインアップに credit card 登録が必須 (無料枠内は課金されない)

### Design Refinements (post-DA review)

critic-design レビュー (2026-05-15) で以下の追加判断を確定。

#### Lang セマンティクスの再設計

旧 `Lang::apply_to_query` は Gemini への「回答言語指示」用に query 末尾へ `(日本語で回答)` 等を追記していた。Brave は文字列を素のトークンとして検索するため、この追記は query 汚染になる。新設計では:

* `apply_to_query` を廃止。query text は変更せず Brave へそのまま渡す
* `Lang::Ja` → Brave の `search_lang=ja` パラメータへマッピング
* `Lang::En` → `search_lang=en` へマッピング
* `Lang::Auto` → lang パラメータ無し（Brave に検出させる）

#### bilingual query expansion の廃止

`src/search/bilingual.rs` および `Lang::Auto` 経路での bilingual 展開を全廃止。理由:

* bilingual は Gemini が両クエリ結果を LLM 側で統合する前提の設計。Brave は素の URL リスト返却のため第 2 クエリは dedup と `depth` 切り詰めで大半が無駄になる
* Brave の `search_lang` パラメータで充分。両言語必要な場合は呼び出し AI エージェントが scout を 2 回呼ぶ責務分離
* API quota 節約 (Brave 無料枠 ~1000q/月)

#### JSON `data` schema の明示定義

`scout --json search` および `scout --json research` の `data` payload schema を v2.0.0 で確定:

```json
// scout --json search "query"
{
  "data": {
    "query": "...",
    "sources": [
      {"url": "...", "title": "...", "description": "..."}
    ]
  }
}

// scout --json research "query"
{
  "data": {
    "query": "...",
    "sources": [...],
    "fetched_pages": [
      {"url": "...", "markdown": "..."}
    ],
    "failed_urls": [
      {"url": "...", "reason": "..."}
    ]
  }
}
```

旧 schema の `data.answer` フィールドは廃止。下流スクリプトの `jq '.data.answer'` 系は全て破壊変更となる (v2.0.0 で告知)。

#### Default 出力フォーマット (非 JSON)

* `scout search "query"`: **URL のみ plain text** (1 行 1 URL、markdown なし、title/description は省略)。`xargs` / `wget` 等にそのまま流せる
* `scout research "query"`: markdown 維持。`Search Result` セクションは削除。残るのは `Sources` と `Fetched Pages`、および fetch に失敗した source があるときだけ出る `Failed URLs`

#### 0 件結果の UX

Gemini 由来の "No answer returned — the query may have been filtered by safety settings" は廃止。新動作:

* search markdown / plain: 空出力 (exit code 0)
* search JSON: `{"data": {"query": "...", "sources": []}}`
* research markdown: `Sources` セクションに `(no results)` と明記
* research JSON: 空配列で同上

#### SearchClient trait の再定義

ADR 旧文では「trait を維持」と書いたが、戻り値型 `GroundedResult { answer, sources }` が `Vec<SearchResult>` に変わる時点で実質的に **trait 再定義**。`MockSearch` (`src/search/engine.rs:255-296`) と wiremock ベースの全 http_tests (`src/gemini/client.rs:317-468`) は書き換え必須。

### Confirmation

* `src/gemini` module 全削除、新規 `src/brave` module に `SearchClient` trait (再定義) 実装
* 統合テストで `search` コマンドが redirect URL を返さないこと (`vertexaisearch.cloud.google.com` を含まないこと) を assert
* `format_search_results` および `Search Result` セクション削除を確認
* `BRAVE_SEARCH_API_KEY` を要求し、`GEMINI_API_KEY` への参照が code に残らないことを CI で確認
* `search --json` 出力に `data.answer` が含まれないこと、`data.sources` が定義 schema に従うことの integration test を追加
* `search` (default) 出力が URL のみ plain text であること (markdown 記法を含まないこと) の test を追加
* `Lang::apply_to_query` 廃止、query 文字列の Brave 送出時に変更されないことの unit test
* bilingual 関連コード (`src/search/bilingual.rs` および参照) 全削除確認

## Pros and Cons of the Options

### Brave Search API

* Good, because 独立 Web index (30B+ pages)、SEO スパム削減を公式に明言
* Good, because $5/月の継続無料クレジット、$5/1000q で予測可能
* Good, because Claude MCP 公式採用、Cohere / Mistral / Kagi も採用、ZDR 対応
* Bad, because credit card 登録必須 (検証用)
* Bad, because Google index 規模より小さい

### Google Custom Search JSON API

* Bad, because **2027-01-01 サービス終了が決定**、新規顧客受付済に停止 (`developers.google.com/custom-search/v1/overview?hl=ja`)
* 実質採用不可

### Vertex AI Search (Grounding with Google Search)

* Bad, because Gemini Grounding が内部で使用する同じ仕組み、redirect URL 問題が解決しない
* Bad, because grounded LLM response 前提の設計で中間 LLM 要約を回避できない
* Bad, because $4-6/1000q + LLM トークンコストの複合課金

### DuckDuckGo Instant Answer API

* Good, because 無料 / API キー不要
* Bad, because **SERP を提供しない** (instant answer のみ、Bing/Yandex の license 都合)
* Bad, because aggressive rate limiting / IP ban で scale しない

### SerpAPI

* Good, because Google 結果スクレイプで品質ストレート
* Bad, because 無料 250q/月は scout の用途で窮屈
* Bad, because Developer plan $75/月 ($15/1000q) で Brave の 3 倍
* Bad, because Google スクレイプ依存で構造的に脆弱

### Tavily

* Good, because 無料 1000 credits/月、credit card 不要
* Good, because AI agent 特化、180ms p50、JetBrains / IBM / Databricks 採用
* Bad, because `/extract` が scout の `fetch` と重複し設計分岐
* Bad, because AI 用整形が OUTCOME の「素」志向と微妙にズレ

### Exa

* Good, because セマンティック検索 + 真の URL、Cursor 等採用
* Bad, because $1000 one-time grant 切れ後は $7/1000q 課金で個人 dev に厳しい
* Bad, because セマンティック特化で scout の keyword 検索用途と一致度低い

## More Information

### Implementation Plan

| Phase | 内容 |
| ----- | ---- |
| 1 | `src/brave/client.rs` + `src/brave/types.rs` 新規実装。`SearchClient` trait 再定義 (`Vec<SearchResult>` 返却)、`BraveClient` で実装 |
| 2 | `Lang` セマンティクス再設計 (`apply_to_query` 廃止、`search_lang` パラメータマッピング)。`src/search/bilingual.rs` 削除 |
| 3 | `src/search/engine.rs` を `research` 専用に縮小。`format_search_results` 削除、Gemini answer 出力経路を排除 |
| 4 | `src/tools.rs` で `search` ハンドラを engine 経由なしの URL 羅列出力に再定義。`research` ハンドラは engine 経由維持 |
| 5 | JSON `data` schema 確定 (上記 Design Refinements 参照)、`--json` 経路の integration test 追加 |
| 6 | `src/gemini/*` 削除、env var を `BRAVE_SEARCH_API_KEY` に統一 |
| 7 | README (ja/en)、CHANGELOG、Cargo.toml バージョン更新 (v2.0.0) |

### Migration Strategy

一発切替。後方互換維持なし。scout は CLI ツールで in-process state を持たないため、deprecation 期間を設けず即時切替が安全。

### Rollback Plan

git revert + v1.1.2 タグからの patch release。Brave サービス停止等の場合は本 ADR を superseded に変更し、新規 ADR で代替バックエンドを選定。

### Success Criteria

* `scout search "query"` 出力に `vertexaisearch.cloud.google.com` ドメインが含まれない
* `scout search "query"` 出力に `Search Result` セクションが含まれない
* 1000 queries/month の無料枠内で個人 dev が運用可能
* 既存 `fetch` / `repo-*` コマンドは無影響

### Reassessment Triggers

| Trigger | Action |
| ------- | ------ |
| Brave Search API が新規受付停止 / 大幅値上げ | 本 ADR を superseded、Tavily / Exa を再評価 |
| OUTCOME.md の Behavior 変更 | 本 ADR の前提を再確認 |
| Brave index 取りこぼしがユーザから頻繁に報告される | 補完バックエンド (Tavily 併用等) 検討 |

### References

* `.claude/OUTCOME.md` (OUTCOME 整合根拠)
* `src/gemini/client.rs`, `src/gemini/grounding.rs` (廃止対象の現実装)
* `src/search/engine.rs:50`, `src/search/engine.rs:185` (orchestration および削除対象)
* `src/search/lang.rs:13-19` (`apply_to_query` 廃止対象)
* `src/search/bilingual.rs` (廃止対象、bilingual expansion)
* `src/tools.rs:214` (`SuccessEnvelope.data` payload 起点)
* `src/envelope.rs:170-177` (`SuccessEnvelope::data` 型定義)
* Brave Search API: https://brave.com/search/api/
* Brave Web Search documentation (getting started): https://api-dashboard.search.brave.com/app/documentation/web-search/get-started
* Brave Web Search query parameters (一次ソース): https://api-dashboard.search.brave.com/app/documentation/web-search/query
* Google Custom Search 廃止: https://developers.google.com/custom-search/v1/overview?hl=ja

## Addendum (2026-08-17): `used_raw_fallback` は data ではなく degradation channel で出す

Decision Outcome の JSON schema block が `data.fetched_pages[]` の各要素に `used_raw_fallback` を書いていたが、実際には出ない。`FetchResult` の `used_raw_fallback` と `decode_uncertain` は `#[serde(skip_serializing)]` で、`data.fetched_pages[i]` は `{url, markdown}` だけになる。フラグは `collect_research_degradations` (`src/tools/query.rs`) が `degraded_reasons` と `notes` へ載せる。

この設計は本 ADR より後 (issue #241) に決めたもので、事故ではない。取得の質に関する情報を data ではなく degradation channel へ集めると、consumer は `degraded` 1 つを見るだけで「本文をそのまま信用してよいか」を判断できる。ADR の schema block だけが元の形のまま残っていた。

`jq '.data.fetched_pages[].used_raw_fallback'` と書いた consumer は `null` を受け取る。`degraded_reasons` に `READABILITY_FALLBACK` があるかで判定する。schema block を実装へ揃えたが、`data.fetched_pages[i]` の要素形を assert するテストは無い。`[T-TS028]` は `fetched_pages` が配列であることまでしか見ないため、この drift はテストでは検出できなかった。

Design Refinements の research 出力セクション一覧に `Failed URLs` を足した。`format_report` は `format_failed_urls` を無条件に呼び、失敗した source があれば `## Failed URLs` を出す (`[T-SE014]`)。このセクションは Gemini から Brave への切り替えより前からあり、本 ADR の「2 セクションのみ」という書き方が引き写しの取りこぼしだった。
