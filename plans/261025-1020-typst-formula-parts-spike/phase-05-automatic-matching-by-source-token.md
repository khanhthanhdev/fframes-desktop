---
phase: 5
title: "Automatic matching by source token"
status: completed
priority: P2
effort: "2h"
dependencies: [3]
---

# Phase 5: Automatic matching by source token

## Goal

Measure whether matching glyphs by the source token they came from (`a`, `\pi`, `2`) makes
transforms as good as hand-written `{{ }}` parts, without the user marking anything.

## Context

Today a glyph outside every part matches by outline only. Two `2`s with the same outline are
ambiguous, and a glyph whose size changes (an exponent becoming a base) does not match. With
spans, each glyph knows its token. Typst can therefore match by token text and occurrence,
which TeX cannot do.

## Files

- Create: `C:\Users\Administrator\fframes-kit\spikes\typst-vs-tex\src\token_matching.rs`.
  A grouping function used for measurement only: it decides which glyphs would move and
  which would fade. It does not change rendering.

## Steps

1. Token key: the LaTeX text of the smallest command or character that the glyph's range
   maps to, plus its occurrence number in the formula (`2#0`, `2#1`). Map ranges back to
   LaTeX through the markers of phase 3, placed around every token instead of every part.
2. Pairs to measure: the `journey` sequence, `pythagoras` to `solved`, the `derivatives`
   rows, and three new pairs that move terms across `=`: `a + b = c` to `a = c - b`,
   `2x + 3 = 7` to `2x = 4`, `x^2 = 4` to `x = \pm 2`.
3. For each pair, count moved, faded and wrongly matched glyphs under three rules: outline
   only (no parts), the hand-written parts with TeX, and token keys with Typst. A glyph is
   wrongly matched when it moves to a different symbol of the target.

## Verification

```powershell
cargo run --release -- out --token-matching
```

Pass: the table exists for every pair. The phase answers yes or no to this question: does
token matching move at least as many glyphs as the hand-written parts, with no more wrong
matches, on these pairs?

## Risk

The occurrence number pairs the first `2` with the first `2`, which is wrong when terms are
reordered. Record such cases. They show where parts stay necessary.
