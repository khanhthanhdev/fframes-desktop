# Studio design system

Every window and screen of Studio uses one look: Cap's editor style. There is one colour palette, one set of grouped sizes and one component kit, each its own crate: `desktop/crates/studio-theme` holds the tokens and `desktop/crates/studio-ui/src/kit` the components. Screens use these and do not declare colours, sizes or fonts of their own; the style guard (below) fails the build when they do.

Studio follows the `theme` setting: `System` (the default) tracks the OS light or dark appearance at runtime, `Light` and `Dark` fix it. The `motion` setting (`System`, `Reduced`, `Full`) is read the same way. Surfaces are flat; there is no glass.

## Principles

- **One style.** The settings window, the start screen, the agent panel and the editor share the editor's palette and components. There are no per-surface constructors or palettes.
- **One kit, one constructor each.** A component takes the `Theme` in `new` and reads colours and sizes itself. A call site passes content, state and handlers, never a colour or a pixel size.
- **Roles, not values.** A colour is a role (`text_2`, `accent`), a size is a `METRICS` entry, a text style is a `TextStyle`.
- **One Primary button per view, at most.**

## Tokens

A view resolves the theme once per render with `Theme::for_window(window, cx)` and passes it to the components. `studio_theme::init_prefs(cx, UiPrefs { theme, motion })` sets the preferences at start-up (`init(cx, theme)` is the same with the default motion), `set_preference` and `set_motion` change them and repaint every window, and each window calls `bind_window` so it repaints when the OS appearance changes.

### Colours: `theme.colors`

| Role | Use for |
| --- | --- |
| `window`, `card`, `card_2`, `stage` | window background, panels, nested surfaces, the player stage |
| `line`, `line_strong` | 1 px borders and dividers |
| `text_1`, `text_2`, `text_3` | primary, secondary and placeholder or disabled text |
| `ctl`, `ctl_hover`, `ctl_active` | control fill at rest, hovered or selected, pressed; translucent, so they work on any surface |
| `accent`, `accent_2`, `on_accent`, `focus` | primary action, toggle on, selection; hover shade; text on accent; the focus ring |
| `success`/`success_bg`, `warning`/`warning_bg`, `danger`/`danger_bg`, `info`/`info_bg` | status text or icon and the soft surface behind it (`kit::tone_color` and `tone_surface` pick them by `Tone`); `danger_hover`, `danger_active` for destructive controls |
| `selection`, `thumb`, `playhead`, `scope`, `scrim`, `present_bg`, `tooltip_bg`, `tooltip_text` | text selection, slider and toggle thumbs, the playhead, the video-scope rectangle, the dimming layer, present mode's black, tooltips |
| `segment_selected`, `caption_*` | the selected segmented item; the Windows caption buttons |
| `track_*`, `seg_*` and `segment_colors_for` | timeline tracks and segments |

The Radix scales behind these are private to `studio-theme`; nothing outside it names a Radix step.

### Sizes: `METRICS`

`METRICS` groups every size: `space_*` spacing; `radius` (`control`, `item`, `segment`, `tab`, `card`, `popover`, `pill`); `size` (control, select and input heights, list rows, icons, icon buttons, chip, thumbnails, slider, progress, grips, divider, focus ring); `layout` (header, transport, status bar, sidebar and timeline dimensions, settings window, painter sizes); `opacity` (disabled, dragging, muted media, default stop lines). The flat `radius_*` and size fields beside them are older names for the same values.

### Text, shadow, motion

- `text_style(element, TextStyle::..)`: `Caption` 10 medium, `Small` 11, `Label` 12 medium, `Body` 13, `BodyStrong` 13 semibold, `Title` 14 medium, `Heading` 16 semibold, `Display` 18 semibold, `Code` and `CodeSmall` in `MONO_FAMILY`. The font is the system UI font on every platform.
- `shadow_for(appearance, Shadow::..)`: `Card`, `Pop`, `Preview`, `RaisedPill`, `Thumb`.
- `MOTION`: `fast` 120 ms (hover and press), `normal` 200 ms (opening and folding, with `ease_out`), `slow` 320 ms, the tooltip delay, the spinner period, the caret blink. Anything that animates asks `prefers_reduced_motion(cx)` and jumps to its end state when it is true.

## Components

All components are gpui `RenderOnce` structs with a fluent API, and every one is in `kit`. Stateful parts (a menu, a text input, a toast host) are owned by the caller. Element ids are chosen by the caller and are what tests and telemetry look up; a component never invents an id a test depends on (`TabRail`, `MenuItem` and `ListRow` take theirs).

