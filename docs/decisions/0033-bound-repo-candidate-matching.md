---
status: "proposed"
date: 2026-10-08
decision-makers: "未確認（Issue #483の範囲内で実装担当が選択。設計の承認待ち）"
---

# repoの候補照合に仕事量上限と中断点を設ける

## 背景と課題

対象はscoutの`repo-read`でContents APIがnot-foundを返した後の候補生成。[Issue #483](https://github.com/thkt/scout/issues/483)と[親Issue #478](https://github.com/thkt/scout/issues/478)が目的・受入条件・制約の正本で、実装方法の選択は実装担当へ委ねられている。この記録は実装案であり、人による設計承認を意味しない。

監査対象v2.6.1 / `388db1f13c741e04a0babe5d8a1b9ce06242ad4d`から開始commit `67ed30f29e60f3bcde64fe75e71416caa16759cd`まで、`src/tools/typo.rs`、`repo.rs`、DR0019には変更がない。距離計算はOSAの全行列で、候補生成の5秒timeout内に同期処理として置かれていた。非同期のtimeoutは、その処理が制御を返すまで期限を再評価できない。

[acceptedなDR0019](0019-env-var-validation-and-timeout-hierarchy.md)の環境変数検証とtimeout階層、および[DR0017](0017-signal-exit-codes-and-graceful-drain.md)の終了要求の契約は維持する。本記録は候補生成中の仕事量と中断点を補う後続判断であり、それらの本文・状態・設定値は変更しない。

## 検討した選択肢

- 全行列OSAに入力量・総計算量の上限と協調的な中断点を加える。
- 距離3の帯だけを計算するOSAへ置き換え、別途tree走査も制限する。計算量は減らせるが、転置の参照行と帯の境界を新たに検証する必要がある。
- 同期処理を`spawn_blocking`へ移す。単に移すだけではtimeout後も無制限の計算が残るため、Issueの制約を満たさない。
- 候補を一律に廃止する。Issueの対象外。

## 実装案と理由

最初の案を実装した。小規模入力の正確な距離と安定順位を既存OSAのまま保ち、候補情報だけを省略できるbest-effort契約を利用する。

- `closest_matches`は対象と候補を最大513 Unicode文字まで読み、512文字超なら候補をすべて省略する。極端に長い文字列全体の走査・確保を避ける。
- 候補poolは4,096件まで。`collect_path_candidates`もblobのfilterより前にraw treeの件数を確認する。ディレクトリだけの巨大treeが同期filterを占有することを防ぐ。
- 距離計算は対象文字数×候補文字数のセル数を、実行前に総予算1,000,000から差し引く。文字数の差だけで距離上限を超える候補には行列計算を行わない。行列は最大513×513要素に収まる。
- OSAの16行ごと、および候補間で`yield_now`を呼ぶ。距離計算の中断点間は最大8,192セルとなり、短いパスや長さで除外する候補でも制御を返す。別taskやblocking workerは作らず、Future破棄で照合も終わる。
- 上限を超えたら、途中までの順位を返さず候補全体を省略する。成功するファイル読み取りへのパス制限ではない。距離3以内、上位3件、同距離でのtree順、元のnot-foundエラーと終了66を保つ。

これらの数値は候補の補助性と計算の有限性を優先した実装上の選択で、実際の大規模リポジトリにおける候補成功率から導いた値ではない。候補fetchの5秒、外側GitHub timeout、終了待ちの上限は変更しない。

### 影響と限界

大きいtree、長い対象や候補、総計算量の多いpoolでは、近いパスがあっても候補を提示しない。JSONでは空の`candidates`を出すのではなく、[DR0010](0010-scout-local-json-envelope-contract.md)に従ってフィールドを省略する。

制御を返す間隔は仕事量で限定する。OSのスケジューリング遅延や、行列確保・treeの受信とJSON解析を含めた厳密な実時間上限を保証するものではない。実CLIで大規模treeが5秒を超える再現、実GitHub APIの動作、速度や不安定さの改善量は未確認。

### 確認方法と今回の観測

- T-TY010は400文字の対象と同じ400文字の候補100件で、7件目までしかpoolを消費せず候補を省略することを確認する。6件の距離計算で960,000セルを使い、次の計算は予算に入らない。
- T-TY012は計算開始のpollが`Pending`になることを確認し、仮想時計を5秒の期限より先へ進めてtimeoutを観測する。pool消費は最初の1件で止まる。短い実時間閾値に依存する速度テストではない。
- T-TY011はUnicode文字数の境界、長さで除外するpoolを含む件数上限、上限超過で部分候補を残さないことを確認する。T-TY006は既存検証を使い、距離0〜3と距離4の除外、上位件数、同距離の順序を具体的な期待値で確認する。単独のT-TY009はここへ統合し、top-Nを2にした場合の保証も保つ。元の`a`〜`abcde`という文字列固有のfixtureはなくなるが、件数制限と順位の検出条件は維持する。
- T-TS040はmock treeを使って`Scout::run(Command::RepoRead)`から実際の候補生成を通す。小規模、400文字×100件、blob1件＋ディレクトリ4,096件を対照に、順位または候補の省略と、JSON・通常エラー本文・終了66を照合する。実GitHub APIは不要。

2026-10-08、Rust 1.99.0で開始commitの同期OSAへT-TY010相当を加えた負の対照は、全100件の消費で失敗した（0成功・1失敗、0 ignored）。同じ同期コードの最初のpollも`Ready(Ok(...))`となり、中断点を要求する対照で失敗した。Issueに記載された1ms timeoutのprobeも同じ400文字×100件で再現し、628.523709ms後に`Ok`を返した（1試行）。この短い閾値は修正前の観測にだけ使い、通常の回帰テストには追加していない。

統合前の`cargo test --offline --lib tools::typo::tests -- --nocapture`および同コマンドの`--all-features`構成は、それぞれ12成功・0失敗・0 ignoredだった。期待値は手で定め、実装の距離計算を使って生成していない。mock serverを必要とするT-TS040はsandboxでは未実行。既存のSSRF、redaction、YAML、出力上限、終了コードの検証は変更していない。

統合後は、開始commitの同期関数に`await`を含まないasyncアダプターだけを付け、現行の同じテストmoduleを`rustc --edition=2024 --test`で実行した。8成功・3失敗・0 ignoredで、T-TY010は100件の消費、T-TY011は513文字でも候補を返すこと、T-TY012は最初のpollで`Ready`になることにより失敗した。元の同期計算へ上限や中断点を加えていない負の対照であり、Cargo全体やrepo経路の修正前実行を示すものではない。修正後の統合版は上記Cargoコマンドのdefault/all-featuresで、それぞれ11成功・0失敗・0 ignored。実行時間の改善や保守費用の削減率は測定していない。

fmt、`cargo clippy --offline --all-targets -- -D warnings`、同コマンドの`--all-features`は成功した。Rust 1.99.0は今回の環境のstableで、CargoのMSRV 1.98.1を満たす。全体nextest、実Chromeのignoredテスト、最終の独立評価はホスト工程に残る。

ホストの設定済みcheckはfmt、default/all-featuresのClippyとnextestを実行し、`SCOUT_NETWORK_TESTS=1`でmock検証の空振りを失敗にする。all-featuresではignoredも実行する。captureはnullで、今回の受入条件には画像・動画を必要としない。変更文書も既存の独立評価に含める。

### 見直しの条件

候補省略が一次ソースへの到達を妨げる実例が確認された場合、候補成功率と仕事量を同じ入力で比較し、帯付きOSAや上限を検討する。上限だけを緩めて無制限の計算へ戻さない。
