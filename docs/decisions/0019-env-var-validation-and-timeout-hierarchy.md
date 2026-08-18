---
status: "accepted"
date: 2026-06-24
decision-makers: thkt (project owner)
---

# Environment-Variable Validation and Timeout Hierarchy

## Context and Problem Statement

scout は timeout と retry 回数を `SCOUT_*` 環境変数で受ける。AI エージェントがこれらを動的に組み立てて起動するため、不正値 (負数、巨大値、非数値、空文字列) が容易に紛れ込む。値を黙って clamp したり既定へ落とすと、エージェントは「自分が指定した値で動いている」と誤認し観測と挙動が乖離する。一方で巨大 timeout は GitHub コマンドの外側 timeout を内側 per-request timeout より下に潜り込ませ、1 リクエストの正常完了前に外側が切れる階層崩れを生む。

scout は `src/tools/config.rs` の `RuntimeConfig::from_env_with` で各 env を起動時に parse + 範囲検証し、範囲外は usage error (exit 64) で fail-fast する。timeout 下限 `TIMEOUT_MIN_SECS = 1`・上限 `TIMEOUT_MAX_SECS = 600`、retry 上限 `RETRIES_CAP = 10`。さらに外側 GitHub コマンド timeout が内側 per-request timeout を上回る階層不変条件を持つ。この検証方針と階層不変条件が ADR として記録されていない。

## Decision Drivers

