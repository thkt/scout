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
  (actual page content as Markdown — not a summary)

  ### https://authjs.dev/getting-started/installation
  (actual page content as Markdown)

  ...3 more pages...

  ## Sources
  - [Next.js Authentication](https://nextjs.org/docs/...)
  - [Auth.js](https://authjs.dev/...)
```

`search` returns raw source URLs from Brave Search. `research` fetches the top N pages as clean Markdown. No LLM summarization layer between you and the primary sources.

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

Or build from source (requires Rust 1.96+):

```sh
cargo install --path .
```

Pre-built binaries in [Releases](https://github.com/thkt/scout/releases) — macOS (Apple Silicon / Intel), Linux (x86_64 / ARM64).

### Environment

```sh
export BRAVE_SEARCH_API_KEY="..."   # Required for search/research (free tier: https://api-dashboard.search.brave.com/)
export GITHUB_TOKEN="..."           # Optional: 5,000 req/hour vs 60/hour unauthenticated
export SLACK_TOKEN="..."            # Optional: required for `fetch` on Slack permalinks (User OAuth token, xoxp-…)
```

`GITHUB_TOKEN` / `GH_TOKEN` / `gh auth token` are all supported, in that order.

### Tuning

Override the built-in timeouts and retry budget. Invalid values fail with exit 64 (usage error) before any request is made.

| Env var                       | Default | Range | Effect                                                                                |
| ----------------------------- | ------- | ----- | ------------------------------------------------------------------------------------- |
| `SCOUT_FETCH_TIMEOUT_SECS`    | 95      | 1–600 | Per-URL wall-clock budget for `fetch`                                                 |
| `SCOUT_RESEARCH_TIMEOUT_SECS` | 45      | 1–600 | Wall-clock budget for `research`                                                      |
| `SCOUT_SLACK_TIMEOUT_SECS`    | 60      | 1–600 | Wall-clock budget for Slack permalink `fetch`                                         |
| `SCOUT_GITHUB_TIMEOUT_SECS`   | 180     | 1–600 | Wall-clock budget per `repo-tree` / `repo-read` / `repo-overview` command             |
| `SCOUT_MAX_RETRIES`           | 2       | 0–10  | Retries on transient API failures, on top of the initial attempt (`0` disables retry) |

### Optional: JS rendering (for SPAs)

`fetch` auto-detects JS-dependent pages (React, Next.js, Vue, Nuxt) and falls back to headless Chrome via CDP. Chrome or Chromium must be installed locally; without it, auto-detected pages fall back to the original HTML and `--js` returns an error.

Prebuilt binaries (Homebrew, GitHub Releases) ship with the `js-rendering` feature enabled. Source builds enable it per-install:

```sh
cargo install --path . --features js-rendering
```

### Claude Code integration

Add to your project's `CLAUDE.md`:

```markdown
## Tools

- `scout search "query"` — web search via Brave (URL list)
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

### `scout search` — Web search returning source URLs

Brave Search API. Returns one URL per line on stdout — no markdown decoration, no summary, no answer. Pipe the result into `scout fetch` (or your agent's tool of choice) to read the actual sources.

```sh
scout search "Next.js server actions security"

  https://nextjs.org/docs/...
  https://...
```

```sh
scout search "Rust async runtime" | head -3 | xargs -I _ scout fetch _
```

| Flag         | Description                                                                                               |
| ------------ | --------------------------------------------------------------------------------------------------------- |
| `-l, --lang` | `ja`, `en`, or `auto` (default) — maps to Brave's `search_lang` parameter, no rewrite of the query string |

JSON envelope: `data = {query, sources}`, where each `sources[i] = {url, title, description}`. `description` is the search-engine snippet (Brave-provided, not an LLM summary). Zero-result responses return `sources: []`, not `null`.

### `scout research` — Multi-source deep research

Searches the web via Brave, fetches the top N source pages, and compiles a report — full page content plus the URL list. Unlike `search` which returns URLs only, `research` actually reads those pages so you (or your AI agent) can verify claims against primary sources.

```sh
scout research "Rust async runtime comparison" --depth 5 --lang ja
```

| Flag          | Description                                                     |
| ------------- | --------------------------------------------------------------- |
| `-d, --depth` | Pages to fetch (1–10, default 3)                                |
| `-l, --lang`  | `ja`, `en`, or `auto` (default) — maps to Brave's `search_lang` |

JSON envelope: `data = {query, sources, fetched_pages, failed_urls}`. All array fields are `[]` (never `null`) when empty.

### `scout fetch` — Web page to Markdown

Downloads a page, extracts main content via Readability, converts to Markdown. With `js-rendering` feature, JS-dependent pages (SPAs) are automatically detected and rendered via headless Chrome (CDP). No LLM round-trip.

```sh
scout fetch https://react.dev/blog/2024/12/05/react-19
```

| Flag    | Description                                                           |
| ------- | --------------------------------------------------------------------- |
| `--js`  | Force JS rendering via CDP (requires `js-rendering` feature + Chrome) |
| `--raw` | Skip Readability, convert the whole page except active HTML           |

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

```sh
# Pin to a tag, branch, or commit SHA
scout repo-tree denoland/deno --ref v2.0.0 --path cli/
```

| Flag         | Description                                                    |
| ------------ | -------------------------------------------------------------- |
| `--ref`      | Branch, tag, or commit SHA                                     |
| `-p, --path` | Filter by path prefix                                          |
| `--pattern`  | Glob matched against the whole repo-relative path (`src/*.rs`) |

### `scout repo-read` — Read remote files

```sh
scout repo-read facebook/react src/ReactElement.js --lines 1-50
```

```sh
# Read a non-UTF-8 file by explicit encoding
scout repo-read owner/repo legacy.txt --encoding shift_jis
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

Repo metadata, README, the 5 open issues and 5 open pull requests GitHub returns first, and the 3 most recent releases. Verifies the repo exists first, then fetches the rest in parallel.

The lists are not paginated. Each shows one page — at most 5 issues, 5 pull requests, and 3 releases — so a busier repository holds more than the overview shows.

All GitHub commands accept `owner/repo`, full URLs (`https://github.com/denoland/deno`), and `.git`-suffixed URLs.

## How it works

**Research** — Single Brave Search call (respecting `--lang` via `search_lang`), then up to N URLs are fetched in parallel (5 at a time) and assembled into the report: fetched page content + failed URLs + source list.

**Fetch** — SSRF defense-in-depth:

```
URL validation → DNS pre-check → Download (per-hop redirect SSRF check) → Post-redirect recheck → Readability → Markdown
```

Literal private/loopback IPs are rejected at URL validation and on every redirect hop, in every mode. In the default direct-connection mode, scout resolves and dials the host itself and re-checks the connect-time IP, closing the DNS-rebinding gap between the pre-check and the actual connection. When a standard proxy environment variable (`HTTPS_PROXY` / `HTTP_PROXY`) is set, scout instead routes fetches through that proxy: it keeps rejecting literal private/loopback targets on every hop but skips its own DNS resolution and delegates name-resolution defenses (DNS rebinding included) to the proxy's egress control. Credentials are redacted from errors. See Limitations for size caps.

**Search** — `GET https://api.search.brave.com/res/v1/web/search` with `X-Subscription-Token` auth. The response's `web.results[]` is mapped 1:1 to `{url, title, description}` and emitted verbatim.

**GitHub** — Git Trees API for full-tree retrieval with client-side glob filtering. Contents API with blob fallback for large files.

## Architecture

```
src/
├── main.rs              CLI entry point (clap)
├── tools/               Command handlers, params, error types
├── search/
│   ├── engine.rs        Research engine (search + fetch + compile)
│   └── lang.rs          Lang → Brave search_lang mapping
├── fetch/
│   ├── extractor.rs     Readability article extraction
│   ├── converter.rs     HTML → Markdown conversion
│   └── ssrf.rs          SSRF defense (URL validation, DNS pre-check)
├── brave/               Brave Search API client and response types
├── github/              GitHub API client (lazy-init), tree filtering, formatting
├── slack/               Slack message fetching (thread, reply permalink)
├── envelope.rs          JSON output envelope
├── markdown.rs          Markdown utilities (heading shift, truncation, escaping)
├── retry.rs             Retry with backoff (transient error, rate limit)
└── redacted.rs          Secret-safe wrapper for tokens
```

Single binary, zero runtime dependencies.

## Exit codes

Following [`sysexits.h`](https://man.openbsd.org/sysexits), with GNU coreutils `timeout` (124), an extension code (104) for unclassifiable failures, and the POSIX signal convention (128 + signal number) for interruption:

| Code | Meaning                                                                  |
| ---- | ------------------------------------------------------------------------ |
| 0    | Success                                                                  |
| 64   | Usage error (clap parse, missing API key, conflicts_with violation)      |
| 65   | Data error (invalid input, malformed format, encoding error, 4xx body)   |
| 66   | Not found (repo/file not found, 404)                                     |
| 70   | Internal (scout-side invariant violation, unexpected response schema)    |
| 74   | IO error (external tool failure such as headless browser)                |
| 75   | Temporary failure (rate limit, 5xx, retryable — short backoff)           |
| 104  | Unknown (unclassifiable failure; rising rate signals classification gap) |
| 124  | Timeout (request/transport timeout, retryable — longer backoff advised)  |
| 130  | Interrupted by SIGINT (128 + 2; e.g. Ctrl-C)                             |
| 143  | Interrupted by SIGTERM (128 + 15; e.g. shell timeout, kill default)      |

## Migration to v2

scout v2.0.0 switches the search backend from Gemini Grounding to Brave Search API. The change is breaking: env var, output format, and JSON schema all changed.

**Env var**

```diff
-export GEMINI_API_KEY="..."
+export BRAVE_SEARCH_API_KEY="..."   # Get one at https://api-dashboard.search.brave.com/
```

`search` and `research` both need `BRAVE_SEARCH_API_KEY`. See Limitations for Brave Search free tier details.

**`scout search` output**

v1 returned a Gemini-synthesized answer plus a `**Sources:**` markdown list. v2 returns plain URLs — one per line, no decoration:

```diff
- Claude, developed by Anthropic, offers robust capabilities...
- ---
- **Sources:**
- - [Claude Code](https://vertexaisearch.cloud.google.com/grounding-api-redirect/...)
+ https://www.anthropic.com/claude-code
+ https://docs.anthropic.com/...
```

Sources are now the actual destination URLs (not Google redirect URLs).

**`scout research` output**

The `## Search Result` section (which carried the Gemini-generated answer) is removed. The report keeps `## Fetched Pages` (page content), `## Sources` (URL list), and `## Failed URLs` (shown only when a source could not be fetched).

`research` no longer hard-fails when Brave Search itself errors after retry. Instead it returns a degraded report (`data.sources: []`, no fetched pages) and adds `BraveSearchFailed` to `degraded_reasons` so callers can detect the search-tier failure without parsing error messages.

**`--json` schema**

- `data.answer` is gone (v1 carried the Gemini answer)
- `data.sources[i]` is now `{url, title, description}` instead of `{url, title}`. `description` is the Brave-provided search-engine snippet, not an LLM summary
- `data.fetched_pages` and `data.failed_urls` (research only) are unchanged in shape; both default to `[]` (never `null`) when empty

**Removed**

- `Lang::apply_to_query`: queries are no longer suffixed with `(日本語で回答)` / `(answer in English)`. `--lang ja/en` now maps to Brave's `search_lang` parameter and the query string itself is unmodified
- Bilingual expansion for `--lang auto`: scout no longer issues a second English-only query for Japanese inputs. If you need both, call `scout` twice from the caller side

## Limitations

| Limitation                  | Details                                                                                                                                   |
| --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Brave Search API key needed | `search` and `research` need `BRAVE_SEARCH_API_KEY`. Free tier: $5/month recurring credit (~1,000 q/month)                                |
| JS rendering needs Chrome   | `fetch` auto-detects SPAs. With `--features js-rendering`, falls back to headless Chrome (CDP) for JS rendering. Requires Chrome/Chromium |
| GitHub rate limits          | Unauthenticated: 60/hour. With token: 5,000/hour. `repo-overview` uses 5–6 requests per call                                              |
| Fetch size cap              | 10 MB download limit (response body), 100 KB output cap (markdown after extraction)                                                       |

## License

MIT
