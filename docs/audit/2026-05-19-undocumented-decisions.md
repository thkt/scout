# Undocumented Decisions Audit: 2026-05-19

`/audit-undocumented` 4th run. 過去 audit (`2026-05-13-undocumented-decisions.md` Part 1+2, `2026-05-13-adr-drift.md`, `2026-05-14-adr-drift.md`) でカバー済みの decision を除外し、**新規大型ファイル**と**大幅増加ファイル**に scope を絞った。Part 1/Part 2 で抽出済 + ADR-0001〜0009 に統合済の decision は重複検出を避けた。

## Summary

| Metric                                   | Value |
| ---------------------------------------- | ----- |
| Large files (>400 lines) detected        | 12    |
| Large files scanned (post-triage)        | 5     |
| Large files skipped (Part 1/2 covered)   | 7 (tools.rs, fetch.rs, slack.rs, github/format.rs, github/encoding.rs, github/helpers.rs, search/engine.rs) |
| Documents scanned                        | 4 (README.md, README.ja.md, Cargo.toml, deny.toml) |
| Decision candidates (post-exclusion)     | 32    |
| ADR-covered (excluded by reviewer in-flight) | reviewer-rust prompt で ADR-0003/0004/0005/0006/0007/0008/0009 範囲を事前除外指示 (件数は reviewer 内で吸収、未集計) |
| ADR promotion candidates (initial)       | 4 (A, B, C, D)  |
| Post-challenge: keep                     | 1 (A → new ADR-0010) |
| Post-challenge: downgrade                | 1 (C → ADR-0004 extension) |
| Post-challenge: drop                     | 2 (B, D → inline comment) |
| ADR drift findings                       | 1 (ADR-0003 8→9 variants) |
| Bug fix follow-ups                       | 3 (`TooLarge` hint, `TooManyRedirects` classification, 401 fallthrough) |

### Scope decision

- 7 files skipped per user triage (Part 1/2 differential too small to warrant re-scan):
  - `tools.rs` (1898, +488): Part 1 で 10 candidates 抽出済、ADR-0001 で構造判断 covered
  - `fetch.rs` (1807, +351): Part 1 で 14 candidates 抽出済、SSRF/CDP/js-rendering は ADR-0001
  - `slack.rs` (1063, +93), `github/format.rs` (860, ±0): Part 2 covered
  - `github/encoding.rs` (470), `github/helpers.rs` (506), `search/engine.rs` (450): Part 2 covered

- 5 files scanned (focus = post-Part2 PR 追加部分 + 新規大型):
  - **envelope.rs** (531, new): JSON envelope = ADR-0065 territory with no scout-local ADR yet
  - **tools/errors.rs** (890, +279): PR #94 で ErrorCode 5→9 variant 拡張後
  - **retry.rs** (484, new): ADR-0006/0008 partial
  - **brave/client.rs** (566, new): ADR-0005/0007 partial
  - **github.rs** (766, +236): ADR-0004 covers 3 Rules

## Large File Decisions

### src/envelope.rs (531 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility | Existing ADR? |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- | -------------- |
| E-01 | 51-77 | `Degradation` を `(Vec<String>, Vec<DegradedReason>)` 2-Vec 1:1 pairing で表現 (tuple Vec/HashMap 不採用) | Partial | Yes | M | medium | ADR-0003 (DegradedReason 範囲) |
| E-02 | 86-123 | `CommandOutput::degraded` を `bool` field として stored、derived ではない | Partial | Yes | M | medium | None |
| **E-03** | **200-205** | **`ErrorCode::is_retryable() = matches!(self, TempFailure \| Timeout)` policy。`Internal` (exit 70) と `Unknown` (exit 104) を non-retryable 固定** | **Partial** | **Yes** | **H** | **low** | **partial ADR-0003** |
| **E-04** | **226-236** | **`ErrorPayload` field omit mix: `code`/`message`/`retryable` は常時、`next_step`/`candidates` は `skip_serializing_if`** | **No** | **Yes** | **H** | **low** | **ADR-0065 (schema 範囲のみ)** |
| **E-05** | **210-217** | **`SuccessEnvelope` asymmetry: `notes` は常に `[]` 出力、`degraded_reasons` は `skip_serializing_if`** | **Partial** | **Yes** | **H** | **low** | **ADR-0065 (notes)、ADR-0003 (degraded_reasons additive) — 統一 rule なし** |
| E-06 | 32-44 | `DegradedReason::label()` で 5 variants を `"resource"` collapse、4 variants は固有 label。doc では "the three `*FetchFailed` variants" と書くが実際は 4 | Partial | Yes | M | medium | ADR-0003 partial (8 variants と書くが実装 9 variants — **drift**) |
| E-07 | 142-158 | `#[cfg(test)]` test-only accessors (production は consume API のみ) | Yes | No | L | high | None (documented で十分) |
| E-08 | 13-14, 168-169 | `serde(rename_all = "SCREAMING_SNAKE_CASE")` を 2 enum で重複 | No | Yes | L | high | ADR-0065 (case convention) |

