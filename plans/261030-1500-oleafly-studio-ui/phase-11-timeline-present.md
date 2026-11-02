---
phase: 11
title: "Timeline and Present"
status: pending
priority: P1
effort: "1.5-2.5d"
dependencies: [10]
---

# Phase 11: Timeline and Present

## Goal and owned paths

Finish video-specific visual details while leaving transform, timing and presentation semantics intact. M5: whole Studio visually covered. Root: `C:\Users\Administrator\fframes-desktop\`.

- Modify presentation in `desktop\crates\studio-ui\src\timeline\{view,ruler,segment,minimap,playhead,waveform}.rs`, `shell\{present,player,transport}.rs`; no engine timing/Deck changes. Named painter constants remain allowed as current style guard documents.
- Tests: `timeline\view\tests.rs` and existing inline painter/geometry tests; gallery root `desktop\app\examples\ui_gallery.rs`, `ui_gallery\capture.rs` only for a real missing fixture. Agent/props/presets files are read-only now.
- Read-only `timeline\{model,edit,stops}.rs`, `shell\{timeline_edits,keymap}.rs`, engine transform/timing/slides Deck behavior. No change to clip, waveform, stop or preview data.

## Just-in-time source scout

Read `TimelineView` render/painter state, theme/track-color access, `TimelineControls` and shell transport responsive calculations; marker hit areas and stop/default styling. Trace PresentState, Fullscreen asynchronous want/observe, Present focus context/keymap, frame letterbox, strip/pointer timers and deferred held-build installation. Check window/texture cache ownership to avoid expensive per-frame recreation.

## Tasks and steps

1. [ ] Add/extend regression fixtures and geometry tests before paint changes: empty/loaded/read-only timeline, selected/muted/disabled clip, explicit vs default stop, playhead/scope/trim/reorder/zoom states, present held/playing/end/update-ready and fullscreen restore. Keep hit-testing assertions tied to actual layout, not copied coordinates.
2. [ ] Align ruler text/track labels/minimap separators/selection/focus to theme/type/metrics, not local palette. Preserve Studio timeline lane colors, red playhead and task scope meaning; don't copy achromatic document UI onto meaningful media colors. Ensure numeric/time labels use mono/tabular style and readable contrast in both themes.
3. [ ] Tune geometry only for visible styling defects; preserve transform and hit regions, trim/reorder gestures, stop markers/default hollow-dashed distinctions, waveform and thumbnail caches. No new timeline header; zoom/Fit/Split/cycle remain in player transport, undo/redo in shell header.
4. [ ] Present stage uses semantic black present_bg/letterbox and restrained existing slide-progress strip. Keep F5/Present from here, Present context arrow/Space/Page/Home/End/numberEnter/click/Escape semantics, two-second strip and pointer behavior. Existing fullscreen state restores asynchronously and Escape focuses preview; visual changes must not switch context to Player.
5. [ ] Check reduced motion on any newly styled indication; do not freeze video content or essential playback state just because chrome motion is reduced. Retain new-build hold while presenting and safe install after exit; don't alter source fences, audio epochs or timeline histories.
6. [ ] Run T/L/G; C `slides`, `shell`, `props` in both themes. Inspect high-DPI marker edges, overlapping clips, long labels, scroll clipping and minimum height. Because there is no existing standalone `timeline`/`present` gallery Page, use live native captures rather than cite nonexistent `--page timeline` or `--page present`.
7. [ ] N installed-SDK disposable video/deck: play/pause/seek/scrub/mute, zoom/Fit, trim/reorder/split then undo/redo, add/drag/Delete stop (K and double-click), edit notes then Present. Exercise all presentation keys, pending build while full screen, exit/restore/focus and audio sync. Record before/after screenshots plus short playback evidence for no stalls/dropped frames.
8. [ ] Compare playback/resize/frame-time counters with matched phase01 workload; no new thumbnail texture rebuild on every playhead tick or whole shell repaint. Record M5 and remaining cross-platform caveats before final integration.

## Acceptance

- [ ] Timeline/Present look consistent but retain media/status color meanings, legible code/time labels and crisp markers at all target DPIs.
- [ ] Native edits, undo history, keyboard contexts, two-second cues, asynchronous fullscreen restoration and deferred preview install preserved.
- [ ] Video/audio playback and caches show no uninvestigated performance regression; gallery claims limited to actual supported pages.
- [ ] M5 complete: no unaccounted Studio window/pane/overlay remains in inventory.

Rollback only own painter/style hunks; never alter timing/source documents or engine state to compensate for a visual regression. Dependency10; estimate1.5-2.5d including real fullscreen/audio verification.
