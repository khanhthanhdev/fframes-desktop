# Phase 1: tokens

## Goal

`studio-theme` exposes one palette of roles, one metrics table, one type scale, one elevation API,
one motion table and the UI preferences. Every other vocabulary is deleted after its callers move.

## 1.1 Colours: `Theme.colors`

Rename `EditorPalette` to `Colors` and `theme.editor` to `theme.colors` in one mechanical commit
(about 300 lines, no value changes). Then extend and consolidate it.

| Role | Light / dark source | Replaces |
| --- | --- | --- |
| `window` | `editor.window` | `shell_bg`, `settings_content_bg`, `settings_sidebar_bg` |
| `card` | `editor.card` | `settings_card_bg`, `header_bg`, `Popover`/`Menu::plain` gray_1, literal `2a2a2c` |
| `card_2` | `editor.card_2` | gray_2/gray_3 fills used as nested surfaces |
| `stage` | `editor.stage` | – |
| `present_bg` | black / black | `gpui::black()` in `shell/present.rs:384,409` |
| `line`, `line_strong` | editor | `settings_border`, `settings_divider`, `header_border`, `body_border`, gray_3/5/6 borders |
| `text_1`, `text_2`, `text_3` | editor | gray_12, gray_11, gray_10, gray_9, `settings_text`, `settings_muted`, `text_primary`, `body_text`, legacy greys |
| `on_accent` | `on_accent()` | – |
| `ctl`, `ctl_hover`, `ctl_active` | editor | `settings_fill`, `settings_hover`, `settings_selection`, `body_fill`, `body_hover_fill`, gray_3/4/5/7 fills |
| `thumb` | editor | – |
| `accent`, `accent_2` | editor | `SETTINGS_ACCENT`, `blue_9`, `blue_500`, `ButtonVariant::Blue` 2563eb, `tile_selected_bg` |
| `selection` | `selection()` | `hsla(211°, .9, .55, .32)` in `kit/selectable_text.rs:27`, `agent/markdown/style.rs:55` |
| `focus` | = `accent` | `FOCUS_RING_PX` colour argument |
| `tooltip_bg`, `tooltip_text` | `tooltip_bg()`, `tooltip_text()` | `gray(12)`/`gray(1)` in `tooltip.rs:94` |
| `success`, `warning`, `danger` | green_11, amber_11, red_11 | raw `green_11`/`red_11`/`amber_11` at call sites |
| `success_bg`, `warning_bg`, `danger_bg` | green_3, amber_3, red_3 | `tone_surface` inputs |
| `danger_hover`, `danger_active` | mix(red_3, red_11, .12/.22) | the soft-red mixes in `button.rs`, `editor_button.rs` |
| `info`, `info_bg` | accent, ctl | – |
| `playhead` | editor | – |
| `track_clip`, `track_overlay`, `track_audio` | `track_color` | – |
| `scope` | amber_11 | `player.rs:1026` rectangle scope |
| `scrim` | black 55% | `present.rs:502` |

- **Segment colours:** keep `segment_colors_for` (`palette.rs:212`) as the one function and
  delete the unused `seg_*` and `tile_*` helpers on the palette.
- **Syntax:** `theme.syntax` holds the steps `agent/syntax` uses (comment, keyword, string,
  number, function, type, punctuation, added, removed), built from gray_10–12, blue_10/11,
  red_10/11, amber_11 and green_11.
- **Deleted from `Theme`:**
  - the public raw steps: they move into a private `radix` module used only to build `Colors` and
    `syntax`;
  - `gray(n)`, `*_legacy`, `blue_100`, `blue_500`, `red_300`, `text_primary`;
  - all `body_*` and `settings_*` helpers, `SETTINGS_ACCENT`;
  - `shell_bg`, `header_bg`, `header_border`, `ring_offset`, `settings_window_radius`,
    `tile_selected_bg`;
  - `rgba_alpha`, `hex`, `hexa` (internal only).
- **Contrast tests:** the existing table tests (`lib.rs:503-680`) move to the new roles. Add:
  `text_3` on `card` and `card_2` at 3:1 at least (placeholder minimum); `on_accent` on `accent`
  at 4.5:1; status foreground on its `_bg` at 4.5:1; `ctl_hover` visibly different from `card`
  (delta-L ≥ 2) in both modes.

Deletion order: add the roles, move the callers (phases 2–8), and delete the old helpers when
their count reaches zero. The guard enforces the count.

## 1.2 Metrics: `METRICS`

Group the fields. Values stay Cap's.

- `space`: 4, 8, 12, 16, 24, 32 (`space_8` is new; settings page gap 28 → 32).
- `radius`: `control` 7, `item` 6, `segment` 8, `tab` 9, `card` 12 (`popover` = `card`;
  `frame` merges into `item`), `pill` 999.