| Component | Role |
| --- | --- |
| `Button`, `IconButton` | actions. `Emphasis::{Neutral, Ghost, Primary, Danger}` and `ButtonSize::{Sm, Md}` (24 and 28 px); icon buttons at `IconButtonSize::{Small, Medium, Large}` (24, 28, 32 px) or `.round()`. An icon button's `label` is its accessible name |
| `Toggle`, `SegmentedControl`, `Select` and `Menu`, `Slider`, `NumberField`, `TextInput`, `ColorPicker`, `RadioCards` | controls. `Select` is the trigger and reports its bounds; build a `MenuState::anchored(bounds, &items)` and render `Menu` while it is open. Disabled `MenuItem`s show in `text_3`, are skipped by arrow keys and ignore clicks |
| `Badge`, `StatusDot`, `Chip` | small tone-coloured labels, dots and removable tags (`Tone::{Neutral, Info, Success, Warning, Danger}`) |
| `Notice` | a blocking or persistent message: tone icon and surface, optional actions, collapsible details (160 px high at most, then they scroll) and dismiss |
| `Toast`, `ToastHost` | a transient confirmation that goes away after `TOAST_DURATION` (3 s), kept while hovered |
| `Progress` (linear), `CircularProgress`, `Spinner` | progress and activity; the spinner holds still under reduced motion |
| `Card`, `Popover`, `Divider`, `ResizeGrip` | surfaces and lines: raised and nested cards, the floating surface of a menu, hairlines on both axes, the drag handle between panes |
| `PanelSection` | a titled group in a panel, tab or settings page (title, description, trailing element), 24 px apart when stacked; `.card()` nests its content on `card_2` |
| `FormField` | a label with its control, `FormField::row(..)` or `FormField::stacked(..)`, with hint, error and reset |
| `List`, `ListRow`, `EmptyState` | lists: a row has a leading element or icon, title, meta, trailing element and hover actions, and can be selected, muted or disabled; `.label(..)` sets its accessible name. `EmptyState` is inline in a panel or `.centered()` on a stage |
| `TabRail`, `SettingsFrame`, `CollapsibleSection` | navigation and folding |
| `Tooltip`, `KbdChip`, `selectable` | tooltip with an optional key chip; text that can be selected and copied: drag for a range, double click for a word, triple click for the block, Ctrl+C (Cmd+C). One selection in the app; button, tab, menu and list-row labels stay unselectable |

Icons are Lucide SVGs embedded by `desktop/app/src/assets.rs` and named by `kit::Icon`; adding one means the file in `desktop/app/assets/icons`, its entry in `assets.rs`, and the variant in `Icon` and `Icon::ALL` (a test checks the three agree).

Run the gallery to see every component in every state, in System, Light or Dark: `cargo run --manifest-path desktop/Cargo.toml -p fframes-studio --example ui_gallery`. Its capture mode writes PNGs of each page for review: `... --example ui_gallery -- --capture <dir> [--appearance light|dark] [--page <name>]`.

## Recipes

- **A panel is `PanelSection`s of `FormField`s.** Stack sections in a column; put a trailing count or action in the section title row ( `PanelSection::column()` stacks them).
- **A list is a `List` of `ListRow`s.** Pass `.empty(EmptyState::new(..))` for the no-rows case.
- **An empty panel is `EmptyState` (inline)**: a title, one line of body and at most one action. Use `.centered()` only for the stage, the start screen and the timeline.
- **A blocking or persistent message is a `Notice`.** Errors are Notices, never toasts.
- **A transient confirmation ("Copied", "Saved") is a `Toast`**, shown through the window's `ToastHost`.
- **One Primary button per view at most.** Other actions are Neutral or Ghost; a destructive action is Danger.

## States

Every control has the same states, drawn by the kit:

- **Hover** and **pressed**: `ctl` to `ctl_hover` to `ctl_active` (Primary: `accent`, `accent_2`); `MOTION.fast`.
- **Selected**: `ctl_active` fill (a tab, a list row, a segmented item) or the accent (a toggle on).
- **Disabled**: `Opacity.disabled` (45 %); ignores clicks and keys and is skipped by Tab.
- **Focus**: a 2 px `focus` ring while the control has keyboard focus (see below).
- **Reduced motion**: the `motion` setting, or the OS preference under `System`. Fades and slides end at once and the spinner stops turning.

## How to add a value

