# Phase 4: shell

Files under `desktop/crates/studio-ui/src/shell` except `export_panel.rs`, `settings*.rs` and
`onboarding.rs` (phase 7).

## Changes

| Surface | File:line | Change |
| --- | --- | --- |
| Header | `header.rs:32`, `shell/mod.rs:597,633` | `action_button` and `action_icon_button` become `Button` and `IconButton` (Export is `Primary`, others Neutral; undo, redo and Settings are icon-only). The divider at `:73` becomes `Divider::vertical`. The logo size and the `TRAFFIC_LIGHTS_W`/`TITLE_MIN_W`/`ACTIONS_*` constants move to `METRICS.layout` |
| Layout and grips | `layout.rs:19,472,510,594` | `ResizeGrip` for both grips. The timeline card is `Card::raised`. `PLAYER_MIN_H` is deleted in favour of `METRICS.layout.player_min_h` |
| Sidebar | `left_column.rs:26-139` | `TabRail::new`. The file row becomes `ListRow`. The drawer list becomes `List` |
| Project tab | `left_column.rs:185` | `Section` instead of `section_title` columns; `max_h 360` becomes a layout metric |
| Assets and Style tabs | `left_column.rs:249,278` | `Section`, `ListRow`, `EmptyState::inline` (replaces Body `text_2` at :160, :167, :330 and Small `text_3` at :289) |
| Start screen | `left_column.rs:320,522,557` | Title in `Display`, subtitle `Body` `text_2`, Create as `Primary`, Open and Import as Neutral; recents as two-line `ListRow`s with the hover action strip in `.actions(..)` (ids `recent-{id}`, `recent-actions-{id}`, `locate-{id}`, `remove-{id}`); "Missing" is a `Badge` Warning |
| Player | `player.rs:764,821,876,1046,1085` | Toolbar badges stay `Badge`. The empty stage becomes `EmptyState::centered`. The canvas message becomes a `Toast`. The rectangle scope colour becomes `colors.scope`. `min_h 120` becomes a metric |
| Transport | `transport.rs:16,118,139,269,299-320` | The zoom slider is `Slider::new`. The play button is `IconButton::new(..).size(Md).round().emphasis(Neutral)` (no `rounded(16)`). Gap 14 becomes `space_3` 12. Breakpoints and widths move to `METRICS.layout`. `vertical_divider` becomes `Divider::vertical` |
| Overflow menu | `overflow.rs:35,105` | `Menu::new`, `IconButton::new` |
| Notices and status bar | `status_strip.rs:17,154-245` | Notices stay `Notice`. The status bar keeps `StatusDot`. Details is a Ghost `Button` Sm. Detail lines are `Small` `text_2` from one helper |
| Present mode | `present.rs:375-502` | Background `colors.present_bg`; progress `Progress::linear(..).on_dark()`; the scrim is `colors.scrim` |

Also fixes the earlier review item: the present start hint gets the `scrim` behind it so it reads
on bright frames.

## Ids to keep

`header`, `drawer`, `player`, `agent-column`, `caption`, `close`, `create`, `settings`, `export`,
`install`, `build-preview`, `cancel-preview`, `play-pause`, `mute-audio`, `previous-frame`,
`next-frame`, `next-audio-output`, `retry-audio-output`, `no-audio-output`, `timeline-undo`,
`timeline-redo`, `timeline-split`, `timeline-cycle`, `timeline-zoom-in`, `timeline-zoom-out`,
`timeline-zoom-slider`, `timeline-fit`, `sidebar-tab-{Name}`, `recent-*`, `locate-*`,
`remove-*`, `file-{name}`, `inventory`, `recents`, `empty-state`, `left-library`,
`preview-surface`, `stage`, `canvas-hint`, `time-readout`, `status-strip`, `status-details`,
`project-path`, `agent-resize-handle`, `timeline-resize-handle`, `present-stage`,
`overflow-actions`, `overflow-menu`, and the `caption-*` buttons.

## Checks

- `x11_shell.rs` (uses `build-preview`, `next-frame`, `sections.agent`, `ruler_bounds`) and the
  `shell/observation.rs` whitelist test.
- The guard allowlist for `shell/` drops to 0 except layout constants that moved into `METRICS`.
- Captures: start screen, open project, notices, status details, overflow, present mode; light and
  dark; 1275×800 and 1920×1080.