### src/tools/errors.rs (890 lines, +279 since Part 2)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility | Existing ADR? |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- | -------------- |
| TE-01 | 299 | Slack `Api` transient code allowlist 3 strings (`internal_error`/`service_unavailable`/`fatal_error`) — closed enumeration、spec citation 無し (`pending_spec_check`) | Partial | Yes | M | high | partial ADR-0003 |
| TE-02 | 303 | `SlackError::Api` 未認識 string を user_error (exit 64) に default。HTTP-based `Unknown(104)` escape (line 222, 345) と asymmetric | Partial | Yes | M | high | partial ADR-0003 |
| TE-03 | 251 | `FetchError::Status(401\|403)` collapsed arm + single hint。GitHub の `Forbidden` (scope hint) と `Api{401}` (auth hint) split (line 178-182) と asymmetric | No | No | L | high | None |
| TE-04 | 267 | `FetchError::DnsResolution` が bespoke inline hint で ADR-0006 transient helpers (`transient_with_network_hint` 等) を bypass | No | No | L | high | ADR-0006 (helper set 定義) |
| TE-05 | 245 | `FetchError::TooLarge` hint が "10MB" hardcoded、`MAX_RESPONSE_BYTES = 10_000_000` 定数と乖離 (single source of truth 違反) | No | Yes | M | high | None |
| TE-06 | 246 | `FetchError::TooManyRedirects → DataError` (exit 65, 非retry)。redirect loop は server-side 状況、TempFailure 検討余地 (`pending_calibration`) | No | Yes | M | medium | None |

### src/retry.rs (484 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility | Existing ADR? |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- | -------------- |
| RT-01 | 14 | `INITIAL_BACKOFF_MS = 1000` (500/2000 でなく 1000 の根拠 無し) | No | Yes | M | high | None |
| **RT-02** | **15** | **`MAX_RETRY_AFTER_SECS = 300` (5min) cap、RFC 9110 §10.2.4 と GitHub/Brave/Slack docs は client-side cap 値を規定しない** | **Partial** | **Yes** | **H** | **high** | **partial ADR-0006** |
| RT-03 | 109 | `parse_retry_after` 過去日時を `saturating_sub` で 0 clamp → "retry now"。clock skew 下で tight loop 化リスク | Partial | Yes | M | medium | None (ADR-0008 は Clock injection seam のみ) |
| RT-04 | 22-26 | Equal jitter (`half + rand[0, half)`) vs Full jitter (`rand[0, base)` AWS 推奨)。選択根拠 無し | Partial | Yes | L | high | None |
| RT-05 | 22, 26 | `jittered_backoff` に base 上限 cap 無し。`RETRIES_CAP = 10` (`tools/config.rs:23`) cross-module invariant が implicit | No | Yes | M | high | None |
| RT-06 | 111, 116 | unparseable `Retry-After` を `warn!` (default visible) level で log。policy 選択 (debug/error と比較) は不文 | No | No | L | high | None |

### src/brave/client.rs (566 lines)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility | Existing ADR? |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- | -------------- |
| BC-01 | 19 | `REQUEST_TIMEOUT = 20s` per-request budget。retry × 20s wall clock 累積関係 unstated | No | Yes | M | high | None |
| BC-02 | 20, 228-230 | `BODY_SNIPPET_BYTES = 200` error body truncation。multi-byte 境界 silent cut | No | Yes | L | high | None |
| **BC-03** | **179-189** | **`build_url` が `q` と optional `search_lang` のみ送信、Brave API の `count`/`offset`/`safesearch`/`freshness`/`country` 等は全 omit (Brave defaults 受け入れ)** | **No** | **Yes** | **H** | **medium** | **None (`pending_spec_check`)** |
| BC-04 | 256-263 | `is_retriable` whitelist が `RateLimited`/`Server`/transient `Network` のみ。408 (Request Timeout), 425 (Too Early) を terminal 扱い | Partial | Yes | M | high | ADR-0006 (mechanism のみ) |
| BC-05 | 198-201 | HTTP 401 と 403 を `BraveError::Unauthorized` に collapsed。RFC 7235 上は 401=credentials missing/invalid, 403=credentials understood but refused | Partial | Yes | M | medium | None (`pending_spec_check`) |
| BC-06 | 74-80 | `pub(crate) trait SearchClient` の contract が未記述 (result ordering, max length, URL normalization, dedup, lang filter semantics) | No | Yes | M | low | ADR-0005 (Brave backend 範囲のみ) |

