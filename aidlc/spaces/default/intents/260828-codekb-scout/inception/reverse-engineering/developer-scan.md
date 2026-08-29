# Developer Code Scan — scout (focused, attempt 2)

**Collaborator:** aidlc-developer-agent

## Developer Code Scan Results

この走査は FOCUSED である。既存 CodeKB (`aidlc/spaces/default/codekb/scout/`、9 artifact) は CURRENT として検証済みで、その 17 パスは再走査していない。本文書は **既存ストアが未読のまま抱えていた 4 ファイルを開いて測った結果だけ** を記録する。ストアが既に述べていることは繰り返さない。ストアの記述を覆した箇所は「**上書き**」と明示し、コマンドまたは `file:line` を添える。

測定基準: `git rev-parse HEAD` = `c8460b59c04b785e7e8378b37bc80504bad2d743` (省略形 `c8460b5`)、version 2.6.0、測定日 2026-08-29。ブランチ `chore/aidlc-v2-install`。ディスパッチが与えた snapshot の source fingerprint は `git:498e1629fc5c29351ee88564fe0d3c66c16f5cfe` で、この HEAD とは別の導出である。両者の突き合わせはこの走査の担当範囲外なので、HEAD の実測値だけを記録して architect へ渡す。

### Scan Coverage

- **Analyzed deeply** (この attempt で端から端まで読んだ 4 ファイル):
  - `src/fetch/converter.rs` (3,131 行 — 実装 1-985、`#[cfg(test)] mod tests` 986-3,131)
  - `src/slack/client.rs` (620 行 — 全行が実装。inline `mod tests` は無い)
  - `src/tools/config.rs` (477 行 — 実装 1-195、inline `mod tests` 196-477)
  - `renovate.json` (33 行)
- **Skimmed only** (証拠として `file:line` を引いただけで、精読していない):
  - `src/slack/client/http_tests.rs` (1,056 行) — 1-60 行と 120-200 行のみ精読。残りは doc コメント行の抽出と、`api_get` 呼び出し箇所の全数列挙にとどまる
  - `src/slack/client/constructor_tests.rs` (96 行) — `api_get` 呼び出し箇所と `Api`/`Decode`/`ok:false` の全数検索のみ。本文は未読
  - `src/slack.rs` — `classify()` (95-190 行) のみ精読
  - `src/body_limit.rs` — `MAX_API_RESPONSE_BYTES` の定義行のみ
  - `src/fetch/cdp/launch.rs` の `mod` 宣言 282-289 行 — 分割の前例確認のため
  - `Cargo.toml` の 5 行 (`rust-version`) と 27-31 行 (`markup5ever_rcdom` の pin コメント) — renovate.json との相互参照確認のため (ストアでは既に analyzed)
  - `docs/audit/2026-08-11-rust-code-assessment.md` の 279-310 行 (E-1〜E-4)

上の skimmed 項目 7 件のうち `src/slack/client/http_tests.rs`、`src/slack/client/constructor_tests.rs`、`src/slack.rs` は **pre-scan snapshot の 21 パスにも、ストアの shallow 48 パスにも入っていない**。ストアの shallow リストは `src/` のテスト専用兄弟ファイル 45 本を列挙していないため、`src/slack/client/` 配下の 2 本はどちらのリストにも現れない。これらを analyzed へ格上げするには新しい snapshot が必要なので、この走査では **証拠の引用元** として扱い、格上げしない。

#### architect へ渡す 2 リスト

`analyzed.paths` — ストアの 17 とこの走査の 4 の和集合。**21 件**。

```
.config/nextest.toml
.github/workflows/ci.yml
Cargo.toml
clippy.toml
deny.toml
docs/decisions/0012-connect-time-ip-guard-for-ssrf-dns-rebinding.md
docs/decisions/README.md
renovate.json
src/classify.rs
src/envelope.rs
src/fetch.rs
src/fetch/cdp/launch.rs
src/fetch/cdp/proxy/transport.rs
src/fetch/converter.rs
src/github/types.rs
src/lib.rs
src/main.rs
src/slack/client.rs
src/tools.rs
src/tools/config.rs
src/tools/test_helpers.rs
```

`shallow.paths` — ストアの shallow 48 から、上へ格上げした 4 件を引いたもの。**44 件**。

```
.github/workflows/label-from-issue.yml
.github/workflows/release.yml
.github/workflows/zizmor.yml
README.ja.md
README.md
docs/audit/
src/body_limit.rs
src/brave.rs
src/brave/client.rs
src/brave/types.rs
src/charset.rs
src/clock.rs
src/fetch/cdp.rs
src/fetch/cdp/proxy.rs
src/fetch/download.rs
src/fetch/extractor.rs
src/fetch/ssrf.rs
src/github.rs
src/github/encoding.rs
src/github/errors.rs
src/github/format.rs
src/github/helpers.rs
src/markdown.rs
src/redacted.rs
src/retry.rs
src/rng.rs
src/search.rs
src/search/engine.rs
src/search/lang.rs
src/signals.rs
src/slack.rs
src/slack/format.rs
src/slack/mention.rs
src/slack/url.rs
src/test_support.rs
src/token_source.rs
src/tools/builder.rs
src/tools/errors.rs
src/tools/params.rs
src/tools/query.rs
src/tools/repo.rs
src/tools/typo.rs
src/yaml.rs
tests/
```

算術: 17 + 4 = 21、48 − 4 = 44。リストは手写しではなく、ストアの `## Scope of Analysis` ブロックを `awk` で抽出し `sort -u` と `grep -vxF -f` で導出した。

---

## 監査項目 E-1 — `surface_overrides` の 5 連 if (`src/tools/config.rs`)

**決着した。閾値「8-10」は 3 通りのどの数え方でも未達である。**

ストアは「該当ファイルは未読で、この記述は監査文書の引き写し」と自ら注記していた。ファイルを開いた結果、監査文書の記述は正確だった。

### 3 通りの数え方をすべて示す

