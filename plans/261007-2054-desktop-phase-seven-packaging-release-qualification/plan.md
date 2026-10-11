---
title: "M7: Installers and consumer release qualification"
description: "Deliver guided setup, immutable MP4 export, signed packages and safe updates using the implemented M0–M6 foundations."
status: in-progress
priority: P1
effort: "35–54 engineer-days, plus external qualification and signing lead time"
branch: main
tags: [desktop, release, infra, qualification]
blockedBy: []
blocks: []
created: 2026-10-07
---

# M7: Installers and consumer release qualification

## Overview

Deliver the full [M7 roadmap scope](../../docs/desktop/implementation-plan.md#m7--installers-and-consumer-release): native packages and compatible SDKs, guided agent/SDK setup, accepted-revision MP4 export, verified updates with rollback, and consumer qualification. Reuse M0–M6; do not redesign the renderer, agent workflow or web editor.

## Baseline and constraints

M0–M5 supply the Linux development foundation. M6 profiles, picker, handoff and restoration are implemented in the current uncommitted workspace; its latest development report passes, but the ledger records no authentic qualified connector. Physical display/audio/IME and Windows/macOS workflow gates remain open. See [baseline and decisions](../reports/planning-261007-2054-m7-baseline-and-validation.md). Preserve and recheck this tree rather than replacing it with historical M6 plan state.

At M7 start, the release workflow packaged phase-zero ZIPs without a consumer-qualification gate. Stage 1 now leaves consumer publication disabled unless trusted keys, candidate provenance and complete qualification are configured. Local SDK staging/rollback, immutable build leases, verified release metadata/download primitives and a first native MP4 export path now exist in this uncommitted worktree. A Linux unsigned Debian candidate can now be built separately from the SDK and launches the product shell from an extracted package under a clean X11 session; apt-install/update/uninstall and full clean-machine qualification remain open. The Linux development export smoke now also verifies cancellation during rendering and preserves a destination created during the render/publication race, but does not qualify exact candidate bytes or codec redistribution. Guided online setup, paired app/SDK updates and signed consumer installers remain incomplete. CPU is the implemented preview backend; unsupported shaders must remain visible.

## Execution stages

| Stage | Detail | Depends on | Tentative effort |
|---|---|---|---|
| 1 | [Release contracts and native package baseline](phase-01-start.md) | Implemented M0–M6 | 5–8d |
| 2 | [Guided setup and verified SDK acquisition](phase-02-guided-setup-and-verified-sdk.md) | 1 | 7–10d |
| 3 | [Revision-pinned native MP4 export](phase-03-revision-pinned-native-export.md) | 1, 2 | 8–12d |
| 4 | [Signed installers and update rollback](phase-04-signed-installers-and-update-rollback.md) | 1–3 | 9–14d |
| 5 | [Clean-machine release qualification](phase-05-clean-machine-release-qualification.md) | 1–4; authentic M6 evidence | 6–10d |

Stages 1–3 are in progress; stages 4–5 remain pending. Execute sequentially; shared files transfer ownership after each gate. Native package experiments exposed the Linux development layout and remaining signing, codec-distribution and non-Linux runner blockers. Estimates are planning ranges, not delivery commitments.

## Accepted release policy

- Ship macOS Apple Silicon, Windows x64 and Linux x64 independently as each passes; preserve the full three-platform implementation scope.
- Install updates automatically when Studio is idle. Idle excludes prompts, permissions, writers, builds, playback, exports, downloads, migrations and active project mutations; recheck immediately before activation and preserve unsaved input and retained drafts.
- Uninstall retains projects, SDKs and app state by default. Explicit app-data removal deletes only inventoried app-owned paths; user source/media/export folders are always retained.
- Advertise only provider/target/backend/codec combinations with authentic and physical evidence. Preserve M6's best-two recommendation rule only when comparable evidence supports it.

## Acceptance criteria

- [ ] Native CI produces versioned app/runtime/SDK artifacts, compatibility metadata and inventory-derived notices without moving dependency URLs or checkout paths.
- [ ] Guided setup handles authentication, prerequisites, progress/resume/cancel, disk shortage, interrupted install and archive-free offline SDK reuse without a terminal for advertised defaults.
- [ ] Export freezes accepted source/assets/styles/SDK/backend, supplies qualified MP4 codec/quality choices and progress/cancel, and atomically publishes only a verified complete file.
- [ ] Packages launch the product shell, integrate Open project/associations and pass signing/notarization, update and rollback checks.
- [ ] Automatic idle updates verify signed metadata, hashes and compatibility, recover interrupted activation and retain a usable previous app/SDK pair.
- [ ] Clean-user install → connect → SDK → generate → select/prompt → preview/audio → export → reopen passes on every advertised target with actual authenticated connectors; update/uninstall retention passes too.

## Dependencies and boundaries

Reuse [architecture](../../docs/desktop/architecture.md), SDK receipts, controller/checkpoints, BuildService, ProcessTreeManager and M6 profiles/session evidence. The [M6 ledger](../../desktop/qualification/m6-results.json) remains provider authority; M7 adds installation/export/update evidence. Signing identities, native runners, physical hardware, accounts, codec distribution review and an immutable release origin are execution prerequisites, recorded as blocked/not_run when absent.

No cloud sync, remote-build service, native provider driver, arbitrary extensions, new UI framework, arbitrary codec string entry or renderer rewrite. Imported projects retain portable source and an explicit advanced compatibility path.

## Red Team Review

Three independent assumption, failure and security reviewers checked 58 source/contract claims (with overlap). Three unique findings were resolved: export uses the latest validated/published source rather than the older saved checkpoint; rollback uses the existing SQLite online backup before candidate migration; stage 3 names the actual Controller and lifecycle generator path. No unresolved findings remain. See the validation report for evidence.

## Validation and handoff

Hard planning route; three release-policy questions answered by the user. CLI format, local-link, file-ownership and whole-plan consistency checks pass. Source verification and review live in the [validation report](../reports/planning-261007-2054-m7-baseline-and-validation.md). Progress stays in checklists because no live task-management surface is available. CLI scaffolding succeeded; its global index is outside the writable workspace. The live CLI confirms phase titles/content/status are file-owned.

Historical planning handoff: `/ak:cook plans/261007-2054-desktop-phase-seven-packaging-release-qualification/plan.md`. Execution is underway; stages 1–3 remain incomplete, with installed-location/runtime behavior, durable export recovery, paired updates, signatures and codec/source-distribution qualification unresolved. No consumer release is published or claimed qualified.

## Windows development run (2026-10-10)

On 2026-10-10 a Windows Server 2022 x64 VM (virtual RDP display, no GPU) ran the Windows development checks: the full desktop workspace suite (79 test targets) passes on MSVC, the Windows SDK assembles with its offline double build, and the packaged app installs the managed SDK into a fresh home, compiles a worker, presents 1,000 frames and accepts native SendInput typing and preview selection with no leftover processes. Results and evidence are in the [M0 ledger](../../desktop/qualification/m0-results.json) under `additional_platforms`. The Windows host doctor now recognizes MSVC through the Visual Studio setup registry and libclang instead of demanding Unix tools, and its install hint no longer prints apt-get. The `x86_64-pc-windows-msvc` candidate in the [M7 ledger](../../desktop/qualification/m7-results.json) keeps every gate open: no installer or installed-location run, no signing identity, no clean-machine launch (the executables now link the C runtime statically), no clean-user guided setup, no authenticated provider and no physical devices.

<!-- slug: desktop-phase-seven-packaging-release-qualification -->
