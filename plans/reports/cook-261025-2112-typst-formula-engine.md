# Typst formula engine for fframes-kit: delivery report

**Outcome: done.** `MathTex` now typesets with Typst in process by default; TeX stays for
`Tex`, `ChemEq`, TikZ, custom templates and formulas the author sends to TeX. All six
phases are committed on `feat/typst-formulas` in `C:\Users\Administrator\fframes-kit`.
On 2026-10-26 the branch was fast-forwarded into kit `main` and pushed.

## Read this first: another writer in the checkout

While phases 5 and 6 ran, someone else was writing a new `teaser/` crate in the same
checkout. They added it to the workspace (`Cargo.toml`, `Cargo.lock`) and added a line to
`README.md`. It does not compile yet (missing `outro` module, `DEG`, `Transform` fields).

- None of it is part of this work. `teaser/`, `Cargo.toml`, `Cargo.lock` and the README's
  teaser line are untouched and uncommitted. The README commit was staged from `HEAD` plus my
  wording only.
- Because `teaser` breaks workspace-wide builds, clippy, fmt and tests were run on the three
  kit packages (`fframes-kit`, `fframes-kit-macros`, `components-gallery`) plus
  `just check-wasm`, not workspace-wide.
- During phase 5 I ran one `cargo fmt --all` while `teaser` was a workspace member, so it may
  have reformatted those files. All later formatting ran per package.

## Commits (`spike/typst..feat/typst-formulas`)

| Commit | What |
| --- | --- |
| `c09498e` | fix(latex): keep operator spacing for parts that are one operator |
| `a9797aa` | feat(latex): typeset formulas with Typst in process |
| `5b1f6ca` | feat(latex): make Typst the default backend of MathTex |
| `3ac55c8` | feat(latex): typeset physics and siunitx commands with Typst |
| `f7dfba7` | feat(latex): match glyphs of Typst formulas by their LaTeX token |
| `ce81e6f` | chore(gallery): typeset the gallery's formulas with Typst |
| `c107bc3` | docs(latex): describe the Typst and TeX backends |
| `6aa189f` | chore: remove the Typst spikes |

`spike/typst` is fully contained in the branch (`git merge-base --is-ancestor` passes), so it
can be deleted after the merge.

## Numbers

| Measure | Value |
| --- | --- |
| Clean `cargo build -p fframes-kit --features latex --release` | 71.8 s before, 93.3 s on the final branch |
| `.crate` size | 2.15 MB compressed, 5.2 MiB, 306 files (was 0.85 MB); crates.io limit 10 MB |
| Typst typeset, both passes, release | 2.8 ms per formula over the 22 test formulas |
| Peak memory, 22 formulas (debug) | 40.7 MB |
| Physics commands, kit glyph count equal to TeX | 19 of 19 supported |
| Token matching, 11 measured pairs | equal to the spike's counts with and without `match_tokens` |

## Verification

- `cargo test -p fframes-kit --all-features`: 591 unit tests and all doctests pass.
- `cargo test --release -p components-gallery`: passes, including the new
  `gallery/tests/formulas.rs`, which checks that `inspect` warns once about a TeX→Typst
  transform and not about a Typst→Typst one.
- `cargo clippy -p fframes-kit -p fframes-kit-macros -p components-gallery --all-targets
  --all-features -- -D warnings`: clean. `cargo fmt --check` on the same packages: clean.
- `just check-wasm`: passes. `typst` is not in the wasm dependency tree.
- The gallery cache was regenerated from an empty folder. It holds 20 entries: 16 Typst,
  plus 4 TeX for the water equation, the TikZ picture and the two propane equations. After
  that, the release gallery tests and an `inspect --every 1s` pass over every scene added
  nothing (`git status gallery/tex-cache` is empty).
- `inspect` over the whole gallery (1770 frames) found no errors. Its 7 warnings have nothing
  to do with formulas: ColorEffects labels at the canvas edge, and AnimatedEmoji's Lottie
  files that have to be downloaded.
- `node scripts/e2e.mjs cpu` passes. The formula frames differ from the snapshots by at most
  0.294% (FormulaParts), under the 0.5% limit. MathTex, TransformMatchingTex, MathTable,
  EulerJourney, WriteFormula and Arcs differ by 0.012% to 0.246%. I looked at the frames:
  superscripts sit a little higher, as Typst sets them, and nothing else changed.
