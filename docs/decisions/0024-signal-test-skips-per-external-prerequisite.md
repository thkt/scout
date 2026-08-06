---
status: "accepted"
date: 2026-08-06
decision-makers: thkt (project owner)
---

# Signal Test Skips per External Prerequisite

## Context and Problem Statement

外部前提を欠く host でテストが静かに緑で終わると、実行されなかった検証が実行されたものとして数えられる。loopback bind についてはこの問題を解決済みで、`guard_loopback_bind` (src/test_support.rs) が env `SCOUT_NETWORK_TESTS` の設定時に panic し、`.github/workflows/ci.yml` が workflow レベルでその env を立てている。

chromium を要求する `t005_t006_cdp_renders_and_removes_profile_dir` (src/fetch/cdp/cdp_integration_tests.rs) には同じ仕組みが無く、`chrome_available()` が false のとき `eprintln!` + 早期 return で終わる。このテストは CDP 経路の SOCKS5 full-tunnel 成功経路を通す唯一の自動テストで、OUTCOME の Constraints が定める防御の成功経路にあたる (ADR-0021)。

同じ policy なので同じ機構を複製すればよいはずだが、guard 方式の伝達チャネルは実行時のローカル観測に使えない。tracing subscriber の初期化は `src/lib.rs` の `run()` 内だけで、`src/**` の単体テストは `run()` を通らない。guard がスキップする通常経路 (`guard_loopback_bind` が `None` を返す経路) には subscriber がいないため、`tracing::warn!` の record は誰にも読まれずに消える。一方 `#[traced_test]` を付けたテスト関数の中でだけ `tracing-test` がその場で subscriber を差し込み、`logs_contain` で record を capture して assert できる (`bind_failure_without_force_returns_none_and_warns` がこの経路を pin する)。`eprintln!` は ci.yml のコメント自身が述べるとおり nextest が passing test の stderr を隠す。前提ごとに何で表すかを決める必要がある。

## Decision Drivers

- CI で外部前提が欠けたら赤くする
- 前提を持たない開発機で `cargo test` が失敗しない状態を保つ
- スキップされた事実がローカルでも読める
- policy を表す機構は少ないほどよい

## Considered Options

- 方式 A: `guard_loopback_bind` の形を chromium へ複製し、`SCOUT_NETWORK_TESTS` に相乗りさせる
- 方式 B: `#[ignore = "requires chromium"]` を付け、CI 側で明示実行する (採用)
- 方式 C: CI に chromium を明示インストールし、gate 自体を撤廃する

## Decision Outcome

Chosen option: 方式 B。loopback bind は既存の env var + guard 方式を維持し、chromium は `#[ignore]` + CI の明示実行で表す。同一 policy に 2 機構が並ぶ形を受け入れる。

決め手は観測可能性で、`#[ignore]` だけが nextest サマリの ignored 件数という読み取れる信号を出す。loopback bind 側を `#[ignore]` へ寄せる統一は採らない。bind 失敗は host の状態に依存し、テストの属性として静的に決められないためである。

CI は js-rendering を走らせる 2 箇所を両方直す。`Test (js-rendering)` に `--run-ignored all`、`Generate coverage` に `-- --include-ignored` を付ける。後者を欠くと full-tunnel 経路が lcov.info から静かに消え、diff-cover の判定対象から外れる。

### Consequences

- Good, because ローカルで ignored 件数としてスキップが読め、env var も guard 関数も新設しない
- Good, because coverage job も経路を実行するため、SOCKS5 proxy の唯一の成功経路が diff-cover の対象に残る
- Bad, because 同一 policy に 2 機構が並び、新しい外部前提が現れたときどちらを採るか判断が要る
- Bad, because chromium を持つ開発機でも既定実行から外れ、明示指定が要る
- Bad, because `--run-ignored all` は将来追加される全 `#[ignore]` を CI で走らせる (本 DR 時点で他に 0 件)

### Confirmation

`cargo nextest run --features js-rendering --run-ignored all --profile ci` が対象テストを実行する。chromium を持たない host で `cargo test` を実行すると suite は緑で終わり、対象テストが ignored に数えられる。`.github/workflows/ci.yml` の js-rendering 実行 2 箇所にフラグが入っていることを確認する。

## Pros and Cons of the Options

### 方式 A

`guard_chrome(test_name, resolved, force)` を置き、`SCOUT_NETWORK_TESTS` が設定されていれば panic する。

- Good, because loopback bind と同じ形になり、policy を表す機構が 1 つで済む
- Bad, because ローカルでは観測結果が現状と変わらない。得られるのは CI を赤くする効果だけ
- Bad, because 呼び出し元が 1 箇所しかない抽象化になる (`bind_loopback` は 6 箇所)

### 方式 C

`browser-actions/setup-chrome` 等で CI に chromium をインストールし、gate を撤廃する。

- Good, because runner イメージの同梱に依存しなくなる
- Bad, because third-party action の SHA pin と保守が増え、supply chain の面が広がる
- Bad, because 現時点で解く問題が無い。actions/runner-images の Ubuntu2404 と Ubuntu2604 の Readme はどちらも Google Chrome 150 を Browsers 節に載せている

## More Information

### Trade-offs

policy の表現を 1 つに揃える価値と、ローカルでスキップが読める価値を天秤にかけ、後者を採った。統一を優先すると、chromium 不在の開発機では現状と同じく「緑で終わったが実行されていない」状態が残る。

### Reassessment Triggers

- guard 方式の skip が、実行フォームに関わらず runner のサマリで読める件数として現れるようになる (`#[ignore]` の ignored 件数と同種の信号)。guard 方式でも観測可能になるため、方式 A への統一を再検討する
- `#[ignore]` 付きテストが 2 件以上になり、CI で走らせたくないものが現れる。`--run-ignored all` から filter 指定へ変更する
- runner イメージから Chrome が外れる。方式 C を再検討する

> **Note (2026-08-06, skip 警告の件数表示を削除)**: `bind_failure_without_force_returns_none_and_warns` (`src/test_support.rs`) を読むと、pin しているのは `logs_contain("permission_denied")` というテスト名の一致だけで、件数は assert 対象に入っていない。`NETWORK_SKIP_COUNT` はプロセス内 static なので、nextest が各テストを別プロセスで走らせる既定設定では 1 テストの skip ごとに 1 から数え直され、プロセスをまたいだ累積件数にはならない。件数を warn 文言に載せると runner の実行形態 (プロセス分離の有無、並列度) で値の意味が変わってしまい、テスト名だけを載せる契約とも整合しない。`NETWORK_SKIP_COUNT` の定義・加算と warn/eprintln 文言の件数部分を `src/test_support.rs` と `tests/common/mod.rs` の両方から削除し、上記 Context の tracing::warn! に関する記述と Reassessment Triggers の該当項目をこの実測に合わせて書き換えた。

Related to issue #319 and ADR-0021 (CDP Chromium Launch Egress Flags).
