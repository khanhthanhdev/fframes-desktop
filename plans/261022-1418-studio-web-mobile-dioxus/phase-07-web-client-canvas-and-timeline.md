---
phase: 7
title: "Web canvas editing and timeline"
status: pending
priority: P1
effort: "3w"
dependencies: [4, 5, 6]
---

# Phase 7: Web canvas editing and timeline

## Goal

Direct manipulation in the browser: select, move, scale and rotate annotated objects, edit their
properties in an inspector, edit text in place, bake into source; and the timeline with split, trim,
move, range selection, waveform and minimap.

## Context

- Proposal section 5 (canvas) and the timing behaviour in `docs/desktop/README.md` ("Editing").
- `studio-canvas` (phase 2), edits session and protocol (phase 5), drag-mode frames (phase 4).
- `desktop/crates/studio-ui/src/canvas_view.rs`, `shell/player.rs`, `timeline/*` as behaviour references.

## Requirements

- Selection is computed in the client from the frame's metadata with no round trip.
- During a drag the outline follows the pointer every animation frame; server frames of the real result
  follow at up to 30 per second; release writes once and records one undo step.
- Handles: eight scale handles, one rotate handle, move by dragging inside the box; Shift keeps
  proportions or snaps rotation to 15°, Alt scales from the centre; arrow keys nudge by 1 px, Shift+arrow
  by 10 px.
- Snapping to canvas centre and edges and to other objects' bounds, with guides; hold Ctrl to disable.
- Ctrl+Z follows the focused surface: stage and inspector undo canvas edits, the timeline undoes timing
  edits, elsewhere it undoes whichever changed last.
- Inspector: editable override values (move X/Y, scale X/Y, rotation, opacity, fill, stroke), the
  resulting bounds read-only, scope switch (whole clip or keyframe
  at playhead), repeat switch (this one or all copies), Reset, Bake into source, and the object's style
  tokens with a link to the Style page.
- Keyframes show as diamonds on the selected object's scene clip in the timeline; they can be moved and
  deleted.
- Text: double-click opens an input over the text; Enter commits to source, Escape cancels.
- Unannotated frames show "This frame has no editable objects" with a link to the docs.
- Edits are refused with the engine's reason while an agent task runs.
- Timeline matches the desktop app: ruler, scene clips, audio rows, mix waveform, minimap, split, trim,
  move, range select, zoom, undo and redo; waveform and clips are drawn on canvas.

## Architecture

- `studio-web/src/canvas/`: `stage.rs` (frame canvas plus an overlay canvas for outlines, handles and
  guides), `gestures.rs` (pointer state machine using `studio-canvas::Gizmo`), `inspector.rs`,
  `text_edit.rs`, `keyframes.rs`.
- `studio-web/src/timeline/`: `view.rs`, `clips.rs`, `waveform.rs` (peaks from the server drawn as a
  path), `minimap.rs`, `edit.rs` (gesture to `Timeline` commands).
- Pan and zoom of the stage as on desktop: Ctrl-wheel zoom, middle-drag pan, **0** fits.

## Related code files

- Create: `desktop/clients/studio-web/src/canvas/*.rs`, `desktop/clients/studio-web/src/timeline/*.rs`
- Modify: `desktop/clients/studio-web/src/player/*` (host the stage), `src/shell/*` (inspector placement)

## Implementation steps

1. Overlay canvas with selection outline, cycling (**C**), rectangle selection, Escape.
2. Gizmo handles and the gesture state machine sending `Edits` begin/update/end commands.
3. Snapping and guides.
4. Inspector with number fields, colour picker, scope and repeat switches, Reset.
5. Keyframes in the inspector and on the timeline clip.
6. In-place text editing to source; Bake into source.
7. Timeline with canvas waveform, clips, minimap and all timing gestures.
8. Browser tests: drag writes one `edits.json` change, undo restores, `render` draws the same frame.

## Todo

- [ ] Selection overlay
- [ ] Gizmo gestures
- [ ] Snapping
- [ ] Inspector
- [ ] Keyframes
- [ ] Text editing and bake
- [ ] Timeline
- [ ] Browser tests

## Success criteria

- The canvas acceptance criteria in `plan.md` pass in Chrome on this VM.
- A drag keeps the outline at the display's refresh rate with 200 annotated objects in the frame.

## Risks

Low frame rate from the CPU renderer during drags. The outline stays responsive; the rendered result
catches up. The spike and phase 4 measurements set expectations.
