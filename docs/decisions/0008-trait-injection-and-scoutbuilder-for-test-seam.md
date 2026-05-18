---
status: "accepted"
date: 2026-05-19
decision-makers: thkt (project owner)
---

# Test seam architecture via `Arc<dyn Trait>` fields and `ScoutBuilder`

## Context and Problem Statement

Issue #103 surfaced four overlapping gaps in the dependency graph of `Scout`:

- `github.rs::secs_until_ratelimit_reset` read `SystemTime::now()` directly; tests could not pin the wall clock so `retry_after` arithmetic flaked under load.
- `retry.rs::jittered_backoff` called `fastrand::u64` as a global; deterministic replay (same seed → same backoff) was not possible without an env-only `FASTRAND_SEED` knob.
- `github.rs::resolve_token` hard-coded the `GITHUB_TOKEN`/`GH_TOKEN`/`gh auth token` chain; any test that exercised an authenticated path either had to mutate `std::env` (forbidden by `unsafe_code = "forbid"`) or spawn the real `gh` binary.
- `tools.rs::Scout::new` constructed `BraveClient`, `RuntimeConfig`, and the GitHub client eagerly from env vars. The `scout_with_github` test helper reached into private `Scout` fields with a struct literal to point clients at wiremock, coupling test code to the struct layout.

The acceptance criteria asked for: `ScoutBuilder` + the four trait abstractions, with at least one existing test migrated through the new seam, while keeping `Scout::new()` callable from `lib::run` (production entry point).

## Decision Drivers

- `unsafe_code = "forbid"` blocks `std::env::set_var` in unit tests, so any env-dependent seam must accept an injectable source rather than monkey-patch the global env.
- `Scout::new()` is the single production entry point and is called by `lib::run` (and indirectly by integration tests). Its signature (`pub async fn new() -> Result<Self, ScoutError>`) must stay stable to avoid a ripple-edit across the CLI surface.
- Production paths must stay zero-extra-allocation per request. Injection cost should be paid at construction time, not per `get_json` call.
- The `scout` binary is single-runtime (tokio) and single-backend (Brave + GitHub REST). Type-parameter explosion across these is not justified.
- MSRV is `rust-version = "1.95"`; `Pin<Box<dyn Future>>` is the only practical way today to put an `async` method behind `dyn Trait` while preserving `Send`, because native `async fn in trait` returns `impl Future` and is not object-safe.

## Considered Options

- (Chosen) `Arc<dyn Trait>` field on `Scout` and `GitHubClient` + a `ScoutBuilder` with `from_env` / `for_test` constructors and chainable `with_*` setters.
- Generic `Scout<C: Clock, R: Rng, T: TokenSource>` type parameters for compile-time DI (zero-cost monomorphization).
- Test-only `for_test()` constructor on `Scout` with no `ScoutBuilder`, mutating the resulting `Scout` in place via `with_*` methods on `Scout` itself.

## Decision Outcome

Chosen: `Arc<dyn Trait>` fields with a `ScoutBuilder` test seam.

Three of the four proposed traits live in dedicated modules: `clock::Clock` (`SystemClock` / `FixedClock(u64)`), `rng::Rng` (`FastrandRng` / `SeededRng(Mutex<fastrand::Rng>)`), and `token_source::TokenSource` (`GhCliSource` / `StaticTokenSource(Option<Redacted>)`). The fourth, `DnsResolver`, is deferred (see *Deferred concerns* below).

`Scout` holds `clock: Arc<dyn Clock>`, `rng: Arc<dyn Rng>`, and `token_source: Arc<dyn TokenSource>`. These are forwarded into `GitHubClient` on first `Scout::github()` call (`OnceCell` lazy init), so non-GitHub commands (`search`, `fetch`, `research`) never pay the `gh auth token` cost.

`ScoutBuilder::from_env()` is the production entry point; `Scout::new()` is now a thin `Ok(ScoutBuilder::from_env()?.build())` wrapper that preserves the async signature. `ScoutBuilder::for_test()` is the env-isolated test entry point; `with_clock` / `with_rng` / `with_token_source` / `with_brave_endpoint` / `with_github_endpoint` chain setters compose freely.

