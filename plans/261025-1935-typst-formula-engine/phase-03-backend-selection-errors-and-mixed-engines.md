---
phase: 3
title: "Backend selection, errors and mixed engines"
status: completed
priority: P1
effort: "5h"
dependencies: [2]
---

# Phase 3: Backend selection, errors and mixed engines

## Goal

`MathTex` typesets with Typst by default. Each formula knows its backend, the errors say
when TeX is needed, the cache holds both backends, and a transform between engines warns.

## Files

- Modify: `fframes-kit/src/latex/engine.rs`:
  - a `backend` field on `TexEngine`, Typst by default, with `with_backend(TexBackend)`;
  - `svg()` dispatches by backend;
  - the wasm `typeset_now` for Typst returns "Typst does not run in the web editor; render
    natively once to fill the cache".
- Modify: `fframes-kit/src/latex/component.rs`:
  - `TexBackend { Typst, Tex }`;
  - `MathTex::backend(TexBackend)`;
  - routing in `typeset`;
  - a `backend` field on `Formula` with `Formula::backend()`.
- Modify: `fframes-kit/src/latex/template.rs`. Add an explicit field, the template's Typst
  prelude, and set it in the kit's own constructors:
  - `default()` sets `Some("")`;
  - `physics()` sets `Some(PHYSICS_PRELUDE)` (filled in by phase 4).

  The public `add_to_preamble` and `with_compiler` set it to `None`. This field is needed
  because `physics()` is itself built with `add_to_preamble`, so the preamble's text cannot
  tell the kit's templates from a user's.
- Modify: `fframes-kit/src/latex/cache.rs`. The Typst key includes `typst` 0.15.1,
  `mitex` 0.2.4 and a kit conversion version; the TeX key is unchanged except for the
  backend.
- Modify: `fframes-kit/src/latex/error.rs`. Add `TexError::NeedsTex { command, formula }`.
- Modify: `fframes-kit/src/latex/matching.rs`. Report the mixed-engine warning.
- Modify: `fframes-kit/src/latex/mod.rs`. Export `TexBackend`.

## Steps

1. **Routing.** The backend of a typeset is fixed before typesetting:
   - `Tex` and `ChemEq` always use TeX.
   - A `MathTex` with a per-formula `backend` uses that backend.
   - Otherwise, a `MathTex` whose template has no Typst prelude (`None`: a user preamble
     or another compiler) uses TeX.
   - Otherwise it uses the engine's backend: Typst by default, with the template's prelude.

   A Typst failure never retries with TeX.
2. **Errors.** Map mitex's `unknown command: \X` and `unknown environment: \X` to
   `TexError::NeedsTex { command: "\\X", formula }`. Its `Display` reads:

   > `\X` is not supported by the Typst backend: typeset this formula with
   > `MathTex::new(..).backend(TexBackend::Tex)`

   Typst compile errors become `TexError::Typst { message, source }`.
3. **Cache.** Native: read the cache file first, otherwise typeset and write it, as TeX
   does today. Wasm: read only. Keys differ per backend, so the same formula is cached
   twice when both backends use it.
4. **Mixed engines.** In `TransformMatchingTex::render`, when `from.backend() !=
   to.backend()`, call `report(Diagnostic::RendererWarning { message })` once per call,
   with this message:

   > TransformMatchingTex: `from` was typeset with TeX and `to` with Typst. Glyphs of
   > different engines never match, so every glyph fades. Typeset both with the same
   > backend.

   The transform still renders (fade out, fade in).
5. **Tests:**
   - The routing table: one test per routing rule.
   - `NeedsTex` names `\ce`, `\begin{tikzpicture}` (environment) and `\div` under Typst.
   - Cache keys differ by backend and match a fixed value, as the existing FNV test does.
   - Mixed engines: a scene rendered through `Previewer::render_inspected` (or the kit's
     existing inspect test helper) returns one `RendererWarning` with the message above.
     Same-engine transforms return none.

## Verification

- `cargo test -p fframes-kit --all-features` passes. TeX-only tests skip without TeX, as
  they do today.
- `cargo clippy --all-targets -- -D warnings` and `just check-wasm` pass.

## Risk

- Routing on "user preamble" needs `TexTemplate` to know which templates are its own. A
  user who builds `TexTemplate::default().add_to_preamble("")` gets TeX. This is documented,
  not special-cased.
- `Formula::from_svg` stays public with TeX as its backend, and the kit sets the backend
  after parsing. A user calling `from_svg` on Typst SVG gets the wrong backend label. Only
  the warning depends on it.
