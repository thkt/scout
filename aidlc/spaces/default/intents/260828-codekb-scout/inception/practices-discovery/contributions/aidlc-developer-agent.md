**Collaborator:** aidlc-developer-agent

## Contribution

レンズは命名・レイヤ境界・エラーハンドリング・ファイル構成・コードスタイル規約。テスト量とセキュリティは別レビュアーの担当なので、テストは規約の裏付けとして引くときだけ挙げる。参照はすべて DR-0028 に従いパスとシンボル名で書き、行番号を使わない。

### 1. `discovered-rules.md` の `## Mandated` — 3 行の訂正

冒頭は「機械的に強制している制約だけを挙げる」と宣言するが、続く裏取り基準に「Decision Record が固定している」が入っている。DR は決定の記録であって、違反を落とす仕組みではない。現状 3 行が実際の強制範囲より広く読める。

**(a) `unreachable_pub` の行が、lint 単独では成り立たない主張になっている。**

現行の行は「crate の外へ出る項目は `pub async fn run() -> ExitCode` の 1 つだけに保つ」を `[lints.rust] unreachable_pub = "deny"` だけで裏付けている。`unreachable_pub` が落とすのは「crate 外から到達できない `pub`」であって、「`pub` の総数が 1 であること」ではない。単一入口が保たれているのは、`src/lib.rs` の `mod` 宣言 20 本がすべて非公開だからで、lint はその前提の上で残った `pub` を落とす役をしている。`src/lib.rs` の 1 本を `pub mod fetch;` へ書き換えると、その配下の `pub` 項目は到達可能になるので lint は沈黙し、crate の契約面だけが静かに広がる。

置き換え案:

> ALWAYS `src/lib.rs` の `mod` 宣言はすべて非公開のまま保ち、crate 外へ出す項目を `pub async fn run() -> ExitCode` の 1 つに留める。モジュール境界を越える他の項目は届く範囲だけの可視性 (`pub(crate)`/`pub(super)`/`pub(in path)`) にする。 (`src/lib.rs` の `mod` 宣言。残った到達不能な `pub` は `Cargo.toml` の `[lints.rust] unreachable_pub = "deny"` がビルドで落とす)

**(b) コメント言語の行が、CI が実際に見ている範囲より広い。**

現行の行は「コメント・doc comment・テスト名・assertion message は英語で書く」を `Comment language check` step で裏付ける。この step のパイプラインは `grep -rnE '//' src tests --include='*.rs'` で `//` を含む行を集め、`sed` で二重引用符の文字列と 1 文字リテラルを落とし、残りに `//` の後の日本語が当たるかを見る。したがって落ちるのは `src/` と `tests/` の `.rs` にある `//` 行コメントと `///` doc comment だけになる。

CI に当たらない 3 形がある。テスト関数名 (`fn 日本語テスト()`) はその行に `//` が無い。assertion message (`assert!(cond, "日本語")`) は `sed` が文字列ごと落とすので、`//` 行に置いても当たらない。`/* */` も `//` を含まない。3 形すべてが `.claude/rules/CONVENTIONS.md` の規約では対象なので、規約のほうが CI より広い。

置き換え案 — `## Mandated` に残すのは強制される範囲だけにし、残りは `team-practices.md` の `## Code Style` へ移す:

> ALWAYS `src/` と `tests/` の `.rs` に書く `//` 行コメントと `///` doc comment は英語にする。原語が残せるのはバイト列注釈内の引用断片だけである。 (`.github/workflows/ci.yml` の `Comment language check` step)

`## Code Style` 側へ:「テスト名と assertion message も英語で書く。この 2 つは `Comment language check` の判定対象外なので、規約としてのみ効く」。

**(c) DR-0028 の行が、`## Mandated` で唯一 CI の裏付けを持たない。**

DR-0028 の `### Confirmation` は残存を測る `grep` を示すが、これを走らせる CI job は無い (`.github/workflows/` に `docs/decisions` を参照する行は 0 件)。他の 6 行は CI step・lint の `deny`/`forbid`・リポジトリ自身のテスト (`src/test_support.rs` の `scan_test_id_violations`) のいずれかを持つので、DR-0028 だけが性質の違う 1 行になる。

