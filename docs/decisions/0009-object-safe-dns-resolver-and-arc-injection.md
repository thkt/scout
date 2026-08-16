---
status: "accepted"
date: 2026-05-19
decision-makers: thkt (project owner)
---

# Object-safe `DnsResolver` and `Arc<dyn DnsResolver>` injection via `ScoutBuilder`

## Context and Problem Statement

ADR-0008 unified test-seam injection across `Clock` / `Rng` / `TokenSource` through `Arc<dyn Trait>` fields on `Scout` and chainable `ScoutBuilder::for_test().with_*()` setters. The fourth proposed seam, `DnsResolver`, was left out and recorded under _Deferred concerns_ because the existing trait shape required structural changes that did not fit inside the PR sequence #127–#132.

Today the picture in `src/fetch/ssrf.rs` is:

- `pub(crate) trait DnsResolver: Clone + Send + Sync + 'static { fn lookup(&self, ...) -> impl Future<Output = ...> + Send; }` — generic-style with a `Clone` bound, not object-safe.
- `src/tools.rs::Scout::fetch` and `Scout::research` reach for `&TokioDnsResolver` literals; no `Scout.dns` slot exists.
- `src/fetch.rs::fetch_with_cdp(..., resolver: impl ssrf::DnsResolver, ...)` consumes the resolver and re-clones it across a `tokio::spawn` boundary via `Clone::clone(resolver)` (`fetch.rs:146`), depending on the trait's `Clone` bound for the CDP intercept task.
- `src/search/engine.rs` carries `&TokioDnsResolver` references into `engine::research` tests.

Issue #134 asks for parity with ADR-0008 — object-safe trait, `Scout.dns: Arc<dyn DnsResolver>`, `ScoutBuilder::with_dns`, and a test double for resolution-strategy injection (private-IP fail-fast, NXDOMAIN, mixed results). Without injection, every SSRF-path test must either talk to a real DNS resolver or live with `TokioDnsResolver` and lose the ability to script outcomes.

## Decision Drivers

- `unsafe_code = "forbid"` prevents mocking DNS via `std::env::set_var` or `/etc/hosts` shims. Injection at the type level is the only sanctioned seam.
- SSRF defense is a hard project constraint (`.claude/OUTCOME.md` Constraints: "全 fetch 経路で SSRF 防御を必須とする"). Any change must keep the contract that `fetch_page` / `engine::research` cannot reach private IPs.
- `Arc<dyn DnsResolver>` requires object-safety: dropping `Clone` and replacing `impl Future` with `Pin<Box<dyn Future + Send + '_>>`. ADR-0008 already paid this cost for `TokenSource`; reusing the same pattern keeps the four traits visually uniform.
- `fetch_with_cdp` spawns an intercept task that needs `resolver: Send + 'static`. The old `Clone + 'static` bound made an owned-clone trivial; with `Arc<dyn DnsResolver>` the clone is an `Arc::clone`, which is also cheap. The signature of internal helpers must allow this.
- `ssrf_check` and `download` only read the resolver. Passing `Arc` everywhere would be over-budget; `&dyn DnsResolver` is enough for pure-read sites. The split (`Arc` at task-spawning sites, `&dyn` at pure-read sites) keeps overhead at one indirection per call without proliferating `Arc::clone`.
- Tests must be able to inject `StaticDnsResolver(Vec<IpAddr>)` / `FailingDnsResolver(pub String)` and prove the injected double was used by an end-to-end assertion, not just `Arc::ptr_eq`. ADR-0008's `T-SB004` is the precedent.

## Considered Options

- (Chosen) Object-safe `DnsResolver` with `Pin<Box<dyn Future + Send + '_>>` return, `Clone` bound removed. `Scout.dns: Arc<dyn DnsResolver>`, `ScoutBuilder::with_dns` setter. Internal helpers split: `Arc` at spawn sites, `&dyn` at pure-read sites.
- Keep `DnsResolver` generic-style and pipe `TokioDnsResolver` through `Scout` as a plain field (no `dyn`). Inject via a generic `Scout<D: DnsResolver>` type parameter.
- Skip injection. Mark `DnsResolver` as production-only and write SSRF-path tests against `TokioDnsResolver` + loopback / `example.com` fixtures.