「フィールドが 8-10 個」という閾値は、どの名詞を数えるかで値が変わりうる。3 つとも測った結果、**すべて 5** で一致した。

| 数える対象                           | 値  | 測定範囲                                                                                                                |
| ------------------------------------ | --- | ----------------------------------------------------------------------------------------------------------------------- |
| `surface_overrides` の `if` アーム   | 5   | `src/tools/config.rs:105-136` の関数本体。`if` は 106、112、118、124、130 行から始まる 5 個                             |
| `RuntimeConfig` 構造体のフィールド   | 5   | `src/tools/config.rs:50-56`。`fetch_timeout`、`research_timeout`、`slack_timeout`、`github_timeout`、`max_retries`      |
| このファイルが読む `SCOUT_*` env var | 5   | `src/tools/config.rs:27-31` の `ENV_*` 定数 5 本。すべて `from_env_with` (78-99 行) から参照され、全部が surface される |

監査文書の「フィールド」が指すのはこの 3 つのうちどれであっても同じ 5 なので、閾値の判定に曖昧さは残らない。**現状維持という監査文書の判断は、この走査時点でも有効である。**

### 監査文書が書いていない 2 点

**1. `info!` のフィールド名は構造体のフィールド名と一致しない。** 監査文書は「フィールド名が `tracing` の構造化ログのキーになっている」と書くが、実際には 4 個で名前が変わる。

| 構造体フィールド   | `info!` のキー          |
| ------------------ | ----------------------- |
| `fetch_timeout`    | `fetch_timeout_secs`    |
| `research_timeout` | `research_timeout_secs` |
| `slack_timeout`    | `slack_timeout_secs`    |
| `github_timeout`   | `github_timeout_secs`   |
| `max_retries`      | `max_retries`           |

`Duration` を `as_secs()` で `u64` に落とすため接尾辞 `_secs` が付く。構造体フィールドを機械的に走査するマクロやループでは、この 4 個のキー名を導出できない。**マクロで畳むコストは監査文書が見積もったより高い。**

**2. 5 個の `if` は「ほぼ同形」であって同形ではない。** 4 個は `self.<field>.as_secs() != DEFAULT_<X>_SECS` (`u64` 比較) だが、5 個目 (130 行) は `self.max_retries != DEFAULT_MAX_RETRIES` で、型が `u32` であり、比較相手の定数がこのファイルではなく `crate::retry` にある (`src/tools/config.rs:6` の `use crate::retry::DEFAULT_MAX_RETRIES;`)。

### 挙動を pin しているテスト

`surface_overrides` の出力は 3 本のテストが押さえる。いずれも `tracing_test::traced_test` + `logs_contain`。

| テスト ID      | 行      | pin している内容                                                                        |
| -------------- | ------- | --------------------------------------------------------------------------------------- |
| `T-CFG-LOG001` | 339-362 | 上書きされたフィールドだけが INFO を出し、`fetch_timeout_secs=120` という構造化値を運ぶ |
| `T-CFG-LOG002` | 364-375 | 全フィールドがデフォルトなら 1 件も出ない (無音経路)                                    |
| `T-CFG-LOG003` | 462-476 | `github_timeout` についても同じ形が成り立つ                                             |

`T-CFG-LOG002` の存在が効いている。ループやマクロへ畳んだときに「差分なしでも出す」退化が起きたら、このテストが落とす。

### ついでに確定した `src/tools/config.rs` の設計

- **env 読み取りは注入可能。** `from_env_with<F>` (78 行) が `Fn(&str) -> Result<String, env::VarError>` を取る。理由が doc コメントにある — `unsafe { std::env::set_var(...) }` が `unsafe_code = "forbid"` で禁じられているため
- **`VarError::NotUnicode` は「未設定」ではなく「設定されているが読めない」として `UsageError` に落ちる** (`read_env_raw`、143-154 行)。デフォルトへのフォールスルーを避ける意図が doc コメントにある
- **範囲は `TIMEOUT_MIN_SECS`=1 / `TIMEOUT_MAX_SECS`=600、`RETRIES_CAP`=10。** デフォルトは fetch 95/research 45/slack 60/github 180 秒、`max_retries` は `DEFAULT_MAX_RETRIES` (テスト `T-CFG001` が 2 と assert)
- **タイムアウト階層が値として pin されている。** `T-CFG021` は `github_timeout > HTTP_TIMEOUT` かつ `> CANDIDATE_FETCH_TIMEOUT`、`T-CFG026` は `research_timeout > REQUEST_TIMEOUT + FETCH_TIMEOUT`、`T-CFG025` (`#[cfg(feature = "js-rendering")]`) は `fetch_timeout > CDP_TIMEOUT`。**内側の定数を縮める変更を外側から落とす仕掛けであり、CodeKB の architecture 面に載る性質**
- `DEFAULT_GITHUB_TIMEOUT_SECS` = 180 の doc コメント (13-24 行) が、180 秒を選んだ根拠と、それが retry 予算 (~279s) の下に置かれる理由と、切り捨てられるケース (~186s) まで数値で書く。ストアの `code-structure.md` が言う「doc コメントが却下を残す」の追加実例

---

## 監査項目 E-3 — `api_get_once` の二重パース (`src/slack/client.rs`)

**決着した。監査文書の機械的な記述はすべて正しい。加えて、監査文書が書いたより棄却理由は強い。**

ストアは「該当ファイルは未読で、この記述は監査文書の引き写し」と注記していた。

### 二重パースの実体

`src/slack/client.rs:247-268`。

1. `serde_json::from_slice(&bytes)` → `serde_json::Value` (247-248 行)。失敗は `SlackError::Decode`
2. `body.get("ok")` を `as_bool` で見て `Some(true)` でなければエラー分岐 (250-266 行)
3. `serde_json::from_value(body)` → 目的型 `T` (268 行)。失敗は `SlackError::Decode`

上限は `MAX_API_RESPONSE_BYTES` = `1024 * 1024` (`src/body_limit.rs:19`)。監査文書の「1MiB」は正しい。`read_body_capped` が超過分を `Decode` に落とす (234-244 行)。

