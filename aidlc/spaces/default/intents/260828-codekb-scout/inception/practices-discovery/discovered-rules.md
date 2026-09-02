# Discovered Rules — scout

このリポジトリが機械的に強制している制約だけを挙げる。好みや推奨は含めない。各行は次のいずれかで裏を取る: CI が落とす、lint が `deny`/`forbid` になっている、または Decision Record が名指しするテストがその振る舞いを assert している (DR 自身が強制しているのではなく、DR が指すテストが強制する)。裏の所在をすべての行に付ける。

## Mandated

- ALWAYS `src/` と `tests/` の `.rs` に書く `//` 行コメントと `///` doc comment は英語にする。原語が残せるのはバイト列注釈内の引用断片だけである。 (`.github/workflows/ci.yml` の `Comment language check` step。テスト関数名と assertion message はこの判定対象に入らないため、規約としての扱いは `team-practices.md` の `## Code Style` に書く)
- ALWAYS モジュール境界を越えて公開する項目は、実際に crate 外から到達する範囲だけの可視性 (`pub(crate)`/`pub(super)`/`pub(in path)`、真に外部 API なら bare `pub`) にする。 (`Cargo.toml` の `[lints.rust] unreachable_pub = "deny"` が到達不能な `pub` をビルドで落とす。crate の唯一の外部入口 `pub async fn run() -> ExitCode` が保たれているのは、これに加えて `src/lib.rs` の `mod` 宣言がすべて非公開であるためだが、この非公開の維持自体は lint の対象外の実践であり、この行は lint が実際に落とす範囲だけを指す)
- ALWAYS HTTP レスポンスの本文を読むときは `body_limit::read_body_capped` (ペイロード用) か `read_body_snippet` (診断用) を経由し、上限なしで読まない。 (`clippy.toml` の `disallowed-methods` が `reqwest::Response::{text, bytes, json}` の直接呼び出しを deny する。どちらの呼び出し元も cap を観測可能にしないため、テストではなく lint で強制している理由が `clippy.toml` 冒頭に書かれている)
- ALWAYS `js-rendering` feature の chromium 依存テストは、chromium が無いランナーでも skip ではなく fail させる。CI は `--run-ignored all` を付けて必ず実行する。 (`.github/workflows/ci.yml` の step comment。DR-0024)
- ALWAYS `[T-<PREFIX><NNN>]` 形式のテスト ID は同一 prefix 内で一意にする。 (`src/test_support.rs` 自身のテスト `T-SUP009` (`scan_test_id_violations`) が重複を検出してスイートを落とす)
- ALWAYS テスト ID は数字で始めない。例外は `src/test_support.rs` の `DIGIT_LEADING_ALLOWLIST` に載る 15 件 (`201-1`〜`201-16` のうち `201-7` を除く) だけである。 (`find_test_id_violations` がこの規則も判定し、許可リストに無い数字始まり ID (`201-17` など) を `T-SUP012` が検出する)
- ALWAYS Direct 経路 (proxy 環境変数なし) では、DNS 事前チェックを通した後も connect 時に接続先 IP を検証し、private IP への接続を落とす。DNS rebind はここで塞ぐ。 (DR-0012。`src/fetch/fetch_page_tests.rs` の `[T-F072]` が、pre-flight に public・connect に private を別 resolver で注入して `"blocked connect to private IP"` の warn を assert する)
- ALWAYS Proxied 経路 (`HTTPS_PROXY`/`HTTP_PROXY` 設定時) でも、literal な private/loopback IP と blocked-suffix の拒否は scout 側で全 hop 維持する。委譲してよいのは名前解決に基づく防御だけである。 (DR-0023。`src/fetch/ssrf/egress_tests.rs` の `[T-FS022, T-FS023, T-FS024, T-FS027]` が `detect_egress_mode` の env→mode 写像を pin し、`src/fetch/ssrf/tests.rs` と `src/fetch/fetch_page_tests.rs` が Proxied で literal reject が残ることを pin する)
- ALWAYS DNS resolver は `Arc<dyn DnsResolver>` で注入し、private IP を返す resolver に対して HTTP connect の前に短絡する。 (DR-0009。`src/tools/builder_tests.rs` の `[T-DNS001, T-DNS002]`)
- ALWAYS CDP/chromium 経路は scout が起動する loopback SOCKS5 proxy を経由させ、`--proxy-server`/`--proxy-bypass-list="<-loopback>"`/`--disable-quic` と egress 抑止フラグ群を起動引数に含める。 (DR-0021。`src/fetch/cdp/launch/cdp_launch_tests.rs` の `[T-F043, T-201-8]`)
- ALWAYS その SOCKS5 proxy は検証済み IP のみ dial し、private 宛の CONNECT は REP=`0x02` で fail-closed にする。 (DR-0012 の Addendum。`src/fetch/cdp/proxy/proxy_tests.rs` の `[T-201-1, T-201-4]`)
- ALWAYS `Redacted` に `Display` と `Serialize` の impl を足さない。Slack と GitHub の token は `Redacted` 経由で構築する。 (DR-0015。`src/redacted.rs` の `Redacted` は `impl fmt::Debug` だけを持ち、`Display`/`Serialize` の impl は存在しない。`{}` や serde へ渡すとコンパイルが落ちる型レベルの強制。`[T-RD001..T-RD004]` が `Debug` の `[REDACTED]` 化を、`src/slack/client/constructor_tests.rs` の `[T-SK033..T-SK035]` と `src/github/http_tests.rs` の `[T-GH018, T-GH019]` が両 token の構築経路を pin する)
- ALWAYS `gh auth token` の subprocess が非ゼロで終了したとき、stderr をログへ出さず終了コードだけを報告する。 (DR-0018。`src/token_source.rs` の `[T-TOK004]`)
- ALWAYS Slack の user token は構築時に `xoxp-` prefix を検証し、bot token を含む他の形を `TokenWrongType` で拒否する。 (DR-0022。`src/slack/client/constructor_tests.rs` の `[T-SK065, T-SK066]`)
- ALWAYS 出力に載る外部文字列は、消費側 AI エージェントへの注入を中和してから出す。閉じないフェンスは fail-closed に倒す。 (DR-0014。層ごとに pin がある — markdown `src/markdown.rs` の `[T-MD001..T-MD037]`、YAML `src/yaml.rs` の `[T-FC003..T-FC007, T-FC030..T-FC033]`、fetch 経路 `tests/output_injection.rs` の `[T-C032, T-C041]`、Slack `src/slack/format/format_tests.rs` の `[T-SK088]`、search `src/search/engine/tests.rs` の `[T-SE010]`、HTML の `suppressed_handler` `src/fetch/converter.rs` の `[T-FC084..T-FC086]`、GitHub README `src/github/format/overview_tests.rs` の `[T-GF044..T-GF047]`)

