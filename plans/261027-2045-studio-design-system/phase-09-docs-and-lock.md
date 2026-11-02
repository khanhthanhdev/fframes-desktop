# Phase 9: docs, gallery completeness, guard at zero

## Docs

Rewrite `docs/desktop/design-system.md` to match the code:

- **Principles:** Cap's editor style everywhere, one palette, one kit, one constructor each.
- **Tokens:** the `Colors` roles table, `METRICS` groups, `TextStyle`, `Shadow`, `MOTION` and
  `UiPrefs`.
- **Components:** one line each, with which role it serves and its ids convention.
- **Recipes:** the patterns screens repeat:
  - a panel is `Section`s of `Field`s;
  - a list is `List` of `ListRow`;
  - an empty panel is `EmptyState::inline`;
  - a blocking message is `Notice`;
  - a transient confirmation is `Toast`;
  - one Primary button per view at most.
- **States:** hover, pressed, selected, disabled, focus and reduced motion.
- **How to add a value:** add the role to the theme, never at the call site; the guard explains
  the same.
- **Deliberate differences from Cap:** focus ring and keyboard activation, `Notice`, `Badge`,
  `StatusDot`, `Toast`, `ListRow`, `EmptyState`, `Field`, `Section`, the motion preference.

Update `AGENTS.md` only if its Studio pointer needs the new doc name (it links to
`docs/desktop/README.md`, which links the design doc).

## Gallery

The `kit` page shows every component in every state:

- buttons: 4 emphases × 2 sizes × rest, hover, pressed, disabled, with icons; icon buttons in 3
  sizes and round;
- controls: toggle, segmented (with disabled options), select and menu open, slider, text input
  (empty, filled, focused, error, multiline), number field, colour picker, radio cards;
- feedback: badges and status dots in 5 tones; notices in 4 tones with actions and details;
  toast; progress (linear, circular); spinner;
- surfaces and layout: `Card` raised and nested; `Popover`; `Section`; `Field` row and stacked
  with hint, error and reset; `ListRow` (single, two-line, selected, muted, with actions, disabled)
  and a `List` with an empty state; `EmptyState` inline and centred; `Divider` both axes;
  `ResizeGrip`; `Chip` (removable, toned); `TabRail`;
- text and keys: tooltip with `KbdChip`; selectable text.

The other pages (shell, sidebar, agent, export, settings, onboarding, timeline) come from phase 0.

## Lock

- The style guard allowlist is empty except the documented `const`-only exceptions (timeline
  painters, agent diff painter, syntax palette).
- The theme's raw Radix module is private.
- `#![allow(dead_code)]` is gone from the kit (phase 2), so unused components show up as warnings,
  which are errors under `just clippy`.

## Final review

Capture every page in both appearances, put it next to the phase 0 baseline and Cap's editor
screenshots, and check the success criteria in `plan.md`.

## Checks

- `cargo test` for `studio-theme` and `studio-ui`, `desktop/app/tests/x11_shell.rs`,
  `just clippy`.
- Full capture set reviewed.