### 監査文書より強い棄却理由 — `Api` は 6 通りに分岐する

監査文書は「`Api` ではなく `Decode` に落ちる (エラー分類が変わる)」と書く。実際に失われるのは **ラベルではなく 6 分岐** である。`src/slack.rs:112-176` の `classify()` で、`SlackError::Api { error }` は error 文字列によって次へ分かれる。

| 分岐先 `ErrorCode` | 代表的な error 文字列                                             |
| ------------------ | ----------------------------------------------------------------- |
| `UsageError`       | `invalid_auth`、`missing_scope`、`not_authed` ほか計 14 文字列    |
| `DataError`        | `invalid_arguments`                                               |
| `NotFound`         | `channel_not_found`、`message_not_found`、`thread_not_found` ほか |
| `TempFailure`      | `internal_error`、`service_unavailable`、`invalid_cursor` ほか    |
| `Internal`         | `invalid_arg_name`、`deprecated_endpoint`、`method_deprecated`    |
| `Unknown`          | 表にない文字列 (ADR-0011 の retreat slot)                         |

対して `SlackError::Decode(_)` は `src/slack.rs:187` で `ErrorCode::Internal` 一択、exit 70、retry なし。

**`internal_error` が retry される経路が実測で pin されている** — `T-SK077` (`src/slack/client/http_tests.rs:800-834`) が「`internal_error` は 1 回 retry され、2 回目の成功が返る」を assert し、その doc コメントが「`is_retriable` は `classify().kind` から導かねばならない」と書く。畳めばこの retry が消える。

### 一方で、監査文書の論拠を直接 pin するテストは無い

**これが新たに判明した事実である。** 監査文書の棄却理由は「`ok: false` **かつ目的型に合わない**本文」というケースに立つが、そのケースを走らせるテストは 1 本も無い。

測定範囲を明示する。`api_get` / `api_get_once` を直接呼ぶ箇所を兄弟テストファイル 2 本から全数列挙すると **13 箇所** で、**13 箇所すべてが目的型に `DummyBody` を指定している**。

```
grep -n 'api_get' src/slack/client/http_tests.rs src/slack/client/constructor_tests.rs
```

内訳は `src/slack/client/http_tests.rs` の 39/64/95/119/138/161/190/210/829/912/945/982 行の 12 箇所と、`src/slack/client/constructor_tests.rs:21` の 1 箇所。すべて `let result: Result<DummyBody, _> = …` の形である。この列挙は 2 ファイル全体を対象にしており、そのうち `Api` / `Decode` を assert する 3 本は下表のとおり。

| テスト ID | 行      | 本文                                          | 目的型      | 結果          |
| --------- | ------- | --------------------------------------------- | ----------- | ------------- |
| `T-SK004` | 123-143 | `{"ok": false, "error": "channel_not_found"}` | `DummyBody` | `Api`         |
| `T-SK031` | 145-166 | `{"ok": false}`                               | `DummyBody` | `Decode`      |
| `T-SK003` | 104-121 | `{"ok": false, "error": "ratelimited"}`       | `DummyBody` | `RateLimited` |

`DummyBody { ok: bool }` (`src/slack/client.rs:610-615`) は serde が既定で未知フィールドを無視するため、上のどの本文も deserialize に成功する。**「目的型に合わずに失敗する」経路は、直接呼び出し 13 箇所のどれでも踏まれない。** 本番の目的型 3 種を通る残りのテストは `fetch_message` などの経路から間接的に到達するが、そちらは `ok: true` の happy path か lookup 失敗の縮退経路であり、「`ok: false` かつ目的型に合わない」本文は与えていない。

さらに、本番の目的型 4 種はすべて全フィールドが `Option` か `#[serde(default)]` である。

| 目的型         | 定義行                        | フィールド                                                                                 |
| -------------- | ----------------------------- | ------------------------------------------------------------------------------------------ |
| `ChannelBody`  | `src/slack/client.rs:272-275` | `channel: Option<ChannelInfo>`                                                             |
| `UserBody`     | `src/slack/client.rs:311-314` | `user: Option<UserDetail>`                                                                 |
| `MessagesBody` | `src/slack/client.rs:381-388` | `#[serde(default)] messages`、`#[serde(default)] has_more`、`response_metadata: Option<…>` |
| `DummyBody`    | `src/slack/client.rs:610-615` | `ok: bool` (`#[cfg(test)]` 専用)                                                           |

したがって **キー欠落では目的型の deserialize は失敗しない**。監査文書が想定する分類変化を実際に起こすには、キーが存在して型が合わない本文 (`"messages": "oops"` など) が必要である。

**この 2 点は矛盾しない。** 棄却の判断は正しく (畳めば 6 分岐と retry を失う)、その判断の論拠として監査文書が挙げた具体ケースは現在の型形状では発生しにくく、直接呼び出し 13 箇所のどれもそれを踏んでいない、というのが実測の姿である。architect へは「E-3 は決着済み・現状維持で正しい。ただし監査文書の論拠を直接 pin するテストは無く、より強い根拠は `T-SK077` と `classify()` の 6 分岐にある」として渡すのが正確。

### `src/slack/client.rs` について新たに確定したこと

