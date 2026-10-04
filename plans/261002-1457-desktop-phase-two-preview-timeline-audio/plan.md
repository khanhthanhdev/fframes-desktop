---
title: "Desktop M2: Revision-safe preview, timeline and audio"
description: "Deliver persistent native playback, compiled timeline controls and matching audio in four sequential M2 stages."
status: in-progress
priority: P2
effort: "18-26 engineer-days (tentative)"
branch: main
tags: [feature, desktop, preview, timeline, audio]
blockedBy: []
blocks: []
created: 2026-10-02
---

# Desktop M2 preview, timeline and audio

## Outcome and constraints

A real multi-scene project plays in the native shell with matching audio, paused/playing seeks, frame step, timeline zoom, scene/range/overlap selection, thumbnails and time display. Rebuilds preserve/clamp playhead; failed builds preserve the playable last successful revision. These four execution stages subdivide **M2**, not milestones M1–M4. Scope remains HOLD. All four stages are implemented locally, including CPAL audio and predicted-output-clock scheduling. Physical audio/presentation and other native-platform qualification remain open. No release is claimed.

Authority: [M2 roadmap](../../docs/desktop/implementation-plan.md), [preview architecture](../../docs/desktop/architecture.md#8-preview-timeline-and-audio), [M0 feasibility](../../docs/desktop/phase-zero-feasibility.md), and [completed M1](../261002-0434-desktop-phase-one-foundation/plan.md). Start execution with current M1 regression tests; historical resolved findings are not new blockers.

Keep both Cargo workspaces, upstream GPUI pin, GPUI-free runtime and existing bounded JSON-control/loopback-binary transport. Preserve M0 v1 entry and regression routes; negotiate additive M2 capabilities through an explicit preview entry. CPU preview is the initial selected backend; display an explicit shader unsupported notice. Compare representative pixels with the existing CLI using the same CPU backend/fonts/media. No new toolkit, shared memory, Skia dependency, M3 agent transaction, M4 editing, M5 object selection or M7 export UI is needed.

## Dependencies and execution ownership

M0/M1 implementation is available. Continue development using the accepted Linux X11 software-rendering evidence; physical GPU/display/IME, Windows/macOS interactive and authenticated ACP qualification remain open. M2 success does not qualify a consumer release.

One executor owns each sequential stage. Shared-file ownership transfers only after the previous gate; no stages edit concurrently. Stage 2 may stage prepared audio without a live output device; Stage 4 supplies the output service and final atomic playback handoff.

## Phases

The table reflects local execution; platform qualification is tracked separately.

| # | Phase | Status | Depends on | Tentative effort |
|---|---|---|---|---|
| 1 | [Worker preview contract](phase-01-start.md) | Completed locally | M0/M1 baseline | 5–7 days |
| 2 | [Revision-safe coordination](phase-02-revision-safe-preview-coordination.md) | Completed locally | 1 | 5–7 days |
| 3 | [Timeline and native controls](phase-03-timeline-model-and-native-controls.md) | Completed locally | 1, 2 | 3–5 days |
| 4 | [Audio clock and M2 qualification](phase-04-audio-clock-and-milestone-qualification.md) | Implemented; qualification in progress | 1, 2, 3 | 5–7 days |

Execution evidence lives in the phase checklists, dated evidence sections and [M2 qualification ledger](../../desktop/qualification/m2-results.json). Real SDK coordination, timeline, CLI pixels, sequential PCM and virtual CPAL regressions pass. Historical Stage 3 silent playback is superseded by Stage 4 audio. All implementation remains local, uncommitted and unpushed; software-rendered/virtual-output results do not qualify physical output timing.

## Acceptance criteria

- [x] Persistent concrete Video/media/cache worker negotiates hello/version, timeline, scaled frame, inspect, prepared audio and shutdown; bounded bulk transport is measured before any replacement.
- [x] Compiled scene/audio tracks support play/pause, frame step, paused/playing scrub, zoom, range selection, overlapping-scene selection, bounded thumbnails and time display; arithmetic lives outside GPUI.
- [x] Every displayed frame/timeline/audio belongs to one immutable project/session/source/generation identity; seek serial rejects stale results within the same worker.
- [x] Real matching audio drives video scheduling; late video drops without clock drift; silent/muted/no-device modes and end/seek/device changes behave explicitly. Predicted timestamps, synthetic clocks and virtual CPAL verify the implementation; physical timing remains a separate gate.
- [x] Rebuilds switch only when matching timeline, inspection, first frame and mix are ready; preserve/clamp playhead/selection; failure leaves prior revision playable.
- [x] Color/alpha conversion and representative CLI parity pass; shader fallback is visibly unsupported.
- [x] Repeated scrub/play/rebuild stays within explicit queue/cache limits and releases images/processes/artifacts on close.
- [x] Focused/root/desktop integration checks and native evidence are recorded with platform limits; documentation handoff describes actual implemented behavior.

## Validation and handoff

Implementation is complete locally; this plan remains in progress until its remaining qualification gates have evidence. [Planning validation](reports/planning-validation.md) records planning checks only; executed implementation/native checks are recorded separately in the phase evidence sections and qualification ledger. Each phase lists absolute file ownership, failure mitigation and rollback. Engine state is per open session; audio/device/image lifetimes are app-owned. Accepted checkpoint bytes and completed BuildJob remain distinct from installed preview readiness. Preserve unchecked physical/platform gates until their own evidence exists.

The 2026-10-04 full Linux native/virtual-output run passes 600.276649 seconds, 2,001 seeks and 50 rebuilds (five deliberate failures), with seek-to-image-submission p95 96.232490 ms and RSS slope 1.228500 MiB/min. All sampled bounds pass; close leaves no owned/orphaned process or PCM/build lease. Paused, resized and failed-build scrolled states were rendered and inspected. Physical A/V timing, unplug/drain latency and Windows/macOS remain unqualified. CI wiring is local and was not remotely triggered.

## Unresolved questions

No blocking product question. Bounds, performance thresholds and artifact format below are proposed engineering defaults to qualify during execution. Existing physical/platform/authentication qualification gates remain open.

<!-- slug: desktop-phase-two-preview-timeline-audio -->
