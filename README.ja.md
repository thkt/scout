[English](README.md) | **日本語**

# scout

Web 調査と GitHub リポジトリ探索 — 人間と AI エージェントのどちらでも使えます。読むのは要約ではなく、ソースそのものです。

## 課題

Next.js App Router の認証方式を調べたい場合に、どのような課題があるでしょうか。

| 方法      | 手順                                    | 結果                                                    |
| --------- | --------------------------------------- | ------------------------------------------------------- |
| scoutなし | `curl` でHTML取得、`gh api` でJSON取得… | HTMLの壁、生JSON、ノイズだらけ                          |
| scoutあり | `scout search` / `scout research`       | 生のソースURL一覧、または上位ページ全文をMarkdownで取得 |

```sh
scout search "Next.js App Router authentication"

  https://nextjs.org/docs/.../authentication
  https://authjs.dev/getting-started/installation
  ...
```

```sh
scout research "Next.js App Router authentication best practices" --depth 5

  # Research: Next.js App Router authentication best practices

  ## Fetched Pages
  ### https://nextjs.org/docs/.../authentication
  （要約ではなく、実際のページ内容がMarkdownで返る）
  ...他4ページ...

  ## Sources
  - [Next.js Authentication](https://nextjs.org/docs/...)
  - [Auth.js](https://authjs.dev/...)
```

`search` は Brave Search から取得した生の URL を返し、`research` は上位 N ページをクリーン Markdown で取得します。一次ソースとの間に LLM の要約レイヤーは入りません。

## scoutを使うべき場面（と使わなくていい場面）

| scoutが向いているとき                             | 理由                                          |
| ------------------------------------------------- | --------------------------------------------- |
| 複数ソースにまたがる調査                          | `research` が検索 → 取得 → まとめを一括で行う |
| ページ全文を見たい                                | `fetch` はLLM要約なしの生Markdownを返す       |
| リモートのGitHubリポジトリをcloneせずに探索したい | `repo-tree`、`repo-read`、`repo-overview`     |

| 既存ツールが向いているとき | 理由                                                                     |
| -------------------------- | ------------------------------------------------------------------------ |
| `curl` で十分なとき        | scoutの利点はReadability抽出とSSRF防御なので、不要なら `curl` で事足りる |
| ファイルがローカルにある   | ネットワーク不要                                                         |
| 複雑なブラウザ操作が必要   | SPAのJSレンダリングには対応するが、ログインフローや動的操作には非対応    |

## セットアップ

### インストール

```sh
brew install thkt/tap/scout
```

ソースからビルドする場合は、Rust 1.96+が必要です。

```sh
cargo install --path .
```

