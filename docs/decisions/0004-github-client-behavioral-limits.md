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

- CLI 利用者の data integrity (silent truncation を防ぐ)
- rate limit retry の reliability (header 不在で retry を失わない)
- glob semantics の intuitive 一致 (`src/*.rs` で path scope を期待)

## Considered Options

- Option A: ADR で behavioral rule を明文化 + 必要な code 修正
- Option B: 現状維持、code コメントだけ追加
- Option C: GitHub API spec に厳密に従う (header 不在 = 仕様外として error)

## Decision Outcome

Chosen option: Option A, because silent behavior は CLI / agent 利用で debug 困難な class of bug。明文化と code 修正で programmatic 利用性を高める。

### Rule 1: 403 + missing rate-limit header → RateLimited (retry default)

`x-ratelimit-remaining` ヘッダの値で判定:

| Condition                      | Classification                         |
| ------------------------------ | -------------------------------------- |
| header 存在 + `remaining == 0` | RateLimited (retry)                    |
| header 存在 + `remaining > 0`  | Forbidden (auth misconfig, no retry)   |
| header 不在                    | **RateLimited (retry default)** ← 新規 |

GitHub docs は 403 で header を必ず付けるとは保証していないため、unknown は retry に倒す。

### Rule 2: per_page must be 1..=100 (type-enforced, compile-time validated)

`per_page` 値は `PerPage` newtype で encapsulate される (`src/github.rs`):

- `pub const fn PerPage::new(n: u8)` で構築。`assert!` が `n` が 1..=100 範囲外なら panic
- `const` context (例: `OVERVIEW_ITEMS = PerPage::new(5)`) では panic は compile-time に発火
- silent clamp は廃止
- 0 は GitHub API の implementation-defined behavior (空配列 or デフォルト) を避けるため明示拒否
- production caller は全て `const` 評価される literal のみ。runtime 入力経路を新設する際は fallible constructor (`TryFrom<u8>` 等) を別途追加する。

### Rule 3: filter_tree_entries glob is path-scoped

`filter_tree_entries(entries, path, pattern)`:

- `pattern` は `entry.path` (full repo-relative path) に対して match
- filename-only match の旧 behavior は廃止
- `src/*.rs` のような glob が intuitive に動作

### Rule 4: List endpoints surface only the first page (no pagination)

`get_issues` / `get_pulls` / `get_releases` (`src/github.rs:316,328,340`):

- `?page=` を送らず、`Link` header (`rel="next"` / `rel="last"`) を parse しない
- `per_page` (1..=100、Rule 2 で type-enforced) で 1 call = 最大 100 件
- `per_page` を超える結果は silent truncation (Rule 2 の per-page cap とは別 layer、per-call cap)
- `repo_overview` の "at a glance" outcome に最適化 (`scout repo-overview` の usage 例で `PerPage::new(5)` 等の small constant が default)
- "全件" semantics は scope 外。caller が closed issues / 完全 PR 履歴 を必要とする場合は GitHub web UI / `gh` CLI へ誘導

### Rule 5: List endpoints use fixed filter / sort

`get_issues` / `get_pulls` の query string は固定:

- `state=open` (closed / all は不可)
- `sort=updated` (created / comments は不可)
- `direction=desc` (asc は不可)

`get_releases` は filter / sort 引数を持たない (GitHub 側 default = published_at desc)。

`repo_overview` の "latest activity" outcome に対する optimal default。flexibility が必要になった時点で signature 変更 (Reassessment Trigger 参照)。

### Consequences

