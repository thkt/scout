# scout

Web research and GitHub exploration for AI agents. Your agent reads the sources, not a summary of the sources.

[日本語版](README.ja.md)

## The problem

An agent needs to research how Next.js App Router handles authentication.

**Without scout:**

```
WebSearch("Next.js App Router authentication")  → Links and snippets.
WebFetch(url, prompt="Explain the auth approaches")  → LLM summary. Prompt written before seeing the page.
```

This works. But every page goes through WebFetch's prompt — a lossy filter written before the agent knows what's on the page. The agent reasons from summaries, not from the source material.

**With scout:**

```
research("Next.js App Router authentication best practices", depth=5)

  Grounded answer with citations...

  ## Fetched Pages
  ### https://nextjs.org/docs/.../authentication
  (actual page content as Markdown — not a summary)

  ### https://authjs.dev/getting-started/installation
  (actual page content as Markdown)

  ...3 more pages...

  ## Sources
  - [Next.js Authentication](https://nextjs.org/docs/...)
  - [Auth.js](https://authjs.dev/...)
  - ...
```

The agent gets a grounded answer from Google Search, plus 5 source pages as Markdown. No LLM intermediary on the fetched content — your agent reads the primary sources and decides what matters.

This isn't something built-in tools can't do at all. It's something they do with information loss at each step. scout removes that loss.

Japanese queries are handled automatically: "Next.js 認証 ベストプラクティス" expands to both the original and an English query extracted from the technical terms, so English-only documentation isn't missed.

## When to use scout (and when not to)

**Use scout when:**

- You need to investigate a topic across multiple sources — `research` does the search → fetch → compile loop for you
- You want the agent to see full page content, not an LLM summary — `fetch` returns raw Markdown
- You need to explore a remote GitHub repo without cloning — `repo_tree`, `repo_read`, `repo_overview`

**Use built-in tools when:**

- You have a specific question about a specific page — WebFetch's prompt parameter is designed for that
- A quick lookup is enough — WebSearch is lighter than `search`
- The file is already on disk — `Read` needs no network

## Setup

### Install

```sh
brew install thkt/tap/scout
```

Or build from source (requires Rust 1.85+):

```sh
cargo build --release
```

Pre-built binaries in [Releases](https://github.com/thkt/scout/releases) — macOS (Apple Silicon / Intel), Linux (x86_64 / ARM64).

### Configure

```sh
claude mcp add scout -- scout
```

Or in `~/.claude/.mcp.json`:

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

You need a [Gemini API key](https://aistudio.google.com/apikey) for `search` and `research` (free tier works). `fetch` and GitHub tools work without it.

Set `GITHUB_TOKEN` or `GH_TOKEN` for higher GitHub rate limits (5,000/hour vs 60/hour unauthenticated). Falls back to `gh auth token` if not set.

## Tools

### `research` — Multi-source deep research

Searches the web via Gemini Grounding, fetches the top N source pages, and compiles a report — grounded answer, full page content, and deduplicated source list.

- `depth`: number of pages to fetch (1–10, default 3)
- `lang`: `"ja"`, `"en"`, or `"auto"` (default) — auto-detects Japanese and expands to bilingual queries

### `search` — Grounded web search

Gemini Grounding with Google Search. Returns a synthesized answer with source URLs — not a list of links to follow.

### `fetch` — Web page to Markdown

Downloads a page, extracts main content via Readability, converts to Markdown. No LLM round-trip.

- `raw`: skip Readability, convert entire page
- `meta`: include title/author/date as YAML frontmatter

### `repo_tree` — Remote file listing

List files in a GitHub repository with path prefix and glob pattern filtering.

```
repo_tree("denoland/deno", path="cli/", pattern="*.rs")

  denoland/deno (ref: main)
  files: 42

  cli/args.rs (38.2 KB)
  cli/build.rs (1.1 KB)
  ...
```

### `repo_read` — Read remote files

Read a file with optional line range (`"1-80"`, `"50-"`, `"100"`). Handles large files via git blob fallback. No base64 decoding needed.

### `repo_overview` — Repository at a glance

Repo metadata, README, open issues, PRs, and recent releases — 5 concurrent API calls, one response.

All GitHub tools accept `owner/repo`, full URLs (`https://github.com/denoland/deno`), and `.git`-suffixed URLs.

## How it works

**Research** — Runs Gemini Grounding search (with bilingual expansion for Japanese queries), collects unique source URLs, fetches up to N pages concurrently (5 parallel), then assembles the report: search answers + page content + source list.

**Fetch** — SSRF defense-in-depth:

```
URL validation → DNS pre-check → Download → Post-redirect recheck → Readability → Markdown
```

Private/loopback IPs blocked at DNS and redirect stages. Credentials redacted from errors. 10 MB download cap, 100K character output.

**Search** — Gemini `generateContent` with `google_search` grounding tool. The response includes both the generated answer and `groundingMetadata` with source URLs extracted from Google Search.

**GitHub** — Git Trees API for full-tree retrieval with client-side glob filtering. Contents API with blob fallback for large files.

## Architecture

```
src/
├── main.rs              MCP server (stdio transport)
├── tools/               Tool handlers and parameter definitions
├── search/
│   ├── engine.rs        Research engine (search + fetch + compile)
│   └── bilingual.rs     Japanese/English query expansion
├── fetch/
│   ├── extractor.rs     Readability article extraction
│   ├── converter.rs     HTML → Markdown conversion
│   └── ssrf.rs          SSRF defense (URL validation, DNS pre-check)
├── gemini/              Gemini API client, grounding response parsing
├── github/              GitHub API client, tree filtering, output formatting
└── markdown.rs          Markdown utilities
```

Single binary, zero runtime dependencies.

## Limitations

- **Gemini API key required** — `search` and `research` need `GEMINI_API_KEY`. Free tier: 100 RPM, 1,500/day.
- **No JavaScript rendering** — `fetch` downloads static HTML. SPAs that require client-side rendering return minimal content.
- **GitHub rate limits** — Unauthenticated: 60/hour. With token: 5,000/hour. `repo_overview` uses 5 requests per call.
- **Fetch size cap** — 10 MB download limit, 100K character output.

## License

MIT
