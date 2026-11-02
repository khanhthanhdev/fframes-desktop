# Phase 2: kit core

## Goal

Every kit component has one constructor that takes `&Theme` and reads roles and `METRICS` itself.
There is one button family, and unused variants and components are gone.

## 2.1 One constructor per component

| Component | Today | After |
| --- | --- | --- |
| `Button` | `plain`, `body`, `settings`; variants Primary, Blue, Destructive, Outline, Gray, White, Ghost, Dark; sizes Xs–Lg | `Button::new(theme, id)` with `Emphasis {Neutral, Ghost, Primary, Danger}` and `ButtonSize {Sm 24, Md 28}`; `.label`, `.left_icon`, `.right_icon`, `.pressed`, `.disabled`, `.tooltip`, `.full_width`, `.on_click`, `.on_bounds` |
| `EditorButton` | `plain` with `filled`, `accent`, `destructive`, `danger`, `text` | merged into `Button`: `filled` = Neutral, plain = Ghost, `accent` = Primary, `destructive` = Danger. `danger` and `text` are deleted |
| `IconButton` | `new`, `editor`, `header`, `body`; manual `.size`/`.icon_size`/`.rounded`/`.filled` | `IconButton::new(theme, id, icon)` with the same `Emphasis`, `IconButtonSize {Sm 24, Md 28, Lg 32}`, `.round()` for send and stop, `.label` as the accessible name |
| `Card` | `editor`, `editor_nested`, `settings`, `settings_rows` | `Card::raised(theme)` (`card`, `line`, radius 12) and `Card::nested(theme)` (`card_2`); `.rows()` adds row dividers |
| `Popover` | `plain` (gray_1) | `Popover::new(theme)` on `card` + `Shadow::Pop` |
| `Menu` | `settings`, `plain`, `editor` | `Menu::new(theme, id, state)` |
| `Select` | `settings`, `plain` | `Select::new(theme, id)`; `.ghost()` for the composer model picker |
| `SegmentedControl` | `settings`, `pills`, `editor`, `icons` | `SegmentedControl::new(theme, id)`; options may have icons |
| `Toggle` | `plain`, `settings`; sizes Sm, Md, Lg | `Toggle::new(theme, id, on)`, one size |
| `TextInput` | `settings`, `search`, `bare`, `plain` | `TextInput::new(theme, state)` with `.search()` (leading icon) and `.multiline(max_lines)` |
| `Slider` | `new` + track/fill/thumb at every site | `Slider::new(theme, id, fraction)` with the track, fill and thumb from the theme |
| `TabRail` | `editor(.., panel_bg, ..)` | `TabRail::new(theme, ..)`, reads `card` and `METRICS.layout.tab_bar_h` |
| `RadioCards` | `plain` (Radix blue) | `RadioCards::new(theme, ..)` on `accent`, `ctl_active`, `line` |
| `Notice` | `new` | `Notice::new(theme, id, tone, title)`; built on `Card::raised` plus a tone bar |
| `Tooltip`, `KbdChip`, `Badge`, `StatusDot`, `CollapsibleSection`, `SettingsFrame`, `selectable` | – | take `&Theme`, no colour arguments |

The kit doc table in `kit/mod.rs:9-27` is replaced by one sentence: every component takes the
theme and has one look.

## 2.2 Call-site moves

| Old | New | Sites |
| --- | --- | --- |
| `Button::plain(Gray, Sm)` | `Button::new(..).emphasis(Neutral)` | 10 |
| `Button::plain(Primary)` (gray_12 fill) | `Emphasis::Primary` (accent) | 2. A deliberate look change: primary is accent everywhere, as in the editor header's Export |
| `Button::settings(Gray/Blue)` | Neutral / Primary | `export/panel.rs` |
| `Button::body` | per role | composer |
| `EditorButton::plain()` | `Button::new` | 9 + `shell/mod.rs:597` `action_button` |
| agent `enum Tone` (`agent/mod.rs:210`) | `Emphasis` | about 45 |
| `IconButton::body(..).size(px(24.)).icon_size(..).rounded(..)` | `IconButton::new(..).size(Sm)` | `composer/mod.rs:552-596` |
| `IconButton::new(..)` with 6 overrides (attachment remove) | the `Chip` remove button (phase 3) | `attachments.rs:193` |
| `Slider` + `.track/.fill/.thumb` | `Slider::new(theme, ..)` | `props_panel.rs:345`, `transport.rs:285` |
| `SegmentedControl::settings` | `SegmentedControl::new` | settings ×5, export ×4, agent setup |
| `Toggle::settings` | `Toggle::new` | 1 |
| `Card::settings_rows` / `SettingRow::settings` / `Section::settings` | `Card::nested(..).rows()`, `Field` and `Section` (phase 3) | settings pages |

## 2.3 Kit internals on tokens

- Replace the kit's own `px(<literal>)` (about 330) with `METRICS` fields. Where a value is a
  component's private geometry (e.g. a toggle knob inset), it becomes a named `const` in that file
  derived from `METRICS` where possible.
- Replace `is_dark()` branches in `menu.rs:235`, `segmented.rs:174-227`, `settings_frame.rs:155`
  and `windows_caption.rs:30` with roles. A branch that encodes a real light/dark difference
  becomes a role value in `Colors`.
- Hover, pressed, selected, disabled and focus come from shared helpers in `kit/interact.rs`:
  hover `ctl_hover`, pressed and selected `ctl_active`, disabled `METRICS.opacity.disabled`,
  focus ring `focus` at `size.focus_ring`. Every interactive component uses them.

## 2.4 Delete or park

- Delete the unused variants: `ButtonVariant::{White, Ghost (old), Dark, Blue, Outline}` once
  moved, `ButtonSize::{Md, Lg}` (old), `IconButton::header`, `EditorButton::{danger, text}`,
  `SegmentedControl::{pills, icons}`, `Toggle` sizes, `TextInput::{settings, bare}`,
  `Tooltip::coming_soon`, `titled_group`.
- Wire up instead of deleting: `NumberField` and `ColorPicker` (Props, phase 5),
  `CircularProgress` (export, phase 7), `Field` (phase 3). Delete `SelectionHeader` unless phase 8
  finds a use.
- Remove `#![allow(dead_code)]` from `kit/mod.rs:30` so unused code shows up from now on.

## Ids

Component ids are passed in by callers and do not change. `IconButton`'s accessible label stays.
`EditorButton`'s probe callbacks (`on_bounds`) carry over to `Button`.

## Checks

- Kit unit tests updated for the new roles (`button.rs:~890`, `editor_button.rs:480`,
  `badge.rs:106`, `notice.rs:213`). The behaviour tests (menu, text input, slider, collapsible)
  pass unchanged.
- The kit guard: no colour literals or Radix steps in `kit/`.
- Gallery `kit` page: every component in every state, light and dark, compared with the
  baseline. Expected differences: primary buttons are accent, settings-styled controls match the
  editor.
- `cargo test -p studio-ui`, `x11_shell.rs`, `just clippy`.