- **補足 (上書きではない) — このファイルは inline `mod tests` を持たない。** 620 行すべてが実装である。テストは兄弟ファイル 2 本 (`src/slack/client/constructor_tests.rs` 96 行、`src/slack/client/http_tests.rs` 1,056 行) にあり、`src/slack/client.rs:617-620` の `#[cfg(test)] mod constructor_tests;`/`#[cfg(test)] mod http_tests;` から参照される。ストアの `code-structure.md` の `## サイズ分布` は 620 行を内訳の列を空のまま載せているだけで、テストを含むとは主張していない。よってこれは**訂正ではなく追加**である。ただし内訳付きの `converter.rs` 行 (3,131 = 実装 985 + テスト 2,146) と並べると誤読を招くので、内訳を添えること
- **retry の増幅を避けるため `api_get_once` を直接呼ぶ箇所が 2 つある。** `resolve_channel` (288 行) と `fetch_user_name` (332 行)。どちらも失敗を握って raw ID へ縮退するので retry が無駄になる。`fetch_user_name` 側の doc コメントは無駄の量まで書く —「`SLACK_MAX_USER_LOOKUPS` (50) 件の失敗 lookup が `1 + DEFAULT_MAX_RETRIES` リクエストずつで 150 リクエストを使う」。`T-SK073`/`T-SK074` が「1 回しか送らない」を pin する
- **cap が 4 つ、すべて定数と doc コメント付き** — `SLACK_REPLIES_LIMIT` = `"200"`、`SLACK_USERS_CONCURRENCY` = 5、`SLACK_MAX_REPLY_PAGES` = 50、`SLACK_MAX_USER_LOOKUPS` = 50。cap ヒットは `SlackFetchOutcome` の 3 bool (`thread_truncated`/`users_capped`/`lookups_failed`) で呼び出し側へ運ばれる。「`String` 返しでは cap が見えない」という設計理由が構造体の doc コメント (36-38 行) にある。ADR-0003 の degradation channel へ繋がる
- **`lookups_failed` と `users_capped` を分ける理由が doc コメントにある** (46-53 行) —「200 が名前を持たずに返ったのは失敗に数えない。lookup は Slack に届いており、返す名前が無かっただけなので、呼び出し側に retry するものが無い」
- **トークン種別を prefix で門前払いする。** `USER_TOKEN_PREFIX` = `"xoxp-"` (84 行)。bot (`xoxb-`)/app-level (`xapp-`)/workflow (`xwfp-`) を弾く理由 (bot トークンはアプリが追加されたチャンネルしか見えない) と検証日 (2026-06) が doc コメントにある
- **非 429 の非 2xx を JSON パースの前で切る** (228-232 行)。理由がコメントにある —「Slack 自身の失敗は 200 の本文の `error` 文字列で来るので、それ以外の非 2xx は scout と Slack の間の何かから来た。JSON パースへ届かせると gateway の一時障害が `Decode` → `Internal(70)` になり、retry されなくなる」。`T-SK068` が 502 で pin する
- **テスト専用の HTTPS 迂回が `#[cfg(test)]` で隔離されている。** `skip_https_check` フィールド (71-72 行) と `should_check_https()` (167-176 行) は `#[cfg(test)]` 側だけが `false` を作れる。本番のコンストラクタは常に `validate_https` を通す

---

## 監査項目 E-4 — `src/fetch/converter.rs` の 3,131 行

**部分的に決着した。「6 群」という記述は上書きする。切り出し単位の判断そのものは依然として未着手だが、判断に必要な材料はこの走査で揃った。**

### 上書き — テスト ID は 6 群ではなく 9 関心・26 連続区間に分かれる

ストアの `code-structure.md` と `code-quality-assessment.md` は、監査文書 E-4 を引いて「テスト ID の並びが表/pre とフェンス/リンクとアンカー/script と style の抑制/frontmatter/リストの 6 群に分かれる」と書く。**ファイルを開いて 79 本すべてを分類した結果、この記述は 2 点で不正確である。**

測定範囲: `src/fetch/converter.rs` の 986-3,131 行 (`#[cfg(test)] mod tests` ブロック全体)。テスト属性 79 個 (`#[test]` のみ、`#[tokio::test]` は 0)、テスト ID 79 個で重複なし。

**この 2 つの数は機械測定だが、下の「9 関心」と「26 区間」は測定ではなく分類である。** 分類の基準は「各テストの doc コメントと assertion がどの実装関数を狙っているか」で、79 本を 1 本ずつ手で割り当てた。区間数はその割り当て列をファイル順に走らせて切り替わり回数を数えたものなので、割り当てが変われば区間数も変わる。architect は再導出してよい。境界例を 1 つ挙げると `T-FC025` (1093 行) は CELLCODE に置いたが、doc コメントは挙動を `normalize_cell_content` (TABLE 側) に帰している。TABLE へ移せば CELLCODE 8 / TABLE 17 になり、区間数 26 は変わらない。

**下の 2 つの上書きは、この分類の粒度に依存しない。** どちらもテスト本体を読めば確かめられる — 「リスト」が 2 本でどちらもリスト変換のテストではないこと、6 群が名指ししない関心が 2 つあってどちらも「リスト」より大きいこと。

```
sed -n '986,3131p' src/fetch/converter.rs | grep -cE '^\s*#\[test\]|^\s*#\[tokio::test'   # → 79
sed -n '986,3131p' src/fetch/converter.rs | grep -oE '\[T-[A-Za-z0-9-]+\]' | sort -u | wc -l  # → 79
```

**不正確な点 1 — 「リスト」は関心ではない。** 該当は 2 本だけで、しかもどちらもリスト変換そのものを見ていない。`T-FC024` (1050 行) は `<li>` の中の `<pre>` がリストマーカーの下にインデントされたまま残るか、`T-FC046` (2295 行) は `<li>` の中の `<br>` が `list_item_handler` のインデント処理で末尾 2 スペースを失うか。**どちらもリスト以外の構造 (`<pre>` / `<br>`) が list という容器の中でどう振る舞うかのテスト**であり、切り出せる単位を構成しない。

**不正確な点 2 — 6 群が名指ししていない関心が 2 つあり、どちらも「リスト」より大きい。**

| 関心                                                 | 本数 | 6 群での扱い           |
| ---------------------------------------------------- | ---- | ---------------------- |
| テーブルセル内 `<pre>` → inline code span (CELLCODE) | 9    | **名指しされていない** |
| `<br>` と空白の畳み込み (BR-WS)                      | 5    | **名指しされていない** |
| リスト容器 (CONTAINER-li)                            | 2    | 「リスト」として名指し |

### 実測した 9 関心の内訳

