# Architecture — scout

## アーキテクチャスタイル

**単一 crate のレイヤード構成に、trait 注入による test seam を通したもの。** `Cargo.toml` に `[workspace]` セクションは無く、`git show HEAD:Cargo.lock` の `[[package]] name = "scout"` は 1 件である。プロセスは 1 本、デプロイ単位も 1 本で、分散要素は一切ない。

レイヤは 4 段である。

1. **エントリ** — `src/main.rs` (6 行) が `scout::run()` を呼び `ExitCode` を返す
2. **CLI 表面** — `src/lib.rs` が `clap` の `Cli` を解析し、tracing を初期化し、シグナルと JSON envelope の分岐を持つ
3. **ハンドラ層** — `src/tools.rs` の `Scout` が `Command` の 6 分岐をディスパッチし、バックエンドを保持する
4. **バックエンドと横断リーフ** — `fetch`/`github`/`slack`/`brave`/`search` が外部 I/O を担い、`envelope` 以下のリーフが分類・整形・注入点を担う

**この 4 段は一方向ではない。** 4 段目の内側に 1 本だけ逆流する辺があり、それが 2 本の循環を閉じている。実形は次節が持つ。

**外部への公開 API は `pub async fn run() -> ExitCode` の 1 つだけである。** `Cargo.toml` の `unreachable_pub = "deny"` がこれを機械的に固定する。つまり crate の契約面は Rust API ではなく CLI 表面にあり、詳細は `api-documentation.md` が持つ。

### なぜ hexagonal ではなく DI seam なのか

ドメイン層を framework から隔離する完全な ports and adapters は採られていない。代わりに `src/tools.rs` の `Scout` が 12 フィールドを持ち、`Arc<dyn Trait>` 形式の注入点を必要な場所にだけ開ける。この判断は DR-0008 (Test seam architecture via `Arc<dyn Trait>` fields and `ScoutBuilder`) と DR-0009 (Object-safe `DnsResolver` and `Arc<dyn DnsResolver>` injection via `ScoutBuilder`) に記録されている。注入点は時計 (`src/clock.rs`)、乱数 (`src/rng.rs`)、トークン解決 (`src/token_source.rs`)、DNS 解決の 4 種で、いずれも「テストが実時間・実ネットワーク・実資格情報を待たない」ために開かれている。

GitHub と Slack のクライアントは `OnceCell` で遅延初期化される。トークンを必要としないサブコマンドがトークン解決を走らせないためである。

## モジュール依存の実形

**この節が crate 内の依存グラフの一次記録である。** コンポーネント単位の依存先は `component-inventory.md` の各節が、外部 crate への依存は `dependencies.md` が持つ。

### 測定範囲

`src/` の全 95 ファイルから `use crate::…;` 文を全数抽出し (複数行の brace group を含む)、各 import を「ファイルが属するトップレベルモジュール → import 先のトップレベルモジュール」の辺に落とした。同一モジュール内の import は辺にしない。各辺は本番とテスト専用に分けてある。テスト専用の判定は、そのファイルが `#[cfg(test)] mod <name>;` で宣言された兄弟テストファイルであるか、その import が inline の `#[cfg(test)] mod <name> { … }` ブロックの内側にあるかの 2 条件である。

**先行ストアはこの主張を `src/tools.rs` と `src/fetch.rs` の 2 ファイルだけで測り、crate 全体の性質として書いていた。** その 2 ファイルには実際に反証が無い。誤りは測定ではなく、測った範囲より広い主張を書いたことにある。

### 本番の辺は 17 ノード・56 本

出次数のあるモジュールは 10 個で、残り 7 個は crate 内への出辺を持たない終端である。

| 起点           | 本番の import 先                                                                                                       | 出次数 |
| -------------- | ---------------------------------------------------------------------------------------------------------------------- | ------ |
| `tools`        | `brave` `classify` `clock` `envelope` `fetch` `github` `markdown` `retry` `rng` `search` `slack` `token_source` `yaml` | 13     |
| `github`       | `body_limit` `charset` `classify` `clock` `envelope` `markdown` `redacted` `retry` `rng` `token_source` `yaml`         | 11     |
| `slack`        | `body_limit` `classify` `clock` `envelope` `redacted` `retry` `rng` `yaml`                                             | 8      |
| `brave`        | `body_limit` `classify` `clock` `envelope` `redacted` `retry` `rng`                                                    | 7      |
| `fetch`        | `body_limit` `charset` `classify` `envelope` `markdown` `yaml`                                                         | 6      |
| `search`       | `brave` `fetch` `markdown` `yaml`                                                                                      | 4      |
| `classify`     | `envelope` `retry`                                                                                                     | 2      |
| `retry`        | `clock` `rng`                                                                                                          | 2      |
| `yaml`         | `markdown` **`search`**                                                                                                | 2      |
| `token_source` | `redacted`                                                                                                             | 1      |

