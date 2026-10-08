---
status: "proposed"
date: 2026-10-08
---

# Slackのメッセージ境界と打ち切り境界でコードフェンスを閉じる

## Context and Problem Statement

[Issue #492](https://github.com/thkt/scout/issues/492)は、Slack本文の未閉鎖フェンスが次の返信を取り込む問題と、100,000 byteの出力打ち切りが閉鎖フェンスを落として注記をコード内へ取り込む問題の修正を要求する。基準版は`a46fb3ea0c2469b599339136824cd8f690301002`。方式の選択は実装担当に委ねられている。この記録は実装案であり、人による採用を表さない。

[DR-0014](0014-output-injection-defense-for-agent-consumers.md)はacceptedなYAML防御の根拠であり、Slackはコード内も無条件にマーカーを中和する。[DR-0031](0031-close-report-body-code-fences.md)はproposedなresearch/repo-overview境界の記録で、Slackの変更を合意したものではない。その基準版に存在する解析用の正規化を再利用し、Slackのブロック観測は閉鎖行範囲と未閉鎖情報を同時に取得する。両文書の本文や過去の検証結果は変更しない。

Issueの引き継ぎは、返信authorがバッククォートまたはチルダ3個だけでもtimestampとの連結で開始フェンスになること、幅90,000の閉鎖を上限外追加すると出力が約190,000 byteになり得ることを指摘している。本文補完のみ、または無制限な閉鎖追加では要求を満たさない。

[先行PR #494](https://github.com/thkt/scout/pull/494)は別のIssue #491の修正である。Issue #492の実装開始時の指示に従い、予約済みDR-0034を使わずDR-0035を使用する。初回実装時の基準版にはDR-0034が存在せず、当時の対象文書・実装に含めなかった。初回実装時の先行PR取得はsandboxのDNS失敗と代替取得のcache missで未確認だった。後続のR1-2調査で、下記の固定headの文書差分を照合した。初回の共有文書変更はSlack節とDR-0035の索引追加に限定し、未マージの先行PRコードは取り込まなかった。

## #494マージ後の統合（2026-10-08）

ユーザーの競合解消依頼に従い、[mainのマージcommit](https://github.com/thkt/scout/commit/e48677eb1ec2b253d82ccbca2fe7368b30b51942)を履歴を改変しないmergeで取り込む。現在の対象には[DR-0034](0034-explicit-non-html-text-xml-fetch.md)が存在する。以下にあるR1-2の外部文書照合と未取り込みの説明は、初回実装時点の履歴である。

README英日版は自動統合でき、Git競合はDR索引の状態別項目と生成履歴で、番号表は自動統合された。索引の番号表と状態別索引の両方に0034・0035を残し、手動追加の範囲を0035までに更新する。DR-0034の本文とmain由来の製品・テスト変更は維持し、Slackの製品・テストは変更しない。通常fetchはMarkdownだけに出力上限を適用し、SlackはJSONも切断後のmarkdownを使うため、それぞれの契約を区別する。

初回の競合解消でDR-0034の状態別項目を落とし、番号表だけの確認では欠落を検出できなかった。統合後の独立評価で欠落が判明したため、DR-0034の既存項目を復元した。両索引の番号・表題・状態を各DRの本文と照合し、番号表だけで保持を判断しない。この修正後の全checkと独立評価は未完了であり、修正前のcheck成功を流用しない。

統合のための重複テストは追加しない。検出条件の削除はなく、実Slack API・別Markdown実装・任意Markdownへの適合、速度や不安定さの改善は未確認のままである。ホストは取得mainの識別値、未commitのmergeと競合解消、main由来の差分と今回の解消差分を証拠へ保存する。統合後の全checkと独立評価は新しいrunで行い、初回実装の成功を統合版の成功として流用しない。

## Considered Options

- YAML中和後のメッセージを補完し、打ち切り時の閉鎖は本文予算内に置く。
- 全本文を追加のコードブロックで包む。
- 未閉鎖ブロックを常に省略する。
- 閉鎖フェンスを本文予算外に追加する。

## Decision Outcome

`src/slack/format.rs`の`finish_message`は、先に`slack_line_endings`で単独CRをLFへ揃えてから`neutralize_yaml_markers`を適用し、所有済みの結果を`observe_slack_fences`で一度だけ解析し、実閉鎖行の範囲と未閉鎖フェンスの情報を取得する。その観測結果から`normalize_closing_tabs`で閉鎖行を整え、必要な補完を行う。単独CR改行を同byte数のLFへ、終端tabのある閉鎖行だけを同byte数の空白へ変換し、コード本文・通常の文章のtabとCRLFは保持する。正規化のブロック解析には`src/markdown.rs`の既存の`report_parser_input`を使うが、共有report経路の出力や判定は変更しない。閉鎖行の出力変換は観測済みの範囲だけに適用し、変換後の同一入力を再解析しない。未閉鎖の場合だけ必要な改行と同文字・同幅の閉鎖をそのバッファへ追加する。Slackはコード内も含む全行を既に中和しているため、report向けの保守的走査・尾部の再中和・借用結果からの全文コピーは行わない。通常の閉鎖済み本文には余分なフェンスを追加しない。返信authorとtimestampの表示は`reply_label`で改行を空白へ畳み、backslash・バッククォート・チルダをエスケープする。通常の表示名・timestamp表記とfrontmatterのキー・値処理は維持する。表示名の改行は表示上の空白になるが、一次メッセージ本文はこの処理を通さない。

`truncate_slack_output`は、既存と同じUTF-8／行境界の切断後、`observe_slack_fences`で切断結果の閉鎖行範囲と未閉鎖情報を同時に取得する。閉鎖フェンスとその改行の予算を確保するため切断位置を戻した場合だけ、新しい保持範囲を解析する。最終位置の観測結果を出力変換と補完へ使い、同じ範囲を再解析しない。開始行まで戻る必要がある場合はそのブロックを省略する。保持範囲にも同じ単独CR・閉鎖行の終端tab変換を適用する。残した本文と補完フェンスの合計を100,000 byte以下に保ち、その後に既存形式のbyte-count注記を付ける。shownの数値は保持した元入力のbyte数で、合成フェンスは含まない。注記とdegradationの前置きの予算外扱いは維持する。

`src/tools/query.rs`の`fetch_slack`だけがこの切断関数を呼ぶ。借用／所有の区別によるSlackOutputTruncated判定、前置き挿入、JSONのurl/markdown、degradationの理由は既存経路のまま。SSRF、redaction、終了コード、他のfetch/research/repo出力の打ち切りは変更しない。

全本文を包む案は通常のMarkdownをコードに変えてしまう。常時省略は保持可能な一次ソースを失う。上限外の閉鎖追加は極端な幅で出力予算を破る。採用案はパーサー走査と必要なコピーを加え、極端なフェンスではブロックを省略する。実行時間や保守費用の改善は測定しておらず、改善を主張しない。

## Confirmation and Review Handoff

T-SK091はformatterから出た本文をpulldown-cmarkのイベントで読み、バッククォート／チルダ、異なる幅、未閉鎖・正常閉鎖、フェンスだけのauthorと改行を含むauthorで、返信本文・timestamp・後続authorがコード外にあり、元のコードが残ることを確認する。T-SK090は実際の切断関数で閉鎖済みの長いUTF-8本文を切り、注記がコード外にあり、幅90,000／100,001でも補完を含めた本文予算が維持されることを確認する。フェンス本数を成功条件にしない。パーサーは出力解析にも補完にも使うが、テストは補完helperを期待値に使わず、コード／非コードのイベントと原入力を観測する。

基準版の既存formatter／dispatch検証は境界の取り込みを検出しなかった。追加した2件は修正前に返信本文の取り込みと注記の取り込みで失敗し、修正後のformatterテスト10件は成功した。既存T-SK052の長い平文・改行なしの打ち切り検証は維持し、T-SK053だけを長い閉鎖済みコードの入力へ更新して、注記・degradationの前置きのコード外表示とJSONのmarkdown一致も確認する。既存のYAML中和・metadata・正常undegradedの検証は維持する。これらのdispatchテストはサーバー禁止のsandboxでは実行せず、設定済みホストcheckが実行する。

Rust 1.99.0、cargo-nextest 0.9.146で`cargo fmt -- --check`と`cargo clippy --offline --all-targets -- -D warnings`、同じClippyの`--all-features`構成が成功した。`cargo nextest run --offline --lib -E 'test(slack::format::) | test(markdown::) | test(yaml::)' --profile ci`はdefaultで69件成功・738件除外、同じコマンドの`--all-features`では69件成功・756件除外だった。ignoredテストは実行していない。最終テストを基準commitの一時コピーへ適用し、既存切断関数を接続した比較でも、追加2件が同じ取り込み理由で失敗した。formatter境界と打ち切り境界の静的独立評価ではコードの必須指摘はなく、DR索引の表と先行PRのIssue番号の指摘を修正した。文書を含む再評価で両指摘の解消が確認された。

ローカルのfocused検証を全体検証の成功とは扱わない。ホストはfmt、default/all-featuresのClippy、SCOUT_NETWORK_TESTS=1のdefault nextestとall-featuresのignoredを含むnextestを契約どおり実行する。実Slack APIの資格情報・実応答は未検証であり、Issueの条件では不要。captureは不要。初回実装の文書更新はこのDR、DR索引、README英日版のSlack節のみで、当時のDR-0034は外部の番号予約参照だった。マージ後の現行関係は上記の統合節に記す。

### ホストcheck失敗後の修正（2026-10-08）

初回修正に対するホストのcheck-1は、fmtと両feature構成のClippyを通過した後、default nextestで852件中851件成功・1件失敗となった。失敗は`scanning_src_and_tests_finds_no_violations`で、新規formatter境界テストのT-SK089が、基準版から存在する`src/slack/mention/mention_tests.rs`のT-SK089と重複していた。all-features nextestへは進んでいない。上記のfocused成功と静的評価は、この規約違反を検出した全体checkの代わりにはならない。

`src/test_support.rs`はprefix内の番号をファイル間でも一意にする規約を定めている。既存mentionテストのT-SK089を維持し、新規formatterテストだけを未使用のT-SK091へ変更した。本節の現行参照も更新したが、前回の実行では同じテストがT-SK089を名乗っていたことを履歴として残す。入力・assertion・テスト登録・実装・検証設定は変更しておらず、失う検出条件はない。返信と注記の取り込み・巨大幅での予算超過を防ぐ2件を維持し、既存の複合degradationテストに統合した境界検証も維持する。新しいテストやサーバー起動は追加しない。実行時間・不安定さ・保守費用の改善は主張しない。

修正後、`CARGO_TARGET_DIR=/private/tmp/scout-492-target cargo nextest run --offline --lib -E 'test(scanning_src_and_tests_finds_no_) | test(slack::format::)' --profile ci`はdefault構成で17件成功・790件除外だった（run ID `17f77837-fd3b-4e53-b77a-6d80a89c135e`）。fmtと`git diff --check`も成功した。ブラウザー・サーバーは起動していない。all-featuresとdispatchを含む全体checkは変更後のホスト実行に残る。過去の結果をこの修正後の全体成功とは扱わない。


### review-1の必須指摘への修正（2026-10-08）

初回の静的評価では必須指摘なしとされたが、後続評価でR1-1（Slack補完の冗長処理）とR1-2（先行PR文書の未照合）が指摘された。前回評価は過去の記録であり、両指摘を解消した証拠ではない。修正開始時の`finish_message`には、全行YAML中和の直後に`finish_report_body(Fetched).into_owned()`を適用する冗長処理が残っていた。

R1-1は上記の所有バッファへの追加へ局所修正した。report側の`finish_report_body`と、切断位置が変わる`truncate_slack_output`の再解析は変更しない。不要な走査・再中和・コピーの除去はコードで確認できるが、速度差は測定していない。

R1-2の再調査ではscoutによるPR取得がDNS失敗、代替Web取得もcache missとなった。その後、[PR #494](https://github.com/thkt/scout/pull/494)の取得済み記録が識別するhead `02501c8e52c0a146f0082a1d126a5d8905ea9c69`を同じheadのGit文書へ照合した。[基準版からそのheadまでの固定差分](https://github.com/thkt/scout/compare/a46fb3ea0c2469b599339136824cd8f690301002...02501c8e52c0a146f0082a1d126a5d8905ea9c69)で、README英日版、DR索引、外部DR0034の実際の変更を読んだ。単なる番号予約やPR本文の要約だけで照合したものではない。

その版のREADME変更はSlack節の直前の非HTML text/XML節だけであり、Markdownだけに適用する出力上限とJSONの切断前本文を説明している。これは通常fetchの契約で、今回のSlack専用経路ではJSONも切断後markdownを使うため、Slackへ同じ形式別条件を適用しない。今回のSlack節とは変更内容・要求が両立する。DR索引は双方で同じ追加位置と生成履歴の末尾を変更するので、統合時にはDR0034とDR0035を両方保持する必要があるが、番号・参照・要求の意味的衝突は確認しなかった。外部DR0034はproposedで非HTML媒体の判断を扱う。この対象へ文書や未マージ実装を取り込んでいない。照合は取得済みheadに限定し、現在のリモートheadや将来のGit統合成功は保証しない。

既存のT-SK091/T-SK090は返信・注記の取り込み、正常閉鎖本文の破損、巨大幅での予算超過を検出し、YAML・metadata・author検証も維持する。既存の複合degradation検証に統合したJSON一致とコード境界のassertionはそのまま残す。今回テストの追加・削除・移動はなく、失う検出条件はない。純粋な文字列・パーサー検証で外部応答やサーバーの不安定さを増やさず、必要な退行を検出するため維持する。保守費用と実行時間の改善は未測定で主張しない。異なるMarkdown実装、実Slack API、任意のMarkdown入力への一般的保証は未確認。

変更後のdefault限定検証 `CARGO_TARGET_DIR=/private/tmp/scout-492-target cargo nextest run --offline --lib -E 'test(slack::format::)' --profile ci`は15件成功・792件除外（run ID `16c4144b-5ad9-4127-bc6d-89d4b4cf31ef`、nextest実行0.076秒、コンパイル除外）。これは変更後の結果であり、過去の全体check成功を流用していない。全体checkと文書を含む独立再評価は設定済みホストの修正ループで新しい対象版へ実行する。ブラウザー・サーバー、commit・push・公開は実行していない。check/capture契約は変更せず、追加の受入検証や媒体は必要ない。


### review-3の閉鎖判定と実出力の不一致への修正（2026-10-08）

R3-1の評価対象は`c121b1a395bfc8ddb318774c0ff64bb6561ba815f117704e43b376ad8fc78c94`。前回の文書修正とホストcheck成功はその版の証拠として残す。修正開始時の実装は、解析用の終端tab変換を実出力へ反映していなかった。T-SK091/T-SK090へ終端tab付き閉鎖行を追加すると、返信本文・打ち切り注記の取り込みという狙った理由で両テストが失敗した。判定結果が閉鎖済みであることだけでは、未加工の出力境界を保証できなかった。

閉鎖行のtabだけを空白化した初回修正の独立静的評価は、LF開始行の後に単独CRで閉鎖行を区切った本文にも同じ不一致を指摘した。既存T-SK091で返信の取り込みを再現し、`slack_line_endings`を加えた。新しいLF行境界からYAMLマーカーが露出しないよう、メッセージでは改行正規化を全行YAML中和より先に行う。切断後の保持範囲にも改行・実閉鎖行の正規化を適用する。変換は同byte数であり、補完込み予算とshownの意味は変えない。共有reportの出力・判定は変更せず、`src/markdown.rs`の`report_parser_input`をcrate内で再利用可能にするだけである。

既存2テストへ条件を統合し、新しいテストIDやサーバー検証は追加しない。未加工のParserイベントで返信・注記のコード外表示、コード本文のtab、短い偽閉鎖行のtab、通常文章のtab、CRLF、単独CRとYAML中和を確認する。既存の両フェンス文字・幅違い・巨大幅・UTF-8・正常閉鎖の保持と、平文・metadata・author・JSON/degradationの検証は維持する。失う検出条件はない。追加の文字列生成・解析とfixture保守の費用は、既存検証が見逃した具体的な取り込みを防ぐために必要であり、時間・不安定さ・保守費用の改善は測定も主張もしない。

途中のall-features限定実行は69件中68件成功・1件失敗だった。短い偽閉鎖行の保存assertionを5幅の既存fixtureにも誤適用した条件を、対象の4幅開始行との完全一致へ直した。保存assertionは維持した。この失敗や単独CR修正前の成功を、最終版の検証成功とは扱わない。最終の限定検証と独立再評価は以下へ記録する。設定済み全体check・capture契約は変更せず、媒体・契約外の受入検証は不要である。実Slack API、任意入力、異なるMarkdown実装は未確認のままである。

最終実装の限定nextestは、既存の`CARGO_TARGET_DIR=/private/tmp/scout-492-target`で `cargo nextest run --offline --lib -E 'test(slack::format::) | test(markdown::) | test(yaml::)' --profile ci`を実行し、defaultで69件成功・738件除外（run ID `42cf483d-b441-418e-9b69-44f0dc4c8fda`、nextest実行0.276秒、コンパイル除外）、同じコマンドのall-featuresで69件成功・756件除外（run ID `def8671d-fe3a-4209-bf86-f83c67825a48`、0.382秒）だった。ignored・dispatch・全体テストは実行していない。変更後のfmt、defaultの`cargo clippy --offline --all-targets -- -D warnings`、`git diff --check`と、このDRの相対参照先の存在検査は成功した。リンク形式の成功は外部URLの現在の到達性や人の採用を証明しない。

文書を含む独立静的再評価では、追加の必須欠陥は確認されず、終端tabと混在改行の不一致、YAML中和の順序、コードtab・CRLF、予算とJSON/degradationの維持を確認した。評価担当はテストを再実行しておらず、この結果は任意入力での欠陥不存在や受入を表さない。設定済みの全体checkとホスト独立評価は変更後の新しい対象版へ実行する。ブラウザー・サーバー、commit・push・公開は実行していない。


### review-4の重複解析への修正（2026-10-08）

R4-1の評価対象は`67e57576278f7e102ca0b74992e849825479577aaa8a0caad25a58d2db47453d`。前回の修正は実閉鎖行のtabと単独CRを出力へ反映したが、閉鎖行正規化と未閉鎖判定が同じ正規化入力を別々に解析していた。今回の開始時のコードにもその処理が残っており、前回の成功・独立評価は重複の解消を示していなかった。

Slack内の一度のブロック観測で閉鎖行範囲と未閉鎖情報を取得する方式へ変更した。切断位置を戻したときの解析は必要なため維持し、最終位置の結果を変換と補完に共用する。工程間のキャッシュやモード分岐は追加しない。全行YAML中和の順序、本文tab・CRLF、実閉鎖行変換、補完込み予算、注記位置、共有reportの防御は維持する。重複解析の除去はコードから確認できるが、速度差や保守費用の削減は未測定である。

既存T-SK091/T-SK090は返信・注記の取り込み、本文tabの破損、正常閉鎖の破損、巨大幅の予算超過を未加工出力から検出するため維持する。平文・YAML・metadata・authorと複合degradationのJSON一致検証も変更しない。今回は内部の観測結果の共用であり、新しい振る舞いの検証不足は確認していないためテストを追加・削除・移動しない。失う検出条件はない。純粋fixtureの生成・解析費用は具体的な退行検出に必要で、サーバーや外部応答の不安定さは増やさない。異なるMarkdown実装、任意入力、実Slack APIと長期的な保守費用は未確認である。

今回の限定nextestは`CARGO_TARGET_DIR=/private/tmp/scout-492-target cargo nextest run --offline --lib -E 'test(slack::format::)' --profile ci`でdefaultが15件成功・792件除外（run ID `8a85e1e2-3b70-4585-bc34-531d0be7fd6e`）、同じコマンドの`--all-features`で15件成功・810件除外（run ID `d2b91278-1e64-4372-9a00-8ad30dbc0216`）だった。変更後のfmt、defaultの`cargo clippy --offline --lib -- -D warnings`、`git diff --check`も成功した。今回の対象はSlack内の観測方法であり、共有reportのテストとdispatch・ignored・全体検証は再実行していない。これらの成功は前回版の結果の流用でも独立評価でもない。README英日版の操作説明は現行の出力条件と一致するため、今回の内部変更では更新しない。設定済みホストの全体checkと独立再評価は新しい対象版で必要であり、captureや契約外の受入検証は必要ない。ブラウザー・サーバー、commit・push・公開は実行していない。