### src/github.rs (766 lines, +236 since Part 2)

| # | Line | Decision | Documented? | Incomplete-contract? | Impact | Reversibility | Existing ADR? |
| - | ---- | -------- | ----------- | -------------------- | ------ | ------------- | -------------- |
| GH-01 | 82-84 + `token_source.rs:51` | Auth chain order rationale unstated (なぜ `GITHUB_TOKEN > GH_TOKEN > gh CLI`) | Partial | Yes | M | medium | None |
| GH-02 | 205-262 | 401 が dedicated arm 無しで `Api{401}` に fall through、`Unauthorized` variant + auth-rotation hint 不在 | No | Yes | M | low | None |
| GH-03 | 400-406 | `secs_until_ratelimit_reset` が `Some(0)` (即 retry) を返す stale 場合の semantics。`None` (jitter fallback) ではない | Partial | Yes | M | high | partial ADR-0008 (Clock injection seam のみ) |
| **GH-04** | **316-350** | **`get_issues`/`get_pulls`/`get_releases` が `?page=` 送らず `Link` header も parse しない。100件で silent truncation** | **No** | **Yes** | **H** | **medium** | **None (ADR-0004 Rule 2 は per_page cap、これは per-call cap)** |
| **GH-05** | **323, 335** | **List endpoint で `state=open&sort=updated&direction=desc` 固定、closed/created/asc 不可** | **No** | **Partial** | **M** | **medium** | **None** |
| GH-06 | 379-387 | `extract_error_message` 200-char truncation (no ellipsis、`.chars()` cut grapheme cluster) | No | No | L | high | None |

## Prose Document Decisions

### README.md (新規行)

| # | Line | Decision Verb | Decision | ADR Coverage |
| - | ---- | ------------- | -------- | ------------ |
| P-A | 158 | always / never | `data = {query, sources, fetched_pages, failed_urls}`、`All array fields are []` (never null) when empty | partial ADR-0065 (schema 範囲) |
| P-B | 173 | always | Page metadata YAML frontmatter block is `always present`; individual fields conditional | None (ADR-0001 範囲外、fetch markdown contract) |
| P-C | 228 | (statement) | "SSRF defense is designed for local CLI use" + embedding service には additional measures 必要 | ADR-0001 (threat model statement に該当) |
| P-D | 314 | never | `data.fetched_pages` / `data.failed_urls` (research only)、both default to `[]` (never null) when empty | partial ADR-0065 (P-A と同様) |

### README.ja.md / Cargo.toml / deny.toml

新規 decision 無し。Part 1 で抽出済 (P-02 sysexits、P-03/04 lints、P-05/06/07 deny) + deny.toml の license policy comment は Part 1 follow-up で追加済み (line 7-8)。

## External ADR Dependencies

新規 external ADR ref: なし (ADR-0065 は既知の deliberately active carve out)。

ADR-0065 への code refs (`src/envelope.rs`, `src/lib.rs`, `src/tools/errors.rs`, `tests/cli_integration.rs`) は引き続き ADR-0002 §"More Information" の supersede note の対象。**本 audit の最大の発見**として、scout-local ADR-0010 (envelope contract) を起票することで ADR-0065 の JSON schema portion を scout に移植する道筋ができる。

## ADR Promotion Candidates (post-challenge)

`critic-design` agent が 4 initial promotion candidates を Part 1+2 で得た heuristic (「個人 OSS の ADR は (1) 型/lint で守れない不変条件 (2) 公開 API 互換性コミットメント のみ価値」) で挑戦:

