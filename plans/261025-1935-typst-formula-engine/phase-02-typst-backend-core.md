---
phase: 2
title: "Typst backend core"
status: completed
priority: P1
effort: "7h"
dependencies: [1]
---

# Phase 2: Typst backend core

## Goal

The kit typesets a LaTeX formula with Typst into the same SVG shape that `Formula::from_svg`
reads, with `{{ }}` parts and `\part{name}{..}` labels, natively and behind the `latex`
feature.

## Context

The spike code to port:
- `spikes/typst-probe/src/lib.rs` (620 lines):
  - the World;
  - the flat module of mitex scope keys, with the symbol aliases and the
    `kitpart`/`kitlabel` markers;
  - the frame walker with spans and `Tag` tracking;
  - the operator search by text;
  - `assign_parts`;
  - `to_svg`.
- `spikes/typst-vs-tex/src/latex.rs` (156 lines): the sentinels U+E000/E002/E001, the mitex
  call, and rewriting the sentinels into `kitpart(#N, ..)` and `kitlabel(#K, ..)`.

Facts the port relies on:
- `eval` erases spans, so mitex output goes into `main.typ` directly.
- mitex 0.2.4 ignores a custom `CommandSpec` while converting, so parts are marked with
  sentinels.
- Typst 0.15 has no `diff`, `planck.reduce`, `angle.l` or `angle.r`, so the flat module
  aliases them.

## Files

- Create: `fframes-kit/src/latex/typst/mod.rs`. `pub(crate) fn typeset(latex: &str) ->
  Result<String, TexError>` returns the SVG with part and label groups.
- Create: `fframes-kit/src/latex/typst/world.rs`. The World, fonts, vendored specs and flat
  module.
- Create: `fframes-kit/src/latex/typst/convert.rs`. Sentinels, mitex, `kitpart`/`kitlabel`.
- Create: `fframes-kit/src/latex/typst/walk.rs`. The frame walker, part assignment and SVG
  output.
- Create: `fframes-kit/src/latex/typst/assets/`, holding:
  - `NewCMMath-Regular.otf` and `NewCM10-Regular.otf` with their OFL license;
  - `mitex-specs/` (tag 0.2.6) with mitex's Apache-2.0 license and a short NOTICE.
- Modify: `fframes-kit/Cargo.toml`. Native-only optional `typst`, `typst-layout`,
  `ttf-parser` and `mitex`, turned on by `latex`; `typst-svg` as a native dev-dependency.
- Modify: `fframes-kit/src/latex/mod.rs`, `error.rs`. Add the module and the
  `TexError::Typst` variant (`NeedsTex` comes in phase 3).

## Steps

1. Record a clean `cargo build -p fframes-kit --features latex --release` time on `main`
   before the change.
2. Port the probe and `latex.rs` into the four files. Keep the behaviour, and drop
   everything only the spike's measurements used: walker comparisons, eval paths and
   tables.
3. Gate the module with `#[cfg(not(target_arch = "wasm32"))]` inside the `latex` module.
   Put the dependencies under
   `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` as optional, and turn them on
   from `latex = [..]` with `dep:`.
4. Keep the World in a `OnceLock`: fonts, specs and the flat module are built once
   (the spike measured 26 ms on first use).
5. Tests (`#[cfg(test)]` in the new module), which need no TeX:
   - The walker places glyphs and shapes like `typst_svg::svg` on the 22 spike formulas,
     within 0.01 pt.
   - The 13 spike cases have these glyph counts per part and label:
     - `[1]` for each single-letter part;
     - `[3]` for `{{\sin}} x`, `{{\frac{a}{b}}} + c` and `{{\sqrt{x}}} + 1`;
     - `lhs: 5` for the label.

     These are the spike's measured values.
   - Markers move no glyph by more than 0.01 pt compared with the unmarked formula.
   - `\hbar`, `\partial` and `\left\langle x \right\rangle` typeset (the symbol aliases).
6. Record the clean build time again and put both times in the commit message body.

## Verification

- `cargo test -p fframes-kit --features latex` passes without TeX: point the variable named
  by `latex::TEX_BIN_VAR` at an empty folder, and check that only the TeX tests skip.
- `cargo clippy --all-targets -- -D warnings` passes.
- `just check-wasm` passes. The gallery bridge builds the kit for wasm32 without typst.

## Risk

- Porting may drop a fallback that the spike needed for one case (the fraction bar, the
  radical, an operator inside a part). The 13-case test covers each of them.
- Typst's comemo caches grow with distinct formulas. The World lives as long as the
  process, as in the spike. Memory is measured on the 22 formulas and noted if it is above
  50 MB.
