---
title: "Revise the fframes-kit plan: LaTeX, no Typst, simple first"
date: 2026-10-23
summary: Formulas now use a user-installed TeX (MiKTeX) through latex and dvisvgm; Typst and CeTZ dropped; basic transitions moved early
---

# Revise the fframes-kit plan: LaTeX, no Typst, simple first

Revised `plans/261023-1255-components-library/` on user direction. Formulas use full LaTeX
from a TeX installation the user provides (MiKTeX on Windows, Ubuntu and macOS; TeX Live works
too) through Manim's pipeline: `latex` to DVI, `dvisvgm --no-fonts` to SVG, parsed once with
`usvgr` into glyph paths, cached as SVG by input hash. A committed `tex-cache/` lets machines
without TeX render. Typst, MiTeX, the vendored Typst packages and the CeTZ crate are gone;
TikZ, chemfig and circuitikz cover the static diagrams instead.

New order, simple first: core, shapes, basic transitions (Fade, Slide, Wipe, linear and spring
timing, SceneFade), general components, LaTeX, math kit (plain-text labels until LaTeX
lands), component tags, Studio props panel, subject kits, effects. The cache path is pinned to
`CARGO_MANIFEST_DIR` because the e2e harness runs binaries from the example directory.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