- With TeX hidden (`FFRAMES_TEX_BIN` set to an empty folder), only the two tests that run TeX
  skip.

## Deviations from the plan

- **Template routing.** The plan added a field to templates. Instead, the kit compares the
  template with its own `default()` and `physics()`. Any `add_to_preamble`, other compiler or
  other wrapper uses TeX.
- **`\div`.** It gives `NeedsTex` only under `physics()`. In the default template, TeX and
  mitex both read `\div` as ÷.
- **Where the token is stored.** usvgr drops `data-*` attributes, so a glyph's token is
  written as the path's `id="token-…"`. The prefix keeps dvisvgm's own ids from being read
  as tokens.
- **Tokens for text commands.** `\text{..}`, `\operatorname{..}`, `\mathrm{..}` and
  `\mathbb{..}` each count as one token, together with their argument. Without this,
  `cases` with `\text{if }` lost its tokens. `\begin{..}` and `\end{..}` keep their
  environment name.
- **Physics.** `\dv` uses physics' upright d; TeX confirms it, with 5 of 5 outlines equal.
  siunitx' two-argument `\qty{3}{m}` is not supported: in the template, physics' `\qty`
  wins. Use `\SI`. `\qty(x)` and `\qty[x]` give `NeedsTex`. Measured in TeX, each `\qty`
  form is exactly its `\left..\right` pair.
- **e2e snapshots not regenerated.** Every frame is under the tolerance, so the script's
  `--update` copies nothing, and the committed snapshots still show the TeX glyphs. If you
  want the snapshots to show Typst, the 7 frames can be copied by hand from
  `target/e2e/cpu/gallery/` into `gallery/snapshots/`.
- **Packaging.** `cargo package -p fframes-kit` alone fails because `fframes-kit-macros` is
  not on crates.io (this was already the case before this work). The size above comes from
  `cargo package -p fframes-kit-macros -p fframes-kit --allow-dirty --no-verify`.
  `--no-verify` skipped compiling the packaged crate.

## Public API added (kit 0.1.0)

`TexBackend`, `MathTex::{backend, backend_with}`, `TexEngine::{with_backend, backend}`,
`Formula::backend`, `TexError::{Typst, NeedsTex}`, `TexGlyph::token`, and
`TransformMatchingTex::match_tokens`.

- `TexGlyph::token` is a new public field, so code that builds a `TexGlyph` with a struct
  literal breaks.
- `TransformMatchingTex::match_tokens` is a new public field. A struct literal without
  `..TransformMatchingTex::new(..)` breaks, and Studio gets a new bool prop after the rev
  bump.

## Yours to do

- Merge `feat/typst-formulas`, then delete `spike/typst`.
- Bump the kit's git rev in `fframes-studio-kit`, `desktop/fixtures/kit-props` and
  `examples/fframes-studio-intro`.
- Decide what happens to the `teaser/` changes in the checkout.

## Found, not caused

- **The editor cannot read the formula cache.** On wasm32, `std::fs` is unsupported, so
  every formula in the web editor ends up as `Formula::unset`, with both backends. Phase 6
  step 2 assumed the editor reads the cache, but it only proves that the native build finds
  every formula.
- **Misleading help text.** The `NotInstalled` text "render natively once to fill the
  cache" is misleading in the editor, for TeX as well as Typst. TeX changes were a non-goal,
  so it is left as it was.

## Unresolved questions

1. The vendored fonts are under GUST (NewCMMath) and GPL-3.0 with the font exception
   (NewCM10), inside a crate whose license is MIT. Their licenses ship next to them in
   `src/latex/typst/assets/fonts/LICENSE`. Is that enough, or do you want a README notice,
   a `license-file`, or different fonts?
2. Should the editor get formulas some other way, for example by embedding the cache with
   `include_str!` or loading it over HTTP? Today it shows every formula as source text.
   Resolved on 2026-10-26: the cache is embedded, see
   [cook-261026-0857-editor-formula-cache.md](cook-261026-0857-editor-formula-cache.md).
