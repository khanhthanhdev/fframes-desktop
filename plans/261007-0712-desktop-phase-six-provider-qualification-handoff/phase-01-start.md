---
title: "Phase 1: Provider profiles and evidence contracts"
status: complete
---

# Provider profiles and evidence contracts

## Overview and dependency

Priority P1. Tentative estimate 4–6 engineer-days. Build a small app-local provider registry and qualification contract for Claude, Codex, Pi and Antigravity. Depends on implemented M3–M5, not on historical M3 plan completion claims. The [M5 ledger](../../desktop/qualification/m5-results.json) is development evidence only. Read [provider research](../reports/research-261007-0712-desktop-phase-six-providers.md) before selecting distributions or commands; recheck observed versions when executing.

## Verified context

- ACP discovery resolves explicit executables through `ExecutableSearch` and `AdapterLaunch` (`desktop/crates/studio-agent-spike/src/discovery.rs:24,141,177`) and probes in scratch cwd (`:308`). Preserve its managed-path rules rather than discovering arbitrary PATH executables.
- `AdapterConfig` stores executable, arguments and authentication variable names (`desktop/crates/studio-agent-spike/src/session.rs:25`); values are read at launch. Native adapter configuration is owned by `AdapterFile` parsing/resolution and atomic saves (`desktop/app/src/conversation_panel/host.rs:79,117,285`). Existing documents remain valid.
- Negotiated `AgentCapabilityInfo` already includes load/resume flags (`desktop/crates/studio-agent-spike/src/driver/events.rs:155`). The driver consumes these before choosing new/resume/load (`desktop/crates/studio-agent-spike/src/driver/runtime.rs:1625`). Provider claims cannot override them.
- Writer qualification is launch/platform-bound and rejects user-asserted ownership (`desktop/app/src/conversation_panel/qualification.rs:148,236,258`). The current ledger route is M3 (`:40–44`); M6 evidence needs a compatible, equally strict resolver extension, never a bypass.

## Requirements and data flow

Provider profile (stable ID, label, configured executable/args/auth names, MCP policy) → existing resolution/probe → sanitized readiness/capabilities and measured launch identity → native status and M6 ledger. Persistent configuration contains no credentials and remains outside portable projects. Separate profile existence, auth readiness, protocol negotiation, writer containment, experimental status and workflow qualification. ACP v1 stdio MCP is a baseline route, not a negotiated MCP capability flag; distinguish host policy/offer from successful provider tool calls.

Use one versioned app-local registry, with four named profiles plus preserved custom configuration. Do not introduce a plugin ecosystem. Record actual ACP distribution and upstream CLI versions, OS/architecture, auth route, negotiated image/config/load/resume capabilities, host MCP policy, actual MCP calls, and CLI fallback results. Adapter installation/login remains provider-owned; do not install or sign in automatically.

Qualification records distinguish development fixtures, authentic provider runs, physical-device checks and platform results. A pass must bind SHA-256 evidence, launch identity, SDK/build identity, named scenario, measured outcomes and teardown. Blocked/not_run records include prerequisites. Evidence has no secret values, opaque session IDs, raw private paths or private prompts.

## File ownership

All paths below are under `/root/fframes-desktop/`. One stage executor owns these files; stage 3 receives shared ownership after this gate.

| Action | Exact path | Purpose |
|---|---|---|
| Modify | `desktop/app/src/conversation_panel/host.rs` | Backward-compatible registry load/save and profile resolution |
| Modify | `desktop/app/src/conversation_panel/qualification.rs` | Strict M6 launch-bound evidence lookup with legacy M3 support |
| Modify | `desktop/crates/studio-agent-spike/src/discovery.rs` | Structured probe observations only where missing |
| Modify | `desktop/scripts/validate-qualification.py` | M6 schema/evidence/ranking validation |
| Create | `desktop/app/src/conversation_panel/provider-profiles.rs` | Small registry boundary; register with an explicit Rust path attribute |
| Create | `desktop/qualification/m6-results.schema.json` | Four-provider/platform/scenario contract |
| Create | `desktop/qualification/m6-results.json` | Initially unqualified, explicit prerequisites |
| Create | `desktop/scripts/test-qualification-m6.py` | Negative evidence and recommendation tests |
| Create | `desktop/app/tests/provider_profiles.rs` | Migration, configuration and qualification integration tests |

