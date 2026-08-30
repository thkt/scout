**Collaborator:** aidlc-devsecops-agent

## Contribution

lint / SAST / secret・dependency scanning / supply chain の観点で draft 3 本を照合した。訂正 6 件、`discovered-rules.md` への追加 8 行、規則にしない観察 5 件を出す。引いた強制点はすべて自分でファイルか API に当たって確かめた。引用したテスト ID は 37 件を 1 件ずつ実在確認した (実行はしていない)。範囲表記 (`[T-MD001..T-MD037]` など) は両端だけを確認しており、中間の連番は数えていない。うち `T-C032` / `T-C041` の 2 件だけは `tests/output_injection.rs` に `// T-C032: <テスト名>` の非ブラケット形で書かれている。

### 1. ブランチ保護の判定 — 区別は正しい。ただし 1 軸だけ取りこぼしている

404 を根拠に全行を「CI ジョブが落ちる」へ限定した判断を支持する。`discovered-rules.md` 冒頭の宣言と結びの注記が一致しており、強制点の言い換えを許さない書き方になっている。

取りこぼしは 1 つある。**GitHub の secret scanning と push protection が両方 enabled で、これは CI ではなく push そのものを止める機械的ブロックである。**

```
gh api repos/thkt/scout → security_and_analysis
  secret_scanning:                     enabled
  secret_scanning_push_protection:     enabled
  secret_scanning_non_provider_patterns: disabled
  secret_scanning_validity_checks:     disabled
  dependabot_security_updates:         disabled
```

`## 適用範囲についての注記` の結び「その運用自体を強制する仕組みはリポジトリの中には無い」は、文字どおりには正しい。この仕組みはリポジトリの**中**ではなく GitHub のリポジトリ設定にある。しかし読み手には「何も機械的に止まらない」と読める。次の 2 点を両方書けば正確になる。

- 止まる範囲は provider パターンに登録された secret に限る。`non_provider_patterns` が `disabled` なので、任意の高エントロピー文字列は対象外である。scout が扱う `GITHUB_TOKEN` / `SLACK_TOKEN` / `BRAVE_SEARCH_API_KEY` のうちどれが登録パターンに当たるかは、この走査では確認していない。
- この設定はリポジトリ設定側にあり、バージョン管理されない。commit を 1 つも作らずに無効化でき、ファイルを読む走査では検出できない。

補強として: `.pre-commit-config.yaml` も `.husky/` も無く、CI にも gitleaks 系の step は無い。secret 検出の層はこの GitHub 側 1 枚だけで、しかもその 1 枚がリポジトリの外にある。

### 2. `discovered-rules.md` の既存行への訂正

**(a) `persist-credentials: false` を無条件の ALWAYS として書いている。反例がリポジトリ自身の設定にある。**

`.github/zizmor.yml` (workflow ではなく zizmor の設定ファイル) が理由コメント付きの ignore を 3 件持つ。

| rule                  | ignore 対象      | 書かれている理由                                                     |
| --------------------- | ---------------- | -------------------------------------------------------------------- |
| `artipacked`          | `release.yml:122` | update-homebrew job は `git push` のため `HOMEBREW_TAP_TOKEN` の保持が要る |
| `cache-poisoning`     | `release.yml:47`  | release はタグ push でのみ走り、PR 由来の cache 注入経路が無い       |
| `superfluous-actions` | `release.yml:109` | `generate_release_notes` は gh CLI に同等物が無い                     |

`release.yml` の `update-homebrew` job の `Checkout homebrew-tap` step は、`repository: thkt/homebrew-tap` と `token: ${{ secrets.HOMEBREW_TAP_TOKEN }}` を渡し、`persist-credentials: false` を**付けていない**。付けたら後続の `git push` が通らない。

したがってこの行は次の形が正確である。

> ALWAYS GitHub Actions の各 step は commit SHA で pin し、`permissions:` は workflow か job のいずれかで最小限に明示し、checkout には `persist-credentials: false` を付ける。外す必要があるときは `.github/zizmor.yml` に理由コメント付きの ignore を書く。(`.github/workflows/zizmor.yml` が `main` への push/PR ごとに検査する。現行の例外 3 件は `.github/zizmor.yml` にある)

