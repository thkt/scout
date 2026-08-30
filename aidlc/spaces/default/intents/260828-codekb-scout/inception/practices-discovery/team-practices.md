# Team Practices — scout

このリポジトリが実際にどう回っているかを、証跡から書き起こしたもの。`aidlc/spaces/default/memory/team.md` の 5 見出しが空だったため、これは再走査ではなく最初の記録である。org.md が持つフレームワーク既定と重なる項目でも、ここに書くのは scout 自身の証跡から確認できた事実だけで、既定へ寄せて書いてはいない。既定と一致する箇所・乖離する箇所のどちらも `evidence.md` に根拠を残す。3 名の support agent (quality/developer/devsecops) の指摘と、人間の 3 件の確認回答を統合した版である。

## Way of Working

- trunk-based。`git branch -a` は `main` と作業中のブランチ 1 本しか持たず、長命ブランチは存在しない。
- squash-merge が現在の実践である。`main` に届いた最後の通常マージコミットは PR #353 (`05ae191`、2026-08-07) で、それ以降は直近でサンプルした 40 コミットすべてが `(#NNN)` で終わる単一コミットになっている (マージコミットを挟まない)。`merged:>=2026-08-07` で絞った PR 73 件・243 コミット (`gh pr list --state merged --search 'merged:>=2026-08-07'`) でも squash 済みであることを確認しており、40 コミットのサンプルより広い範囲で裏が取れている。#353 より前は `git log --oneline --merges main` が 216 件のマージコミットを返すので、squash-merge は途中で採用された運用変更であり、プロジェクト発足時からの一貫した実践ではない。
- `main` に GitHub 側のブランチ保護は設定されていない (`gh api repos/thkt/scout/branches/main/protection` は `404 Branch not protected` を返す)。したがって下の CI ゲートは GitHub がマージを機械的に止める仕組みではなく、CI ジョブ自身の終了コードが落ちるという形で効いている。
- コミット件名は Conventional Commits の type prefix (`chore`/`docs`/`refactor`/`fix`/`test`/`feat`/`ci`) を持ち、本文は日本語で書かれる。直近 100 件のうち type が判定できたものの内訳は chore 14、docs 13、refactor 6、fix 6、test 4、feat 1、ci 1 (`git log --oneline -100` を type prefix で正規表現走査)。
- CI は `.github/workflows/ci.yml` が持ち、`main` への push と `main` 向け PR の両方で起動する。3 job・27 step (`test` 15 分、`coverage` 20 分 PR のみ、`security` 10 分)。
- `.github/workflows/` は 4 本である。`ci.yml`・`release.yml`・`zizmor.yml` に加えて `label-from-issue.yml` があり、issue の `opened`/`edited` で起動して third-party action 2 本 (`stefanbuck/github-issue-parser`、`redhat-plumbers-in-action/advanced-issue-labeler`) が issue 本文を解析し `issues: write` で label を付ける。scout は公開リポジトリなので issue 本文は誰でも書け、この workflow が「書き込み権限」と「信頼できない入力」が出会う唯一の箇所になる。危険度は限定的 (`pull_request_target` 不使用、信頼できないコードの checkout・実行なし、両 action とも SHA pin 済み、影響は label 付与に閉じる) だが、変更を加える際はこの前提を崩さないことを確認する。
- GitHub Actions のワークフロー自体にも様式がある: 全 action を commit SHA で pin する、checkout に `persist-credentials: false` を付ける、`permissions:` を最小化する。ただしどちらも無条件ではない。`persist-credentials: false` は `release.yml` の `update-homebrew` job の checkout には付いていない — `thkt/homebrew-tap` へ `git push` するために `HOMEBREW_TAP_TOKEN` を保持する必要があり、この逸脱は `.github/zizmor.yml` の `artipacked` ignore (`release.yml:122`、理由コメント付き) に明記されている。`.github/zizmor.yml` は他に `cache-poisoning` (`release.yml:47`)・`superfluous-actions` (`release.yml:109`) の ignore も持ち、いずれも理由コメント付きである。`permissions:` の最小化も「job ごと」ではなく「workflow か job のいずれかで明示する」が実態で、`label-from-issue.yml` は workflow レベルにしか置いていない (4 本中 1 本)。`zizmor.yml` はこれらを `main` への push/PR ごとに検査するが、finding が出たときに CI ジョブ自体を落とすのか、code scanning alert に留まるのかはこのセッションでは確認できていない (`evidence.md` の残る不確実性)。
- GitHub の secret scanning と push protection は両方 enabled で (`gh api repos/thkt/scout` の `security_and_analysis`)、これはリポジトリ内の CI ではなく push そのものを機械的に止める、CI とは別系統の防御である。ただし対象は provider 登録パターンの secret に限られ (`secret_scanning_non_provider_patterns: disabled`)、設定自体もリポジトリ内のファイルではなく GitHub 側のリポジトリ設定にあるため、コミットを介さず無効化でき、ファイル走査では検出できない。`.pre-commit-config.yaml`・`.husky/` は無く、CI にも gitleaks 系の step は無いので、secret 検出はこの GitHub 側 1 枚だけが担っている。

