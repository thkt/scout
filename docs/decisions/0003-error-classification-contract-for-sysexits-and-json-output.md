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

- exit code + JSON `--json` mode は CLI script/agent のメタ判断 source of truth
- Slack 5xx/GitHub 5xx の retryability は CLI script の retry/fail-fast 判断に直結
- partial failure (`repo_overview` で readme は取れたが issues は失敗等) を programmatic 通知できないと自動化価値が下がる

## Considered Options

- Option A: HTTP status → ErrorCode の uniform mapping rule + partial failure に `degraded` flag
- Option B: per-error-source ad-hoc mapping (現状)
- Option C: 全 error type をリッチに展開し、JSON output に full error union を expose

## Decision Outcome

Chosen option: Option A, because public CLI contract として一貫した classification を提供しつつ、既存 ADR-0002 を拡張する形で実装コストを抑えられる。

### HTTP status → ErrorCode mapping

| HTTP status   | ErrorCode   | sysexits       | Retryable?          |
| ------------- | ----------- | -------------- | ------------------- |
| 5xx (500-599) | TempFailure | 75 EX_TEMPFAIL | Yes                 |
| 408, 429      | TempFailure | 75 EX_TEMPFAIL | Yes                 |
| 404           | NotFound    | 66 EX_NOINPUT  | No                  |
| 401, 403      | UsageError  | 64 EX_USAGE    | No (auth misconfig) |
| 4xx (other)   | DataError   | 65 EX_DATAERR  | No                  |

例外: API-specific な再分類が必要な場合 (例: Slack 4xx の中で transient なものがあれば) は doc コメントで明示する。

### Partial Failure Handling

`repo_overview` 等の multi-fetch 経路で部分失敗が発生した場合:

- 結果 struct に `degraded: bool` と `reason: Option<DegradedReason>` フィールドを追加
- `--json` 出力にも expose
- `DegradedReason` は `ReadmeMissing`, `IssuesFetchFailed`, `PullsFetchFailed`, `ReleasesFetchFailed` 等 enum で typed
- silent log-only fallback (`unwrap_or_note` 系) は廃止

