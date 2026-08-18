---
status: "accepted"
date: 2026-05-17
decision-makers: thkt (project owner)
---

# Unify Brave client init via factory and Result-based https check

## Context and Problem Statement

`BraveClient` has two construction paths that diverge in validation: `from_env` (production, reads env, trims key, uses constant `API_BASE`) and `with_base_url` (test-only, hardcoded "test-key", arbitrary `base_url`, no validation). The HTTPS enforcement (`redacted::assert_https`) is a debug assertion that becomes a no-op under `cfg!(test)`. Audit `/audit main..HEAD` (2026-05-16) recorded this as RC-02 with two structural findings: FN-MT-002 (`from_env` reads env directly, not injectable, untestable without `unsafe set_var`) and FN-MX-002 (`assert_https` bypassed in test path, HTTPS enforcement never exercised by the test suite). How should the test and production paths be unified so that key validation and HTTPS enforcement are exercised by tests?

(Template note: select-adr-template.sh would suggest `technology-selection`; the intent here is structural — code-organization policy for client construction — so this is `architecture-pattern`.)

## Decision Drivers

- `unsafe_code = "forbid"` blocks `std::env::set_var` in unit tests; production env path is otherwise untestable.
- HTTPS enforcement is dead code in `cargo test`; FN-MX-002 surfaces the gap.
- ADR-0005 fixes Brave as the single search backend; avoid over-abstracting for hypothetical second backends.
- Critic-design verdict on Approach A (`BraveConfig` struct) and Approach C (HTTPS mock infrastructure): both weakened by half-fixes or scope inversion against medium-severity findings.

## Considered Options

- Hybrid B+: factory parameter for env DI + `validate_https` returns `Result` + per-client `skip_https_check` flag (chosen).
- Approach A: introduce `BraveConfig` struct with unified constructor and a `for_wiremock` escape hatch.
- Approach B (pure): only add `from_env_with<F>`; leave `assert_https` and `with_base_url` untouched.
- Approach C: convert all wiremock tests to HTTPS via rustls + self-signed cert.

## Decision Outcome

Chosen option: "Hybrid B+", because it closes both findings with a small, localized refactor that keeps wiremock tests on `http://127.0.0.1` while ending the global `cfg!(test)` bypass.

### Consequences

- Good, because the HTTPS check is no longer silently disabled by `cfg!(test)`; it runs in every test that doesn't explicitly opt out.
- Good, because the env-reading path is testable via injectable closures, matching the `from_env_with` pattern recommended in `rules/frameworks/rust/LANG.md`.
- Good, because the test-only bypass is now an explicit per-client flag set only by `with_base_url`; it cannot accidentally apply to a production-path client.
- Good, because `validate_https` becomes a pure function whose logic is directly unit-testable without wiremock.
- Bad, because `BraveError` gains an `InsecureBaseUrl` variant that, given the constant `API_BASE`, will never fire on the production path. The variant exists to make the test-path validation observable; this is accepted cost.

### Confirmation

`grep -n 'cfg!(test)' src/redacted.rs` returns zero hits after the change. Unit tests cover `validate_https("http://...")` → `Err(InsecureBaseUrl)` and `validate_https("https://...")` → `Ok(())`. The existing wiremock-based tests in `brave/client/http_tests.rs` continue to pass via the explicit `skip_https_check` flag on test-only clients.

## Pros and Cons of the Options

### Hybrid B+ (chosen)

`from_env_with<F>(http, get_var: F)` adopts the LANG.md factory-parameter pattern; production `from_env` delegates to it with `std::env::var`. `assert_https` becomes the generic `validate_https<E>(url: &str, err: impl FnOnce() -> E) -> Result<(), E>` (`src/redacted.rs`), so each caller injects its own error variant; `BraveClient` supplies `|| BraveError::InsecureBaseUrl` (`src/brave/client.rs` の `send_request`). `send_request` gates the call through the per-client `should_check_https()` helper (`src/brave/client.rs` の `should_check_https`), which returns `false` only when `skip_https_check` is set, and only `with_base_url` (cfg(test)) sets it.

- Good, because both findings are closed with one cohesive change.
- Good, because wiremock churn is zero; the new flag isolates the bypass to a single constructor.
- Bad, because the bypass still exists, narrowly scoped. It is intentional: wiremock cannot serve HTTPS without infrastructure expansion outside the scope of RC-02.

### Approach A: `BraveConfig` struct with `for_wiremock` escape hatch

A new struct holds `api_key` and `base_url`; `new()` validates both; `for_wiremock` (cfg(test)) skips HTTPS.

- Good, because the construction surface becomes one type.
- Bad, because `for_wiremock` is the same bypass renamed; FN-MX-002 survives behind a new door.
- Bad, because `InsecureBaseUrl` variant is still dead in production but with no testable production scenario.

### Approach B (pure factory, no HTTPS fix)

Only `from_env_with<F>` is added.

- Good, because the change is minimal.
- Bad, because FN-MX-002 (HTTPS untested) remains open; audit re-opens.

### Approach C: HTTPS wiremock infrastructure

rustls + self-signed cert across `brave/client.rs` (~8), `slack.rs` (~4), `github.rs` (~8) wiremock sites.

- Good, because every test path goes through real HTTPS.
- Bad, because 20+ test rewrites scope-creep into Slack and GitHub.
- Bad, because the cost of HTTPS mock infrastructure exceeds the medium severity of FN-MX-002.

## More Information

### Deferred concerns

- FN-MC-010 (CLI test for legacy `GEMINI_API_KEY` regression) is dropped: the Gemini code path was removed in the v2.0.0 migration, so there is no regression to guard. The old env var is now ignored as any other unknown variable would be.

### Reassessment Triggers

- `BraveError::InsecureBaseUrl` proves to be dead code over six months. Re-evaluate whether the validation belongs at construction time instead of request time.
- The `#[cfg(test)]` bool that each of the three clients carries grows past a single flag, or a fourth client needs the same shape. Approach A was rejected because `for_wiremock` renames the bypass and `InsecureBaseUrl` stays dead in production; neither reason moves with the client count, so the count alone is not the trigger.
