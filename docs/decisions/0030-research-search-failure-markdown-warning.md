---
status: "proposed"
date: 2026-10-08
---

# Show research search failures in default Markdown

## Context and Problem Statement

[Issue #480](https://github.com/thkt/scout/issues/480), split from
[#478](https://github.com/thkt/scout/issues/478), requires callers reading only
Markdown to distinguish a failed search from a successful search with zero hits.
The agreed scope preserves the search backend, degraded-success exit code 0,
JSON envelope, and authentication-error contract.

At audit commit `388db1f13c741e04a0babe5d8a1b9ce06242ad4d` and implementation
base `67ed30f29e60f3bcde64fe75e71416caa16759cd` (v2.6.1), `Scout::research`
(`src/tools/query.rs`) creates the same empty `ResearchReport` in both cases.
The relevant handler, tests, and DR-0003/0005/0010 have no changes between these
commits. JSON records the failure, but the Markdown formatter sees only the
empty report. This statement follows the code and Issue reproduction; the
regression has not been executed in the implementation sandbox.

## Decision Drivers

- Deliver the search-failure signal in the default output itself.
- Preserve successful-empty output and existing JSON and exit-code contracts.
- Keep backend error text out of a new Markdown rendering boundary.

## Considered Options

- Prepend a fixed warning in the research handler on the degradable-search path.
- Put backend error details into Markdown.
- Change degraded searches into nonzero exits.

## Decision Outcome

The implementation chooses a fixed blockquote before the report heading:

> Warning: Brave search failed; this is a degraded report, not a successful search with no results.

This is an implementation choice authorized by Issue #480, pending review of
this proposed record. No change to the accepted decisions is proposed:
[DR-0003](0003-error-classification-contract-for-sysexits-and-json-output.md),
[DR-0005](0005-switch-search-backend-from-gemini-grounding-to-brave-search-api.md),
and [DR-0010](0010-scout-local-json-envelope-contract.md) continue to govern error
classification, the backend, and JSON respectively. Historical migration and
service-pricing statements in DR-0005 are not new requirements or verified
service facts for this change.

Only Brave failure has populated `Degradation` when the handler adds the
warning; page-fetch degradations are collected later. The warning uses no
query, URL, or backend error interpolation. The existing sanitized report,
source sections, and JSON notes are preserved. Both empty cases still show
`(no results)` under Sources; the leading warning explains its failure context.
Authentication errors continue to propagate instead of producing this report.

### Consequences

- Markdown consumers can identify degraded search results without stderr or JSON.
- Fixed text adds no backend-controlled Markdown or secret-bearing content.
- Detailed diagnostics remain in the existing JSON notes and logs.
- Consumers comparing entire failed-search Markdown strings will see a change.
- Moving other degradation collection before the warning requires revisiting
  the condition, so page-fetch failures do not become search-failure warnings.

## Confirmation

T-TS029 in `src/tools/query_tests.rs` compares HTTP 503 and HTTP 200 with empty
results through `Scout::research` and the CLI's shared `emit_success` writer.
Both Markdown and JSON writes assert exit code 0. It checks the leading failure
warning, the absence of that warning on success, `BRAVE_SEARCH_FAILED`, notes,
and equal data payloads with three empty arrays. Restoring the old handler
makes the Markdown comparison and warning assertions fail by inspection;
execution against the old handler has not been recorded.

The former standalone T-TS031 empty-array test is integrated into this comparison.
Its empty-array detection conditions remain; only separate failure isolation is
lost. T-TS030 retains the HTTP 401 case and now checks exit 64, `USAGE_ERROR`,
and non-retryable JSON errors. The comparison acquires one result per response
scenario, then uses test-only `CommandOutput::clone` (`src/envelope.rs`) to feed
both consuming `emit_success` branches (`src/lib.rs`). The mock requires at
least one HTTP call per scenario. With the current default retry budget of two
(`DEFAULT_MAX_RETRIES` in `src/retry.rs`), the fixed responses require three
HTTP calls for 503 and one for 200. Production ownership and retry behavior are
unchanged. No live API key is needed.

The initial version acquired the same fixed response once per output mode:
four research invocations, six HTTP calls for 503, and two for 200. The first
independent evaluation (`review-1.json`, host-held record), target
`274f11ae926a435444c026566437d7b5a5fd08bd99aad4ed122d1dac8cc3d220`, identified
that redundant acquisition as required finding R1-1. Its corresponding
host log (`check-1.stderr`, host-held record) recorded T-TS029 passing in
4.743 seconds under default features and 5.479 seconds under all features;
the suites passed 855 and 864 tests respectively, with zero skipped. These
are results for the version before R1-1 was addressed, not for the revised test.
The revision removes one identical retry sequence and one successful HTTP
acquisition while preserving all output assertions and both real writer
branches. No detection condition is lost by reusing the fixed result; output
mode never affected acquisition. The small test-only clone avoids another
fixture or production cache. Runtime and flakiness changes remain unmeasured;
the historical times are not a controlled before/after comparison.

The unchanged configured host check runs default and all-features nextest with
`SCOUT_NETWORK_TESTS=1`, which rejects unavailable loopback fixtures instead of
silently skipping. No browser or mock server was started in the implementation
sandbox. Execution of the revised regression, full suites, and ignored Chrome
test remains for the configured host check. Its new results and independent
evaluation must be compared with R1-1; neither the initial check's success nor
static inspection guarantees the absence of other defects. T-TS030 checks
shared error classification and JSON rendering, not subprocess stderr capture.
Historical audit counts are not current results.

For the R1-1 revision, `cargo fmt -- --check` and `git diff --check` passed in
the sandbox (Rust 1.99.0; Cargo declares minimum Rust 1.98.1 and CI uses stable).
`cargo check --tests --offline` could not open the configured shared target's
build lock because sandbox write permission was denied; no compilation success
is claimed. The configured host check includes compilation of test targets in
both feature configurations, so this does not require a separate host command.

## Reassessment Triggers

Revisit this choice if research gains another pre-render degradation source or
changes the agreed degraded-success policy. Other commands' output contracts
and the separate test-cleanup issues are outside this decision.
