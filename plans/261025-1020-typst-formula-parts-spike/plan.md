---
title: "Typst formula parts spike"
description: "Test whether Typst can give fframes-kit the {{ }} parts, labels and glyph matching that TransformMatchingTex needs, and decide whether Typst is worth integrating."
status: completed
priority: P1
effort: 16h
branch: spike/typst
tags: [experimental, kit, formulas, typst]
blockedBy: []
blocks: []
created: 2026-10-25
---

# Typst formula parts spike

## Overview

The first experiment ([report](../reports/experiment-261025-1640-typst-vs-miktex.md)) showed
that Typst typesets 22 of 24 formulas with the same glyphs as MiKTeX, in about 1 ms instead of
about 1 s. But it dropped every `{{ }}` part. `TransformMatchingTex` matches labels first, then
parts, then single glyphs by outline (`fframes-kit/src/latex/matching.rs`), so a video full of
formula transforms needs parts. This spike answers one question: **can Typst give the kit the
same parts as TeX, and maybe better matching, so that videos without `\ce` or TikZ never need
MiKTeX?**

All work stays in the spike crates of the fframes-kit repo (`C:\Users\Administrator\fframes-kit`,
branch `spike/typst`, folder `spikes/`). `fframes-kit/src` is not changed.

## Known facts this plan builds on

- Each Typst `Glyph` carries `span: (Span, u16)`, and `FrameItem::Shape(Shape, Span)` does
  too (typst-library 0.15.1). A span resolves to a byte range of its source.
- Typst's `eval` gives every node one uniform span (`root.synthesize(span)` in
  `typst-eval-0.15.1/src/lib.rs:eval_string`). The spike's current mitex path evaluates
  mitex's output with `eval`, so every glyph points at the `eval` call. **Per-glyph mapping
  needs mitex's output placed in the source directly, without `eval`.**
- mitex's scope is a dictionary built by `process-spec` in `mitex-specs/prelude.typ`. Its
  keys can be turned into top-level `let`s of a generated module.
- `mitex::convert_math(input, Some(CommandSpec))` takes extra commands, whose `alias` names a
  Typst function. That can carry part markers through the conversion.
- `Formula::from_svg(source, svg, parts)` reads parts from `<g id="part-N">` and labels from
  `<g id="label-NAME">` groups. If the spike writes its own SVG with those groups, the kit's
  `Formula`, `FormulaView` and `TransformMatchingTex` are used unchanged.

## Decisions (validated 2026-10-25)

- Kit users write formulas in **LaTeX**. Phase 3 (parts through mitex) is the go/no-go gate.
  Phase 2 (Typst syntax) is a stepping stone, not an acceptable end result.
- Phase 5 (automatic token matching) stays in the spike.
- Phase 6 tests the **physics** and **siunitx** packages.
- The first experiment's code in `spikes/` was committed as `838c345` on `spike/typst`, as
  the baseline. No further commit is needed before phase 1.

## Phases

| # | Phase | Effort | Depends on | Status |
|---|-------|--------|------------|--------|
| 1 | [Frame walker with source ranges](./phase-01-frame-walker-with-source-ranges.md) | 2h | none | Done |
| 2 | [Parts for Typst source](./phase-02-parts-for-typst-source.md) | 2h | 1 | Done |
| 3 | [Parts for LaTeX through mitex](./phase-03-parts-for-latex-through-mitex.md) | 4h | 1, 2 | Done |
| 4 | [TransformMatchingTex end to end](./phase-04-transform-matching-end-to-end.md) | 3h | 3 | Done |
| 5 | [Automatic matching by source token](./phase-05-automatic-matching-by-source-token.md) | 2h | 3 | Done |
| 6 | [Physics commands and mixed engines](./phase-06-physics-commands-and-mixed-engines.md) | 2h | 3 | Done |
| 7 | [Report and decision](./phase-07-report-and-decision.md) | 1h | 4, 5, 6 | Done |

Phases 5 and 6 are independent of 4 and of each other.

## Acceptance criteria

Result: **go**, see the [report](../reports/experiment-261025-1803-typst-formula-parts.md).

- [x] Every `{{ }}` and `\part{name}{..}` case that the kit's TeX tests cover (`parts-sup`,
      `parts-frac`, `parts-left`, `parts-bin`) and the gallery's three part formulas give the
      same glyph count per part with Typst as with TeX.
- [x] Part markers move no Typst glyph by more than 0.01 pt compared with the unmarked formula.
- [x] The gallery's transforms (`identity`, the `journey` sequence, `pythagoras` to `solved`)
      render through `TransformMatchingTex` from Typst formulas, with a contact sheet next to
      the TeX version, and `inspect` reports no errors.
- [x] Token matching, the physics command list and a mixed-engine transform each have a
      measured result, not an opinion.
- [x] A report under `plans/reports/` states go or no-go for integrating Typst into the kit,
      with the numbers behind it.

## Non-goals

- No `FormulaEngine`, cargo feature or public API in `fframes-kit/src`. That is the
  integration plan, written only after a "go".
- No Studio banner, no editor WASM, no `\ce` or TikZ support in Typst.
- No commits to `main` in either repository.

## Constraints

- Windows, CPU only. Run cargo from PowerShell, not Bash. Cargo needs
  `CARGO_NET_GIT_FETCH_WITH_CLI=true`.
- Pin `typst` 0.15.1 and `mitex` 0.2.4 exactly. Vendored mitex specs stay at tag 0.2.6.
- MiKTeX 26.5 is installed per user. The kit's discovery finds it.

## Risks

| Risk | Effect | Mitigation |
|------|--------|------------|
| The marker approach of phase 3 fails | No-go, since users write LaTeX | Phase 3 step 7 records a partial fallback (whole-token parts) for the report |
| A marker function changes math spacing (TeX keeps `{{+}}` as an ordinary symbol) | Glyphs move, transforms look different | Phase 3 measures the shift and compares the spacing classes with TeX |
| mitex output that references scope names fails without `eval` | No parts for LaTeX input | The generated flat module exposes every scope key |
| Glyph order differs between engines | Per-glyph comparison is wrong | Compare per part (count and bounding box), not by index |
| The spike grows into the integration | Time lost on a "no-go" | Non-goals above. Nothing goes into `fframes-kit/src` |

## Rollback

Delete the `spike/typst` branch and the `spikes/` folder. Nothing else changes.
