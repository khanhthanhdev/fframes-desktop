# Studio design system

Every window and screen of Studio uses one visual language, Oleafly's: quiet neutral surfaces, one blue accent (`#2563eb`) kept for primary actions, selection and focus, flat panes that meet at hairlines, quiet chrome, Geist type and rounded compact controls with Lucide icons. The [Oleafly extraction and reproduction reference](oleafly-design-system.md) records the source values. Studio keeps its own video-editor layout and workflows (see [Layout and behaviour](#layout-and-behaviour)) and a few deliberate differences (see [Deliberate differences from Oleafly](#deliberate-differences-from-oleafly)).

There is one colour palette, one set of grouped sizes and one component kit, each its own crate: `desktop/crates/studio-theme` holds the tokens and `desktop/crates/studio-ui/src/kit` the components. Screens use these and do not declare colours, sizes or fonts of their own; the style guard (below) fails the build when they do.

Studio follows the `theme` setting: `System` (the default) tracks the OS light or dark appearance at runtime, `Light` and `Dark` fix it. The `motion` setting (`System`, `Reduced`, `Full`) is read the same way. Surfaces are solid; there is no glass, acrylic or vibrancy.

## Principles

- **One style.** The settings window, the start screen, the agent panel and the editor share one palette and one kit. There are no per-surface constructors or palettes.
- **Flat panes, raised overlays.** Persistent panes (player, sidebar, timeline, settings) are flat and separated by hairlines; only temporary surfaces (menus, popovers, tooltips) carry a shadow.
- **Quiet chrome.** Neutral text and transparent or translucent controls; the accent appears only where it means something.
- **One kit, one constructor each.** A component takes the `Theme` in `new` and reads colours and sizes itself. A call site passes content, state and handlers, never a colour or a pixel size.
- **Roles, not values.** A colour is a role (`text_2`, `accent`), a size is a `METRICS` entry, a text style is a `TextStyle`.
- **One Primary button per view, at most.**

## Tokens

A view resolves the theme once per render with `Theme::for_window(window, cx)` and passes it to the components. `studio_theme::init_prefs(cx, UiPrefs { theme, motion })` sets the preferences at start-up (`init(cx, theme)` is the same with the default motion), `set_preference` and `set_motion` change them and repaint every window, and each window calls `bind_window` so it repaints when the OS appearance changes.

### Colours: `theme.colors`

