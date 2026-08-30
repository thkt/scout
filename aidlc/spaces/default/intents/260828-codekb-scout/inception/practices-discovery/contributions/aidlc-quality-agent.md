**Collaborator:** aidlc-quality-agent

## Contribution

### 1. Ordering の証跡は残っている。2 つの窓を別々に測った

**`evidence.md` の「実装とテストのどちらが先に書かれるかを判定する手がかりが構造的に失われている」は成り立たない。** 理由は 2 つある。

第 1 に、squash-merge は GitHub 側の commit 履歴を消さない。`gh api repos/thkt/scout/pulls/452/commits` は #452 (squash-merge 済み、merged 2026-08-18) の 6 コミットの headline をそのまま返す。ローカルの `main` が単一コミットに畳まれているだけで、元の並びは API 1 回で取れる。

第 2 に、#353 以前のマージコミット 216 件はローカルにトポロジーを持つ。`evidence.md` は「遡れば限定的なデータは得られる可能性があるが、この走査では実施していない」と書いており、実施した結果が以下である。

**2 つの窓は測り方が違うので、合算した数を書いてはならない。** 窓ごとに母数と判定方法を分けて示す。

| 窓                                     | 判定方法                                       | 母数                           | 判別できた PR | 実装コミットが先 | テストコミットが先 |
| -------------------------------------- | ---------------------------------------------- | ------------------------------ | ------------- | ---------------- | ------------------ |
| `main` のマージコミット (〜2026-08-06) | 変更ファイルのパス分類                         | 216 PR (複数コミットの枝は 83) | 9             | 7                | 2                  |
| `merged:>=2026-08-07` の PR            | コミット headline の Conventional Commits type | 73 PR (243 コミット)           | 15            | 13               | 2                  |

判定方法の詳細と、その方法が持つ誤りの向き:

