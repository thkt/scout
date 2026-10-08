---
status: "proposed"
date: 2026-10-08
---

# Close code fences at report body boundaries

## Context and Problem Statement

[Issue #481](https://github.com/thkt/scout/issues/481), split from
[#478](https://github.com/thkt/scout/issues/478), requires truncated research
pages and repo-overview READMEs to leave subsequent sections outside their code
blocks. The Issue authorizes the implementer to choose the method. This record
presents that choice for review; it does not claim owner acceptance.

At audit commit `388db1f13c741e04a0babe5d8a1b9ce06242ad4d` and implementation
base `67ed30f29e60f3bcde64fe75e71416caa16759cd` (v2.6.1), the relevant code in
`src/markdown.rs`, `src/yaml.rs`, `src/search/engine.rs`, and
`src/github/format.rs` is identical. Both formatter regressions fail on the base
because no closing fence precedes the following section. The Issue calls the
research limit 4,500 characters; `MAX_PAGE_BYTES` actually limits bytes. This
implementation preserves the byte contract rather than changing the Issue.
No separate audit report was supplied for this implementation.

[DR-0014](0014-output-injection-defense-for-agent-consumers.md) governs YAML
neutralization, including the unclosed-fence defense from
[#405](https://github.com/thkt/scout/issues/405).
[DR-0016](0016-github-formatter-output-schema.md) governs README truncation and
section order. Their accepted texts remain historical records. This follow-up
changes the open-fence consequence described in DR-0016, while preserving its
body cap, note, and section order and DR-0014's fail-closed marker rewriting.

## Considered Options

- Append a closing delimiter after neutralizing the retained Markdown.
- Enclose the whole retained body in an additional code block.
- Drop the unfinished code block at the truncation point.

## Decision Outcome

Choose delimiter completion. `dangling_report_fence` in `src/markdown.rs`
uses pulldown-cmark 0.13.4's block events and source ranges to find top-level
fenced code blocks. It checks the final source line separately from the opening
line for a same-character closing run at least as wide as the opener, with no
info string. HTML blocks, list/blockquote code blocks and indented code cannot
cause a synthetic top-level opening delimiter. Parser-only normalization of
lone CR and trailing tabs to LF/spaces preserves byte offsets and emitted source
text: this parser version scans block lines by LF and closing whitespace by
spaces (`src/firstpass.rs::parse_fenced_code_block` and
`src/scanners.rs::scan_nextline` / `scan_closing_code_fence` in the pinned crate).
The parser dependency has default features disabled; its CLI and HTML renderer
are not needed. The lock adds only pulldown-cmark and unicase, retaining existing
dependency versions. This adds a parser dependency and block parsing cost; it
avoids maintaining an incomplete local Markdown context scanner. No net runtime
or maintenance improvement is claimed.

`track_fence`, used by heading shifting and YAML neutralization, retains its
existing conservative behavior, including fence-looking inline-code runs.
`finish_report_body` in `src/yaml.rs` selects the union of this tracker's dangling
range and the actual top-level fenced tail before rewriting. Research uses the
earliest required tail because fetch already neutralizes the body. Raw README
uses the entire body when the conservative tracker is dangling, otherwise
rewrites outside conservative fences and the actual dangling tail. Each line is
rewritten at most once within this report completion step, directly into the
output buffer. Closed conservative fences outside these ranges retain markers
verbatim. The two trackers intentionally have different guarantees.

`format_fetched_pages` in `src/search/engine.rs` keeps media-aware heading
handling → byte cut and note → integrated tail neutralization and completion.
`FetchResult::with_heading_offset` shifts HTML-derived headings and preserves
plain-text literals and terminal blank lines before the report byte cut.
`format_readme_section` in `src/github/format.rs` keeps byte cut → heading
shift → note → integrated YAML neutralization and completion. Moving the note
before neutralization changes no marker behavior because the generated note
contains no YAML marker. The original source must be neutralized before a
synthetic close is appended; otherwise #405's defense would be weakened.
The note's wording and position relative to retained source text stay unchanged;
when the cut is inside code, the note remains inside the completed code block.
Standalone fetch's truncation/re-neutralization remains separate from report
completion. The later main integration retains its uncut borrowed return.

Only the report composition boundaries invoke completion. Already unclosed
short bodies also need completion to protect following sections. Fully closed
bodies get no extra delimiter. Fetch/Slack standalone output contracts, CLI
flags, JSON schemas, SSRF, and redaction are unchanged. The existing 4,500-byte
research and 24,000-byte README source budgets remain; the synthetic delimiter,
like headings and the existing note, is formatting overhead outside those
budgets. Container content is retained without adding container closing syntax; only a
top-level dangling code block can absorb the later top-level report sections.

Wrapping the entire body would turn prose, headings, and links into literal
code. Dropping the unfinished block would discard primary-source text inside
the existing budget. Completion retains that text with a small structural
addition, at the cost of a forward scan and a copy when completion is needed.
No runtime or maintenance improvement is claimed without measurement.

## Confirmation and Review Handoff

T-SE023 and T-GF048 exercise the actual report formatters with closed source
bodies whose closing delimiters lie after the byte cap. They compare literal
closing-fence/section boundaries instead of repeating the production scanner.
They cover both fence characters, differing widths, decoy closing lines, YAML
markers, the second page and Failed URLs/Sources, and all three recent GitHub
sections. T-MD038 supplies short, bare-opening, closed, invalid-info, indented-code,
CRLF/CR and trailing-tab controls so completion does not add a fence to ordinary
code or alter an already closed body. T-GF048 also retains short and truncated
normal HTML-comment, list, pre/CDATA, CR and trailing-tab bodies unchanged at the
section boundary, and checks the conservative whole-body YAML fallback with a
prior closed block. Its oracle remains literal boundaries, not the production
parser or tracker.

Keep the existing cap/UTF-8/line-boundary, heading-shift, and YAML-injection
regressions: they defend separate failure conditions. The new formatter tests
add the missing structural guarantee; the small lexical test catches false
completion without external services, browser startup or sockets. The production
parser is not reused as an expected-value oracle. No existing test is removed or consolidated, so no existing
detection condition is lost. The broader seven-group cleanup belongs to #484.
These boundary assertions do not establish arbitrary CommonMark conformance or
real Web/GitHub responses. The Issue's acceptance fixtures do not require them.

The reserved DR-0029/0030 and test IDs were checked against local PR source
refs: #485 at `96ddd62908e081b507c053912ee4adec0c78f4b5` and #486 at
`c3a003994f97fa260a273ed06a8e31bf01cd6a1e`. At the original implementation stage, those independent implementations
were not incorporated. #485 also edits `src/search/engine.rs` and its tests;
#486 changes the caller of `format_report` in `src/tools/query.rs` and tests in
`src/tools/query_tests.rs`, not the engine or its tests. Both PRs also change
the README files and decision index. Overlapping edits must be reconciled when
integrating the PRs. This local comparison does not certify
the current remote PR state; remote reads were unavailable in the sandbox.

The host contract runs fmt, clippy for default/all-features, and nextest for
both feature sets, including ignored tests in all-features with
`SCOUT_NETWORK_TESTS=1`. This includes the new offline regressions; capture is
null and the Issue requires no media or real API response. Host check and
independent review must assess this document with the code and README changes.
Local focused results and limitations are passed through implementation
findings; they do not replace host check or prove browser behavior. The audit's
864 successes and one ignored test are historical evidence in #481, not a
result for this changed checkout.

### Repair verification (2026-10-08)

The first host check stopped at Clippy: `absolute_paths` rejected the qualified
`std::iter::repeat_n` call, and `needless_update` rejected a `ResearchReport`
fixture that specified every field before `..Default::default()`. Importing
`repeat_n` and removing that redundant update fixes the causes without changing
the fence logic or test assertions. The earlier focused-test success did not
establish lint success; the failed host check remains evidence of that gap.

On the repaired checkout based on `67ed30f`, `cargo fmt -- --check` and
`cargo clippy --offline --all-targets -- -D warnings` succeeded, as did the
same Clippy command with `--all-features`. The environment selected Rust 1.99.0;
Cargo declares a minimum of 1.98.1 and CI selects stable.
`cargo nextest run --offline --lib -E 'test(markdown::) | test(yaml::) | test(github::format::) | test(combined_research_output) | test(truncated_fences_keep) | test(failed_url_line) | test(format_report)' --profile ci`
passed 114 tests with no failures in each of default and `--all-features`.
It skipped 691 and 699 tests respectively; ignored tests were not run.
No browser, server, or real API was used. These focused results do not replace
the configured host check. Independent review also found the incorrect #486
overlap description above; comparison with its unchanged fixed ref corrected
that factual claim without replacing the reference or incorporating its code.

### Second host-check repair (2026-10-08)

Host check-2 passed both Clippy configurations, then default nextest ran
859 tests: 858 passed and T-C042 failed. All-features nextest was not reached.
T-C042 expects a later fence-looking inline-code line to trigger whole-body
YAML rewriting, including markers in an earlier closed block. The previous
implementation and independent review had claimed unchanged YAML behavior,
but stricter syntax in shared `track_fence` contradicted that claim. The
previous focused 114-test runs did not include this combined input; their
success and the earlier lint repair remain historical evidence, not proof of
this contract.

Restore shared `track_fence` to the implementation-base behavior and keep
strict Markdown recognition local to `close_open_fence`. Merely restoring
the shared function exposed a second failure in T-SE023: an info-bearing
decoy close could hide dangling-tail markers from the conservative tracker.
Completion therefore re-neutralizes that tail before adding the delimiter.
The existing marker and section-boundary assertions remain unchanged.

Extend T-FC030 with T-C042's observed converted inline-code shape and an earlier
closed block. This assertion failed before repair because the earlier markers
survived, and can run without the server required by T-C042. It adds a
diagnostic condition to an existing test rather than replacing the CLI
pipeline check: Readability/conversion/proxy composition still requires
T-C042 in host check. Keep the cap, UTF-8, heading, marker, and completion
controls and the formatter boundary regressions. No tests or detection
conditions are removed. These string fixtures have no socket/browser or
external response cost; no speed or maintenance improvement is claimed.

The original implementation findings and the first repair findings are the
handoff history; this section corrects their unchanged-YAML premise against
the current code and host failure. No supplied report blob or reference ID
was replaced. The accepted DR-0014/0016 and the proposed status of this record
are unchanged. Latest remote PR state remains unverified.

After this repair, the same focused nextest command recorded above passed
114 tests in default and all-features (0 failures, 691/699 excluded, ignored
not run; nextest execution 0.189/0.193 seconds). `cargo fmt -- --check`,
`cargo clippy --offline --all-targets -- -D warnings`, its `--all-features`
variant, and `git diff --check` passed. These commands used
`CARGO_TARGET_DIR=/private/tmp/scout-481-target` because the inherited build
directory is read-only in the sandbox. The first attempt using that inherited
directory failed to acquire its build lock and is not counted as a test run.
No browser or server was started; T-C042's original CLI regression remains
for the unchanged host check. The focused results verify the YAML leaf and
report structure, not the full fetch integration or real Chrome behavior.

Independent static re-evaluation after this repair found no mandatory
implementation, test-value, or documentation findings. It compared the
base, previous findings, check-2 failure and current artifacts, including
the verification update above; it did not independently rerun checks.
The unchanged configured host check remains pending.

### Required review repairs (2026-10-08)

The host's review-1 (`e95d86957151e71a6489e3c90863291292938d4501e8f5498c30b285d741d11e`,
R1-1/R1-2) found two required defects after the earlier check-3 success: the
line scanner manufactured a new code block after a normal HTML comment or
closed list fence, and completion rewrote tails already neutralized by the
report formatter. The earlier independent assessment above is historical;
check success and that assessment did not detect these defects. The reviewed
completion and double-rewrite conditions were still present at this repair's
start; the shared `track_fence` restoration remains valid, while the completion
scanner and separate tail rewrite did not.
The review is preserved at the host's `verification/review-1.json`; these IDs
identify its original findings, not a replacement assessment.

Extending T-GF048 first failed on the unchanged completion implementation with
`false completion` for `<!--\n```\n-->`. The same formatter fixture now includes
normal closed list fences, with and without README truncation. An initial
comment/list-specific scanner was rejected by independent evaluation because
normal pre/CDATA blocks still reproduced the same false completion. This led
to the context parser above rather than further ad hoc scanner exceptions.
Independent evaluation also identified the CR-only line boundary mismatch.
The parser's pinned source showed the trailing-tab boundary limitation; both
controls are now integrated into the existing lexical/formatter tests.

R1-2 is addressed by selecting required ranges before rewriting, rather than
assuming the two trackers agree or deleting standalone fetch validation.
T-GF048 checks README's whole-body fallback, T-SE023 retains the false-close
YAML/section check, and the existing closed-fence, cap, UTF-8, heading and fetch
YAML regressions remain. No tests or detection conditions are removed. These
in-memory cases add normal-input regression detection to existing tests without
sockets, browser or response fixtures. The parser dependency adds build and
maintenance cost; protecting existing valid Markdown justifies it. Reducing
lines, moving files or an unmeasured speedup is not an acceptance criterion.

Focused command results for this repair and the updated independent assessment
are recorded below. The unchanged host contract includes all these tests in
both configurations and keeps the original CLI/Chrome integration checks.
It requires no capture or contract-external acceptance action. Remote Issue/PR
state remains unverified; supplied #481 requirements and existing fixed source
references remain authoritative for this repair.

On the final repair code, the focused nextest command recorded above, using
`CARGO_TARGET_DIR=/private/tmp/scout-481-target`, passed 114 tests in each of
default and `--all-features`: 0 failures, 691/699 excluded, ignored not run;
nextest execution 0.198/0.203 seconds. The intermediate comment/list scanner
run had 113 passes and one fixture comparison failure: retained output included
a trailing newline. Correcting that comparison to ignore terminal whitespace
still rejects every synthetic delimiter and preserves the literal content and
section-boundary assertions. That scanner was subsequently replaced by block
parsing, rather than treated as a successful fix of R1-1.
`cargo fmt -- --check`, `cargo clippy --offline --all-targets -- -D warnings`,
its `--all-features` variant, and `git diff --check` passed. Clippy initially
rejected passing a non-Copy `ReportBody` by value; deriving Clone/Copy for the
value-only mode enum resolved it without a lint suppression. Rust 1.99.0 was
selected by the existing environment, above Cargo's 1.98.1 minimum; CI selects
stable. No browser/server or real service was started. These focused results
are evidence for this repair, not full host-check or real Chrome success.

The final independent static re-evaluation includes the parser's pinned source,
current code/tests, this document and the README/index descriptions, alongside
review-1 and the previous repair history. It found no mandatory defects and
classified R1-1/R1-2 as resolved. It checked the focused log summaries but did
not rerun the full check. Real Chrome/services and arbitrary CommonMark inputs
remain unverified; the configured host check still runs separately on this
artifact. No contract change, capture, commit, push or publication was made.

### Main integration (2026-10-08)

PR #487 repair starts at `d5ac13252c9f0f002a0d8f23144a938304cb1cc1` and
integrates main `22354cdad4917ce095b6403c3fdb89fc4988b68d` through the authorized
host merge. Unlike the original independent implementation described above,
this version includes #485's media-aware fetch path and regressions from main;
those are not new #481 work. Research composes main's heading-offset method
with this record's truncation, YAML defense and completion. T-SE021 and T-SE023
coexist, as do DR-0029 and this proposed record in the index. Merging does not
change either record's proposed status.

The six Rust files changed by this PR receive comment/doc cleanup only after
merge resolution. Fixtures, assertions, test IDs, registrations, settings and
non-doc Rust tokens remain identical to that resolved baseline. Descriptions
are limited to the actual observations: marker absence does not establish full
body retention, and a handcrafted formatter fixture is not a fetch integration
run. The existing plain-text and fence-boundary tests check the two composed
behaviors; no additional test, fixture, assertion or lint is added.
Earlier check/CI and independent-review results above remain historical
evidence for their respective versions; the merged artifact needs the
configured default/all-features host check and a new independent evaluation.
The contract's ignored all-features run covers Chrome; local offline structure
checks do not. Capture remains null and this Issue needs no media.

On this merged working tree, `cargo fmt -- --check`,
`cargo clippy --offline --all-targets -- -D warnings`, its `--all-features`
variant and `git diff --check` succeeded. The focused nextest command recorded
above passed 116 tests in each feature configuration, with 0 failures and
691/699 excluded respectively; ignored tests were not run. Nextest execution
was 0.974/0.595 seconds, excluding compilation. Both Cargo checks used the
existing writable `CARGO_TARGET_DIR=/private/tmp/scout-481-target` and Rust
1.99.0. These runs used no browser, server or real Web/GitHub response. They
confirm the local composition regressions, not the full host contract, CI,
current remote mergeability or a new independent acceptance. No runtime or
maintenance improvement is inferred from comment cleanup or these timings.
