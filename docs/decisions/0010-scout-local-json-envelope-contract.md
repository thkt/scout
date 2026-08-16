---
status: "accepted"
date: 2026-05-19
decision-makers: thkt (project owner)
---

# Scout-local JSON envelope contract

## Context and Problem Statement

scout の `--json` 出力は 2 layer の policy で governed:

- exit codes: ADR-0002 (scout-local, sysexits.h convention 8 non-zero codes)
- `error.code` JSON tag、envelope schema 構造: `~/.claude/docs/decisions/0065-...` (dotclaude meta, external)

ADR-0002 §"More Information" は "until a scout-local ADR is promoted to capture it" と書き、JSON schema portion の scout-local 化を 2026-05-13 時点で伏線として残していた。

2026-05-19 audit (`docs/audit/2026-05-19-undocumented-decisions.md` Candidate A) で `src/envelope.rs` + README L158/L314 に **scout-local の field-level rule** が 4 つ存在することが判明した:

1. `ErrorCode::is_retryable()` mapping (`envelope.rs:201-206`): `TempFailure | Timeout` のみ true、`Internal`/`Unknown` は false 固定。
2. `ErrorPayload` field omit policy (`envelope.rs:228-236`): `code`/`message`/`retryable` は常時出力、`next_step`/`candidates` は `skip_serializing_if`。
3. `SuccessEnvelope` array field 出力 asymmetry (`envelope.rs:211-218`): `notes: Vec<String>` は常時 `[]`、`degraded_reasons` は `skip_serializing_if`。
4. `data` 配下 array field の `[]`-never-`null` invariant: README L158 と L314 で公開契約として宣言済。

これらは ADR-0065 で governed されない (schema structure 自体は ADR-0065、field-level の omit policy/retryable mapping/`[]`-vs-`null` commitment は scout 判断)。型/lint/serde derive で機械 enforce できず、test は個別 case を pin できても **新規 field 追加時の rule** を未来の reviewer に伝える媒体が無い。

## Decision Drivers

- ADR-0002 §"More Information" の supersede 伏線を実装で成就する。
- AI agent caller (`--json` の主要 consumer) は parser を `if "next_step" in payload`/`data.array.length === 0` 等の pattern で書く。field 存在性/`[]`-vs-`null` の予測可能性が破綻すると silent regression が起きる。
- scout-local の field rule (omit policy, retryable mapping) は型/lint/serde derive で機械 enforce できない。例: 将来 `Vec<String>` を `Option<Vec<String>>` に変える PR が field rule を silent に反転させる可能性がある。
- README L158/L314 は既に "All array fields are `[]` (never null) when empty" を public contract として宣言済。コードは公開契約に追従する責務がある。

## Considered Options

- (Chosen) Option A: 新規 scout-local ADR-0010 起票、ADR-0065 JSON schema portion を supersede。
- Option B: ADR-0002/ADR-0003 への section 追加で済ませる。
- Option C: inline comment + test で field-level rule を pin (ADR 化なし)。

## Decision Outcome

Chosen: Option A — 新規 ADR-0010、ADR-0065 JSON schema portion を supersede。

### Rule 1: `ErrorCode::is_retryable()` = `TempFailure | Timeout` 固定

`src/envelope.rs:201-206` の `is_retryable` は `matches!(self, TempFailure | Timeout)` のみ true を返す。各 variant の retryable 判定:

| ErrorCode                  | Retryable | Rationale                                                                                                                          |
| -------------------------- | --------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| `UsageError` (64)          | false     | caller 入力 fix が必要 (token rotation 等)                                                                                         |
| `DataError` (65)           | false     | URL / data の差し替え必要                                                                                                          |
| `NotFound` (66)            | false     | resource 不在、retry で解決しない                                                                                                  |
| `Internal` (70)            | **false** | scout-side invariant violation、bug fix が必要。retry は scout のバグを mask する                                                  |
| `IoError` (74)             | false     | external tool / IO failure、root cause fix が必要                                                                                  |
| `TempFailure` (75)         | **true**  | server-side transient (5xx, rate limit)                                                                                            |
| `Timeout` (124)            | **true**  | transport timeout、retry で解決可能                                                                                                |
| `Unknown` (104)            | **false** | classification 漏れ。`true` だと未知 cause で blind retry になり caller を巻き込む                                                 |
| `InterruptedSigint` (130)  | false     | operator が意図して中断した。ADR-0017 の「130 で retry」は caller が exit code で選ぶ戦略であり、scout 側の transient 判定ではない |
| `InterruptedSigterm` (143) | false     | 上と同じ。orchestrator による停止で、再実行が成功する根拠は無い                                                                    |

