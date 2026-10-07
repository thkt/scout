---
status: "proposed"
date: 2026-10-08
---

# Future破棄時にもブラウザgroupを停止する

## Context and Problem Statement

[Issue #482](https://github.com/thkt/scout/issues/482)は、外側fetch timeoutや
終了時のFuture破棄でブラウザの子孫と一時profileが残る問題を扱う。
親Issueは[#478](https://github.com/thkt/scout/issues/478)。方式選択は実装担当へ
委ねられているが、この記録の保存を人の設計承認とは扱わない。

監査版`388db1f13c741e04a0babe5d8a1b9ce06242ad4d`と実装開始版
`67ed30f29e60f3bcde64fe75e71416caa16759cd`（ともにv2.6.1）の
`fetch_with_cdp_with`、`spawn_chromium_pgroup`、DR-0017に差分はない。
Issueの偽chromiumによる残留観測は監査時の証拠であり、実Chromeでの残留数は
未測定である。今回の検証結果として読み替えない。

[DR-0017](0017-signal-exit-codes-and-graceful-drain.md)はacceptedで、7秒のdrainを
超えた際の子孫残留を受容している。本件はその清掃上の限界を改善する後続判断である。
元のaccepted本文と過去の結果は保持し、signalの写像と有限のdrainは維持する。
[DR-0019](0019-env-var-validation-and-timeout-hierarchy.md)のtimeout値域と
[DR-0021](0021-cdp-chromium-launch-egress-flags.md)の起動・egressフラグは変更しない。

## Decision Drivers

- 清掃責任をspawn直後からDevTools URL待ち、接続、描画、終了待ちまで保持する。
- Runtime終了時にも、追加タスクの実行や無制限の待機に清掃を依存させない。
- 正常終了と内部timeoutでは既存のgraceful closeと親のbounded reapを維持する。

## Considered Options

- process group、Child、一時profileを一つの所有ガードへまとめ、Dropで同期的にgroupを停止する。
- Future破棄時に非同期の清掃タスクを起動する。
- 外側timeoutやdrain上限を延ばし、明示的な完了経路へ到達するまで待つ。

## Decision Outcome

実装には最初の案を選んだ。`ChromiumProcess`は、spawnから次のawaitまでの間に
groupの所有権を取得し、stderr取得失敗も含めて保持する。`reap`は従来どおり
SIGTERM、50msの猶予、SIGKILL、最大2秒の親waitを行う。清掃中のawaitで破棄されても
DropがSIGKILLを送る。SIGKILL成功またはESRCHでgroupの再送を無効にし、親reap後の
数値PIDの再利用へ古いsignalを送らない。一時profileはその所有ガードが保持し、
Dropのsignal送信より後に削除する。

CDP handlerとrequest interceptorも既存の`AbortOnDrop`で保持し、Future破棄時の
タスク切離しを防ぐ。proxyのabortガードはブラウザ所有ガードより後に破棄される。
正常経路では従来のabortとawaitによるタスク結果の確認を続ける。

非同期清掃だけでは、CLI終了時にRuntimeが停止するとそのタスクの実行を保証できない。
上限延長だけでは任意のFuture破棄を守れず、無制限drainはIssueの制約に反する。

### Consequences

- 外側fetch timeoutと7秒drainの打ち切りにも、所有groupへの同期的な停止処理を持てる。
- Dropではgraceful closeや親waitの完了を待たない。通常経路の5秒close、50ms猶予、
  2秒wait、外側7秒drainの上限と124/130/143は変えない。
- SIGKILLはOSへ停止を要求する操作であり、他のprocess groupへ移ったプロセスや
  uninterruptibleなkernel待機、scout自体へのSIGKILL、signal送信やprofile削除の
  OSエラーまで保証しない。通常のreapでは親を待ち、破棄時の親回収はTokio Childの
  kill-on-dropとOSに依存する。残留確認では実行不能のzombieと生存プロセスを区別する。
- 実Chromeのtimeout後の残留数、各環境での実行時間・不安定さは未測定である。

### Confirmation

`tests/exit_code_contract/browser_cleanup.rs`はローカルHTTP proxyと一時PATHの偽ブラウザを使い、
実際の`Scout::fetch`の外側timeoutとOSのSIGINT/SIGTERMを通す。
ブラウザの起動済みprofileと生きた子孫、実際のgroup所属を確認してから、
終了コードと当該PID・profileの清掃を確認する。signalはDevTools未通知の状態で送り、
7秒drainの打ち切りを通す。fixtureとscoutの所有ガードは、失敗時にも記録したgroup、
profile、CLI子プロセスを後始末する。

`cdp_integration_tests`は内部DevTools timeoutとstderr EOFの完了経路を確認する。
内部timeoutはOS資源の準備完了後にTokioの仮想時刻を進める。
`signal_drain_completion_cleans_browser_group_and_profile`は、実際の`drive`へ
取消通知でreapを完了するcommandを注入し、両signalでdrainの完了と清掃を確認する。
このcommandはCDP通信を行わず、ブラウザ所有ガードとdriveの組合せを対象とする。
`cdp_launch_tests::browser_owner_cleans_interrupted_reap`は、TERM猶予中の
Future破棄を確認する。通常reapの検証はdrain完了ケースへまとめて重複を避ける。
TERMを無視する子孫を用いるため、
親だけのkillやTERMだけの清掃へ退行した場合も検出する。

既存のdrive・signal・終了コード・JSON・SSRF検証は再利用する。実Chromeの既存
ignoredテストは、内容の描画とその起動のprofile削除を一回のrenderで検証する。
profileの全体snapshotを起動引数の記録へ変更し、並行する別テストのprofileを
残留と誤認しない。別の起動が残したprofileを検出する条件は失うが、このrenderが
所有するprofileの検出を維持する。実Chromeの子孫残留を検出するテストではない。

これらの追加は、既存のexit code検証だけでは検出できない子孫残留を観測するためで、
件数やcoverageを増やす目的ではない。signalケースは実際の7秒drainをそれぞれ待つ
費用がある。CLIの追加は既存の終了コードtest binaryに組み込み、共通helperの
単体テストを新しいbinaryでもう一度実行する重複を作らない。内部timeoutの60秒を
実時間で待つテストは追加しない。全体の実行時間の改善や安定性は、実測前に主張しない。既存7群の整理は本件へ混ぜない。

## Verification and Review Handoff

実行契約は既存のdefault/all-featuresのfmt・clippy・nextestで、all-featuresでは
`--run-ignored all`を指定する。captureは不要。sandboxではブラウザとサーバーを
起動しないため、実行成功・失敗・ignoredの件数はホストの記録で確認する。
変更したREADME両言語版、この記録、索引を、コードとともに既存の独立評価へ含める。

設定済みcheckは修正後のテストだけを実行する。Issueが求める「修正前に失敗」の
実行証拠は別途必要である。ホスト担当AIは開始commitの隔離した比較用コピーへ
`tests/exit_code_contract.rs`、`tests/exit_code_contract/browser_cleanup.rs`、
`src/test_support/browser_fixture.rs`を同じ版で配置し、既存の`tests/common/mod.rs`を
使い、次を実行する（現checkoutのコードは戻さない）。

```sh
SCOUT_NETWORK_TESTS=1 cargo test --all-features --test exit_code_contract \
  browser_cleanup::outer_fetch_timeout_reaps_browser_descendant_and_profile -- --exact --nocapture
```

期待する修正前の証拠は、proxy接続とブラウザ・子孫の準備確認、124でのCLI終了、
所有子孫残留のassert失敗、fixtureによる最終清掃である。同じ入力とテスト版で
修正後は通ることを設定済みcheckで確認する。生ログはホストrunへ保持し、
PR説明にはコマンド、対象版、feature、成否、未確認条件を記す。初回実装段階では
この修正前の実行証拠は未取得だった。以下のホスト比較で補い、静的な原因確認とは区別する。

先行#481のローカル最終版`d5ac13252c9f0f002a0d8f23144a938304cb1cc1`
（[PR #487](https://github.com/thkt/scout/pull/487)）の変更ファイルとtest IDを照合した。
その既存findings・assessments・handoffは別のMarkdown打ち切り問題を対象とし、
本件のブラウザ清掃の根拠へ流用しない。DR0029・0030・0031と既知のtest IDは
再利用していない。本件はmain向けの独立差分で、未マージの先行変更を含まない。

## Host verification (2026-10-08)

開始commit `67ed30f29e60f3bcde64fe75e71416caa16759cd` をGit archiveから隔離コピーし、上記3テストファイルだけを現在版と同一の内容で配置した。CDP実装・Cargo設定・lockfileは開始版と一致し、テスト3ファイルのSHA-256一致も確認した。現在の修正コードは戻していない。

all-features・`SCOUT_NETWORK_TESTS=1`・Rust 1.99.0で、上記単一CLIテストを修正前と修正後へ実行した（依存は取得済みで`--offline`、独立したCARGO_TARGET_DIRを使用）。修正前はcargo終了101、0成功・1失敗・0ignored。proxy到達、生きた子孫とgroup所属、CLI終了124のassertを通過し、`owned browser or descendant survived cleanup` で失敗した。これは期待する負の対照で、テスト成功には読み替えない。profile削除のassertは残留失敗より後のため、修正前は到達していない。fixtureの後始末失敗はログに記録されていない。

同一テスト版を未commitの修正後checkoutへ実行すると、cargo終了0、1成功・0失敗・0ignoredとなり、124、実行可能な所有PIDの停止、当該profileの削除まで通過した。zombieはfixtureの既存観測条件に従って生存プロセスと区別する。生ログ4件と対象・fixture hashはホスト検証記録へ保持する。

測定後の変更は本記録の説明更新のみで、実装・テストは変更していない。比較結果は当該外側timeoutの偽ブラウザ経路に適用する。全体チェック・独立評価・実Chromeのtimeout残留計測の成功を代用しない。通常checkと新しいrunの独立評価で、残る経路とこの証拠の適用条件を確認する。

## Host check repair (2026-10-08)

上節は比較測定直後の履歴として保持する。続くホストcheckはfmtと
default/all-featuresのclippyを通過したが、default nextestは856件中855成功・
1失敗・0 skippedで停止した。失敗は`test_support::tests::scanning_src_and_tests_finds_no_violations`
による新規6件の数字始まりIDの拒否であり、all-features nextestには到達していない。
原因はIssue番号をtest IDのprefixに使い、`src/test_support.rs`冒頭にある
「対象を表すprefixと、そのprefix内で一意の番号」という既存規則へ照合しなかったこと。
allowlistと検査規則は変更せず、browser group cleanupを表すBGCへ修正した。

旧IDと現在IDの対応は、T-482-LAUNCH1 → T-BGC001、T-482-CDP1 → T-BGC002、
T-482-CDP2 → T-BGC003、T-482-CDP3 → T-BGC004、T-482-CLI1 → T-BGC005、
T-482-CLI2 → T-BGC006である。関数名・assert・fixture・製品コードは変更していない。
既存のT-MD038・T-GF048・T-SE023とDR0029～0031も再利用していない。

Rust 1.99.0（Cargo.tomlの最小版1.98.1以上）で、
`CARGO_TARGET_DIR=/private/tmp/scout-482-repair-target cargo test --offline --lib test_support::tests::scanning_src_and_tests_finds_no_violations -- --exact`
を実行し、default構成で1成功・0失敗・0ignored（801 filtered out）を確認した。
この既存検査はfeatureの有効・無効によらず`src/`と`tests/`の全ファイルを読むため、
今回の6箇所すべてと既存IDとの重複を検査する。ブラウザ・サーバーは起動していない。
最初の既定targetへの実行はbuild lockのsandbox権限エラーで終了101となり、テスト未実行。
checkoutのtargetが指すホスト領域を変更せず、書込み可能な一時targetへ切り替えて上記検査を通した。
`cargo fmt -- --check`と`git diff --check`も成功した。全体checkの代用とは扱わない。

### Assessments（ID修正時点の履歴、独立受入ではない）

- 根拠と対象版: 差分基準は`67ed30f29e60f3bcde64fe75e71416caa16759cd`。
  [DR-0017](0017-signal-exit-codes-and-graceful-drain.md)、
  [DR-0019](0019-env-var-validation-and-timeout-hierarchy.md)、
  [DR-0021](0021-cdp-chromium-launch-egress-flags.md)のaccepted本文は監査版・開始版・
  現在版で一致する。7秒drain、timeout値域、egressフラグは合意済みの制約として保持し、
  DR0032のproposed保存は人の設計承認と扱わない。
- 前回停止の未解決事項（修正前の回帰失敗証拠）:
  上節「Host verification」の比較結果と、内部ホスト記録の生ログ5件のhashを照合した。
  対象の回帰検証は[CLIテスト](../../tests/exit_code_contract/browser_cleanup.rs)の
  `outer_fetch_timeout_reaps_browser_descendant_and_profile`（T-BGC005）である。
  このリンクは検証定義を指し、生ログの公開先ではない。
  修正前の終了101はfixture準備・CLI124の後の子孫残留assertに起因し、修正後は
  profile削除まで成功している。比較用コピーの3テストファイルのhashは測定記録と一致。
  ID修正時点のCLIテストはIDコメント2箇所を正規化すると測定版とbyte単位で一致し、
  当時は他の2ファイルも無変更だった。この証拠は外側timeout回帰の比較に十分で、追加の修正前実行は不要。
  修正前のprofile assertは未到達であり、その清掃失敗を観測したとは言わない。
- 成果物の変化: 初回停止snapshotとの差は比較結果追記前には本DRだけで、
  ホストcheck直前snapshotと修正開始時のcheckoutは全ファイル一致した。
  今回は新規テストのIDコメント6箇所と本DRの再評価・引き継ぎを更新する。
  内部ホスト記録の初回findingsは当時の未実施条件を説明する履歴で、
  現在のcheck成功や受入へ引き継がない。未実施だった比較の条件と結果は上節に残す。
- Tests: 所有子孫が残る退行は終了コード単独では検出できないため、実CLI外側timeout・
  両signalのdrain打ち切りと、内部timeout・EOF・drain完了・reap中の破棄の検証を維持する。
  正常reapはdrain完了ケースに統合済みであり、今回さらにテストを追加・削除しない。
  ID検査も再発する採番違反と重複参照を検出する既存検証として維持する。
  両signalの打ち切りには各7秒の実待機とOSプロセスの準備費用があるが、実際の配線と
  清掃の保証に必要である。時間短縮・安定性・保守費用の改善は測定していない。
  ID変更で失う動作検出条件はない。初回変更で実Chromeのprofile全体snapshot検査を
  当該起動のprofile記録へ置き換えたため、他の起動が残したprofileを拾う条件は失うが、
  このrenderのprofile削除は維持する。実Chromeのtimeout子孫残留は依然未測定。
- 未完了の評価: ID修正時点ではホストの`reviewHistory`は空で、独立評価の既存受入はなかった。
  内部ホスト記録のcheck-1失敗ログは、[既存ID検査](../../src/test_support.rs)の
  `scanning_src_and_tests_finds_no_violations`によるdefaultのID違反を明示した。
  追加証拠の`passed`も全体の受入を意味しない。
  全体checkと残る清掃経路の実行証拠は現時点では不十分（needs_changes）であり、
  修正後の設定済みcheckと独立評価で更新する。README両言語版・索引・本DRの
  説明は清掃実装・終了コード・既存accepted DRへ照合し、最終評価にも含める。

## Independent review repair (2026-10-08)

### Assessments（修正担当、独立受入ではない）

独立評価対象`8fb80fffc12e8c08f573c05e2b66c580bbedbf96009c742421d14fd0a86d9b6a`の
check-2は、fmt・default/all-features clippyを通過し、default nextestは856成功・
0失敗・0 skipped、all-features nextest（`--run-ignored all`）は871成功・0失敗・
0 skippedだった。既存の実Chrome検証は描画と当該profile削除まで通った。
これはR1-1/R1-2修正前の実行記録であり、以下の修正後のcheck成功や独立受入を代用しない。

- R1-1: [fixture](../../src/test_support/browser_fixture.rs)の`FakeBrowser::wait_ready`と
  `FakeBrowser::assert_clean`は、ready作成前に一度だけ書かれるgroup・descendantの
  記録を各検証境界で一度ずつ読み、局所変数を共有するよう修正した。
  工程をまたぐキャッシュは持たず、`ps`による生存・所属の再観測、5秒の準備期限、
  3秒の清掃期限、profileの事後検査、Dropの独立した読取りと失敗時清掃は維持する。
  変更の目的は不変な入力の反復open・read・parseを除くこと。
  残留を検出するassertとOS観測は変えず、失う動作検出条件はない。
  実装をなぞる読取り回数のテストは追加せず、既存の清掃回帰を維持する。
  実行時間・不安定さ・保守費用の改善量は未測定である。
- R1-2: このDRから一時ファイルへの絶対リンクを除き、
  [Issue #482](https://github.com/thkt/scout/issues/482)、リポジトリ内のテスト定義、
  accepted DRへ辿れる参照を使う。生ログ・hash・初回findings・評価記録は内部記録へ
  保持する。対象版、比較条件、負・正の結果、修正前のprofile assert未到達という
  限界は上節に残す。検証定義へのリンクを実行証拠や公開済みログとは扱わない。

比較測定からはIDコメントに加え、今回のfixtureの局所変数への変更があるため、
現在fixtureを測定版とbyte単位で同じとは扱わない。偽ブラウザの生成script、記録するPID、
生存・group所属の観測、残留とprofileのassert、失敗時Drop、製品コードは変更していない。
その条件の照合により、負の対照は同じ残留退行を検出する根拠として利用できる。
現在版の正の結果は設定済みcheckで更新する。古いcheckだけでは修正後の証拠は
不十分（needs_changes）であり、R1-1/R1-2の独立再評価を残す。

実CLIの外側timeout・両signal打ち切りは、終了コードだけでは見逃す子孫残留を検出する。
内部timeout・EOF・drain完了・reap中の破棄も別の終了条件を守るため維持する。
通常reapの検証はdrain完了へ統合済みで、今回のテスト追加・統合・削除はない。
両signalの打ち切りは各7秒の待機が必要で、check-2では合計14.568秒だった。
既存実Chrome検証の変更で失う他起動のprofile検出条件と、実Chromeの外側timeout後の
子孫残留数が未測定という限界は維持する。通常描画の成功から残留清掃を推定しない。

今回のsandbox検証はRust 1.99.0で`cargo fmt -- --check`、
書込み可能なtargetを使った`cargo clippy --offline --all-targets --all-features -- -D warnings`、
`git diff --check`が成功した。DRの相対リンク先もリポジトリ内で実在を確認した。
これは意味の正しさや公開の保証ではない。ブラウザ・サーバーを伴う検証は実行していない。

### Handoff

対象は同じ開始commitに未commit差分を適用したcheckout。前run・比較生ログを変更せず、
設定済みのfmt、default/all-features clippy、`SCOUT_NETWORK_TESTS=1`のdefault nextest、
all-features nextest（`--run-ignored all`）をホストで再実行する。captureはnullのままで、
Issueは画像・動画を要求していない。sandboxではブラウザとサーバーを起動しない。
設定済みcheckは今回残る全経路と既存のCLI・SSRF・JSON等の検証を実行するため、
契約外の追加ホスト検証は要求しない。実Chromeignoredテストの成否と外側timeoutの
残留未測定は分けて記録する。check失敗は修正へ戻し、成功後も独立評価担当が
現行差分、比較証拠の適用範囲、Testsの価値判断、変更文書を評価する。
R1-1/R1-2を現在版の新しい証拠で個別に再評価し、内部記録の生ログ参照を辿る。
旧check-2成功と評価対象IDを修正後の受入へ引き継がない。
PR説明へfeature構成・成功/失敗/ignored・対象版・未確認条件を引き継ぐ。
今回commit・push・公開は行わない。