## Forbidden

- NEVER `unsafe` コードを書かない。 (`Cargo.toml` の `[lints.rust] unsafe_code = "forbid"`)
- NEVER wildcard import (`use foo::*`) や enum の glob use を書かない。 (`Cargo.toml` の `[lints.clippy] wildcard_imports`/`enum_glob_use = "deny"`)
- NEVER clippy が truncation (`cast_possible_truncation`) や精度損失 (`cast_precision_loss`) と判定するキャストを、指摘に対処せず残さない。 (同上 `[lints.clippy]`)
- NEVER `&str` への `.to_string()` (`str_to_string`)、clippy がメソッド参照で書けると判定するクロージャ (`redundant_closure_for_method_calls`)、または禁じられたイテレータアダプタの形 (`filter_map_next`/`flat_map_option`/`manual_filter_map`/`manual_find_map`) を書かない。 (同上 `[lints.clippy]`)
- NEVER 絶対パスでのモジュール参照 (`absolute_paths`) や、借用で足りる箇所での所有権渡し (`needless_pass_by_value`) を書かない。 (同上 `[lints.clippy]`)
- NEVER `reqwest::Response::text()`/`.bytes()`/`.json()` を直接呼ばない。例外は `#[expect(clippy::disallowed_methods, reason = "...")]` を付けた 4 箇所 (`src/retry/tests.rs` に 2 件、`src/tools/errors/exit_code_tests.rs`、`src/slack/classify_tests.rs` に各 1 件) だけで、いずれもテストコードにある。この 4 件の `reason` は「本文を読み返すため」ではなく「decode 失敗そのものがテストの実験装置であるため」と書いている — 上限なし read が失敗すること自体を検証する目的で、本文を観測したいのではない。 (`clippy.toml` の `disallowed-methods`)
- NEVER `rustfmt` の整形から外れたコードや、`-D warnings` (通常 feature と `--all-features` の両方) で clippy が指摘するコードを提出しない。 (`.github/workflows/ci.yml` の `test` job、`cargo fmt -- --check` と `cargo clippy --all-targets -- -D warnings` の 2 系統)
- NEVER PR の差分カバレッジを 95% 未満に落とさない。例外は CI が明示的に除外する `src/fetch/cdp/proxy/transport.rs` の 1 本だけである。 (`.github/workflows/ci.yml` の `coverage` job、`diff-cover --fail-under=95`。除外理由は同ファイルの module doc にも書かれている。このゲートは `pull_request` イベントでのみ走り、`main` への直 push には掛からない)
- NEVER 許可リスト 13 件の外のライセンス、crates.io 以外/unknown な registry や git source からの解決、未使用の依存を持ち込まない。 (`deny.toml` の `[licenses] allow`/`[sources]`。許可リストの 13 件は `dependencies.md` の `## ライセンスとソースの方針` でも再確認済み。`security` job が `cargo deny check`/`cargo audit`/`cargo machete --with-metadata` で毎回検査する)
- NEVER loopback bind に依存するテストの skip を CI 上で見過ごさない。CI は job 外の `env:` で `SCOUT_NETWORK_TESTS: "1"` を立て、ローカルでは skip する経路を CI では fail させる。 (`.github/workflows/ci.yml` の env comment。DR-0024)
- NEVER `NFR-`/`FR-`/`BR-` に続けて ASCII 数字ちょうど 3 桁を置いた要件コードを `src/` と `tests/` の中に書かない (直前が ASCII 英字でなく、直後が数字でないもの限定)。引用は `docs/` から行う。 (`src/test_support.rs` の `extract_requirement_codes`/`scan_requirement_code_violations` が実ツリーを走査し、`T-SUP016` がその結果が空であることを assert してスイートを落とす。除外は `src/test_support.rs` 自身のみ。理由は ADR-0013 の Context — GitHub 経路のコメントが定義文書の無い決定コードを参照していたことが動機。AI-DLC 側の `FR1`/`FR1.2`/`NFR2`/`BR1.1`/`FR-1` の形はハイフン無しか 1 桁のため一致しないが、`FR-018` の形は一致する。詳細は `evidence.md`)

