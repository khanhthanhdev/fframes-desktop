# Pulse

High-contrast dark theme with neon accents, oversized tight type, hard offset shadows and springy, fast motion.

## Typography

Every text style uses DM Sans (Medium, bundled with the preset (in a project: the top-level `media/preset-fonts-*` file) under the SIL Open Font License). Bind text to `typography.title`, `typography.heading`, `typography.body` and `typography.caption`; never hard-code font sizes or families.

## Layout

Centered, full-bleed and dense. Stack the title, heading and body flush at `spacing.margin` from every edge with `spacing.sm` between lines, place content on `color.surface` panels with `radius.lg` and the hard offset `shadow.card`, and use `stroke.thick` underlines in `color.accent`.

## Motion

Snappy, springy cuts: enter with `motion.easing.enter` in `motion.duration.fast`, exit with `motion.easing.exit`, stagger siblings by `motion.stagger.tight`, and pulse the glow with `motion.easing.emphasis`.

## Rules

- Use semantic tokens (`color.*`, `typography.*`, `spacing.*`, `radius.*`, `stroke.*`, `shadow.*`, `motion.*`) instead of literal values so the project can switch presets.
- Override a token locally only when a scene truly needs it; overrides survive preset reapplication.
- Bundled fonts and licenses travel with the project. Do not substitute system fonts.
