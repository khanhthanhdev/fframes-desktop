# Fresh

Lifestyle and wellness look: mint background, deep green type, coral accents, pill shapes and gentle, lively motion.

## Typography

Every text style uses Geist (Medium, bundled with the preset (in a project: the top-level `media/preset-fonts-*` file) under the SIL Open Font License). Bind text to `typography.title`, `typography.heading`, `typography.body` and `typography.caption`; never hard-code font sizes or families.

## Layout

Friendly and rounded. Place content on `color.surface` pills and cards with `radius.lg`, tint key words with `color.highlight`, use `color.accent` for the one action, and keep `spacing.margin` around the edges.

## Motion

Lively but calm: rise in with `motion.easing.enter` over `motion.duration.enter`, stagger by `motion.stagger.loose`, and settle with `motion.easing.standard`.

## Rules

- Use semantic tokens (`color.*`, `typography.*`, `spacing.*`, `radius.*`, `stroke.*`, `shadow.*`, `motion.*`) instead of literal values so the project can switch presets.
- Override a token locally only when a scene truly needs it; overrides survive preset reapplication.
- Bundled fonts and licenses travel with the project. Do not substitute system fonts.
