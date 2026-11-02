---
phase: 4
title: "TransformMatchingTex end to end"
status: completed
priority: P1
effort: "3h"
dependencies: [3]
---

# Phase 4: TransformMatchingTex end to end

## Goal

Render the gallery's real formula transforms from Typst formulas through the kit's unchanged
`FormulaView` and `TransformMatchingTex`, next to the TeX versions.

## Context

The gallery's transform scenes are in `C:\Users\Administrator\fframes-kit\gallery\src\scenes\latex.rs`:
- `journey`: four formulas played one after another.
- `identity`: Euler's identity with five parts.
- `pythagoras` to `solved`: parts move across the equals sign.

## Files

- Create: `C:\Users\Administrator\fframes-kit\spikes\typst-transforms\` (own `[workspace]`).
  `Cargo.toml` copies the gallery's `fframes` dependency and features verbatim from
  `C:\Users\Administrator\fframes-kit\gallery\Cargo.toml`, because that set already builds on
  this machine with the media and ffmpeg stack. It adds the kit by path with `latex`, and
  `typst-probe` by path.
- Create: `C:\Users\Administrator\fframes-kit\spikes\typst-transforms\src\main.rs`. A video
  with one scene per transform and engine (TeX, Typst), the same `FormulaView` settings as
  the gallery, and `fframes::cli`.

## Steps

1. Typeset each formula with both engines in the constructor: TeX with `MathTex` and a
   temporary cache, Typst with `typeset_latex_parts` and then `Formula::from_svg`.
2. One scene per transform and engine, each 3 s long, running `TransformMatchingTex` from
   0.5 s to 2.5 s.
3. Render the contact sheets and check the frames on the CPU backend.
4. Compare the match structure without pixels: per transform, the number of glyphs in matched
   label and part groups, and the number that fade in or out. Count these from
   `Formula::glyphs()` and `parts()` in the spike, with the grouping rules of
   `matching.rs::groups`.

## Verification

```powershell
cd C:\Users\Administrator\fframes-kit\spikes\typst-transforms
$R = ".\target\release\typst-transforms.exe"
cargo build --release
& $R timeline
& $R inspect
& $R strip -n 8          # once per scene: strip <Scene> -n 8
```

Pass:
- `inspect` exits 0.
- For every transform, the moved and faded glyph counts are the same for TeX and Typst.
- The strips show the same motion. Judge them by eye and put them in the report.

## Risk

Small differences in ink size (5–9% in height in the first experiment) make the Typst
transforms land a little differently. That is expected. The match counts are the criterion.
