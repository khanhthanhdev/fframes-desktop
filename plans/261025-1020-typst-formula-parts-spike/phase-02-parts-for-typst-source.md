---
phase: 2
title: "Parts for Typst source"
status: completed
priority: P1
effort: "2h"
dependencies: [1]
---

# Phase 2: Parts for Typst source

## Goal

Give `{{ }}` parts to formulas written in Typst syntax, from glyph spans alone, and compare
them with the parts TeX gives for the same formula.

## Context

With Typst input there is no conversion, so span ranges point straight into the text the
user wrote. The kit's `split_parts(source, wrap)` already finds `{{ }}` groups and gives the
rewritten source. Here `wrap` returns the group's text unchanged, without braces, and the
spike records where each part lands in the rewritten source.

## Files

- Modify: `C:\Users\Administrator\fframes-kit\spikes\typst-probe\src\lib.rs`. Add
  `typeset_parts(source) -> Result<(String, Vec<PartSource>), String>`: split, typeset,
  assign every glyph whose range lies inside part `n`'s range to part `n`, then `to_svg`.
- Modify: `C:\Users\Administrator\fframes-kit\spikes\typst-vs-tex\src\main.rs`. Add a
  `--parts` table.

## Steps

1. Record each part's byte range in the rewritten source while `split_parts` runs. The
   `wrap` closure sees the parts in order, so the ranges come from the running output length.
2. A glyph belongs to part `n` when its source range lies inside part `n`'s range. Shapes
   inherit the part of their span the same way.
3. Write Typst-syntax versions of the cases: `{{a}}^2`, `frac({{a}}, b)`,
   `lr(( {{x}} ))`, `{{a}} + {{b}} = {{c}}`, the gallery's `{{e}}^({{i}} {{pi}}) + {{1}} = {{0}}`,
   `{{a}}^2 + {{b}}^2 = {{c}}^2` and `{{a}}^2 = {{c}}^2 - {{b}}^2`.
4. Compare with TeX: for each part, the glyph count, and the part's bounding box centre
   relative to the formula's ink box.

## Verification

```powershell
cargo run --release -- out --parts
```

Pass: every part has the same glyph count as TeX's part. Each part's relative centre is
within 3% of the formula's width and height. No glyph that TeX leaves outside a part is
inside one in Typst.

## Risk

Typst puts the exponent's digit and the base in one `TextItem` with one span per glyph.
Check that `a` in `{{a}}^2` does not take the `2`. If spans are coarser than single
glyphs, record the granularity: it decides phase 5.
