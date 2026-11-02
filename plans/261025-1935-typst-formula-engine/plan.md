---
title: "Typst formula engine for fframes-kit"
description: "Typeset the kit's LaTeX formulas with Typst by default, keeping {{ }} parts, labels and TransformMatchingTex, with TeX for what Typst cannot do."
status: completed
priority: P1
effort: 26h
branch: feat/typst-formulas
tags: [kit, formulas, typst, latex]
blockedBy: []
blocks: []
created: 2026-10-25
---

# Typst formula engine for fframes-kit

## Overview

The spike ([plan](../261025-1020-typst-formula-parts-spike/plan.md),
[report](../reports/experiment-261025-1803-typst-formula-parts.md)) gave a **go**:
- LaTeX goes through mitex into Typst.
- Parts and labels come out the same as with TeX.
- All five gallery transforms match the same glyphs with either engine.

This plan moves that path into `fframes-kit/src`. Typst becomes the default engine for
`MathTex`, because it needs no TeX installation and takes about 1 ms per formula. TeX stays
for text-mode `Tex`, `ChemEq`, TikZ, custom preambles, and every formula an author sends to
TeX.

All work is in the fframes-kit repository (`C:\Users\Administrator\fframes-kit`), on the
branch `feat/typst-formulas`, with one commit per phase. The branch starts from `spike/typst`
(itself based on `main`'s tip `01f5b32`), so the spike code being ported and measured is on
the branch. Phase 6 removes `spikes/` in its last commit. The user merges the branch and bumps
the kit's git rev in Studio's three consumers.

## Decisions (user, 2026-10-25)

- **Default engine:** the best developer experience wins. Existing videos may change.
  `TexEngine` typesets `MathTex` with Typst unless told otherwise.
- **When Typst cannot typeset a formula:** fail with an error that names the command and
  says how to use TeX. There is no silent fallback to TeX.
- **TeX spacing of operator parts:** change it for everyone. `{{+}}` and `{{=}}` keep
  operator spacing in TeX as they do in Typst.
- **Branch:** `feat/typst-formulas`. The user merges it.

## Architecture decisions (this plan)

- **API names.** The report's `FormulaEngine` becomes:
  - an enum `TexBackend { Typst, Tex }`;
  - `TexEngine::with_backend` to change the engine-wide default;
  - `MathTex::backend` to override it for one formula.

  `TexEngine`, `MathTex`, `Formula` and `TransformMatchingTex` keep their names (Manim's
  names; the source is still LaTeX).
- **Which formulas use which backend.** The choice is made before typesetting and never
  depends on whether Typst succeeds:
  - `Tex` and `ChemEq` always use TeX.
  - A `MathTex` whose template has a user preamble or another compiler uses TeX.
  - `TexTemplate::default()` and `TexTemplate::physics()` use the backend that is set.
  - `physics()` maps to a Typst prelude of `\newcommand` definitions.
- **Errors.**
  - `TexError::NeedsTex { command, formula }`: an unknown command or environment under
    Typst.
  - `TexError::Typst { message, source }`: any other Typst failure.
  - The web editor gets its own "Typst does not run in the web editor" cache-miss text.
- **Mixed engines.** `TransformMatchingTex` reports a `RendererWarning` (see phase 3).
  fframes core cannot be changed, and the kit can only raise warnings, so `inspect` shows
  a warning rather than an error. The spike report has been amended to match.
- **Cache.** Typst results are cached as SVG next to TeX results.
  - The key includes the backend, `typst` and `mitex` versions, and a kit conversion
    version. `CACHE_VERSION` is bumped.
  - The web editor reads committed caches for both backends, which is how formulas are
    "baked" for the editor.
- **Cargo.** Feature `latex` turns on Typst. The `typst` and `mitex` dependencies are
  optional and native only (`cfg(not(target_arch = "wasm32"))`). The clean build time is
  recorded before and after.
- **Spike leftovers.** Phase 6 runs `git rm -r spikes/` as its last commit, so the merged
  branch is clean. The `spike/typst` branch stays until the merge, and then the user deletes
  it.

## Phases

| # | Phase | Effort | Depends on | Status |
|---|-------|--------|------------|--------|
| 1 | [Operator spacing of TeX parts](./phase-01-operator-spacing-of-tex-parts.md) | 3h | none | Done |
| 2 | [Typst backend core](./phase-02-typst-backend-core.md) | 7h | 1 | Done |
| 3 | [Backend selection, errors and mixed engines](./phase-03-backend-selection-errors-and-mixed-engines.md) | 5h | 2 | Done |
| 4 | [Physics prelude](./phase-04-physics-prelude.md) | 3h | 3 | Done |
| 5 | [Token matching option](./phase-05-token-matching-option.md) | 4h | 3 | Done |
| 6 | [Gallery, caches, docs and release checks](./phase-06-gallery-caches-docs-and-release-checks.md) | 4h | 4, 5 | Done |

Phases 4 and 5 are independent of each other.

## Acceptance criteria

- [ ] `MathTex::new(..).typeset(&TexEngine::new(dir))` uses Typst with no TeX installed.
      Parts and labels match TeX's per-part glyph counts on the spike's 13 cases.
- [ ] `\ce`, TikZ, `\div` and an unknown command under Typst give `TexError::NeedsTex`
      with the command named. `Tex`, `ChemEq` and custom preambles still typeset with TeX.
- [ ] `{{+}}`, `{{=}}` and a leading `{{-}}` keep their plain-formula spacing in TeX, and
      the measured gaps are recorded for both backends.
- [ ] A TeX-to-Typst `TransformMatchingTex` reports a `RendererWarning` that `inspect`
      shows.
- [ ] Physics commands typeset with `TexTemplate::physics()` under Typst as measured in the
      spike (19 of 20). `\div` and the `\qty` forms that cannot be supported give
      `NeedsTex`.
- [ ] `TransformMatchingTex { match_tokens: true, .. }` reproduces the spike's token counts
      on its 11 pairs.
- [ ] Every check passes:
  - `cargo test -p fframes-kit --all-features`
  - the gallery tests
  - `cargo clippy --all-targets -- -D warnings`
  - `just check-wasm`
  - `node scripts/e2e.mjs cpu`, with snapshots updated
  - `cargo package -p fframes-kit`
- [ ] The kit docs (`latex-setup.md`, `components.md`) describe the backends, errors,
      cache and physics support.

## Non-goals

- No Typst in the web editor's WASM. The editor reads cached SVGs.
- No `\ce` or TikZ under Typst. No changes to fframes core or Studio.
- No merge to the kit's `main`, and no rev bump in the Studio consumers. Both are the
  user's.
- No change to how TeX typesets, apart from the spacing of operator parts.

## Constraints

- Windows, CPU only. Run cargo and just from PowerShell with Git's `bin` on PATH, never
  the Bash tool. Cargo needs `CARGO_NET_GIT_FETCH_WITH_CLI=true`.
- Pin `typst` =0.15.1 and `mitex` =0.2.4. The vendored mitex specs stay at tag 0.2.6.
- MiKTeX 26.5 is installed per user. It regenerates TeX samples and caches.
- Commits use conventional format, with no plan or phase IDs in code, tests or messages.

## Risks

| Risk | Effect | Mitigation |
|------|--------|------------|
| typst adds minutes to a clean build | Slower first build for every `latex` user | Phase 2 records the time; the report states it |
| Typst output differs visibly from TeX (superscripts, `pmatrix`, `cases`) | Gallery snapshots change | Expected: phase 6 regenerates and reviews them |
| A mitex upgrade changes symbol names or fixes `CommandSpec` | Aliases or sentinels become wrong | Versions are pinned, and the cache key holds them; tests cover the aliases |
| The physics prelude's braces read as `{{ }}` parts | Wrong parts | The prelude is inserted after parts become sentinels (phase 4) |
| Crate size from the fonts (about 2 MB) | Larger crates.io package | Phase 6 checks `cargo package` size and limits |

## Rollback

Delete the branch `feat/typst-formulas`. `main` and the Studio consumers are untouched.