| # | Source | Candidate | Initial | Challenge | Final | Action |
| - | ------ | --------- | ------- | --------- | ----- | ------ |
| **A** | envelope.rs E-03 + E-04 + E-05 + README P-A/P-D | scout-local JSON envelope contract (is_retryable mapping + omit policy + `[]`-never-`null`) | promote | **keep** | **ADR-0010** | 新規 ADR 起票、ADR-0065 JSON schema portion を supersede。ADR-0002 §"More Information" の伏線 "until a scout-local ADR is promoted" を成就 |
| B | brave/client.rs BC-03 | Brave search request defaults (count/safesearch/freshness omit) | promote | **drop** | inline-comment | `build_url` body 上に `// Intentionally omit count/safesearch/freshness/country/ui_lang — accept Brave defaults.` 1 行追加 |
| **C** | github.rs GH-04 + GH-05 | GitHub list endpoint silent 100-cap + 固定 filter/sort | promote | **downgrade** | **ADR-0004 extension** | ADR-0004 に Rule 4 (pagination 不在 / silent 100-cap) + Rule 5 (固定 `state=open&sort=updated&direction=desc`) 追加 |
| D | retry.rs RT-02 | `MAX_RETRY_AFTER_SECS = 300` cap 値 | promote | **drop** | inline-comment | `retry.rs:15` 上に `// 5-min cap matches interactive CLI patience; tuning via SCOUT_MAX_RETRY_AFTER_SECS is future work.` 追加 |

### Summary

| Verdict | Count |
| ------- | ----- |
| keep | 1 (A → ADR-0010 新規) |
| downgrade | 1 (C → ADR-0004 拡張) |
| drop | 2 (B, D → inline comment) |

### 最終 ADR 候補

| # | ADR タイトル案 | 統合元 | 規模 |
| - | -------------- | ------ | ---- |
| 1 | `0010-scout-local-json-envelope-contract.md` (新規) | envelope.rs E-03/E-04/E-05 + README P-A/P-D | 中規模 (omit policy + retryable mapping + `[]`-never-`null` 公開契約。ADR-0065 supersede 部分含む) |
| 2 | ADR-0004 amendment | github.rs GH-04 + GH-05 | 小規模 (Rule 4/Rule 5 + Reassessment Trigger row + Confirmation row 追加) |

### Critic-design からの insight

> 個人 OSS で ADR が価値を持つのは **(1) 型/lint で守れない不変条件** と **(2) 公開 API 互換性コミットメント** の 2 領域。本回の 4 candidate を heuristic に当てると、A のみ両方を満たす (omit policy/`[]`-never-`null` は型で守れず、README 公開契約)。C は (2) のみ満たすが、ADR-0004 が既存の正しい家。B (Brave defaults 受け入れ) は contract 不在の decision-by-omission、D (300s cap) は実装 tuning であり interface promise ではない。Impact: H ラベルは ADR-worthy の代理指標として弱く、heuristic 4 が真の filter。

## ADR Drift (情報のみ、別 audit 系統)

| # | Source | Drift | Recommended action |
| - | ------ | ----- | ------------------ |
| D-01 | envelope.rs E-06 + ADR-0003 | ADR-0003 が `DegradedReason` を "8 variants" と書くが実装は **9 variants**。rustdoc も "Only the three `*FetchFailed` variants" と書くが実際 4 variants が固有 label | ADR-0003 line 57 Note を "9 variants (post-implementation)" に更新 + envelope.rs:36 doc を "the four variants that flow through that helper" に修正 |

## Bug fix follow-ups (情報のみ、ADR ではない)