`Unknown=false` を採用した理由: `Unknown` の rate 上昇は classification 設計の verification signal として機能する。`true` を返すと caller が blind retry し、signal が消える。

### Rule 2: `ErrorPayload` field omit policy

`src/envelope.rs:228-236` の serialization 規約:

| Field        | Serialization                             | Why                                                                    |
| ------------ | ----------------------------------------- | ---------------------------------------------------------------------- |
| `code`       | always                                    | parser の枝分かれ根拠 (`code === "RATE_LIMIT"`)                        |
| `message`    | always                                    | human-readable explanation、empty string 含む                          |
| `retryable`  | always                                    | parser の retry 判定根拠 (`retryable === true`)                        |
| `next_step`  | `skip_serializing_if = "Option::is_none"` | optional hint、`null` を avoid (parser の `if "next_step" in payload`) |
| `candidates` | `skip_serializing_if = "Vec::is_empty"`   | optional alternatives、`[]` も省略                                     |

`code`/`message`/`retryable` の always emit と `next_step`/`candidates` の skip-if-empty は **意図的な分岐**:

- always emit field = parser の primary 判断に使う signal
- skip-if-empty field = secondary hint、parser が `if "X" in payload` で feature-detect

新規 field 追加時の判断 rule: parser の primary 判断に使う field は always emit、secondary hint は skip-if-empty。

### Rule 3: `SuccessEnvelope` array field omit asymmetry

`src/envelope.rs:211-218` の `notes: Vec<String>` は常時 `[]` 出力 (`skip_serializing_if` なし)、`degraded_reasons: Vec<DegradedReason>` は `skip_serializing_if = "Vec::is_empty"`。

| Field              | Serialization            | Rule basis                                                                                                     |
| ------------------ | ------------------------ | -------------------------------------------------------------------------------------------------------------- |
| `notes`            | always (`[]` when empty) | ADR-0065 で defined 済の original field、parser は `.notes.length` を常に読める前提                            |
| `degraded_reasons` | skip-if-empty            | ADR-0003 で post-hoc 追加された additive field、parser は `if "degraded_reasons" in payload` で feature-detect |

新規 additive field 追加時の rule:

- ADR-0065 で defined 済の original field を変更 → always emit を維持
- scout-local で post-hoc 追加 → `skip_serializing_if` で additive、既存 caller を壊さない

### Rule 4: `data` 配下 array field の `[]`-never-`null` invariant

`data` 配下の array field (`data.sources`, `data.fetched_pages`, `data.failed_urls` 等) は empty 時 `null` ではなく `[]` を返す。

公開契約 (README L158, L314):

> JSON envelope: `data = {query, sources, fetched_pages, failed_urls}`. All array fields are `[]` (never `null`) when empty.
>
> `data.fetched_pages` and `data.failed_urls` (research only) are unchanged in shape; both default to `[]` (never `null`) when empty

実装側 commitment:

- `Vec<T>` を `serde(skip_serializing_if = "Vec::is_empty")` で omit しない (omit すると parser の `data.failed_urls.length` が破綻)
- `Option<Vec<T>>` を使わない (`null` 出力を生む)
- 新規 array field 追加時、empty `[]` 出力を test で pin する

### Consequences

- Good, because AI agent parser が `data.array.length === 0`/`if "next_step" in payload` 等の pattern を確実に書ける。silent regression は test と本 ADR Rule で防止。
- Good, because `Unknown=false` retryable mapping により classification 漏れが silent retry で隠蔽されず、設計の verification signal が保たれる。
- Good, because ADR-0002 §"More Information" の supersede 伏線が成就、JSON schema portion が scout-local 化される。
- Good, because 新規 field 追加 PR の reviewer が omit policy rule (Rule 2/3) を本 ADR の table で確認できる。
- Bad, because ADR-0065 supersede による code-side ref (`src/envelope.rs`, `src/lib.rs`, `src/tools/errors.rs`, `tests/cli_integration.rs`) の `per ADR-0065` → `per ADR-0010` 移行 PR が follow-up として必要 (ADR-0002 supersede 時と同様の作業)。
- Bad, because Rule 4 で `Option<Vec<T>>` を禁じることで "field 不在" を意味的に表現できなくなり、`data.foo: null` が必要な case で workaround (`Option<NonEmptyVec<T>>` 等) が必要になる。

