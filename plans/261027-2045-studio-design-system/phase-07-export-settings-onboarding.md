# Phase 7: export, settings window, onboarding

These are the screens that mix token sets the most. They all move to the one palette.

## Export panel (`export/panel.rs`, `export/progress.rs`, `shell/export_panel.rs`)

- Layout: `Card::raised` with padding `space_4` 16 and gap `space_3` 12, replacing p 20 / gap 14
  at :453-456. The title is `TextStyle::Display` (or `Heading`) in `text_1`, not raw 16 px in
  `settings_text`.
- Settings: `Section`s of `Field::row` with `SegmentedControl::new` for target, format, scale and
  fps (ids `export-target`, `export-format`, `export-scale`, `export-fps` and their `-{i}`
  options).
- Buttons: Back and Cancel Neutral, Export Primary (`export-back`, `export-cancel`,
  `export-start`).
- Progress: `Progress::linear`. Done and failed states use `Notice` (Success or Danger) with
  "Show in folder" and "Retry" actions.
- Estimate and message lines: `Small` `text_2` (`export-estimate`, `export-message`).
- Deck export controls keep their `export-deck-*` ids.

## Settings window (`shell/settings_window.rs`, `shell/settings.rs`, `kit/settings_frame.rs`)

- Window chrome: the Windows title strip uses `colors.window` and `colors.line`, like the main
  header (:130-132). `header_h - 10.` at :155 becomes a metric.
- `SettingsFrame`: the nav column on `window`, pages on `card`. Nav rows are `ListRow`s with a
  leading icon, `.selected` `ctl_active` (no settings-accent tint). Ids `settings-general` …
  `settings-export`, `settings-back` stay.
- Pages: `Section::new(..).card()` with `Field::row`s; the page gap is `space_8` 32 (was 28).
  Buttons are `Button` (they already look like the editor, so they now match the page).
  `settings_note` (raw 13 px) becomes `Small` `text_2`.
- General: Theme `SegmentedControl` (`theme-0..2`) plus Motion (`motion-0..2`, phase 1.6).
- Agent: review policy `RadioCards::new` (`review-policy-*`, `permission-policy`). The adapter
  setup keeps its own view but uses `Section` and `Field`.
- SDK: `sdk_controls` (`left_column.rs:452-519`) becomes `Field::row`s with `StatusDot` and
  `Button`s (`install`, `update-check`, `download-update`, `check-update`).
- Presets: the phase 5 panel, now in `Section`s.
- Export defaults: `default-format-*`, `default-scale-*`, `default-fps-*` segments.

## First-run setup (`shell/onboarding.rs`)

- Show it as its own centred `Card::raised` page (max width `METRICS.layout.start_max_w`), with
  steps as `Section`s rather than a `SettingsFrame` nested inside an editor card.
- A step indicator at the top: `Small` `text_3` "Step 2 of 4" plus `Progress::linear`.
- Footer: Skip (Ghost), Back (Neutral), Next (Primary) (`onboarding-skip`, `onboarding-back`,
  `onboarding-next`).
- Permissions use the same `RadioCards::new` as settings.

## `setup_view.rs`

`SetupView` is exported (`lib.rs:12`) but never mounted. Delete it and its export, after
confirming nothing in `desktop/app` constructs it. If it is meant for a later screen, restyle it
with `Section`, `Field` and `StatusDot` instead of the raw `green_11`/`red_11` icons.

## Checks

- The settings segment and nav ids above, and `x11_shell.rs` (`review-policy-0/1`, Ctrl+W
  closing settings).
- The guard allowlist is 0 for `export/`, `settings*.rs` and `onboarding.rs`. `SETTINGS_ACCENT`
  and `settings_*` have no callers, so phase 1's deletions go in.
- Captures: export (settings, running, done, failed); each settings page; each onboarding step;
  light and dark. Compare dark mode especially: the accent and fills must match the editor.
