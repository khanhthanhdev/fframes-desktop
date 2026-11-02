---
phase: 6
title: "Gallery, caches, docs and release checks"
status: completed
priority: P1
effort: "4h"
dependencies: [4, 5]
---

# Phase 6: Gallery, caches, docs and release checks

## Goal

The gallery renders with the Typst default natively and in the editor. The committed caches
and snapshots are regenerated, the docs describe the backends, and the crate packages.

## Files

- Regenerate: `gallery/tex-cache/` (21 files today). It holds Typst entries for the
  `MathTex` formulas and TeX entries for `ChemEq` and TikZ.
- Regenerate: the gallery's e2e snapshots, with `node scripts/e2e.mjs cpu --update`.
- Modify: `gallery/src/scenes/latex.rs`. Do this only where a formula has to stay on TeX,
  marked with `.backend(TexBackend::Tex)` and a comment saying why.
- Modify: `fframes-kit/docs/latex-setup.md`:
  - TeX is needed only for `Tex`, `ChemEq`, TikZ, custom preambles and `NeedsTex`
    formulas;
  - the cache now holds both backends;
  - the Errors section adds `NeedsTex`, `Typst` and the editor cache miss.
- Modify: `fframes-kit/docs/components.md`:
  - `TexBackend`, `MathTex::backend` and `TexEngine::with_backend`;
  - physics support and its exceptions;
  - `match_tokens`;
  - the mixed-engine warning;
  - the `{{+}}` spacing.
- Modify: `README.md` of the kit, if it says formulas need TeX.

## Steps

1. Delete the gallery cache, render the gallery natively once (MiKTeX installed) so both
   backends fill it, and commit the new files. Remove entries nothing uses any more.
2. Check that the editor will find every formula, without a browser:
   - `just check-wasm`;
   - with the committed cache checked out, run `cargo test --release -p components-gallery`
     and render the gallery natively once;
   - `git status --porcelain gallery/tex-cache` must be empty. If it is, no formula the
     gallery builds is missing from the cache that the editor reads.
3. Run `node scripts/e2e.mjs cpu`. Review the changed key frames:
   - Typst moves superscripts and changes `pmatrix` and `cases`, as measured in the first
     experiment; this is expected.
   - Anything else is a regression to fix.

   Then run with `--update` and commit the snapshots. Judge the result by the script's log.
4. `cargo package -p fframes-kit --allow-dirty`. Check that the vendored specs, fonts and
   license files are included and the `.crate` is under crates.io's 10 MB limit. Note the
   size in the commit body.
5. Write the docs changes.
6. Run every check:
   - `cargo test -p fframes-kit --all-features`
   - `cargo test --release -p components-gallery`
   - `cargo clippy --all-targets -- -D warnings`
   - `cargo fmt --all`
7. Commit `git rm -r spikes/` as the branch's last commit.
8. Report back to the user:
   - the branch name and the commits;
   - the clean build time before and after;
   - the package size;
   - that merging and the rev bump in `fframes-studio-kit`, `desktop/fixtures/kit-props` and
     `examples/fframes-studio-intro` are theirs;
   - that the `spike/typst` branch can be deleted after the merge.

## Verification

- All checks in step 6 pass, and the e2e run passes against the new snapshots.
- `inspect` on the gallery reports no errors. The only `RendererWarning`s are ones the
  gallery means to show, and there should be none.

## Risk

- The gallery cache holds entries for both backends. A formula with no committed entry
  fails in the editor. Step 2 catches that, because the same code builds the formulas
  natively and in the editor.
