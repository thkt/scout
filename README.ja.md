[English](README.md) | **日本語**

# scout

Web調査とGitHubリポジトリ探索 — 人間とAIエージェントのどちらでも使えます。読むのは要約ではなく、ソースそのものです。

## 課題

Next.js App Routerの認証方式を調べたい場合に、どのような課題があるでしょうか。

| 方法      | 手順                                    | 結果                                                    |
| --------- | --------------------------------------- | ------------------------------------------------------- |
| scoutなし | `curl` でHTML取得、`gh api` でJSON取得… | HTMLの壁、生JSON、ノイズだらけ                          |
| scoutあり | `scout research "クエリ"` 一発          | Google検索ベースの回答 + ソースページを生Markdownで取得 |

```sh
scout research "Next.js App Router authentication best practices" --depth 5

  Google検索に裏付けされた回答（ソースURL付き）...

  ## Fetched Pages
  ### https://nextjs.org/docs/.../authentication
  （要約ではなく、実際のページ内容がMarkdownで返る）
  ...他4ページ...

  ## Sources
  - [Next.js Authentication](https://nextjs.org/docs/...)
  - [Auth.js](https://authjs.dev/...)
```

LLMは介在せず、一次ソースを直接読んで何が重要かを自分で判断できます。

日本語クエリは自動で処理されます。「Next.js認証ベストプラクティス」は日本語のまま検索しつつ、技術用語を抽出した英語クエリにも展開するため、英語しかないドキュメントも取りこぼしません。

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

ソースからビルドする場合は、Rust 1.85+が必要です。

```sh
cargo install --path .
```