出辺を持たない 7 モジュールは `body_limit`、`charset`、`clock`、`envelope`、`markdown`、`redacted`、`rng` である。`signals` も出辺を持たないが、入辺の側もこの表に現れない (下の測定の限界を見る)。

### 循環は 2 本あり、どちらも同じ 1 辺を通る

単純閉路を全列挙した結果は次の 2 本だけである。

```
search -> yaml -> search
fetch  -> yaml -> search -> fetch
```

**`yaml → search` の 1 辺を除くと、残る 55 辺は非巡回になる。** したがって層の規則の正しい言い方は「循環が無い」ではなく、**「文書化された派生定数の辺 1 本を除いて非巡回である」** になる。

その 1 辺は `src/yaml.rs` の `use crate::search::engine::MAX_PAGE_BYTES;` で、**両側に理由の doc コメントがある。意図された参照であって事故ではない。**

- `src/search/engine.rs` の `MAX_PAGE_BYTES` の doc コメントが、`pub(crate)` にしている理由を「`yaml::MAX_FIELD_BYTES` が同じページ予算からフィールドごとの上限を導くため」と書く
- `src/yaml.rs` の `MAX_FIELD_BYTES` の doc コメントが、4,500 の 1/10 を選んだ算術を数値で書く (title/author/date の 3 フィールドで 3/10、`escape_yaml` が最悪で倍にするので 6/10、残りが body の取り分)

逆向きの `search → yaml` は `src/search/engine.rs` の `use crate::yaml::truncate_and_reneutralize;` で、`format_fetched_pages` が `truncate_and_reneutralize(&content, MAX_PAGE_BYTES)` を呼ぶ。**同じ 1 つの予算値を、上限を決める側と切る側の両方が参照している。** 3 ノードの閉路も同じ辺を通る。

**この意図を守る仕掛けは何も無い。** `Cargo.toml` の lint にも `clippy.toml` にも CI にも、循環や層の向きを検査するものは 1 つも無い。この点は判断が要る所見として `code-quality-assessment.md` の `## 層の向きに検査点が無い` に置いてある。

### テスト専用の逆向き辺が 2 本ある

本番のグラフには現れず、`cfg(test)` の下にだけ立つ辺で、どちらも層の向きに逆らう。

| 辺               | 出どころ                                                    | 内容                                                             |
| ---------------- | ----------------------------------------------------------- | ---------------------------------------------------------------- |
| `slack → tools`  | `src/slack/client/http_tests.rs`                            | `use crate::tools::ScoutError;`。ハンドラ層へ逆流する            |
| `fetch → search` | `src/fetch/converter.rs` の inline `#[cfg(test)] mod tests` | `use crate::search::engine::MAX_PAGE_BYTES;`。バックエンド間の辺 |

`fetch → search` は本番の辺ではない。`src/fetch/converter.rs` の実装部が持つ `use crate::` は `markdown` と `yaml` の 2 本だけである。このほか `test_support` へ向かうテスト専用辺が 8 モジュールから立つ。`src/test_support.rs` は `#[cfg(test)]` 配下なのでリリースビルドには入らない。

### この測定が覆わない範囲が 3 つある

1. **`src/lib.rs` 発の辺は入らない。** crate root なので `use envelope::{…}` のように `crate::` 接頭辞なしで書く。手で足すと `envelope`・`signals`・`tools` への 3 辺になる。**`signals` はこの 3 辺以外にどこからも参照されないので、`use crate::` だけを見るグラフには 1 度も現れない。**
2. **`use` を経由しないパス参照は入らない。** 実コードでの該当は `src/tools/builder.rs` の `.user_agent(crate::USER_AGENT)` 2 箇所だけで、`tools → crate root` の辺を 1 本足す。残りは可視性修飾子とコメント内の参照である。
3. **非巡回性はトップレベルモジュール粒度での話である。** モジュール内部のファイル間循環は測っていない。

## コンポーネント関係

