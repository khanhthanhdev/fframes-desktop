# Promo

Sale and promo look: loud yellow, hard black type, red price tags, chunky strokes and punchy spring motion.

## Typography

Every text style uses Geist (SemiBold, bundled with the preset (in a project: the top-level `media/preset-fonts-*` file) under the SIL Open Font License). Bind text to `typography.title`, `typography.heading`, `typography.body` and `typography.caption`; never hard-code font sizes or families.

## Layout

Loud and stacked. Fill the frame with `color.background`, set the offer in `typography.title` edge to edge at `spacing.margin`, put prices on `color.accent` tags with `radius.sm` and the hard `shadow.card`, and outline cards with `stroke.thick` in `color.text`.

## Motion

Punchy: slam elements in with the springy `motion.easing.enter` over `motion.duration.fast`, stagger by `motion.stagger.tight`, and wobble the price tag with `motion.easing.emphasis`.

## Rules

- Use semantic tokens (`color.*`, `typography.*`, `spacing.*`, `radius.*`, `stroke.*`, `shadow.*`, `motion.*`) instead of literal values so the project can switch presets.
- Override a token locally only when a scene truly needs it; overrides survive preset reapplication.
- Bundled fonts and licenses travel with the project. Do not substitute system fonts.
