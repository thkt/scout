# Slack mention differential experiment

## Result (2026-09-27)

Comparing 80,956 unique deterministic inputs found **one defect class**, producing
715 differing outputs before the fix and zero afterward. The 37 explicit
input/output oracles had 20 Rust failures before the fix and zero Go failures;
both implementations pass all 37 after the fix. These are synthetic cases, not
an estimate of a real-world error rate.

Example: `<@U\u3000123>` (an actual ideographic space in the ID) was rewritten to
`@U\u3000123` instead of being preserved verbatim. The production parser tested
bytes with `is_ascii_whitespace`, allowing 19 non-ASCII White_Space characters
and ASCII vertical tab into the ID. Those IDs also entered the user lookup queue.
Using `char::is_whitespace` fixes the documented no-whitespace rule without
introducing a stricter ID alphabet.

The added regression test covers all 25 White_Space characters, with and without
a label, and verifies preservation, lookup exclusion, and continued substitution
of the valid mention that follows. It failed before the fix (20 existing module
tests passed) and all 21 module tests passed after the fix.

Validation: `cargo test --locked --lib slack::` passed all 97 Slack tests;
`cargo fmt -- --check` passed. The full repository suite was not run. The changed
Rust files pass `git diff --check`; a repository-wide whitespace check reports
pre-existing trailing whitespace in an unrelated, already-modified audit file.

`before.json` and `after.json` contain outputs, toolchain versions, and source
hashes. The Go reference hash is identical in both runs. The baseline was captured
before the harness gained its direct Rust module-test invocation.

## Reproduce

Requires Go, Rust, and Python 3 on PATH; no Go modules, network requests, or Cargo
dependencies are needed for this comparison. Run from the repository root:

```sh
python3 experiments/mention-differential/run.py --report /tmp/scout-mention-after.json
```

Use `--go /absolute/path/to/go` when Go is not on PATH. The initial experiment used
Go 1.27.1 downloaded from the official distribution into `/tmp` with SHA-256
verification; it did not install Go globally. The runner builds binaries and a Go
build cache in a temporary directory and removes them on exit. Exit status is 1
on a comparison/oracle mismatch; compilation and module-test failures also fail
the run. Inputs combine bounded exhaustive token sequences, seeded random
sequences, whitespace-focused cases, and explicit expected outputs.

## Scope and method

Scope: compare the production Rust mention substitution with an independent Go
reference. The reference was written after reading
`src/slack/mention/mention_tests.rs`, before reading `src/slack/mention.rs`.
This is a different implementation by the same author, not a blind independent
review or evidence that either language is inherently more correct.

## Contract fixed before inspecting production

- Recognize `<@ID>` and `<@ID|label>`; the first `>` ends a token.
- IDs must be nonempty and contain neither whitespace nor `<`.
- Malformed tokens are preserved, including nested mention syntax; scanning
  continues after the rejected token.
- Unclosed tokens remain unchanged.
- Replacement precedence: nonempty cached name, nonempty embedded label, ID.
- Preserve all surrounding text, including Unicode and line breaks.
- Fixed comparison cache: `U123=Bob`, `U100=Alice`, `EMPTY=""`.

The first-delimiter behavior and labels containing unusual characters are
experimental assumptions where the existing tests do not fully specify behavior.
Differences must be classified against the contract, not treated as Rust bugs
automatically. This experiment covers substitution only, not returned span
offsets, lookup ordering, Slack HTTP behavior, or web extraction quality.

The harness uses hex-encoded UTF-8 lines to carry arbitrary multiline inputs.
Go uses a regular expression and replacement callback; the Rust adapter calls
the production source directly rather than copying it.
