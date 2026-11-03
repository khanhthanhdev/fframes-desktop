# Launch

Product-launch look: crisp white, electric indigo, confident semibold type, soft cards and smooth eased motion.

## Typography

Every text style uses Geist (SemiBold, bundled with the preset (in a project: the top-level `media/preset-fonts-*` file) under the SIL Open Font License). Bind text to `typography.title`, `typography.heading`, `typography.body` and `typography.caption`; never hard-code font sizes or families.

## Layout

Product-first. Put one headline and one supporting line on `color.background`, show the product on a `color.surface` card with `radius.lg` and `shadow.card`, and give the call to action a `color.accent` pill (`radius.lg`) with `color.surface` text. Keep `spacing.margin` around every edge.

## Motion

Smooth and assured: enter with `motion.easing.enter` over `motion.duration.enter`, stagger the headline, line and button by `motion.stagger.tight`, and pop the call to action with `motion.easing.emphasis`.

## Rules

- Use semantic tokens (`color.*`, `typography.*`, `spacing.*`, `radius.*`, `stroke.*`, `shadow.*`, `motion.*`) instead of literal values so the project can switch presets.
- Override a token locally only when a scene truly needs it; overrides survive preset reapplication.
- Bundled fonts and licenses travel with the project. Do not substitute system fonts.
