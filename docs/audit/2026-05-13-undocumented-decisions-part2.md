# Undocumented Decisions Audit Part 2: 2026-05-13

`/audit-undocumented` skill の本番運用 (improved skill: 4-way classification + triage step + external ADR detection)。Part 1 で audit 済の `fetch.rs`/`tools.rs` を除く残り 8 大型ファイル (>400 行) を対象。

## Summary

| Metric                       | Value |
| ---------------------------- | ----- |
| Large files scanned          | 8 |
| Decision candidates          | 40 |
| ADR-covered (excluded)       | 0 |
| Net new candidates           | 40 |
| ADR promotion candidates (pre-challenge) | 17 |

## Large File Decisions

### src/slack.rs (970 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- |
| 1 | 274-279 | `prefetch_users` fires `users.info` per-user via `join_all` with no concurrency cap | No | Yes | **H** | medium |
| 2 | 261-265 | Display-name priority: `profile.display_name` > `real_name`, empty-string = absent | No | Partial | M | low |
| 3 | 302-327 | Two-fetch pattern (`history` then `replies` if `reply_count > 0`); race window | No | Yes | M | medium |
| 4 | 456-466 | `extract_target` returns context messages; non-thread callers pass `&[]`, data silently dropped | No | Partial | L | low |
| 5 | 469-509 | YAML frontmatter + Markdown output; `context_messages` optional key undocumented | No | Yes | M | medium |

### src/github/format.rs (860 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- |
| 6 | 6 | `MAX_README_BYTES = 24_000` byte budget for README truncation | No | Yes | **H** | medium |
| 7 | 20-33 | `fence_delimiter` longest-run scan + `max(run, 2) + 1`, min-3 floor unattributed | Partial | Yes | M | low |
| 8 | 156-173 | README truncation: char-boundary-first then newline-snap (ordering invariant) | Partial | Yes | **H** | low |
| 9 | 177 | `filter(`pull_request.is_none()`)` silently drops PRs from issues API mix | No | Yes | M | medium |
| 10 | 253 | `d.get(..10)` slices RFC3339 to date; assumes always `YYYY-MM-DD…` | No | Yes | M | medium |

### src/search/engine.rs (598 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- |
| 11 | 131 | `buffer_unordered(5)` magic concurrency cap (no rationale) | No | Yes | **H** | medium |
| 12 | 22 | `MAX_PAGE_BYTES = 4_500` hard byte cap (no LLM context link) | No | Yes | **H** | low |
| 13 | 87-100 | Partial-success policy: any success OK, all-fail error (boundary unstated) | No | Yes | **H** | medium |
| 14 | 56-58 | `Lang::Auto` fans out bilingual queries; other langs emit one | No | Yes | M | medium |
| 15 | 23 | `FETCH_TIMEOUT = 15s` (no SLA/budget rationale) | No | Yes | M | high |

### src/tools/errors.rs (611 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- |
| 16 | 51 | `internal()` unconditionally maps to `IoError` (exit 74) regardless of cause | No | Yes | M | medium |
| 17 | 225-242 | `SlackError::Api` blanket maps to `user_error` (exit 64); breaks retryability for Slack 5xx | No | Yes | **H** | medium |
| 18 | 266-279 | `unwrap_or_note` silently downgrades `GitHubError` to log+note; no structured signal | No | Yes | **H** | medium |
| 19 | 195-210 | HTTP 408/429 → transient; other non-404 4xx → data_error (rationale absent) | Partial | Yes | M | low |
| 20 | 12-14 | Shared hint constants only for retry/network; policy for when to add hints unstated | No | No | L | high |

### src/github/encoding.rs (470 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- |
| 21 | 93-95 | Null-byte-only binary heuristic rejects valid UTF-16/UTF-32 without BOM | Partial | Yes | **H** | low |
| 22 | 102-114 | Chardetng 8-encoding allowlist (manual list, no `is_single_byte()` family predicate) | Partial | Partial | **H** | medium |
| 23 | 38 | `is_whitespace()` over-broad strip before base64 (GitHub returns only `\n`/`\r`) | No | Yes | M | medium |
| 24 | 130 | `Utf8Detection::Allow` blurs `DetectionSource::Detected("utf-8")` vs `AssumedUtf8` | No | Yes | M | medium |
| 25 | 69 | `decode_without_bom_handling` in explicit path preserves U+FEFF when user passes `--encoding utf-8` | No | Yes | M | high |

