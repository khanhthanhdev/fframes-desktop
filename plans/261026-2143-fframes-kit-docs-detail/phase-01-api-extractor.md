---
phase: 1
status: completed
---

# Phase 1: API extractor and props tables

## As built

No new Rust crate. `cargo rustdoc -p fframes-kit --all-features --lib -- -Z unstable-options
--output-format json` (stable toolchain, `RUSTC_BOOTSTRAP=1`) writes `target/doc/fframes_kit.json`.
`website/scripts/lib/kit-api.mjs` reads it: every public struct with fields (types without
lifetimes, doc comments), methods, cargo feature (from the gating module's cfg), whether it
implements `Component` / `Themed` / `Props`. Defaults come from the source of the `Themed`
(or `Default`) impl; structs written by `shape!`, `filter_effect!` and similar macros take
them from the invocation's `field: Type = default` entries and the macro definition.
sync-docs caches the result in `src/generated/api.json` (stale when `fframes-kit/src` is newer).

Rustdoc found 332 components (the old regex count of `impl Component for` was 247: it missed
the macro-generated ones). The landing's component count now reads 332.

## Page placement

`assignComponents`: the page whose `##` heading names the component, else the one that
mentions it most (backticked prose x2, struct literals in code x1, overviews at half weight).
`docs-map.json` `"api"` overrides three: `Area`, `RiemannRectangles`, `ParametricFunction`.
A component on no page fails sync-docs.

## Rendering

`## API` after the page body: a theme-defaults sentence, per component `### Name`, its
summary (omitted when the page title is the component), a `Field | Type | Default |
Description` table, methods, `Feature` and a GitHub source link; `## See also` lists pages of
component types used in fields.

## Coverage gate

`sync-docs --check` fails on any undocumented public field of a placed component;
`--api-report <file>` writes them as JSON.

## Validation

- 30 website unit tests pass (`kit-api.test.mjs` and `agent-docs.test.mjs` are new).
- `pnpm build` passes with links valid (112 pages).
