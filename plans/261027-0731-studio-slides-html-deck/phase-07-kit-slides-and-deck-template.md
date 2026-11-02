---
phase: 7
title: "fframes-kit slides and the slide-deck template"
status: pending
priority: P1
effort: 5d
dependencies: [1, 6]
---

# Phase 7: fframes-kit slides and the slide-deck template

## Goal

Put slide building blocks in fframes-kit, so any fframes video can be a deck, and give Studio
(and `cargo fframes new`) a **Slide deck** template that uses them. Studio keeps only the
deck features (stops, notes, Present, export); the kit owns how slides look and animate.

## Split of responsibilities

| fframes-kit (any fframes video) | Studio (phases 1-4) |
| --- | --- |
| `Slide` scenes with layouts, themed, with build-in animations | Where the deck pauses (`slides.json`), notes, Present mode, HTML export |
| Each slide is an fframes `Scene` with a stable `editor_instance_key` | Clip keys, trims and order in `timeline.json` |
| Bullets reveal one by one; reveal times are known to the slide | Default stops at clip ends; per-bullet stops via phase 8 suggestions or the template's `slides.json` |

## Context

- The kit has no slide concept. Useful pieces: `BulletedList` (draws all items at once,
  `fframes-kit/src/text/bulleted_list.rs:17-88`), `LaggedStart` (`animate/group.rs`),
  `TransitionSeries` (`transitions/series.rs`), `ChapterList`, `Theme` (`theme.rs`), editable
  props behind the `studio` feature (`props.rs`). `Ui::for_scene` builds a scene's `Ui`.
- Studio creates projects from one template (`desktop/crates/studio-project/src/lifecycle.rs:229-296`,
  files under `desktop/crates/studio-project/templates/`). The worker template does not wire
  props hooks; `desktop/fixtures/kit-props/src/main.rs:49-66` shows how
  (`PropsStore::listen`, `hooks()`, `print_export_capabilities_with_props`,
  `run_export_with_props`).
- `cargo-fframes` templates: `cargo-fframes/templates/{lib_single_scene,lib_multi_scene}.rs.tmpl`.
- Phase 6 makes the kit available to managed projects.

## Requirements

### fframes-kit 0.2.0 (`slides` module, no new feature)

- `Slide { key: &'static str, layout: SlideLayout, build: Option<f32>, hold: f32, notes: Option<&str> }`
  implementing fframes `Scene`: `editor_instance_key()` returns `key`; duration = build-in time
  plus `hold` (default 1 s) so the slide's last frame is fully built.
- `SlideLayout`: `Title { title, subtitle }`, `Section { title }`, `Bullets { title, items }`
  (one reveal per item, `stagger` default 0.6 s), `Image { title, image, caption }`,
  `TwoColumns { title, left, right }`, `Code { title, code, language }`, `Quote { text, author }`,
  `Custom(fn(&mut Ui))` for anything else. Text is measured with the existing kit text helpers
  and fits its box (`text_fit`, `text_break_lines`).
- `Slide::reveal_times() -> Vec<f32>` (scene seconds of each bullet's end) and
  `Slide::notes()`: plain data Studio and phase 8 can read.
- `Deck::new(theme, slides)` returns the slides as `fframes::Scenes`, with an optional
  transition between slides (`Fade` default, from `transitions`).
- With the `studio` feature, title, subtitle, items and quote text are editable props
  (`.editable(key)`), so the Props panel edits slide text without code.
- `BulletedList` gains optional `delay`/`stagger` reveal fields; its default look is unchanged.
- Gallery scenes `SlidesTitle`, `SlidesBullets`, `SlidesTwoColumns`; reference
  `fframes-kit/docs/slides.md`; the kit skill gets a "make a deck" section.
- Publish fframes-kit 0.2.0 (and macros if changed) to crates.io; the SDK vendors it (phase 6
  process).

### Studio slide-deck template

- `lifecycle::create` takes a `ProjectTemplate::{Blank, SlideDeck}`; the Create flow in Studio
  asks which one (default Blank, unchanged files).
- `templates-slides/` holds: `Cargo.toml` (fframes, runtime, protocol, `fframes-kit`,
  `fframes-studio-kit` at the SDK's exact versions), `src/video.rs` with a six-slide sample deck
  built from `Deck`, `src/bin/studio_worker.rs` with `PropsStore` hooks wired, `slides.json`
  with a stop at each bullet and `"end"` of each slide plus sample speaker notes, `props.json`
  absent (code values), DM Sans, `AGENTS.md` (version 2 text plus a slides section: add a slide
  by adding a `Slide`, keep keys stable, notes live in `slides.json`).
- Template `slides.json` stops are generated from `reveal_times()` by a test, so the template
  never drifts from the sample deck.

### cargo-fframes

- `cargo fframes new --template slides` writes the same sample deck (without the Studio
  worker), using the published kit version. This touches `cargo-fframes` only, not `fframes/`.

## Files

- fframes-kit repository: `fframes-kit/src/slides/{mod.rs,slide.rs,layout.rs,deck.rs}`,
  `fframes-kit/src/prelude.rs`, `fframes-kit/src/text/bulleted_list.rs`,
  `fframes-kit/docs/slides.md`, `gallery/src/scenes/slides.rs`, `gallery/snapshots/`,
  `skills/fframes-kit/SKILL.md`, `CHANGELOG.md`, crate versions.
- This repository: `desktop/crates/studio-project/src/lifecycle.rs`,
  `desktop/crates/studio-project/templates-slides/**` (create), the Create UI in `studio-ui`
  (template choice), `cargo-fframes/src/**` and `cargo-fframes/templates/lib_slides.rs.tmpl`
  (create), `desktop/scripts/assemble-phase-zero-sdk.py` (kit 0.2.0).

## Tasks & Steps

1. Kit `slides` module, `BulletedList` reveal, gallery scenes and snapshots, docs.
2. Publish fframes-kit 0.2.0; bump `fframes-studio-kit` and the SDK vendor graph.
3. Studio template files and `ProjectTemplate`; the Create choice.
4. Test that generates the template's `slides.json` stops from `reveal_times()`.
5. `cargo fframes new --template slides`.

## Verification

- Kit: `just test`, `just clippy`, `just e2e cpu` with new snapshots; `just check-wasm` (the
  web editor preview of the gallery still builds).
- `strip SlidesBullets -n 8` shows the bullets appearing one by one; `inspect --pops` clean.
- Studio: `lifecycle` unit test creates both templates; ignored real-SDK test creates a Slide
  deck project, builds it, resolves its stops (phase 1) to the expected frames, and exports
  an HTML deck (phase 4) on Windows.
- `cargo fframes new demo --template slides --yes` then `cargo run --release -- strip -n 6`.

## Success criteria

- [ ] A Slide deck project opens in Studio, previews, edits slide text in the Props panel,
      presents with a stop per bullet, and exports an HTML deck with no code changes.
- [ ] The same deck code renders as a normal video with the fframes CLI, outside Studio.
- [ ] The Blank template is byte-identical to today's.

## Risks

- **Layout quality** decides whether people use it; review the gallery frames at 1080p and
  4K before publishing.
- **Text that does not fit** a layout box: `text_fit` truncates with an ellipsis and `inspect`
  warns, never overflows the slide.