Add the role to the theme (`Colors`, `METRICS` or `TextStyle`) and use it from there, never at the call site. A new colour is a new `Colors` role set for both appearances; a new size is a `METRICS` entry in the group it belongs to. If the same value would be used by one component only, it belongs inside that component, still read from the theme.

The style guard enforces this. `desktop/crates/studio-ui/tests/style_guard.rs` fails when a screen or kit file under `studio-ui/src` uses a colour literal, a Radix step, `text_size`, `font_weight`, a literal `px(..)` size, a literal opacity or the old per-surface constructors. Both allowlists (`style_guard_allowlist.txt` for screens and `style_guard_kit_allowlist.txt` for the kit) are empty and must stay so: a count may only go down, and a new file gets no entry. Two exceptions are by design and need no entry: `px(<digit>` inside `const` items (the timeline painters, the agent diff painter) and the `syntax` colour set the theme gives the code highlighter.
## Screen behaviours

- The overflow menu in the transport bar always lists Refresh, Save checkpoint, Restore as copy, Reveal retained draft and Reveal checkpoint. Actions that do not apply are disabled rows, and the rows keep the ids their buttons had (`refresh`, `checkpoint`, `restore`, `draft`, `reveal-checkpoint`).
- The composer's send control is an icon button (`composer-send`) with a menu toggle beside it (`composer-send-menu-toggle`); the menu holds the other send actions, **Send now** (`composer-send-now`, cancel the running turn and send) among them. When sending is not possible the reason shows as text beside the controls (`desktop/crates/studio-ui/src/agent/composer/mod.rs`).
- Reasoning rows stay collapsed. While one streams, its header shows a "Thinking…" badge; the header has a fixed height so the row does not change size when the badge goes away.
- The setup screen remembers which step was active when setup failed and marks it failed (earlier steps done, later ones pending). Install, build and render run as one pipeline, so a failure inside it is attributed to "Install SDK".

## Keyboard and focus

Every control is a tab stop (disabled ones are skipped), shows a 2 px `accent` focus ring while it has keyboard focus, and activates on Enter and Space. Disabled controls ignore clicks and keys and drop to 45 % opacity.

## Telemetry ids

Tests find controls by the bounds the app reports, through the observation telemetry described in [architecture, production probes](architecture.md#13-production-probes). Element ids and labels are chosen by the caller and must not change. A control that is measured passes `.on_bounds(BoundsProbe::new(..).into_bounds_handler())` (`BoundsProbe` is in `desktop/crates/studio-ui/src/probe.rs`), and the callback receives `[left, top, width, height]` on every prepaint. Components never invent ids that a test looks up.

While telemetry is being written, the agent panel's `buttons` telemetry reports the ids listed in `probed_button` in `desktop/crates/studio-ui/src/agent/mod.rs` (the panel's tabs, Apply, Undo, Stop, review, setup and question controls), so the X11 shell test clicks measured bounds instead of fixed window coordinates and the parity tests can check what was laid out.

## Deliberate differences from Cap