- **前半の窓**: パス分類は `tests/`・`*_tests.rs`・`test_support.rs` をテスト、その他の `src/*.rs` を実装とした。この分類は inline `#[cfg(test)] mod tests` だけを触ったコミットを実装と誤る (該当 19 ファイル、`src/fetch/converter.rs` はテスト 2,146 行を内包する)。**そこで「実装が先」と出た 7 枝を個別に読み、7 枝すべてが実際に兄弟の `*_tests.rs` を後続コミットで足していることを確認した。** 誤分類は 0 件だった。
- **後半の窓**: 型なし headline を最初は無分類にしていたため、「テストが先」が 5 件に膨らんだ。型なしコミットは実装であることが実体だったので (例: #399 の `suppress empty fragment anchors`)、型なしを実装として数え直して 2 件になった。この 2 件が下の「テスト保守 PR」である。
- 前半の窓の残り: 単一コミット枝 133、テストか実装の片方しか触らない枝 50、**両方を同一コミットに載せた枝 24**。
- 後半の窓の残り: 単一コミット PR 36、テストコミットと実装コミットの片方しか持たない PR 22。

**「テストが先」の 4 件は 1 つも Red 先行ではない。** 4 件とも、既に出荷済みのコードに対してテストを足す・整理する PR である。

| PR                                  | 実体                                                                                                                             |
| ----------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| #342 `test/loopback-server-cleanup` | `spawn_accept_loop` などテスト基盤の抽出。実装変更を伴わない                                                                     |
| #318 `test/ssrf-loopback`           | 既存の SSRF 拒否経路に対する e2e テストの追加                                                                                    |
| #402                                | seam テストの重複排除。「実装」は `refactor(fetch): share the article shell the seam tests wrap around` で、テストのための後追い |
| #357                                | テスト ID への prefix 付与。テスト ID 規約の整備そのもの                                                                         |

**したがって主張できることと、できないことは以下で分かれる。**

- **主張できる**: 判別可能な 24 PR (前半 9 + 後半 15) のうち 20 PR で実装コミットがテストコミットに先行する。残り 4 PR はテスト保守 PR で、新しい本番挙動を持たない。**失敗するテストを単独コミットで先に置く cadence は、24 PR のどれにも現れない。**
- **主張できない**:「TDD ではない」とは言えない。Red-Green-Refactor を Green で 1 コミットに畳む書き方は、テストと実装が同一コミットに載る形になる。この形は前半の窓で 24 枝、後半の窓で相当数を占め、**test-after を 1 コミットで書いた場合と区別が付かない。**

**そこで人間への問いを差し替えることを提案する。** `evidence.md` の項目 2 は現在「scout 自身の証跡から見つけた値ではない」で止まっているが、以下に置き換えられる。

> 判別できた 24 PR のうち 20 PR で、実装コミットがそのテストコミットに先行する。残り 4 件はテスト保守 PR である。失敗するテストを単独コミットで先に置く形は 1 件も無い。ただしテストと実装を同一コミットに載せた PR が多数あり、これは Green で畳んだ TDD と 1 コミットの test-after を区別しない。確認したいのは 1 点 — **`Methodology` は `test-after` でよいか。TDD なら「サイクルは Green で 1 コミットに畳む」という運用であることを併せて確認したい。**

`team-practices.md` の `## Testing Posture` の 2 フィールドは現行値 (`**Methodology**: test-after`/`**Ordering**`) のままでよい。**変えるべきなのは値ではなく、その根拠の書き方である** —「org.md の既定を踏襲した暫定値」ではなく「計測が指す方向と一致する値。人間の affirm 待ち」と書ける。

再現手順: 前半の窓は各マージコミット `M` について `git rev-list --reverse "M^1..M^2"` と `git show --pretty=format: --name-only`。後半の窓は `gh pr list --state merged --search 'merged:>=2026-08-07' --limit 120 --json number` と `gh api repos/thkt/scout/pulls/<n>/commits`。

なお `evidence.md` が置いた **MADR の Confirmation 節を test-after の証跡と読むのは誤りだという注意書きは、そのまま残すべきである。** Confirmation は「決定が成立したことをどう確認するか」を書く欄で、書かれた時点が実装の前か後かを示さない。この注記は後続のレビューアが同じ誤読をするのを止める。

### 2. `discovered-rules.md` に欠けている `## Forbidden` 1 行 — 要件コードは `src`/`tests` に書けない

**このリポジトリは `NFR-`/`FR-`/`BR-` で始まる要件コードが `src/` と `tests/` に現れることを、テスト失敗として禁じる。** `src/test_support.rs` の `scan_requirement_code_violations` が実ツリーを走査し、`T-SUP016` がその結果が空であることを assert する。根拠は ADR-0013 の Context にあり、「GitHub 経路のコメントは `BR-001/002/003`・`FR-007/008` という決定コードを参照するが、その定義文書がリポジトリに存在しない」ことが動機である。

**この行を書くときは一致条件を正確に写すこと。緩く書くと過剰に縛り、書かないと Code Generation がスイートを赤にする。** `extract_requirement_codes` の一致条件は次の 4 つをすべて満たす場合だけである。

1. 接頭辞が `NFR-`・`FR-`・`BR-` のいずれか
2. その直後が ASCII 数字ちょうど 3 桁
3. 3 桁の次の文字が数字でない (文字列末尾も可)
4. 接頭辞の直前が ASCII 英字でない

除外は `src/test_support.rs` の 1 ファイルだけで、フルパス比較で判定する (自分自身がコードを列挙するため)。`docs/` は走査対象外なので、DR や監査文書での引用は自由である。

**AI-DLC 側の ID 形との衝突面は狭い。** `verification.md` が定める `FR1`・`FR1.2`・`NFR2`・`US1.3`・`AC1.1.1`・`BR1.1` はハイフンを持たないか 3 桁でないため、いずれも一致しない。`org.md` の preserved tokens が例示する `FR-1` も 1 桁なので一致しない。**一致するのは `FR-018` の形である。** scout の履歴にも実例があり、commit `e5a2883` の subject が `test(cli): cover whitespace-only API key (T-027 / FR-018)` と書いている。

提案する行 (`## Forbidden` へ):

> NEVER `src/` と `tests/` の中に `NFR-`/`FR-`/`BR-` + 数字 3 桁の要件コードを書かない。引用は `docs/` から行う。(`src/test_support.rs` の `scan_requirement_code_violations` と `find_requirement_code_violations`。実ツリーを走査する `T-SUP016` がスイートを落とす。理由は ADR-0013 の Context)

### 3. カバレッジゲートの引用がフラグを 2 つ落としている — DR-0024 が名指しする

`team-practices.md` の `## Testing Posture` はゲートを `cargo llvm-cov --features js-rendering --lcov` の出力を `diff-cover lcov.info --compare-branch=origin/main --fail-under=95` に通す、と書く。**実際のコマンドは 2 つのフラグを追加で持ち、片方は DR がその必要性を明記している。**

`.github/workflows/ci.yml` の `coverage` job:

```
cargo llvm-cov --features js-rendering --lcov --output-path lcov.info -- --include-ignored
diff-cover lcov.info --compare-branch=origin/main \
  --exclude '*/fetch/cdp/proxy/transport.rs' --fail-under=95
```

**`-- --include-ignored` は DR-0024 の Decision Outcome が名指しで要求する。** 同 DR は「後者を欠くと full-tunnel 経路が lcov.info から静かに消え、diff-cover の判定対象から外れる」と書き、Confirmation 節も「`.github/workflows/ci.yml` の js-rendering 実行 2 箇所にフラグが入っていることを確認する」を確認条件に置く。**フラグを落とした形を practices として持ち出すと、後続の作業が黙ってゲートを弱めうる。** `--exclude` は散文では触れられているが、コマンドの引用の側にも入れておきたい。

### 4. 95% ゲートは PR イベントでのみ走り、`main` への直 push には掛からない

`coverage` job は `if: github.event_name == 'pull_request'` を持つ。`test` job と `security` job にはこの条件が無く、`on.push.branches: [main]` で起動する。**したがって `main` へ直接 push した変更は、fmt・clippy・テスト・supply chain 検査を受けるが、差分カバレッジ 95% の判定だけを受けない。** `main` にブランチ保護が無いことは `discovered-rules.md` の注記が既に押さえているので、この 2 つを繋げて書けば「どのゲートがどの経路で外れるか」が 1 箇所で読める。

**org.md との関係も併せて書くべきである。** org.md の `classic` スコープは 80% の line-coverage floor を additive に足し、「Build and Test で弱めてはならない」と定める。scout の 95% diff-coverage はこれより厳しいので、org.md の 80% を「ここまで下げてよい」と読んではならない。一方で 95% ゲートは直 push には届かないため、**どちらのゲートも `main` への直 push を覆わない。** ここは人間が affirm する価値のある点である。

### 5. テストの設計パターンの本体は DR-0008 の DI seam である

`team-practices.md` の `## Testing Posture` は test double を `tokio` の `test-util`・`wiremock`・`tracing-test` の 3 つで挙げる。**これらは道具であって、このリポジトリのテスト設計そのものは DR-0008 が定めている。** DR-0008 は 28 本の DR のうち唯一テスト容易性を主題にした決定である。

| 要素                  | 中身                                                                                                                              |
| --------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| seam の形             | `Scout` が `Arc<dyn Clock>` / `Arc<dyn Rng>` / `Arc<dyn TokenSource>` を保持する                                                  |
| 本番入口 / テスト入口 | `ScoutBuilder::from_env()` / `ScoutBuilder::for_test()`                                                                           |
| テスト実装            | `FixedClock` / `SeededRng` / `StaticTokenSource`                                                                                  |
| 環境からの隔離        | `for_test()` は `SCOUT_*` env を一切読まない。開発機の `SCOUT_MAX_RETRIES=abc` が無関係なテストを落とさない                       |
| assertion の分業      | `T-SB001`/`T-SB002`/`T-SB003` が `Arc::ptr_eq` で「スロットが繋がったか」を、`T-SB004` が「繋がった物が振る舞うか」を assert する |

**このスロット assertion と挙動 assertion を分ける形は、Code Generation が新しい依存を足すときに従うべきパターンである。** `## Testing Posture` へ 1 行足す価値がある。`TokenSource::fetch` が `Pin<Box<dyn Future>>` を返す理由 (object-safety。`async fn in trait` は `Arc<dyn Trait>` にできない) も同 DR にあり、新しい trait を足すときに再導出せずに済む。

### 6. `find_test_id_violations` は 2 つの規則を強制する

`discovered-rules.md` の `## Mandated` はテスト ID の一意性 1 つだけを挙げる。**`find_test_id_violations` はもう 1 つ、数字で始まる ID を落とす規則を持つ。** `DIGIT_LEADING_ALLOWLIST` に載る **15 件** だけが例外で、その 15 件は `201-1` から `201-16` のうち **`201-7` を欠いた** 並びである (連続した範囲ではないので、範囲として写さないこと)。許可リストに無い `201-17` が報告されることを `T-SUP012` が assert している。

一意性の書き方 (「同一 prefix 内で一意」) は直す必要がない。走査が拾うトークンが prefix を含むため、全体一意と prefix 内一意は同じ判定になる。

提案する追記 (`## Mandated` の既存 1 行へ):

> テスト ID は数字で始めてはならない。例外は `src/test_support.rs` の `DIGIT_LEADING_ALLOWLIST` に載る 15 件 (`201-1`〜`201-16` のうち `201-7` を除く) だけである。

### 7. `.claude/rules/CONVENTIONS.md` は version control されていない

`git check-ignore -v` は `.gitignore:3:.claude/` を返し、`git ls-files --error-unmatch` はこのファイルを知らないと答える。**したがって `discovered-rules.md` がこのファイルを裏として挙げる行は、チームで共有された記録に裏付けられていない。**

影響は行ごとに分かれる。

| 行                                                        | 影響                                                                                                    |
| --------------------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| コメント・doc comment・テスト名・assertion message は英語 | **影響なし。** `ci.yml` の `Comment language check` が機械的に落とすので、裏は CI 側で足りている        |
| テストファイルは関心ごとに分ける (行数は基準にしない)     | **裏が無い。** CI の検査は無く、唯一の明文が untracked なファイルにある。制約ではなく好みとして扱うべき |

`team-practices.md` はこの分割規約を `## Testing Posture` と `## Code Style` の 2 箇所で挙げているので、どちらにも「強制機構は無い」と添えたい。`code-structure.md` の `## ファイル分類` が実例 (`src/fetch/cdp/launch/` の 4 分割) を持つので、実践されている事実は残る。

### 8. nextest の `retries` は設定されていない

`team-practices.md` は `final-status-level = "flaky"` を「再試行で通ったテストも flaky として CI ログに出す」と説明する。**この設定の意味は正しいが、再試行を起こす設定がこのリポジトリのどこにも無い。** 確認した範囲は以下で、いずれも `retries` を持たない。

- `.config/nextest.toml` の `[profile.default]` と `[profile.ci]` (全文 10 行)
- `.cargo/` ディレクトリは存在しない
- `Cargo.toml`、`.github/workflows/` に `retries` も `NEXTEST_RETRIES` も無い
- CI の 2 つの `cargo nextest run` は `--retries` を渡さない

`cargo nextest run --help` は `--retries <N>` を `[default: from profile]` と説明する。**profile が値を持たないときに nextest 本体がどの既定値を採るかは、このセッションでは確認できなかった** (`cargo nextest show-config` のサブコマンドは `version` と `test-groups` だけで、profile の実効値を出さない)。既定が 0 なら `final-status-level = "flaky"` は現状では発火しない設定になる。確認するなら、意図的に不安定なテストを 1 本置いて `--profile ci` で走らせるか、nextest 0.9.143 の profile 既定値の文書を読む。**結論を書く前にどちらかを通したい。**

### 9. 一言 — この repo は規約をテスト失敗として強制する

`T-SUP009` (テスト ID 違反ゼロ) と `T-SUP016` (要件コード違反ゼロ) は、リポジトリ自身のテストが実ツリーを走査する形である。**規約違反は lint の警告ではなくスイートの赤として現れる。** Code Generation が規約から外れたときにどこで気付くかが決まるので、`## Testing Posture` に 1 行あると後段が助かる。

## Positions

- AGREE: `evidence.md` が MADR の Confirmation 節を test-after の証跡と読むのは誤りだと明示した点 — Confirmation は決定の成立確認を書く欄で、記述時点が実装の前か後かを示さないので、この注記が後続レビューアの誤読を止める。
- AGREE: Ordering を人間に確認する項目として残した結論 — 計測は「Red を単独コミットで先に置く形は無い」までしか示さず、Green で畳んだ TDD と test-after を区別しないので、意図の確認は依然として必要である。
- AGREE: 95% diff-coverage と org.md の 80% line-coverage を優劣で比べず、測る対象が違うと書いた扱い — 両者は絶対値と差分で対象が異なり、単純比較すると弱い側を許容値と誤読させる。
- OBJECT: `evidence.md` の「実装とテストのどちらが先に書かれるかを判定する手がかりが構造的に失われている」— squash-merge 済み PR のコミット一覧は `gh api repos/thkt/scout/pulls/<n>/commits` が返し (#452 で確認)、#353 以前 216 件のトポロジーはローカルに残る。判別できた 24 PR のうち 20 PR で実装がテストに先行する。
- OBJECT: `## Testing Posture` のカバレッジコマンドの引用 — `-- --include-ignored` と `--exclude '*/fetch/cdp/proxy/transport.rs'` が落ちており、前者は DR-0024 の Decision Outcome が「欠くと full-tunnel 経路が lcov.info から静かに消える」と名指しするフラグである。
- OBJECT: `discovered-rules.md` に要件コード禁止の行が無い — `NFR-`/`FR-`/`BR-` + 数字 3 桁は `src`/`tests` で `T-SUP016` がスイートを落とす制約で、AI-DLC の `FR-018` 形の ID をコードへ書くと直ちに赤になる。
- OBJECT: `## Testing Posture` が DR-0008 の DI seam に触れていない — `ScoutBuilder::for_test()` と `Arc<dyn Trait>` スロットがこのリポジトリのテスト設計の本体で、`wiremock` などの道具はその上に乗る。
- OBJECT: テスト ID の `## Mandated` 行が一意性だけを挙げている — `find_test_id_violations` は数字始まりの ID も落とし、例外は `DIGIT_LEADING_ALLOWLIST` の 15 件に限られる。
- OBJECT: `.claude/rules/CONVENTIONS.md` を裏として扱っている箇所 — このファイルは `.gitignore` の `.claude/` で除外され untracked なので、CI の裏を持たない「テストは関心ごとに分ける」は制約ではなく好みとして書き分けたい。
