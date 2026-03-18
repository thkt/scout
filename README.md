**English** | [日本語](README.ja.md)

# scout

Web research and GitHub exploration — for humans and AI agents alike. Read the sources, not a summary of the sources.

## The problem

You need to research how Next.js App Router handles authentication.

**Without scout:**

```
curl https://nextjs.org/docs/.../authentication | # wall of HTML
gh api /repos/vercel/next.js/... | # raw JSON
```

Multiple tools, multiple formats, lots of noise.

**With scout:**

```sh
scout research "Next.js App Router authentication best practices" --depth 5

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

One command, grounded answer from Google Search, plus 5 source pages as clean Markdown. No LLM intermediary — you read the primary sources and decide what matters.

Japanese queries are handled automatically: "Next.js 認証 ベストプラクティス" expands to both the original and an English query extracted from the technical terms, so English-only documentation isn't missed.

## When to use scout (and when not to)

**Use scout when:**

- You need to investigate a topic across multiple sources — `research` does the search → fetch → compile loop for you
- You want full page content, not an LLM summary — `fetch` returns raw Markdown
- You need to explore a remote GitHub repo without cloning — `repo-tree`, `repo-read`, `repo-overview`

**Use existing tools when:**

- A quick `curl` is enough — scout adds Readability extraction and SSRF protection on top
- The file is already on disk — no network needed
- You need complex browser interactions — scout handles JS rendering for SPAs but not login flows or dynamic interactions

## Setup

### Install

```sh
brew install thkt/tap/scout
```

Or build from source (requires Rust 1.85+):

```sh
cargo install --path .
```

Pre-built binaries in [Releases](https://github.com/thkt/scout/releases) — macOS (Apple Silicon / Intel), Linux (x86_64 / ARM64).

### Environment

```sh
export GEMINI_API_KEY="..."   # Required for search/research (free tier: https://aistudio.google.com/apikey)
export GITHUB_TOKEN="..."     # Optional: 5,000 req/hour vs 60/hour unauthenticated
```

`GITHUB_TOKEN` / `GH_TOKEN` / `gh auth token` are all supported, in that order.

### Optional: JS rendering (for SPAs)

`fetch` auto-detects JS-dependent pages (React, Next.js, Vue, Nuxt) and falls back to `playwright-cli`. It works via `npx` out of the box, but installing globally is faster:

```sh
npm install -g @playwright/cli
```

### Claude Code integration

Add to your project's `CLAUDE.md`:

```markdown
## Tools

- `scout search "query"` — web search via Gemini Grounding
- `scout fetch URL` — web page to clean Markdown
- `scout research "query" --depth N` — multi-source deep research
- `scout repo-tree owner/repo` — list files in a GitHub repo
- `scout repo-read owner/repo path` — read a file from a GitHub repo
- `scout repo-overview owner/repo` — repository overview
```

Claude Code will pick up the commands naturally — no MCP configuration needed.

## Commands

### `scout research` — Multi-source deep research

Searches the web via Gemini Grounding, fetches the top N source pages, and compiles a report — grounded answer, full page content, and deduplicated source list.

```sh
scout research "Rust async runtime comparison" --depth 5 --lang ja
```

| Flag          | Description                                                                              |
| ------------- | ---------------------------------------------------------------------------------------- |
| `-d, --depth` | Pages to fetch (1–10, default 3)                                                         |
| `-l, --lang`  | `ja`, `en`, or `auto` (default) — auto-detects Japanese and expands to bilingual queries |

### `scout search` — Grounded web search

Gemini Grounding with Google Search. Returns a synthesized answer with source URLs — not a list of links to follow.

```sh
scout search "Next.js server actions security"
```

### `scout fetch` — Web page to Markdown

Downloads a page, extracts main content via Readability, converts to Markdown. JS-dependent pages (SPAs) are automatically detected and rendered via `playwright-cli`. No LLM round-trip.

```sh
scout fetch https://react.dev/blog/2024/12/05/react-19
```

| Flag    | Description                                                    |
| ------- | -------------------------------------------------------------- |
| `--js`  | Force JS rendering via playwright-cli (auto-detected for SPAs) |
| `--raw` | Skip Readability, convert entire page                          |

Page metadata (title, author, date) is included as YAML frontmatter. The frontmatter block is always present; individual fields appear when the page provides them.

### `scout repo-tree` — Remote file listing

```sh
scout repo-tree denoland/deno --path cli/ --pattern "*.rs"

  denoland/deno (ref: main)
  files: 42

  cli/args.rs (38.2 KB)
  cli/build.rs (1.1 KB)
  ...
```

| Flag         | Description                |
| ------------ | -------------------------- |
| `--ref`      | Branch, tag, or commit SHA |
| `-p, --path` | Filter by path prefix      |
| `--pattern`  | Glob pattern for filenames |

### `scout repo-read` — Read remote files

```sh
scout repo-read facebook/react src/ReactElement.js --lines 1-50
```

| Flag          | Description                                         |
| ------------- | --------------------------------------------------- |
| `--ref`       | Branch, tag, or commit SHA                          |
| `-l, --lines` | Line range: `1-80`, `50-`, or `100` (first N lines) |

### `scout repo-overview` — Repository at a glance

```sh
scout repo-overview denoland/deno
```

Repo metadata, README, open issues, PRs, and recent releases — 5 concurrent API calls, one response.

All GitHub commands accept `owner/repo`, full URLs (`https://github.com/denoland/deno`), and `.git`-suffixed URLs.

## How it works

**Research** — Runs Gemini Grounding search (with bilingual expansion for Japanese queries), collects unique source URLs, fetches up to N pages concurrently (5 parallel), then assembles the report: search answers + page content + source list.

**Fetch** — SSRF defense-in-depth:

```
URL validation → DNS pre-check → Download → Post-redirect recheck → Readability → Markdown
```

Private/loopback IPs blocked at DNS and redirect stages. Credentials redacted from errors. 10 MB download cap, 100K byte output.

**Search** — Gemini `generateContent` with `google_search` grounding tool. The response includes both the generated answer and `groundingMetadata` with source URLs extracted from Google Search.

**GitHub** — Git Trees API for full-tree retrieval with client-side glob filtering. Contents API with blob fallback for large files.

## Architecture

```
src/
├── main.rs              CLI entry point (clap)
├── tools/               Command handlers, params, error types
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

| Limitation                 | Details                                                                                                                           |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Gemini API key required    | `search` and `research` need `GEMINI_API_KEY`. Free tier: 100 RPM, 1,500/day                                                      |
| JS rendering needs Node.js | `fetch` auto-detects SPAs and falls back to `playwright-cli` for JS rendering. Requires `npx` (Node.js) if not installed globally |
| GitHub rate limits         | Unauthenticated: 60/hour. With token: 5,000/hour. `repo-overview` uses 5 requests per call                                        |
| Fetch size cap             | 10 MB download limit, 100K byte output                                                                                            |

## License

MIT