## Walking Skeleton

**この intent では使わない。** アクティブスコープ `classic` (`.claude/scopes/aidlc-classic.md`) は `skeleton: on` を宣言するが、scout は既に 6 サブコマンドが動いている brownfield CLI で、繋ぐべき部品は既に繋がっている。この点を人間に確認し (Q1)、**A. 使わない** の回答を得て `classic` の宣言を明示的に上書きした。最初の Bolt もソロ・ゲート付きにはせず、通常の Bolt として走らせる。

## Testing Posture

- **Methodology**: tdd
- **Ordering**: 各対象について先に失敗するテストを書き、それを通す実装をしてからコミットする

上記 2 行は人間の確認 (Q2、回答 B) によるチームの今後の意図であり、`main` の履歴が実際に示す傾向 (実装コミットがテストコミットに先行する) とは一致しない。この不一致は測定ミスではなく、**チームが計測結果を見た上で選んだ実践の変更**である。判定方法・PR 単位の内訳・区別できないケースは `evidence.md` の「Ordering」節にまとめてある。

以下は上記 2 フィールドを置き換えない追加の事実。

- テストランナーは `cargo-nextest`。`.config/nextest.toml` に `default` と `ci` の 2 プロファイルがあり、`ci` プロファイルは `fail-fast = false`、`slow-timeout = { period = "120s", terminate-after = 2 }`、`final-status-level = "flaky"` (再試行で通ったテストも flaky として CI ログに出す) を持つ。ただしこの設定がいつ発火するか (再試行の回数自体) は `evidence.md` に残す未確認事項がある。
- テスト量: `[T-<PREFIX><NNN>]` 形式のテスト ID が重複なしで 806、`#[test]`/`#[tokio::test]` 系の属性宣言が 851 (内訳 `#[test]` 646、`#[tokio::test]` 191、`#[tokio::test(start_paused = true)]` 13、`#[tokio::test(flavor = "multi_thread")]` 1)。commit `c8460b5`/v2.6.0/測定日 2026-08-29 の値 (`code-quality-assessment.md` の `## テスト`)。ID の採番規約は `src/test_support.rs` の crate doc が持つ (`code-structure.md` の `## コードパターンと規約`)。
- カバレッジゲートは絶対値ではなく差分カバレッジで、**PR イベントでのみ走る** (`coverage` job は `if: github.event_name == 'pull_request'`)。実行するコマンドは次の 2 本で、`-- --include-ignored` と `--exclude` を含めた完全形である。

  ```
  cargo llvm-cov --features js-rendering --lcov --output-path lcov.info -- --include-ignored
  diff-cover lcov.info --compare-branch=origin/main \
    --exclude '*/fetch/cdp/proxy/transport.rs' --fail-under=95
  ```

  `-- --include-ignored` は DR-0024 の Decision Outcome が名指しで要求するフラグで、これを欠くと `#[ignore]` 付きの full-tunnel 経路が `lcov.info` から静かに消え、diff-cover の判定対象から外れる。`--exclude` の対象は実ソケット障害でしか通らないエラーアーム 1 本で、除外理由は `ci.yml` のコメントと当該ファイルの module doc の両方に書かれている。`test` job と `security` job には PR 限定の条件が無く `main` への push でも走るため、**`main` へ直接 push した変更は fmt・clippy・テスト・supply chain 検査を受けるが、95% の差分カバレッジ判定だけは受けない。**