## Decision Outcome

Chosen: object-safe `DnsResolver` trait with `Arc<dyn DnsResolver>` injection mirroring ADR-0008's `TokenSource` pattern.

The trait becomes:

```rust
pub(crate) type DnsLookupFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<IpAddr>, FetchError>> + Send + 'a>>;

pub(crate) trait DnsResolver: Send + Sync {
    fn lookup(&self, host: &str, port: u16) -> DnsLookupFuture<'_>;
}
```

`TokioDnsResolver` keeps the same async body but wraps it with `Box::pin(async move { ... })`. The test doubles `StaticDnsResolver(Vec<IpAddr>)` and `FailingDnsResolver(pub String)` move into `#[cfg(test)]` inside `fetch/ssrf.rs` and replace the ad-hoc `AllowDns` / `FailDns` types in the same module; `FailingDnsResolver::lookup` wraps its stored `String` message into `FetchError::DnsResolution` (`src/fetch/ssrf.rs:279-287`).

`Scout` gains `dns: Arc<dyn DnsResolver>`. `ScoutBuilder::for_test()` and `ScoutBuilder::from_env()` both default it to `Arc::new(TokioDnsResolver)` — production has only one implementation, so no env knob is added. `ScoutBuilder` exposes `#[cfg(test)] pub(crate) fn with_dns(self, dns: Arc<dyn DnsResolver>) -> Self`, matching the `#[cfg(test)] pub(crate)` visibility of `with_clock` / `with_rng` / `with_token_source` (`src/tools/builder.rs:130-151`, `with_dns` at `:148`). The cfg gate prevents the injection surface from leaking into the production crate API.

Internal helper signatures split by need:

| Site                    | Old signature                                              | New signature                          | Why                                           |
| ----------------------- | ---------------------------------------------------------- | -------------------------------------- | --------------------------------------------- |
| `ssrf_check`            | `resolver: &impl DnsResolver`                              | `resolver: &dyn DnsResolver`           | Pure read; one indirection acceptable         |
| `download`              | `resolver: &impl DnsResolver`                              | `resolver: &dyn DnsResolver`           | Pure read                                     |
| `check_browser_request` | `resolver: &impl ssrf::DnsResolver`                        | `resolver: &dyn ssrf::DnsResolver`     | Pure read inside spawn body                   |
| `fetch_page`            | `resolver: &impl DnsResolver`                              | `resolver: Arc<dyn DnsResolver>`       | Hands ownership to `fetch_with_cdp` via clone |
| `fetch_with_cdp`        | `resolver: impl ssrf::DnsResolver` (owned, `Clone` cloned) | `resolver: Arc<dyn ssrf::DnsResolver>` | Cloned across `tokio::spawn` move boundary    |
| `cdp_navigate`          | `resolver: impl ssrf::DnsResolver`                         | `resolver: Arc<dyn ssrf::DnsResolver>` | Owned by spawned intercept task               |
| `engine::research`      | `resolver: impl DnsResolver`                               | `resolver: Arc<dyn DnsResolver>`       | Forwards into `fetch_page`                    |
| `engine::fetch_one`     | `resolver: &impl DnsResolver`                              | `resolver: &dyn DnsResolver`           | Pure read in pre-fetch SSRF check             |

`Scout::fetch` and `Scout::research` pass `self.dns.clone()` (cheap `Arc::clone`) instead of `&TokioDnsResolver`.

注: `engine::fetch_one` は後の refactor で独立関数として消滅し、その pre-fetch SSRF read は `fetch_page` にインライン化された。`&dyn DnsResolver` を渡す挙動は保持されているため、上表の行は当時の signature 変更記録として残す。

### Consequences

