---
status: "accepted"
date: 2026-05-13
decision-makers: thkt (project owner)
---

# Error Classification Contract for sysexits and JSON Output

## Context and Problem Statement

ADR-0002 は sysexits.h exit code の値 (64/65/66/74/75) を採用したが、各 domain error (SlackError, GitHubError, FetchError, GeminiError) を `ErrorCode` に mapping するルールは未明示。Part 2 audit (`docs/audit/2026-05-13-undocumented-decisions-part2.md`) で 2 つの contract gap が露出した。

1. `SlackError::Api` が全 HTTP status を `user_error` (exit 64) に mapping。Slack 5xx も exit 64 を返し、ADR-0002 の sysexits 規約に反する (Slack 5xx は本来 EX_TEMPFAIL = 75)
2. `unwrap_or_note` 関数が GitHub error を log warning + prose note に silent 格下げ。`--json` 出力で partial failure を caller が programmatic 検出できない

ADR-0002 の table は exit code 値のみで domain mapping を扱わず、JSON output schema は ADR-0065 (dotclaude) と分散している。本 ADR は両者を統合した classification contract を確立する。

## Decision Drivers

* exit code + JSON `--json` mode は CLI script / agent のメタ判断 source of truth
* Slack 5xx / GitHub 5xx の retryability は CLI script の retry/fail-fast 判断に直結
* partial failure (`repo_overview` で readme は取れたが issues は失敗等) を programmatic 通知できないと自動化価値が下がる

## Considered Options

* Option A: HTTP status → ErrorCode の uniform mapping rule + partial failure に `degraded` flag
* Option B: per-error-source ad-hoc mapping (現状)
* Option C: 全 error type をリッチに展開し、JSON output に full error union を expose

## Decision Outcome

Chosen option: Option A, because public CLI contract として一貫した classification を提供しつつ、既存 ADR-0002 を拡張する形で実装コストを抑えられる。

### HTTP status → ErrorCode mapping

| HTTP status         | ErrorCode    | sysexits    | Retryable? |
| ------------------- | ------------ | ----------- | ---------- |
| 5xx (500-599)       | TempFailure  | 75 EX_TEMPFAIL | Yes |
| 408, 429            | TempFailure  | 75 EX_TEMPFAIL | Yes |
| 404                 | NotFound     | 66 EX_NOINPUT | No |
| 401, 403            | UsageError   | 64 EX_USAGE | No (auth misconfig) |
| 4xx (other)         | DataError    | 65 EX_DATAERR | No |

例外: API-specific な再分類が必要な場合 (例: Slack 4xx の中で transient なものがあれば) は doc コメントで明示する。

### Partial Failure Handling

`repo_overview` 等の multi-fetch 経路で部分失敗が発生した場合:

* 結果 struct に `degraded: bool` と `reason: Option<DegradedReason>` フィールドを追加
* `--json` 出力にも expose
* `DegradedReason` は `ReadmeMissing`, `IssuesFetchFailed`, `PullsFetchFailed`, `ReleasesFetchFailed` 等 enum で typed
* silent log-only fallback (`unwrap_or_note` 系) は廃止

> **Note (2026-05-13, post-implementation audit)**: `degraded: bool` field は本 ADR 起票前から `src/envelope.rs` に既存。本 ADR は既存 behavior を canonical contract として formalize し、残り (`DegradedReason` typed enum + `unwrap_or_note` → `unwrap_or_degraded` refactor + JSON schema 拡張) を follow-up scope として明示する。
>
> **Note (2026-05-13, follow-up implementation)**: 残り scope を additive route で実装完了。実装した variants は callsite から逆算した 8 種類 (`IssuesFetchFailed`, `PullsFetchFailed`, `ReleasesFetchFailed`, `ReadmeFetchFailed`, `ReadmeBlobFetchFailed`, `ReadmeDecodeFailed`, `UrlFetchFailed`, `ReadabilityFallback`)。当初挙げた `ReadmeMissing` は `resolve_readme` 実装が 404 を silent 扱い (notes 追加なし) としていたため variant 化せず、README 系は failure mode で 3 種に分割。JSON schema 変更は additive (`degraded_reasons` field を `skip_serializing_if = "Vec::is_empty"` で追加) なので Cargo `version = "1.0.0"` → `"1.1.0"` の minor bump。既存 caller (`notes` の `Vec<String>` 構造を見る) は無影響。
>
> **README 404 silent の意図的選択**: 「README が存在しない repo」は scout として degraded ではない (overview は他フィールドだけで成立)。404 を silent にすることで `degraded` flag を「abnormal なときだけ立てる」契約に保つ。代わりに JSON consumer は `data.readme` が null か否かで「README の有無」を直接判別可能 (404 と fetch error の区別は `degraded_reasons` の `ReadmeFetchFailed` の有無で行う)。403 等の non-404 4xx は `ReadmeFetchFailed` で集約しており、status code 別の細分は当面 scope 外。