| 関心         | 本数 | 対応する実装 (行範囲は `src/fetch/converter.rs`)                                                                                                                                                                                         |
| ------------ | ---- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| PRE          | 24   | `pre_handler` 114-169、`has_code_child` 341、`raw_pre_content` 365、`push_element_content` 393、`span_handler` 422、`has_pre_ancestor` 429                                                                                               |
| TABLE        | 16   | `table_handler` 678-782、`row_children` 784、`is_row` 793、`row_is_all_header_cells` 808、`extract_data_row` 829、`extract_row_cells` 854、`normalize_cell_content` 885、`format_table_row` 907、`format_separator_row` 921              |
| ANCHOR       | 10   | `a_handler` 546、`anchor_href` 577、`anchor_attr` 581、`strip_link_title` 604、`process_title_like_htmd` 621、`split_trailing_document_whitespace` 651                                                                                   |
| CELLCODE     | 9    | `pre_handler` のセル分岐 120-130、`has_table_cell_ancestor` 433、`inline_code_span` 504、`text_content` 472、`push_text_content` 478                                                                                                     |
| SUPPRESS     | 8    | `suppressed_handler` 171、`SUPPRESSED_TAGS` 181、`is_suppressed_element` 190、`element_namespace` 203、`RAW_TEXT_TAGS` 215、`close_self_closed_raw_text_tags` 238、`raw_text_tag_at` 290、`end_tag_at_or_after` 306、`start_tag_end` 317 |
| BR-WS        | 5    | 実装なし。htmd 組み込みの `br_handler` / `compress_whitespace` の挙動を pin する回帰テスト群                                                                                                                                             |
| FRONTMATTER  | 4    | `format_with_frontmatter` 958-985                                                                                                                                                                                                        |
| CONTAINER-li | 2    | 実装なし。htmd の `list_item_handler` の挙動を pin する                                                                                                                                                                                  |
| RESULT       | 1    | `to_fetch_result` 930-956、`FetchResult` 19-68                                                                                                                                                                                           |

### 連続性 — 26 区間に割れている。cut は素直に取れない

ファイル順に関心が切り替わる回数を数えると **26 区間**。9 関心が 26 区間に散っているので、「6 群が連続して並んでいる」という像は成り立たない。

```
TABLE x1        (1016)          ← 冒頭 4 本は 1 関心 1 本ずつの回帰ヘッダ
CONTAINER-li x1 (1050)
CELLCODE x1     (1093)
ANCHOR x1       (1128)
FRONTMATTER x3  (1149-1185)
RESULT x1       (1203)
PRE x5          (1240-1324)
TABLE x1        (1343)          ← 侵入: T-FC082 が PRE の連なりを割る
PRE x9          (1367-1554)
TABLE x4        (1585-1682)
PRE x1          (1706)          ← 侵入: T-FC055 が TABLE の連なりを割る
TABLE x9        (1725-1979)
PRE x7          (2009-2160)
BR-WS x2        (2184-2207)
PRE x1          (2227)          ← 侵入: T-FC043
BR-WS x2        (2249-2269)
CONTAINER-li x1 (2295)          ← 侵入: T-FC046
BR-WS x1        (2322)
ANCHOR x9       (2350-2561)     ← 最長の素直な塊
SUPPRESS x7     (2588-2813)     ← 2 番目に素直な塊
CELLCODE x7     (2841-2983)     ← 3 番目に素直な塊
PRE x1          (3005)          ← 末尾 5 本はまた 1 関心 1 本ずつ
FRONTMATTER x1  (3024)
CELLCODE x1     (3053)
TABLE x1        (3078)
SUPPRESS x1     (3105)
```

**割れ方には規則がある。** ファイル順は ID 番号順ではない (`T-FC083` → `T-FC082` → `T-FC020`、`T-FC068` → `T-FC067`、`T-FC091` → `T-FC078`)。**このファイルは追加された順に積まれており、関心順ではない。** その結果、冒頭 1016-1203 の 8 本と末尾 3005-3131 の 5 本が「1 関心 1 本ずつ」の散らばりになり、中央部だけが関心ごとの塊になっている。

**切り出し可能な塊は 3 つある。** `ANCHOR` (2350-2561、9 本連続)、`SUPPRESS` (2588-2813、7 本連続)、`CELLCODE` (2841-2983、7 本連続)。この 3 つは連続していてそのまま切り出せる。**残る `PRE` 24 本と `TABLE` 16 本は 4 区間・4 区間に散っており、切り出す前に並べ替えが要る。**

### 可視性のコスト — テストだけの分割は無料、実装の分割は高い

実装側 (1-985 行) の内訳:

| 種別                 | 数  | 内容                                                                                                                                                                              |
| -------------------- | --- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 素の private `fn`    | 38  | 関心表に挙げたほぼ全部。measurement: `fn` 定義行を可視性接頭辞ごとに分類 (インデント込み)                                                                                         |
| 素の private `const` | 3   | `SUPPRESSED_TAGS`、`SVG_NAMESPACE`、`RAW_TEXT_TAGS`                                                                                                                               |
| `pub(crate) fn`      | 6   | すべて `impl FetchResult` (33-68 行) のメソッド。`url` / `markdown` / `used_raw_fallback` / `decode_uncertain` / `for_test` / `with_decode_uncertain`。後ろ 2 つは `#[cfg(test)]` |
| `pub(super) fn`      | 1   | `to_fetch_result` 930                                                                                                                                                             |
| `pub(crate) const`   | 2   | `RAW_FALLBACK_NOTE` 70、`DECODE_UNCERTAIN_NOTE` 73                                                                                                                                |
| `pub(crate) struct`  | 1   | `FetchResult` 20                                                                                                                                                                  |

`pub` 付きは合計 10 個である。**列 0 の `^pub\(` だけを数える素朴な grep は 4 を返す** — `impl FetchResult` の 6 メソッドはインデントされているため落ちる。「crate 外/親モジュールへ出ている項目」を数えるなら 10 が正しい。