**例外を設定ファイルへ理由付きで書くこと自体が、記録に値する実践である。** Rust 側の `#[expect(..., reason = "...")]` と同じ形が CI 設定にも通っている。

`evidence.md` の調査対象には `.github/workflows/zizmor.yml` はあるが `.github/zizmor.yml` が無い。設定ファイルを読んでいないことが、例外を落とした原因である。

**(b) 「job ごとに `permissions:` を最小化する」は 4 本のうち 1 本に当たらない。**

| workflow              | permissions の置き場所                                       |
| --------------------- | ------------------------------------------------------------ |
| `ci.yml`              | job ごと 3 箇所 (`contents: read`)                            |
| `release.yml`         | job ごと 3 箇所 (`contents: read` ×2、`contents: write` ×1)   |
| `zizmor.yml`          | workflow レベル `{}` + job レベル                             |
| `label-from-issue.yml` | workflow レベルのみ (`contents: read` + `issues: write`)      |

「workflow か job のいずれかで明示する」が実態である。

**(c) workflow は 3 本ではなく 4 本。`evidence.md` の調査対象一覧が `label-from-issue.yml` を落としている。**

`code-quality-assessment.md` の `## CI/CD` 自身が「`.github/workflows/` に 4 本」と書いており、draft は自分が引いた資料と食い違っている。

`label-from-issue.yml` は私の観点では見落としてはいけない 1 本である。**書き込み権限と信頼できない入力が出会う唯一の場所だからである。** `issues: opened/edited` で起動し、公開リポジトリ (`private: false`) なので issue 本文は誰でも書ける。その本文を third-party action 2 つ (`stefanbuck/github-issue-parser`、`redhat-plumbers-in-action/advanced-issue-labeler`) が解析し、`issues: write` と `secrets.GITHUB_TOKEN` を持つ。

危険度は限定的である。`pull_request_target` を使わず、信頼できないコードの checkout も実行もせず、2 つの action はどちらも SHA pin されている。影響範囲は label 付与に閉じる。それでも `team-practices.md` の CI 記述に 1 行足す価値がある。

**(d) `disallowed_methods` の例外の説明が、実在する 4 件と合わない。**

例外は 4 件ある。`src/retry/tests.rs` に 2 件、`src/tools/errors/exit_code_tests.rs` と `src/slack/classify_tests.rs` に各 1 件。すべてテストコードで、すべて `reason` 付き — ここまでは draft のとおり。

合わないのは例外の中身である。4 件の `reason` はいずれも「the decode failure is the fixture, not a body this test wants」と書く。**テストが欲しいのは body ではなく decode 失敗そのもので、上限なし read が失敗することが実験装置になっている。** draft の「自分が応答した本文を読み返すテスト」は `clippy.toml` 冒頭のコメント (`Tests that read a body they themselves served are the exception`) をそのまま写した表現で、想定された例外の類型であって実在する 4 件の説明ではない。

再測定するときの注意: 4 件は属性が複数行に分かれて書かれている。`#[expect(clippy::disallowed_methods` を 1 行で当てる grep は 0 件を返す。`disallowed_methods` の文字列だけで当てること。

**(e) `unreachable_pub` の行が、lint の効果を超えて読める。**

現行の文は「crate の外へ出る項目は `pub async fn run() -> ExitCode` の 1 つだけに保ち」と読める。`unreachable_pub` が落とすのは、crate 外から到達できない `pub` 項目であって、`pub` 項目の**数**ではない。`src/lib.rs` の `pub async fn run() -> ExitCode` が唯一の外部入口であるのは現在の形であり、lint がその数を固定しているのではない。`Cargo.toml` のコメント自身も field をこの lint の対象外と書いている。後半の「モジュール境界を越える項目は届く範囲だけの可視性にする」だけを残すのが正確である。

**(f) `body_limit` の行はこのままでよい。** むしろこの repo で強制点の書き方の手本になっている。`clippy.toml` 冒頭が「なぜテストではなく lint か」を書いている — どちらの caller も cap を観測可能にしないので、`text()` を戻しても全テストが通ってしまう。だから build を落とす。他の行の書き方をここへ寄せるとよい。