推奨する扱いは **セクションを分けること**。`## Mandated` の 6 行はそのまま残し、DR-0028 を「文書化された規約 (自動検査なし)」の見出しへ移す。冒頭文を緩めて DR を強制へ含める案は採らない — 6 行の強度が 1 行に合わせて下がり、`team.md` を読む側が「CI が落とすのはどれか」を区別できなくなる。

上の (a) と (b) は行の一部が未強制だという指摘であり、この (c) は行全体が未強制だという指摘になる。DR-0028 の行だけが移動の対象で、(a) と (b) は `## Mandated` に残したまま文言を狭める。

### 2. `team-practices.md` の `## Code Style` — 削除を 1 件

**clippy deny リストの列挙を落とす。** 現行は「deny リストは `Cargo.toml` の `[lints.clippy]` 13 件と `[lints.rust]` 2 件」と数を書き、`discovered-rules.md` の `## Forbidden` が同じ内容を lint 名まで展開して繰り返す。数はどちらも正しい (`[lints.clippy]` 13 件、`[lints.rust]` 2 件を確認した) が、`team.md` へ昇格させる価値が無い。

理由は 2 つ。org.md の `## Code Style` が「framework がコードスタイルを提案するときは先に linter config を読み、linter が既に覆っていないときだけ提案が出る」と定めているので、linter が覆う内容の写しは提案の発火条件を変えない。もう 1 つはこのリポジトリ自身の `.claude/rules/CONVENTIONS.md` が「この表は所在だけを示し、規則の内容を写さない。写した時点で 2 箇所が食い違う」と決めていることで、deny リストの写しはこの規約に正面から反する。lint を 1 本足した日に `team.md` が古くなる。

置き換え案 — 所在と、config を読んでも分からない運用だけを残す:

> リンタは `cargo clippy --all-targets -- -D warnings` を通常 feature と `--all-features` の 2 回。deny の一覧は `Cargo.toml` の `[lints.clippy]`/`[lints.rust]` と `clippy.toml` が持つので写さない。禁止された read の代替関数名は `clippy.toml` の各 `reason` に書いてあり、`reason` を読めば置き換え先が分かる。

なお、これは org.md への抵触ではなく重複と陳腐化のリスクであって、§13 の admission conflict-check で弾かれる類のものではない。

### 3. `team-practices.md` の `## Code Style` — 追加

現行の `## Code Style` は「rustfmt 既定/clippy deny リスト/コメント英語/DR-0028/テスト分割/doc コメント文化/Rust 慣用の命名/Actions の様式」の 8 項目。このうち 5 項目は linter・CI・別文書が既に持つ内容で、linter の config を読んでも出てこない規約は「doc コメント文化」1 つだけになっている。以下は実コードから確認した、どのツールも強制しない規約である。

**(a) エラーハンドリング — 現行の `## Code Style` に 1 行も無い。**

`team-practices.md` は scout のエラー設計に触れていない。CodeKB の `architecture.md` は経路を書くが、Code Generation が読むのは `team.md` なので、新規コードを書く側に届かない。以下 6 点は `src/classify.rs`、`src/envelope.rs`、`src/tools/errors.rs`、`src/fetch.rs`、`src/github/errors.rs` を読んで確認した。