### src/github/helpers.rs (485 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- |
| 26 | 10-21 | `PATH_ENCODE_SET` omits `:`; `validate_ref` treats `:` as invalid (asymmetry unexplained) | No | Yes | **H** | medium |
| 27 | 27-33 | `is_valid_github_name` accepts `.hidden` (GitHub rejects); only blocks `.`/`..` as whole | No | Yes | **H** | medium |
| 28 | 56-65 | `validate_ref` partial impl of `git-check-ref-format` (skips `@{`, `//`, leading `/`/`.`) | No | Yes | **H** | low |
| 29 | 161-177 | `filter_tree_entries` matches glob only against filename (not path); `src/*.rs` silently no-match | No | Yes | **H** | medium |
| 30 | 119-125 | `parse_line_range`: bare integer = "first N lines" (not "line N"); overloaded surface | No | Partial | M | high |

### src/github.rs (530 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- |
| 31 | 153-165 | 403 split RateLimited vs Forbidden via `x-ratelimit-remaining`; missing header → Forbidden (bypass retry) | No | Yes | **H** | medium |
| 32 | 79-81 | Auth resolution: `GITHUB_TOKEN > GH_TOKEN > gh CLI` (order rationale unstated) | Partial | Yes | M | medium |
| 33 | 239,252,265 | `per_page.min(100)` silent clamp; caller asks 200 gets 100, no log/error | No | Yes | **H** | low |
| 34 | 29 | `TOKEN_RESOLVE_TIMEOUT = 5s`; slow `gh` startup silently falls back to unauth (60/hr) | No | Yes | M | high |
| 35 | 273-278 | Error body `.chars().take(200)` truncation (200 arbitrary, char vs grapheme) | No | No | L | high |

### src/gemini/client.rs (402 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- |
| 36 | 21 | `REQUEST_TIMEOUT = 20s` hardcoded, no env override, no latency budget cite | No | Yes | **H** | medium |
| 37 | 184-198 | 403 = QuotaExhausted, 429 = RateLimited; IAM permission 403 silently misclassified | No | Yes | **H** | low |
| 38 | 119-136 | Error body `take(200)` snippet truncation (arbitrary, applied silently) | No | Partial | M | high |
| 39 | 165-174 | `is_retriable`: all 5xx retriable (incl 501/505); 3 unnecessary retries on permanent | No | Yes | M | medium |
| 40 | 54-70 | `GEMINI_MODEL` env override accepted with no validation (URL path injection surface) | No | Yes | M | high |

## External ADR Dependencies

External ADR cross-check (improved skill Step 4 addition) で確認:

| # | File:Line | External ADR ref | Recommended action |
| - | --------- | ---------------- | ------------------ |
| - | (Part 1 で既出: `src/envelope.rs:55`, `src/tools/errors.rs` 4 箇所) | ADR-0065 (dotclaude) | Part 1 で ADR-0002 supersede + ref 更新済み |

新規 external ADR ref: なし (Part 1 で発見した範囲のみ)。

## ADR Promotion Candidates (pre-challenge)

initial: 17 件 (impact=H かつ reversibility=low/medium):

| # | Source | Line | Summary |
| - | ------ | ---- | ------- |
| 1 | slack.rs | 274-279 | prefetch_users 並列キャップ無し (Slack Tier-4 rate limit 違反リスク) |
| 2 | github/format.rs | 6 | MAX_README_BYTES = 24_000 byte budget 根拠 |
| 3 | github/format.rs | 156-173 | README truncation ordering invariant (char-boundary + newline) |
| 4 | search/engine.rs | 131 | buffer_unordered(5) concurrency cap (rate limit/memory 関係不明) |
| 5 | search/engine.rs | 22 | MAX_PAGE_BYTES = 4500 byte cap (Gemini context budget 関係不明) |
| 6 | search/engine.rs | 87-100 | partial-success policy (k-of-n boundary unstated) |
| 7 | tools/errors.rs | 225-242 | SlackError::Api 一律 user_error マッピング (5xx の retryability 破壊) |
| 8 | tools/errors.rs | 266-279 | unwrap_or_note silent downgrade (programmatic recovery 不能) |
| 9 | github/encoding.rs | 93-95 | null-byte binary heuristic over-rejects UTF-16/32 BOM 無し |
| 10 | github/encoding.rs | 102-114 | chardetng allowlist governance (8 encoding 手動列挙) |
| 11 | github/helpers.rs | 10-21 | PATH_ENCODE_SET と validate_ref の `:` 非対称 |
| 12 | github/helpers.rs | 27-33 | is_valid_github_name dot policy false-safe |
| 13 | github/helpers.rs | 56-65 | validate_ref partial git-check-ref-format impl |
| 14 | github/helpers.rs | 161-177 | filter_tree_entries glob filename-only (path 不可) |
| 15 | github.rs | 153-165 | 403 RateLimited/Forbidden 分岐の header fallback |
| 16 | github.rs | 239-265 | per_page silent clamp (caller に通知無し) |
| 17 | gemini/client.rs | 184-198 | Gemini 403 quota vs IAM 誤分類 |

