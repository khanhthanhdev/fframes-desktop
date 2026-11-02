# Phase 5: sidebar tabs

Props, Slides and Presets panels. Project, Assets and Style are in phase 4 with the rest of
`left_column.rs`.

## Props (`props_panel.rs`)

- Rows: `Field::row` replaces the local `field_row` at :230. The reset button uses `Field::reset`
  (`prop-{field}-reset`).
- Numbers: `NumberField` with the slider below it for ranged values (`prop-{field}-input`,
  `prop-{field}-slider`). `Slider::new` with no overrides.
- Choice: `SegmentedControl::new` when there are 4 options or fewer, otherwise `Select`. This
  replaces the wrap-row of accent/filled buttons at :283-306. Option ids stay
  `prop-{field}-{option}`.
- Colour: `ColorPicker` in a `Popover`, opened from the 20 px swatch (swatch radius `item`).
- Booleans: `Toggle::new`. Text: `TextInput::new`.
- Groups: `Section` per group. The empty state is `EmptyState::inline`.
- Delete the private `small()` helper (:224), the `opacity(0.9)` at :523 (use the disabled
  opacity), and the raw slider sizes at :345-347.
- Ids kept: `props-panel`, `prop-{field}` and children, `props-undo`, `props-redo`.

## Slides (`slides_panel.rs`)

- Rows: `ListRow` with a number in `.leading` (`Small` `text_3`, width 20), title and meta,
  `.selected`, `.muted` for default stops, and the "Notes" `Badge` in `.trailing`. Hover is lighter
  than selection, which fixes the review item where two rows looked selected.
- Toolbar: Add stop (Neutral, `Plus`), Remove (Ghost); "Use clip ends" moves into the empty
  state's action, so the toolbar no longer wraps.
- Editor: `Field::stacked` for Title and Speaker notes. The notes input is
  `TextInput::multiline` and the panel scrolls, which fixes the clipped notes box.
- Problems: one `Notice` per problem titled by the stop's name, e.g.
  `"Closing" is not on the timeline`, not by its position in the list. This fixes the numbering
  mismatch. Actions are Neutral `Button` Sm.
- Fix the dark-mode title field (text drawn in placeholder grey). Look at it when moving to
  `TextInput::new`: the input must use `text_1` for content and `text_3` only for the placeholder.
- `Divider::horizontal` replaces `border_t_1` at :565. Delete `small()`. `SLIDES_NOTES_H` moves
  into `METRICS.layout`.
- Ids kept: `slides-panel`, `slide-{n}`, `slides-add`, `slides-remove`,
  `slides-use-clip-ends`, `slides-present-from-here`, `slide-title-input`, `slide-notes-input`,
  `slides-problem-{i}`.

## Presets (`preset_panel.rs`, shown in the settings window)

- Preset rows: `ListRow` with `.selected` on `ctl_active` (today `ctl`), `PRESET_ROW_H` deleted.
- Override rows: `Field::row` with a `TextInput` and a clear `IconButton` (`override-*`,
  `clear-{token}`).
- Actions: one row of `Button`s at their natural width instead of full-width buttons stacked for a
  216 px column. Apply is Primary; Reapply, Reset, Import, Export and CSS are Neutral.
- "Skipped" presets: a `Notice` Warning instead of raw `red_11` text (:379).
- The CSS report `max_h 200` becomes a metric. The page is wrapped in `Section`s so it matches the
  other settings pages.
- Ids kept: `preset-panel`, `preset-{key}`, `preset-apply`, `preset-reapply`, `preset-reset`,
  `preset-import`, `preset-export`, `preset-css`, `override-token`, `override-value`,
  `override-set`, `clear-{token}`, `css-report`.

## Checks

- Unit tests in the three panels pass. Slides tests cover the problem titles.
- The guard allowlist is 0 for `props_panel.rs`, `slides_panel.rs` and `preset_panel.rs`.
- Captures: Props with every control kind; Slides in the three gallery states (no stops, selected
  with notes, problems) and the two marker states; Presets in the settings window; light and dark;
  sidebar at 320 and 520.
