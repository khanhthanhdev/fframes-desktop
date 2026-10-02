---
title: "Desktop M1: Native shell, portable projects and recovery"
description: "Execute M1 in four sequential stages using the existing Phase 0 desktop foundation."
status: completed
priority: P2
effort: "12-17 engineer-days (tentative)"
branch: main
tags: [feature, desktop, project, recovery]
created: 2026-10-02
---

# Desktop M1 foundation

## Overview

Deliver create/open/import, copied assets, a portable `studio.json`, recent projects, SDK status, explicit revision/job state, crash recovery and the native shell. These four execution stages subdivide **M1**, not milestones M1–M4. Implementation and Linux development verification cover all four stages; execution evidence is recorded in the phase files. Changes are local and uncommitted; completion does not mean shipped or release-qualified.

Scope is HOLD. Reuse the accepted [M1 requirements](../../docs/desktop/implementation-plan.md), [architecture](../../docs/desktop/architecture.md) and [Phase 0 evidence](../../docs/desktop/phase-zero-feasibility.md). Reuse the existing desktop workspace, GPUI pins, managed SDK, process supervisor, root worker protocol and runtime bridge.

## Dependencies and qualification

Phase 0 implementation exists; qualification remains PENDING. User-confirmed development decision: develop M1 using recorded Linux X11 software-rendering evidence while physical GPU/display/IME, Windows/macOS interactive gates and authenticated configurable ACP adapters remain open. Passing M1 never qualifies a consumer release.

## Phases

This table preserves the planning snapshot. Execution state is owned by the linked phase checklists.

| # | Phase | Status | Depends on | Tentative effort |
|---|---|---|---|---|
| 1 | [Portable models/state](phase-01-start.md) | Pending | Existing M0 implementation | 2-3 days |
| 2 | [Portable lifecycle/SDK binding](phase-02-portable-project-lifecycle.md) | Pending | 1 | 3-4 days |
| 3 | [Durable state/recovery](phase-03-durable-state-and-recovery.md) | Pending | 1, 2 | 4-6 days |
| 4 | [Native shell/verification](phase-04-native-shell-and-foundation-verification.md) | Pending | 1, 2, 3 | 3-4 days |

One executor owns each sequential stage. Shared file edits transfer only after the previous gate passes; these stages must not run in parallel.

## Acceptance criteria

- [x] Create, close/reopen and relocate a project; copied assets and portable configuration remain intact.
- [x] Open/import preserves dirty source and Git history; invalid/newer manifests and missing files show corrective actions.
- [x] Source, accepted, candidate and job identities are explicit; external edits invalidate derived state and stale results never overwrite them.
- [x] Restart after an interrupted job exposes accepted checkpoint and retained draft without silently replacing external edits.
- [x] Project/assets/style navigation, preview/timeline regions and agent panel exist with honest M1 empty states; SDK status and recent projects persist.
- [x] Focused tests, native shell flow and Phase 0 regression commands pass with evidence limits recorded.

## Boundaries and handoff

Use one new GPUI-free `studio-project` crate for portable format/files and one `studio-engine` crate for controller/persistence. Reuse existing SDK/bootstrap/app/protocol. Agent transactions belong to M3, playback to M2, preset behavior to M4, generalized element selection to M5 and release qualification to M7. Do not invent separate UI/media/preset/agent crates for M1.

The root roadmap's ST-13 depends on full M3 transactions; M1 implements checkpoint/interruption foundations only, not task Apply/Undo or agent repair loops. This M1 execution does not include commits, publishing or release claims.

Phase checkboxes are execution authority. Structural validation, local links and the controller consistency review passed; see the [validation log](reports/planning-validation.md). Stage 1 evidence remains in [the implementation report](../reports/implementation-261002-1216-desktop-phase-one-contracts.md); stages 2–4 record lifecycle, recovery, real-worker and native-flow evidence in their linked phase files. Restore exports a separate independent copy; it never overwrites current source or Git. Physical/platform/authentication qualification remains pending.

## Unresolved questions

No blocking product question. The user selected “Plan M1 development now using the recorded Linux evidence; keep qualification gates open.” Remaining M0 platform/auth qualification stays open.

<!-- slug: desktop-phase-one-foundation -->