ビルド済みバイナリは[Releases](https://github.com/thkt/scout/releases)から入手できます（macOS Apple Silicon / Intel、Linux x86_64 / ARM64）。

### 環境変数

```sh
export GEMINI_API_KEY="..."   # search/researchに必要（無料枠: https://aistudio.google.com/apikey）
export GITHUB_TOKEN="..."     # 任意: 5,000回/時 vs 未設定60回/時
```

`GITHUB_TOKEN` / `GH_TOKEN` / `gh auth token` の順で認証されます。

### オプション: JSレンダリング（SPA対応）

`fetch` はJS依存ページ（React、Next.js、Vue、Nuxt）を自動検出し `playwright-cli` にフォールバックします。`npx` 経由でそのまま動きますが、グローバルインストールすると高速です。

```sh
npm install -g @playwright/cli
```

### Claude Code連携

プロジェクトの `CLAUDE.md` に追加します。

```markdown
## Tools

- `scout search "query"` — Gemini GroundingによるWeb検索
- `scout fetch URL` — WebページをクリーンなMarkdownに変換
- `scout research "query" --depth N` — 複数ソース深掘り調査
- `scout repo-tree owner/repo` — GitHubリポジトリのファイル一覧
- `scout repo-read owner/repo path` — GitHubリポジトリのファイル読み取り
- `scout repo-overview owner/repo` — リポジトリ概要
```

`CLAUDE.md` に記載すると、Claude Codeは `WebFetch` や `WebSearch` の代わりに `scout` コマンドを使うようになります。MCP設定は不要です。

## コマンド

### `scout research` — 複数ソース深掘り調査

Gemini Groundingで検索し、上位Nページを取得してレポートにまとめます。回答・ページ全文・ソースリストを一括で返します。

```sh
scout research "Rust async runtime comparison" --depth 5 --lang ja
```

| フラグ        | 説明                                                                                  |
| ------------- | ------------------------------------------------------------------------------------- |
| `-d, --depth` | 取得するページ数（1〜10、デフォルト3）                                                |
| `-l, --lang`  | `ja`、`en`、または `auto`（デフォルト）— 日本語を検出すると日英両方のクエリに自動展開 |

### `scout search` — ソース付きWeb検索

Gemini GroundingとGoogle検索で、リンク一覧ではなくソースURL付きの合成回答を返します。

```sh
scout search "Next.js server actions security"
```

### `scout fetch` — WebページをMarkdownに変換

ページをダウンロードし、Readabilityで本文を抽出してMarkdownに変換します。JS依存ページ（SPA）は自動検出し `playwright-cli` でレンダリングします。LLMは介在しません。

```sh
scout fetch https://react.dev/blog/2024/12/05/react-19
```

| フラグ  | 説明                                                      |
| ------- | --------------------------------------------------------- |
| `--js`  | playwright-cliによるJSレンダリングを強制（SPAは自動検出） |
| `--raw` | Readabilityをスキップしてページ全体を変換                 |

ページのメタデータ（タイトル/著者/日付）はYAMLフロントマターとして付与されます。フロントマターブロックは常に出力され、各フィールドはページから取得できた場合に含まれます。

### `scout repo-tree` — リモートファイル一覧

```sh
scout repo-tree denoland/deno --path cli/ --pattern "*.rs"

  denoland/deno (ref: main)
  files: 42

  cli/args.rs (38.2 KB)
  cli/build.rs (1.1 KB)
  ...
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

| フラグ        | 説明                                           |
| ------------- | ---------------------------------------------- |
| `--ref`       | ブランチ、タグ、またはコミットSHA              |
| `-l, --lines` | 行範囲: `1-80`、`50-`、または `100`（先頭N行） |

### `scout repo-overview` — リポジトリ概要

```sh
scout repo-overview denoland/deno
```

リポジトリのメタデータ、README、オープンなIssue/PR、最近のリリースを5つのAPIコールで並行取得します。

全GitHubコマンドは `owner/repo`、フルURL（`https://github.com/denoland/deno`）、`.git`付きURLを受け付けます。

## 仕組み

| コマンド | 仕組み                                                                                                                    |
| -------- | ------------------------------------------------------------------------------------------------------------------------- |
| Research | Gemini Grounding検索（日本語クエリはバイリンガル展開）→ ソースURL収集 → 最大Nページを並行取得（5並列） → レポート組み立て |
| Fetch    | SSRF多層防御（下記参照）                                                                                                  |
| Search   | Gemini `generateContent` に `google_search` グラウンディングツールを有効化し、AI生成回答とソースURLの両方を返す           |
| GitHub   | Git Trees APIでツリー全体を取得 → クライアント側でglobフィルタリング。大きなファイルにはContents APIのblobフォールバック  |

### Fetchパイプライン

```text
URL検証 → DNS事前チェック → ダウンロード → リダイレクト後再チェック → Readability → Markdown
```

プライベート/ループバックIPはDNS解決とリダイレクトの両段階でブロックし、エラーメッセージ中のクレデンシャルも除去します。

## アーキテクチャ

```text
src/
├── main.rs              CLIエントリーポイント（clap）
├── tools/               コマンドハンドラー、パラメータ、エラー型
├── search/
│   ├── engine.rs        リサーチエンジン（検索 + 取得 + まとめ）
│   └── bilingual.rs     日英クエリ展開
├── fetch/
│   ├── extractor.rs     Readability記事抽出
│   ├── converter.rs     HTML → Markdown変換
│   └── ssrf.rs          SSRF防御（URL検証、DNS事前チェック）
├── gemini/              Gemini APIクライアント、グラウンディングレスポンス解析
├── github/              GitHub APIクライアント、ツリーフィルタリング、出力整形
└── markdown.rs          Markdownユーティリティ
```

シングルバイナリで、ランタイム依存はありません。

## 制限事項

| 制限                          | 内容                                                                                                                 |
| ----------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| Gemini APIキーが必要          | `search` と `research` には `GEMINI_API_KEY` が必要。無料枠: 100 RPM、1,500回/日                                     |
| JSレンダリングにNode.jsが必要 | `fetch` はSPAを自動検出し `playwright-cli` でJSレンダリングする。グローバル未インストール時は `npx`（Node.js）が必要 |
| GitHubレート制限              | 未認証: 60回/時。トークンあり: 5,000回/時。`repo-overview` は1回あたり5リクエスト消費                                |
| 取得サイズ上限                | ダウンロード10MB、出力100Kバイト                                                                                     |

## ライセンス

MIT