```mermaid
graph TD
    MAIN["main.rs: 6 lines, calls run"]
    LIB["lib.rs: clap Cli, tracing, signals, JSON envelope"]
    TOOLS["tools: Command dispatch, Scout DI, stdin fallback"]
    FETCH["fetch: URL validate, download, extract, markdown"]
    GH["github: REST v3 client and formatter"]
    SLACK["slack: Web API client, permalink, mention"]
    BRAVE["brave: Search API client"]
    SEARCH["search: research orchestration and report"]
    YAML["yaml: frontmatter emit and neutralize"]
    LEAF["cross-cutting leaves, 11 modules: envelope classify retry body_limit markdown redacted clock rng token_source charset signals"]

    MAIN --> LIB
    LIB --> TOOLS
    LIB --> LEAF
    TOOLS --> FETCH
    TOOLS --> GH
    TOOLS --> SLACK
    TOOLS --> BRAVE
    TOOLS --> SEARCH
    TOOLS --> LEAF
    TOOLS --> YAML
    SEARCH --> BRAVE
    SEARCH --> FETCH
    SEARCH --> LEAF
    SEARCH --> YAML
    FETCH --> LEAF
    FETCH --> YAML
    GH --> LEAF
    GH --> YAML
    SLACK --> LEAF
    SLACK --> YAML
    BRAVE --> LEAF
    YAML --> LEAF
    YAML -->|MAX_PAGE_BYTES| SEARCH
```

<!-- Text fallback: main.rs calls lib.rs; lib.rs drives tools and the cross-cutting leaves; tools dispatches to fetch, github, slack, brave, and search; search calls brave and fetch; every backend depends on the cross-cutting leaves and on yaml. yaml is drawn separately from the other eleven leaves because it carries the one edge that runs back into a backend: yaml imports MAX_PAGE_BYTES from search::engine, closing the cycles search-yaml-search and fetch-yaml-search-fetch. The other eleven leaves have no edge into any backend. -->

**`yaml` を 11 個のリーフから分けて描いてあるのは、この 1 辺を束ねたノードから出すと「リーフがバックエンドを import する」という別の偽の主張になるためである。** 実際に backend への辺を持つリーフは `yaml` 1 つだけで、残る 11 個は持たない。

コンポーネントごとの責務と依存先は `component-inventory.md` が持つ。

## Interaction Diagrams

業務トランザクションがコンポーネントをどう横断するかを 4 本の経路で示す。

### fetch — 単一 URL の取得

SSRF 防御が経路の骨格そのものになっている。検証は 1 回ではなく、redirect のホップごとに繰り返される。

```mermaid
sequenceDiagram
    participant U as User or Agent
    participant T as tools
    participant F as fetch
    participant S as fetch ssrf
    participant D as fetch download
    participant X as fetch extractor
    participant C as fetch converter
    participant E as envelope
    U->>T: scout fetch URL
    T->>F: fetch pipeline entry
    F->>S: validate URL and pre resolve DNS
    S-->>F: ValidatedUrl or FetchError
    F->>D: download with connect time IP guard
    D->>S: re check every redirect hop
    D-->>F: capped body bytes
    F->>X: extract main content
    X-->>F: article HTML
    F->>C: HTML to Markdown
    C-->>F: markdown text
    F-->>T: outcome or FetchError
    T->>E: render success or error
    E-->>U: stdout line and exit code
```

<!-- Text fallback: tools hands the URL to fetch; fetch validates it and pre-resolves DNS through the ssrf module; download applies the connect-time IP guard and re-checks every redirect hop; the body is read under a size cap, extracted by the readability layer, converted to Markdown, and rendered by envelope into a stdout line plus an exit code. -->

判定の閾値は `src/fetch.rs` の `is_js_dependent`/`has_thin_body`/`is_thin_extract` の 3 つが持ち、抽出が薄いときに `js-rendering` 経路へ落ちる契機になる。エラーは `FetchError` の 14 variant で表され、`classify()` のアーム順が終了コードへの写像を決める (DR-0003, DR-0011)。

### research — 検索と並列取得の合成

```mermaid
sequenceDiagram
    participant U as User or Agent
    participant T as tools
    participant SE as search engine
    participant B as brave client
    participant F as fetch
    participant E as envelope
    U->>T: scout research QUERY
    T->>SE: run research
    SE->>B: web search one endpoint
    B-->>SE: result URLs
    SE->>F: buffer_unordered fan out over URLs
    F-->>SE: markdown per URL or DegradedReason
    SE-->>T: report plus degraded reasons
    T->>E: render success with degraded flags
    E-->>U: report or JSON envelope
```