- Good, because the four trait seams (`Clock` / `Rng` / `TokenSource` / `DnsResolver`) are now visually uniform: same `Arc<dyn Trait>` field, same `ScoutBuilder::with_*` setter, same `Pin<Box<dyn Future + Send + '_>>` future alias convention.
- Good, because resolution-strategy scripting becomes a one-liner in tests: `ScoutBuilder::for_test().with_dns(Arc::new(StaticDnsResolver(vec![ip]))).build()` plugs the double into every fetch path without touching `tools.rs` internals.
- Good, because `fetch_with_cdp` stops depending on the `Clone` bound — `Arc::clone` is the universal shared-ownership primitive across the rest of the codebase (ADR-0008 plumbing).
- Bad, because `dyn` dispatch adds one indirection per `DnsResolver::lookup` call. DNS lookups are I/O-dominant (network round-trip in production, in-memory `Vec` lookup in tests), so the cost is negligible.
- Bad, because the internal-helper signature split (`Arc` vs `&dyn`) is a judgment call that new contributors may not recognize. The rule is documented above and in the trait module's rustdoc.
- Bad, because `Pin<Box<dyn Future + Send + '_>>` adds visual noise at every `DnsResolver::lookup` implementation. The `DnsLookupFuture<'a>` alias mitigates but does not erase the cost. Same trade-off ADR-0008 accepted for `TokenSource`.

### Confirmation

- `cargo test --offline` passes including the new injection e2e tests `T-DNS001` (private-IP block) and `T-DNS002` (resolver failure propagation).
- `cargo test --offline --features js-rendering` also passes, exercising the `Arc<dyn DnsResolver>` flow through `fetch_with_cdp` / `cdp_navigate` / `check_browser_request`. The CDP path is gated behind the feature, so default-features builds do not surface its compile errors — verifying both is mandatory.
- `cargo clippy --offline --all-targets` and `cargo clippy --offline --all-targets --features js-rendering` are clean (no `-D clippy::absolute-paths` violations from the new test paths).
- `ScoutBuilder::for_test().with_dns(Arc::new(StaticDnsResolver(vec!["10.0.0.1".parse().unwrap()]))).build().fetch(FetchParams { url: Some("https://example.com".into()), .. }).await` returns a `ScoutError` mapped from `FetchError::InternalHost`. The injected resolver's `Ok(vec![10.0.0.1])` short-circuits inside `ssrf_check` before any HTTP connect, so the assertion holds independent of `fetch_timeout`.
- `ugrep -rn 'Clone::clone\(resolver\)' src/` returns 0 hits.
- ADR-0008 _Deferred concerns_ now links to ADR-0009.

## Pros and Cons of the Options

### Object-safe `DnsResolver` + `Arc<dyn DnsResolver>` (chosen)

`Scout` and internal helpers share one shape; tests inject through the same path that production uses.

- Good, because seam uniformity with `Clock` / `Rng` / `TokenSource` lowers the cost of adding a fifth trait field later.
- Good, because `Arc::ptr_eq` seam assertions become trivially writable (precedent: ADR-0008 `T-SB001`–`T-SB003`).
- Bad, because the `Pin<Box<dyn Future>>` shape will look outdated once `async fn in dyn Trait` ships in stable Rust. Same reassessment trigger as ADR-0008.

### Generic `Scout<D: DnsResolver>`

Type parameter propagates through `Scout`'s methods; production is `Scout<TokioDnsResolver>`, tests pick a different type.

- Good, because dispatch is fully static (zero-cost monomorphization).
- Bad, because every caller of `Scout` (`lib::run`, integration tests, future subcommand modules) would have to either name `Scout<TokioDnsResolver>` or be generic itself. ADR-0008 rejected this path for the same reason and the rationale has not changed.
- Bad, because mixing one generic parameter (`D`) with three `Arc<dyn Trait>` fields would make `Scout` look schizophrenic.

### Skip injection

Leave `DnsResolver` production-only; test SSRF paths against `TokioDnsResolver` and live hosts / loopback fixtures.

- Good, because the diff is zero.
- Bad, because resolution strategies like NXDOMAIN, partial-failure, or mixed public/private results cannot be scripted. Coverage of `ssrf_check`'s "any private IP in the address set" branch stays accidental.
- Bad, because the deferred concern in ADR-0008 stays unresolved — the four-trait pattern remains a three-and-a-half-trait pattern.

