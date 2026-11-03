# Swiss

International style: black type on white, one signal red, square corners, heavy rules and precise, quick in-out motion.

## Typography

Every text style uses DM Sans (Medium, bundled with the preset (in a project: the top-level `media/preset-fonts-*` file) under the SIL Open Font License). Bind text to `typography.title`, `typography.heading`, `typography.body` and `typography.caption`; never hard-code font sizes or families.

## Layout

A strict grid, flush left. Align every block to `spacing.margin`, separate sections with a `stroke.thick` rule in `color.text`, keep corners square (`radius.*` is 0), and use `color.accent` for exactly one element per scene.

## Motion

Precise and quick: slide blocks in along the grid with `motion.easing.enter` over `motion.duration.enter`, stagger by `motion.stagger.tight`, and move between states with `motion.easing.standard`.

## Rules

- Use semantic tokens (`color.*`, `typography.*`, `spacing.*`, `radius.*`, `stroke.*`, `shadow.*`, `motion.*`) instead of literal values so the project can switch presets.
- Override a token locally only when a scene truly needs it; overrides survive preset reapplication.
- Bundled fonts and licenses travel with the project. Do not substitute system fonts.