<!-- Text fallback: research asks brave for result URLs, then fans out over them with futures buffer_unordered calling the same fetch pipeline; per-URL failures come back as DegradedReason values rather than aborting the run, and envelope reports them in the degraded_reasons array. -->

**部分失敗が正常系である。** 1 本の URL が落ちても run 全体は成功として返り、落ちた理由が `degraded_reasons` に載る。これが `DegradedReason` 14 variant の存在理由で、DR-0003 に記録されている。並列度は `src/search/engine.rs` が `futures` の `stream::buffer_unordered(5)` で与える。

**この経路がページごとに切る上限が `MAX_PAGE_BYTES` (4,500) である。** 前節の循環辺はこの値をめぐるもので、`format_fetched_pages` が `yaml::truncate_and_reneutralize` へ渡し、`yaml` 側は同じ値から frontmatter のフィールド上限を導く。

### repo-read — GitHub 単一ファイルの復号

```mermaid
sequenceDiagram
    participant U as User or Agent
    participant T as tools
    participant G as github client
    participant EN as github encoding
    participant FM as github format
    participant E as envelope
    U->>T: scout repo-read owner repo path
    T->>G: GET contents endpoint
    G-->>T: ContentsPayload untagged two arms
    T->>EN: base64 decode then charset decode
    EN-->>T: decoded text
    T->>FM: format with README byte cap
    FM-->>T: markdown
    T->>E: render success
    E-->>U: stdout line and exit code
```

<!-- Text fallback: repo-read calls the GitHub contents endpoint; the untagged ContentsPayload distinguishes a file from a directory; the body is base64-decoded then charset-decoded, formatted under the README byte cap, and rendered through envelope. -->

`ContentsPayload` の `untagged` 2 arm は `src/github/types.rs` にあり、ディレクトリ側を `Vec<IgnoredAny>` にした理由がコメントに残る — 素の `IgnoredAny` にすると、エラーオブジェクトも文字列も `null` も同じアームへ落ち、caller が全部 `PathIsDirectory` に潰した。出力スキーマと README のバイト上限は DR-0016 が定める。

### js-rendering — CDP 経路と SOCKS5 proxy

`js-rendering` feature が有効なときだけコンパイルされる経路である。既定は無効。

```mermaid
sequenceDiagram
    participant F as fetch
    participant L as cdp launch
    participant P as cdp proxy SOCKS5 on loopback
    participant CR as chromium process group
    participant W as target site
    F->>L: extraction was thin, escalate
    L->>P: spawn ssrf proxy
    L->>CR: spawn chromium with proxy server and bypass list
    CR-->>L: DevTools listening ws URL on stderr
    L-->>F: browser connected
    F->>CR: Page.navigate
    CR->>P: SOCKS5 CONNECT target
    P->>P: resolve once and check first blocked IP
    P->>W: dial and tunnel, else REP NOT ALLOWED
    W-->>CR: rendered DOM
    CR-->>F: outer HTML
    F->>L: reap pgroup SIGTERM then SIGKILL
```

<!-- Text fallback: when extraction is thin, fetch launches a loopback SOCKS5 proxy and a chromium process group pointed at it; the proxy resolves the CONNECT target once and fails closed if any resolved address is private; the rendered DOM returns over CDP and the process group is reaped with SIGTERM followed by SIGKILL. -->

この経路が SSRF 防御に穴を開けない仕組みは 3 つのフラグに載っている。`src/fetch/cdp/launch.rs` の `build_launch_args` が返す 12 フラグのうち `--proxy-server=socks5://127.0.0.1:{port}`/`--proxy-bypass-list=<-loopback>`/`--disable-quic` の 3 つに DR-0021 を引く根拠コメントが付く。`--proxy-bypass-list` は chromium の implicit bypass (loopback と 169.254/16 の IMDS) を subtract する。`src/fetch/cdp/proxy.rs` の `handle_conn` は CONNECT target を 1 回だけ解決し、`first_blocked_ip` が private を 1 つでも返せば `REP_NOT_ALLOWED` で fail-closed する。

## データフローの共通形

4 経路すべてが同じ 3 段を通る。

