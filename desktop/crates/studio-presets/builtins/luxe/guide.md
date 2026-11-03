# Luxe

Premium brand look: deep black, champagne gold, light airy type with wide tracking, hairlines and slow, elegant fades.

## Typography

Every text style uses DM Sans (Regular, bundled with the preset (in a project: the top-level `media/preset-fonts-*` file) under the SIL Open Font License). Bind text to `typography.title`, `typography.heading`, `typography.body` and `typography.caption`; never hard-code font sizes or families.

## Layout

Sparse and centered. Leave generous `spacing.xl` around one product or line, separate sections with a `stroke.thin` hairline in `color.accent`, keep corners nearly square, and set captions in capitals with `typography.caption`.

## Motion

Slow and elegant: fade and drift in with `motion.easing.enter` over `motion.duration.slow`, stagger by `motion.stagger.loose`, and never bounce.

## Rules

- Use semantic tokens (`color.*`, `typography.*`, `spacing.*`, `radius.*`, `stroke.*`, `shadow.*`, `motion.*`) instead of literal values so the project can switch presets.
- Override a token locally only when a scene truly needs it; overrides survive preset reapplication.
- Bundled fonts and licenses travel with the project. Do not substitute system fonts.