> **Note (2026-05-13, post-implementation audit)**: `degraded: bool` field は本 ADR 起票前から `src/envelope.rs` に既存。本 ADR は既存 behavior を canonical contract として formalize し、残り (`DegradedReason` typed enum + `unwrap_or_note` → `unwrap_or_degraded` refactor + JSON schema 拡張) を follow-up scope として明示する。
>
> **Note (2026-05-13, follow-up implementation)**: 残り scope を additive route で実装完了。実装した variants は callsite から逆算した 8 種類 (`IssuesFetchFailed`, `PullsFetchFailed`, `ReleasesFetchFailed`, `ReadmeFetchFailed`, `ReadmeBlobFetchFailed`, `ReadmeDecodeFailed`, `UrlFetchFailed`, `ReadabilityFallback`)。当初挙げた `ReadmeMissing` は `resolve_readme` 実装が 404 を silent 扱い (notes 追加なし) としていたため variant 化せず、README 系は failure mode で 3 種に分割。JSON schema 変更は additive (`degraded_reasons` field を `skip_serializing_if = "Vec::is_empty"` で追加) なので Cargo `version = "1.0.0"` → `"1.1.0"` の minor bump。既存 caller (`notes` の `Vec<String>` 構造を見る) は無影響。
>
> **Note (2026-05-19, post-ADR-0005 update)**: ADR-0005 (Brave Search switch, 2026-05-15) に伴い `BraveSearchFailed` variant 追加で**計 9 variants**。`unwrap_or_degraded` 経由で meaningful label を持つのは `*FetchFailed` の 3 つに加えて `BraveSearchFailed` の合計 4 variants。命名規約 (`*FetchFailed`) の例外として `BraveSearchFailed` は Brave API endpoint 機能名 (search) に倣う。発見元 audit: `docs/audit/2026-05-19-undocumented-decisions.md` E-06。
>
> **Note (2026-06-17, post-issue-#222 update)**: `fetch_slack` の cap/truncate を degradation channel に接続するため Slack 用 3 variant 追加 (`SlackThreadTruncated`, `SlackUsersCapped`, `SlackOutputTruncated`) で**計 12 variants**。この 3 つは `fetch_slack` が callsite で note text を直接構築するため `unwrap_or_degraded` を経由せず、`label()` は exhaustive match 用の placeholder。よって `unwrap_or_degraded` 経由で meaningful label を持つのは引き続き 4 variants で不変。追加は serde additive (新 variant は `degraded_reasons` の skip-if-empty で feature-detect 可能) のため既存 JSON consumer に無影響。発見元: issue #222 実装時の docs drift 確認。
>
> **Note (2026-06-17, post-issue-#241 update)**: `scout fetch` が charset mislabel/unsupported 時に mojibake を exit 0 で silent 返却する fail-silent を解消するため `DecodeUncertain` variant 追加で**計 13 variants**。label-first decode が had_errors を出し、かつ chardetng の reliability gate (multi-byte のみ信頼) が detection を拒否した場合に、best-effort lossy body (`String::from_utf8_lossy`) を exit 0 のまま返しつつ `DECODE_UNCERTAIN` を degraded_reasons に立てる。multi-byte の mislabel (例: Shift_JIS を utf-8 ラベル) は detection で復元され uncertain を立てない。`FetchError` に decode variant は追加せず exit 65 も使わない (issue #241 で degraded signal 中心の設計を選択)。この variant は `fetch` callsite (`src/tools/query.rs`) が note text を直接構築するため `unwrap_or_degraded` を経由せず、`label()` は exhaustive match 用の placeholder ("resource")。よって `unwrap_or_degraded` 経由で meaningful label を持つのは引き続き 4 variants で不変。追加は serde additive のため既存 JSON consumer に無影響。発見元: issue #241。
>
> **Note (2026-08-02, table single-sourced / exit code change)**: 上の status 表は 3 backend が raw status から個別に再導出しており、2 箇所ずれていた。`Classification::from_http_status` (`src/classify.rs`) に 1 本化し、`GitHubError::Api` / `BraveError::Api` / `BraveError::Server` / `FetchError::Status` が委譲する。これに伴い caller 向け exit code が変わる: GitHub の HTTP 408 が 65 DataError (retryable=false) から 75 TempFailure (retryable=true) へ、Brave の HTTP 404 が 65 から 66 NotFound へ。Brave 408 も 75 になるため `BraveError::is_degradable` が false から true に変わり、`scout search` が error を propagate せず degraded な空結果を返すようになる。表と実装の一致は `[T-ER034]` が 4 backend 横断で pin する。`SlackError::Server` は委譲しない (下の Note を参照)。
>
> **Note (2026-08-02, Slack の API-specific 再分類)**: 上の表の「例外」行が要求する doc コメントとして記録する。Slack は app 層の失敗を HTTP 200 の body 内 `ok: false` で返すため、非 2xx (429 を除く) は Slack の判断ではなく間に入った proxy / gateway 由来である。したがって status を表どおりに読むと gateway の 404 を「リソース不在」と誤報する。`SlackError::Server(u16)` は status を問わず TempFailure として扱う。
>
> **Note (2026-08-02, drift correction)**: 上の 3 つの Note が「`unwrap_or_degraded` 経由で meaningful label を持つのは 4 variants」と記載しているが、実際は `*FetchFailed` の 3 つのみ。`unwrap_or_degraded` の引数は `Result<Vec<T>, github::GitHubError>` であり、Brave の失敗はこの型で表現できないため helper に渡せない。`BraveSearchFailed` は導入時 (cf32ebf) から `src/tools/query.rs` の callsite が note text (`"Brave search failed: {e}"`) を直接構築しており、`label()` の `"Brave search"` arm は一度も到達していなかった。当該 arm は `"resource"` 群へ統合済み。variant 総数 13 と JSON schema は不変。
>
> **Note (2026-08-07, 名前解決の呼び出し失敗と cap の区別)**: `fetch_slack` の名前解決が `users.info` / `conversations.info` 呼び出し自体の失敗 (transport/API error) で raw ID にフォールバックしても、従来は `SlackUsersCapped` (件数超過による cap) と区別がつかなかった。呼び出し側 AI エージェントが raw ID の原因を判別できるよう `SlackLookupFailed` variant 追加で**計 14 variants**。`SlackClient::fetch_message` が `resolve_channel` の失敗と `prefetch_users` の in-cap 失敗のいずれかで `SlackFetchOutcome::lookups_failed` を立て、`fetch_slack` (`src/tools/query.rs`) が cap 系 3 variant と同じ形で preamble note (`"Some user or channel lookups failed, so those authors and mentions show raw IDs."`) を組み立てて `degradation.push` する。この variant も callsite で note text を直接構築するため `unwrap_or_degraded` を経由せず、`label()` は既存の `"resource"` 群にそのまま合流する。よって `unwrap_or_degraded` 経由で meaningful label を持つのは引き続き `*FetchFailed` の 3 variants で不変。追加は serde additive (新 variant は `degraded_reasons` の skip-if-empty で feature-detect 可能) のため既存 JSON consumer に無影響。
>
> **README 404 silent の意図的選択**: 「README が存在しない repo」は scout として degraded ではない (overview は他フィールドだけで成立)。404 を silent にすることで `degraded` flag を「abnormal なときだけ立てる」契約に保つ。代わりに JSON consumer は `data.readme` が null か否かで「README の有無」を直接判別可能 (404 と fetch error の区別は `degraded_reasons` の `ReadmeFetchFailed` の有無で行う)。403 等の non-404 4xx は `ReadmeFetchFailed` で集約しており、status code 別の細分は当面 scope 外。

### Consequences

- Good, because CLI script が exit code で retry/fail-fast 判断可能、ADR-0002 contract が enforced される
- Good, because `--json` caller が partial failure を programmatic 検出可能、agent が次アクションを自動判断できる
- Bad, because 既存 code (`SlackError::Api`, `unwrap_or_note`) の refactoring が必要 (本 ADR 後の別 PR で実装)
- Bad, because `degraded` flag 導入で JSON schema 変更 (semver bump 検討)

### Confirmation

- 各 error source (Slack/GitHub/Fetch/Gemini) で HTTP status → ErrorCode mapping unit test
- `repo_overview` partial failure path で `--json` 出力に `degraded` フィールドが含まれることの integration test
- T-ER001a/b/c, T-ER002, T-ER003 は ADR-0002 (exit code values) と本 ADR (mapping rule) の両方で binding。両 ADR ref を doc コメントに記載

## Pros and Cons of the Options

### Option A: Uniform mapping rule + degraded flag (採用)

- Good, because mapping rule が単一 table、enforce が test 一本化可能
- Good, because `degraded` flag で partial failure 検出が programmatic
- Bad, because 既存 ad-hoc mapping (Slack, unwrap_or_note 等) refactoring 必要
- Bad, because behavior 変更で CLI 既存 caller (exit code を hardcode していた script) が break する可能性

### Option B: per-error-source ad-hoc mapping (現状)

- Good, because 現コードを変えなくて済む
- Bad, because ADR-0002 contract 違反が散発、契約として機能しない
- Bad, because reviewer が mapping rule を都度判断、code review 負荷大

### Option C: 全 error type を JSON union で expose

- Good, because 完全な error type 情報を caller に提供
- Bad, because JSON schema が複雑化、agent/script の処理コスト増
- Bad, because internal error type のリーク (encapsulation 違反)

## More Information

### Implementation Guidelines

- 各 `ErrorCode` バリアントの construction site で本 ADR の mapping table を参照
- `unwrap_or_note` を `unwrap_or_degraded` に rename 済み (`src/tools/errors.rs` `unwrap_or_degraded`)。`DegradedReason` を受け取り、`Degradation::push` 経由で `(notes[i], reasons[i])` の pair invariant を保ったまま統一的に蓄積する形に refactor 済み
- `DegradedReason` の variants は `repo_overview` 等の現実の failure mode を反映 (実装後 12 variants、上記 Note 2026-06-17 post-issue-#222 update を参照)
- ADR-0011 §Classification Priority Table の 5 段ルール (USAGE → DATA → NOT_FOUND → TEMP_FAILURE → INTERNAL → UNKNOWN 退避) を各 error type の `classify()` メソッド (`src/slack.rs:56-94`, `src/github/errors.rs:58-108`) の match arm 順序と `// Priority N` コメントで明示する。各 `From<...>` 実装は `e.classify()` に委譲する (`src/tools/errors.rs:191-217`)。`*Error::Api { code }` の 4xx は priority 2 (DataError) に集約する。Priority 5 (INTERNAL) 以下は 3 つの sibling constructor に分離する: `internal_bug()` は scout-side invariant violation (例: deserialize 想定外 schema) を `ErrorCode::Internal` (exit 70 EX_SOFTWARE) で表し、`io_error()` は scout の不変条件外にある external tool/IO failure (例: headless browser CDP error) を `ErrorCode::IoError` (exit 74 EX_IOERR) で表し、`unknown()` は priority 1-5 のどれにも該当しない unclassifiable failure を `ErrorCode::Unknown` (exit 104 PJ extension) で退避する。3 つの分離により caller script/agent は scout 側 bug (70) と外部要因 (74) と分類欠落 (104) を programmatic 判別できる

### Reassessment Triggers

| Trigger                                              | アクション                                     |
| ---------------------------------------------------- | ---------------------------------------------- |
| 5xx の中で 501/505 等 permanent な status が一般化   | mapping table の 5xx 全 retryable 規則を再評価 |
| GitHub / Slack / Gemini が新規 4xx status を導入     | mapping table に row 追加                      |
| `degraded` flag が caller 側で無視される傾向が広まる | flag 設計の usability 再評価                   |

### 参照

- `docs/decisions/0002-adopt-sysexitsh-exit-code-convention-for-cli.md` (exit code 値の source)
- `docs/decisions/0010-scout-local-json-envelope-contract.md` (JSON schema portion を scout-local 化、本 ADR の Degradation field omit policy も Rule 2-3 で governed)
- `docs/decisions/0011-scout-local-classification-priority-policy.md` (Classification Priority portion を scout-local 化、本 ADR が参照する 5 段 ranking の現行 source of truth)
- `~/.claude/docs/decisions/0065-scout-json-output-schema-and-sysexits-exit-code-policy.md` (全 portion supersede 済 meta ADR、historical reference)
- `docs/audit/2026-05-13-undocumented-decisions-part2.md` (本 ADR の根拠 audit、Candidate #7 + #8)
- `src/tools/errors.rs` (`From<SlackError>` priority-2 集約 + `unwrap_or_degraded` 実装), `src/envelope.rs` (`Degradation`/`DegradedReason` typed enum) — implementation sites (audit 時点の違反は resolved 済み、historical record は `docs/audit/2026-05-13-undocumented-decisions-part2.md` を参照)
- `docs/audit/2026-05-14-adr-drift.md` (PR #94 後の追従 audit)
