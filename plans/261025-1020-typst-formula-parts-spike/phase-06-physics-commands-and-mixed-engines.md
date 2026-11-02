---
phase: 6
title: "Physics commands and mixed engines"
status: completed
priority: P2
effort: "2h"
dependencies: [3]
---

# Phase 6: Physics commands and mixed engines

## Goal

Measure two things that decide how Typst would be used in physics videos: which common
`physics` and `siunitx` commands mitex handles, and whether a transform between a TeX
formula and a Typst formula matches any glyph.

## Files

- Modify: `C:\Users\Administrator\fframes-kit\spikes\typst-vs-tex\src\main.rs`. Add a
  `--physics` table and a `--mixed` check.

## Steps

1. **Commands.** Run each of these through mitex (without `eval`, as in phase 3) and Typst,
   and through TeX with `TexTemplate::physics()`: `\dv{f}{x}`, `\pdv{f}{x}`, `\vb{F}`,
   `\vu{n}`, `\abs{x}`, `\norm{v}`, `\ev{A}`, `\ket{\psi}`, `\bra{\phi}`,
   `\braket{\phi}{\psi}`, `\grad f`, `\curl \vb{E}`, `\div \vb{E}`, `\SI{9.81}{m/s^2}`,
   `\si{kg}`, `\qty{3}{m}`, `\unit{m/s}`, `\hbar`, `\partial`, `\nabla`.
2. For each command mitex rejects, try the cheapest fix and record whether it works:
   - a `\newcommand` in the formula, for example `\newcommand{\dv}[2]{\frac{d #1}{d #2}}`;
   - a `CommandSpec` entry with an alias to a small Typst function in `flat.typ`.
3. **Mixed engines.** Typeset `e^{i\pi} = -1` with TeX and `e^{i\pi} + 1 = 0` with Typst.
   Count how many glyph outline keys (the rule of `matching.rs::outline_key`) are equal
   across the two. The first experiment predicts zero.

## Verification

```powershell
cargo run --release -- out --physics --mixed
```

Pass: one table row per command (mitex ok, Typst ok, TeX ok, cheapest fix), and the number
of matching outline keys across engines.

## Risk

Physics video authors may use many more commands than this list. The list covers the common
ones. The report states that the rest needs testing.