### Confirmation

- `src/envelope.rs:201-206` の `is_retryable` 実装は Rule 1 と機械的に一致 (`matches!(self, TempFailure | Timeout)`)。
- `src/envelope.rs:228-236` の `ErrorPayload` derive は Rule 2 と機械的に一致 (`code`/`message`/`retryable` 無修飾、`next_step` に `Option::is_none`、`candidates` に `Vec::is_empty`)。
- `src/envelope.rs:211-218` の `SuccessEnvelope` derive は Rule 3 と機械的に一致 (`notes` 無修飾、`degraded_reasons` に `Vec::is_empty`)。
- 既存 unit test (T-EN001..T-EN015) が omit/always-emit を個別 case で pin。
- `tests/cli_integration.rs` の `--json` end-to-end が `data.failed_urls = []` 等の `[]` 出力を検証。

## Pros and Cons of the Options

### Option A: 新規 ADR-0010 + ADR-0065 supersede (採用)

- Good, because ADR-0002 §"More Information" の伏線 ("until a scout-local ADR is promoted") を成就する。
- Good, because 4 つの field-level rule を一本化、reviewer の判断 rule が一箇所に集約。
- Good, because ADR-0065 (dotclaude meta) と scout-local の責務境界が clarify される。
- Bad, because supersede 後の code ref 移行作業が follow-up PR で発生。

### Option B: ADR-0002 / ADR-0003 への section 追加

- Good, because 既存 ADR を再利用、新規番号消費なし。
- Bad, because ADR-0002 (exit code) と ADR-0003 (error classification mapping) は責務が違う。JSON envelope の field-level rule は third concern、混入で各 ADR の Decision section が肥大化。
- Bad, because supersede note を ADR-0002 と ADR-0003 の両方に書く必要が生じ、責務分散。

### Option C: inline comment + test で pin (ADR 化なし)

- Good, because doc 追加コストゼロ。
- Bad, because comment は個別 site にしか書けず、新規 field 追加時の "omit policy はどちらか" の判断 rule が見えない。
- Bad, because test は個別 case を pin できるが、未来追加 field の rule (always emit vs skip-if-empty) は test で表現できない (型/lint 同様、enforce 不能)。

## More Information

### Supersedes (ADR-0065 carve out)

本 ADR は `~/.claude/docs/decisions/0065-scout-json-output-schema-and-sysexits-exit-code-policy.md` (dotclaude meta) の **JSON output schema portion** を scout-local 化する:

- `error.code` JSON tag (Rule 1 retryable mapping を含む)
- `ErrorEnvelope`/`SuccessEnvelope`/`ErrorPayload` 構造 (Rule 2-3 field omit policy を含む)
- `data` 配下 array field の `[]`-never-`null` invariant (Rule 4)

ADR-0065 の **sysexits portion** (8 non-zero exit codes 64/65/66/70/74/75/104/124) は ADR-0002 (scout-local) で governed (2026-05-13 supersede 済)。**Classification Priority portion** (priority 1–5 + Unknown 退避 ranking) は ADR-0011 (scout-local) で governed (2026-05-19 supersede 済)。これで ADR-0065 全 portion が scout-local 化された。

code-side migration (`per ADR-0065` → 各 scout-local ADR ref 更新) は follow-up PR で実施する。

### Reassessment Triggers

| Trigger | アクション |
| ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------ | ------------------------------------------------------- |
| 新規 `ErrorCode` variant が追加され、その retryable 判定が `TempFailure | Timeout` mapping に収まらない | Rule 1 table 拡張、`is_retryable()` の `match` arm 追加 |
| 新規 field 追加で omit policy の判断が必要 | Rule 2/3 table に新規 row 追加、`always emit`/`skip-if-empty` の判断軸を記録 |
| `data` 配下に新規 array field 追加 | Rule 4 に従い `[]`-never-`null` test を追加 |
| `Unknown` rate が caller logs で持続的に上昇 | classification design を audit、`Unknown` をさらに分類できないか検討 (新規 ErrorCode variant 化) |
| `data.foo: null` の表現が真に必要な case が発生 | Rule 4 に `Option<NonEmptyVec<T>>` 等の例外 pattern を追加 |