**テストだけを兄弟ファイルへ出すなら可視性の変更は 0 件で済む。** 子モジュールは親の private 項目へ `super::` で到達できるため。テストが private を直接触るのは 3 箇所 (5 呼び出し) だけで、いずれも子モジュールから届く。

| テスト    | 触る private 項目                                | 行 (テストモジュール内) |
| --------- | ------------------------------------------------ | ----------------------- |
| `T-FC008` | `format_with_frontmatter`                        | 1193                    |
| `T-FC068` | `pre_handler` / `span_handler` / `table_handler` | 1834-1836               |
| `T-FC090` | `close_self_closed_raw_text_tags` (3 呼び出し)   | 2797 / 2802 / 2807      |

残る 76 本はすべて `to_fetch_result` (`pub(super)`) 経由なので、そもそも private に触らない。

**実装側を割るなら、38 個の private `fn` の多くを `pub(super)` / `pub(in …)` へ広げることになる。** さらに 6 つのヘルパーが関心をまたいで共有されているため、切り出した先へ素直には付いていかない。

呼び出し箇所は **実装 1-985 行から定義行を除いた範囲での出現数** で数える (`grep -oE '\b<name>\('` の hit 数であって、行数ではない)。

| 共有ヘルパー               | 定義行 | 呼び出し箇所 | 使う関心                                                                              |
| -------------------------- | ------ | ------------ | ------------------------------------------------------------------------------------- |
| `element_tag`              | 331    | 8            | PRE / TABLE / SUPPRESS / CELLCODE                                                     |
| `trim_document_whitespace` | 899    | 3            | TABLE / ANCHOR                                                                        |
| `is_suppressed_element`    | 190    | 2            | SUPPRESS / CELLCODE                                                                   |
| `push_text_content`        | 478    | 2            | CELLCODE。うち 1 つは自身への再帰なので、外から呼ぶのは `text_content` の 1 つだけ    |
| `has_ancestor_matching`    | 437    | 2            | PRE (`has_pre_ancestor`) / CELLCODE (`has_table_cell_ancestor`)                       |
| `get_parent`               | 452    | 2            | 上の `has_ancestor_matching` 経由                                                     |

`T-FC095` (2912 行) が SUPPRESS と CELLCODE の結合を実際に踏む — テーブルセルの `<pre>` が `<script>` の中身を落とすことを assert し、その doc コメントが「セルの `<pre>` は walked text ではなく自分の部分木を読むので、同じ抑制を自分で適用しなければならない」と書く。

### 既存の前例との突き合わせ

`src/fetch/cdp/launch.rs` は 282-289 行で 4 本の兄弟テストファイルを宣言する。

```
#[cfg(test)] mod browser_binary_tests;
#[cfg(test)] mod browser_request_tests;
#[cfg(test)] mod cdp_launch_tests;
#[cfg(test)] mod ws_url_parse_tests;
```

**実装は `launch.rs` 1 本のまま、テストだけを 4 関心へ割っている。** `converter.rs` に同じ形を適用するなら実装 985 行は 1 本のまま残り、テスト 2,146 行が関心ごとの `*_tests.rs` になる。

### architect へ渡す E-4 の判断材料 (まとめ)

- テストだけの分割は可視性コスト 0。前例 (`src/fetch/cdp/launch/`) と同形
- そのまま切り出せる塊は 3 つ — `ANCHOR` 10 本/`SUPPRESS` 8 本/`CELLCODE` 9 本。この 3 つは **本数** である。連続区間の長さは順に 9/7/7 本で、残りは冒頭・末尾に 1 本ずつ散っている (`ANCHOR` は `T-FC026` 1128 行、`SUPPRESS` は `T-FC097` 3105 行、`CELLCODE` は `T-FC025` 1093 行と `T-FC098` 3053 行)
- `PRE` 24 本と `TABLE` 16 本は各 4 区間に散っているので、切り出す前に並べ替えが要る
- `BR-WS` 5 本と `CONTAINER-li` 2 本は実装を持たず、htmd 組み込みの挙動を pin する回帰群。関心としては「scout の実装」ではなく「依存ライブラリの契約」なので、1 本の `htmd_behavior_tests.rs` へまとめるのが自然
- 実装側の分割は 38 個の private を広げる作業になり、6 個の共有ヘルパーが関心をまたぐ。**テスト分割とは別の判断として扱うべき**
- 監査文書が言う「切り出す単位を先に決めないとファイルが 6 本に割れる」は、実測では **9 本に割れる** が正しい

---

## `renovate.json` — 3 つの UNCONFIRMED をすべて確定

**3 つとも、ストアが書いたとおりのものが実在した。ただしストアの枠組みに 2 つ誤りがある。**

### 上書き 1 — 「3 規則」ではなく customManager 1 本 + packageRule 3 本

`renovate.json` は 33 行。構成は `$schema`/`extends`/`customManagers` (1 要素)/`packageRules` (3 要素)。**ストアは packageRule のうち 1 本を数え落としている。**

### 上書き 2 — 実効設定はこのファイルだけでは決まらない

2 行目に `"extends": ["github>thkt/renovate-config"]` がある。**共有プリセットは取得していない。** 以下で確定したのは「ローカルに書かれた 4 規則の中身」であって、renovate が最終的に適用する設定の全体ではない。

### 確定した 4 規則

**規則 1 (customManager) — MSRV を regex custom manager で追う。**

| フィールド            | 値                                                     |
| --------------------- | ------------------------------------------------------ |
| `customType`          | `regex`                                                |
| `managerFilePatterns` | `["/^Cargo\\.toml$/"]`                                 |
| `matchStrings`        | `["rust-version\\s*=\\s*\"(?<currentValue>[^\"]+)\""]` |
| `depNameTemplate`     | `rust`                                                 |
| `datasourceTemplate`  | `docker`                                               |
| `versioningTemplate`  | `semver-coerced`                                       |

**この manager は空振りしていない。** `Cargo.toml:5` に `rust-version = "1.97.1"` が実在し、`matchStrings` の正規表現に当たる。したがって規則 1 は依存 `rust` を生成し、規則 2 のラベル付けもその依存に対して発火する。

