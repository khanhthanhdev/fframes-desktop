---
phase: 7
title: "Report and decision"
status: completed
priority: P1
effort: "1h"
dependencies: [4, 5, 6]
---

# Phase 7: Report and decision

## Goal

Write the result down and give a go or no-go for integrating Typst into fframes-kit.

## Files

- Create: `C:\Users\Administrator\fframes-desktop\plans\reports\experiment-<date>-typst-formula-parts.md`.
- Copy next to it the contact sheets of phase 4.

## Steps

1. One table per phase with the measured numbers.
2. Decision rules, written before reading the numbers:
   - **Go:** phase 3 passes (parts and labels match TeX for LaTeX input, shift at most
     0.01 pt) and phase 4 passes (same match counts for every gallery transform).
   - **No-go:** phase 3 fails. Kit users write LaTeX, so parts in Typst syntax alone
     (phase 2) are not enough. Typst stays an experiment and TeX stays the only engine. The
     report says how far phase 3 got and what blocked it.
   - Phase 5 decides whether the integration plan includes automatic token matching.
   - Phase 6 decides what the kit says to physics authors: the list of commands that need TeX,
     and whether a mixed-engine transform must be an error.
3. List what the integration plan has to contain on a "go": the `FormulaEngine` API, cargo
   features, the "needs TeX because …" errors, and baked formulas for the editor.
4. Commit the spike code of this plan on `spike/typst` (approved), without the `out/` folders.

## Verification

- The report links resolve, and every number in it comes from a command output of phases 1–6.
- Status of every phase in `plan.md` is updated.