### 3. 追加すべき行 — security 制約が 1 行も入っていない

これが 2 つ目の flag への答えである。**draft は lint と CI が落とすものをよく捉えているが、DR が named test で pin している security 制約が 1 行も無い。** テストがスイートを落とすという強制点は、draft が `scan_test_id_violations` に対して既に受け入れた基準と同じである。だから入るべきである。

**先に、入れてはいけないものを名指しする。** `.claude/OUTCOME.md` の `## Constraints` が持つ「全 fetch 経路で SSRF 防御を必須とする」という宣言そのものは、`discovered-rules.md` に入れてはならない。DR-0001 が自分でそう書いている — Consequences に「型強制ではないので contract 違反は code review にのみ依存」、Confirmation は reviewer の確認と「将来追加検討」と付記された CI 行数チェックである。**制約文書は強制点ではない。** これは `team-practices.md` に「チームが自分に課している制約」として書く対象で、`discovered-rules.md` の対象ではない。

同じ形の指示が、私が下で足す DR にもう 2 つある。どちらも今後の書き手へ向けた文で、違反を落とす仕組みを持たない。SSRF 契約と並べて `team-practices.md` へ回す。

- 新しい secret を `Redacted` に載せる (DR-0015。既存の Slack / GitHub 2 経路は pin されているが、将来の 3 本目を止めるものは無い)
- 新しい出力経路を中和の境界関数へ通す (DR-0014 の Confirmation 末尾)

入るのは、DR の Confirmation が named test で pin している具体的な振る舞いだけである。以下は `## Mandated` へそのまま貼れる形にした。

- ALWAYS Direct 経路 (proxy 環境変数なし) では、DNS 事前チェックを通した後も connect 時に接続先 IP を検証し、private IP への接続を落とす。DNS rebind はここで塞ぐ。 (DR-0012。`src/fetch/fetch_page_tests.rs` の `[T-F072]` が、pre-flight に public・connect に private を別 resolver で注入して `"blocked connect to private IP"` の warn を assert する。guard を外すと warn が消えるのでトートロジーにならない)
- ALWAYS Proxied 経路 (`HTTPS_PROXY` / `HTTP_PROXY` 設定時) でも、literal な private/loopback IP と blocked-suffix の拒否は scout 側で全 hop 維持する。委譲してよいのは名前解決に基づく防御だけである。 (DR-0023。`src/fetch/ssrf/egress_tests.rs` の `[T-FS022, T-FS023, T-FS024, T-FS027]` が `detect_egress_mode` の env→mode 写像を pin し、`src/fetch/ssrf/tests.rs` と `src/fetch/fetch_page_tests.rs` が Proxied で literal reject が残ることを pin する)
- ALWAYS DNS resolver は `Arc<dyn DnsResolver>` で注入し、private IP を返す resolver に対して HTTP connect の前に短絡する。 (DR-0009。`src/tools/builder_tests.rs` の `[T-DNS001, T-DNS002]`)
- ALWAYS CDP/chromium 経路は scout が起動する loopback SOCKS5 proxy を経由させ、`--proxy-server` / `--proxy-bypass-list="<-loopback>"` / `--disable-quic` と egress 抑止フラグ群を起動引数に含める。 (DR-0021。`src/fetch/cdp/launch/cdp_launch_tests.rs` の `[T-F043, T-201-8]`)
- ALWAYS その SOCKS5 proxy は検証済み IP のみ dial し、private 宛の CONNECT は REP=`0x02` で fail-closed にする。 (DR-0012 の Addendum。`src/fetch/cdp/proxy/proxy_tests.rs` の `[T-201-1, T-201-4]`。guard を削ると REP=`0x01` へ変わり log も消える)
- ALWAYS `Redacted` に `Display` と `Serialize` の impl を足さない。Slack と GitHub の token は `Redacted` 経由で構築する。 (DR-0015。`src/redacted.rs` の `Redacted` は `impl fmt::Debug` だけを持ち、`Display`/`Serialize` の impl が存在しない。**この 1 件だけは型で強制されていて、`{}` や serde へ渡すとコンパイルが落ちる。** `[T-RD001..T-RD004]` が `Debug` の `[REDACTED]` と空文字/空白の `None` 化を pin し、`src/slack/client/constructor_tests.rs` の `[T-SK033..T-SK035]` と `src/github/http_tests.rs` の `[T-GH018, T-GH019]` が両 token が `Redacted` 経由で構築されることを pin する)
- ALWAYS `gh auth token` の subprocess が非ゼロで終了したとき、stderr をログへ出さず終了コードだけを報告する。 (DR-0018 がこれを SEC 判断と明記する。`src/token_source.rs` の `[T-TOK004]`)
- ALWAYS Slack の user token は構築時に `xoxp-` prefix を検証し、bot token を含む他の形を `TokenWrongType` で拒否する。 (DR-0022。`src/slack/client/constructor_tests.rs` の `[T-SK065, T-SK066]`)
- ALWAYS 出力に載る外部文字列は、消費側 AI エージェントへの注入を中和してから出す。閉じないフェンスは fail-closed に倒す。 (DR-0014。層ごとに pin がある — markdown `src/markdown.rs` の `[T-MD001..T-MD037]`、YAML `src/yaml.rs` の `[T-FC003..T-FC007, T-FC030..T-FC033]`、fetch 経路 `tests/output_injection.rs` の `[T-C032, T-C041]`、Slack `src/slack/format/format_tests.rs` の `[T-SK088]`、search `src/search/engine/tests.rs` の `[T-SE010]`、HTML の `suppressed_handler` `src/fetch/converter.rs` の `[T-FC084..T-FC086]`、GitHub README `src/github/format/overview_tests.rs` の `[T-GF044..T-GF047]`)