- A visible focus ring and Enter or Space activation on every control; Cap's widgets mostly have neither.
- The system UI font on every platform; no bundled font.
- Components Cap does not have: `Notice`, `Badge`, `StatusDot`, `Toast`, `ListRow`, `EmptyState`, `FormField`, `PanelSection`, and the `motion` preference (reduced motion).
- A 16 px `Heading` size for empty-state titles, and the `Notice`, `Badge` and `StatusDot` components, which Cap does not have.
- The window follows Cap's editor layout: under the 52 px header, the player card and a 416 px sidebar card share a row, and the timeline card spans the full width below them. The sidebar has six tabs (`sidebar-tab-Agent`, `sidebar-tab-Project`, `sidebar-tab-Assets`, `sidebar-tab-Style`, `sidebar-tab-Props`, `sidebar-tab-Slides`) in place of Cap's editing tabs. The Slides tab lists the slide stops; on the timeline each stop is a flag on the ruler and a line through the lanes (hollow and dashed for default clip-end stops), dragged along the ruler, added by a double click on it or `K`, removed with Delete. **Present** (`present` in the player toolbar, F5, or "Present from here" in the Slides tab) plays the slides full screen: the player section shows only the frame, letterboxed on black, with a thin strip (slide N of M) for two seconds after each change; the pointer hides after two seconds still. Its keys live in the `Present` context of the presentation stage: →, Space, PageDown and a click play the next slide or finish the one playing, ←/PageUp go back a stop, Home/End go to the first/last slide, a number and Enter go to that slide, Esc leaves and focuses the preview. Agent is selected at start. While another tab is open, the Agent tab shows an accent dot when the agent waits for an answer, and a new permission request selects the Agent tab. The grip on the sidebar's left edge (`agent-resize-handle`) keeps it between 320 and 520 px and leaves the player 480 px; a window too narrow for both shrinks them in proportion (`sidebar_width` in `shell/layout.rs`). The grip above the timeline (`timeline-resize-handle`) drags the split between the row and the timeline with Cap's limits. Neither split is saved between launches.
- The timeline card has no header row. Its zoom, Fit, Split and scene-cycle controls sit in the player's transport bar (`timeline-zoom-out`, `timeline-zoom-slider`, `timeline-zoom-in`, `timeline-fit`, `timeline-split`, `timeline-cycle`), and undo and redo are in the header (`timeline-undo`, `timeline-redo`). The transport keeps Cap's three parts: the clock, playback in the centre, and the tools. Playback stays centred while the tools fit half the bar, so below a 1290 px player the zoom slider and the Split and Build labels go, and below 820 px the zoom buttons and Fit (`shell/transport.rs` has the measured widths). Studio-only controls stay: mute, build and cancel preview, and the overflow menu. `FocusPrompt` focuses the composer. Source, checkpoint and job sit in a 28 px status bar at the window's foot (`status-strip`), whose **Details** button (`status-details`) shows the other status lines above it; an error opens them.
- The header is flush with the window's top edge on every system, as in Cap's editor. On Windows it is the title bar: the row around the header is the drag area and Cap's caption buttons (`caption-minimize`, `caption-maximize`, `caption-close`) sit at its right end. Cap presses them through its own Win32 calls; here they are native window control areas, so Windows itself minimizes, maximizes, restores and closes, and offers Snap Layouts over maximize on Windows 11. They live outside the cached header because a cached view does not replay control areas. Like the system's own caption buttons, and unlike every other control, they are not tab stops and have no focus ring: Alt+F4 and Alt+Space reach the same commands from the keyboard. On macOS the header leaves 82 px for the traffic lights, inset to (20, 20), and its empty middle moves the window; Linux keeps the system's title bar, as Cap's editor does. The actions are undo and redo, a divider, Create, Open, Import Rust and Close project, Settings, and Export last with the accent fill, as Cap ends its header with Export. When the window is too narrow for the title, the labelled actions and the window controls, the actions show only their icons (with the label as tooltip); Close project has no icon and keeps its label. Undo, redo and Settings are always icon-only.
- Settings open in a window of their own, as in Cap (782 × 775, at least 780 × 560): the header's Settings button (`settings`) opens it or brings it to the front, and the editor keeps playing beside it. On Windows a 36 px title strip across the window holds the caption buttons; on macOS the traffic lights sit above the nav column. Its close button or Ctrl+W (Cmd+W on macOS) closes it; only closing the main window quits. It does not open while the first-run setup shows, and Run setup again closes it. Its pages hold every setting of the app: **General** (theme, audio output, Run setup again, updates), **Agent** (review policy, permissions, then the agent panel's adapter setup), **SDK** (the managed SDK), **Presets** (the project's style presets) and **Export** (the defaults every export starts from); nav rows `settings-general` to `settings-export`. Per-project parts ask for an open project when none is open. The adapter setup and the preset panel are drawn by their own views, so their fields and telemetry stay theirs; the panel's settings button and `OpenAgentSetup` open the Agent page, the Style tab's **Style presets…** (`open-presets`) the Presets page. The editor keeps only read-outs of these settings: the audio device above the preview and the review policy in the agent's Changes tab. The export panel's choices apply to that export only. On General a `SegmentedControl` (System, Light, Dark; options `theme-0` to `theme-2`) replaces Cap's preview tiles; the choice applies at once and is saved in `app-settings.json`. Cap's nav rows answer only the mouse; here they are tab stops that Enter and Space open.
- Cap's colours, with `accent`, `accent_2`, `text_2` and `text_3` darkened to meet contrast: white on `accent` 4.5:1, `text_2` on `window` 4.5:1, `text_3` on `card` and `card_2` 3:1 (a `studio-theme` test checks them).
- Canvas action errors are a dismissible `Notice` over the stage and stay until dismissed; only plain status messages are 3-second toasts.

## Licensing

The component kit in `studio-ui` is ported from Cap's desktop app, with the permission of Cap's owners. Icons are Lucide (ISC); the licence is in `desktop/packaging/sdk/notices/lucide-LICENSE.txt`. The four Windows caption glyphs (`caption-*-windows.svg`) come from Cap.
