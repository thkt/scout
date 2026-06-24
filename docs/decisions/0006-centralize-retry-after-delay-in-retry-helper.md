---
status: "accepted"
date: 2026-05-16
decision-makers: thkt (project owner)
---

# Centralize retry-after delay in retry helper

## Context and Problem Statement

scout has 3 HTTP client modules (`brave/client.rs`, `github.rs`, `slack.rs`). Audit `/audit main..HEAD` (2026-05-16, snapshot `audit-2026-05-15-181215.json`) identified RC-01: each module independently re-implements the retry-after delay function (`brave_delay`, `github_delay`, `slack_delay`) with bit-identical bodies, while `is_retriable` is similar in structure but diverges in semantics. The duplication risks drift if retry strategy is tuned in one module but not the others. How should retry-after delay be consolidated?

(Template note: select-adr-template.sh suggested `technology-selection`; overridden to `architecture-pattern` because this decides code-structure policy, not a library choice.)

## Decision Drivers

- DRY for "same knowledge": delay formula is identical across backends.
- Avoid over-abstracting when ADR-0005 fixes Brave as the single search backend.
- critic-design verdict on Retryable trait approach: gain ≈ 6 lines vs cost = trait + 3 impls + `retry_with` generics rewrite. Net negative.

## Considered Options

- Helper function in `retry.rs` (`retry_with_rate_limit`) + per-backend extractor closure.
- `Retryable` trait + generic `retry_with<E: Retryable>`.
- `macro_rules!` to generate `*_delay` from a variant pattern.

## Decision Outcome

Chosen option: "Helper function in `retry.rs` + per-backend extractor closure", because it removes the bit-identical `*_delay` bodies without introducing trait indirection that the divergent `is_retriable` rules would defeat anyway.

### Consequences

- Good, because delay formula lives in one place (`retry.rs`); a tuning change touches one file.
- Good, because each backend keeps full control over `is_retriable`, where the semantics genuinely differ (brave matches `Server(_)`, github matches `Api { code: 500..=599, .. }`, slack matches `Network(_) | Timeout(_)`).
- Good, because no new trait is exposed; `retry.rs` API stays narrow.
- Bad, because the per-backend extractor closure (~3 lines) is the residual duplication: each backend must say "RateLimited → retry_after, else None".

### Confirmation

`grep -rn 'fn .*_delay' src/` returns zero hits for `brave_delay` / `github_delay` / `slack_delay` after refactor. `retry_with_rate_limit` callers in the 3 client modules verify the closure shape. Existing tests (345+) continue to pass without behavior change.

## Pros and Cons of the Options

### Helper function in `retry.rs` (chosen)

A `retry_with_rate_limit<T, E, F, Fut>` function wraps `retry_with` and embeds the `retry_after_or_backoff(extract(e), attempt)` formula. Each backend supplies only the `RateLimited` extractor closure.

- Good, because deletes 3 named `*_delay` functions (~18 lines total)
- Good, because no new trait, no generics rewrite of `retry_with`
- Bad, because each backend still writes a ~3-line extractor closure inline at the call site

### Retryable trait

A `pub(crate) trait Retryable { fn is_retriable(&self) -> bool; fn retry_after(&self) -> Option<u64>; }`. `retry_with` is rewritten as generic over `E: Retryable`.

- Good, because retry semantics live next to the error definition (one place per backend)
- Bad, because `is_retriable` still copies divergent per-backend logic into each impl — the trait only abstracts `retry_after()`, gaining ~6 lines while adding a trait + 3 impls + generics rewrite
- Bad, because future readers must learn the trait before reading retry code

### macro_rules! to generate `*_delay`

A `define_delay!(BraveError)` macro generates the `*_delay` function for each backend.

- Good, because zero code at the call site
- Bad, because macros add cognitive load and rust-analyzer support is weaker than for plain code
- Bad, because the macro hides 3 generated functions, making `grep` and stack-trace navigation harder

## More Information

### Deferred concerns

Two adjacent audit findings are intentionally not addressed here.

- FN-MD-002 (`engine::research` returns `Result<_, BraveError>` leaking Brave error into engine layer): wrapping in a `SearchError` enum is pure overhead under ADR-0005's single-backend constraint. Defer until a second backend exists.
- FN-HU-002 (`RateLimited{retry_after}` + `Network` variant shape duplicated across 3 enums): introducing a shared `RateLimitInfo` struct couples 3 independent error enums for cosmetic gain. Defer.

### Adjacent dedup: From transient cluster

`tools/errors.rs` has a parallel duplication: `From<BraveError>` and `From<GitHubError>` for `ScoutError` share a structurally identical 4-arm transient cluster (RateLimited / Server-or-Api-5xx / Network / Network-is-timeout). This ADR's scope includes consolidating that pattern via three small `Classification` constructors (`transient_retry`, `transient_network`, `timeout_retry` at `src/tools/errors.rs:43-58`) rather than via macro or trait. Same reasoning as the main decision: helpers are type-safe, debuggable, and avoid macro indirection.

### Reassessment Triggers

- A second HTTP backend is added (ADR-0005 currently forbids this for search). At that point, re-evaluate whether the trait abstraction's cost has dropped below its benefit.
- `retry.rs` API needs to expose more retry policies (e.g., circuit breaker). The helper approach may scale less well than a trait.
