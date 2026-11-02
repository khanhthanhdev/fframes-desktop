---
phase: 3
title: "Parts for LaTeX through mitex"
status: completed
priority: P1
effort: "4h"
dependencies: [1, 2]
---

# Phase 3: Parts for LaTeX through mitex

## Goal

Keep `{{ }}` parts and `\part{name}{..}` labels when the user writes LaTeX, which is how
every kit formula is written today.

## Context

Two things break per-glyph mapping on the current mitex path:
1. **`eval` erases spans.** It gives every node one span. So mitex's output must go into
   `main.typ` directly, and the scope must come from an import.
2. **Ranges point into mitex's output, not into the LaTeX.** So each part must be marked in a
   way that survives the conversion.

## Files

- Modify: `C:\Users\Administrator\fframes-kit\spikes\typst-probe\src\lib.rs`
  - A generated module `/mitex/flat.typ`: `#import "mod.typ": mitex-scope`, then one
    `#let NAME = mitex-scope.at("NAME")` per scope key.
  - `typeset_latex_parts(latex) -> Result<(String, Vec<PartSource>), String>`.
- Create: `C:\Users\Administrator\fframes-kit\spikes\typst-probe\tests\latex_parts.rs`.

## Steps

1. **Scope keys.** Compile a document that writes `mitex-scope.keys()` into a `metadata`
   element, and read it from the document's introspector. Or, simpler, write the keys into a
   text element and take them from the frame. Generate `flat.typ` from the keys. Keys that are
   not valid Typst identifiers keep `mitex-scope.at(..)` access.
2. **Without `eval`.** Write `#import "/mitex/flat.typ": *` and then `$ <mitex output> $` in
   `main.typ`. Re-run the 22 formulas of the first experiment. They must still typeset with
   the same glyph counts.
3. **Markers.** Give mitex a `CommandSpec` with two commands of two arguments each:
   `\kitpart{N}{tex}` with the alias `kitpart`, and `\kitlabel{name}{tex}` with the alias
   `kitlabel`. Define both in `flat.typ` as `#let kitpart(n, body) = body`. Rewrite each
   `{{tex}}` as `\kitpart{N}{tex}` with `split_parts`, and each `\part{name}{..}` as
   `\kitlabel{name}{..}`. Check `\part`'s exact shape first in
   `fframes-kit/src/latex/template.rs`.
4. **Ranges.** Take the `Source` of `main.typ` that was compiled and find each `kitpart(N, ..)`
   and `kitlabel(name, ..)` call in its syntax tree. The second argument's range is the part's
   range, in the same coordinates as the glyph spans. Assign glyphs as in phase 2.
5. **Marker shift.** Typeset every case with and without markers. Measure the largest glyph
   shift, as the kit's `part_markers_keep_glyph_positions` test does for TeX.
6. **Spacing class.** In TeX, `{{+}}` is a braced group, so `+` spaces like an ordinary
   symbol. Check what Typst does with `kitpart0(+)`, and record whether that matches TeX.
7. If markers cannot be made to work, fall back to formulas whose parts are whole tokens:
   map by token text and occurrence. Record this as a partial result.

## Verification

```powershell
cd C:\Users\Administrator\fframes-kit\spikes\typst-probe
cargo test --release --test latex_parts -- --nocapture
```

Pass:
- The 22 formulas typeset without `eval`, with the same glyph counts as with it.
- Every case of phase 2, written in LaTeX, plus `\frac{{{a}}}{b}`, `\left({{x}}\right)` and
  one `\part{lhs}{..}` label, gives the same glyph count per part as TeX.
- The largest marker shift is 0.01 pt or less. `{{+}}` either spaces as in TeX or the
  difference is recorded.

## Risk

A mitex scope handler that inspects its argument (for example `mitexsqrt`) may treat a
`kitpart` call differently from bare content. Test `\sqrt{{{x}}}` explicitly.
