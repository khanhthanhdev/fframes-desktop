---
title: "Desktop Phase Zero: managed setup and feasibility"
description: "Prove that a minimal GPUI Studio can set up prerequisites, generate and build an fframes project, render through an isolated worker, run one real ACP edit, and resolve an explicit source anchor."
status: pending
priority: P1
effort: "8-12 engineer days"
branch: main
tags: [feature, desktop, gpui, build-tooling, experimental]
blockedBy: []
blocks: []
created: 2026-10-01
---

# Desktop Phase Zero: managed setup and feasibility

## Outcome

Deliver a runnable, minimally packaged GPUI application that owns setup and can take a clean account from prerequisite diagnosis through SDK readiness, generated CPU project compilation, and a displayed real fframes frame. Record measured evidence for all five M0 gates: managed compilation, GPUI input/image presentation, crash-isolated worker seeks, one real authenticated ACP edit, and revision-validated explicit source selection.

The user confirmed all five M0 checks, automatic installation of app-owned dependencies plus guided/user-reviewed OS prerequisite installation, and Linux x64 as the first GUI target. Windows x64 and macOS arm64 still receive native build/setup qualification; blocked evidence keeps that target's gate unmet.

## Scope boundary

- Included: a separate pinned desktop workspace; minimal GPUI window/text input/image; setup doctor and guided prerequisite flow; checksum-pinned app SDK artifacts; generated CPU project outside the core workspace; minimal worker/protocol/bootstrap crates; one current ACP adapter selected by real authenticated evidence; explicit annotated source anchor; clean-account and offline-second-build evidence on all three targets.
- Deferred to M1: the full workspace shell, durable project/session state, recovery model, complete provider UI, and general project import.
- Deferred to M7: production signing/notarization, updater machinery, final installers, release channels, and full uninstall/retention policy. M0 still produces a minimal launchable app artifact and SDK bundle per target so clean-user tests are real.

## Phases

| # | Phase | Depends on | Effort | Status |
|---|---|---|---|---|
| 1 | [Bootstrap the native desktop spike](./phase-01-start.md) | — | 2-3d | Pending |
| 2 | [Build managed SDK and project setup](./phase-02-managed-sdk-and-project-setup.md) | Phase 1 | 3-4d | Pending |
| 3 | [Qualify feasibility boundaries and platforms](./phase-03-feasibility-and-platform-qualification.md) | Phases 1-2 | 3-5d | Pending |

Effort is provisional and assumes access to native hosts and an authenticated provider; failed SDK/platform experiments require re-estimation before changing scope.

## Dependency and data flow

`compatibility manifest → doctor → reviewed OS prerequisite action → staged SDK install → generated project → Cargo build → versioned worker → bounded RGBA frame → GPUI image`. In parallel after the project builds: `authenticated ACP adapter → draft edit → authoritative prompt completion → rebuild`, and `displayed frame metadata → explicit ID → revision/hash/span validation → Rust implementation`.

Phase 1 fixes the workspace, GPUI, and protocol base. Phase 2 cannot begin integration until the Linux GPUI spike opens and accepts text. Phase 3 cannot claim a platform until its native SDK/package evidence exists and the Phase 2 setup path completes without repository or home Cargo cache access.

## Plan-wide acceptance

- [ ] All five M0 experiments meet their explicit pass gates; unavailable provider credentials are recorded as `NOT RUN`, leaving the ACP gate unmet.
- [ ] Windows x64, Linux x64, and macOS arm64 each have a native clean-account result, an offline second template compile, measured cold/warm timings and SDK size, and an explicit pass/block decision.
- [ ] The app never mutates global `PATH` or a global rustup installation; every child receives an app-local environment.
- [ ] The decision record pins GPUI, Rust/fframes/native SDK artifacts, worker schema, transport, first qualified agent, and initially supported target from measured evidence.
- [ ] Failure leaves the prior SDK/app artifact usable, no partial project promoted, no orphaned worker/adapter, and logs sufficient to reproduce the failing gate.

## Sources and review

The five M0 gates and measurements are defined in [implementation-plan.md](../../docs/desktop/implementation-plan.md#L32). Process and workspace boundaries come from [architecture.md](../../docs/desktop/architecture.md#L43), setup constraints from [architecture.md](../../docs/desktop/architecture.md#L355), and the current Windows native linkage contract from [README.md](../../README.md#L260) and [main.yml](../../.github/workflows/main.yml#L318). Technical research: [GPUI](../reports/researcher-261001-1707-gpui-phase-zero.md) and [managed SDK/FFmpeg](../reports/researcher-261001-1707-sdk-ffmpeg-phase-zero.md). Implementation starts only after the [plan review](../reports/plan-review-261001-1707-desktop-phase-zero.md) has no blocking finding.

## External execution gates

- A real provider account must be available for ACP qualification; otherwise that row is `NOT RUN` and M0 remains incomplete.
- Native clean-account runners/VMs and the exact host SDK/tool versions must be available before Windows/macOS/Linux support can be claimed.

<!-- slug: desktop-phase-zero-setup -->