`Cargo.toml` の `rust-version = "…"` を名前付きキャプチャ `currentValue` で拾う。**datasource が `docker` である点は非自明** — Rust の MSRV を、crates.io ではなく Docker Hub の `rust` イメージのタグ一覧で追う。`semver-coerced` は `1.85` のような 2 桁表記を semver へ寄せるための指定。

**規則 2 (packageRule) — ストアが数え落としていた 1 本。** `matchDepNames: ["rust"]` に `addLabels: ["dependencies", "rust"]` と `commitMessageTopic: "rust-version (MSRV)"` を付ける。規則 1 が作る依存へラベルとコミット見出しを与えるだけで、バージョン制約は無い。

**規則 3 (packageRule) — `htmd` と `markup5ever_rcdom` のグループ化。** `matchManagers: ["cargo"]`、`matchDepNames: ["htmd", "markup5ever_rcdom"]`、`groupName: "htmd + markup5ever_rcdom"`。`description` が理由を書く —「バージョン分裂がコンパイルエラーではなくレビュー可能な更新として届くよう、両 crate を 1 つの PR に載せる」。

**規則 4 (packageRule) — `allowedVersions: "<0.39"` で `markup5ever_rcdom` を固定。** `matchManagers: ["cargo"]`、`matchDepNames: ["markup5ever_rcdom"]`。`description` が **規則 3 だけでは足りない理由** を書く —「上のグループ規則だけではこれを覆えない。renovate は同時に発生した更新を 1 つの PR にまとめるが、更新がある方の crate については単独でも PR を開く。その単独の bump はコンパイルできない」。

### 相互参照は両方向で一致している

規則 4 の description は「`Cargo.toml` の markup5ever_rcdom pin コメントを見よ」と指す。`Cargo.toml:27-31` を確認した。

```
# renovate.json holds this crate at `allowedVersions: "<0.39"` so it cannot be
# raised past htmd on its own. Bump this pin and drop that bound together once
# htmd moves, or renovate stops offering updates for the crate entirely.
markup5ever_rcdom = "0.38"
```

**双方が相手を名指しし、値 (`<0.39` と `"0.38"`) も整合する。** 古い相互参照ではない。`Cargo.toml` 側だけが持つ情報が 1 つある —「htmd が動いたら、この pin の引き上げと `allowedVersions` の除去を同時にやること。さもないと renovate はこの crate の更新を一切提示しなくなる」という解除手順。

---

### Packages Found / Build System / APIs Discovered / Frameworks & Libraries

この FOCUSED 走査はこれらを測り直していない。既存ストアの `technology-stack.md`、`dependencies.md`、`api-documentation.md`、`component-inventory.md` が持つ記述をそのまま維持すること。上の 4 ファイルから、これらの節を変える所見は出ていない。

### Test Coverage

- **Test Directories**: 変更なし。ストアの `code-structure.md` の `## ファイル分類` を維持
- **Test Frameworks**: 今回読んだ範囲で確認できた追加なし。`tracing_test::traced_test` + `logs_contain` (`src/tools/config.rs` の `T-CFG-LOG001`〜`003`)、`wiremock` (`src/slack/client/http_tests.rs`) はストアの記述どおり
- **この走査で加わる実測値**:

| 指標                                  | 値  | 測定範囲                                                                                                                                |
| ------------------------------------- | --- | --------------------------------------------------------------------------------------------------------------------------------------- |
| `src/fetch/converter.rs` のテスト属性 | 79  | 986-3,131 行。すべて `#[test]`、`#[tokio::test]` は 0                                                                                   |
| 同 テスト ID (重複なし)               | 79  | 同上。`grep -oE '\[T-[A-Za-z0-9-]+\]' \| sort -u \| wc -l`                                                                              |
| `src/tools/config.rs` のテスト属性    | 20  | 196-477 行。行頭アンカー付きの `^\s*#\[test\]$` で測定。テスト ID も 20 個で重複なし                                                    |
| 同 `#[tracing_test::traced_test]`     | 3   | 345 / 367 / 463 行。素の `grep 'traced_test'` は 4 hit を返すが、341 行は doc コメントが `traced_test` に言及しているだけで属性ではない |
| 同 `#[cfg(feature = "js-rendering")]` | 1   | 442 行 (`T-CFG025`)                                                                                                                     |

### Code Quality Indicators

この走査で確認した範囲で、ストアの記述を変える所見は 1 件。

- **上書き — `src/slack/client.rs` の 620 行は全部が実装行である。** ストアの `## サイズ分布` は内訳なしで 620 を載せており、`converter.rs` 行の「実装 985 + テスト 2,146」と並べて読むと、620 にもテストが含まれるように見える。実際には inline `mod tests` は無く、テストは兄弟 2 ファイル (計 1,152 行) にある

lint 抑制について: 今回読んだ 4 ファイルで見つかった抑制は 2 個。`src/fetch/converter.rs:110-113` の `#[expect(clippy::needless_pass_by_value, reason = "…")]` と `src/slack/client.rs:611` の `#[expect(dead_code, reason = "field exists for serde, never read back")]`。どちらも `reason` 付きで、ストアの「`#[expect(...)]` には必ず `reason` が付く」と `#[expect]` 8 個という測定に矛盾しない。

### Technical Debt Signals

**4 ファイルすべてで、負債マーカー (TODO / FIXME / HACK / XXX) は 0 件。** ストアの「一般的な負債マーカーは実質存在しない」を、この 4 ファイルについて実測で裏付けた。

E-1/E-3/E-4 はいずれも「文書化されたうえで未着手の判断」であり、放置ではない。この走査でその位置づけが変わったのは 1 件だけである。

