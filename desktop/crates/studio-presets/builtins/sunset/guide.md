# Sunset

Warm coral and gold on deep plum, generous rounded cards, soft glows and a gentle overshoot on entrances.

## Typography

Every text style uses DM Sans (Medium, bundled with the preset (in a project: the top-level `media/preset-fonts-*` file) under the SIL Open Font License). Bind text to `typography.title`, `typography.heading`, `typography.body` and `typography.caption`; never hard-code font sizes or families.

## Layout

Centered and roomy. Float content on `color.surface` cards with `radius.lg` and the soft `shadow.card`, keep `spacing.margin` around every edge, use `color.highlight` for one key word per scene and `color.accent` for rules drawn with `stroke.regular`.

## Motion

Warm and bouncy: enter with `motion.easing.enter`, which overshoots slightly, over `motion.duration.enter`, stagger siblings by `motion.stagger.loose`, and settle with `motion.easing.standard`.

## Rules

- Use semantic tokens (`color.*`, `typography.*`, `spacing.*`, `radius.*`, `stroke.*`, `shadow.*`, `motion.*`) instead of literal values so the project can switch presets.
- Override a token locally only when a scene truly needs it; overrides survive preset reapplication.
- Bundled fonts and licenses travel with the project. Do not substitute system fonts.
