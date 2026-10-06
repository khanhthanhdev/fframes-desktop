# Editorial

Warm paper, ink-black type and a single red accent. Large margins, hairline rules and slow, confident motion.

## Typography

Every text style uses DM Sans (Medium, bundled with the preset (in a project: the top-level `media/preset-fonts-*` file) under the SIL Open Font License). Bind text to `typography.title`, `typography.heading`, `typography.body` and `typography.caption`; never hard-code font sizes or families.

## Layout

Left-aligned, asymmetric composition. Keep text in the left two thirds, anchor titles to `spacing.margin` from the left and top edges, and separate sections with `stroke.divider` hairlines in `color.muted`. Cards use `radius.card` and the soft `shadow.card`.

## Motion

Slow, confident reveals: fade and rise with `motion.easing.enter` over `motion.duration.slow`, stagger siblings by `motion.stagger.loose`. Use `motion.easing.emphasis` once per scene for the accent word.

## Rules

- Use semantic tokens (`color.*`, `typography.*`, `spacing.*`, `radius.*`, `stroke.*`, `shadow.*`, `motion.*`) instead of literal values so the project can switch presets.
- Override a token locally only when a scene truly needs it; overrides survive preset reapplication.
- Bundled fonts and licenses travel with the project. Do not substitute system fonts.