| 段                   | 担当                                                                    | 決めていること                                                                                                            |
| -------------------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| 入力の検証           | `fetch/ssrf.rs`、`github/helpers.rs`、`slack/url.rs`、`brave/client.rs` | 外部へ出る前に宛先を型で確定させる。`ValidatedUrl` がこの型                                                               |
| 実行とリトライ       | `src/retry.rs`、各クライアント                                          | `Retry-After` の解釈を 1 箇所へ集約する (DR-0006)。バックオフのジッタは `Rng` trait 経由                                  |
| 出力の中和と封筒詰め | `src/markdown.rs`、`src/yaml.rs`、`src/envelope.rs`、`src/classify.rs`  | エージェント向けの注入防御 (DR-0014) を通し、成功/失敗を envelope と終了コードへ写す (DR-0002, DR-0003, DR-0010, DR-0017) |

秘密はこの流れの外側にある。`src/redacted.rs` の `Redacted` 型が保持し、`Display` を通さないことで出力経路へ落ちない (DR-0015)。

## 主要な設計判断 (DR 索引)

決定の本文・却下した選択肢・帰結は `docs/decisions/` にある。ここは「どの関心がどの DR に決まっているか」だけを持つ。索引表は `docs/decisions/README.md`。

| 関心                                                      | DR               |
| --------------------------------------------------------- | ---------------- |
| SSRF 防御の構造と `fetch.rs` のモジュール分割             | 0001             |
| 終了コード体系 (sysexits.h + coreutils + POSIX 128+signo) | 0002, 0017       |
| エラー分類の契約と優先順位                                | 0003, 0011       |
| GitHub クライアントの振る舞い上限と出力スキーマ           | 0004, 0016       |
| 検索バックエンドの選択と初期化                            | 0005, 0007, 0020 |
| リトライ遅延の集約                                        | 0006             |
| test seam (trait 注入と `ScoutBuilder`)                   | 0008, 0009       |
| JSON envelope の契約                                      | 0010             |
| DNS rebinding に対する connect 時 IP guard                | 0012             |
| 文字コード判定とデコード方針                              | 0013             |
| エージェント向け出力注入防御                              | 0014             |
| 秘密の型封じ込め                                          | 0015             |
| トークン解決の優先順位と漏洩封じ込め                      | 0018             |
| 環境変数の検証とタイムアウト階層                          | 0019             |
| CDP chromium の起動 egress フラグ                         | 0021             |
| Slack user token の prefix 検証                           | 0022             |
| proxy 経由経路での防御委譲                                | 0023             |
| 外部前提ごとのテスト skip                                 | 0024             |
| `<pre>` / `<br>` と改行の扱い                             | 0025, 0026, 0027 |
| DR からコードを指す参照の形                               | 0028             |

**DR-0012 のタイトルは `with CDP Path Asymmetry` で終わるが、その非対称は現在解消済みである。** 同 DR の Addendum (issue #201、2026-06-16 close) が loopback SOCKS5 proxy 方式で穴を塞いだことを記録し、OUTCOME Constraint の carve-out 文言も置換済みである。DR 本文の Consequences 箇条書きは MADR の慣行どおり決定時点の帰結を残しているだけで、未解決の課題ではない。

**設計判断の索引に無い判断が 1 つある。** 上の循環辺は両側の doc コメントに理由が書かれているが、DR も lint も持たない。この扱いは `code-quality-assessment.md` の `## 層の向きに検査点が無い` が持つ。

## 改善余地

構造上の負債は 1 点に集中している。詳細と現状の判断は `code-quality-assessment.md` が持つ。

- **`src/fetch/converter.rs` が 3,131 行** — 実装 985 行と `#[cfg(test)] mod tests` 2,146 行。このリポジトリ自身の「1 ファイルのテストが 2 つ以上の関心を持ったら分ける」規約から外れる唯一の実装ファイルである。**テスト 79 本が分かれる関心は 6 つではなく 9 つで、ファイル順では 26 の連続区間に散っている** (先行資料の「6 群」を上書きした。内訳は `code-structure.md` の `## サイズ分布`)。切り出す単位の判断は依然として未着手だが、判断に要る材料は揃っている
- **`with_clock` / `with_rng` が 4 クライアントに同形で並ぶ** — `github.rs`/`brave/client.rs`/`slack/client.rs`/`tools/builder.rs`。共通化は実測のうえ棄却済みで、再検討の着手条件が closed issue #310 の Backlog candidates の中にしか無い

いずれも「知らないまま放置している」のではなく「測って現状維持を選んだ」判断であり、再検討の閾値が文書に残っている。監査文書 E-1 (`src/tools/config.rs` の `surface_overrides`) と E-3 (`src/slack/client.rs` の `api_get_once`) は、先行する走査が該当ファイルを開いて決着させ、どちらも現状維持が正しいことを実測で裏付けた。
