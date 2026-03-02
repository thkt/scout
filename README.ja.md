# scout

AIエージェントのためのWeb調査とGitHubリポジトリ探索。エージェントが読むのは要約ではなく、ソースそのもの。

## 課題

エージェントにNext.js App Routerの認証方式を調べさせたい。

**scoutなしの場合:**

```
WebSearch("Next.js App Router authentication")  → リンクとスニペット。
WebFetch(url, prompt="Explain the auth approaches")  → LLMによる要約。ページを見る前にプロンプトを書く。
```

これでも動く。ただし、すべてのページがWebFetchのプロンプトを通る。ページの中身を知る前に書いた非可逆フィルタだ。エージェントはソースではなく、要約を元に推論する。

**scoutありの場合:**

```
research("Next.js App Router authentication best practices", depth=5)

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

エージェントはGoogle検索に基づく回答と、5つのソースページのMarkdownを受け取る。取得したコンテンツにLLMは介在しない。一次ソースを直接読み、何が重要かをエージェント自身が判断する。

ビルトインツールにできないことではない。ただし、各ステップで情報が落ちる。scoutはその損失をなくす。

日本語クエリは自動で処理される。「Next.js 認証 ベストプラクティス」は日本語のまま検索されると同時に、技術用語を抽出した英語クエリにも展開される。英語しかないドキュメントも取りこぼさない。

## scoutを使うべき場面（と使わなくていい場面）

**scoutが向いているとき:**

- 複数ソースにまたがる調査 — `research` が検索 → 取得 → まとめを一括で行う
- エージェントにページ全文を見せたい — `fetch` はLLM要約なしの生Markdownを返す
- リモートのGitHubリポジトリをcloneせずに探索したい — `repo_tree`、`repo_read`、`repo_overview`

**ビルトインツールが向いているとき:**

- 特定ページに特定の質問がある — WebFetchのpromptパラメータはそのためのもの
- 軽い検索で十分 — WebSearchの方が軽量
- ファイルがローカルにある — `Read` はネットワーク不要

## セットアップ

### インストール

```sh
brew install thkt/tap/scout
```

ソースからビルドする場合（Rust 1.85+が必要）:

```sh
cargo build --release
```

ビルド済みバイナリは[Releases](https://github.com/thkt/scout/releases)から入手可能 — macOS (Apple Silicon / Intel)、Linux (x86_64 / ARM64)。

### 設定

```sh
claude mcp add scout -- scout
```

または `~/.claude/.mcp.json` に直接記述:

```json
{
  "mcpServers": {
    "scout": {
      "command": "scout",
      "env": {
        "GEMINI_API_KEY": "${GEMINI_API_KEY}"
      }
    }
  }
}
```

`search` と `research` には [Gemini APIキー](https://aistudio.google.com/apikey)が必要（無料枠で動作）。`fetch` とGitHubツールはキーなしで使える。

GitHubのレート制限を上げるには `GITHUB_TOKEN` または `GH_TOKEN` を設定（5,000/時。未設定だと60/時）。`gh auth token` へのフォールバックあり。

## ツール

### `research` — 複数ソース深掘り調査

Gemini Groundingで検索し、上位Nページを取得してレポートにまとめる。回答・ページ全文・ソースリストを一括で返す。

- `depth`: 取得するページ数（1〜10、デフォルト3）
- `lang`: `"ja"`、`"en"`、または `"auto"`（デフォルト）— 日本語を検出すると日英両方のクエリに自動展開

### `search` — ソース付きWeb検索

Gemini Grounding + Google検索。リンク一覧ではなく、ソースURL付きの合成回答を返す。

### `fetch` — WebページをMarkdownに変換

ページをダウンロードし、Readabilityで本文を抽出してMarkdownに変換。LLMは介在しない。

- `raw`: Readabilityをスキップしてページ全体を変換
- `meta`: タイトル/著者/日付をYAMLフロントマターで付与

### `repo_tree` — リモートファイル一覧

GitHubリポジトリのファイルをパスプレフィックスとglobパターンでフィルタリングして一覧表示。

```
repo_tree("denoland/deno", path="cli/", pattern="*.rs")

  denoland/deno (ref: main)
  files: 42

  cli/args.rs (38.2 KB)
  cli/build.rs (1.1 KB)
  ...
```

### `repo_read` — リモートファイル読み取り

行範囲を指定して読み取り可能（`"1-80"`、`"50-"`、`"100"`）。大きなファイルはgit blob APIにフォールバック。base64デコード不要。

### `repo_overview` — リポジトリ概要

リポジトリのメタデータ、README、オープンなIssue/PR、最近のリリースを5つのAPIコールを並行実行して一括取得。

全GitHubツールは `owner/repo`、フルURL（`https://github.com/denoland/deno`）、`.git`付きURLを受け付ける。

## 仕組み

**Research** — Gemini Grounding検索（日本語クエリはバイリンガル展開）を実行し、ソースURLを収集、最大Nページを並行取得（5並列）してレポートを組み立てる。

**Fetch** — SSRF多層防御:

```
URL検証 → DNS事前チェック → ダウンロード → リダイレクト後再チェック → Readability → Markdown
```

プライベート/ループバックIPはDNS解決とリダイレクトの両段階でブロック。エラーメッセージ中のクレデンシャルは除去。ダウンロード上限10MB、出力上限10万文字。

**Search** — Gemini `generateContent` に `google_search` グラウンディングツールを有効化。レスポンスにはAI生成回答とGoogle検索から抽出されたソースURLの両方が含まれる。

**GitHub** — Git Trees APIでツリー全体を取得し、クライアント側でglobフィルタリング。Contents APIに大きなファイル用のblobフォールバック付き。

## アーキテクチャ

```
src/
├── main.rs              MCPサーバー（stdioトランスポート）
├── tools/               ツールハンドラーとパラメータ定義
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
- **GitHubレート制限** — 未認証: 60回/時。トークンあり: 5,000回/時。`repo_overview` は1回あたり5リクエスト消費。
- **取得サイズ上限** — ダウンロード10MB、出力10万文字。

## ライセンス

MIT
