# scout

Web調査とGitHubリポジトリ探索 — 人間にもAIエージェントにも。読むのは要約ではなく、ソースそのもの。

## 課題

Next.js App Routerの認証方式を調べたい。

**scoutなしの場合:**

```
curl https://nextjs.org/docs/.../authentication | # HTMLの壁
gh api /repos/vercel/next.js/... | # 生JSON
```

複数ツール、バラバラのフォーマット、ノイズだらけ。

**scoutありの場合:**

```sh
scout research "Next.js App Router authentication best practices" --depth 5

  Google検索に裏付けされた回答（ソースURL付き）...

  ## Fetched Pages
  ### https://nextjs.org/docs/.../authentication
  （要約ではなく、実際のページ内容がMarkdownで返る）

  ### https://authjs.dev/getting-started/installation
  （実際のページ内容がMarkdownで返る）

  ...他3ページ...

  ## Sources
  - [Next.js Authentication](https://nextjs.org/docs/...)
  - [Auth.js](https://authjs.dev/...)
  - ...
```

コマンド一発で、Google検索に基づく回答と5つのソースページのMarkdownを取得。取得したコンテンツにLLMは介在しない。一次ソースを直接読み、何が重要かを自分で判断する。

日本語クエリは自動で処理される。「Next.js 認証 ベストプラクティス」は日本語のまま検索されると同時に、技術用語を抽出した英語クエリにも展開される。英語しかないドキュメントも取りこぼさない。

## scoutを使うべき場面（と使わなくていい場面）

**scoutが向いているとき:**

- 複数ソースにまたがる調査 — `research` が検索 → 取得 → まとめを一括で行う
- ページ全文を見たい — `fetch` はLLM要約なしの生Markdownを返す
- リモートのGitHubリポジトリをcloneせずに探索したい — `repo-tree`、`repo-read`、`repo-overview`

**既存ツールが向いているとき:**

- `curl` で十分 — scoutはReadability抽出とSSRF防御を追加する
- ファイルがローカルにある — ネットワーク不要
- JSレンダリングが必要 — scoutは静的HTMLのみ取得

## セットアップ

### インストール

```sh
brew install thkt/tap/scout
```

ソースからビルドする場合（Rust 1.85+が必要）:

```sh
cargo install --path .
```

ビルド済みバイナリは[Releases](https://github.com/thkt/scout/releases)から入手可能 — macOS (Apple Silicon / Intel)、Linux (x86_64 / ARM64)。

### 環境変数

```sh
export GEMINI_API_KEY="..."   # search/researchに必要（無料枠: https://aistudio.google.com/apikey）
export GITHUB_TOKEN="..."     # 任意: 5,000回/時 vs 未設定60回/時
```

`GITHUB_TOKEN` / `GH_TOKEN` / `gh auth token` の順で認証される。

### Claude Code連携

プロジェクトの `CLAUDE.md` に追加:

```markdown
## Tools

- `scout search "query"` — Gemini GroundingによるWeb検索
- `scout fetch URL` — WebページをクリーンなMarkdownに変換
- `scout research "query" --depth N` — 複数ソース深掘り調査
- `scout repo-tree owner/repo` — GitHubリポジトリのファイル一覧
- `scout repo-read owner/repo path` — GitHubリポジトリのファイル読み取り
- `scout repo-overview owner/repo` — リポジトリ概要
```

Claude Codeはコマンドを自然に認識する。MCP設定は不要。

## コマンド

### `scout research` — 複数ソース深掘り調査

Gemini Groundingで検索し、上位Nページを取得してレポートにまとめる。回答・ページ全文・ソースリストを一括で返す。

```sh
scout research "Rust async runtime comparison" --depth 5 --lang ja
```

| フラグ        | 説明                                                                                  |
| ------------- | ------------------------------------------------------------------------------------- |
| `-d, --depth` | 取得するページ数（1〜10、デフォルト3）                                                |
| `-l, --lang`  | `ja`、`en`、または `auto`（デフォルト）— 日本語を検出すると日英両方のクエリに自動展開 |

### `scout search` — ソース付きWeb検索

Gemini Grounding + Google検索。リンク一覧ではなく、ソースURL付きの合成回答を返す。

```sh
scout search "Next.js server actions security"
```

### `scout fetch` — WebページをMarkdownに変換

ページをダウンロードし、Readabilityで本文を抽出してMarkdownに変換。LLMは介在しない。

```sh
scout fetch https://react.dev/blog/2024/12/05/react-19 --meta
```

| フラグ   | 説明                                         |
| -------- | -------------------------------------------- |
| `--raw`  | Readabilityをスキップしてページ全体を変換    |
| `--meta` | タイトル/著者/日付をYAMLフロントマターで付与 |

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

リポジトリのメタデータ、README、オープンなIssue/PR、最近のリリースを5つのAPIコールを並行実行して一括取得。

全GitHubコマンドは `owner/repo`、フルURL（`https://github.com/denoland/deno`）、`.git`付きURLを受け付ける。

## 仕組み

**Research** — Gemini Grounding検索（日本語クエリはバイリンガル展開）を実行し、ソースURLを収集、最大Nページを並行取得（5並列）してレポートを組み立てる。

**Fetch** — SSRF多層防御:

```
URL検証 → DNS事前チェック → ダウンロード → リダイレクト後再チェック → Readability → Markdown
```

プライベート/ループバックIPはDNS解決とリダイレクトの両段階でブロック。エラーメッセージ中のクレデンシャルは除去。ダウンロード上限10MB、出力上限100Kバイト。

**Search** — Gemini `generateContent` に `google_search` グラウンディングツールを有効化。レスポンスにはAI生成回答とGoogle検索から抽出されたソースURLの両方が含まれる。

**GitHub** — Git Trees APIでツリー全体を取得し、クライアント側でglobフィルタリング。Contents APIに大きなファイル用のblobフォールバック付き。

## アーキテクチャ

```
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

シングルバイナリ、ランタイム依存なし。

## 制限事項

- **Gemini APIキーが必要** — `search` と `research` には `GEMINI_API_KEY` が必要。無料枠: 100 RPM、1,500回/日。
- **JavaScriptレンダリング非対応** — `fetch` は静的HTMLをダウンロードする。クライアントサイドレンダリングが必要なSPAでは最小限のコンテンツしか返らない。
- **GitHubレート制限** — 未認証: 60回/時。トークンあり: 5,000回/時。`repo-overview` は1回あたり5リクエスト消費。
- **取得サイズ上限** — ダウンロード10MB、出力100Kバイト。

## ライセンス

MIT