- 28 本の Decision Record (MADR v4、全て `status: "accepted"`) はいずれも Confirmation 節でその決定を pin しているテスト ID を名指しする。索引は `docs/decisions/README.md`。
- `#[ignore]` は全体で 1 本 (`src/fetch/cdp/cdp_integration_tests.rs`) だけで、CI は `--run-ignored all` を付けて必ず実行する。CI は `SCOUT_NETWORK_TESTS: "1"` を job 外の `env:` で立てており、loopback bind に失敗するテストがローカルでは skip でも CI では fail するようにしている (DR-0024)。
- 時間・HTTP・ログの test double は `tokio` の `test-util` (`start_paused`)、`wiremock`、`tracing-test` の `logs_contain` に統一されている (`technology-stack.md` の dev 依存表)。**この上に立つテスト設計そのものは DR-0008 が定める。** `Scout` は `Arc<dyn Clock>`/`Arc<dyn Rng>`/`Arc<dyn TokenSource>` を保持し、本番は `ScoutBuilder::from_env()`、テストは `ScoutBuilder::for_test()` から入る。`for_test()` は `SCOUT_*` env を一切読まないので、開発機の環境変数が無関係なテストを落とさない。assertion は「スロットが繋がったか」(`T-SB001`〜`T-SB003`、`Arc::ptr_eq`) と「繋がった物が振る舞うか」(`T-SB004`) に分かれており、新しい依存を足すときはこの 2 段の assertion に合わせる。
- テストファイルの分割単位は行数ではなく「関心」で決める (`.claude/rules/CONVENTIONS.md`)。実例は `src/fetch/cdp/launch/` の 4 ファイル分割 (`code-structure.md` の `## ファイル分類`)。ただし `.claude/rules/CONVENTIONS.md` は `.gitignore` の `.claude/` で除外され `git ls-files` に現れない untracked なファイルで、この分割規則を CI が検査することもない。実践されている事実ではあるが、強制されている規則ではない。
- テスト ID の一意性・要件コードの不在は、lint の警告ではなくこのリポジトリ自身のテスト (`T-SUP009`/`T-SUP016`) が実ツリーを走査して assert する形で強制されている。規約違反はスイートの赤として現れるので、Code Generation が規約から外れた場合もこの経路で気付ける。具体的な一致条件は `discovered-rules.md` の `## Forbidden`/`## Mandated` を見る。

## Deployment

- scout は単一 crate・単一デプロイ単位の Rust CLI である。`Cargo.toml` に `[workspace]` セクションは無く、`Cargo.lock` の `[[package]] name = "scout"` は 1 件 (`architecture.md` の `## アーキテクチャスタイル`)。永続ストア・キャッシュ層・設定ファイルのいずれも持たず、外部 API への要求はすべて GET (`business-overview.md` の `## ドメイン境界`)。org.md の「staging へマージで deploy、production は手動承認」という既定はデプロイ先の環境が無い scout には当てはまらない。
- ここでの「deploy」は GitHub Release のタグ切りを指す。`v*` タグを push すると `.github/workflows/release.yml` が起動し、4 ターゲット (`x86_64`/`aarch64` × `apple-darwin`/`unknown-linux-gnu`、`js-rendering` feature を焼き込んでビルド) のクロスビルド → `softprops/action-gh-release` が release note 自動生成付きで公開 → 別 job が `thkt/homebrew-tap` (token 経由) を checkout し、テンプレートから `Formula/scout.rb` を再生成して commit・push する、という 3 段の自動化が走る。
- タグは `v0.3.0` (2026-03-13) から `v2.6.0` (2026-08-18) まで 24 件あり (`git tag --sort=-creatordate`)、間隔は 1 日 (`v2.3.0` → `v2.3.1`) から約 5 週間 (`v2.2.0` → `v2.2.1`) まで幅がある。バッチでまとめてリリースするのではなく、随時に近い頻度で出している。
- **タグ push は人が手で行う。** リポジトリ内に `release-please` のような自動 tag 生成の仕組みは無く、`chore(release): bump version to 2.6.0 (#455)` のようなバージョンアップコミットがタグに先行することは確認できていたが、タグ自体を打つ操作の主体はリポジトリ内の証跡だけでは判定できなかった。人間に確認し (Q3、回答 A)、人が `git tag` して push する運用であることを確定した。

## Code Style