最後の 1 行は scout に固有で、汎用の security チェックリストには出てこない。**出力そのものを攻撃面として扱う** — 呼び出し側の AI エージェントが scout の出力を指示と誤読することを防ぐ設計である。practices の記録から落とすと、この repo で最も非自明な制約が消える。

### 4. 規則にしない観察 — 人間へ

以下は強制点を持たないので `discovered-rules.md` へは入れない。`team-practices.md` か `evidence.md` に事実として置くかどうかの判断を返す。

1. **依存スキャンの起点が人の push しかない。** `.github/workflows/` に `schedule:` も `cron:` も無い (`grep -rn 'schedule:\|cron:' .github/workflows/` が 0 件)。`cargo audit` と `cargo deny check` は `main` への push と PR でのみ走る。したがって、変更されていない依存に対して新しく公開された CVE は、次の push まで検出されない。`deny.toml` の `[advisories] ignore = []` で waiver は 0 件なので、検出したものは必ず落ちる。落ちる強さと検出の起点は別の話である。実運用では renovate が version PR を上げ、その PR が security job を回すので、renovate の頻度がスキャンの頻度になっている。Dependabot security updates は `disabled` で、`.github/dependabot.yml` は `94d6ca5` (`chore(deps): migrate Dependabot to Renovate with shared preset`) で削除されている。
2. **pin の非対称。** action は全て SHA pin だが、その中で走らせる検査ツールは浮動である。`cargo install cargo-deny --locked` の `--locked` はその crate 自身の lockfile を固定するだけで、cargo-deny の**バージョン**は固定しない。`cargo-audit` / `cargo-machete` も同じ。`dtolnay/rust-toolchain` も action は SHA pin だが `toolchain: stable` は浮動である。直すべき欠陥として挙げているのではない。「pipeline は完全に pin されている」と後段が読み違えないよう、非対称を記録に残したい。
3. **zizmor の ignore が行番号で指している。** `release.yml:47` / `release.yml:109` / `release.yml:122` の 3 件は、いま意図した step に当たることを確認した (47 と 109 は `uses:` 行、122 は step の `- name:` 行)。`release.yml` に行が挿入されれば黙って別の step を指す。DR-0028 が行番号参照を禁じた理由と同じ形の劣化が、DR の適用範囲外であるここに残っている。zizmor の ignore 構文が `filename:line` なので回避しにくい。`release.yml` を編集するときの確認項目として書いておく価値がある。
4. **zizmor が finding で job を落とすかは未検証。** `zizmorcore/zizmor-action@…v0.6.2` は threshold 系の input を渡されておらず、`security-events: write` で SARIF を code scanning へ上げる。過去の finding 2 件 (`zizmor/dependabot-cooldown`、severity `warning`、対象 `.github/dependabot.yml`) は code scanning alert として残り、どちらも `fixed`。ただしこの 2 件は 2026-05-13〜05-18 で、私がサンプルした直近 15 runs (2026-08-19 以降、すべて `success`) の窓の外にある。**したがって「finding が出れば CI が落ちる」も「落ちない」も、この走査では確定していない。** 2-(a) で書き直した SHA pin の行を `## Mandated` に残すなら、この 1 点を確認してからにしたい。確認は zizmor-action の README か、finding を 1 つ意図的に作った PR で取れる。
5. **SBOM の生成は無い。** 明文化された方針に反しているわけではない。後段のステージが「あるはず」と仮定しないための記録である。