- **分類はバックエンドのエラー enum 自身の `classify() -> Classification` に置き、`From` impl には置かない。** `src/tools/errors.rs` の `From<GitHubError>`/`From<FetchError>`/`From<SlackError>`/`From<BraveError>` は 4 本とも同形で、`e.to_string()` と `e.classify()` を `ScoutError::from_classification` へ渡すだけになっている。バックエンドを 1 つ足す作業は「エラー enum を定義し、`classify()` を実装し、`From` impl を 1 本足す」の 3 手に固定されている。
- **未分類の transport 失敗は `Unknown` (104) であって `TempFailure` (75) ではない。** `src/classify.rs` の `Classification::from_reqwest` は timeout でも transient network でもない `reqwest::Error` を `ErrorCode::Unknown` へ落とす。doc コメントが理由を書いている — `Unknown` 率の上昇が ADR-0011 の求める「分類が取りこぼしている」信号であり、正体不明の transport 失敗を retryable と呼ぶとその信号が埋まる。**新規実装者の反射 (「よく分からないネットワークエラーだから、たぶん一時的。retryable にしておく」) がまさにこのコードベースが禁じている形である。** `[T-ER033]` が全バックエンドについてこれを pin する。
- **`Classification::from_reqwest` のアーム順は入れ替えられない。** timeout 判定が `is_transient_network` より先に来る。`is_transient_network` は timeout にも true を返し、ADR-0002 が timeout を別コード (124) へ分けているためで、順序の理由が doc コメントに書かれている。
- **HTTP status からコードへの写像は `Classification::from_http_status` の 1 表だけが持つ。** バックエンドが足す先行アームは hint を足すためのものである。実例 3 つ (`src/fetch.rs` の `FetchError::classify` の `Status(401 | 403)` と `Status(404)`、`src/github/errors.rs` の `GitHubError::classify` の `Api { code: 401 }`) はいずれも表と同じコードを返し、そのバックエンドにしか書けない `next_step` を足している。コード自体を変える先行アームは DR-0003 が doc コメントでの明示を要求すると `from_http_status` の doc コメントが述べる。走査した範囲にコード側の逸脱は無かった。
- **`retryable` を手で書かない。** `ScoutError::new` が `kind.is_retryable()` から導出し、`bare_error_line` も同じ経路を通る。`[T-W006]` が呼び出し側での再記述を落とす。
- **部分失敗はエラーではなく degraded な成功として返す。** `src/envelope.rs` の `Degradation` はフィールドを非公開にし `push` を唯一の変更手段にすることで `(notes[i], reasons[i])` の対応を保証する。**落とし穴が 1 つある** — `DegradedReason::label` の呼び出し箇所は `src/` 全体で 1 つだけで (`.label()` を `src/` で走査)、`src/tools/errors.rs` の `unwrap_or_degraded` にある。この関数は `Result<Vec<T>, GitHubError>` を取るので、届くのは GitHub の 3 variant (`IssuesFetchFailed`/`PullsFetchFailed`/`ReleasesFetchFailed`) に限られる。新しい variant に固有の label を書いても、その経路を通らなければ出力に現れず、汎用の `"resource"` アームへ落ちる。他の variant は note の文面を呼び出し側で組み立てている。

**(b) 共有する定数とヘルパーの置き場に明文の規則がある。**

`src/body_limit.rs` の module doc が定めている:「2 つ以上のバックエンドが共有する cap やヘルパーはここに置く。1 つのバックエンドだけが使う cap はそのバックエンドに残す」。実際 `read_body_capped` は 4 バックエンドが使うのでここにあり、`MAX_API_RESPONSE_BYTES` は Brave と Slack が共有するのでここにあり、`MAX_GITHUB_RESPONSE_BYTES` は `src/github.rs` に、`MAX_RESPONSE_BYTES` は `src/fetch.rs` に残っている。どのツールも強制しないが、新規コードの置き場を一意に決める規則になっている。

**(c) 外部依存はコンストラクタで作らず `Arc<dyn Trait>` フィールドで注入する。**

`src/tools.rs` の `Scout` が `clock`/`rng`/`token_source`/`dns` の 4 つをこの形で持ち、`ScoutBuilder` の `with_<dep>` 系メソッドが差し替え口になる (DR-0008、DR-0009)。実時間・実ネットワーク・実資格情報をテストが待たないための seam なので、新しい外部依存を足すときはこの形に合わせる。各フィールドの doc コメントに「なぜ `Scout` が持ち、生成箇所の内側に置かないか」が書かれている。

**(d) 依存するフィールドは呼び出し側に書かせず導出する。**

同じ形が 3 箇所にある。`ScoutError::new` が `kind` から `retryable` を導出する。`CommandOutput::with_degradation` が `Degradation` の空判定から `degraded` を導出する。`DegradedReason::label` が variant から label を導出する。いずれも「呼び出し側が両方を書けるとずれる」という理由を doc コメントが書いている。新しい出力型を足すときは、フィールドを非公開にして導出コンストラクタを 1 本だけ出す形に合わせる。

**(e) lint 抑制は `#[expect]` に `reason` を付けて書く。`#[allow]` は使わない。**