## ADR Promotion Candidates (post-challenge)

`critic-design` agent (devil's advocate) で 17 → 5 keep / 3 downgrade / 9 drop:

| # | Source | Verdict | Reason / Action |
| - | ------ | ------- | --------------- |
| 1 | slack.rs:274 prefetch_users | downgrade | Slack Tier-4 (50+/min) で burst risk のみ。inline comment で足る |
| 2 | format.rs:6 MAX_README_BYTES | drop | 内部 Gemini context budget、公開 contract 無し |
| 3 | format.rs:156 truncation ordering | drop | 既存 comment (line 157) で十分 |
| 4 | engine.rs:131 buffer_unordered(5) | downgrade | Tunable、inline comment で rate limit 関係を記述 |
| 5 | engine.rs:22 MAX_PAGE_BYTES | drop | 内部 quality budget、公開 contract 無し |
| 6 | engine.rs:87 partial-success | drop | warn! + if 分岐で legible |
| 7 | errors.rs:225 SlackError::Api → user_error | **keep** | ADR-0002 sysexits contract 違反 (Slack 5xx → exit 64) |
| 8 | errors.rs:266 unwrap_or_note silent downgrade | **keep** | `--json` で partial data を caller が検出不可能 |
| 9 | encoding.rs:93 null-byte heuristic | drop | error message (line 122) が UTF-16 case を documented |
| 10 | encoding.rs:102 chardetng allowlist | drop | `is_reliable_detection` comment 済み |
| 11 | helpers.rs:10 PATH_ENCODE/`:` 非対称 | downgrade | inline comment on `PATH_ENCODE_SET` で十分 |
| 12 | helpers.rs:27 is_valid_github_name dot policy | **drop (FALSE PREMISE)** | **reviewer-rust が誤判定**: GitHub は `.github` 等 dot-prefix repo を許可。code は正しい |
| 13 | helpers.rs:56 validate_ref partial git-check-ref-format | drop | GitHub API が validation を enforce (422 返却) |
| 14 | helpers.rs:161 filter_tree_entries glob filename-only | **keep** | `src/*.rs` silent no-match。CLI public behavior |
| 15 | github.rs:153 403 header fallback | **keep** | `x-ratelimit-remaining` 不在で retry 抑止。API contract |
| 16 | github.rs:239 per_page silent clamp | **keep** | `u8` で 255 OK、GitHub cap 100。silent data truncation |
| 17 | gemini/client.rs:184 Gemini 403 quota vs IAM | **drop (BUG)** | IAM 403 を quota 誤分類。**ADR でなく `classify_api_error` の bug fix** |

### Summary

| Verdict | Count |
| --- | ----- |
| keep | 5 |
| downgrade | 3 |
| drop | 9 |

### 最終 ADR 候補 (5 keep → 2 ADR)

| ADR | 統合元 | テーマ |
| --- | ------ | ------ |
| 0003 (extend ADR-0002 or new) | #7 SlackError, #8 unwrap_or_note | Error mapping contract (sysexits 拡張) |
| 0004 (new) | #14 glob, #15 403 header, #16 per_page | GitHub API behavioral limits |

### Bug fix (ADR ではない)

- #17 Gemini 403 quota vs IAM 誤分類: `src/gemini/client.rs::classify_api_error` の 403 branch を修正。IAM permission error は `user_error`/`forbidden` に再分類。billing hint は QuotaExhausted 専用に。

### inline comment 強化 (downgrade 3 件)

- #1 slack.rs:274 prefetch_users: Slack Tier-4 rate limit 内 burst risk のみと注記
- #4 engine.rs:131 buffer_unordered(5): fetch concurrency vs Gemini rate limit 関係を注記
- #11 helpers.rs:10 PATH_ENCODE_SET: `:` 非対称は URL path vs git ref の rule 差と注記

## Skill Design Feedback (本番運用で発見)

### reviewer-rust 精度問題

1. **外部仕様の検証不足**: #12 で GitHub の dot policy を仮定で判定 (false premise)。reviewer prompt に「外部 API 仕様の主張は引用源を明示。引用源なしの仕様違反は flag しない」を追加すべき
2. **既存 documentation の見逃し**: drop 9 件中複数 (#3, #6, #9, #10) で既存 comment / error message を reviewer が見落とし。reviewer prompt に「documented? 判定前にファイル全体を scan して existing comment/error/test を確認」を追加すべき

### ADR vs Bug distinction

3. #17 が「bug fix を ADR 化」しそうになった。skill Step 6.2 に「Is this candidate a fix-the-bug case, or an invariant-to-document case?」判定軸を追加すべき。bug は drop、ADR 化は invariant のみ

