# Evidence — Practices Discovery (scout)

## 調査した対象

- `aidlc/spaces/default/codekb/scout/code-structure.md`、`technology-stack.md`、`dependencies.md`、`code-quality-assessment.md`、`architecture.md`、`business-overview.md` (commit `c8460b5`/v2.6.0/測定日 2026-08-29 の CodeKB)
- `.github/workflows/ci.yml`、`release.yml`、`zizmor.yml`、`label-from-issue.yml` (4 本すべて)、`.github/zizmor.yml` (zizmor 自身の設定ファイル。3 件の理由付き ignore を持つ)
- `.claude/rules/CONVENTIONS.md`、`docs/decisions/README.md` (DR 28 本の索引)、および個別に開いた DR (0001, 0002, 0003, 0008, 0009, 0011, 0012, 0013, 0014, 0015, 0018, 0021, 0022, 0023, 0024, 0028)
- `Cargo.toml` (`[lints.clippy]`/`[lints.rust]`/`[features]`)、`clippy.toml`、`deny.toml`、`.config/nextest.toml`。`rustfmt.toml`/`.rustfmt.toml` は不在を確認した
- `src/test_support.rs`、`src/redacted.rs`、`src/classify.rs`、`src/envelope.rs`、`src/tools/errors.rs`、`src/fetch.rs`、`src/github/errors.rs`、`src/body_limit.rs`、`src/tools.rs`、`src/yaml.rs`、`src/search/engine.rs` ほか、各訂正の根拠として個別に開いたソースファイル
- `git log --oneline`、`git log --oneline --merges`、`git branch -a`、`git tag --sort=-creatordate`、`git rev-list --reverse "M^1..M^2"` と `git show --pretty=format: --name-only` (マージコミット単位のファイル分類)
- `gh api repos/thkt/scout/branches/main/protection` (ブランチ保護の直接確認)、`gh api repos/thkt/scout` の `security_and_analysis` (secret scanning の直接確認)、`gh api repos/thkt/scout/pulls/<n>/commits` (squash 済み PR のコミット一覧復元)、`gh pr list --state merged --search 'merged:>=2026-08-07'`
- `.claude/scopes/aidlc-classic.md` (`skeleton:` フラグ)、`aidlc/spaces/default/intents/260828-codekb-scout/aidlc-state.md` (アクティブスコープが `classic` であることの確認)
- `practices-discovery-questions.md` (人間の 3 件の回答)

## 証跡から確認できたこと

`team-practices.md` の 5 節と `discovered-rules.md` に根拠付きで書いた。ここでは再掲せず、そこに書かなかった判断過程だけを残す。

- squash-merge の「現在の実践」判定は、直近 40 コミットのサンプルと「`main` へ届いた最後の通常マージコミットが PR #353 (2026-08-07)」という 1 点測定に加え、`merged:>=2026-08-07` で絞った PR 73 件・243 コミットのサンプルでも squash 済みであることを確認した。#354 から現在までの全 PR を 1 件ずつ確認したわけではないが、40 コミットより広い範囲で同じ結論が出ている。
- リリース間隔 (1 日〜約 5 週間) は `git tag --sort=-creatordate` が返した 24 タグの日付差から機械的に出した値であり、頻度に関する明文化されたポリシーを見つけたわけではない。

## Ordering (テストの前後関係) — 計測とチームの選択