### Consequences

* Good, because CLI script が exit code で retry/fail-fast 判断可能、ADR-0002 contract が enforced される
* Good, because `--json` caller が partial failure を programmatic 検出可能、agent が次アクションを自動判断できる
* Bad, because 既存 code (`SlackError::Api`, `unwrap_or_note`) の refactoring が必要 (本 ADR 後の別 PR で実装)
* Bad, because `degraded` flag 導入で JSON schema 変更 (semver bump 検討)

### Confirmation

* 各 error source (Slack / GitHub / Fetch / Gemini) で HTTP status → ErrorCode mapping unit test
* `repo_overview` partial failure path で `--json` 出力に `degraded` フィールドが含まれることの integration test
* T-ER001a/b/c, T-ER002, T-ER003 は ADR-0002 (exit code values) と本 ADR (mapping rule) の両方で binding。両 ADR ref を doc コメントに記載

## Pros and Cons of the Options

### Option A: Uniform mapping rule + degraded flag (採用)

* Good, because mapping rule が単一 table、enforce が test 一本化可能
* Good, because `degraded` flag で partial failure 検出が programmatic
* Bad, because 既存 ad-hoc mapping (Slack, unwrap_or_note 等) refactoring 必要
* Bad, because behavior 変更で CLI 既存 caller (exit code を hardcode していた script) が break する可能性

### Option B: per-error-source ad-hoc mapping (現状)

* Good, because 現コードを変えなくて済む
* Bad, because ADR-0002 contract 違反が散発、契約として機能しない
* Bad, because reviewer が mapping rule を都度判断、code review 負荷大

### Option C: 全 error type を JSON union で expose

* Good, because 完全な error type 情報を caller に提供
* Bad, because JSON schema が複雑化、agent / script の処理コスト増
* Bad, because internal error type のリーク (encapsulation 違反)

## More Information

### Implementation Guidelines

* 各 `ErrorCode` バリアントの construction site で本 ADR の mapping table を参照
* `unwrap_or_note` は `unwrap_or_degraded` (仮称) に rename し、`Result<T, DegradedReason>` を返す形に refactor
* `DegradedReason` の variants は `repo_overview` 等の現実の failure mode を反映 (4-6 variant 程度を想定)
* ADR-0065 §Classification Priority の 5 段ルール (USAGE → DATA → NOT_FOUND → TEMP_FAILURE → INTERNAL → UNKNOWN 退避) を各 `From<...>` 実装の match arm 順序と `// Priority N` コメントで明示する。`*Error::Api { code }` の 4xx は priority 2 (DataError) に集約し、`internal()` への fold off は priority 5 (scout-side invariant violation) と Unknown 退避にのみ使用する

### Reassessment Triggers

| Trigger                                                          | アクション                            |
| ---------------------------------------------------------------- | ------------------------------------- |
| 5xx の中で 501/505 等 permanent な status が一般化               | mapping table の 5xx 全 retryable 規則を再評価 |
| GitHub / Slack / Gemini が新規 4xx status を導入                 | mapping table に row 追加              |
| `degraded` flag が caller 側で無視される傾向が広まる             | flag 設計の usability 再評価          |

### 参照

* `docs/decisions/0002-adopt-sysexitsh-exit-code-convention-for-cli.md` (exit code 値の source)
* `~/.claude/docs/decisions/0065-scout-json-output-schema-and-sysexits-exit-code-policy.md` (JSON schema 部分は依然 ADR-0065 active)
* `docs/audit/2026-05-13-undocumented-decisions-part2.md` (本 ADR の根拠 audit、Candidate #7 + #8)
* `src/tools/errors.rs:225` (SlackError::Api 違反箇所), `src/tools/errors.rs:266` (unwrap_or_note 違反箇所)
