---
phase: 1
title: "Operator spacing of TeX parts"
status: completed
priority: P1
effort: "3h"
dependencies: []
---

# Phase 1: Operator spacing of TeX parts

## Goal

A part that is a single binary operator or relation keeps the spacing it has without the
part, in TeX as in Typst. The doc comment of `TexTemplate::physics()` is corrected.

## Context

- `PART_MACROS` (`fframes-kit/src/latex/template.rs:51`) wraps every part in braces. In TeX
  a braced `+` is an ordinary symbol: `a {{+}} b` has gaps of 0.863 and 1.034 pt instead of
  3.077 and 3.248 pt.
- The spike measured that `a \mathbin{{{+}}} b` and `a \mathrel{{{=}}} b` give exactly the
  plain gaps and keep the part (spike commit `30f811e`).
- The braces stay for all other parts, because `{{a}}^2` needs them for the exponent.

## Files

- Modify: `fframes-kit/src/latex/component.rs`. `mark_part` picks a macro by the part's
  class.
- Modify: `fframes-kit/src/latex/template.rs`:
  - add `\fframesPartBin` and `\fframesPartRel` to `PART_MACROS`, as `\mathbin{..}` and
    `\mathrel{..}` around the two specials;
  - fix the doc comment of `physics()`.
- Modify: `fframes-kit/src/latex/cache.rs`. Bump `CACHE_VERSION` to `fframes-kit-tex-2`.
- Regenerate: `fframes-kit/src/latex/samples/parts-bin.svg` with MiKTeX, and any other
  sample whose document changes.

## Steps

1. Create the branch `feat/typst-formulas` from `spike/typst` in the kit repository, so
   `spikes/` is available for the measurements and the port.
2. Classify a part by its trimmed source. It is a binary operator or a relation when the
   source is exactly one item of these lists (plain TeX's math classes):
   - **Binary:** `+ - * \pm \mp \times \div \cdot \ast \star \circ \bullet \cap \cup
     \setminus \wedge \land \vee \lor \oplus \ominus \otimes \oslash \odot`
   - **Relation:** `= < > \le \leq \ge \geq \ne \neq \approx \equiv \sim \simeq \cong
     \propto \in \notin \ni \subset \subseteq \supset \supseteq \to \rightarrow \leftarrow
     \leftrightarrow \Rightarrow \Leftarrow \Leftrightarrow \implies \iff \mapsto \gets
     \parallel \perp \mid`
   - Anything else keeps `\fframesPart`.
3. Unit test for `mark_part`: one case per class, including surrounding spaces such as
   `{{ + }}`, and a non-operator part.
4. Physics doc comment. `physics` and `siunitx` both define `\qty`, and the template loads
   `physics` first, so `\qty` has the `physics` meaning (its delimiter follows the
   argument's own bracket). Units are `\SI`, `\si` and `\unit`.
5. Measure in the spike (`typst-vs-tex --latex-parts`, the operator table), with TeX now
   running the kit's new macros:
   - TeX gaps of `a {{+}} b` and `a {{=}} b` equal those of `a + b` and `a = b`;
   - a new row `{{-}}1` with `-1` beside it, in both engines. TeX demotes a leading binary
     operator to ordinary, and the row shows whether Typst does the same.

   Record the numbers in the commit message body.
6. Regenerate the TeX samples whose documents changed, and update the `parts-bin` test so
   its bbox assertions follow the new spacing.

## Verification

- `cargo test -p fframes-kit --all-features` passes (with MiKTeX installed).
- `cargo clippy --all-targets -- -D warnings` passes.
- The spike table shows TeX gaps equal to the plain gaps for `{{+}}` and `{{=}}`. The
  `{{-}}1` row is recorded for both engines.

## Risk

- A part such as `{{-}}` before a number is unary. `\mathbin` lets TeX demote it to ordinary
  as it does for a plain `-`, so the spacing follows TeX's own rule. The `{{-}}1` row
  confirms it.
- Every TeX document that has an operator part changes, so its cache entry is regenerated.
  The gallery cache is regenerated in phase 6.
