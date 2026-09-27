# Repository-wide verification experiment — 2026-09-27

The experiment broadened the mention-only comparison to the repository's complete
test suite, 21 deterministic operations compared with Go, independent CLI
fixtures, and a sample of deliberate faults to test the tests themselves.

Final results: **856 default-feature tests passed; 865 all-feature tests passed
with zero skips, including real Chrome; 31/31 independent CLI fixtures passed;
both Clippy configurations and formatting passed.** The Go comparison has zero
unresolved differences after classifying the 57 intentional ref-policy cases.
All 21 deliberate faults are detected in the final isolated mutation run.

## Findings and changes

1. **Content-Type case sensitivity was a production defect.**
   `Text/HTML`, `TEXT/PLAIN`, and mixed-case XML media types were rejected even
   though their lowercase forms worked. This produced 24 differences in the Go
   comparison and five failures through the real CLI. The type/subtype are
   case-insensitive under [RFC 9110 section 8.3.1](https://www.rfc-editor.org/rfc/rfc9110.html#section-8.3.1).
   The fix normalizes the comparison while preserving the original header in
   rejection diagnostics. Two tests cover acceptance and continued rejection of
   unsupported media types; the new acceptance test failed before the fix.

2. **Three deliberate faults survived the existing tests.**
   Swapping `GITHUB_TOKEN` and `GH_TOKEN`, fetching `depth + 1` sources, and
   rejecting a Retry-After of exactly 300 seconds all passed their existing module
   suites. New tests assert both-token precedence, actual HTTP request paths and
   counts at the depth boundary, and the inclusive retry cap. These are test gaps,
   not three additional bugs in the production implementation.

3. **The browser smoke test had a weak success assertion.**
   A page containing only the string `example` could pass, including a Chrome
   error page naming the destination host. The assertion now requires the
   expected `Example Domain` content. This finding came from reviewing the test,
   not from the mutation sample.

4. **Git ref differences are an existing policy choice.**
   The Go model implements full `git check-ref-format --allow-onelevel` rules;
   scout intentionally implements a subset and leaves remaining validation to
   GitHub. The historical disposition is in
   `docs/audit/2026-05-13-undocumented-decisions-part2.md`, candidate 13. The 57
   differences are retained as evidence rather than changing production or
   quietly weakening the reference model to make everything match.

The previous mention-whitespace correction remains a separate experiment under
`../mention-differential/`.

## Evidence

- `before.json` / `after.json`: seeded corpus, per-operation counts, differences,
  source hashes, reference hash and toolchain versions. The original run compared
  67,997 inputs. The Go model and corpus hashes are unchanged between the two
  runs. After the fix, the only remaining differences are the 57 documented ref
  cases; Git's own validator agrees with the Go result for all 57.
- `cli-before.json` / `cli-after.json`: real-process checks with a local mock
  proxy. No service tokens or caller proxy settings are inherited. Request counts
  ensure a scenario cannot accidentally pass without contacting the fixture.
- `mutations-before.json` / `mutations-after.json`: deliberate-fault results and
  exact failing test names. The first run attempted 20 faults: 16 detected,
  three survived, and one invalid mutation (a missing comma). Compilation errors
  are never counted as successful fault detection. The corrected YAML mutation
  and all surviving faults are rerun, along with a focused HTML fault and a
  regression of the Content-Type fix. The final isolated run repeats all 21
  faults against the current source. `mutation-summary.json` records
  **21 of 21 detected**. In particular, each of
  the three previously surviving faults now fails its newly added regression test.
- `INVENTORY.md` / `inventory.json`: all 99 Rust source/test files, with explicit
  markings for additional Go and mutation checks. An inventory entry is not a
  claim of branch coverage.
- `validation.json`: final commands and outcomes, including both feature
  configurations, real Chrome, linting, and formatting.

## Reproduce

Run from the repository root, with Rust, Python 3, and Go installed:

```sh
python3 experiments/repository-differential/run.py --report /tmp/scout-comparison.json
python3 experiments/repository-differential/mutations.py --report /tmp/scout-mutations.json
cargo build --locked --all-features
python3 experiments/repository-differential/cli_probes.py --report /tmp/scout-cli.json
SCOUT_NETWORK_TESTS=1 cargo test --locked --all-targets
SCOUT_NETWORK_TESTS=1 cargo nextest run --locked --all-features --run-ignored all --profile ci
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo fmt -- --check
```

`run.py` accepts `--go /absolute/path/to/go`. Its nonzero result deliberately
surfaces all differences, including the documented ref-policy differences. Inspect
the report to distinguish these from newly introduced differences.

The Rust comparison and mutation runners copy source into temporary directories;
they never inject faults into the working tree. Their build artifacts live in
separate `target/repository-differential/` and `target/repository-mutations/`
directories. Sharing the ordinary `target/` initially allowed Cargo to reuse an
instrumented test binary in a normal run; this was detected when the experiment's
extra test ran without its required input file. The runners now isolate their
artifacts, and the affected normal suite is rebuilt before final validation. Compilation
is offline and locked: dependencies must already be cached. The mutation runner
requires local socket access, and the all-features suite additionally requires
Chrome/Chromium and connectivity to example.com. `SCOUT_NETWORK_TESTS=1` turns
missing local socket support into a failure instead of a silent skip.

## Scope and limits

All checks ran on this macOS ARM64 host; other operating systems were not tested.
The Go code is a contract model, **not a complete second implementation of scout**.
Models were derived from tests, comments, decisions, standards and inspected code;
this broader round is not a blind, independent-author implementation. The code
uses different standard-library primitives where possible and includes a stricter
external ref model. The comparison alone cannot establish which side is correct.

Existing tests were run across the repository, while additional differential and
mutation checks are sampled. Async scheduling, complete HTML/Readability behavior,
live Brave/GitHub/Slack services, every failure branch, and every possible input
have not been independently reimplemented. The real CLI fixtures test local HTTP
fetch, input errors for all six commands, and URL rejection; they do not claim six
live backend success paths. Tests and faults are finite evidence, not a guarantee
of correctness or an exhaustive mutation score. No performance comparison was made.

## PR validation

The historical experiment reports above were produced in a working tree with an
uncommitted dependency-lock update. That unrelated `Cargo.lock` change is not
included in this PR. The PR was prepared separately from upstream main `4ad81f0`
with its committed lockfile; `pr-validation.json` records verification of that
exact dependency configuration. The original experiment reports are preserved
rather than relabeled as runs against a different lockfile.
