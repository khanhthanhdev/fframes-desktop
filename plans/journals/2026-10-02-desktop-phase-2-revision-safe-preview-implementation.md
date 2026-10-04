---
title: Desktop Phase 2 revision-safe preview implementation
date: 2026-10-02
summary: Local revision-safe preview verified; timeline and live audio remain open.
---

# Desktop Phase 2 revision-safe preview implementation

Implemented M2 Phase 2 revision-safe native CPU preview coordination and its missing worker prerequisites. Build/checkpoint/install identities stay distinct. Failed builds, invalid live media and cancellation preserve the immutable displayed worker. Tagged bounded transport rejects stale/malformed responses before payload allocation; disk-backed PCM is prepared, not played.

Verification: 113 desktop tests; 17 root protocol/runtime tests; six packaging tests; four explicitly executed real compiler/worker regressions against a fresh SDK; format/Clippy checks. Native Linux X11 software-rendered build, step, failed compile, prior-revision seek, shorter rebuild/clamp, cancel, focused Home/End and close were captured and inspected.

Native inspection caught read/access watcher hints blocking the install fence; a failing regression was added and passed after filtering access events. Close removed every leased build tree and preview process.

Delivery is local and uncommitted. Phase 3 timeline controls and Phase 4 live audio/scheduling, sustained resource qualification and other native platforms remain open. Evidence is recorded in the phase file and excluded .amp/in/artifacts. Original planning edits were preserved.

Review follow-up, 2026-10-03: fixed missing installation-frame inspection and candidate re-priming blocking last-good seeks. The real SDK regression failed on each defect before its fix and now passes, including blocked-candidate latest seeks, cancellation and cleanup. All 115 default desktop tests, workspace Clippy, formatting and diff checks pass; no new native UI/platform qualification or delivery action occurred.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