## 適用範囲についての注記

`main` に GitHub 側のブランチ保護は設定されていない (`gh api repos/thkt/scout/branches/main/protection` は `404 Branch not protected` を返す)。したがって上の各行が指すのは「CI ジョブが違反時に落ちる」という事実であり、「GitHub がマージそのものを技術的にブロックする」という事実ではない。チームは CI が緑になるのを待ってマージする運用を敷いているとは見えるが、その運用自体を強制する仕組みはリポジトリの中には無い。

ゲートの経路にはさらに濃淡がある。`test` job と `security` job は `main` への直 push でも走るが、`coverage` job は PR イベント限定である。したがって `main` へ直接 push した変更は fmt・clippy・テスト・supply chain 検査を受けるが、95% の差分カバレッジ判定だけを受けない。org.md の `classic` スコープが additive に足す 80% の line-coverage floor は scout の 95% diff-coverage より緩い数字だが、測る対象 (絶対値 vs 差分) が異なるため単純に「95% の方が厳格」とは言えず、かつ **どちらのゲートも `main` への直 push には届かない** 点は共通する。

GitHub Actions のワークフロー自体の様式 (SHA pin・`persist-credentials: false`・`permissions:` 最小化) は、`.github/zizmor.yml` に理由付きの例外があり、かつ zizmor の finding が CI ジョブを実際に落とすのか code scanning alert に留まるのかがこの走査では確認できていないため、この節には含めない。事実としての記録は `team-practices.md` の `## Way of Working` に、未確認事項は `evidence.md` に置いた。同じ理由で DR-0028 (シンボル名でのコード参照) も、それ自身を検査する CI job が無いため、`team-practices.md` の `## Code Style` に文書化された規約として記録し、この節には含めない。