`TokenSource::fetch` returns `Pin<Box<dyn Future<Output = Option<Redacted>> + Send + '_>>` (aliased as `TokenFuture<'_>`) because object-safety of the trait is the load-bearing constraint — `Arc<dyn TokenSource>` would not compile with a native `async fn in trait` (returns `impl Future`, not object-safe). No `async_trait` crate is added.

### Consequences

- Good, because tests are isolated from wall clock, global RNG, and the `gh auth token` subprocess. `FixedClock(1000)` + `SeededRng::new(7)` + `StaticTokenSource(Some(Redacted::new("token")))` produces fully deterministic behavior.
- Good, because `Scout::new()` is signature-stable; `lib::run` and integration tests compiled unchanged across all six PRs.
- Good, because `ScoutBuilder::for_test()` reads no `SCOUT_*` env, so a stray `SCOUT_MAX_RETRIES=abc` in the developer environment cannot panic unrelated tests (problem first reported by Codex P2 during PR 5 review).
- Good, because the `with_*` setters compose: `T-SB004` injects `FixedClock` and a wiremock GitHub endpoint in a single chain, exercising the full `Scout::github()` plumbing through to `secs_until_ratelimit_reset`.
- Bad, because `Scout` gains three new fields, growing API surface even though the fields are private. Field doc comments now carry their own WHY (lazy-init plumbing) load.
- Bad, because `Pin<Box<dyn Future>>` adds visual noise at every `TokenSource::fetch` implementation. The alias mitigates but does not erase the cost.
- Bad, because issue #103 produced six PRs (#127–#132) all merged in a single afternoon (2026-05-18), each kept reviewable in isolation; the cumulative diff is the documentation lookup cost.

### Confirmation

- PRs #127 (tracing `try_init`), #128 (`Clock` trait), #129 (`Rng` trait), #130 (`TokenSource` trait), #131 (`ScoutBuilder`), #132 (test helper migration) are all merged.
- `cargo test --offline` reports 392 lib + 11 integration tests passing.
- `SCOUT_MAX_RETRIES=abc cargo test --lib --offline scout_lazy_github_initially_none` passes, confirming `for_test()` env isolation.
- `grep -n 'Scout {' src/tools.rs | grep -v 'pub struct\|impl Scout\|-> Scout'` returns the single private struct literal at `tools.rs:701` inside `ScoutBuilder::build`; the previous `scout_with_github` reach-in literal is gone.
- `T-SB001`/`T-SB002`/`T-SB003` assert `Arc::ptr_eq` between the injected `Arc<dyn Trait>` and `Scout.{clock,rng,token_source}`. `T-SB004` exercises the full seam end-to-end (`ScoutBuilder::for_test().with_clock(FixedClock(1000)).with_github_endpoint(wiremock_uri).build()` → `Scout::github().get_repo("owner","repo")` → asserts `RateLimited { retry_after: Some(600) }` derived from `x-ratelimit-reset=1600 − clock=1000`).

## Pros and Cons of the Options

### `Arc<dyn Trait>` + `ScoutBuilder` (chosen)

`Scout` holds `Arc<dyn Trait>` fields; `ScoutBuilder` constructs both the production graph (`from_env`) and the test graph (`for_test`) with shared `build_default_clients()` helper to prevent drift.

- Good, because `Scout::new()` stays signature-stable so the CLI entry point never recompiles its callers across the rollout.
- Good, because each trait can grow new implementations (e.g. a future `RecordingRng` for fuzz seeds) without touching `Scout`'s public type.
- Good, because `Arc::ptr_eq`-based seam assertions become trivially writable (`T-SB001`/`002`/`003`), separating "did the slot wire" from "does the wired thing behave" (`T-SB004`).
- Bad, because the `Pin<Box<dyn Future>>` for `TokenSource` will look outdated once `async fn in dyn Trait` ships, requiring a follow-up rewrite. Documented as a reassessment trigger.
- Bad, because `dyn` dispatch adds one indirection per `Clock::now_secs` and `Rng::u64_below` call. These are not in the request hot path (retry construction and rate-limit arithmetic only), so the cost is acceptable.

### Generic `Scout<C: Clock, R: Rng, T: TokenSource>`

