# Architecture Decision Records

This directory contains important decisions about the project's architecture.

## ADR List

| Number                                                                           | Title                                                                             | Status   | Date       |
| -------------------------------------------------------------------------------- | --------------------------------------------------------------------------------- | -------- | ---------- |
| [0001](0001-ssrf-defense-architecture-and-fetchrs-module-structure.md)           | SSRF Defense Architecture and fetch.rs Module Structure                           | accepted | 2026-05-13 |
| [0002](0002-adopt-sysexitsh-exit-code-convention-for-cli.md)                     | Adopt sysexits.h Exit Code Convention for CLI                                     | accepted | 2026-05-13 |
| [0003](0003-error-classification-contract-for-sysexits-and-json-output.md)       | Error Classification Contract for sysexits and JSON Output                        | accepted | 2026-05-13 |
| [0004](0004-github-client-behavioral-limits.md)                                  | GitHub Client Behavioral Limits                                                   | accepted | 2026-05-13 |
| [0005](0005-switch-search-backend-from-gemini-grounding-to-brave-search-api.md)  | Switch Search Backend from Gemini Grounding to Brave Search API                   | accepted | 2026-05-15 |
| [0006](0006-centralize-retry-after-delay-in-retry-helper.md)                     | Centralize retry-after delay in retry helper                                      | accepted | 2026-05-16 |
| [0007](0007-unify-brave-client-init-via-factory-and-result-based-https-check.md) | Unify Brave client init via factory and Result-based https check                  | accepted | 2026-05-17 |
| [0008](0008-trait-injection-and-scoutbuilder-for-test-seam.md)                   | Test seam architecture via `Arc<dyn Trait>` fields and `ScoutBuilder`             | accepted | 2026-05-19 |
| [0009](0009-object-safe-dns-resolver-and-arc-injection.md)                       | Object-safe `DnsResolver` and `Arc<dyn DnsResolver>` injection via `ScoutBuilder` | accepted | 2026-05-19 |
| [0010](0010-scout-local-json-envelope-contract.md)                               | Scout-local JSON envelope contract                                                | accepted | 2026-05-19 |
| [0011](0011-scout-local-classification-priority-policy.md)                       | Scout-local Classification Priority Policy                                        | accepted | 2026-05-19 |
| [0012](0012-connect-time-ip-guard-for-ssrf-dns-rebinding.md)                     | Connect-time IP Guard for SSRF DNS Rebinding, with CDP Path Asymmetry             | accepted | 2026-05-30 |
| [0013](0013-charset-detection-and-decode-policy.md)                              | Charset Detection and Decode Policy                                               | accepted | 2026-06-24 |
| [0014](0014-output-injection-defense-for-agent-consumers.md)                     | Output-Injection Defense for AI-Agent Consumers                                   | accepted | 2026-06-24 |
| [0015](0015-redacted-mandatory-secret-carrier.md)                                | Redacted Mandatory Secret Carrier                                                 | accepted | 2026-06-24 |
| [0016](0016-github-formatter-output-schema.md)                                   | GitHub Formatter Output Schema and README Byte Cap                                | accepted | 2026-06-24 |
| [0017](0017-signal-exit-codes-and-graceful-drain.md)                             | Signal Exit Codes and Graceful Drain                                              | accepted | 2026-06-24 |
| [0018](0018-github-token-resolution-precedence.md)                               | GitHub Token Resolution Precedence and Leak Containment                           | accepted | 2026-06-24 |
| [0019](0019-env-var-validation-and-timeout-hierarchy.md)                         | Environment-Variable Validation and Timeout Hierarchy                             | accepted | 2026-06-24 |
| [0020](0020-search-default-output-one-url-per-line.md)                           | Search Default Output: One URL Per Line                                           | accepted | 2026-06-24 |
| [0021](0021-cdp-chromium-launch-egress-flags.md)                                 | CDP Chromium Launch Egress Flags                                                  | accepted | 2026-06-24 |
| [0022](0022-slack-user-token-prefix-enforced-at-construction.md)                 | Slack User-Token Prefix Enforced at Construction                                  | accepted | 2026-06-24 |
| [0023](0023-proxy-egress-delegation-for-fetch.md)                                | Proxy Egress Delegation for Fetch                                                 | accepted | 2026-07-21 |

## By Status

### Accepted

- **0001**: SSRF Defense Architecture and fetch.rs Module Structure
- **0002**: Adopt sysexits.h Exit Code Convention for CLI
- **0003**: Error Classification Contract for sysexits and JSON Output
- **0004**: GitHub Client Behavioral Limits
- **0005**: Switch Search Backend from Gemini Grounding to Brave Search API
- **0006**: Centralize retry-after delay in retry helper
- **0007**: Unify Brave client init via factory and Result-based https check
- **0008**: Test seam architecture via `Arc<dyn Trait>` fields and `ScoutBuilder`
- **0009**: Object-safe `DnsResolver` and `Arc<dyn DnsResolver>` injection via `ScoutBuilder`
- **0010**: Scout-local JSON envelope contract
- **0011**: Scout-local Classification Priority Policy
- **0012**: Connect-time IP Guard for SSRF DNS Rebinding, with CDP Path Asymmetry
- **0013**: Charset Detection and Decode Policy
- **0014**: Output-Injection Defense for AI-Agent Consumers
- **0015**: Redacted Mandatory Secret Carrier
- **0016**: GitHub Formatter Output Schema and README Byte Cap
- **0017**: Signal Exit Codes and Graceful Drain
- **0018**: GitHub Token Resolution Precedence and Leak Containment
- **0019**: Environment-Variable Validation and Timeout Hierarchy
- **0020**: Search Default Output: One URL Per Line
- **0021**: CDP Chromium Launch Egress Flags
- **0022**: Slack User-Token Prefix Enforced at Construction
- **0023**: Proxy Egress Delegation for Fetch

## About MADR Format

This project uses [MADR (Markdown Any Decision Records)](https://adr.github.io/madr/) format, v4.

### How to Create an ADR

```bash
/adr "Decision Title"
```

### Status Meanings

- **Proposed**: Awaiting review
- **Accepted**: Approved, implementing or completed
- **Rejected**: Considered but not adopted
- **Deprecated**: Retired without a replacement ADR
- **Superseded**: Replaced by another ADR (e.g. `superseded by ADR-NNNN`)

---

_Last updated: 2026-07-21_
_Auto-generated by: update-index.py_