ビルド済みバイナリは[Releases](https://github.com/thkt/scout/releases)から入手できます（macOS Apple Silicon / Intel、Linux x86_64 / ARM64）。

### 環境変数

```sh
export BRAVE_SEARCH_API_KEY="..."   # search/researchに必要（無料枠: https://api-dashboard.search.brave.com/）
export GITHUB_TOKEN="..."           # 任意: 5,000回/時 vs 未設定60回/時
export SLACK_TOKEN="..."            # 任意: Slackパーマリンクを `fetch` するときに必要（User OAuthトークン、xoxp-…）
```

`GITHUB_TOKEN` / `GH_TOKEN` / `gh auth token` の順で認証されます。

### チューニング

ビルトインのタイムアウトとリトライ予算を上書きできます。不正な値はリクエストを送る前に終了コード 64（使い方エラー）で失敗します。

| 環境変数                      | デフォルト | 範囲   | 効果                                                                    |
| ----------------------------- | ---------- | ------ | ----------------------------------------------------------------------- |
| `SCOUT_FETCH_TIMEOUT_SECS`    | 95         | 1〜600 | `fetch` のURLごとの実時間予算                                           |
| `SCOUT_RESEARCH_TIMEOUT_SECS` | 45         | 1〜600 | `research` の実時間予算                                                 |
| `SCOUT_SLACK_TIMEOUT_SECS`    | 60         | 1〜600 | Slackパーマリンク `fetch` の実時間予算                                  |
| `SCOUT_GITHUB_TIMEOUT_SECS`   | 180        | 1〜600 | `repo-tree` / `repo-read` / `repo-overview` ごとの実時間予算            |
| `SCOUT_MAX_RETRIES`           | 2          | 0〜10  | 一時的なAPIエラー時のリトライ回数（初回試行に加算、`0` でリトライ無効） |

### オプション: JSレンダリング（SPA対応）

`fetch` は JS 依存ページ（React、Next.js、Vue、Nuxt）を自動検出し、ヘッドレス Chrome（CDP）でレンダリングします。Chrome/Chromium のローカルインストールと `js-rendering` feature が必要です。

```sh
cargo install --path . --features js-rendering
```

### Claude Code連携

プロジェクトの `CLAUDE.md` に追加します。

```markdown
## Tools

- `scout search "query"` — BraveによるWeb検索（URLリスト）
- `scout fetch URL` — WebページをクリーンなMarkdownに変換
- `scout research "query" --depth N` — 複数ソース深掘り調査
- `scout repo-tree owner/repo` — GitHubリポジトリのファイル一覧
- `scout repo-read owner/repo path` — GitHubリポジトリのファイル読み取り
- `scout repo-overview owner/repo` — リポジトリ概要
```

`CLAUDE.md` に記載すると、Claude Code は `WebFetch` や `WebSearch` の代わりに `scout` コマンドを使うようになります。MCP 設定は不要です。

## コマンド

すべてのコマンドはクエリ/URL/リポジトリを位置引数・パイプ入力・対話的 stdin（`-`）のいずれかで受け取れます（例: `echo "クエリ" | scout search`、`scout search -`）。

任意のコマンドに `--json` を付けると、Markdown の代わりに 1 行 JSON エンベロープが返ります。`jq` パイプラインや AI エージェントへの構造化データ受け渡しに便利です。成功時の出力は stdout、エラー時の JSON エンベロープは stderr へ出力されます。

バージョン確認は `scout --version`（または `-V`）、ヘルプは `scout --help` / `scout <command> --help` で表示できます。

### `scout search` — ソースURLを返すWeb検索

Brave Search API で検索し、1 行 1URL で stdout に出力します。Markdown 装飾・要約・回答は含まれません。結果を `scout fetch`（あるいはエージェント側のツール）に渡して実際のソースを読みます。

```sh
scout search "Next.js server actions security"

  https://nextjs.org/docs/...
  https://...
```

```sh
scout search "Rust async runtime" | head -3 | xargs -I _ scout fetch _
```

| フラグ       | 説明                                                                                                            |
| ------------ | --------------------------------------------------------------------------------------------------------------- |
| `-l, --lang` | `ja`、`en`、または `auto`（デフォルト）— Braveの `search_lang` パラメータにマップ（クエリ文字列は書き換えない） |

JSON エンベロープ: `data = {query, sources}`、各 `sources[i] = {url, title, description}`。`description` は検索エンジンのスニペット（LLM 要約ではなく、Brave 側で生成されたもの）。0 件結果時は `sources: []`（`null` ではなく空配列）。

### `scout research` — 複数ソース深掘り調査

Brave で Web 検索し、上位 N ページを取得してレポートにまとめます。ページ全文と URL リストを返します。`search` が URL 一覧のみ返すのに対し、`research` は実際にページを読みに行き全文を含めるため、一次ソースに基づいた判断ができます。

```sh
scout research "Rust async runtime comparison" --depth 5 --lang ja
```

| フラグ        | 説明                                                                    |
| ------------- | ----------------------------------------------------------------------- |
| `-d, --depth` | 取得するページ数（1〜10、デフォルト3）                                  |
| `-l, --lang`  | `ja`、`en`、または `auto`（デフォルト）— Braveの `search_lang` にマップ |

JSON エンベロープ: `data = {query, sources, fetched_pages, failed_urls}`。配列フィールドは空のときも `[]`（`null` ではない）。

### `scout fetch` — WebページをMarkdownに変換

ページをダウンロードし、Readability で本文を抽出して Markdown に変換します。`js-rendering` feature 有効時、JS 依存ページ（SPA）は自動検出しヘッドレス Chrome（CDP）でレンダリングします。LLM は介在しません。

```sh
scout fetch https://react.dev/blog/2024/12/05/react-19
```

| フラグ  | 説明                                                                 |
| ------- | -------------------------------------------------------------------- |
| `--js`  | CDP経由のJSレンダリングを強制（`js-rendering` feature + Chrome必要） |
| `--raw` | Readabilityをスキップしてページ全体を変換                            |

ページのメタデータ（タイトル/著者/日付）は YAML フロントマターとして付与されます。フロントマターブロックは常に出力され、各フィールドはページから取得できた場合に含まれます。

**Slackパーマリンク** — `fetch` は `*.slack.com/archives/{channel}/p{ts}` 形式の URL を検出し、HTML スクレイピングではなく Slack Web API へルーティングします。スレッドの親メッセージとリプライが、著者・タイムスタンプのメタデータ付きで保持されます。`SLACK_TOKEN`（User OAuth トークン、`xoxp-…`）が必要です。

### `scout repo-tree` — リモートファイル一覧

```sh
scout repo-tree denoland/deno --path cli/ --pattern "*.rs"

  denoland/deno (ref: main)
  files: 42

  cli/args.rs (38.2 KB)
  cli/build.rs (1.1 KB)
  ...
```

```sh
# タグ・ブランチ・コミットSHAを指定
scout repo-tree denoland/deno --ref v2.0.0 --path cli/
```

| フラグ       | 説明                              |
| ------------ | --------------------------------- |
| `--ref`      | ブランチ、タグ、またはコミットSHA |
| `-p, --path` | パスプレフィックスでフィルタ      |
| `--pattern`  | ファイル名のglobパターン          |

### `scout repo-read` — リモートファイル読み取り

```sh
scout repo-read facebook/react src/ReactElement.js --lines 1-50
```

```sh
# UTF-8以外のファイルをエンコーディング指定で読む
scout repo-read owner/repo legacy.txt --encoding shift_jis
```

| フラグ        | 説明                                                                                                                                                                                                           |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `--ref`       | ブランチ、タグ、またはコミットSHA                                                                                                                                                                              |
| `-l, --lines` | 行範囲: `1-80`、`50-`、または `100`（先頭N行）                                                                                                                                                                 |
| `--encoding`  | 文字エンコーディング（例: `shift_jis`, `euc-jp`, `gbk`）。省略時はUTF-8・Shift_JIS・EUC-JP・GBK・EUC-KRなどマルチバイトを自動検出。windows-1252・ISO-8859-\*等のシングルバイトは `--encoding` で明示指定が必要 |

### `scout repo-overview` — リポジトリ概要

```sh
scout repo-overview denoland/deno
```

リポジトリのメタデータ、README、オープンな Issue/PR、最近のリリース。リポジトリの存在確認後、残りを並列取得します。

全 GitHub コマンドは `owner/repo`、フル URL（`https://github.com/denoland/deno`）、`.git`付き URL を受け付けます。

## 仕組み

| コマンド | 仕組み                                                                                                                                                         |
| -------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Research | Brave Search を1回呼び出し（`--lang` は `search_lang` にマップ）→ 最大Nページを並行取得（5並列）→ レポート組み立て                                             |
| Fetch    | SSRF多層防御（下記参照）                                                                                                                                       |
| Search   | `GET https://api.search.brave.com/res/v1/web/search` を `X-Subscription-Token` 認証で呼び出し、`web.results[]` を `{url, title, description}` にマップして出力 |
| GitHub   | Git Trees APIでツリー全体を取得 → クライアント側でglobフィルタリング。大きなファイルにはContents APIのblobフォールバック                                       |

### Fetchパイプライン

```text
URL検証 → DNS事前チェック → ダウンロード → リダイレクト後再チェック → Readability → Markdown
```

リテラルなプライベート/ループバック IP は URL 検証時と各リダイレクトホップで、全モード共通に拒否します。既定の直接接続モードでは、scout 自身がホストを解決・接続し、接続時の IP を再検証することで、事前チェックと実接続の間の DNS リバインディングの穴を塞ぎます。標準のプロキシ環境変数（`HTTPS_PROXY` / `HTTP_PROXY`）が設定されている場合は、そのプロキシ経由で fetch します。この場合もリテラルなプライベート/ループバック宛先は各ホップで拒否し続けますが、scout 自身の DNS 解決は行わず、名前解決に基づく防御（DNS リバインディングを含む）はプロキシの egress control へ委譲します。エラーメッセージ中のクレデンシャルは除去します。サイズ上限は Limitations を参照してください。

## アーキテクチャ

```text
src/
├── main.rs              CLIエントリーポイント（clap）
├── tools/               コマンドハンドラー、パラメータ、エラー型
├── search/
│   ├── engine.rs        リサーチエンジン（検索 + 取得 + まとめ）
│   └── lang.rs          Lang → Brave search_lang のマッピング
├── fetch/
│   ├── extractor.rs     Readability記事抽出
│   ├── converter.rs     HTML → Markdown変換
│   └── ssrf.rs          SSRF防御（URL検証、DNS事前チェック）
├── brave/               Brave Search APIクライアントとレスポンス型
├── github/              GitHub APIクライアント（遅延初期化）、ツリーフィルタリング、出力整形
├── slack/               Slackメッセージ取得（スレッド、リプライパーマリンク）
├── envelope.rs          JSON出力エンベロープ
├── markdown.rs          Markdownユーティリティ（見出しシフト、切り詰め、エスケープ）
├── retry.rs             バックオフ付きリトライ（一時エラー、レート制限）
└── redacted.rs          トークン用秘匿ラッパー
```

シングルバイナリで、ランタイム依存はありません。

## 終了コード

[`sysexits.h`](https://man.openbsd.org/sysexits) に GNU coreutils の `timeout` コード（124）、分類不能用の拡張コード（104）、POSIX シグナル規約（128 + シグナル番号）による中断コードを加えた体系です。

| コード | 意味                                                                        |
| ------ | --------------------------------------------------------------------------- |
| 0      | 成功                                                                        |
| 64     | 使い方エラー（clapパース失敗、APIキー未設定、`conflicts_with` 違反）        |
| 65     | データエラー（不正入力、フォーマット異常、エンコーディングエラー、4xx本文） |
| 66     | Not Found（リポジトリ/ファイルが存在しない、404）                           |
| 70     | 内部エラー（scout側の不変条件違反、想定外のレスポンススキーマ）             |
| 74     | IOエラー（ヘッドレスブラウザなど外部ツールの失敗）                          |
| 75     | 一時的失敗（レート制限、5xx、短時間バックオフで再試行可能）                 |
| 104    | 不明（分類不能。発生率上昇は分類カテゴリ不足のシグナル）                    |
| 124    | タイムアウト（リクエスト/転送タイムアウト、より長めのバックオフ推奨）       |
| 130    | SIGINT による中断（128 + 2、例: Ctrl-C）                                    |
| 143    | SIGTERM による中断（128 + 15、例: シェルの timeout、kill デフォルト）       |

## v2への移行ガイド

scout v2.0.0 は検索バックエンドを Gemini Grounding から Brave Search API に切り替えました。env var・出力フォーマット・JSON スキーマすべてが変更されています（破壊的変更）。

**環境変数**

```diff
-export GEMINI_API_KEY="..."
+export BRAVE_SEARCH_API_KEY="..."   # 取得先: https://api-dashboard.search.brave.com/
```

`search` と `research` の両方が `BRAVE_SEARCH_API_KEY` を必要とします。Brave Search の無料枠詳細は制限事項を参照してください。

**`scout search` の出力**

v1 は Gemini が合成した回答と `**Sources:**` Markdown リストを返していましたが、v2 は装飾なしの生 URL を 1 行ずつ出力します。

```diff
- Claude, developed by Anthropic, offers robust capabilities...
- ---
- **Sources:**
- - [Claude Code](https://vertexaisearch.cloud.google.com/grounding-api-redirect/...)
+ https://www.anthropic.com/claude-code
+ https://docs.anthropic.com/...
```

Sources は実際の到達先 URL（Google のリダイレクト経由ではない）になります。

**`scout research` の出力**

`## Search Result` セクション（Gemini が生成した回答を載せていた箇所）は削除されました。`## Fetched Pages`（ページ本文）と `## Sources`（URL リスト）は維持されます。

`research` は Brave Search 自体が retry 後も失敗した場合に hard-fail しなくなりました。代わりに degraded report（`data.sources: []`、fetched pages なし）を返し、`degraded_reasons` に `BraveSearchFailed` を追加するため、呼び出し側はエラーメッセージを parse せずに検索段階の失敗を検知できます。

**`--json` スキーマ**

- `data.answer` は廃止（v1 では Gemini の回答を載せていた）
- `data.sources[i]` は `{url, title, description}` の 3 フィールド（v1 は `{url, title}` の 2 フィールド）。`description` は Brave 検索エンジンのスニペットで、LLM 要約ではない
- `data.fetched_pages` と `data.failed_urls`（research のみ）は形は変わらず、空のときも `[]` を返す（`null` にはならない）

**削除**

- `Lang::apply_to_query`: クエリ末尾に `(日本語で回答)` / `(answer in English)` を追記する動作は廃止。`--lang ja/en` は Brave の `search_lang` パラメータにマップされ、クエリ文字列自体は変更されません
- `--lang auto` のバイリンガル展開: 日本語入力に対する英語クエリの追加発行は廃止。両方必要な場合は呼び出し側で 2 回 `scout` を実行してください

## 制限事項

| 制限                         | 内容                                                                                                                            |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Brave Search APIキーが必要   | `search` と `research` には `BRAVE_SEARCH_API_KEY` が必要。無料枠: $5/月相当の継続クレジット（約1,000クエリ/月）                |
| JSレンダリングにChromeが必要 | `fetch` はSPAを自動検出。`--features js-rendering` でビルドするとヘッドレスChrome（CDP）でJSレンダリング。Chrome/Chromiumが必要 |
| GitHubレート制限             | 未認証: 60回/時。トークンあり: 5,000回/時。`repo-overview` は1回あたり5〜6リクエスト消費                                        |
| 取得サイズ上限               | ダウンロード10MB、出力100Kバイト                                                                                                |

## ライセンス

MIT
