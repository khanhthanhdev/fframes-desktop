---
phase: 4
title: "Physics prelude"
status: completed
priority: P2
effort: "3h"
dependencies: [3]
---

# Phase 4: Physics prelude

## Goal

`MathTex::new(..).template(TexTemplate::physics())` typesets the common `physics` and
`siunitx` commands under Typst, with the same meaning TeX gives them. The commands that
cannot be matched give `NeedsTex`.

## Context

The spike measured these results:
- Typst handled 6 of 20 commands directly.
- 13 more worked with a `\newcommand` in front of the formula.
- `\div` cannot be redefined in mitex.
- `\braket` from mitex takes one argument (4 glyphs instead of 5), and the definition
  fixes it.
- `\qty` follows the `physics` package, where the delimiter follows the argument's own
  bracket: `\qty(x)` gives parentheses, `\qty[x]` brackets and `\qty{x}` braces. A
  one-argument `\newcommand` reproduces only one of those forms.

## Files

- Modify: `fframes-kit/src/latex/typst/convert.rs`. Add the physics prelude and insert it
  for the physics template.
- Modify: `fframes-kit/src/latex/template.rs`. The physics template carries its Typst
  prelude.
- Modify: `spikes/typst-vs-tex/src/physics.rs` (measurement only):
  - rows for `\qty(x)`, `\qty[x]` and `\qty{x}`;
  - a column that runs the kit's own physics path.

## Steps

1. Ship the spike's 13 definitions plus `\braket` (copy them from `COMMANDS` in
   `spikes/typst-vs-tex/src/physics.rs`).
2. Insert the prelude **after** `{{ }}` groups have become sentinels and before mitex runs.
   The prelude's braces (`\mathbf{#1}}`) are then never read as part delimiters.
3. **`\qty`.**
   - Measure the three forms in TeX with the physics template, counting glyphs and
     comparing outline keys with `\left( \right)`, `\left[ \right]` and `\left\{ \right\}`.
   - Define the form mitex can express as a one-argument command: `\qty{x}`, braces.
   - Before mitex runs, check whether `\qty` is followed by `(` or `[`. If it is, return
     `NeedsTex { command: "\\qty" }`, unless mitex can express that form. Record what the
     measurement shows.
4. `\div` gives `NeedsTex` under the physics template: physics' `\div` is the divergence,
   and mitex reads it as `÷`.
5. **Tests:**
   - Every supported command typesets with the physics template.
   - Glyph counts equal the spike's TeX column: `\dv` 5, `\pdv` 5, `\vb` 1, `\vu` 2, `\abs`
     3, `\norm` 3, `\ev` 3, `\ket` 3, `\bra` 3, `\braket` 5, `\grad f` 2, `\curl \vb{E}` 3,
     `\SI{9.81}{m/s^2}` 8, `\si{kg}` 2, `\unit{m/s}` 3, `\hbar` 1, `\partial` 1, `\nabla` 1.
   - `\div` and the unsupported `\qty` forms give `NeedsTex`.
   - A part inside a physics command (`\dv{{{f}}}{x}`) keeps its part: `[1]`.

## Verification

- `cargo test -p fframes-kit --all-features` passes.
- The spike's `--physics` table, rerun against the kit, shows the kit column equal to the
  TeX column for every supported command.

## Risk

- `\SI` with units mitex does not know (`\meter`, `\per`) is not covered. Those give
  `NeedsTex` through the unknown-command path. The docs list the supported forms.