`src/` と `tests/` を `#!?\[(allow|expect)\(` で走査すると、`src/` の `#[expect]` が 8 箇所、`#[allow]` は 0 箇所。`tests/common/mod.rs` の `#![allow(dead_code)]` 1 箇所だけが例外で、これはファイル全体に掛かる inner attribute である。8 箇所すべてに `reason` が付く。

理由が一次ソースにある。`src/slack/client.rs` の `DummyBody` の doc コメントが「`allow` ではなく `expect`。テストがこのフィールドを読む日が来たら、抑制自体が不要になったと報告される」と書いている。**`allow` は不要になっても黙って残るが、`expect` は不要になった時点でビルドが落ちる。** clippy の config はこの選択を強制しないので、規約として書く価値がある。

8 箇所のうち本番経路は 3 つ (`src/fetch/converter.rs` の `pre_handler`、`src/github/format.rs` の `format_size`、`src/github/types.rs` の `ContentsPayload::Directory`)。残る 5 つはテスト側で、うち 4 つが `clippy::disallowed_methods` の局所例外にあたる。

**(f) 可視性は「届く範囲だけ」を毎回選ぶ。lint はこの判断を検査しない。**

`unreachable_pub` が落とすのは到達不能な `pub` だけで、`pub(super)` で足りる項目に `pub(crate)` を付けても通る。段階の実例は `pub(crate)`、`pub(super)` (`src/tools/errors.rs` の `ScoutError::user_error` など)、`pub(in crate::fetch::cdp)` (`src/fetch/cdp/proxy.rs` の `spawn_ssrf_proxy` の再輸出) の 3 形。決め方の手順と注意点は `docs/audit/2026-08-11-rust-code-assessment.md` の B-6 が持つので、`team.md` には所在だけを書く。

**(g) 命名 — 現行の draft は「Rust の慣用に従う」の 1 行しか持たない。**

慣用は rustfmt と clippy が既に覆うので、`team.md` に足す価値があるのは慣用の外側にある 2 つの形だけになる。

- **同じクラスの失敗には同じ文字列を返す名前付き定数を使う。** `src/classify.rs` の `HINT_RETRY_DELAY` と `HINT_CHECK_NETWORK` がそれで、「同じクラスの失敗を報告するどのバックエンドも、呼び出し側へ同一の `next_step` 文字列を渡す」ためだとコメントが述べる。
- **役割が分かれる関数は、役割を名前に入れる。** `src/body_limit.rs` の `read_body_capped` (ペイロード用) と `read_body_snippet` (診断用) がその形で、`clippy.toml` の各 `reason` が用途で置き換え先を指し分けられるのはこの命名があるからである。

**(h) テスト専用の import は `#[cfg(test)]` で囲う。**

`src/tools.rs` に `#[cfg(test)] use` のブロックがあり、コメントが理由を書いている — 本番コードはこれらを使わず、同モジュール内のテストファイルが `use super::*` 経由で届くため。兄弟テストファイル方式 (`#[cfg(test)] mod <name>_tests;`) を採る以上、この形は繰り返し必要になる。

### 4. `evidence.md` へ — レイヤ規則を書くなら付ける必要がある但し書き

現行の `team-practices.md` は依存の向きに触れていないので、以下は draft の誤りではなく、**レイヤ規則を追加するなら先に知っておく必要がある制約**である。

CodeKB の `architecture.md` は `## コンポーネント関係` に「循環は無い」「横断リーフ側からバックエンドへの import も無い」と書き、直後にその測定範囲を `src/tools.rs` と `src/fetch.rs` の `use crate::…` を読んだ範囲に限ると明記している。横断リーフ 12 本の `^use crate::` を直接読むと、この 2 文は測定範囲の外で成り立たない。

`src/yaml.rs` が `crate::search::engine::MAX_PAGE_BYTES` を import し、`src/search/engine.rs` が `crate::yaml::truncate_and_reneutralize` を import する。リーフからバックエンドへの import が 1 件あり、それが双方向になっている。`src/yaml.rs` の `MAX_FIELD_BYTES` は `MAX_PAGE_BYTES / 10` として定義され、doc コメントが「`search::engine::MAX_PAGE_BYTES` (4,500) から導出。リーフが遭遇する最も厳しい予算だから」と理由を書いているので、意図された参照であって取り残しではない。

