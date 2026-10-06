# Quiet Motion

Calm blue-grey palette in regular-weight type with generous whitespace, soft shadows and long, gentle easing.

## Typography

Every text style uses DM Sans (Regular, bundled with the preset (in a project: the top-level `media/preset-fonts-*` file) under the SIL Open Font License). Bind text to `typography.title`, `typography.heading`, `typography.body` and `typography.caption`; never hard-code font sizes or families.

## Layout

Centered with lots of air. Keep at most one idea per frame, center content with at least `spacing.margin` on every side, separate blocks with `spacing.lg`, and float `color.surface` cards with `radius.lg` and the diffuse `shadow.card`.

## Motion

Slow and gentle: crossfade with `motion.easing.standard` over `motion.duration.base`, drift elements into place with `motion.easing.enter` over `motion.duration.slow`, and stagger siblings by `motion.stagger.loose`. Avoid hard cuts.

## Rules

- Use semantic tokens (`color.*`, `typography.*`, `spacing.*`, `radius.*`, `stroke.*`, `shadow.*`, `motion.*`) instead of literal values so the project can switch presets.
- Override a token locally only when a scene truly needs it; overrides survive preset reapplication.
- Bundled fonts and licenses travel with the project. Do not substitute system fonts.
