---
status: "accepted"
date: 2026-06-24
decision-makers: thkt (project owner)
---

# Signal Exit Codes and Graceful Drain

## Context and Problem Statement

scout は shell script・エージェント・orchestration から起動され、中断時に呼び出し側へ意味のある終了コードを返す必要がある。SIGINT (Ctrl-C) と SIGTERM (shell timeout / `kill` 既定) を区別できれば、呼び出し側は retry とタイムアウト失敗を出し分けられる。

さらに `js-rendering` 経路は chromium subprocess を起動するため、中断時に `browser.close()` を呼ばずに即終了すると Helper/Renderer プロセスが orphan する (issue #121)。よって中断時は in-flight 処理を一定時間 drain してから終了する必要がある。

scout は SIGINT→130 / SIGTERM→143 (POSIX 128+signo) を返し、`SHUTDOWN_DRAIN_TIMEOUT = 7s` で in-flight command を drain するが、この signal→exit-code 対応と graceful drain 方針が ADR として記録されていない。

## Decision Drivers

- shell/orchestration は中断種別を終了コードで区別したい (130 で retry、143 で timeout 失敗)
- chromium subprocess を orphan させず graceful に閉じる必要がある (issue #121)
- drain は無限 hang を避けつつ cleanup に十分な時間を与える必要がある
- sysexits.h 系コード (ADR-0002) と整合する POSIX 拡張であること

## Considered Options

- Option A: signal を 128+signo へ写像 + cancel 通知 + 上限付き drain 後に終了 (採用)
- Option B: signal 受信で即 `process::exit`、drain 無し
- Option C: cancel 通知 + 無制限 drain (timeout 無し)

## Decision Outcome

Chosen option: Option A。SIGINT は 130、SIGTERM は 143 (128+signo の POSIX 規約) へ写像する。中断を受けると cancel handle を通知し、in-flight command を `SHUTDOWN_DRAIN_TIMEOUT = 7s` まで await してから signal 写像の終了コードを返す。7s は CDP `browser.close()` の内部 timeout (5s) に chromium subprocess cleanup の余白を足した値で、graceful path には十分かつ呼び出し側が hang と感じない範囲に収める。signal-vs-command の配線は signal source を注入する `drive` 関数に切り出し、実 OS signal を起動せずユニットテスト可能にする (issue #228)。

Option B は chromium subprocess を ppid=1 へ orphan させ OS reaper に cleanup を委ねるため却下。Option C は subtask が stuck した場合に無限 hang するため却下。

### Consequences

- Good, because shell が SIGINT (130) と SIGTERM (143) を区別し retry 戦略を変えられる
- Good, because chromium subprocess が `browser.close()` で graceful に閉じ orphan を避ける
- Good, because 上限付き drain が無限 hang を防ぎつつ cleanup の時間を与える
- Good, because signal source 注入により実 OS signal 無しで `drive` をユニットテストできる
- Bad, because 非 Unix (Windows) は SIGINT→130 のみで SIGTERM は `#[cfg(unix)]` 限定
- Bad, because 7s はヒューリスティックで、cleanup が 7s を超えると command future が drop され subprocess は OS reaper に残りうる
- Bad, because command 内の subtask が I/O で hang すると、drain timeout までは待つが handler 側 cancel check に到達していない場合がある

### Confirmation

`src/signals.rs` の `[T-S001/T-S002]` が `Sigint.exit_code() == 130` / `Sigterm.exit_code() == 143` を assert する。`src/lib.rs` の `[T-DRV001]` は `start_paused` で 7s を自動進行させ、`drive(pending, ready(SIGINT))` が `Interrupted(Sigint)` (exit 130) を返すこと、`[T-DRV002]` は中断で cancel handle が通知されること、`[T-DRV003]` は command 先行完了時に cancel が通知されないことを検証する。`[T-H000]` は `--help` が sysexits + 130/143 と "Interrupted by SIGINT/SIGTERM" を含むことを assert し、コード追加時の更新漏れを検出する。

## Pros and Cons of the Options

### Option A: 128+signo 写像 + cancel 通知 + 上限付き drain (採用)

signal を POSIX コードへ写像し、cancel 通知後 7s drain してから終了する。

- Good, because 標準的な終了コードと graceful な subprocess cleanup を両立する
- Good, because drain が上限付きで hang しない
- Bad, because 7s 超過 cleanup は subprocess を残しうる

### Option B: signal で即終了

drain せず `process::exit(130/143)`。

- Good, because 実装が最も単純
- Bad, because chromium subprocess を orphan させる

### Option C: 無制限 drain

cancel 通知後、command を無制限に await。

- Good, because graceful path を必ず待ち切る
- Bad, because subtask stuck で無限 hang する

## More Information

### signal → exit code (一次ソース src/signals.rs:3-23)

| signal  | signo | exit code | 算出                      |
| ------- | ----- | --------- | ------------------------- |
| SIGINT  | 2     | 130       | 128 + 2                   |
| SIGTERM | 15    | 143       | 128 + 15 (`#[cfg(unix)]`) |

`wait_for_signal` は Unix で `ctrl_c()` (SIGINT) と `signal(SignalKind::terminate())` (SIGTERM) を race し、SIGTERM install 失敗時は warn して SIGINT のみで継続する。

### drain (src/lib.rs:128-152)

`const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(7)` (issue #121)。`drive` が `tokio::select!` で command と signal を race し、signal 側に倒れたら `cancel.send(true)` で通知 → `timeout(7s, &mut cmd_fut)` で drain → `Outcome::Interrupted(sig)` を返す。drain timeout 時は warn を出し command future を drop する。

drain 対象は主に CDP browser 操作 (`fetch_with_cdp`: cancel で navigate loop を抜け `browser.close()` → subprocess reap) と research の fetch loop。

### 参照

- `src/signals.rs:3-101` (signal 写像 + テスト)
- `src/lib.rs:128-152` (drain orchestration `drive`)、`:262-476` (テスト)
- ADR-0002 (sysexits.h 終了コード規約。130/143 はその POSIX 拡張)
- `docs/audit/2026-06-24-020601-adr-gaps.md` (本 ADR の根拠 audit、候補 #9)