No deletions. Do not pin guessed versions or rewrite driver transport. Preserve existing M3–M5 ledger claims.

## Implementation steps

1. Read current research and verify each available distribution through provider documentation and a scratch probe. Research candidates are Claude ACP 0.86.0, Codex ACP 2.1.1 (App Server), community svkozak Pi ACP 0.0.34, and official Google Antigravity ACP 1.3.0; these are not Studio pins or passed results. Resolve stable artifact/dependency/runtime bytes, record package/runtime version and content fingerprints, then pin the exact tested launch. Refresh qualification when any closure changes. Pi currently does not connect supplied MCP servers: visibly qualify CLI fallback. Antigravity image/tool/cancel behavior remains unproven. Record unavailable distributions/accounts honestly. Define the four-provider matrix before implementation.
2. Add registry schema versioning and migration from `agent-adapter.json`: preserve all existing fields/custom labels; save a rollback copy before writing the new file; keep the old file intact. Unknown newer formats are diagnostic-only and launch nothing. Serialize saves through the existing off-UI owner.
3. Extend readiness/status to report each independent capability and limitation. Keep runtime prerequisites separate from SDK readiness. Display only negotiated models/options, with no invented defaults.
4. Specify qualification evidence for create/edit/export, selected-context edit, completion, permission/error/quiet periods, cancel/crash, restart resume/fallback, MCP and CLI parity, image/text context, two-provider handoff and twenty-cycle cleanup. Define export as CLI backend qualification and reserve native app export/release acceptance for M7.
5. Extend the validator and writer resolver together. Require exact current launch/platform identity and proven writer cleanup; replaced binaries or argument files invalidate evidence. Fixture overrides remain test-only.
6. Add deterministic ranking: only providers with all mandatory authentic workflow/safety gates passed are eligible; compare identical scenario completion/failure/repair rates, restoration and tool/visual results, then latency/resource data on named environments. Stable ID resolves true ties. Recommend exactly two only when at least two qualify; otherwise show insufficient evidence. Others remain experimental.

## Validation and test matrix

Run from repository root. These package/test targets exist in the current Cargo manifests; newly created targets run only after they are added.

```bash
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-agent-spike --test acp_v1
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --lib conversation_panel
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test provider_profiles
python3 desktop/scripts/test-qualification-m6.py
python3 desktop/scripts/validate-qualification.py
```

| Level | Cases and pass condition |
|---|---|
| Unit | Migration round trip, duplicate IDs, unknown schema, invalid auth names, absent executable, atomic save failure; no original configuration lost |
| Integration | Missing auth vs failed protocol, capability downgrade, changed executable/arg file, legacy authentic M3 containment and new M6 containment; unsafe writer remains blocked |
| Evidence | Missing/tampered/fixture hashes, mismatched OS/launch, secret/path content, stale versions, inflated recommendation and fake export passes rejected |
| Authentic probe | One result per available provider; unready accounts produce prerequisites rather than a pass |

## Todo and success criteria

- [x] Four profiles and custom/legacy migration retain exact configuration without credentials.
- [x] Every advertised capability comes from negotiation or named measured evidence.
- [x] Negative validator tests refuse fake authentic passes and ineligible recommendations.
- [x] M6 ledger starts honest and has complete scenario/platform prerequisites.
- [x] Stage 2 can consume profiles and safety status without a second supervisor.

## Risk, security and rollback

High likelihood × high impact: distribution/auth differences are discovered by per-provider scratch probes; no speculative native adapter. Medium × high: migration/evidence bugs could authorize a writer; preserve legacy files, fail closed and test identity mismatch. Medium × medium: incomparable ranking; require common suite and preserve raw measured summaries before ranking. Authentication remains provider-managed; redact diagnostics before storage.

Rollback restores the preserved legacy adapter config, disables M6 registry/evidence consumption, and leaves project source, draft archives and prior ledgers untouched. Unsupported new registry data must not be interpreted by older builds. Stage 2 starts only after configuration migration and strict evidence tests pass.
