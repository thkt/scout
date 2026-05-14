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

Japanese queries are handled automatically: "Next.js 認証 ベストプラクティス" expands to both the original and a query built from extracted ASCII technical terms (e.g., "Next.js"), improving coverage of English documentation that uses those terms. Pure Japanese queries without ASCII terms are searched as-is.

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

Or build from source (requires Rust 1.95+):

```sh
cargo install --path .
```

Pre-built binaries in [Releases](https://github.com/thkt/scout/releases) — macOS (Apple Silicon / Intel), Linux (x86_64 / ARM64).

### Environment

```sh
export GEMINI_API_KEY="..."   # Required for search/research (free tier: https://aistudio.google.com/apikey)
export GITHUB_TOKEN="..."     # Optional: 5,000 req/hour vs 60/hour unauthenticated
export SLACK_TOKEN="..."      # Optional: required for `fetch` on Slack permalinks (User OAuth token, xoxp-…)
```

`GITHUB_TOKEN` / `GH_TOKEN` / `gh auth token` are all supported, in that order.

### Optional: JS rendering (for SPAs)

`fetch` auto-detects JS-dependent pages (React, Next.js, Vue, Nuxt) and falls back to headless Chrome via CDP. Requires Chrome or Chromium installed locally and the `js-rendering` feature:

```sh
cargo install --path . --features js-rendering
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

All commands accept the query/URL/repo as a positional argument, piped stdin, or `-` to read stdin interactively (e.g., `echo "query" | scout search`, `scout search -`).

Add `--json` to any command for a one-line JSON envelope instead of Markdown — useful for `jq` pipelines and feeding structured data back to AI agents. Successful output goes to stdout; errors emit a JSON envelope on stderr.

Use `scout --version` (or `-V`) to print the version and `scout --help` / `scout <command> --help` for built-in help.

### `scout research` — Multi-source deep research

Searches the web via Gemini Grounding, fetches the top N source pages, and compiles a report — grounded answer, full page content, and deduplicated source list. Unlike `search` which returns an AI answer with URLs, `research` actually reads those pages and includes the full content — so you (or your AI agent) can verify claims against primary sources.

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

| Flag         | Description                                                                              |
| ------------ | ---------------------------------------------------------------------------------------- |
| `-l, --lang` | `ja`, `en`, or `auto` (default) — auto-detects Japanese and expands to bilingual queries |

### `scout fetch` — Web page to Markdown

Downloads a page, extracts main content via Readability, converts to Markdown. With `js-rendering` feature, JS-dependent pages (SPAs) are automatically detected and rendered via headless Chrome (CDP). No LLM round-trip.

```sh
scout fetch https://react.dev/blog/2024/12/05/react-19
```

| Flag    | Description                                                           |
| ------- | --------------------------------------------------------------------- |
| `--js`  | Force JS rendering via CDP (requires `js-rendering` feature + Chrome) |
| `--raw` | Skip Readability, convert entire page                                 |

Page metadata (title, author, date) is included as YAML frontmatter. The frontmatter block is always present; individual fields appear when the page provides them.

**Slack permalinks** — `fetch` detects `*.slack.com/archives/{channel}/p{ts}` URLs and routes them to the Slack Web API instead of HTML scraping. Thread parent + replies are preserved with author/timestamp metadata. Requires `SLACK_TOKEN` (User OAuth token, `xoxp-…`).

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

| Flag          | Description                                                                                                                                                                                                                                      |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `--ref`       | Branch, tag, or commit SHA                                                                                                                                                                                                                       |
| `-l, --lines` | Line range: `1-80`, `50-`, or `100` (first N lines)                                                                                                                                                                                              |
| `--encoding`  | Character encoding (e.g., `shift_jis`, `euc-jp`, `gbk`). When omitted, auto-detects UTF-8, Shift_JIS, EUC-JP, GBK, EUC-KR, and other multi-byte encodings. Single-byte encodings (windows-1252, ISO-8859-\*, etc.) require explicit `--encoding` |

### `scout repo-overview` — Repository at a glance

```sh
scout repo-overview denoland/deno
```

Repo metadata, README, open issues, PRs, and recent releases. Verifies the repo exists first, then fetches the rest in parallel.

All GitHub commands accept `owner/repo`, full URLs (`https://github.com/denoland/deno`), and `.git`-suffixed URLs.

## How it works

**Research** — Runs Gemini Grounding search (with bilingual expansion for Japanese queries), collects unique source URLs, fetches up to N pages concurrently (5 parallel), then assembles the report: search answers + page content + source list.

**Fetch** — SSRF defense-in-depth:

```
URL validation → DNS pre-check → Download (per-hop redirect SSRF check) → Post-redirect recheck → Readability → Markdown
```

Private/loopback IPs blocked at URL validation, DNS, and each redirect hop. Post-redirect recheck kept as defense-in-depth. Credentials redacted from errors. 10 MB download cap, 100K byte output. Note: SSRF defense is designed for local CLI use where the user controls URL input. If embedding scout in a service that accepts untrusted URLs, additional measures (e.g., DNS pinning) are required to close the TOCTOU gap between DNS check and connection.

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
├── github/              GitHub API client (lazy-init), tree filtering, formatting
├── slack/               Slack message fetching (thread, reply permalink)
├── envelope.rs          JSON output envelope
├── markdown.rs          Markdown utilities (heading shift, truncation, escaping)
├── retry.rs             Retry with backoff (transient error, rate limit)
└── redacted.rs          Secret-safe wrapper for tokens
```

Single binary, zero runtime dependencies.

## Exit codes

Following [`sysexits.h`](https://man.openbsd.org/sysexits), with a GNU coreutils `timeout` code (124) and a PJ extension code (104) for unclassifiable failures:

| Code | Meaning                                                             |
| ---- | ------------------------------------------------------------------- |
| 0    | Success                                                             |
| 64   | Usage error (clap parse, missing API key, conflicts_with violation) |
| 65   | Data error (invalid input, malformed format, encoding error, 4xx body) |
| 66   | Not found (repo/file not found, 404)                                |
| 70   | Internal (scout-side invariant violation, unexpected response schema) |
| 74   | IO error (external tool failure such as headless browser)           |
| 75   | Temporary failure (rate limit, 5xx, retryable — short backoff)      |
| 124  | Timeout (request/transport timeout, retryable — longer backoff advised) |
| 104  | Unknown (unclassifiable failure; rising rate signals classification gap) |

## Limitations

| Limitation                | Details                                                                                                                                   |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Gemini API key required   | `search` and `research` need `GEMINI_API_KEY`. Free tier: 100 RPM, 1,500/day                                                              |
| JS rendering needs Chrome | `fetch` auto-detects SPAs. With `--features js-rendering`, falls back to headless Chrome (CDP) for JS rendering. Requires Chrome/Chromium |
| GitHub rate limits        | Unauthenticated: 60/hour. With token: 5,000/hour. `repo-overview` uses 5–6 requests per call                                              |
| Fetch size cap            | 10 MB download limit, 100K byte output                                                                                                    |

## License

MIT