したがって `team.md` に書ける層の規則は「バックエンド → 横断リーフの一方向」ではなく、「`tools` がバックエンドへディスパッチし、逆向きの import を持たない」までになる。CodeKB 側の 2 文にも測定範囲を反映する訂正が要る。

### 5. `zizmor.yml` の項目の置き場

`team-practices.md` の `## Code Style` 末尾に GitHub Actions の SHA pin・`persist-credentials: false`・最小 `permissions:` が入っている。内容は正しいが、コードスタイルではなくサプライチェーンの防御であり、判断はセキュリティレビュアーの担当になる。`## Code Style` から外し、セキュリティ側の節へ回すことを提案する。

## Positions

- AGREE: `#[expect(clippy::disallowed_methods, reason = "...")]` の例外がテストに限られるという `discovered-rules.md` の記述 — `src/` の `disallowed_methods` 抑制 4 箇所 (`src/tools/errors/exit_code_tests.rs` の `[T-ER033]`、`src/retry/tests.rs` の `[T-R008]`/`[T-R009]`、`src/slack/classify_tests.rs` の `[T-SLNET002]`/`[T-SLNET004]` が並ぶ箇所) をすべて開いて確認し、本番経路のものは無かった。
- AGREE: doc コメントが「なぜこの値か」「何を却下したか、それを測った数値」を書くという `## Code Style` の記述 — 現行 `## Code Style` の 8 項目で唯一、linter の config を読んでも出てこない規約であり、走査した範囲でも `Classification::from_reqwest` のアーム順、`body_limit.rs` の module doc、`DummyBody` の `expect` 選択と一貫して成立していた。
- AGREE: ブランチ保護が無いという但し書きを `discovered-rules.md` の末尾に置いた判断 —「CI が落ちる」と「GitHub がマージを止める」は別の事実で、これを混ぜると `team.md` を読む側が強制力を過大に見積もる。
- AGREE: `Methodology: test-after` を org.md の未確認時デフォルトの仮置きだと明示し、証跡ではないと繰り返した扱い — テストのメソドロジーは私のレンズ外だが、値の出所を偽らずに人間へ回す形として正しい。
- OBJECT: `discovered-rules.md` の `## Mandated` にある `unreachable_pub` の行 — lint 単独では「crate 外へ出る項目が 1 つだけ」は保証されず、実際に効いているのは `src/lib.rs` の非公開 `mod` 宣言との組み合わせである。上の 1(a) の置き換え案を提案する。
- OBJECT: `discovered-rules.md` の `## Mandated` にあるコメント言語の行 — `Comment language check` が判定するのは `//` を含む行だけなので、テスト名と assertion message は CI に当たらない。強制される範囲と規約の範囲を 1 行に混ぜている。上の 1(b) で分割案を出した。
- OBJECT: `discovered-rules.md` の `## Mandated` にある DR-0028 の行 — DR の `### Confirmation` の grep を走らせる CI job は存在せず (`.github/workflows/` に `docs/decisions` の参照は 0 件)、行全体が自動検査を持たない唯一の行になっている。別セクションへ分けることを提案する。
- OBJECT: `team-practices.md` の `## Code Style` にある clippy deny リストの数と内容の列挙 — `.claude/rules/CONVENTIONS.md` 自身が「所在だけを示し、規則の内容を写さない。写した時点で 2 箇所が食い違う」と定めており、lint を 1 本足した日に `team.md` が古くなる。所在と `clippy.toml` の `reason` の読み方だけを残す案を 2 に書き、この枠が空いた分に入れる未強制の規約 7 件 (共有ヘルパーの置き場、`Arc<dyn Trait>` 注入、導出フィールド、`#[expect]` に `reason`、可視性の選び方、hint 定数の共有、`#[cfg(test)] use`) を 3(b) から 3(h) に草案として置いた。
- OBJECT: `team-practices.md` にエラーハンドリングの節が無いこと — Code Generation が読むのは `team.md` であり、CodeKB の `architecture.md` は届かない。未分類の transport 失敗を `TempFailure` ではなく `Unknown` へ落とすという最も反直感的な規約が、どの成果物にも新規実装者向けの形で書かれていない。上の 3(a) に 6 点の草案を置いた。
