# Phase 8: timeline

The timeline is custom painting, which is fine. It only needs its values to come from the theme.

## Changes

- Move the painter constants into `METRICS.layout.timeline`:
  - `view.rs:1118-1126`: waveform padding, trim handle, stop flag sizes;
  - `segment.rs:10-12`: label padding, accent bar width;
  - `ruler.rs:7-12`: tick heights, label size (label text uses `TextStyle::CodeSmall` or `Caption`);
  - `playhead.rs:7-8`: head size, line width;
  - `minimap.rs:8-13`: size, bar height, handle width.
- `segment.rs:81` `rounded(px(6.))` becomes `radius.item`. The minimap's radius 2 becomes a
  timeline metric.
- Opacities become `METRICS.opacity.dragging` (view.rs:1371), `muted_media` (:1730,
  minimap.rs:151,169) and a timeline `stop_default` (:1584).
- The timeline takes the shell's `&Theme` instead of resolving `Theme::for_window` itself, like
  every other view.
- The empty state becomes `EmptyState::centered` ("No compiled timeline" plus Build preview).

Review items from the Slides work, fixed here:

- Stop flags no longer overlap the ruler labels: labels skip ticks that collide with a flag, and
  the last flag stays inside the ruler.
- Default-stop flags keep their drawn size, but get a 12 px hit area.
- While dragging a stop, a `Small` time readout (`Toast`-style pill) follows the flag.

## Ids to keep

`compiled-timeline`, `timeline-lanes`, `timeline-minimap`, `trim-start`, `trim-end`, `clip`,
`scene-{instance}`, and the `ruler_bounds` telemetry.

## Checks

- `timeline/view/tests.rs`, `canvas_selection.rs` and `x11_shell.rs` (`ruler_bounds`).
- The guard allows `px(` only in `const` items under `timeline/`.
- Captures: scenes, stops (default and explicit, dragging), trim, audio lanes, minimap; light and
  dark.
