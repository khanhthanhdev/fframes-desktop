---
title: Plan desktop M7 installers and consumer release
date: 2026-10-07
summary: "Planned the five M7 stages, then began stage 1: added compatibility/signature/evidence contracts and fail-closed release guards. M7 remains in progress; consumer qualification, signing and later stages remain open."
---

# Plan desktop M7 installers and consumer release

Planned five M7 stages from source-verified M0–M6 development foundations; recorded independent platform releases, automatic idle updates and retained-data uninstall. Began stage 1 by adding app/SDK compatibility enforcement, signed-manifest verification, evidence schemas and release eligibility guards. Pinned the Windows FFmpeg CI artifact by release URL and SHA-256. SDK/app tests, Clippy, M6/M7 evidence checks, packaging tests, a full phase-zero package with SDK, and managed/portable worker integration checks passed. The exact archived FFmpeg produced an HEVC/AAC MP4 that decoded, but an HEVC-enabled renderer resolved `libx265.so.199` from the host rather than the SDK; installed-location/runtime and GPL/source-distribution checks remain blockers. The worktree remains uncommitted; stage-1 qualification items remain open, stages 2–5 are pending, and consumer release remains disabled. See the [execution report](../reports/planning-261007-2054-m7-baseline-and-validation.md#execution-update-stage-1-in-progress).

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