### 参照

- `docs/decisions/0002-adopt-sysexitsh-exit-code-convention-for-cli.md` §"More Information" (本 ADR の supersede 伏線元)
- `docs/decisions/0003-error-classification-contract-for-sysexits-and-json-output.md` (error mapping は本 ADR Rule 1 `is_retryable` の前提)
- `docs/decisions/0011-scout-local-classification-priority-policy.md` (本 ADR と並ぶ ADR-0065 supersede portion: Classification Priority)
- `~/.claude/docs/decisions/0065-scout-json-output-schema-and-sysexits-exit-code-policy.md` (本 ADR が JSON schema portion を supersede する meta ADR)
- `docs/audit/2026-05-19-undocumented-decisions.md` (本 ADR の根拠 audit、Candidate A = envelope E-03/E-04/E-05 + README P-A/P-D)
- `src/envelope.rs:201-236` (実装 site)
- `README.md` L158, L314 (公開契約の declared form)

## Addendum (2026-06-24): research の `sources` と `fetched_pages` の cardinality 非対称

ADR ギャップ監査 (`docs/audit/2026-06-24-020601-adr-gaps.md`、downgrade 候補 16) で、research の `data` 配下 array 間の cardinality 関係がコードにのみ pin され、Rule 4 (`[]`-never-`null`) では語られていないと判定された。Rule 4 を補う転記として追記する。実装は `src/search/engine.rs:55-122` (`fetch_sources`) と `:169-177` (`format_sources`) が真実源。

`research` の envelope は同じ `data` に 3 つの array を載せるが、要素数は同数ではない。

| field           | 内容                                                   | 件数                                      |
| --------------- | ------------------------------------------------------ | ----------------------------------------- |
| `sources`       | Brave 検索結果の全件 (title + url、未 fetch)           | 検索が返した全件                          |
| `fetched_pages` | `sources` の先頭 `depth` 件を fetch して成功したページ | `take(depth)` のうち成功分 (depth 1..=10) |
| `failed_urls`   | 先頭 `depth` 件のうち fetch 失敗した URL + 理由        | `take(depth)` のうち失敗分                |

`fetch_sources` は `sources.iter().take(depth)` のみ fetch するため、不変条件は `len(fetched_pages) + len(failed_urls) <= depth <= len(sources)` となる。consumer は `sources` と `fetched_pages` を同じ index で対応づけてはならない (`sources[i]` が `fetched_pages[i]` に対応する保証は無い。`fetched_pages` は成功分のみを元の取得順に並べ替えて詰める)。3 array はいずれも Rule 4 に従い空時 `[]` を返す。新規 array field を `data` に足す際は、件数が他 array と独立しうる場合この非対称を本節に追記する。

## Addendum (2026-08-17): signal 中断も `--json` では envelope で出す

`--json` の下で SIGINT / SIGTERM による中断だけが `error: interrupted (SIGINT)` という bare line を stderr へ書いており、他の全 error path が envelope を出す中でここだけ契約から外れていた。stderr を JSON 前提で parse する caller は、この 1 行を落として中断そのものを見失う。本 ADR の Rule 1 table に `InterruptedSigint` (130) と `InterruptedSigterm` (143) を追加し、`error.code` の値集合を 8 から 10 へ広げる。実装は `src/lib.rs` の `interrupted_line` / `interrupt_code` と `src/envelope.rs` の `ErrorCode` が真実源。

exit code 130 / 143 は ADR-0017 の管轄で変えない。`ErrorCode::exit_code()` は同じ 2 値を `InterruptSignal` (`src/signals.rs`) とは別に持つ。`InterruptSignal::Sigterm` が `#[cfg(unix)]` である一方、`error.code` の値集合は platform で変わってはならないため、`ErrorCode` 側の 2 variant は無条件で定義する。2 つの table が食い違わないことは T-W009 が assert する。

`retryable` は両方 false とし、Rule 1 の `matches!(self, TempFailure | Timeout)` は変えない。ADR-0017 が書く「130 で retry」は caller が exit code を見て選ぶ戦略であって、scout 側が transient と判定したという意味ではない。
