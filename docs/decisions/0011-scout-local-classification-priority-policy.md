---
status: "accepted"
date: 2026-05-19
decision-makers: thkt (project owner)
---

# Scout-local Classification Priority Policy

## Context and Problem Statement

scout の `ScoutError` → `ErrorCode` 分類は `src/tools/errors.rs` の `match` arm 順序で実装されている。複数 priority arm が同一 error に当てはまる場合 (例: `*Error::Api { 4xx }` が "URL data error" にも "auth misconfig" にも該当しうる)、評価順序が contract の一部となる。

この order は ADR-0065 (`~/.claude/docs/decisions/0065-scout-json-output-schema-and-sysexits-exit-code-policy.md`, dotclaude meta) §Classification Priority で定義され、code-side では `// per ADR-0065 priority N` の inline comment で各 arm に明記してきた。

2026-05-19 時点で ADR-0065 の portion 切り出しは以下まで進んでいる:

- sysexits portion (8 non-zero exit codes 64/65/66/70/74/75/104/124): ADR-0002 (scout-local, 2026-05-13 supersede)
- JSON schema portion (envelope structure, `error.code` JSON tag, field omit policy, `[]`-never-`null` invariant): ADR-0010 (scout-local, 2026-05-19 supersede, PR #142)

残るのが **Classification Priority portion** で、code-side ref 11 箇所 (`src/envelope.rs:197`, `src/tools/errors.rs:84, 165, 256, 277, 577, 580, 602, 624, 648, 667, 713`) が依然 `per ADR-0065` を指している。dotclaude meta への external dep が code レベルで残っている状態。

scout repo を clone した contributor は `~/.claude/docs/decisions/0065-...` を読めず、`// per ADR-0065 priority 2` の参照先が解決できない。ADR-0002 / ADR-0010 と同パターンで Classification Priority portion を scout-local 化する。

## Decision Drivers

- ADR-0002 (sysexits) / ADR-0010 (JSON schema) の supersede と並ぶ「meta ADR の scout-local 化」series の完結。
- 分類 priority は `match` arm 順序として code-level invariant。order が崩れると 4xx classification が priority 4 (TempFailure) に流れる等の silent regression を起こす。型 / lint で機械 enforce できず、ADR + inline comment が source of truth。
- AI agent contributor が match arm 順序の rationale を問うとき、参照先が scout repo 外 (dotclaude meta) だと clone-only な user に解決できない。
- `Unknown` rate の上昇は classification 設計 audit の signal として機能する。この設計意図を scout-local ADR で pin する。

## Considered Options

- (Chosen) Option A: 新規 scout-local ADR-0011 起票、ADR-0065 §Classification Priority を supersede。
- Option B: ADR-0010 に Classification Priority section を追加 (scope 拡大)。
- Option C: ADR-0003 (HTTP status → ErrorCode mapping) に Classification Priority section を追加。
- Option D: code-side ref を inline comment + table のみで維持 (ADR 化なし)。

## Decision Outcome

Chosen: Option A — 新規 ADR-0011、ADR-0065 §Classification Priority を supersede。

### Classification Priority Table

`ScoutError` → `ErrorCode` mapping は以下の優先順位で評価する。上から順、最初にマッチした priority で確定。

| 優先 | 条件 | `ErrorCode` | sysexits (per ADR-0002) |
| --- | --- | --- | --- |
| 1 | env var missing / 設定起因 / 引数誤り / auth misconfig (401/403) | `UsageError` | 64 EX_USAGE |
| 2 | URL / owner / repo / encoding の形式不正、API 4xx (other than 401/403/404) | `DataError` | 65 EX_DATAERR |
| 3 | リソース不在 (404, search 0 件) | `NotFound` | 66 EX_NOINPUT |
| 4 | retry で回復見込みあり (rate limit, 5xx, transport timeout) | `TempFailure` または `Timeout` | 75 EX_TEMPFAIL / 124 |
| 5 | scout 内部不変条件違反 (schema mismatch, invalid state) | `Internal` | 70 EX_SOFTWARE |
| 退避 | priority 1-5 のどれにも fall through しなかった | `Unknown` | 104 (PJ extension) |

`IoError` (74 EX_IOERR) は priority slot を占めない: external tool / IO failure (例: headless browser CDP error) は priority 5 (scout-side bug) でも priority 4 (retry 見込み) でもない別 axis のため、`io_error()` constructor で直接 `ErrorCode::IoError` に分類する (ADR-0003 §Decision Outcome 参照)。

### Application Rule

上から順に評価、マッチした時点で確定。

例 1: GitHub API から 404 と rate limit 余地のあるレスポンスが同時にあった場合、優先 3 で `NotFound` (exit 66) を採用 (rate limit より先に 404 評価)。

例 2: `*Error::Api { code }` で 401/403/404 以外の 4xx (例: 422) は、優先 2 で `DataError` を採用 (`TempFailure` priority 4 より先に DataError priority 2 評価)。

例 3: Slack `internal_error` レスポンスは API endpoint からの transient signal なので priority 4 `TempFailure` を採用 (priority 5 `Internal` ではない、scout-side bug ではないため)。

### Unknown 退避の意味

`Unknown` rate の上昇は classification 設計の verification signal として機能する。`Unknown` を `false` retryable mapping (per ADR-0010 Rule 1) としているのは、blind retry によって signal が消えるのを防ぐため。`anyhow::Error` 等の握り潰しを `Unknown` で expose する目的で意図的に独立した priority slot を持たない (退避扱い)。

### Consequences

- Good, because Classification Priority portion が scout-local 化、dotclaude meta への code-level dep が解消される。
- Good, because ADR-0002 (sysexits) / ADR-0010 (JSON schema) / ADR-0011 (Classification Priority) の 3 つで ADR-0065 (meta) 全 portion を scout-local 化、code から ADR-0065 を ref する必要が無くなる。
- Good, because `match` arm 順序の rationale が ADR table で確認可能。AI agent contributor が new ErrorCode variant 追加時に priority slot を決定できる。
- Bad, because supersede 後の code-side ref 移行 PR が follow-up として必要 (本 PR に同梱)。

### Confirmation

- `src/tools/errors.rs` の各 `From<*Error>` 実装が本 ADR table の priority 順で `match` arm を並べていることを inline comment (`// per ADR-0011 priority N`) で確認可能。
- 既存 unit test `T-ER023` (priority 2 wins over priority 5 for Api 4xx) / `T-ER024` (priority 4 TempFailure takes precedence for Api 5xx) が priority 評価順を pin。
- `ugrep "ADR-0065" src/ tests/` で hit 0 (本 PR で移行完了)。

## Pros and Cons of the Options

### Option A: 新規 ADR-0011 + ADR-0065 §Classification Priority supersede (採用)

- Good, because ADR-0002 / ADR-0010 と同じ shape で meta ADR の portion 切り出しを完結。
- Good, because Classification Priority の rationale が単独 ADR として閲覧可能、ADR-0010 の field-level rule とは別軸で読める。
- Bad, because ADR 件数が増える (number 消費)。

### Option B: ADR-0010 への section 追加

- Good, because 既存 ADR 再利用、新規番号消費なし。
- Bad, because ADR-0010 は field-level rule (omit policy, retryable mapping, `[]`-never-`null`) を扱う ADR で、Classification Priority は別 concern。混入で ADR-0010 の Decision section が肥大化。
- Bad, because ADR-0010 supersede notice の boundary (JSON schema portion) を侵食する。

### Option C: ADR-0003 への section 追加

- Good, because ADR-0003 が既に HTTP status → ErrorCode mapping を扱う。Classification Priority も近接 concern。
- Bad, because ADR-0003 は HTTP-axis (status code → ErrorCode) の mapping、本 ADR は ScoutError variant-axis (どの priority に振るか) で軸が違う。混入で ADR-0003 の責務が unclear に。
- Bad, because ADR-0065 supersede の line (sysexits / JSON schema / Classification Priority の 3 portion 切り出し) に対し、ADR-0003 は ADR-0065 supersede chain の外にある。Pattern を壊す。

### Option D: inline comment + table のみ (ADR 化なし)

- Good, because doc 追加コストゼロ。
- Bad, because match arm 順序の rationale (なぜ priority 2 が priority 4 より優先か) を ADR table で justify できない。
- Bad, because dotclaude meta への code-level dep が残り続ける。

## More Information

### Supersedes (ADR-0065 Classification Priority portion)

本 ADR は `~/.claude/docs/decisions/0065-scout-json-output-schema-and-sysexits-exit-code-policy.md` (dotclaude meta) の **§Classification Priority** を scout-local 化する。

ADR-0065 の他 portion は以下で scout-local 化済:

- sysexits portion: ADR-0002 (scout-local, 2026-05-13 supersede)
- JSON schema portion: ADR-0010 (scout-local, 2026-05-19 supersede)
- Classification Priority portion: **本 ADR-0011** (2026-05-19 supersede)

これで ADR-0065 全 portion が scout-local 化、code-side の `per ADR-0065` ref を本 PR で `per ADR-0002` / `per ADR-0010` / `per ADR-0011` に migrate する。

### Reassessment Triggers

| Trigger | アクション |
| --- | --- |
| 新規 `ErrorCode` variant 追加、priority slot が現行 5 段に収まらない | priority table 拡張、`match` arm 順序を本 ADR に同期 |
| `Unknown` rate が caller logs で持続的に上昇 | classification design audit、`Unknown` を新規 priority slot に分類できないか検討 (新規 ErrorCode variant 化) |
| priority 評価順 (上から順、マッチで確定) が用法上不適と判明 | 評価 model 再考 (例: priority weight に変更、本 ADR supersede + 移行 ADR 起票) |
| IoError を priority slot に取り込みたい case が surface | 本 ADR table の axis 再評価 (`io_error` constructor の存在意義を再検討) |

### 参照

- `docs/decisions/0002-adopt-sysexitsh-exit-code-convention-for-cli.md` (本 ADR と並ぶ supersede portion 1)
- `docs/decisions/0003-error-classification-contract-for-sysexits-and-json-output.md` (HTTP status → ErrorCode mapping、本 ADR と別軸)
- `docs/decisions/0010-scout-local-json-envelope-contract.md` (本 ADR と並ぶ supersede portion 2、Rule 1 `is_retryable` が本 ADR の Unknown 退避と整合)
- `~/.claude/docs/decisions/0065-scout-json-output-schema-and-sysexits-exit-code-policy.md` §Classification Priority (本 ADR が supersede する meta ADR section)
- `src/tools/errors.rs` (match arm 順序の実装 site)
