# Source and test inventory

This inventory covers every Rust file under `src/` and `tests/`. A listed
Go comparison applies only to the operations in `run.py`, not every function
in that file. Test counts are declarations, not branch coverage or assertions.
Files without additional checks rely on the existing full-suite runs; this
does not imply an independent implementation or mutation audit of every path.

| File | Test declarations | Go operations | Mutation probes |
|---|---:|---|---|
| `src/body_limit/tests.rs` | 5 | — | — |
| `src/body_limit.rs` | 0 | — | body_boundary |
| `src/brave/client/classify_tests.rs` | 9 | — | — |
| `src/brave/client/http_tests.rs` | 20 | — | — |
| `src/brave/client.rs` | 0 | — | — |
| `src/brave/types.rs` | 7 | — | — |
| `src/brave.rs` | 0 | — | — |
| `src/charset.rs` | 3 | — | charset_single_byte |
| `src/classify.rs` | 0 | yes | http_not_found |
| `src/clock.rs` | 1 | — | — |
| `src/envelope/tests.rs` | 16 | — | — |
| `src/envelope.rs` | 0 | — | retryability |
| `src/fetch/cdp/cdp_integration_tests.rs` | 2 | — | — |
| `src/fetch/cdp/launch/browser_binary_tests.rs` | 2 | — | — |
| `src/fetch/cdp/launch/browser_request_tests.rs` | 7 | — | — |
| `src/fetch/cdp/launch/cdp_launch_tests.rs` | 3 | — | — |
| `src/fetch/cdp/launch/ws_url_parse_tests.rs` | 4 | — | — |
| `src/fetch/cdp/launch.rs` | 0 | — | — |
| `src/fetch/cdp/proxy/proxy_tests.rs` | 14 | — | — |
| `src/fetch/cdp/proxy/transport.rs` | 0 | — | — |
| `src/fetch/cdp/proxy.rs` | 0 | — | — |
| `src/fetch/cdp.rs` | 0 | — | — |
| `src/fetch/classify_tests.rs` | 11 | — | — |
| `src/fetch/converter.rs` | 79 | — | html_paragraph_break |
| `src/fetch/download/charset_tests.rs` | 13 | — | — |
| `src/fetch/download/content_type_tests.rs` | 6 | — | — |
| `src/fetch/download/download_tests.rs` | 14 | — | — |
| `src/fetch/download.rs` | 0 | yes | content_type_case |
| `src/fetch/extractor.rs` | 18 | — | — |
| `src/fetch/fetch_page_tests.rs` | 11 | — | — |
| `src/fetch/js_dependent_tests.rs` | 8 | — | — |
| `src/fetch/ssrf/dns_tests.rs` | 13 | — | — |
| `src/fetch/ssrf/egress_tests.rs` | 6 | — | — |
| `src/fetch/ssrf/tests.rs` | 8 | — | — |
| `src/fetch/ssrf.rs` | 0 | yes | — |
| `src/fetch/thin_body_tests.rs` | 7 | — | — |
| `src/fetch/thin_extract_tests.rs` | 8 | — | — |
| `src/fetch.rs` | 0 | — | — |
| `src/github/encoding/tests.rs` | 16 | — | — |
| `src/github/encoding.rs` | 0 | yes | github_binary |
| `src/github/errors/classify_tests.rs` | 12 | — | — |
| `src/github/errors.rs` | 0 | — | — |
| `src/github/format/file_content_tests.rs` | 10 | — | — |
| `src/github/format/overview_tests.rs` | 25 | — | — |
| `src/github/format/size_tests.rs` | 8 | — | — |
| `src/github/format/tree_tests.rs` | 4 | — | — |
| `src/github/format.rs` | 0 | yes | — |
| `src/github/helpers/tests.rs` | 31 | — | — |
| `src/github/helpers.rs` | 0 | yes | github_range_zero |
| `src/github/http_tests.rs` | 23 | — | — |
| `src/github/types.rs` | 4 | — | — |
| `src/github.rs` | 0 | — | — |
| `src/lib.rs` | 16 | — | — |
| `src/main.rs` | 0 | — | — |
| `src/markdown.rs` | 37 | yes | inline_pipe |
| `src/redacted.rs` | 7 | — | redacted_trim |
| `src/retry/tests.rs` | 14 | — | — |
| `src/retry.rs` | 0 | yes | retry_boundary |
| `src/rng.rs` | 2 | — | — |
| `src/search/engine/tests.rs` | 18 | — | — |
| `src/search/engine.rs` | 0 | — | research_depth, research_failure_order |
| `src/search/lang.rs` | 3 | — | lang_auto |
| `src/search.rs` | 0 | — | — |
| `src/signals.rs` | 4 | — | sigint_code |
| `src/slack/classify_tests.rs` | 23 | — | — |
| `src/slack/client/constructor_tests.rs` | 6 | — | — |
| `src/slack/client/http_tests.rs` | 26 | — | — |
| `src/slack/client.rs` | 0 | — | — |
| `src/slack/format/format_tests.rs` | 8 | — | — |
| `src/slack/format/resolve_messages_tests.rs` | 5 | — | — |
| `src/slack/format.rs` | 0 | — | — |
| `src/slack/mention/mention_tests.rs` | 21 | — | — |
| `src/slack/mention.rs` | 0 | yes | slack_whitespace |
| `src/slack/url/url_tests.rs` | 8 | — | — |
| `src/slack/url.rs` | 0 | — | slack_empty_channel |
| `src/slack.rs` | 0 | — | — |
| `src/test_support.rs` | 19 | — | — |
| `src/token_source.rs` | 7 | — | token_priority |
| `src/tools/builder.rs` | 0 | — | — |
| `src/tools/builder_tests.rs` | 19 | — | — |
| `src/tools/config.rs` | 20 | yes | timeout_minimum |
| `src/tools/errors/classification_tests.rs` | 23 | — | — |
| `src/tools/errors/exit_code_tests.rs` | 10 | — | — |
| `src/tools/errors.rs` | 0 | — | — |
| `src/tools/params.rs` | 19 | — | stdin_trim |
| `src/tools/query.rs` | 0 | — | — |
| `src/tools/query_tests.rs` | 21 | — | — |
| `src/tools/repo.rs` | 3 | — | — |
| `src/tools/repo_io_tests.rs` | 3 | — | — |
| `src/tools/repo_lazy_tests.rs` | 13 | — | — |
| `src/tools/stdin_tests.rs` | 5 | — | — |
| `src/tools/test_helpers.rs` | 0 | — | — |
| `src/tools/typo.rs` | 9 | — | — |
| `src/tools.rs` | 0 | — | — |
| `src/yaml.rs` | 15 | yes | yaml_null |
| `tests/cli_integration.rs` | 18 | — | — |
| `tests/common/mod.rs` | 5 | — | — |
| `tests/exit_code_contract.rs` | 8 | — | — |
| `tests/output_injection.rs` | 14 | — | — |