## Positions

- AGREE: ブランチ保護 404 を根拠に全行を「CI ジョブが落ちる」へ限定した判断 — 強制点の言い換えを許さない書き方で、`discovered-rules.md` 冒頭の宣言とも結びの注記とも整合している。
- AGREE: `body_limit` の行 — `clippy.toml` 冒頭が「なぜテストではなく lint か」を書いており、強制点が build 失敗であることが確定している。この repo で最も強い強制の形で、他の行の手本になる。
- AGREE: カバレッジを 95% の diff-coverage として記録し、org.md の 80% line-coverage とは「測る対象が違う」と書いた点 — 単純な優劣比較を避けており正しい。
- AGREE: `deny.toml` を「許可リストに列挙しないことで strong copyleft を拒否する」形で引いた点 — `[licenses] deny` が空であることと整合し、13 件という数も `[licenses] allow` 配列と一致する。
- AGREE: Testing Posture の 2 行を「org.md の既定を仮置きした値であって scout の証跡ではない」と明記して人間確認へ回した点 — 既定を証跡に見せかけない扱いとして正しい。
- OBJECT: `persist-credentials: false` を無条件の ALWAYS として書いている — `.github/zizmor.yml` が `release.yml:122` の `artipacked` ignore を持ち、`update-homebrew` の checkout は `git push` のため token を保持する。
- OBJECT: 「job ごとに `permissions:` を最小化する」— `label-from-issue.yml` は workflow レベルにしか置いておらず、4 本のうち 1 本に当たらない。
- OBJECT: zizmor を SHA pin 行の強制点として引いているが、finding が出たときに job が落ちるかを確認していない — 落ちないなら、この行の強制点は code scanning alert であって CI ではない。
- OBJECT: `## 適用範囲についての注記` の結びが「何も機械的に止まらない」と読める — secret scanning push protection が enabled で、push を止める。ただし provider パターンに限り、設定はリポジトリの外にある。
- OBJECT: SSRF・secret・出力注入の制約が `discovered-rules.md` に 1 行も無い — DR が named test で pin している振る舞いは、draft が `scan_test_id_violations` に適用した基準を満たす。DR-0001 の SSRF 契約など review にしか依存しない 3 つは、逆に入れてはならない。
- OBJECT: `unreachable_pub` の行が「crate 外へ出る `pub` は 1 つだけ」と読める — この lint が落とすのは到達できない `pub` であって `pub` の数ではなく、field は対象外である。
- OBJECT: `disallowed_methods` の例外の説明が実在する 4 件と合わない — 4 件の `reason` は「decode 失敗そのものが実験装置」と書いており、`clippy.toml` の想定類型を写した文とは違う。
- OBJECT: `evidence.md` の調査対象が workflow 3 本と `.github/workflows/zizmor.yml` 止まり — 実際は 4 本で、`.github/zizmor.yml` (例外 3 件を持つ設定ファイル) が抜けている。
