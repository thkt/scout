# Undocumented Decisions Audit: 2026-05-13

`/audit-undocumented` skill の試運転を兼ねた audit (scope: top 2 大型ファイル + 主要ドキュメント)。残り 7 大型ファイル (>400 行) は未スキャン。

## Summary

| Metric                       | Value |
| ---------------------------- | ----- |
| Large files scanned          | 2 of 9 (試運転スコープ) |
| Documents scanned            | 4 (README.md, README.ja.md, Cargo.toml lints, deny.toml) |
| Decision candidates          | 31 |
| ADR-covered (excluded)       | 0 (`docs/decisions/` empty) |
| Net new candidates           | 31 |
| ADR promotion candidates     | 12 |

### Scope

- Scanned files: `src/tools.rs` (1410 lines), `src/fetch.rs` (1456 lines)
- Unscanned >400 line files: `src/slack.rs` (970), `src/github/format.rs` (860), `src/tools/errors.rs` (611), `src/search/engine.rs` (598), `src/github.rs` (530), `src/github/helpers.rs` (485), `src/github/encoding.rs` (470), `src/gemini/client.rs` (402)

## Large File Decisions

### src/tools.rs (1410 lines)

| # | Line | Decision | Documented? | Impact | Reversibility |
| - | ---- | -------- | ----------- | ------ | ------------- |
| 1 | 1-1410 | All 6 command handlers + tests in single file (`tools/` subdir holds only support modules) | No | M | high |
| 2 | 128-137 | Dual HTTP client: `http` (auto-redirect, 5 hops) for API + `fetch_http` (`Policy::none()`) for SSRF-safe user URLs | Partial (field comment, no contract for future contributors) | **H** | **low** |
| 3 | 136, 163-168 | GitHub client lazy init via `OnceCell` to avoid `gh auth token` cost on non-GitHub commands | Partial (motivation in comment, cost unquantified) | M | high |
| 4 | 463-470 | `repo_overview` sequential `get_repo` gate before parallel fan-out (issue #18 referenced inline) | Partial (issue ref, tradeoff unrecorded) | M | medium |
| 5 | 119-126 | `FETCH_TOOL_TIMEOUT = 95s` derived as `HTTP_TIMEOUT (30s) + CDP_TIMEOUT (60s) + 5s`; `SLACK_TOOL_TIMEOUT = 60s` for "3 API + N user resolutions" | Partial (formula stated, cross-refs absent) | L | high |
| 6 | 56-104 | `StdinResolver` with `stdin_consumed: bool` to distinguish "stdin empty" vs "already consumed by prior arg" | Yes (struct comment + tests) | M | high |
| 7 | 493-496 | GitHub `/issues` returns PRs too; client-side filter applied to JSON output only (not markdown) | Partial (JSON rationale only) | L | high |
| 8 | 516-567 | `collect_path_candidates`: 5s hard timeout + silent empty fallback on any failure | Yes (comment + constants) | L | high |
| 9 | 214, 249, 305 | `serde_json::to_value(...).expect("X is Serialize")` for type-level claims | Partial (message names invariant, failure mode absent) | L | high |
| 10 | 200, 611 | Heading shift +2 applied uniformly to all markdown output ("consistent with fetch standalone") | Partial ("consistent" stated, "+2 because h2 host context" unrecorded) | M | high |

### src/fetch.rs (1456 lines)

| # | Line | Decision | Documented? | Impact | Reversibility |
| - | ---- | -------- | ----------- | ------ | ------------- |
| D-01 | 528-612 | Manual redirect loop with per-hop SSRF check instead of reqwest's `Policy::limited` | Partial (contract stated, rationale absent) | **H** | **low** |
| D-02 | 100-102 | TOCTOU gap (DNS check vs reqwest connect) accepted as CLI-only risk | Yes | H | medium |
| D-03 | 85, 210 | Two byte thresholds: `EXTRACT_TEXT_THRESHOLD = 50`, `BODY_TEXT_THRESHOLD = 100` for JS-render fallback heuristic | Partial (50 has comment, 100 has none, asymmetry unexplained) | M | high |
| D-04 | 186-189 | `used_raw_fallback` forces thin-extract regardless of byte count | Yes (doc comment + tests) | M | high |
| D-05 | 466-488, 372-390 | CDP `Fetch.RequestPaused` interceptor covers browser subrequests (not just top-level URL); blocks `ws://`/`wss://`/`data:`/`about:`/`chrome:`/`blob:` | Partial (scheme list documented, "why interceptor not pre-check" absent) | **H** | **low** |
| D-06 | 328-353 | `OnceLock` cache for browser binary path, process-lifetime | No | L | high |
| D-07 | 511-513 | `which <cmd>` subprocess instead of `$PATH` split (portability over speed) | No | L | high |
| D-08 | 15-20, 105-178, 304-505 | `js-rendering` feature interleaved with plain-HTTP path via `#[cfg(...)]` throughout single file (vs. separate `fetch/browser.rs` module) | No | **H** | **medium** |
| D-09 | 110-111 | Post-redirect SSRF re-check as defense-in-depth for manual-loop bugs | Yes | H | low |
| D-10 | 226-301 | Hand-rolled byte-walker HTML scanner for thin-body detection (skips `<script>`/`<style>`; ignores comments/CDATA/malformed) | No | M | medium |
| D-11 | 249, 276-279 | `tag_buf = [u8; 16]`: silent truncation for tag names >16 bytes | No | L | high |
| D-12 | Cargo.toml:28, fetch.rs:629-643 | `chardetng` declared as dependency but not used in `decode_body`; falls back to UTF-8 when no `charset=` header | No | M | high |
| D-13 | 571-574 | Missing `Content-Type` header treated as permissive ("proceed as text"), not rejected | Partial (debug log only) | M | high |
| D-14 | 1-1456 | Six concerns (SSRF, redirect, charset, heuristic, CDP, 51 tests) in single 1456-line module | No | **H** | **medium** |

## Prose Document Decisions

### README.md

| # | Line | Decision Verb | Decision | ADR Coverage |
| - | ---- | ------------- | -------- | ------------ |
| P-01 | 155 | always | YAML frontmatter block is always present (individual fields conditional) | None |

### README.ja.md

| # | Line | Decision Verb | Decision | ADR Coverage |
| - | ---- | ------------- | -------- | ------------ |
| P-02 | 232 | 規約 | exit codes follow `sysexits.h` convention | None |

### Cargo.toml `[lints.*]`

| # | Line | Decision | ADR Coverage |
| - | ---- | -------- | ------------ |
| P-03 | 38-57 | `[workspace.lints.clippy]` 多数 deny (absolute_paths, wildcard_imports, str_to_string, cast_possible_truncation, needless_pass_by_value など) | None |
| P-04 | 39 | `unsafe_code = "forbid"` (rurico の `deny` より厳しい) | None |

### deny.toml

| # | Line | Decision | ADR Coverage |
| - | ---- | -------- | ------------ |
| P-05 | 7-21 | License allow list 12 種 (MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2/3-Clause, ISC, 0BSD, Zlib, Unicode-3.0, CDLA-Permissive-2.0, BSL-1.0, MPL-2.0, Unlicense) | None |
| P-06 | 23-25 | `multiple-versions = "warn"`, `wildcards = "allow"` | None |
| P-07 | 27-30 | `unknown-registry = "deny"`, `unknown-git = "deny"`, `allow-registry = ["crates.io"]` (supply chain trust boundary) | None |

## ADR Promotion Candidates (post-challenge)

初期 promotion 12 件を `critic-design` agent (devil's advocate) で challenge した結果:

| # | Source | Candidate | Initial | Challenge | Final | Action |
| - | ------ | --------- | ------- | --------- | ----- | ------ |
| 1 | fetch.rs D-01 | Manual redirect loop | promote | **downgrade** | inline-comment | `fetch.rs:528` の doc comment に "reqwest `Policy::limited` は redirect 時 SSRF 検査を skip するため不可" を 1 行追記 |
| 2 | fetch.rs D-05 | CDP subrequest interceptor | promote | **downgrade** | inline-comment | `check_browser_request` 関数直上に scheme blocklist (ws/wss/data/about/chrome/blob) の理由コメント 3 行追加 |
| 3 | tools.rs #2 | Dual HTTP client SSRF contract | promote | **keep** | ADR | 型未強制 invariant。SSRF 統合 ADR の核 (with #7) |
| 4 | fetch.rs D-09 | Post-redirect SSRF re-check | promote | **drop** | skip | L110 に "Defense-in-depth: catch bugs in the manual redirect loop" 既に記述済み |
| 5 | fetch.rs D-14 | 1456-line monolith | promote | **downgrade** | GitHub Issue | "fetch.rs split plan" Issue として download/heuristic/browser の 3 モジュール案を記録 |
| 6 | tools.rs #1 | 1410-line monolith | promote | **drop** | skip | tools/ サブディレクトリに partial split 進行中、ADR 化は未完状態の正当化になる |
| 7 | fetch.rs D-08 | js-rendering feature 混在 | promote | **keep** | ADR | `allow(dead_code)` 構造 smell。SSRF 統合 ADR と統合 (with #3) で "fetch.rs 構造決定" 1 本に |
| 8 | README.ja.md P-02 | `sysexits.h` 規約準拠 | promote | **keep** | ADR | exit code 公開 API。lib.rs L220-252 の enforcement test の根拠を ADR で裏付け |
| 9 | Cargo.toml P-04 | `unsafe_code = "forbid"` | promote | **drop** | skip | 1 行で自己説明的、git commit message が判断記録になる |
| 10 | deny.toml P-05 | License allow list 12 種 | promote | **drop** | comment | deny.toml の `[licenses]` block 先頭に "OSI permissive + MPL copyleft-weak" 等 2 行追加 |
| 11 | deny.toml P-07 | Supply chain (`crates.io` 限定) | promote | **drop** | skip | `[sources]` block が完全な仕様で自己説明的 |
| 12 | tools.rs #4 | `repo_overview` sequential gate | promote | **downgrade** | inline-comment | L463 を "Verify repo exists first: a 404 here avoids 4 wasted parallel API calls (issue #18)" に拡充 |

### Summary

| Verdict | Count |
| ------- | ----- |
| keep | 3 |
| downgrade | 4 |
| drop | 5 |

### 最終 ADR 候補 (2 本)

| # | ADR タイトル案 | 統合元 | 規模 |
| - | -------------- | ------ | ---- |
| 1 | `0001-ssrf-defense-architecture-and-fetch-module-structure.md` | tools.rs #2 (dual client) + fetch.rs D-08 (js-rendering feature 配置) | 中規模 (SSRF 型未強制 + module 構造判断を統合) |
| 2 | `0002-sysexits-exit-code-convention.md` | README.ja.md P-02 | 小規模 (exit code policy + enforcement test 根拠) |

### Critic-design からの insight

> drop の主因は一貫して「enforce が already mechanical (cargo-deny / clippy / テスト) で動いており、ADR が行動を変えない」こと。個人 OSS で ADR が価値を持つのは **(1) 型や lint で守れない不変条件** と **(2) 公開 API の互換性コミットメント** の 2 領域に限られる。

## Follow-up

### ADR 起票 (post-challenge 2 件)

- [ ] `docs/decisions/0001-ssrf-defense-architecture-and-fetch-module-structure.md` (tools.rs #2 dual client + fetch.rs D-08 js-rendering 構造を統合)
- [ ] `docs/decisions/0002-sysexits-exit-code-convention.md` (P-02)

### Inline コメント強化 (downgrade 4 件)

- [ ] `fetch.rs:528` `download` doc comment に reqwest `Policy::limited` 不可理由を追記
- [ ] `fetch.rs:check_browser_request` 関数直上に scheme blocklist の理由コメント追加
- ~~`fetch.rs` 全体: GitHub Issue "fetch.rs module split plan" を起票~~ → ADR-0001 Reassessment Triggers に統合済み (trigger 達成前の Issue 起票は noise になるため drop)
- [ ] `tools.rs:463` `repo_overview` のコメントを "a 404 here avoids 4 wasted parallel API calls (issue #18)" に拡充

### Config コメント追加 (drop だがガイダンス追加 1 件)

- [ ] `deny.toml:[licenses]` block 先頭にポリシーコメント (例: "OSI permissive + MPL copyleft-weak") 2 行追加

### 残り audit

- [ ] 残り 8 大型ファイル (slack.rs, github/format.rs, tools/errors.rs, search/engine.rs, github.rs, github/helpers.rs, github/encoding.rs, gemini/client.rs) を audit-undocumented で追加スキャン

## Skill Design Feedback

`/audit-undocumented` 試運転で agent が指摘した改善案:

1. 4-way classification 提案: "inline-documented, incomplete" を 3-way (Yes/Partial/No) と独立に加えると、Finding 2 系 (現状コメントあるが contract が未記述) を 1 級カテゴリとして扱える
2. 大型ファイル triage step: 200 行スキャン時点で finding 数推定を出し、scope 絞り提案する option を skill に追加
3. Permission syntax: `Bash(cargo clippy:*)` の空白 matcher は機能せん (試運転で `NEEDS_USER_APPROVAL`)。`Bash(cargo:*)` または subcommand 毎に分割推奨