Type parameters propagate through `Scout`'s methods; production is `Scout<SystemClock, FastrandRng, GhCliSource>`, tests pick test doubles at the type level.

- Good, because dispatch is fully static (zero-cost monomorphization) and `async fn in trait` works directly without `Pin<Box>` boilerplate.
- Good, because the compiler enforces seam wiring at the type level (cannot construct a `Scout` with a missing dependency).
- Bad, because every caller of `Scout` (`lib::run`, integration tests, future subcommand modules) has to either name the concrete type or be generic itself. The CLI surface bloats with `impl Clock` / `impl Rng` parameters that have no business being there.
- Bad, because monomorphization compiles `Scout::run` once per `(Clock, Rng, TokenSource)` tuple. With three traits and two implementations each, that is up to 8 specializations — measurable compile-time hit on a CLI binary that ships a single concrete production combination.
- Bad, because `Scout::new()` cannot return a single named type without `impl Trait` in return position, which is a breaking-change lock for the public API.

### Test-only `for_test()` on `Scout` with `with_*` setters on `Scout` itself

Skip `ScoutBuilder` entirely; add `#[cfg(test)] fn for_test()` and `#[cfg(test)] fn with_clock(self, ...)` directly on `Scout`.

- Good, because the type surface stays at one struct.
- Good, because the test-time API looks identical to what `ScoutBuilder` would offer.
- Bad, because production and test construction paths diverge inside the same struct. Future production-side seams (e.g. config-file overrides per ADR-0007) would either need a separate path or end up sharing logic with the test-only branch, inviting drift.
- Bad, because `with_*` on `Scout` consumes-and-returns `self` chains. Combined with `OnceCell<GitHubClient>` initialization, the ordering of `with_clock` (changes `Scout.clock`) versus pre-setting the github cell (needs the final `clock`) becomes implicit. A separate `Builder` makes the staging explicit: configure, then `build()` finalizes.
- Bad, because env-isolation requires a parallel `for_test()` that mirrors `from_env`'s `Client::builder()` chain. The shared `build_default_clients()` helper (chosen design) still applies, but the lack of a `Builder` means the test code path looks asymmetric — a private constructor for one purpose, a public chain API for another.

## More Information

### Deferred concerns

- **`DnsResolver` `Arc<dyn ...>`-ification**: Issue #103 listed `dns: Arc<dyn DnsResolver>` as a proposed action, but the existing `DnsResolver` trait in `fetch/ssrf.rs` is generic-style (`fn lookup(&self, host: &str, port: u16) -> impl Future<Output = Result<Vec<IpAddr>, FetchError>> + Send`) with a `Clone + Send + Sync + 'static` bound. Object-safety requires changing the return type to `Pin<Box<...>>` and dropping `Clone`; the SSRF path's `Arc<TokioDnsResolver>` clone semantics need re-analysis. Captured as a separate issue rather than expanded into PR 4. The ScoutBuilder slot for it is already shaped by precedent.
- **`with_brave` / `with_github`** that inject pre-built `BraveClient` / `GitHubClient` instances: not added. The `with_*_endpoint(&str)` setters cover every existing test case by re-using the builder's `http` client to construct the test double. If a future test needs a fully custom client (e.g. a recording proxy), the precedent is to add the setter then, not now.

### Reassessment Triggers

- `async fn in dyn Trait` becomes idiomatic in stable Rust without the `Send` auto-bound dance (currently requires `trait_alias` or returning `Pin<Box<...>>` to make the future `Send`). At that point, simplify `TokenSource::fetch` from `TokenFuture<'_>` to a native `async fn`.
- The `DnsResolver` follow-up issue lands. Re-evaluate whether to keep generic-style traits for "always single-implementation in production" cases or push everything to `Arc<dyn ...>` for uniformity.
- A second async runtime besides tokio is added (currently unforeseen). The `Send + Sync` bounds on `Clock` / `Rng` / `TokenSource` were chosen for tokio compatibility; another runtime may require revisiting `Send` strategy.
- `Scout` accumulates a fifth `Arc<dyn Trait>` field beyond `clock` / `rng` / `token_source` / future `dns`. At that point, default-derived `ScoutBuilder` (e.g. via a private `Default` impl on the trait collection) may pay for itself.