squash-merge は `main` 上のコミット履歴を畳むが、GitHub 側の PR コミット一覧は消えない。`gh api repos/thkt/scout/pulls/<n>/commits` が squash 済み PR (例: #452) の元のコミット並びをそのまま返し、#353 (2026-08-07) 以前の 216 件のマージコミットはローカルにトポロジーを持つ。この 2 つの経路で「実装コミットとテストコミットのどちらが先か」を測定した。

2 つの窓は判定方法も母数も異なるため、合算せず窓ごとに示す。

| 窓                                     | 判定方法                                       | 母数                           | 判別できた PR | 実装が先 | テストが先 |
| -------------------------------------- | ---------------------------------------------- | ------------------------------ | ------------- | -------- | ---------- |
| `main` のマージコミット (〜2026-08-06) | 変更ファイルのパス分類                         | 216 PR (複数コミットの枝は 83) | 9             | 7        | 2          |
| `merged:>=2026-08-07` の PR            | コミット headline の Conventional Commits type | 73 PR (243 コミット)           | 15            | 13       | 2          |

判定方法の限界と、確認して埋めた誤りの向き:

- 前半の窓のパス分類は `tests/`・`*_tests.rs`・`test_support.rs` をテスト、その他の `src/*.rs` を実装とし、inline `#[cfg(test)] mod tests` だけを触ったコミットを実装と誤りうる (該当 19 ファイル)。「実装が先」と出た 7 枝を個別に読み、全て兄弟の `*_tests.rs` を後続コミットで足していることを確認し、誤分類は 0 件だった。
- 後半の窓は型なし headline を最初は無分類にしたため「テストが先」が 5 件に膨らんだが、型なしコミットの実体を確認すると実装だったため数え直して 2 件になった。
- 残りは、前半の窓が単一コミット枝 133・片方しか触らない枝 50・両方を同一コミットに載せた枝 24、後半の窓が単一コミット PR 36・片方しか持たない PR 22。

**「テストが先」と判定された計 4 件は 1 つも Red 先行ではない。** いずれも既に出荷済みのコードに対するテスト追加・整理である (#342 テスト基盤の抽出、#318 既存 SSRF 拒否経路への e2e テスト追加、#402 seam テストの重複排除、#357 テスト ID への prefix 付与)。

主張できることとできないことは次で分かれる。

- **主張できる**: 判別可能な 24 PR (前半 9 + 後半 15) のうち 20 PR で実装コミットがテストコミットに先行する。残り 4 PR はテスト保守 PR で新しい本番挙動を持たない。失敗するテストを単独コミットで先に置く cadence は、24 PR のどれにも現れない。
- **主張できない**:「TDD ではない」とまでは言えない。Red-Green-Refactor を Green で 1 コミットに畳む書き方は、テストと実装が同一コミットに載る形になり、これは前半の窓で 24 枝・後半の窓で相当数を占める。test-after を 1 コミットで書いた場合と、この形は区別が付かない。

再現手順: 前半の窓は各マージコミット `M` について `git rev-list --reverse "M^1..M^2"` と `git show --pretty=format: --name-only`。後半の窓は `gh pr list --state merged --search 'merged:>=2026-08-07' --limit 120 --json number` と `gh api repos/thkt/scout/pulls/<n>/commits`。

**MADR の Confirmation 節を test-after の証跡と読むのは誤りである。** Confirmation は「決定が成立したことをどう確認するか」を書く欄であり、記述された時点が実装の前か後かを示さない。この注記は、DR がテスト ID を名指しする慣行を「テストが後追いである証拠」と誤読することを防ぐために残す。

**人間の選択。** 上記の測定結果を質問文でそのまま提示した上で (`practices-discovery-questions.md` Q2)、人間は **tdd** (先に失敗するテストを書いてから実装する) を選んだ。したがって `team-practices.md` の `**Methodology**: tdd` は、**scout の過去の履歴が示す傾向 (実装コミットがテストコミットに先行する) の記述ではなく、測定結果を見た上でチームが選んだ今後の実践の変更である。** この不一致を smooth に書き換えて過去の傾向に寄せてはならない。

## Walking Skeleton — 確定

`practices-discovery-questions.md` Q1 で人間に確認し、**A. 使わない** の回答を得た。アクティブスコープ `classic` の `skeleton: on` 宣言は、この intent に限り明示的に上書きされる。

## リリースタグ — 確定

`practices-discovery-questions.md` Q3 で人間に確認し、**A. 人が手で `git tag` して push する** の回答を得た。リポジトリ内にタグ push を自動化する仕組み (release-please 等) が無いこと自体は証跡から確認済みで、その先の「誰が/何が打つか」だけが人間確認を要する部分だった。

## architecture.md / dependencies.md の記述との食い違い — 人間へ

`aidlc/spaces/default/codekb/scout/architecture.md` の `## コンポーネント関係` は「循環は無い」「横断リーフ側からバックエンドへの import も無い」と書き、直後に測定範囲を `src/tools.rs` と `src/fetch.rs` の `use crate::…` を読んだ範囲に限ると明記する (`dependencies.md` の `## crate 内のモジュール依存` も同じ主張を持つ)。横断リーフ 12 本の `^use crate::` を直接読むと、この 2 文は測定範囲の外で成り立たない。

`src/yaml.rs` が `crate::search::engine::MAX_PAGE_BYTES` を import し、`src/search/engine.rs` が `crate::yaml::truncate_and_reneutralize` を import する。リーフからバックエンドへの import が実在し、双方向になっている。`src/yaml.rs` の `MAX_FIELD_BYTES` は `MAX_PAGE_BYTES / 10` として定義され、doc コメントが「`search::engine::MAX_PAGE_BYTES` (4,500) から導出。リーフが遭遇する最も厳しい予算だから」と理由を書いており、意図された参照であって取り残しではない。

この discrepancy は practices-discovery の `produces` の範囲外にある `architecture.md`/`dependencies.md` を書き換えることでは解決できない。`team-practices.md` の `## Code Style` に置いたレイヤの向きの記述は、この事実と矛盾しない弱い形 (「`tools` がバックエンドへディスパッチし、逆向きの import を持たない」) に留めてある。`architecture.md`/`dependencies.md` の該当 2 文の訂正は、この intent の後続ステージか別 intent で扱う。

## 要件コードの一致条件 — AI-DLC 側 ID との衝突面

`discovered-rules.md` の `## Forbidden` に書いた要件コード禁止 (`NFR-`/`FR-`/`BR-` + 数字 3 桁) は `src/test_support.rs` の `extract_requirement_codes` が 4 条件全てを満たす場合だけ一致する: (1) 接頭辞が `NFR-`/`FR-`/`BR-` のいずれか、(2) 直後が ASCII 数字ちょうど 3 桁、(3) 3 桁の次が数字でない (文字列末尾も可)、(4) 接頭辞の直前が ASCII 英字でない。

AI-DLC 側の `verification.md` が定める ID 形 (`FR1`・`FR1.2`・`NFR2`・`US1.3`・`AC1.1.1`・`BR1.1`) はハイフンを持たないか 3 桁でないため、いずれも一致しない。`org.md` の preserved tokens が例示する `FR-1` も 1 桁なので一致しない。**一致するのは `FR-018` のようにハイフン + ちょうど 3 桁の形だけである。** scout の履歴にも実例があり、commit `e5a2883` の subject が `test(cli): cover whitespace-only API key (T-027 / FR-018)` と書いている。したがって AI-DLC がこの形式の要件 ID を `src`/`tests` に書く場面が実際に来たときにだけ `T-SUP016` が落ちる。

## nextest の再試行 — 未確認

`team-practices.md` の `final-status-level = "flaky"` は「再試行で通ったテストも flaky として CI ログに出す」という意味自体は正しいが、**再試行 (`retries`) を起こす設定がこのリポジトリのどこにも見つからない。** 確認した範囲: `.config/nextest.toml` の `[profile.default]`/`[profile.ci]` (全文 10 行) に `retries` は無く、`.cargo/` ディレクトリは存在せず、`Cargo.toml`・`.github/workflows/` に `retries`/`NEXTEST_RETRIES` は無く、CI の 2 つの `cargo nextest run` は `--retries` を渡さない。

`cargo nextest run --help` は `--retries <N>` を `[default: from profile]` と説明するが、profile が値を持たないときの既定値はこのセッションでは確認できなかった (`cargo nextest show-config` は `version`/`test-groups` しか持たず profile の実効値を出さない)。既定が 0 なら `final-status-level = "flaky"` は現状発火しない設定になる。**確認するなら**、意図的に不安定なテストを 1 本置いて `--profile ci` で走らせるか、使用中の nextest バージョンの profile 既定値のドキュメントを読む。結論を書く前にどちらかを通す必要がある。

## GitHub Actions ワークフロー・サプライチェーンの残る不確実性

- **zizmor の finding が CI ジョブを落とすかは未検証。** `zizmor-action@…v0.6.2` は threshold 系の input を渡されておらず、`security-events: write` で SARIF を code scanning へ上げる。過去の finding 2 件 (`zizmor/dependabot-cooldown`、severity `warning`、対象 `.github/dependabot.yml`、いずれも `fixed`) は 2026-05-13〜05-18 のもので、直近 15 runs (2026-08-19 以降、すべて `success`) の窓の外にある。確認は zizmor-action の README を読むか、finding を 1 つ意図的に作る PR で取れる。
- **secret scanning のパターン網羅は未確認。** `secret_scanning_non_provider_patterns: disabled` なので任意の高エントロピー文字列は対象外だが、scout が扱う `GITHUB_TOKEN`/`SLACK_TOKEN`/`BRAVE_SEARCH_API_KEY` のうちどれが provider 登録パターンに当たるかは、この走査では確認していない。
- **依存スキャンの起点は人の push (または renovate の version PR) だけ。** `.github/workflows/` に `schedule:`/`cron:` は無く (`grep -rn 'schedule:\|cron:' .github/workflows/` が 0 件)、`cargo audit`/`cargo deny check` は push と PR でのみ走る。変更されていない依存に対して新しく公開された CVE は次の push まで検出されない。`deny.toml` の `[advisories] ignore = []` で waiver は 0 件なので、検出したものは必ず落ちる — 落ちる強さと検出の起点は別の話である。Dependabot security updates は `disabled` で、`.github/dependabot.yml` は commit `94d6ca5` で削除され renovate へ移行済み。
- **pin の非対称。** action は全て SHA pin だが、走らせる検査ツール自身のバージョンは浮動 (`cargo install cargo-deny --locked` の `--locked` は crate 自身の lockfile を固定するだけで cargo-deny のバージョンは固定しない。`cargo-audit`/`cargo-machete` も同様。`dtolnay/rust-toolchain` も action は SHA pin だが `toolchain: stable` は浮動)。欠陥として指摘しているのではなく、「pipeline は完全に pin されている」と後段が読み違えないための記録である。
- **zizmor の ignore が行番号で指している。** `.github/zizmor.yml` の `release.yml:47`/`release.yml:109`/`release.yml:122` は現時点で意図した step に当たることを確認したが (47・109 は `uses:` 行、122 は `- name:` 行)、`release.yml` に行が挿入されれば黙って別の step を指しうる。DR-0028 が行番号参照を禁じた理由と同じ形の劣化が、DR の適用範囲外であるここに残っている。`release.yml` を編集するときの確認項目として記録する。
- **SBOM の生成は無い。** 明文化された方針への違反ではなく、後段のステージが「あるはず」と仮定しないための記録である。

## 測定基準の差

この統合は commit `f92f89c` (2026-08-29) 時点のツリーの上で行っている。引用した CodeKB の数値 (テスト ID 806、DR 28 本など) は commit `c8460b5`/v2.6.0/測定日 2026-08-29 のものである。`f92f89c` は `c8460b5` の 3 コミット後で、差分は AI-DLC 導入と CodeKB 作成に関する `docs`/`chore` コミットのみ (`46b0e48`、`106b255`、`f92f89c`) であり、`src`/`Cargo.toml`/CI 設定への変更は無い。したがって CodeKB の数値をこの統合の基準としてそのまま引用してよい。

## org.md 既定との一致・乖離

- **一致**: squash-merge。org.md の既定と同じ形が、証跡上も 2026-08-07 以降の現在の実践として確認できる (ただし採用時期が途中である点は既定の記述には無い scout 固有の事実)。trunk-based も一致する。
- **乖離 — Deployment**: org.md は「マージで staging へ deploy、production は手動承認」を既定とするが、scout にはデプロイ先の環境が無い。単一バイナリの CLI で、GitHub Release のタグ切りと Homebrew tap 更新が「リリース」に当たる。この既定は書き換えず、`team-practices.md` の `## Deployment` に scout の実際の形だけを書いた。
- **乖離 (人間確認済み) — Walking Skeleton / Testing Posture**: 上記のとおり、いずれも人間の回答で確定した。Walking Skeleton は `classic` の既定を明示的に上書きし、Testing Posture (`tdd`) は測定結果とは逆方向をチームが選んだ。
- **参考 — カバレッジ**: org.md の既定は 80% の line-coverage floor だが、scout の実測ゲートは 95% の diff-coverage (`diff-cover --fail-under=95`、PR イベントのみ)。両者は測る対象 (絶対値 vs 差分) が異なるため単純な優劣比較にはならない。加えて、org.md の 80% floor も scout の 95% diff-coverage ゲートも、どちらも `main` への直 push には届かない — この 1 点は org.md の記述にはなく、scout 固有の運用の穴として記録する。