| # | Source | Bug | Recommended fix |
| - | ------ | --- | --------------- |
| BG-01 | tools/errors.rs TE-05 | `FetchError::TooLarge` hint "10MB" hardcoded、`MAX_RESPONSE_BYTES` 定数と乖離 | `format!("URL response exceeds {} bytes; fetch a smaller resource", MAX_RESPONSE_BYTES)` または `MAX_RESPONSE_BYTES_HUMAN` 公開定数 |
| BG-02 | tools/errors.rs TE-06 | `FetchError::TooManyRedirects → DataError(65)` 非retry。CDN A/B test 等で transient 可能性 | `pending_calibration`: 実データで transient vs permanent を測定後、TempFailure (75) に flip 検討 |
| ~~BG-03~~ | ~~github.rs GH-02~~ | ~~401 が `Api{401}` fall through、`Unauthorized` variant + token-rotation hint 不在~~ | **取り下げ (false premise)**: 2026-05-19 follow-up 着手時に発見、既に T-ER030 (`src/tools/errors.rs:644`) で `Api { code: 401, .. }` arm + auth hint mapping (issue #101 fix) が実装済。reviewer-rust が "no UX hint" を誤判定。型安全性のための `Unauthorized` variant 化は YAGNI、現状の `Api { code: 401, .. }` match で機能している |

## Inline comment downgrades (情報のみ、軽微)

incomplete-contract=Yes だが impact=M または reversibility=high の findings は ADR ではなく inline comment で対応推奨。promotion 候補ではないが、refactor 時の reader 体験向上のため記録:

- E-01 / E-02: `Degradation` 2-Vec representation + `degraded` stored bool の rationale comment
- TE-01 / TE-02: Slack code list の "closed-world vs Unknown escape asymmetry" 注記
- TE-03 / TE-04: `FetchError` arm asymmetry (401/403 collapse, DnsResolution helper bypass) の 1 行説明
- RT-01 / RT-03 / RT-04 / RT-05: retry magic constants と implicit cross-module invariant 注記
- BC-01 / BC-04: Brave timeout と is_retriable 4xx boundary の RFC ref
- BC-06: `SearchClient` trait contract rustdoc (ordering, max length, dedup invariant)
- GH-01: auth chain order の rationale (`GITHUB_TOKEN` が `GH_TOKEN` を超える理由)
- GH-03: `secs_until_ratelimit_reset` stale 時 `Some(0)` vs `None` の policy 注記

## Follow-up

### ADR 起票 (post-challenge 1 件)

- [ ] `docs/decisions/0010-scout-local-json-envelope-contract.md`
  - `ErrorCode::is_retryable()` mapping (`TempFailure | Timeout` のみ true、`Internal`/`Unknown` non-retryable 固定)
  - `ErrorPayload` / `SuccessEnvelope` の field omit policy (always-out vs `skip_serializing_if`)
  - README L158/L314 で公開済の `[]`-never-`null` array invariant
  - ADR-0065 JSON schema portion を supersede

### ADR 拡張 (post-challenge 1 件)

- [ ] `docs/decisions/0004-github-client-behavioral-limits.md`
  - Rule 4: list endpoint pagination 不在 (silent 100-cap、`Link` header parse 無し)
  - Rule 5: 固定 filter/sort (`state=open&sort=updated&direction=desc`、knob 無し)
  - Reassessment Trigger: "Caller requests closed-state issues or non-update-sort access"
  - Confirmation: `--help` で `repo-overview` の silent 100-cap を文書化

### Inline comment 追加 (drop だが trace 用 2 件)

- [ ] `src/brave/client.rs:179-189` `build_url` body 上に omitted params の意図 comment
- [ ] `src/retry.rs:15` `MAX_RETRY_AFTER_SECS` 上に CLI patience 理由 + env override future note

### ADR drift fix (1 件)

- [ ] ADR-0003 line 57 Note: `DegradedReason` 8 → 9 variants 更新
- [ ] `src/envelope.rs:36` rustdoc: "the three" → "the four" variants

### Bug fix follow-ups (3 件 → 別 PR / Issue)

- [ ] `FetchError::TooLarge` hint を `MAX_RESPONSE_BYTES` 定数から format
- [ ] `FetchError::TooManyRedirects` classification 再検討 (`pending_calibration`)
- [ ] `GitHubError::Unauthorized` variant + 401 dedicated arm + auth-rotation hint

## Skill Design Feedback

### Triage step が有効

>800 line files 4 件 (tools.rs, fetch.rs, slack.rs, github/format.rs) を user triage で skip。差分対比 (Part 1 の line count から増加分) を提示して skip 推奨 → ユーザ即決 → reviewer-rust 5 invocations に集約。Part 1 が 9 of 12 files を scan して 31 candidates だったのに対し、本回は 5 of 12 で 32 candidates と同等密度を達成。

### Reviewer-rust の existing ADR cross-reference 精度向上

Part 2 で指摘した "外部仕様の主張は引用源を明示" "documented? 判定前にファイル全体 scan" を reviewer prompt に明示的に含めた結果、`pending_spec_check` ラベルが reviewer 側で付与され、本回 critic-design が "B drop (`pending_spec_check` で ADR より bug fix)" に倒すのを助けた。

### Heuristic 4 の filter 力

「個人 OSS の ADR が価値を持つのは (1) 型/lint で守れない不変条件 (2) 公開 API 互換性コミットメント のみ」の heuristic を challenge 段階で明示的に渡したことで、Impact: H ラベル単独では ADR 化されない (B/D drop) ことが定量化された。Part 1+2 の累積実践知が本回 final candidate を 4 → 1 promote + 1 downgrade に絞り込むのに寄与した。