- Good, because invalid per_page 値が compile-time panic として検出される (silent data truncation 不可能)
- Good, because rate limit retry が header 不在でも確実に発動
- Good, because glob semantics が intuitive、`src/*.rs` 等が動作
- Good, because list endpoint の固定 filter / sort と pagination 不在は `repo_overview` outcome に最適化された intentional limit、`gh` CLI 代替への誘導が明確
- Bad, because behavior 変更で既存 caller (per_page > 100、per_page == 0、filename-only glob 依存) が break する可能性 (semver bump 検討)
- Bad, because header 不在を retry に倒す decision は false-positive retry を増やす (secondary rate limit が稀に出る場合)
- Bad, because Rule 4 で >100 件の真の "全件" を期待する caller が silent truncation を被る (Rule 2 と同じ class、layer が異なる)

### Confirmation

- `src/github.rs:153` 周辺で 403 + header 不在の unit test (mock response)、`RateLimited` に分類されることを確認
- `src/github.rs` の `PerPage::new` boundary test: `per_page=1` / `per_page=100` accept (T-GH011a)、`per_page=0` / `per_page=101` で panic 検証 (T-GH011b/c、`#[should_panic]`)
- `src/github/helpers.rs:161` で `filter_tree_entries(entries, None, Some("src/*.rs"))` が `src/foo.rs` を含むことの test
- `src/github.rs:316,328,340` の list endpoint URL に `?page=` が含まれないこと、固定 query string (`state=open&sort=updated&direction=desc`) が caller の引数で変更されないことの compile-time enforcement (関数 signature が `per_page: PerPage` のみ受け取る)
- `scout repo-overview --help` で list endpoint が "first N items" の semantics であることを README / `--help` で文書化

## Pros and Cons of the Options

### Option A: ADR + code 修正 (採用)

- Good, because silent behavior が明示的 error / typed path に置き換わる
- Good, because CLI / agent の自動判断が信頼できる
- Bad, because behavior 変更が breaking、既存 caller が影響
- Bad, because header 不在 retry は false-positive retry を増やす trade-off

### Option B: コメントだけ追加

- Good, because behavior 変更ゼロ
- Bad, because silent truncation / silent retry 抑止が残る
- Bad, because CLI / agent caller が動作を予測できない

### Option C: GitHub API spec に厳密従う (header 不在 = error)

- Good, because retry false-positive を避ける
- Bad, because GitHub API が header 配信を保証していないため、本来 retryable な状況を error に倒す
- Bad, because CLI script の retry script が頻繁に fail-fast に倒れる

## More Information

### Reassessment Triggers

| Trigger                                                              | アクション                                                                                         |
| -------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| GitHub API が per_page max を緩和                                    | Rule 2 cap 値を更新                                                                                |
| GitHub API が 403 で常に rate-limit header を保証                    | Rule 1 fallback を Forbidden に変更検討                                                            |
| filter_tree_entries の filename-only behavior を期待する caller 多発 | Rule 3 に option パラメータ追加検討                                                                |
| Caller が closed issues / 全件 PR 履歴 / 非 updated-sort を要求      | Rule 4 で pagination (Link header walk) 追加 + Rule 5 で filter / sort knob を別 method として導入 |
| `repo_overview` 用途以外の list call site が増加                     | Rule 4/5 で knob を struct param 化 (現状は `repo_overview` 専用 default として最適化)             |

### 参照

- `docs/audit/2026-05-13-undocumented-decisions-part2.md` (Rule 1-3 の根拠 audit、Candidate #14, #15, #16)
- `docs/audit/2026-05-19-undocumented-decisions.md` (Rule 4-5 の根拠 audit、Candidate GH-04, GH-05)
- `src/github.rs:187-214` (403 header fallback の現実装、Rule 1)
- `src/github.rs:322-336` (`PerPage` struct + `PerPage::new` validation、Rule 2)
- `src/github/helpers.rs:153-184` (filter_tree_entries glob scope の現実装、Rule 3)
- `src/github.rs:281,293,305` (list endpoint の pagination 不在 + 固定 filter/sort、Rule 4-5)
- GitHub Rate Limit docs: https://docs.github.com/en/rest/rate-limit
- GitHub Pagination docs: https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api