- フォーマッタは `rustfmt` の既定設定 (`rustfmt.toml` は存在しない)。`test` job の `cargo fmt -- --check` が強制する。
- リンタは `cargo clippy --all-targets -- -D warnings` を通常 feature と `--all-features` の 2 回走らせる。deny の一覧は `Cargo.toml` の `[lints.clippy]`/`[lints.rust]` と `clippy.toml` が持つので、ここには写さない — `.claude/rules/CONVENTIONS.md` 自身が「一次ソースの内容を写さない。写した時点で 2 箇所が食い違う」と定めており、lint を 1 本足すたびにここが古くなるのを避ける。禁止された read の代替関数名は `clippy.toml` の各 `reason` に書いてあり、`reason` を読めば置き換え先が分かる。
- コメント・doc comment は英語のみで、CI の `Comment language check` が `src/`・`tests/` の `.rs` にある `//` 行コメントと `///` doc comment を機械的に判定する。例外はバイト列注釈内の引用断片 1 種類だけ (`// "テスト" in Shift_JIS` の形)。**テスト関数名と assertion message (`assert!(cond, "...")`) も英語で書く規約だが、この 2 形は `//` を含まないため上記 CI の判定対象には入らない。** 規約としてのみ効き、`.claude/rules/CONVENTIONS.md` (untracked) が明文を持つ。
- DR がコードを指すときはシンボル名で指し、行番号は使わない (DR-0028)。行番号参照から書き換えた履歴が `a1f7707`/`ca745a3`/`340e793`/`5ac8a38` などに残っている。**これを検査する CI job は無い** (`.github/workflows/` に `docs/decisions` を参照する行は 0 件)。自動検査を持たない、文書化された規約として扱う。
- doc comment は「何をするか」ではなく「なぜこの値か」「なぜこの順序か」「何を却下したか、それを測った数値」を書く文化がある。実例が `code-structure.md` の `### doc コメントが却下を残す` に多数ある。
- 命名は Rust の慣用 (snake_case/CamelCase を項目種別ごとに使い分け) に従う。慣用の外側にある規約が 2 つある。**同じクラスの失敗を報告するどのバックエンドも、呼び出し側へ同一の `next_step` 文字列を渡すために名前付き定数を使う** (`src/classify.rs` の `HINT_RETRY_DELAY`/`HINT_CHECK_NETWORK`)。**役割が分かれる関数は役割を名前に入れる** (`src/body_limit.rs` の `read_body_capped` (ペイロード用) と `read_body_snippet` (診断用) — `clippy.toml` の各 `reason` が用途で置き換え先を指し分けられるのはこの命名があるため)。
- GitHub Actions のワークフロー自体の様式 (SHA pin、`persist-credentials: false`、`permissions:` の最小化、zizmor の役割) は `## Way of Working` に記録する。これは Rust コードのスタイルではなくサプライチェーンの防御であり、判断の性質が異なる。

**エラーハンドリング** — `src/classify.rs`、`src/envelope.rs`、`src/tools/errors.rs`、`src/fetch.rs`、`src/github/errors.rs` から確認した、どのツールも強制しない設計規約。Code Generation がバックエンドを 1 つ足すときに従う。

- 分類はバックエンドのエラー enum 自身の `classify() -> Classification` に置き、`From` impl には置かない。`src/tools/errors.rs` の `From<GitHubError>`/`From<FetchError>`/`From<SlackError>`/`From<BraveError>` は 4 本とも `e.to_string()` と `e.classify()` を `ScoutError::from_classification` へ渡すだけの同形になっている。
- **未分類の transport 失敗は `Unknown` (104) であって `TempFailure` ではない。** `Classification::from_reqwest` は timeout でも transient network でもない `reqwest::Error` を `ErrorCode::Unknown` へ落とす。`Unknown` 率の上昇が ADR-0011 の求める「分類の取りこぼし」信号であり、正体不明の transport 失敗を retryable と呼ぶとその信号が埋まる。「よく分からないネットワークエラーだから retryable にしておく」という反射がこのコードベースが禁じている形で、`[T-ER033]` が全バックエンドについてこれを pin する。
- `Classification::from_reqwest` のアーム順は入れ替えられない。timeout 判定が `is_transient_network` より先に来る — `is_transient_network` は timeout にも true を返すため、ADR-0002 が timeout を別コード (124) へ分けている。理由は doc コメントに書かれている。
- HTTP status からコードへの写像は `Classification::from_http_status` の 1 表だけが持つ。バックエンド固有の先行アームは `next_step` の hint を足すためのもので、コード自体を変える先行アームは DR-0003 が doc コメントでの明示を要求する。
- `retryable` を手で書かない。`ScoutError::new` が `kind.is_retryable()` から導出し、`bare_error_line` も同じ経路を通る。`[T-W006]` が呼び出し側での再記述を落とす。
- 部分失敗はエラーではなく degraded な成功として返す。`src/envelope.rs` の `Degradation` はフィールドを非公開にし `push` を唯一の変更手段にすることで `(notes[i], reasons[i])` の対応を保証する。新しい variant 固有の label は `unwrap_or_degraded` (`src/tools/errors.rs`) の経路を通らなければ出力に現れず、汎用の `"resource"` アームへ落ちる点に注意する。

