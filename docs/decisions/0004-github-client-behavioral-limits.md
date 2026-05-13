---
status: "accepted"
date: 2026-05-13
decision-makers: thkt (project owner)
---

# GitHub Client Behavioral Limits

## Context and Problem Statement

GitHub API クライアント (`src/github.rs`, `src/github/helpers.rs`) には 3 つの silent な behavioral limit が code に埋め込まれており、CLI 利用者 / agent が programmatic に依存している:

1. `src/github.rs:153-165`: HTTP 403 を `RateLimited` と `Forbidden` に分岐する際、`x-ratelimit-remaining` ヘッダが不在の場合は `Forbidden` (retry 抑止) と判定される。secondary rate limit や header 配信失敗時に retry path が silently 失われる
2. `src/github.rs:239,252,265`: `per_page` パラメータが `u8` (max 255) で受け取られ、`.min(100)` で silent clamp される。caller が 200 を渡しても 100 件しか返らず、log / error / warning 無し
3. `src/github/helpers.rs:161-177`: `filter_tree_entries` の glob pattern は filename component (`rsplit('/').next()`) のみに match。`src/*.rs` のような path glob は silent に no-match

これらはいずれも GitHub API 仕様外の behavior decision (拡張) であり、ADR で明文化する必要がある。

## Decision Drivers

* CLI 利用者の data integrity (silent truncation を防ぐ)
* rate limit retry の reliability (header 不在で retry を失わない)
* glob semantics の intuitive 一致 (`src/*.rs` で path scope を期待)

## Considered Options

* Option A: ADR で behavioral rule を明文化 + 必要な code 修正
* Option B: 現状維持、code コメントだけ追加
* Option C: GitHub API spec に厳密に従う (header 不在 = 仕様外として error)

## Decision Outcome

Chosen option: Option A, because silent behavior は CLI / agent 利用で debug 困難な class of bug。明文化と code 修正で programmatic 利用性を高める。

### Rule 1: 403 + missing rate-limit header → RateLimited (retry default)

`x-ratelimit-remaining` ヘッダの値で判定:

| Condition                                  | Classification |
| ------------------------------------------ | -------------- |
| header 存在 + `remaining == 0`              | RateLimited (retry) |
| header 存在 + `remaining > 0`               | Forbidden (auth misconfig, no retry) |
| header 不在                                 | **RateLimited (retry default)** ← 新規 |

GitHub docs は 403 で header を必ず付けるとは保証していないため、unknown は retry に倒す。

### Rule 2: per_page > 100 → explicit error, no silent clamp

`per_page` 受領値が 100 を超える場合:

* explicit な `DataError` を返却 (exit 65 EX_DATAERR)
* error message に "GitHub API limits per_page to 100" を含める
* silent clamp は廃止

caller が GitHub API spec を知らない場合でも error で確実に通知される。

### Rule 3: filter_tree_entries glob is path-scoped

`filter_tree_entries(entries, path, pattern)`:

* `pattern` は `entry.path` (full repo-relative path) に対して match
* filename-only match の旧 behavior は廃止
* `src/*.rs` のような glob が intuitive に動作

### Consequences

* Good, because CLI script が silent data truncation を検出可能 (per_page 違反は error)
* Good, because rate limit retry が header 不在でも確実に発動
* Good, because glob semantics が intuitive、`src/*.rs` 等が動作
* Bad, because behavior 変更で既存 caller (per_page > 100 や filename-only glob 依存) が break する可能性 (semver bump 検討)
* Bad, because header 不在を retry に倒す decision は false-positive retry を増やす (secondary rate limit が稀に出る場合)

### Confirmation

* `src/github.rs:153` 周辺で 403 + header 不在の unit test (mock response)、`RateLimited` に分類されることを確認
* `src/github.rs:239` 周辺で per_page=200 受領 → `DataError` returned の test
* `src/github/helpers.rs:161` で `filter_tree_entries(entries, None, Some("src/*.rs"))` が `src/foo.rs` を含むことの test

## Pros and Cons of the Options

### Option A: ADR + code 修正 (採用)

* Good, because silent behavior が明示的 error / typed path に置き換わる
* Good, because CLI / agent の自動判断が信頼できる
* Bad, because behavior 変更が breaking、既存 caller が影響
* Bad, because header 不在 retry は false-positive retry を増やす trade-off

### Option B: コメントだけ追加

* Good, because behavior 変更ゼロ
* Bad, because silent truncation / silent retry 抑止が残る
* Bad, because CLI / agent caller が動作を予測できない

### Option C: GitHub API spec に厳密従う (header 不在 = error)

* Good, because retry false-positive を避ける
* Bad, because GitHub API が header 配信を保証していないため、本来 retryable な状況を error に倒す
* Bad, because CLI script の retry script が頻繁に fail-fast に倒れる

## More Information

### Reassessment Triggers

| Trigger                                                            | アクション                            |
| ------------------------------------------------------------------ | ------------------------------------- |
| GitHub API が per_page max を緩和                                  | Rule 2 cap 値を更新                   |
| GitHub API が 403 で常に rate-limit header を保証                  | Rule 1 fallback を Forbidden に変更検討 |
| filter_tree_entries の filename-only behavior を期待する caller 多発 | Rule 3 に option パラメータ追加検討   |

### 参照

* `docs/audit/2026-05-13-undocumented-decisions-part2.md` (本 ADR の根拠 audit、Candidate #14, #15, #16)
* `src/github.rs:153-165` (403 header fallback の現実装)
* `src/github.rs:239,252,265` (per_page silent clamp の現実装)
* `src/github/helpers.rs:161-177` (filter_tree_entries glob scope の現実装)
* GitHub Rate Limit docs: https://docs.github.com/en/rest/rate-limit
