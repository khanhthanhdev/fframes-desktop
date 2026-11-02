---
phase: 2
title: "Docs generator and scene coverage"
status: completed
priority: P1
effort: "10h"
dependencies: [1]
---

# Phase 2: Docs generator and scene coverage

## Goal
Generate one Starlight page per `##` section of `fframes-kit/docs/*.md`. Each page holds its prose, its code with hidden lines stripped, and a video slot for every gallery scene it names. Every non-bench scene must be named exactly once.

## Files to Create / Modify
All paths are under `C:\Users\Administrator\fframes-kit\`.
- Create: `website/scripts/sync-docs.mjs`: the generator, with a `--check` mode.
- Create: `website/scripts/lib/rustdoc-code.mjs`: hidden-line stripping and fence attributes.
- Create: `website/scripts/lib/rustdoc-code.test.mjs`: `node --test` cases.
- Create: `website/docs-map.json`: the categories, their order, and the mapping from doc file and `##` heading to a category (hand-maintained and small).
- Modify: `fframes-kit/docs/*.md`: add "Gallery scene(s): `X`" lines where a section shows a scene that the docs do not name yet (about 250 scenes). Change no prose meaning and no code.
- Generated (gitignored): `website/src/content/docs/docs/<category>/<slug>.mdx`, `website/src/generated/catalog.json`, `website/src/generated/sidebar.json`.

## Design
- **Fences first.** Tokenise each file into code fences and prose before doing anything else. Heading splits, scene-reference detection, link rewriting and MDX escaping run only on prose. Hidden-line stripping runs only inside `rust` fences. The docs contain fence lines such as `# #[cfg(feature = "latex")]` and `# fn main() {`, which a line-based heading split would break on.
- **Input.** Each docs file is split at `##`. The text before the first `##` becomes the category's index page (`index.mdx`). `###` stays inside its page as a heading and shows in the on-page table of contents.
- **Code.** In ```` ```rust ```` fences (including `rust,ignore`, `no_run`, `should_panic` and `compile_fail`), apply rustdoc's rules. A line whose trimmed form is `#` or starts with `# ` is hidden. `##` at the start becomes `#`. `#[..]` and `#![..]` stay. Fence attributes are dropped and the language is set to `rust`. `ignore` and `compile_fail` blocks get a small "not compiled" or "does not compile" caption.
- **Scenes.** A regex finds `Gallery scenes?:` followed by backticked names, including ones that wrap onto the next line (as in biology.md). After each such paragraph the generator inserts `<SceneVideo name="X" />` (the phase 4 component). The names go into frontmatter `scenes: [...]`.
- **Frontmatter.** `title` is the heading. For a category index, it is the `#` title without its `fframes-kit: ` prefix ("math kit", which becomes "Math kit"). `description` is the first prose sentence, cut to 155 characters at a word boundary. `feature` comes from the file's feature line (``the `math` feature``). `sidebar.order` follows the source order.
- **MDX safety.** Escape `{`, `}`, `<` and `>` in prose outside code, so text like `Vec<Svgr>` in prose does not break MDX.
- **Links between docs files.** Rewrite `components.md#theme`-style links to the generated slugs. An unresolved link fails the build.
- **Catalog.** `catalog.json` holds `{ scenes: [{ name, seconds, page, category }], counts: { components, scenes, kits, pages, compiledExamples } }`. Phase 5 reads the counts.
- **`--check`.** Read `timeline --json` from `cargo run --release -p components-gallery -- timeline --json`, drop the `Bench*` scenes, and fail with a list of names when a scene is on no page, a scene is on two pages, a docs reference names an unknown scene, or a `##` heading has no category in `docs-map.json`.

## Categories (sidebar)
- **Start:** Introduction, Getting started, LaTeX setup.
- **Core:** Basics (One import, The model, Time containers and fades, Animated props, Theme); Text and formulas; Layout and shapes (Coordinate systems, Shapes, Geometry animations, Paths); Motion (Transitions, Effects); Media and overlays (Overlays; Data, audio, decoration and media; Cursors, emoji, sound effects and GIFs); Advanced (Your own component, Tag form, Editable props, A whole video).
- **Kits:** Math, Physics, Chemistry, Computer science, Machine learning, Biology, 3D. Each kit has an index page with a thumbnail grid and one page per `##`.

## Tasks & Steps
1. Write `rustdoc-code.mjs` with tests first. Cases: `# use x;` is hidden; `#` alone is hidden; `##[derive]` becomes `#[derive]`; `#[derive(Debug)]` stays; `#![allow]` stays; indented `    # let _ = a;` is hidden; non-rust fences are untouched. For the splitter: `## Not a heading` and `# fn main() {` inside a fence never start a page, and a `Gallery scene:` inside a fence is not a scene reference.
2. Write `sync-docs.mjs` and `docs-map.json`. Generate, then read a sample of the output: `math/function-plot.mdx`, `components` "Shapes", and `biology` "DnaHelix".
3. Run `--check`. For each scene it lists as missing, find the section whose component the scene shows (the scene names the component, and the scene modules follow the docs' order). Add the reference to that section's existing "Gallery scene" sentence, or as a new last sentence.
4. Run `just test` in the kit after the docs edits, because the doctests must still pass.

## Verification
- `cd C:\Users\Administrator\fframes-kit\website; node --test scripts/lib/` passes.
- `node scripts/sync-docs.mjs --check` exits 0.
- `cd C:\Users\Administrator\fframes-kit; just test` passes.

## Risks
- Mapping about 250 scenes to sections by hand takes most of this phase. `--check` makes it mechanical and catches mistakes.
- MDX escaping bugs show up as build errors, not silent errors. Phase 4's `pnpm build` catches them.