| Role | Use for |
| --- | --- |
| `window`, `card`, `card_2`, `sidebar`, `popover`, `stage` | the document surface (workspace panes, fields), cards, nested surfaces, persistent navigation (the tab rail, the settings nav), menus and other floating overlays, the player stage |
| `line`, `line_strong` | hairlines (`METRICS.size.divider`, 0.5 px) and stronger field outlines |
| `text_1`, `text_2`, `text_3` | primary, secondary and placeholder or disabled text |
| `ctl`, `ctl_hover`, `ctl_active` | translucent control fills at rest, hovered, pressed or selected |
| `accent`, `accent_2`, `on_accent`, `focus` | primary action, toggle on, the user's bubble; its hover shade (`#1d4ed8`); text on accent; the focus ring (the accent) |
| `success`/`success_bg`, `warning`/`warning_bg`, `danger`/`danger_bg`, `info`/`info_bg` | status text or icon and the soft surface behind it (`kit::tone_color` and `tone_surface` pick them by `Tone`); `info` is the accent and `info_bg` the accent at 10 % (Oleafly's `bg-primary/10`); `danger_hover`, `danger_active` for destructive controls |
| `selection`, `thumb`, `playhead`, `scope`, `scrim`, `present_bg`, `tooltip_bg`, `tooltip_text` | text selection (the accent at 25 %), slider and toggle thumbs, the playhead, the video-scope rectangle, the dimming layer, present mode's black, tooltips |
| `segment_selected`, `caption_*` | the selected segmented item; the Windows caption buttons |
| `track_*`, `seg_*` and `segment_colors_for` | timeline tracks and segments |

The surfaces are opaque and follow Oleafly's neutrals: light `window` `#ffffff`, `card` and `popover` `#fafafa`, `sidebar` and `stage` `#f7f7f7`, `card_2` `#f1f1f1`; dark `window` `#0a0a0a`, `card`, `sidebar` and `stage` `#171717`, `popover` `#121212`, `card_2` `#262626`. Dark hairlines are white at 10 % (`line`) and 15 % (`line_strong`).

The control fills are translucent: black at 6, 10 and 14 % in light, white at 8, 12 and 16 % in dark. A control sits on every surface (window, card, nested card, sidebar, popover), and an opaque fill equal to one of them, as `card_2` once was, made neutral buttons and segmented troughs look like plain text there. `control_fills_show_on_every_surface` composites each fill over each surface and requires a visible step from the surface to rest, hover and pressed.

White on `accent` and on `accent_2` meets 4.5:1; `text_2` meets 4.5:1 on window, card and nested card, and `text_3` 3:1 on cards (light `text_2` is `#6b6b6b`, a little stronger than Oleafly's, for that reason). `studio-theme`'s tests check these. Status, timeline and syntax colours keep Studio's meanings. The Radix scales behind the status and syntax colours are private to `studio-theme`; nothing outside it names a Radix step.

### Sizes: `METRICS`

`METRICS` groups every size: `space_*` spacing (4, 8, 12, 16, 24, 32); `radius` (`control` 8, `item` 6, `segment` 8, `tab` 6, `card` 10, `popover` 8, `pill`); `size` (control, select and input heights, list rows, icons, icon buttons, chip, thumbnails, slider, progress, grips, divider, focus ring); `layout` (header, transport, status bar, sidebar and timeline dimensions, breakpoints, settings window, workflow column, painter sizes); `opacity` (disabled, dragging, muted media, default stop lines). The flat `radius_*` and size fields beside them are older names for the same values.

The sizes follow what Oleafly uses most, not its defaults:

| What | Size |
| --- | --- |
| `Button`, `ButtonSize::Md` (Oleafly `sm`) | 32 px, 12 px padding, `Label` text |
| `Button`, `ButtonSize::Sm` (Oleafly `xs`) | 24 px, 8 px padding, `Label` text |
| `IconButton` `Sm`, `Md`, `Lg` | 24, 28, 32 px; 28 is the toolbar size |
| `TextInput`, `Select` | 36 px (Oleafly `h-9`) |
| `ListRow`, menu rows | 32 px; a two-line list row 44 px |
| `Chip` | 28 px |
| Header, transport, status bar | 52, 48, 28 px |
| `layout.gutter` | 0: the panes meet at hairlines, with no gaps or shadows between them |

The header and transport drop labels by width, at breakpoints measured from the rendered bounds on Windows with Geist, 32 px labelled buttons and 28 px icon buttons, plus a margin for other systems' fonts: the header's actions measure 649 px with a project open and 239 px without (`layout.actions_w` 677 and `actions_closed_w` 258); the transport goes compact below a 1270 px player, narrow below 830 px, and drops the clock's total below 520 px (`shell/transport.rs` has the measured widths; tests in `header.rs` and `transport.rs` check the constants against them).

### Text

Interface text is Geist and code is Geist Mono, both bundled: the full Geist 1.9.0 and Geist Mono 1.9.0 builds from `vercel/geist-font` (OFL; Regular, Medium, SemiBold and Mono Regular), not the web's Latin subset. `desktop/app/src/assets.rs` registers them with `init_fonts` before any window opens, and `bundled_fonts_cover_vietnamese_and_extended_scripts` checks that every file maps Vietnamese, Latin Extended, Cyrillic, Greek and arrows. `UI_FONT` is set once at each window root (the shell, the settings window, Present, the gallery) and on tooltips, which are roots of their own; views inherit it. `MONO_FAMILY` comes with the `Code` styles; the transport clock sets it directly.

`text_style(element, TextStyle::..)` sets size, weight and line height:

| Style | Size / line | Weight | Use |
| --- | --- | --- | --- |
| `Caption` | 10/14 | medium | key chips, message times |
| `Small` | 12/16 | regular | secondary text, descriptions, inline values |
| `Label` | 12/16 | medium | buttons, labels, segmented items, menus, chips, badges |
| `Overline` | 12/16 | semibold | section headings, in capitals |
| `CodeSmall` | 11/16 | regular, mono | diff gutters, hunk headers, changed-file paths |
| `Code` | 12/18 | regular, mono | code blocks, inline code, diffs |
| `Body` | 14/20 | regular | body text, fields, the composer |
| `BodyMedium` | 14/20 | medium | setting-row labels, the selected settings nav row |
| `BodyStrong` | 14/20 | semibold | agent action-card titles, the settings page header |
| `Title` | 16/24 | medium | card titles |
| `Heading` | 18/24 | semibold | dialog, workflow and empty-state titles |
| `Display` | 24/32 | semibold | the start screen title, markdown h1, the agent's home |

### Shadow and motion

- `shadow_for(appearance, Shadow::..)`: `Card` is none, so raised and nested cards are flat; `Pop` (menus, popovers, tooltips, toasts, the colour picker, the agent's jump pill) is black at 15 % in light and 30 % in dark, 4 px down, 12 px blur, −2 px spread; `RaisedPill` lifts the selected segmented item in light (none in dark, where `segment_selected` is a lighter wash) and the agent composer; `Preview` under the player's frame and `Thumb` under slider thumbs.
- `MOTION`: `fast` 150 ms (hover and press), `normal` 200 ms (opening and folding, with `ease_out`), `slow` 320 ms, tooltip delay 300 ms, the spinner period, the caret blink. Anything that animates asks `prefers_reduced_motion(cx)` and jumps to its end state when it is true.

## Components

All components are gpui `RenderOnce` structs with a fluent API, and every one is in `kit`. Stateful parts (a menu, a text input, a toast host) are owned by the caller. Element ids are chosen by the caller and are what tests and telemetry look up; a component never invents an id a test depends on (`TabRail`, `MenuItem` and `ListRow` take theirs).

| Component | Role |
| --- | --- |
| `Button`, `IconButton` | actions. `Emphasis::{Neutral, Ghost, Primary, Danger}` and `ButtonSize::{Sm, Md}` (24 and 32 px); icon buttons at `IconButtonSize::{Sm, Md, Lg}` (24, 28, 32 px), `.round()` or `.inverted()` (a `text_1` disc: the transport's play, the composer's Stop). An icon button's `label` is its accessible name |
| `Toggle`, `SegmentedControl`, `Select` and `Menu`, `Slider`, `NumberField`, `TextInput`, `ColorPicker`, `RadioCards` | controls. The toggle is Oleafly's 36 × 20 switch with a 16 px knob, `ctl_active` off and `accent` on. A segmented control is a `ctl` trough with 24 px items, the selected one a raised `segment_selected` pill. `Select` is the trigger and reports its bounds; build a `MenuState::anchored(bounds, &items)` and render `Menu` while it is open. Disabled `MenuItem`s show in `text_3`, are skipped by arrow keys and ignore clicks |
| `Badge`, `StatusDot`, `Chip` | a badge is a soft pill: `Label` text in the tone's colour on its soft surface, fully round, no border (neutral is `text_2` on `ctl`; dark info text is lifted toward `text_1` to stay legible). A status dot is 6 px. A chip is a 28 px `ctl` token at the control radius with an optional icon and a remove button (`Tone::{Neutral, Info, Success, Warning, Danger}`) |
| `Notice` | a blocking or persistent message: tone icon and surface, optional actions, collapsible details (160 px high at most, then they scroll) and dismiss |
| `Toast`, `ToastHost` | a transient confirmation that goes away after `TOAST_DURATION` (3 s), kept while hovered |
| `Progress` (linear), `CircularProgress`, `Spinner` | progress and activity; the spinner holds still under reduced motion |
| `Card`, `Popover`, `Divider`, `ResizeGrip` | surfaces and lines: `Card::panel` is a flat square workspace pane on `window` behind hairlines; `Card::raised` a rounded `card` with a hairline and no shadow; `Card::nested` the same on `card_2`; `Popover` the floating `popover` surface with `Pop`; hairlines on both axes; the drag handle between panes |
| `PanelSection` | a titled group in a panel, tab or settings page: the title is an Oleafly section heading (`Overline`, `text_2`, upper-cased by the component), then an optional `Small` description and a trailing element in the title row; content 8 px below. `PanelSection::column()` stacks sections 24 px apart; `.card()` nests the content on `card_2` |
| `SettingRow`, `setting_card` | one setting, Oleafly's setting row: a raised card 12 px in, the label (`BodyMedium`) and description (`Small`, `text_2`) on the left and the control on the right, wrapping below the text when the row is too narrow; `.below(..)` puts a wide control under the text. `setting_card` is the same surface for content that is not a label and a control. Used by settings, setup, Create and Export |
| `workflow_column`, `workflow_title` | a workflow page shown in place of the editor (Create, Export): a centred column at most 672 px wide (Oleafly's `max-w-2xl`), under a `Heading` title and a `Body` `text_2` line |
| `FormField` | a label with its control, `FormField::row(..)` or `FormField::stacked(..)`, with hint, error and reset |
| `List`, `ListRow`, `EmptyState` | lists: a 32 px row (44 px with a meta line) has a leading element or icon, title, meta, trailing element and hover actions, and can be selected, muted or disabled; `.label(..)` sets its accessible name. `EmptyState` is inline in a panel or `.centered()` on a stage |
| `TabRail`, `SettingsFrame`, `CollapsibleSection` | navigation and folding. The tab rail is a bar of 40 × 30 icon tabs on `sidebar` above a hairline. `SettingsFrame` is a 227 px nav column on `sidebar` beside the document surface, whose 48 px content header holds the page's title (`BodyStrong`) over a hairline; its nav rows are Oleafly's `NavItem` (16 px icon, `Body` in `text_2`; selected: `window` fill behind a hairline, `text_1`, `BodyMedium`) |
| `Tooltip`, `KbdChip`, `selectable` | tooltip with an optional key chip; text that can be selected and copied: drag for a range, double click for a word, triple click for the block, Ctrl+C (Cmd+C). One selection in the app; button, tab, menu and list-row labels stay unselectable |

Icons are Lucide outline SVGs embedded by `desktop/app/src/assets.rs` and named by `kit::Icon`, 56 in all (among the latest: `Copy`, `Brain`, `Paperclip`, `Clock`, `ShieldQuestion`, `FileDiff`), plus the four Windows caption glyphs. Adding one means the file in `desktop/app/assets/icons`, its entry in `assets.rs`, and the variant in `Icon` and `Icon::ALL` (tests in `assets.rs` check that they agree).

Run the gallery to see every component in every state, in System, Light or Dark: `cargo run --manifest-path desktop/Cargo.toml -p fframes-studio --example ui_gallery`. Its capture mode writes PNGs of each page for review: `... --example ui_gallery -- --capture <dir> [--appearance light|dark] [--page <name>]`.

## Agent panel

The agent panel follows Oleafly's ACP panel with its own denser sizes, in `studio-ui/src/agent/ui/metrics.rs` and `agent/composer/metrics.rs`; global buttons and fields keep theirs.

- **Header**: a 36 px row (Oleafly's `h-9`) with the panel's tabs on the left and 28 px icon buttons on the right, a hairline under it; a one-line status strip below.
- **Messages**: the user's message is an `accent` bubble with `on_accent` text on the right, at most 85 % of the row; the agent's answer is framed across the row (light: `window` behind `line` at 60 %; dark: `card_2` with no visible line), both at the card radius. Each has a 28 px footer with the time and Copy, always reserved so hover and focus never move a row.
- **Activity**: consecutive tool calls, reasoning and permission notes form one run, a muted summary line ("Read 3 files · edited 2 files · 1 failed") that opens into 24 px step rows. Reasoning is a step ("Thought" and its first line) that is open while it streams or while the reader keeps it open; open, its text sits beside a rule and grows to 224 px before it scrolls.
- **Composer**: a `card` with 22 px corners (Oleafly's `rounded-[1.375rem]`), the `RaisedPill` shadow and a `line` border that turns `focus` while the input has focus. The input rests at 56 px and grows to 8 lines (160 px) before it scrolls; attachment thumbnails are 56 px. Send (`composer-send`, a round Primary icon button) and, while the agent runs, Stop (`composer-stop`, an inverted disc with a filled square) are both shown, 32 px, with the attach buttons and the send menu toggle at 28 px. When the controls row is at most 384 px wide (Oleafly's 24rem container query) its gaps tighten to 2 px, the attach targets to 24 px and Send and Stop to 28 px.
- **Permission requests**: a card inside a 2 px static `accent` ring at a 14 px corner, a 20 px shield, the title (`BodyStrong`) over the `Small` body, and the actions under a hairline: refusals first, every allowing option Primary. The ring is used for nothing else, so it keeps meaning "the agent waits for your permission".
- **Questions**: a neutral card (`card`, `line`, no tone wash) with a `Label` title and `RadioCards` per item; answers are sent together only once every item has one.
- **Changes**: a muted `card` at the control radius with the file-diff glyph, the count in `Label` and a compact 24 px Review button, then a 24 px row per file with its mark and its path in `CodeSmall`.

## Recipes

- **A panel is `PanelSection`s of `FormField`s.** Stack sections in a column (`PanelSection::column()`); put a trailing count or action in the section title row.
- **A settings page, setup step or workflow is `PanelSection`s of `SettingRow`s.** A workflow page goes in a `workflow_column` under a `workflow_title`.
- **A list is a `List` of `ListRow`s.** Pass `.empty(EmptyState::new(..))` for the no-rows case.
- **An empty panel is `EmptyState` (inline)**: a title, one line of body and at most one action. Use `.centered()` only for the stage, the start screen and the timeline.
- **A blocking or persistent message is a `Notice`.** Errors are Notices, never toasts.
- **A transient confirmation ("Copied", "Saved") is a `Toast`**, shown through the window's `ToastHost`.
- **One Primary button per view at most.** Other actions are Neutral or Ghost; a destructive action is Danger.

## States

Every control has the same states, drawn by the kit:

- **Hover** and **pressed**: a Neutral button goes `ctl` to `ctl_hover` to `ctl_active`, a Ghost one from no fill; Primary `accent` to `accent_2`; Danger `danger_bg` to `danger_hover` to `danger_active`; `MOTION.fast`.
- **Selected**: a list row or a sidebar tab sits on `ctl_hover` and the others turn the quieter `ctl` under the pointer (Oleafly's sidebar and file tree); a pressed button `ctl_active`; a segmented item the raised `segment_selected` pill; a settings nav row the `window` fill behind a hairline; a toggle the accent.
- **Disabled**: `Opacity.disabled` (50 %); ignores clicks and keys and is skipped by Tab.
- **Focus**: a 2 px `focus` ring while the control has keyboard focus (see below).
- **Reduced motion**: the `motion` setting, or the OS preference under `System`. Fades and slides end at once and the spinner stops turning.

## How to add a value

Add the role to the theme (`Colors`, `METRICS` or `TextStyle`) and use it from there, never at the call site. A new colour is a new `Colors` role set for both appearances; a new size is a `METRICS` entry in the group it belongs to. If the same value would be used by one component only, it belongs inside that component, still read from the theme.

The style guard enforces this. `desktop/crates/studio-ui/tests/style_guard.rs` fails when a screen under `studio-ui/src` uses a colour literal, a Radix step, a font family named by a string literal (`font_family("...")`, raw strings included), `text_size`, `font_weight`, a literal `px(..)` size, `rounded(px(..))`, a literal opacity or the old per-surface helpers, and when a kit file uses a colour literal, a Radix step or a literal font family. Fonts come from `UI_FONT`, `MONO_FAMILY` or a `TextStyle`. Both allowlists (`style_guard_allowlist.txt` for screens and `style_guard_kit_allowlist.txt` for the kit) are empty and must stay so: a count may only go down, and a new file gets no entry. Two exceptions are by design and need no entry: `px(<digit>` inside `const` items (the timeline painters, the agent diff painter) and the `syntax` colour set the theme gives the code highlighter.

## Screen behaviours

- The overflow menu in the transport bar always lists Refresh, Save checkpoint, Restore as copy, Reveal retained draft and Reveal checkpoint. Actions that do not apply are disabled rows, and the rows keep the ids their buttons had (`refresh`, `checkpoint`, `restore`, `draft`, `reveal-checkpoint`).
- The composer's send control is an icon button (`composer-send`); while the agent runs, a menu toggle beside it (`composer-send-menu-toggle`) holds the other send actions, **Send now** (`composer-send-now`, cancel the running turn and send) among them. When sending is not possible the reason shows as text beside the controls (`desktop/crates/studio-ui/src/agent/composer/mod.rs`).
- The setup screen remembers which step was active when setup failed and marks it failed (earlier steps done, later ones pending). Install, build and render run as one pipeline, so a failure inside it is attributed to "Install SDK".

## Keyboard and focus

Every control is a tab stop (disabled ones are skipped), shows a 2 px `focus` (accent) ring while it has keyboard focus, and activates on Enter and Space. Disabled controls ignore clicks and keys and drop to 50 % opacity.

## Telemetry ids

Tests find controls by the bounds the app reports, through the observation telemetry described in [architecture, production probes](architecture.md#13-production-probes). Element ids and labels are chosen by the caller and must not change. A control that is measured passes `.on_bounds(BoundsProbe::new(..).into_bounds_handler())` (`BoundsProbe` is in `desktop/crates/studio-ui/src/probe.rs`), and the callback receives `[left, top, width, height]` on every prepaint. Components never invent ids that a test looks up.

While telemetry is being written, the agent panel's `buttons` telemetry reports the ids listed in `probed_button` in `desktop/crates/studio-ui/src/agent/mod.rs` (the panel's tabs, Apply, Undo, Stop, review, setup and question controls), so the X11 shell test clicks measured bounds instead of fixed window coordinates and the parity tests can check what was laid out.

## Layout and behaviour

Studio's window layout and workflows come from Cap's editor, not from Oleafly; only their look changed.

- Under the 52 px header, the player pane and a 416 px sidebar pane share a row, and the timeline pane spans the full width below them; the panes are flat and meet at hairlines (`shell/layout.rs`). The sidebar has six tabs (`sidebar-tab-Agent`, `sidebar-tab-Project`, `sidebar-tab-Assets`, `sidebar-tab-Style`, `sidebar-tab-Props`, `sidebar-tab-Slides`) in place of Cap's editing tabs. The Slides tab lists the slide stops; on the timeline each stop is a flag on the ruler and a line through the lanes (hollow and dashed for default clip-end stops), dragged along the ruler, added by a double click on it or `K`, removed with Delete. **Present** (`present` in the player toolbar, F5, or "Present from here" in the Slides tab) plays the slides full screen: the player section shows only the frame, letterboxed on black, with a thin strip (slide N of M) for two seconds after each change; the pointer hides after two seconds still. Its keys live in the `Present` context of the presentation stage: →, Space, PageDown and a click play the next slide or finish the one playing, ←/PageUp go back a stop, Home/End go to the first/last slide, a number and Enter go to that slide, Esc leaves and focuses the preview. Agent is selected at start. While another tab is open, the Agent tab shows an accent dot when the agent waits for an answer, and a new permission request selects the Agent tab. The grip on the sidebar's left edge (`agent-resize-handle`) keeps it between 320 and 520 px and leaves the player 480 px; a window too narrow for both shrinks them in proportion (`sidebar_width` in `shell/layout.rs`). The grip above the timeline (`timeline-resize-handle`) drags the split between the row and the timeline with Cap's limits. Neither split is saved between launches.
- The timeline has no header row. Its zoom, Fit, Split and scene-cycle controls sit in the player's transport bar (`timeline-zoom-out`, `timeline-zoom-slider`, `timeline-zoom-in`, `timeline-fit`, `timeline-split`, `timeline-cycle`), and undo and redo are in the header (`timeline-undo`, `timeline-redo`). The transport keeps Cap's three parts: the clock (Geist Mono), playback in the centre, and the tools. Playback stays centred while the tools fit half the bar, so below a 1270 px player the zoom slider and the Split and Build labels go, below 830 px the zoom buttons and Fit, and below 520 px the clock's total (`shell/transport.rs` has the measured widths). Studio-only controls stay: mute, build and cancel preview, and the overflow menu. `FocusPrompt` focuses the composer. Source, checkpoint and job sit in a 28 px status bar at the window's foot (`status-strip`), whose **Details** button (`status-details`) shows the other status lines above it; an error opens them.
- The header is flush with the window's top edge on every system, as in Cap's editor. On Windows it is the title bar: the row around the header is the drag area and Cap's caption buttons (`caption-minimize`, `caption-maximize`, `caption-close`) sit at its right end. Cap presses them through its own Win32 calls; here they are native window control areas, so Windows itself minimizes, maximizes, restores and closes, and offers Snap Layouts over maximize on Windows 11. They live outside the cached header because a cached view does not replay control areas. Like the system's own caption buttons, and unlike every other control, they are not tab stops and have no focus ring: Alt+F4 and Alt+Space reach the same commands from the keyboard. On macOS the header leaves 82 px for the traffic lights, inset to (20, 20), and its empty middle moves the window; Linux keeps the system's title bar, as Cap's editor does. The actions are undo and redo, a divider, Create, Open, Import Rust and Close project, Settings, and Export last with the accent fill, as Cap ends its header with Export. When the window is too narrow for the title, the labelled actions and the window controls, the actions show only their icons (with the label as tooltip); Close project has no icon and keeps its label. Undo, redo and Settings are always icon-only.
- Settings open in a window of their own, as in Cap (782 × 775, at least 780 × 560): the header's Settings button (`settings`) opens it or brings it to the front, and the editor keeps playing beside it. On Windows a 36 px title strip across the window holds the caption buttons; on macOS the traffic lights sit above the nav column. Its close button or Ctrl+W (Cmd+W on macOS) closes it; only closing the main window quits. It does not open while the first-run setup shows, and Run setup again closes it. Its pages hold every setting of the app: **General** (theme, audio output, Run setup again, updates), **Agent** (review policy, permissions, then the agent panel's adapter setup), **SDK** (the managed SDK), **Presets** (the project's style presets) and **Export** (the defaults every export starts from); nav rows `settings-general` to `settings-export`. Per-project parts ask for an open project when none is open. The adapter setup and the preset panel are drawn by their own views, so their fields and telemetry stay theirs; the panel's settings button and `OpenAgentSetup` open the Agent page, the Style tab's **Style presets…** (`open-presets`) the Presets page. The editor keeps only read-outs of these settings: the audio device above the preview and the review policy in the agent's Changes tab. The export panel's choices apply to that export only. On General a `SegmentedControl` (System, Light, Dark; options `theme-0` to `theme-2`) replaces Cap's preview tiles; the choice applies at once and is saved in `app-settings.json`. Cap's nav rows answer only the mouse; here they are tab stops that Enter and Space open.
- Canvas action errors are a dismissible `Notice` over the stage and stay until dismissed; only plain status messages are 3-second toasts.

## Deliberate differences from Oleafly

- **Focus ring kept.** Every control shows a 2 px accent ring under keyboard focus and activates on Enter and Space; Oleafly's global outline and ring suppression is not copied. The Windows caption buttons are the one exception (above).
- **Bundled fonts.** Oleafly names Geist but ships no font files, so its typeface depends on what is installed; Studio bundles the full Geist and Geist Mono builds and applies them at the window roots.
- **No letter spacing.** gpui has no tracking, so `Overline` headings are capitals without Oleafly's wide spacing, at 12 px rather than its 11.
- **Hairlines.** Borders are 0.5 px as in Oleafly; gpui snaps a border to whole device pixels and never below one, so a hairline draws as one device pixel at the usual 100–200 % scales.
- **Translucent control fills** over Studio's opaque surfaces, and a slightly stronger light `text_2` and field outline (`line_strong`), for visible controls and contrast on nested surfaces.
- **52 px header**, against Oleafly's 48 px toolbar, holding Cap's header layout and the Windows caption buttons; the editor splits and their limits stay Studio's.
- **Composer**: the input stops growing at 160 px (8 lines) rather than Oleafly's 224 px or 60 % of the panel, attachment thumbnails stay 56 px (Oleafly's are 24) so a video frame stays legible, and Send and Stop are both shown while the agent runs, with the queue and steer actions in the send menu, instead of Stop alone.
- **Static permission ring.** The ring is the accent, drawn still, not Oleafly's animated six-colour spectrum: the theme has no spectrum colours, animating it would redraw the window every frame while a request is open, and a still ring is what reduced motion asks for anyway. Permission options keep Studio's order and emphasis (refusals first, every allow Primary).
- **Components Oleafly's reproduction does not cover**: `Notice`, `Toast`, `EmptyState`, `FormField`, the timeline, Present and the `motion` preference. Status, timeline, playhead and syntax colours stay Studio's, and only the blue accent is adopted, with no accent picker.

## Licensing

The component kit in `studio-ui` was originally ported from Cap's desktop app, with the permission of Cap's owners; its visual tokens now follow the Oleafly reference above. The four Windows caption glyphs (`caption-*-windows.svg`) come from Cap. Icons are Lucide (ISC); the licence is in `desktop/packaging/sdk/notices/lucide-LICENSE.txt`. Geist and Geist Mono are licensed under the SIL Open Font License 1.1; see `desktop/app/assets/fonts/LICENSE.txt`.
