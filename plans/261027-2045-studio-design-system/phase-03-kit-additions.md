# Phase 3: kit additions

## Goal

Add the components screens currently hand-build, so phases 4–8 only compose kit parts.

| Component | Replaces | Spec |
| --- | --- | --- |
| `ListRow` | file row `left_column.rs:101`, recent row `:360`, slide row `slides_panel.rs:304`, preset row `preset_panel.rs:205`, override row `:498`, history row, queue row | `ListRow::new(theme, id)`. Single line `list_row_h` 28, or two lines `list_row_2_h` 40 with `.meta(..)`. Slots: `.leading(icon or element)`, `.title(..)` `Label` `text_1`, `.meta(..)` `Small` `text_3`, `.trailing(..)` (badge, status dot), `.actions(..)` shown on hover in reserved space. px 8, radius `item`, hover `ctl_hover`, `.selected(true)` `ctl_active`, `.disabled(true)` at disabled opacity, `.muted(true)` for defaults (title `text_3`). Focusable, with Enter/Space activation |
| `List` | gaps of 0/2/4 between rows | a column of `ListRow`s with gap 2 and an optional `.empty(EmptyState)` |
| `EmptyState` | the six variants listed in the screen audit | `EmptyState::new(theme, title)` with `.body(..)` and `.action(Button)`. Two sizes: `inline` (title `Label` `text_2`, body `Small` `text_3`, left-aligned, used in panels and tabs) and `centered` (title `Title` `text_1`, body `Body` `text_2`, centred, used on the stage, start screen and timeline) |
| `Section` | `section_title` + column, `Section::settings`, `agent/widgets::section`, raw 16 px titles | `Section::new(theme, title)` with `.description(..)`, `.trailing(..)` and `.children`. Title `Label` `text_2`, gap 8 to content, 24 between sections. `.card()` wraps the content in `Card::nested` (settings pages) |
| `Field` | props `field_row` `props_panel.rs:230`, kit `field_row`, `SettingRow`, slides `small()` labels | `Field::row(theme, label, control)` (label left, control right, `field_row_h` 34) and `Field::stacked(theme, label, control)` (label above, gap 4). `.hint(..)` `Small` `text_3`, `.error(..)` `Small` `danger`, `.reset(on_reset)` shows a reset `IconButton` Sm on hover |
| `Divider` | kit `divider`, `border_t_1` (`slides_panel.rs:565`), vertical 1×16 ×2 (`header.rs:73`, `transport.rs:299`) | `Divider::horizontal(theme)` and `Divider::vertical(theme)` (16 high), `line`, 1 px |
| `ResizeGrip` | `layout.rs:472-539` | `ResizeGrip::new(theme, id, Axis)`: a 4×36 bar, radius 2, `line` → `line_strong` on hover, 14 px hit area; drag callbacks as today; ids `agent-resize-handle`, `timeline-resize-handle` |
| `Progress` | `LinearProgress` `export/progress.rs:115`, present strip `present.rs:465` | `Progress::linear(theme, fraction)` (height `progress_h` 4, radius pill, track `ctl`, fill `accent`; `.on_dark()` uses white 20% / white for present mode) and `Progress::circular` (the existing `CircularProgress`) |
| `Spinner` | todo panel spinner, "Thinking…" badges, running text | 12 px, `accent`, `MOTION.spin_period`; static ring under reduced motion |
| `Chip` | composer context and file chips, suggestion chips | `chip_h` 28, radius `item`, `ctl` fill, px 8, gap 6, 14 icon, `Label`, max width, `.removable(on_remove)` (16 px button, shown on hover, space reserved), `.tone(..)` changes the icon colour only |
| `Toast` | canvas message pill `player.rs:1085` | a transient `Popover` pill: `Small` `text_1`, radius `control`, `Shadow::Pop`, optional action, auto-dismiss after 3 s (kept while hovered). One toast host per window, bottom-centre of the player |

Notes:

- `Notice` stays for persistent messages (with a tone bar). `Toast` is only for transient
  confirmations. Errors are never toasts.
- `Section` and `Field` replace `SettingRow` and `Card::settings_rows`; `SettingsFrame` keeps the
  nav column and page scroll.

## Checks

- Each component has unit tests for its behaviour (ListRow activation and selection, Field reset,
  Toast timer, Progress clamping, Spinner reduced motion) and a gallery entry in every state.
- Gallery captures, light and dark.
