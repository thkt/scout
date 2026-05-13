# ADR Drift Scan (Post-Implementation): 2026-05-13

`/audit-adr-drift` 2nd dogfooding。`feat/adr-implementation` branch で ADR-0003 (partial) + ADR-0004 (full) implementation 完了後の状態を verify。

## Summary

| Metric             | Value |
| ------------------ | ----- |
| ADRs scanned       | 4     |
| Drift findings     | 2     |
| H priority         | 0     |
| M priority         | 1     |
| L priority         | 1     |
| Unverifiable ADRs  | 0     |
| External ADR refs  | 1 (ADR-0065) |

## Per-ADR Findings

### ADR 0001: SSRF Defense Architecture and fetch.rs Module Structure

Status: accepted

**No drift detected.** ADR Decision の全シンボルが現コードと整合:

| 言及シンボル | 検出箇所 | 整合 |
| --- | --- | --- |
| `fetch_http` / `http` dual client | `src/tools.rs:128-137, 233, 295` | OK |
| `redirect::Policy::none()` | `src/fetch.rs` | OK (PR #86 で doc comment 拡充済み) |
| `check_browser_request` (CDP) | `src/fetch.rs:381` | OK (PR #86 で scheme rationale 追加済み) |
| `#[cfg(feature = "js-rendering")]` | 13 箇所 | OK (split trigger 未到達) |

#### Reassessment Trigger 現状

| Trigger | 現状 | 状態 |
| --- | --- | --- |
| `fetch.rs > 2000 行` | 1456 | OK |
| 新規 command 計 9 以上 | 6 | OK |
| SSRF contract 違反 incident | 未発生 | OK |

### ADR 0002: Adopt sysexits.h Exit Code Convention for CLI

Status: accepted

**No drift detected.** 前回 part-1 audit で発見した ADR-0065 → ADR-0002 ref drift は merge 済み PR で 5 箇所 fix 完了:

- `src/envelope.rs:55-57`: ADR-0002 (exit-code) + ADR-0065 (JSON tag) の責務分離 doc comment
- `src/tools/errors.rs:91, 97, 103, 112, 123, 128, 487` 等: `per ADR-0002` で統一

T-H000 enforcement test 健在 (lib.rs:220-254)。

### ADR 0003: Error Classification Contract for sysexits and JSON Output

Status: accepted

**Partial implementation (scope-aware), 1 documentation drift.**

#### 実装状況 (4 sub-decision)

| Sub-decision | 実装状況 | コメント |
| --- | --- | --- |
| HTTP status → ErrorCode mapping table | ✅ Mostly implemented | GeminiError/GitHubError は既存実装で table 適合、SlackError は本 PR で追加 (T-ER020/021/022) |
| `degraded: bool` field on result struct | ✅ Already existed | `src/envelope.rs:17` に既存 (ADR 起票前から!) |
| `DegradedReason` typed enum | ❌ Not implemented | ADR 内で「本 ADR 後の別 PR で実装」と明記済み (scope-aware) |
| silent log-only fallback 廃止 (`unwrap_or_note`) | ❌ Not implemented | 同上、ADR で明記済み |

#### Findings

| # | File:Line | Description | Direction | Priority |
| - | --------- | ----------- | --------- | -------- |
| 1 | `docs/decisions/0003-...md` | ADR Decision で「`degraded: bool` field 追加」と書いたが、実は `src/envelope.rs:17` に **ADR 起票前から既存**。ADR は post-hoc documentation だった | `adr-update` (Note 追記) | L |

ADR-0003 Note 追記推奨:
> `degraded: bool` field on result structs was already implemented in `src/envelope.rs` at the time of this ADR's authoring. This ADR formalizes the existing behavior as the canonical contract rather than introducing a new field. The remaining work (DegradedReason enum + unwrap_or_note refactor) is captured in a separate follow-up issue.

### ADR 0004: GitHub Client Behavioral Limits

Status: accepted

**Full implementation, no drift.** 3 Rule すべて feat/adr-implementation branch で実装:

| Rule | 実装箇所 | Test |
| --- | --- | --- |
| 1. 403 + missing header → RateLimited | `src/github.rs:157-181` | T-GH010 |
| 2. per_page > 100 → InvalidPerPage | `src/github.rs:255, 268, 281` + `validate_per_page`:294 | T-GH011 |
| 3. filter_tree_entries glob path-scope | `src/github/helpers.rs:171-177` | T-GHH023 |

すべての test pass (98 → 98 + 3 = 101 tests)。

## External ADR Dependencies

improved skill の external ADR cross-check で検出:

| # | File:Line | External ADR ref | Status |
| - | --------- | ---------------- | ------ |
| 1 | `src/lib.rs:109, 121` | ADR-0065 (JSON envelope) | Expected per ADR-0002 supersede note (JSON schema portion remains in ADR-0065) |
| 2 | `src/envelope.rs:1, 43, 56-57, 69, 77, 84, 155, 170, 241` | ADR-0065 (JSON envelope structure) | Expected per ADR-0002 supersede note |

**Action**: 現状維持 OK。ADR-0002 supersede note で明示済み:
> The JSON output schema portion of ADR-0065 (`error.code` field, `--json` mode, error envelope structure) is not included in this ADR and remains active in ADR-0065 until a scout-local ADR is promoted to capture it.

将来 scout-local ADR (ADR-0005?) を起票して JSON schema 部分を移植する場合、これらの ref を一括更新する。

## Skill Validation (3rd dogfood)

`/audit-adr-drift` の improved skill 機能を本 audit で verification:

| 機能 | 結果 |
| --- | --- |
| External ADR cross-check (Step 4 拡張) | ✅ ADR-0065 ref を正しく検出 |
| `verification: pending_spec_check` (reviewer-rust 改善) | N/A (本 audit は ugrep 中心) |
| `Bug vs Invariant` gate (Step 6.2) | N/A (challenge step 省略、軽量試運転) |
| Partial implementation 検出 | ✅ ADR-0003 partial を「scope-aware drift」として識別 |

## Follow-up Issue Candidates

### M priority

- [ ] ADR-0003 に Note 追加: 「`degraded: bool` field は ADR 起票前から既存。ADR は post-hoc documentation」(`docs/decisions/0003-*.md` More Information section)

### L priority (低優先度)

- なし

### 別 PR / follow-up issue 候補 (ADR-0003 scope-aware drift から派生)

- [ ] **DegradedReason enum 導入 + unwrap_or_note → unwrap_or_degraded refactor**: ADR-0003 残り scope、複数 call sites + JSON schema 変更で semver bump 検討
- [ ] scout-local ADR-0005 起票検討: ADR-0065 JSON envelope schema portion を scout に移植 (envelope.rs / lib.rs / tools/errors.rs の `per ADR-0065` ref を `per ADR-0005` に更新)
