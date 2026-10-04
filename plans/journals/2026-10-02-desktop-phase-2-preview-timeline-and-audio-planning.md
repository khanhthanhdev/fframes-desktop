---
title: Desktop Phase 2 preview timeline and audio planning
date: 2026-10-02
summary: Plan M2 in four sequential stages using the existing M0 worker and M1 project foundation.
---

# Desktop Phase 2 preview timeline and audio planning

# Desktop Phase 2 planning

Created the [M2 execution plan](../261002-1457-desktop-phase-two-preview-timeline-audio/plan.md) and expanded the [desktop roadmap](../../docs/desktop/implementation-plan.md) with its execution stages and acceptance boundaries.

The plan reuses the M0 runtime/protocol/bounded transport and M1 project/revision/recovery foundation. It covers worker contracts, revision-safe preview coordination, compiled timeline controls and matching audio-clock playback. Failed builds retain the immutable playable preview; checkpoint bytes remain distinct from preview readiness. Legacy worker and SDK compatibility remain protected.

The accepted Linux development evidence boundary carries forward. Native audio timing, physical GPU/display/IME, Windows/macOS interaction and authenticated ACP qualification remain open. This task authors documentation only and does not execute implementation tests, change code or qualify a release. Planning verification is recorded in the plan's linked validation report.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
