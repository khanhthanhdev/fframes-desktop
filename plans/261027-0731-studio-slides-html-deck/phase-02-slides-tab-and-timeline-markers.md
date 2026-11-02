---
phase: 2
title: "Slides tab and timeline markers"
status: pending
priority: P1
effort: 3d
dependencies: [1]
---

# Phase 2: Slides tab and timeline markers

## Goal

Let a person see and edit slides in Studio: a Slides sidebar tab with the list of slides,
titles and speaker notes, and stop markers on the timeline that can be added, dragged and
removed.

## Context

- Sidebar tabs: `SidebarTab` and `ALL` in `desktop/crates/studio-ui/src/shell/left_column.rs:602-670`,
  drawer routing at `:78-97`, id test `sidebar_tabs_keep_their_element_ids` at `:689-697`.
  Icons: `desktop/crates/studio-ui/src/kit/icon.rs:7, 116+`, SVGs in `desktop/app/assets/icons/`.
- A tab that is its own entity: `PropsPanel` (`desktop/crates/studio-ui/src/props_panel.rs:34, 54, 382`),
  which emits events the shell turns into engine commands. The drawer is a cached section
  (`shell/sections.rs:1-5`), so the tab must be an entity or the shell must notify it.
- Timeline: `TimelineView` (`desktop/crates/studio-ui/src/timeline/view.rs:64-97`), overlay
  pattern for markers at `:1164-1224` (absolute children of `tracks`, x from
  `viewport.x_at_frame`), drag pattern `EditDrag` / `edit_drag_move` at `:512-734`, handle
  pattern `trim_handle` at `:787-818`, snapping in `timeline/edit.rs:23-104`.
- Events: `TimelineEvent` (`view.rs:36-48`) handled in `shell/mod.rs:294-321`.
- Design system: `docs/desktop/design-system.md`, components shown in
  `desktop/app/examples/ui_gallery.rs`.

## Requirements

- **Slides tab** (sixth tab, icon "slides"): one row per resolved stop with number, title, scene
  and time; default stops are shown greyed with "Default (clip end)". Selecting a row seeks the
  preview to that stop. Inline title field and multi-line notes field; edits commit as one
  undoable gesture when the field loses focus. Buttons: "Add stop at playhead", "Remove",
  "Use clip ends" (materializes default stops into the document so they can get notes).
  Problems (unresolved, duplicate) show as a warning list with the reason.
- **Timeline markers:** a thin flag on the ruler and a line through the lanes at each resolved
  stop; default stops dashed. Dragging an explicit marker moves the stop (snaps to clip ends and
  the playhead like trims; hold the snap-disable key as for trims). A marker cannot be dragged
  across a clip boundary into another scene: it clamps to its clip. Double-click on the ruler
  adds a stop. Delete removes a selected marker.
- Markers re-render when stops change: the shell notifies `TimelineView` on slides events.
<!-- Updated: Red Team 2026-10-27 - undo routing (RT8), "end" stops and unresolved stops (RT14) -->
- Dragging an `"end"` stop turns it into a numeric stop; snapping back onto the clip end turns
  it into `"end"` again. Unresolved stops (frame trimmed away) show in the tab with "Move to
  clip end" and "Remove" actions; they have no timeline marker.
- Undo and redo use the shared run-time document history from phase 1: Ctrl+Z in the Timeline
  context and the header buttons undo the latest timing or slides gesture.

## Files

- Create: `desktop/crates/studio-ui/src/slides_panel.rs` (entity, `SlidesEvent`).
- Create: `desktop/crates/studio-ui/src/timeline/stops.rs` (marker geometry and hit testing as
  pure functions, rendering helpers).
- Modify: `desktop/crates/studio-ui/src/shell/left_column.rs`, `kit/icon.rs`,
  `desktop/app/assets/icons/slides.svg` (create), `timeline/view.rs`, `timeline/mod.rs`,
  `timeline/edit.rs` (stop snapping), `shell/mod.rs` (route `SlidesEvent` and new
  `TimelineEvent::Stop(..)` to engine commands from phase 1), `actions.rs` and
  `shell/keymap.rs` (`AddStopAtPlayhead`, `RemoveStop` in the Timeline context).
- Modify: `desktop/app/examples/ui_gallery.rs` (Slides tab and markers in their states).

## Tasks & Steps

1. Marker geometry in `timeline/stops.rs`: frame to x, hit test with a 6 px tolerance, clamp to
   the stop's clip. Unit-test the clamp and hit tests.
2. Draw markers in `TimelineView::render` using the overlay pattern; add drag state next to
   `EditDrag`.
3. `TimelineEvent::Stop(StopEdit::{Add(frame), Move(id, frame), Remove(id)})` and shell
   routing.
4. `SlidesPanel` entity with list, fields and buttons; hook it into `SidebarTab::Slides`.
5. Gallery entries for: no stops (defaults), explicit stops with notes, a problem list, a
   marker being dragged.

## Verification

- `#[gpui::test]` in `timeline/view/tests.rs` following
  `split_cuts_the_clip_under_the_playhead` (`:357-370`): dragging a marker emits `Stop(Move)`
  with the snapped frame; dragging past the clip end clamps; double-click on the ruler emits
  `Stop(Add)`.
- Update `sidebar_tabs_keep_their_element_ids` for the new tab.
- `#[gpui::test]`: trim a clip, drag a marker, press Ctrl+Z in the Timeline context: the marker
  returns and the trim stays; a second Ctrl+Z reverts the trim.
- Visual check: run `cargo run --manifest-path desktop/Cargo.toml -p fframes-studio --example ui_gallery`,
  capture the Slides tab and the timeline markers, inspect them, and attach the images to the
  phase report.
- `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-ui` and clippy.

## Success criteria

- [ ] Every resolved stop appears once in the tab and once on the timeline, in the same order.
- [ ] Notes typed in the tab are in `slides.json` after focus leaves, and undo restores them.
- [ ] Dragging, adding and removing markers write `slides.json` and never `timeline.json`.

## Risks

- The drawer is cached; a stale tab after an agent edit means a missing notify. Test by writing
  `slides.json` from outside and checking the tab updates.
- Marker drags and clip trim drags must not both start from one press: markers take priority
  only on the ruler and within the marker tolerance.