**共有する定数とヘルパーの置き場** — `src/body_limit.rs` の module doc が「2 つ以上のバックエンドが共有する cap やヘルパーはここに置く。1 つのバックエンドだけが使う cap はそのバックエンドに残す」と定める。`read_body_capped` は 4 バックエンド共有なのでここにあり、`MAX_GITHUB_RESPONSE_BYTES`/`MAX_RESPONSE_BYTES` は単一バックエンド専用なので `src/github.rs`/`src/fetch.rs` に残る。

**外部依存は `Arc<dyn Trait>` フィールドで注入する** — `src/tools.rs` の `Scout` が `clock`/`rng`/`token_source`/`dns` をこの形で持ち、`ScoutBuilder` の `with_<dep>` 系メソッドが差し替え口になる (DR-0008、DR-0009)。新しい外部依存を足すときはコンストラクタで直接作らず、この形に合わせる。

**依存するフィールドは呼び出し側に書かせず導出する** — `ScoutError::new` の `retryable`、`CommandOutput::with_degradation` の `degraded`、`DegradedReason::label` がいずれも導出コンストラクタを 1 本だけ出す形になっている。「呼び出し側が両方を書けるとずれる」という理由がそれぞれの doc コメントにある。

**lint 抑制は `#[expect(..., reason = "...")]` を使い、`#[allow]` は使わない。** `src/` の抑制は `#[expect]` 8 箇所のみで `#[allow]` は 0 箇所 (`tests/common/mod.rs` の `#![allow(dead_code)]` はファイル全体に掛かる inner attribute で唯一の例外)。`allow` は不要になっても黙って残るが、`expect` は不要になった時点でビルドが落ちる — `src/slack/client.rs` の `DummyBody` の doc コメントがこの選択理由を書いている。

**可視性は「届く範囲だけ」を毎回選ぶ。** `unreachable_pub` が落とすのは到達不能な `pub` だけで、`pub(super)` で足りる箇所に `pub(crate)` を付けても lint は通る。段階の実例は `pub(crate)`、`pub(super)` (`src/tools/errors.rs` の `ScoutError::user_error` など)、`pub(in crate::fetch::cdp)` (`src/fetch/cdp/proxy.rs` の `spawn_ssrf_proxy` の再輸出) の 3 形。決め方の手順と注意点は `docs/audit/2026-08-11-rust-code-assessment.md` の B-6 が持つ。

**テスト専用の import は `#[cfg(test)]` で囲う。** `src/tools.rs` に例がある。本番コードは使わず、同モジュール内の兄弟テストファイルが `use super::*` 経由で届くための形で、兄弟テストファイル方式 (`#[cfg(test)] mod <name>_tests;`) を採る限り繰り返し必要になる。

**レイヤの向き — 弱い形でのみ記録する。** `tools` がバックエンドへディスパッチし、逆向きの import を持たない。それ以上強い「循環が無い」「横断リーフからバックエンドへの import も無い」という形の主張はここには書かない — `src/yaml.rs` が `crate::search::engine::MAX_PAGE_BYTES` を import し `src/search/engine.rs` が `crate::yaml::truncate_and_reneutralize` を import する、意図された双方向の参照が実在する (`evidence.md` を見る)。

**チームが自分に課している、ツールが検査しない制約が 3 つある。** いずれも DR に記録されているが、強制点は code review であって CI ではない。

- 全ての fetch 経路で SSRF 防御を必須とする (`.claude/OUTCOME.md` の `## Constraints`、DR-0001)。DR-0001 自身が「型強制ではないので契約違反は code review にのみ依存する」と Consequences に書いている。
- 新しい secret を扱うときは `Redacted` に載せる (DR-0015)。既存の Slack/GitHub の 2 経路は型と named test で pin されているが、3 本目以降の secret を止める仕組みは無い。
- 新しい出力経路は中和の境界関数へ通す (DR-0014 の Confirmation 末尾)。既存の層は named test で pin されているが、新しい出力経路そのものを検出する仕組みは無い。