- `size`:
  - controls: `control_h` 28, `control_sm_h` 24, `select_h` 30, `input_h` 32, `field_row_h` 34;
  - rows: `list_row_h` 28, `list_row_2_h` 40;
  - icons: `icon_sm` 14, `icon_md` 16, `icon_lg` 20;
  - icon buttons: `icon_button` 24/28/32;
  - other: `chip_h` 28, `thumb_sm` 56, `slider_track` 3, `slider_thumb` 12, `progress_h` 4,
    `grip` 4×36 with a 14 hit area, `divider` 1, `focus_ring` 2.
- `layout`: the shell and timeline fields as today, plus the screen constants that are currently
  local:
  - header: `TRAFFIC_LIGHTS_W` 82, `TITLE_MIN_W`, `ACTIONS_W`, `ACTIONS_CLOSED_W`;
  - transport: breakpoints 1290, 820, 620; `zoom_slider_w` 72;
  - `player_min_h` (delete the duplicate `PLAYER_MIN_H`), `sidebar_*`;
  - settings window size 782×775, min 780×560, `windows_title_h` 36;
  - panel widths `start_max_w` 520, `slides_notes_h` 96;
  - timeline painter sizes: ruler ticks, stop flags, trim grip, playhead head, minimap.
- `opacity`: `disabled` 0.45 (moves from `kit/interact.rs:28`), `dragging` 0.7, `muted_media`
  0.55.
- Delete `control_pad_*` (unused). Kit internals use `METRICS` instead of their own `px(7.)`,
  `px(28.)` and `px(30.)`.

## 1.3 Type: `TextStyle`

Keep `Caption` 10, `Small` 11, `Label` 12, `Body` 13, `BodyStrong` 13 semibold, `Title` 14,
`Heading` 16, and add:

- `Code` 12/18 in `MONO_FAMILY` (moves into the theme; deletes `CODE_SIZE` ×2 and
  `MONO_FAMILY` in `agent/markdown/style.rs` and `agent/changes/paint.rs`);
- `CodeSmall` 11/16 for gutters and hunk headers;
- `Display` 18 semibold for the start screen title and markdown h1 (replaces the raw 16 export
  title and markdown 18/14.5).

`UI_FONT` is the only family constant for UI text. `text_style(el, style)` stays the API.
`ButtonSize` text sizes and the composer `INPUT_*` read from `TextStyle`.

## 1.4 Elevation

Only `Shadow::{Card, Pop, Preview, RaisedPill, Thumb}` through `shadow_for(appearance, ..)`.
Delete `preview_shadow`, `raised_pill_shadow`, `thumb_shadow`, `card_shadow`, `pop_shadow`, the
inline shadow in `Menu::settings` (`menu.rs:250`) and the gpui `shadow_lg()`/`shadow_sm()` calls.

## 1.5 Motion: `MOTION`

`fast` 120 ms, `normal` 200 ms, `slow` 320 ms, `tooltip_delay` 200 ms, `spin_period` 1100 ms,
`caret_blink` 500 ms, `veil_min` / `veil_max` from `veil.rs`. Easing `ease_out` for everything
that opens or folds.

Replaces `COLLAPSIBLE_DURATION_SECS`, `TOOLTIP_SHOW_DELAY`, `SPIN_PERIOD`, `CARET_BLINK_MS`,
`MIN_FADE_MS`/`MAX_FADE_MS` and the 12 `Duration::from_millis` sites.

`reduced_motion(cx)` reads the preference (1.6). When true, every animation snaps, the spinner
shows a static ring, and the streaming veil is off.

## 1.6 UI preferences

`AppSettings` (`studio-engine/src/app_settings.rs`) gains `motion: MotionPreference {System,
Reduced, Full}`. `System` reads the OS setting where GPUI exposes it, and is otherwise `Full`.

The preference lives in the same global as `ThemePreference`, renamed `UiPrefs { theme, motion }`.
`studio_theme::init(cx, prefs)`, `set_prefs` repaints every window, and `save` writes
`app-settings.json`. The settings General page adds a Motion `SegmentedControl` (id `motion-0..2`)
under Theme.

Out of scope: density and font scale. The metrics grouping makes a later `Density` a single
multiplier, but nobody has asked for it.

## 1.7 Theme access

Keep `Theme::for_window(window, cx)` per render. Views that cache `theme` keep `refresh`. No
global theme object. Kit components take `&Theme` and read roles themselves; callers no longer pass
colours (see phase 2).

## Checks

- `cargo test -p studio-theme`: role tables, the new contrast tests, metrics, typography,
  elevation, appearance and motion preference.
- After 1.1's rename commit: `cargo build` and `cargo test -p studio-ui` pass with no other diff.
- The guard allowlist does not grow.
- Captures: no visual change after this phase except the settings accent in dark mode, which now
  follows `accent` (`0a84ff`).
