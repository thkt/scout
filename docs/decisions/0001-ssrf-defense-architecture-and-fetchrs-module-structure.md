---
status: "accepted"
date: 2026-05-13
decision-makers: thkt (project owner)
---

# SSRF Defense Architecture and fetch.rs Module Structure

## Context and Problem Statement

scout は Web fetch + GitHub repo exploration CLI で、user 入力 URL を取得する責務がある。SSRF 攻撃を防ぐため fetch 経路は複数の防御層 (DNS pre-check, redirect 制御, headless browser subrequest 制御) を持つ。

`Scout` 構造体は 2 つの `reqwest::Client` を保持: `http` (auto-redirect、API 用) と `fetch_http` (`Policy::none()`、user URL 用)。field コメントは現状を記述するが、未来 contributor 向けの rule (どちらを使うべきか) は明示されておらず、型でも強制されない。

`fetch.rs` (1456 行) は `js-rendering` feature を `#[cfg]` で plain HTTP path と同居させており、`#[cfg_attr(...)] allow(dead_code)` の散在が構造的 smell を生んでいる。両者ともに「現状の判断」を明文化する必要がある。

## Decision Drivers

* SSRF contract 違反は silent security incident に直結 (型で守られない不変条件)
* 個人 OSS scale で型強制 (Newtype) と code review 依存のコスト比較が必要
* `fetch.rs` の module split は thin-extract heuristic の locality を損なう可能性

## Considered Options

* Option A: Newtype 化 + module split (full enforcement)
* Option B: ADR で contract 明文化 + 現状構造維持 (lightweight)
* Option C: コメント拡充のみ (no ADR)

## Decision Outcome

Chosen option: Option B, because 個人 OSS scale の review 負荷とコード変更コストのバランスで、ADR が「未来 contributor 向け rule」を提供しつつ実装変更を回避できる。Newtype 化と split は trigger 条件 (incident or 規模超過) まで保留する。

### Consequences

* Good, because 新規 command 追加時の SSRF 配慮 path が明文化された
* Good, because fetch.rs の現状構造を意図ある選択として記録、split trigger を数値化
* Bad, because 型強制ではないので contract 違反は code review にのみ依存
* Bad, because `fetch.rs` の `allow(dead_code)` smell は残る

### Confirmation

新規 command PR で `self.http(...)` 呼び出し箇所に対し reviewer が URL source (user 入力か信頼済みか) を確認する。CI で `fetch.rs` の行数を check し、2000 行超過時に warning (将来追加検討)。

## Pros and Cons of the Options

### Option A: Newtype 化 + module split

`SsrfSafeClient` newtype 導入と `fetch.rs` を `fetch/{download,heuristic,browser}.rs` に split。

* Good, because contract 違反が compile error として検出される
* Good, because module boundary が明確になり、`#[cfg]` smell が消える
* Bad, because 既存 6 command のリファクタリング cost が大きい
* Bad, because fallback heuristic と orchestrator が別 module になり、code review 時の文脈分断

### Option B: ADR 明文化 + 現状維持 (採用)

ADR で SSRF contract と fetch.rs 構造判断を記録、Newtype/split は trigger 条件で再評価。

* Good, because 実装変更ゼロで未来 contributor への rule を提供
* Good, because trigger 条件 (incident or 規模超過) で再評価できる
* Bad, because 型強制ではないので review 依存
* Bad, because `allow(dead_code)` smell は残る

### Option C: コメント拡充のみ

field コメントに contract を追記、ADR は作らない。

* Good, because 最小コスト
* Bad, because rule の根拠 (なぜ Newtype を選ばないか) が記録されない
* Bad, because contract がコメントに散在し、未来 contributor が全体像を把握しにくい

## More Information

### Implementation Guidelines

| Client              | 用途                                                    | Redirect Policy                                       |
| ------------------- | ------------------------------------------------------- | ----------------------------------------------------- |
| `Scout::http`       | Gemini API / GitHub API / 信頼済みエンドポイント        | `limited(5)` (reqwest 既定)                           |
| `Scout::fetch_http` | user 入力 URL を扱う全 fetch 経路                       | `Policy::none()` + 手動 redirect + per-hop SSRF check |

新規 command 追加時のルール:

* user 入力 URL を含む経路は MUST `fetch_http` を使う
* 信頼済みエンドポイントなら `http` でよい

### Reassessment Triggers

| Trigger                                                              | アクション                            |
| -------------------------------------------------------------------- | ------------------------------------- |
| SSRF contract 違反 incident 発生                                     | `SsrfSafeClient` newtype 化を即検討   |
| 新規 command 追加で計 9 以上                                         | Review 漏れリスク上昇、Newtype 化検討 |
| `fetch.rs` 行数 > 2000                                               | Module split 検討                     |
| `#[cfg(feature = "js-rendering")]` 累積行数 > plain path             | Module split 検討                     |

### 参照ファイル

* `src/tools.rs:128-137` (dual HTTP client 定義)
* `src/fetch.rs:528-612` (manual redirect loop, `download` function)
* `src/fetch.rs:466-488` (CDP `Fetch.RequestPaused` interceptor)
* `docs/audit/2026-05-13-undocumented-decisions.md` (本 ADR の根拠 audit)