| 項目 | 走査前 (ストア)                  | 走査後                                                                                     |
| ---- | -------------------------------- | ------------------------------------------------------------------------------------------ |
| E-1  | 監査文書の引き写し・未確認       | **確定。閾値未達 (3 通りの数え方すべてで 5)。現状維持で正しい**                            |
| E-3  | 監査文書の引き写し・未確認       | **確定。現状維持で正しい。ただし棄却の根拠は監査文書の論拠より `T-SK077` と 6 分岐が強い** |
| E-4  | 6 群・切り出し単位の判断が未着手 | **6 群を 9 関心 26 区間へ上書き。判断そのものは依然未着手だが、材料は揃った**              |

E-2 (`with_clock`/`with_rng` の 4 重複) はこの走査の対象外。ストアが残す「後継 open issue の有無は未確認」はそのまま持ち越す。ただし `src/slack/client.rs:156-164` に該当する `with_clock`/`with_rng` が実在することは確認した (4 箇所のうち 1 箇所)。

---

## Handoff Summary

- **Intent-relevant finding**: ストアが監査文書 E-4 から引き写していた「テスト ID の並びが 6 群に分かれる」は不正確である。`src/fetch/converter.rs` の 986-3,131 行にある 79 本を 1 本ずつ「doc コメントと assertion がどの実装関数を狙っているか」で割り当てると **9 関心・26 連続区間**になる (割り当ては手作業の分類であり、architect は再導出してよい。本数 79 と ID 79 は機械測定)。6 群が名指しする「リスト」は 2 本 (`T-FC024` 1050 行、`T-FC046` 2295 行) しかなく、どちらもリスト変換ではなく `<pre>`/`<br>` がリスト容器の中でどう振る舞うかのテストである。逆に 6 群が名指ししていない関心が 2 つあり、どちらも「リスト」より大きい — テーブルセル内 `<pre>` → inline code span が 9 本 (`T-FC078`/`093`/`094`/`095`/`096`/`079`/`080`/`098`/`025`)、`<br>` と空白の畳み込みが 5 本。監査文書の「切り出す単位を先に決めないとファイルが 6 本に割れる」は、実測では **9 本に割れる** が正しい。

- **Risks / follow-up**:
  1. **E-4 の切り出し可能な塊は 3 つに限られる。** `ANCHOR` (2350-2561、9 本連続)、`SUPPRESS` (2588-2813、7 本連続)、`CELLCODE` (2841-2983、7 本連続) はそのまま出せる。`PRE` 24 本と `TABLE` 16 本は各 4 区間に散っており、出す前に並べ替えが要る。ファイルは追加順に積まれていて ID 番号順でもファイル順でも関心順ではない (`T-FC083` → `T-FC082` → `T-FC020` など)。
  2. **テスト分割と実装分割はコストが 2 桁違う。** テストだけなら可視性の変更は 0 件 (子モジュールが `super::` で private へ届く。private を直接触るテストは `T-FC008` / `T-FC068` / `T-FC090` の 3 本のみで、残り 76 本は `pub(super) fn to_fetch_result` 経由)。実装を割ると素の private `fn` 38 個の多くを広げることになり、さらに 6 個のヘルパーが関心をまたぐ — `element_tag` (呼び出し 8)、`trim_document_whitespace` (3)、`is_suppressed_element` (2)、`push_text_content` (2)、`has_ancestor_matching` (2)、`get_parent` (2)。呼び出し数は実装 1-985 行から定義行を除いた範囲での出現数。前例 `src/fetch/cdp/launch.rs:282-289` は実装 1 本のままテストだけを 4 関心へ割る形なので、これに揃えるのが既存規約に沿う。
  3. **E-3 の棄却理由を CodeKB へ書くときは、監査文書の文言をそのまま使わないこと。** 監査文書は「`ok: false` かつ目的型に合わない本文が `Api` ではなく `Decode` に落ちる」と書くが、そのケースを走らせるテストは無く、本番の目的型 4 種 (`ChannelBody`/`UserBody`/`MessagesBody`/`DummyBody`) はすべて全フィールドが `Option` か `#[serde(default)]` なのでキー欠落では deserialize が失敗しない。より強い根拠が実装にある — `SlackError::Api` は `src/slack.rs:112-176` で 6 通りの `ErrorCode` へ分岐するのに対し `Decode` は `Internal` 一択 (`src/slack.rs:187`) で、`T-SK077` が `internal_error` の retry を実測で pin している。畳めば失うのはラベルではなく 6 分岐と retry である。
  4. **`renovate.json` は「3 規則」ではなく customManager 1 本 + packageRule 3 本。** ストアが数え落としていた 4 本目は `matchDepNames: ["rust"]` + `addLabels: ["dependencies", "rust"]` + `commitMessageTopic: "rust-version (MSRV)"`。また 2 行目の `"extends": ["github>thkt/renovate-config"]` が指す共有プリセットは取得していないので、「4 規則をローカル記述として確定した」とは言えても「実効設定を確定した」とは言えない。この境界を CodeKB の文面に残すこと。
  5. **`src/slack/client.rs` の 620 行は全部が実装行である。** inline `mod tests` は無く、テストは `src/slack/client/constructor_tests.rs` (96 行) と `src/slack/client/http_tests.rs` (1,056 行) にある。ストアの `## サイズ分布` は内訳なしで 620 を載せているため、内訳付きの `converter.rs` 行と並べると誤読を招く。内訳を添えること。
  6. **ストアの shallow リストは `src/` のテスト専用兄弟ファイル 45 本を列挙していない。** そのため `src/slack/client/http_tests.rs` のように実際に証拠として引いたファイルが analyzed にも shallow にも現れない。この走査ではそれらを **証拠の引用元** として扱い、格上げしていない (pre-scan snapshot の 21 パスの外にあるため)。次回の snapshot 設計で扱いを決める必要がある。
  7. **fingerprint の不一致。** `git rev-parse HEAD` = `c8460b59c04b785e7e8378b37bc80504bad2d743` で、ディスパッチが渡した snapshot の source fingerprint `git:498e1629fc5c29351ee88564fe0d3c66c16f5cfe` とは別の導出。突き合わせはこの走査の担当範囲外なので、HEAD の実測値だけを記録した。