## More Information

### Implementation order

1. Rewrite `DnsResolver` to be object-safe; convert `TokioDnsResolver` and test doubles. Existing `ssrf::tests` and `dns_tests` modules compile against the new shape.
2. Update internal helper signatures in `src/fetch.rs` and `src/search/engine.rs` per the table above. `fetch_with_cdp` switches from `Clone::clone(resolver)` to `Arc::clone(&resolver)`.
3. Add `dns: Arc<dyn DnsResolver>` field to `Scout`; thread `Arc::new(TokioDnsResolver)` through `ScoutBuilder::{from_env, for_test, build_default_clients}`.
4. Add `#[cfg(test)] fn with_dns(self, dns: Arc<dyn DnsResolver>) -> Self` to `ScoutBuilder` (`src/tools/builder.rs:148`).
5. Replace the two `&TokioDnsResolver` literals in `tools.rs::Scout::fetch` and `Scout::research` with `self.dns.clone()`.
6. Add `StaticDnsResolver(Vec<IpAddr>)` and `FailingDnsResolver(pub String)` under `#[cfg(test)]` in `fetch/ssrf.rs` (`src/fetch/ssrf.rs:279-287`), where `FailingDnsResolver::lookup` wraps the stored `String` into `FetchError::DnsResolution`; remove the now-redundant `AllowDns` / `FailDns` ad-hoc types and re-point existing T-FS004..T-FS007 tests.
7. Add `T-DNS001` end-to-end test: `with_dns` + `Scout::fetch` → assert `FetchError::InternalHost` when the injected resolver returns a private IP.
8. Update ADR-0008 _Deferred concerns_ to link to ADR-0009.

### Reassessment Triggers

- `async fn in dyn Trait` becomes idiomatic in stable Rust without the `Send` auto-bound dance. At that point, simplify `DnsResolver::lookup` from `DnsLookupFuture<'_>` to a native `async fn` (same trigger ADR-0008 records for `TokenSource`).
- A second SSRF strategy is added (e.g., per-request resolver override for proxy-aware fetches). Re-evaluate whether the single-`Arc<dyn DnsResolver>` slot is still enough or whether resolution should become per-request configuration.
- `Scout` accumulates a fifth `Arc<dyn Trait>` field beyond `clock` / `rng` / `token_source` / `dns`. Reassess whether default-derived `ScoutBuilder` pays for itself (same trigger as ADR-0008).

## Addendum (2026-08-17): `with_dns` は connect 時の resolver には届かない

Consequences の「plugs the double into every fetch path without touching `tools.rs` internals」は、本 ADR を書いた 2026-05-19 時点では正しかった。ADR-0012 (2026-05-30) が connect 時の IP 検証を足し、注入できない 2 つ目の解決点ができた。

`build_default_clients` (`src/tools/builder.rs`) が `EgressMode::Direct` の `fetch_http` に `Arc::new(SsrfResolver::new(TokioDnsResolver))` を直書きで渡す。この関数は `from_env` と `for_test` の両方から、`with_dns` が適用される前に呼ばれる。`Scout.dns` は `fetch_page` の事前 SSRF check にしか届かず、reqwest の `ClientBuilder::dns_resolver` には届かない。`StaticDnsResolver` や `FailingDnsResolver` を注入しても、connect 時は常に実際の `TokioDnsResolver` を引く。

ADR-0012 の `[T-F072]` はこの分離を前提にしていて、connect 時 resolver を差し替えるテストは `with_dns` ではなく `ScoutBuilder::with_fetch_http` の `.resolve()` 経由という別の入口を使う。

つまり seam は 2 つある。事前 check 側は `with_dns`、connect 時側は `with_fetch_http`。この 2 つを 1 つにまとめるかどうかは未決で、まとめる場合は `build_default_clients` の呼び出し順を変えて `with_dns` の値を待つ形にする必要がある。
