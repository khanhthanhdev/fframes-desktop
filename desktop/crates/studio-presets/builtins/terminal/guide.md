# Terminal

Phosphor-green monospace on near-black, square corners, thin glowing rules and quick, linear motion.

## Typography

Every text style uses JetBrains Mono (Regular, bundled with the preset (in a project: the top-level `media/preset-fonts-*` file) under the SIL Open Font License). Bind text to `typography.title`, `typography.heading`, `typography.body` and `typography.caption`; never hard-code font sizes or families.

## Layout

Left-aligned like a console. Set everything at `spacing.margin` from the top-left, one line per idea with `spacing.sm` between lines, frame panels in `color.surface` with a `stroke.thin` outline in `color.accent` and almost square `radius.sm` corners, and let `shadow.glow` stand in for a CRT bloom.

## Motion

Typed, not animated: reveal lines with `motion.easing.enter` (linear) over `motion.duration.fast`, stagger characters or lines by `motion.stagger.tight`, and keep exits as short hard cuts with `motion.easing.exit`.

## Rules

- Use semantic tokens (`color.*`, `typography.*`, `spacing.*`, `radius.*`, `stroke.*`, `shadow.*`, `motion.*`) instead of literal values so the project can switch presets.
- Override a token locally only when a scene truly needs it; overrides survive preset reapplication.
- Bundled fonts and licenses travel with the project. Do not substitute system fonts.
