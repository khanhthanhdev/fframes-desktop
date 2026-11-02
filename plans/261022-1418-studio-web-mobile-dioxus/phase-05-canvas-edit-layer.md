---
phase: 5
title: "Canvas edit layer"
status: pending
priority: P1
effort: "2.5w"
dependencies: [1]
---

# Phase 5: Canvas edit layer

## Goal

`edits.json` overrides of annotated objects (transform, opacity, fill and stroke; whole clip or
keyframes) are applied by fframes at run time in the CLI, the Studio worker and export, and the engine
edits them with gestures, undo and safe writes, without rebuilding.

## Context

- Proposal section 5.
- `fframes/src/editor.rs` (`EditorObjectKey`, `render_id`, `from_render_id`)
- `fframes/src/renderer/cli.rs:362-552` and `fframes/src/renderer/preview.rs:365` (timing wiring to mirror)
- `fframes/src/lib.rs:187` (timing exports)
- `desktop/crates/studio-project/src/revision.rs:56,102` (`FileKind::Timing`)
- `desktop/crates/studio-engine/src/timing/`, `controller/timing.rs` (session, gestures, staged writes)
- `fframes-studio-runtime/src/anchors.rs` (literal rewrite for text)
- Spike S5 and S6 results.

## Requirements

- Opt-in: a video without `edits_file` or `RenderOptions::edits` renders byte-identically to today.
- Overrides are deltas composed after the object's own transform; paint overrides replace the paint.
- Transform and opacity go on a new wrapper group (no static hash). A fill or stroke override clears the
  static identity of every descendant it repaints, so no cached picture, path or paint is reused.
- Edits are applied before the worker collects `EditorFrameMetadata`
  (`fframes-studio-runtime/src/preview_worker.rs` around line 160), so bounds include the edit.
- Invalid documents (unknown fields, wrong version, over 512 KiB, NaN values, more than 4096 objects)
  are rejected whole with a reason; keys that match nothing are reported by `inspect`.
- Keyframe frames are scene-relative.
- Text content is never overridden here; it goes to source (step 7).

## Architecture

- `fframes/src/edits.rs`: `EditDoc`, `EditTarget`, `EditValue`, `EditKeyframe`, `EditError`,
  `EditDoc::read_file`, `EditDoc::sample(key, scene_frame) -> Option<EditValue>`, and
  `apply_edits(svgr: &mut Svgr, doc, frame_ctx)` that runs on the `Svgr` nested tree before conversion
  (the patch point spike S5 proves; a converted `usvgr::Tree` cannot be mutated from outside `usvgr`),
  walks nodes, decodes render ids and wraps
  matches in a group (transform about the chosen origin of the node's bounding box, opacity), replaces
  fill/stroke and clears the node's static identity.
- Wiring like timing: `RenderOptions::edits`, `cli::new(..).edits_file(..)`, `Previewer::set_edits`,
  and an `edits_key` beside `timing_key` so preview caches invalidate.
- `fframes-studio-runtime`: a `SetEdits` preview request like `SetTiming`; export reads the session's
  document.
- `studio-project`: `FileKind::Edits` for `edits.json`, excluded from the build revision like timing;
  template `src/main.rs` opts in with `.edits_file("edits.json")`.
- `studio-engine/src/edits/`: an edits session copied from the timing session's shape: begin/update/end
  gesture, set value, add/remove keyframe, clear object, snapshot undo/redo, staged atomic write in
  `.fframes/cache/edits/`, refusal during agent tasks, reload on outside change.
- Text: `studio-engine` gains `rewrite_anchored_literal` on top of `validate_source_anchor`, run as a
  durable edit transaction followed by a build.
- Bake into source: an agent task kind whose brief carries the object's overrides and source anchor;
  after the accepted build the object's entry is removed from `edits.json` in the same transaction.

## Related code files

- Create: `fframes/src/edits.rs`, `fframes/src/renderer/tests/edits_tests.rs`
- Modify: `fframes/src/lib.rs`, `fframes/src/renderer/cli.rs`, `fframes/src/renderer/preview.rs`,
  `fframes/src/renderer/mod.rs` (apply before draw on every backend)
- Modify: `fframes-studio-protocol/src/preview.rs`, `fframes-studio-runtime/src/preview_worker.rs`,
  `fframes-studio-runtime/src/export.rs`
- Modify: `desktop/crates/studio-project/src/revision.rs`, `desktop/crates/studio-project/templates/src/main.rs`,
  `desktop/crates/studio-project/templates/AGENTS.md`
- Create: `desktop/crates/studio-engine/src/edits/{mod.rs,doc.rs,file.rs}`, `src/controller/edits.rs`
- Modify: `desktop/crates/studio-engine/src/engine/backend.rs` (`Command::Edits`)

## Implementation steps

1. `EditDoc` parse, validate and sample, with unit tests (clip scope, keyframe interpolation, wildcard
   repeat, rejection cases).
2. `apply_edits` with snapshot tests through `fframes-test-utils` on the CPU backend and the Skia CPU
   backend, including a node inside a static subtree.
3. CLI, `RenderOptions` and `Previewer` wiring; a CLI test like `timing_file_is_opt_in_and_rejects_malformed_documents`.
4. Worker `SetEdits` and export; protocol golden tests.
5. `FileKind::Edits` and template opt-in; inventory test proving an edit keeps the build revision.
6. Engine edits session with gesture, undo/redo, staged writes, refusal rules and reload, with tests
   mirroring the timing session tests.
7. Anchored literal rewrite for text, with stale-hash refusal test.
8. Bake-into-source task kind.
9. Update the template `AGENTS.md`: annotate new objects with `EditorObjectKey`, respect `edits.json`.

## Todo

- [ ] EditDoc and tests
- [ ] apply_edits on CPU and Skia CPU
- [ ] CLI, RenderOptions, Previewer wiring
- [ ] Worker and export
- [ ] FileKind::Edits and template
- [ ] Engine edits session
- [ ] Text literal rewrite
- [ ] Bake into source
- [ ] Template AGENTS.md

## Success criteria

- `cargo test -p fframes` and `just e2e cpu` pass with unchanged snapshots.
- `cargo test --manifest-path desktop/Cargo.toml -p studio-engine -p studio-project` passes.
- A template project with an `edits.json` renders the same frame through `render`, the worker and export.

## Risks

- Patching the `Svgr` right after `render_frame` covers every backend, because all of them convert that
  `Svgr` before drawing (`render_tree` in `fframes/src/renderer/cpu.rs:93`, `frame_export.rs`,
  `fframes-skia-renderer/src/frame_renderer.rs:131`). The runtime-string `Svgr` used without
  `compile-time-svgtree` (the WASM editor) is not covered; Studio's worker always uses the nested tree.
- Elements without annotations cannot be edited; the docs and UI say so.
- Repeated keys inside one frame are expected; the protocol already caps objects per frame at 4096.