- エージェントは指定値で動いていることを確信したい。silent clamp は観測と挙動を乖離させる
- 不正値はできるだけ早く (network I/O 前に) 失敗させる
- 外側コマンド timeout が内側 per-request timeout を下回ると、正常完了前に外側が切れる (issue #185)
- 設定された値が未読 (非 UTF-8) のとき、既定へ素通しせず明示的に失敗させる

## Considered Options

- Option A: 起動時に範囲検証し範囲外は fail-fast、境界定数を単一箇所に集約、階層を既定値で固定 (採用)
- Option B: 範囲外を silent に clamp して継続する
- Option C: 検証せず値をそのまま使い、下流の失敗に委ねる

## Decision Outcome

Chosen option: Option A。`RuntimeConfig::from_env_with` が各 `SCOUT_*` を読み、未設定 (`VarError::NotPresent`) は hard-coded 既定へ、設定済みは parse + 範囲検証する。timeout は `TIMEOUT_MIN_SECS..=TIMEOUT_MAX_SECS` (1..=600 秒) 外、retry は `RETRIES_CAP` (10) 超で `ScoutError::user_error` (exit 64, ADR-0002) を返し、メッセージに変数名・与値・許容範囲を含める。非数値・負数・空文字列は parse 失敗として、非 UTF-8 (`VarError::NotUnicode`) は「設定されたが読めない」として、いずれも usage error にする (既定への素通しを禁ずる)。境界定数とデフォルトは `src/tools/config.rs` に集約し、`--help` (after*help) のレンジ表記と同じ真実源を指す。github コマンドの外側 timeout 既定 (180s) は内側 per-request timeout (`HTTP_TIMEOUT`, `CANDIDATE_FETCH_TIMEOUT`) を上回る値に選び、`SCOUT*\*`override 時もこの大小を崩さない値域 (≤600s) に収める。override が効いたフィールドは`info!` で 1 件ずつ surface し、operator が active な tuning を既定ログレベルで確認できる。

Option B は silent clamp でエージェントの観測と実挙動を乖離させデバッグを困難にするため却下。Option C は不正値を network I/O 後の遠い場所で失敗させ原因特定を遅らせるため却下。

### Consequences

- Good, because 不正な env は実行前に変数名つき exit 64 で失敗し、エージェントが即自己修正できる (T-CFG010..015)
- Good, because 範囲上限が極端な値を弾き、外側 > 内側の timeout 階層 (T-CFG021) を守る
- Good, because 境界定数とデフォルトが `src/tools/config.rs` の 1 箇所に集約され、実装とテストはそこを読む
- Bad, because `src/lib.rs` の `Cli` の `after_help` は範囲と既定を文字列に手で書いており、定数を参照しない。`TIMEOUT_MIN_SECS` などを変えても help は黙って古くなる。`[T-H010]` は `SCOUT_*` の名前が出ることだけを assert し、値は見ない
- Good, because 非 UTF-8 を「設定されたが無効」として失敗させ、既定への意図せぬ素通しを防ぐ
- Good, because override の `info!` surface で active な tuning が既定ログレベルで見える (T-CFG-LOG001/003)
- Bad, because 正当だが範囲外の極端な値 (例: 意図的な 1200s) も拒否され、上限変更にはコード修正が要る
- Bad, because 階層不変条件は既定値の選択で守られ型では強制されないため、将来の定数変更で崩れうる (T-CFG021 が検出)
- Bad, because 検証は個々の env の範囲に限られ、全て範囲内だが互いに非整合な組み合わせは検出しない

### Confirmation

`src/tools/config.rs` のテストが境界と階層を pin する。`[T-CFG001]` は未設定で既定 (95/45/60/180s, retries 2) になること、`[T-CFG010]` は非数値が exit 64 で変数名を含むこと、`[T-CFG011]` は空文字列、`[T-CFG012/013]` は timeout の下限未満 (0) / 上限超 (601)、`[T-CFG014]` は retries 上限超 (11)、`[T-CFG015]` は負数がいずれも usage error になることを assert する。`[T-CFG021]` は `github_timeout` 既定が `HTTP_TIMEOUT` と `CANDIDATE_FETCH_TIMEOUT` を上回る階層を assert し、内側定数を縮めた将来の変更が不等式を壊したら検出する (issue #185)。`[T-CFG-LOG001/002/003]` は override 時のみ `info!` が出ることを assert する。

## Pros and Cons of the Options

### Option A: 範囲検証 + fail-fast + 定数集約 + 階層固定 (採用)

起動時に検証し範囲外を即 usage error にし、階層を既定値で守る。

- Good, because 観測と挙動が一致し早期に失敗する
- Good, because 上限と階層検証が timeout 崩れを防ぐ
- Bad, because 上限変更にコード修正が要る

### Option B: silent clamp

範囲外を黙って境界値へ丸める。

- Good, because 実行が止まらない
- Bad, because エージェントの指定値と実挙動が乖離しデバッグ困難

### Option C: 無検証

そのまま使い下流に委ねる。

- Good, because 実装コストゼロ
- Bad, because network I/O 後の遠い場所で失敗し原因特定が遅い

## More Information

### env と境界 (一次ソース `src/tools/config.rs` の `RuntimeConfig` と module 冒頭の既定値・env 名・範囲の定数群)

| env                           | 既定                      | 範囲    | 範囲外時 |
| ----------------------------- | ------------------------- | ------- | -------- |
| `SCOUT_FETCH_TIMEOUT_SECS`    | 95                        | 1..=600 | exit 64  |
| `SCOUT_RESEARCH_TIMEOUT_SECS` | 45                        | 1..=600 | exit 64  |
| `SCOUT_SLACK_TIMEOUT_SECS`    | 60                        | 1..=600 | exit 64  |
| `SCOUT_GITHUB_TIMEOUT_SECS`   | 180                       | 1..=600 | exit 64  |
| `SCOUT_MAX_RETRIES`           | 2 (`DEFAULT_MAX_RETRIES`) | 0..=10  | exit 64  |

`NotPresent` は既定、`NotUnicode` は usage error。parse 失敗 (非数値・空・負数) も usage error。

### timeout 階層 (T-CFG021, issue #185)

外側 `github_timeout` (180s) は GitHub コマンド全体 (`repo-tree` / `repo-read` / `repo-overview`) を縛り、内側の per-request `HTTP_TIMEOUT` (src/tools/builder.rs) と `CANDIDATE_FETCH_TIMEOUT` (src/tools/repo.rs) を上回る。180s は最複雑コマンド `repo-overview` の happy path を通し、全リクエストが retry を尽くす all-timeouts budget (~279s) は下回る fail-fast 寄りの値 (`src/tools/config.rs` の `DEFAULT_GITHUB_TIMEOUT_SECS` の doc comment に算出根拠)。

### downgrade #14 の吸収

audit の downgrade 候補 (env 検証の散在) は本 ADR に統合した。検証は `RuntimeConfig::from_env_with` の単一箇所に集約され、個別の検証コメントは本 ADR を参照すれば足りる。

### 参照

- `src/tools/config.rs` の module 冒頭の定数群と `RuntimeConfig::from_env_with` / `parse_timeout` / `parse_max_retries` (定数・parse・範囲検証)、同ファイルの `mod tests`
- `src/lib.rs` の `Cli` の `after_help` の Tuning 節 (レンジ表記)
- ADR-0002 (sysexits。範囲外は `EX_USAGE` = 64)
- ADR-0017 (drain timeout。中断時の別系統の上限)
- ADR-0018 (token resolve timeout)
- `docs/audit/2026-06-24-020601-adr-gaps.md` (本 ADR の根拠 audit、downgrade を統合)
