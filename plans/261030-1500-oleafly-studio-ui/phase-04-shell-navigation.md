---
phase: 4
title: "Shell and navigation"
status: pending
priority: P1
effort: "1.5-2.5d"
dependencies: [3]
---

# Phase 04: Shell and navigation

## Goal and owned paths

Deliver the first complete live editor slice: flat neutral shell, consistent navigation and functioning native chrome around a real preview. Preserve editor architecture, not Oleafly's research layout. Root: `C:\Users\Administrator\fframes-desktop\`.

- Modify presentation only in `desktop\crates\studio-ui\src\shell\{layout,header,left_column,player,transport,status_strip,overflow}.rs`; kit fixes require a serial ownership handoff, not duplicated screen controls.
- Gallery: `desktop\app\examples\ui_gallery.rs` and capture `shell` fixtures if missing a state. Inspect `shell\sections.rs`, `keymap.rs`, `observation.rs`, `kit\windows_caption.rs`, `canvas_view.rs` but preserve caching/actions/probes.
- `left_column.rs` is released to phase05 after sidebar frame acceptance; `player`/`transport` to phase11 after current gate. No backend/preview-install code edits in the large `layout.rs` render function.

## Just-in-time scout

Read `StudioShell::render`, `render_header`, `compact_header_controls`, `render_sidebar`, `sidebar_width`, `render_drawer`, transport measured-width logic, status-details and overflow ids; inspect nearby tests before edits. Locate native drag/control areas and their placement outside cached header. Trace sections' theme invalidation and bounds probes. Read exact resizing calculations rather than replacing them from documentation.

## Tasks and steps

1. [ ] Lock shell fixtures and geometry tests first: no-project/export/error; long project name; normal/narrow widths; visible/hidden status details; disabled actions. Preserve header and transport id assertions and existing responsive behavior.
2. [ ] Finalize flat `Card::panel` player/sidebar/timeline framing, fine separators and sidebar fill; no persistent-pane drop shadows or floating-card gutters. Use phase02 theme, not local backgrounds, without touching frame/audio install fences.
3. [ ] Style header actions with existing shared controls. Keep52px header, action order, export accent, title truncation and icon-only collapse. Keep Close project labelled when no icon exists, undo/redo/settings icon-only, correct tooltips. Do not apply agent36px header to the main shell.
4. [ ] Finish six-tab rail selection and attention dot: Agent/Project/Assets/Style/Props/Slides, all shortcuts and ids `sidebar-tab-*` stable. Preserve automatic Agent selection for a new permission request and attention dot while another tab is selected. Sidebar frame only here; panel contents phase05.
5. [ ] Align transport/status/overflow vocabulary. Preserve timeline zoom and Fit/Split/scene cycle in transport and undo/redo in header; keep Refresh/Save checkpoint/Restore as copy/Reveal draft/Reveal checkpoint disabled rows and original ids. Retain status28px with error-opened Details, persistent errors vs transient Toast.
6. [ ] **Re-measure the width breakpoints (F5); don't keep them as they are.** Their numbers were measured with Cap-era 28 px buttons. Read the rendered widths of the header action cluster (project open and closed) and the transport tools (Full, Compact, Narrow) from the bounds telemetry or a gallery capture at 100 %. Then update `METRICS.layout.{actions_w, actions_closed_w, transport_compact_below, transport_narrow_below, transport_total_hidden_below}`, the comment in `transport.rs` and the `transport_mode` and header tests together. Keep the existing rule that a 1280 × 800 window still shows the zoom buttons, or record why it can't. Then check the player toolbar at the new breakpoints ± 1 px, measured by player width, not window width; controls never invade caption areas. Keep sidebar default416, limits320-520 and minimum480 player, proportional shrink at insufficient width; no persistence feature added for splits.
7. [ ] Run T/L/G; C `shell`, `slides`, `agent` in both themes. In N open a disposable SDK-backed project, play/seek/mute/build/cancel preview, resize sidebar and timeline, change tabs, open overflow, reveal status error and restore focus to composer. Capture this vertical flow after changes, not only a simulated shell.
8. [ ] On Windows exercise drag/maximize/restore/minimize/close, caption hover and OS keyboard alternatives (AltSpace/AltF4); Snap Layouts only on Windows11, mark N/A on current Windows10. Check125/150% DPI and long labels. On available macOS/Linux verify traffic-light82px allowance/system title bar; otherwise record explicit platform gate pending.

## Acceptance

- [ ] Real preview, tabs and window actions remain reachable and unchanged; no lost probes/ids/caption control areas through caching.
- [ ] Pane hierarchy looks quiet and consistent in both themes and at narrow/short sizes, keyboard focus never clipped or hidden under native chrome.
- [ ] Native open→preview→agent→settings-return flow succeeds; static `shell` fixture is supplemental only.
- [ ] Shared files handed serially to phase05; no playback/frame/audio logic changes hidden in visual diff.

Rollback: reverse only new chrome/layout styling hunks, keeping native control areas and pre-existing pane edits. If a change requires altering engine installation or resize semantics, stop and revise scope instead. Dependency03; estimate1.5-2.5d including native verification.
