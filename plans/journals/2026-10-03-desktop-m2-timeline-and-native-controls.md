---
title: Desktop M2 timeline and native controls
date: 2026-10-03
summary: Stage 3 locally verified; live audio and sustained M2 qualification remain open.
---

# Desktop M2 timeline and native controls

Completed local M2 Stage 3 continuation on the existing uncommitted Stage 2 work. Compiled scene/audio timeline geometry, overlap/range selection, focus-local transport/scrubbing/zoom, and bounded revision-tagged thumbnails are wired into the native shell. Playback is explicitly silent and monotonic; no live device/output-clock implementation is claimed.

Native thumbnails exposed tiny-skia static-cache pixels being reused across scales. A regression failed before the core fix and passed after it; a freshly assembled and installed SDK verified native thumbnail/main-frame parity. Review also fixed viewport-dependent audio lane placement.

Verification: 131 desktop tests and all-targets Clippy, 61 compile-time-tree core tests and library/test Clippy, 17 root protocol/runtime tests, six packaging tests, explicitly executed fresh-SDK timeline/thumbnail and existing real failed-build coordination regressions. Linux X11 software-rendered overlap cycling, step, reverse range, zoom/pan, playback/scrub/end/replay, resize, shorter-rebuild clamping and close were captured and inspected. Close left no owned preview worker or leased build tree.

Core all-targets Clippy still reports two pre-existing warnings in the untouched yuv420_conversion benchmark. Physical GPU/display/IME, Windows/macOS, live audio/clock/device handoff, CLI/alpha/shader M2 parity and sustained stress/resource qualification remain open. Stage 3 checklists/evidence and owning docs are reconciled; the overall plan remains in progress. All code is local/uncommitted/unpushed. Artifacts are excluded under .amp/in/artifacts/; exact evidence and reproduction commands are in the Stage 3 phase file.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
