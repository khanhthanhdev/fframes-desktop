---
phase: 5
title: "Token matching option"
status: completed
priority: P3
effort: "4h"
dependencies: [3]
---

# Phase 5: Token matching option

## Goal

`TransformMatchingTex { match_tokens: true, .. }` matches glyphs of Typst formulas by the
LaTeX token they come from. It is off by default.

## Context

- The spike (`spikes/typst-vs-tex/src/token_matching.rs`) marked every leaf token as a part
  in a second Typst pass and keyed glyphs by token text and occurrence (`2#1`).
- That rule was at least as good as hand-written parts on 10 of 11 pairs, and better on
  `x^2 → 2x` (2 moved instead of 1) and `x^2=4 → x=\pm 2` (3 instead of 2).
- It needs spans, so it exists only for Typst formulas.

## Files

- Modify: `fframes-kit/src/latex/svg_to_glyphs.rs`. Read a `data-token` attribute into
  `TexGlyph::token: Option<String>`.
- Modify: `fframes-kit/src/latex/typst/walk.rs`, `convert.rs`. Do the second pass with
  every token marked, then write `data-token` on each glyph.
- Modify: `fframes-kit/src/latex/matching.rs`. Add the `match_tokens: bool` field and the
  token key.

## Steps

1. Port `mark_tokens` and the `WITH_ARGS` list.
2. Typeset the token-marked source as a second pass. When its glyph count equals the main
   pass, write the tokens. When it differs, write no tokens for that formula.
3. Key glyphs with `match_tokens` on: label first, then `{{ }}` part, then token
   (`text#occurrence`), then outline. With it off, matching is unchanged.
4. When `match_tokens` is on and either formula has no tokens (TeX, or a count mismatch),
   report one `RendererWarning`, then fall back to the normal keys:

   > TransformMatchingTex: match_tokens needs Typst formulas with token data; matching by
   > parts and outlines instead.

5. Tests against the spike's 11 pairs, each with `match_tokens: true`:
   - The moved, faded-out and faded-in counts equal the spike's "tokens" column for all 11
     pairs. For example, `x^2 → 2x` gives 2/0/0, `x^2=4 → x=\pm 2` gives 3/1/1 and
     `\sin x → \cos x` gives 1/3/3. Where the parts and tokens columns agree, the numbers
     are the same anyway.
   - With the option off, every pair equals today's counts.

## Verification

- `cargo test -p fframes-kit --all-features` passes.
- The typeset time of the 22 spike formulas stays below 5 ms each with the second pass.
  Time it once and note it in the commit body.

## Risk

- The second pass doubles Typst's work per formula (about 1 ms more). It runs only once,
  in the constructor, and is cached.
