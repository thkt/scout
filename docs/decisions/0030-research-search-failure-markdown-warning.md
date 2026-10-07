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
both consuming `emit_success` branches (`src/lib.rs`). Each scenario explicitly
verifies the mock's minimum of one HTTP call after acquisition and before the
next reset can discard its expectation. With the current default retry budget
of two (`DEFAULT_MAX_RETRIES` in `src/retry.rs`), the fixed responses require three
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

### R1-1 revision handoff (historical)

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

### PR #486 CI repair

The adopted repair request starts from published commit
`6eb88329e2f088e37bbee47ad46b9ed993daa18f`. In
[Actions run 37664301132](https://github.com/thkt/scout/actions/runs/37664301132),
coverage job `112939453599` reported four unexecuted lines in the new
`emit_success` write-error branches (`src/lib.rs`): 11 of 15 changed lines
were covered (73%, below the unchanged 95% gate). This is a historical CI
result, not a measurement of the repair. The adopted request reports the other
CI checks passing; their success does not establish coverage of those branches.

T-W003 in `src/lib.rs` now injects BrokenPipe and StorageFull at `emit_success`
in both output modes, expecting exit 0 and exit 74 respectively. Previously it
called only `write_output` and checked the propagated BrokenPipe kind, leaving
the CLI's special-case success and other-error exit mapping unobserved. The
updated test detects making a closed downstream pipe a command failure or
silently treating a full output device as success. Expected exit codes are
literal public-contract values, not derived from the mapping under test.

This replaces the lower-level BrokenPipe test rather than adding an overlapping
test. Its independent observation of the returned `io::ErrorKind` is lost; the
writer-to-exit behavior is now observed through the actual CLI success-output
entry. T-W001/T-W002 still check newline handling, and T-W004/T-W005 still check
the JSON IO_ERROR/non-retryable and plain error renderings. T-TS029/T-TS030 and
the single-acquisition fixture arrangement remain unchanged. The updated test
performs four synchronous fault injections, with no HTTP, browser,
subprocess, or timing dependency. No measured runtime or flakiness improvement
is claimed. Production code, README behavior, CI thresholds, and accepted DRs
are unchanged. No new test ID or decision number is allocated.

For this repair, `cargo fmt -- --check` and `git diff --check` passed in the
sandbox. The targeted command `cargo test --offline --lib
tests::emit_success_preserves_write_failure_exit_codes -- --exact` could not
open the shared target's build lock because sandbox write permission was denied;
it did not execute the test and is neither a regression failure nor a pass.
The configured host check runs this test under default and all features. The
configured CI coverage check uses `cargo llvm-cov --features js-rendering --lcov
--output-path lcov.info -- --include-ignored`, followed by the existing diff
coverage gate. New check and CI results must be recorded
for the repaired version; the previous run and the R1-1 history above remain
evidence for their own versions only. The fault-injection test observes return
codes, not subprocess stderr; error text remains covered by T-W004/T-W005.

### Mock expectation repair after CI-repair evaluation

The CI-repair evaluation (`review-1.json`, host-held record at
`/private/tmp/scout-implement-480-ci-repair-20261008/verification/review-1.json`),
target `d8aa14472a277f828a3c18ce62f5a2f27ec5c98f2525a97a34a1cb46a2afc71c`,
found that T-TS029 erased the 503 mock expectation when starting the 200
scenario. This is a separate evaluation from the initial acquisition review
above. In wiremock 0.6.5 (`Cargo.lock`), `MockServer::reset` delegates to
`MountedMockSet::reset`, which clears mocks without verification. Drop verifies
only mocks still present. A network failure before reaching the 503 fixture
could therefore satisfy the general degraded-output assertions. The earlier
minimum-call guarantee was incomplete despite successful host checks; the
review does not claim those checks actually substituted a network failure.

T-TS029 now calls `MockServer::verify` immediately after each acquisition,
before any subsequent reset. The existing `expect(1..)` rejects zero matching
requests, so a failure before reaching the 503 response cannot substitute for
the required HTTP scenario. This failure condition follows the pinned
dependency's verification implementation; a deliberate no-request execution
has not been run in the sandbox, where starting a mock server is prohibited.
The configured host check executes the revised regression under default and
all features with `SCOUT_NETWORK_TESTS=1`.

The same acquired result still feeds both output formats, and all warning,
JSON, exit-code, and empty-array assertions remain. No detection condition,
test ID, or fixture acquisition is removed or added. Two expectation checks
add no HTTP requests or retry waits; their runtime and flakiness impact has
not been measured. Keeping verification beside acquisition prevents the next
reset from silently invalidating this scenario's HTTP-boundary guarantee.
This local repair requires no repository-wide rule or new lint. Production
behavior, authentication checks, and the writer-failure repair are preserved.
New host check and independent-evaluation results remain pending for this
revision; earlier successes are evidence only for their recorded versions.
`cargo fmt -- --check` and `git diff --check` passed for this local revision.

## Reassessment Triggers

Revisit this choice if research gains another pre-render degradation source or
changes the agreed degraded-success policy. Other commands' output contracts
and the separate test-cleanup issues are outside this decision.
