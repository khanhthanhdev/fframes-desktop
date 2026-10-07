---
phase: 1
title: "Identity and SVG conversion proof"
status: complete
priority: P2
effort: "5–7 engineer-days"
dependencies: []
---

# Phase 1: Identity and SVG conversion proof

## Goal and context

Prove semantic identity and frame geometry through the actual SVG-to-renderer route, then ship an optional editor contract on the active preview worker. Read [selection architecture](../../docs/desktop/architecture.md#6-selection-and-source-retrieval), [render scout](../reports/scout-261006-2010-m5-render-identities.md) and [baseline](../reports/planning-261006-2010-m5-baseline-and-validation.md).

Legacy `ElementRegistration`, `ElementMetadata` and `SourceLocation` establish explicit-anchor primitives, not reliable animated geometry. The active `preview.rs` contract has no object metadata; its existing scene IDs derive from timeline position. A lexical static_hash is a rendering cache key and must never become an editor identity. Automatic macro locations remain unproven.

## Requirements and proposed contract

- Opt-in registration gives a semantic scene-instance key, component/object key and explicit repeat instance key. Identity is their tuple; the revision and preview identity belong to the containing envelope. Do not derive repeat keys from text, paint order, a cache hash or transient sequence position. Missing explicit keys retain visibly best-effort scene behavior.
- Existing Scene/Video implementations and legacy worker requests still compile and render. Use default optional trait hooks or a wrapper selected by the proof; record the final syntax before updating templates.
- Separate a revision-specific editor index (object ownership/parent/source/tokens/assets) from frame-specific geometry (identity, global transform, visible video-pixel bounds, paint order and support/approximation reason). Registration may omit a source anchor; missing anchors never imply exact source ownership.
- An anchor names a validated project-relative file, symbol and explicit marker/key. Resolve spans and file hashes from immutable source, rather than embedding a changing whole-project revision into project code. Do not expose absolute macro file paths.
- Return frame metadata with or linked to the exact frame response: full PreviewIdentity, frame index, seek serial, request id, output dimensions/scale and editor-index digest. Publish image and metadata together. Thumbnail/tool frames cannot replace the displayed bundle.
- Add optional capability negotiation; M2-only SDKs continue preview/audio and report element selection unavailable. Distinguish an unannotated project from an incompatible/malformed contract. Never require new capabilities globally for all previews.
- Proposed starting ceilings: 4,096 objects/frame, 8,192 registered objects/revision, bounded 256-character keys, 1 MiB metadata and 4 MiB editor index, additionally capped by the negotiated control-message limit. Reject overflow or return explicit partial support; never treat a truncated object set as exhaustive. Record constants and measurement-driven changes at the gate.

## Files and ownership

All paths below are relative to `/root/fframes-desktop`; this stage owns them until its gate passes.

| Action | Files | Purpose |
|---|---|---|
| Modify | `fframes/src/scenes.rs`, `fframes/src/video.rs`, `fframes/src/fframes_context.rs`, `fframes/src/lib.rs`, `fframes/src/svgr.rs`, `fframes/src/renderer/preview.rs` | Optional registration and identity propagation, preserving render APIs |
| Create | `fframes/src/editor.rs`, `fframes/tests/editor_identity.rs` | Runtime editor contract and stable-key tests |
| Modify if proof requires | `svgr-macro/src/nodes_to_svgtree.rs`, `fframes-skia-renderer/src/render/convert.rs` | Carry semantic identity through conversion without changing cache identity |
| Modify | `fframes-studio-protocol/src/preview.rs`, `fframes-studio-runtime/src/preview_worker.rs`, `fframes-studio-runtime/src/lib.rs` | Optional editor index/frame metadata on active worker |
| Create | `fframes-studio-runtime/tests/editor_metadata.rs` | Real conversion/geometry/order test corpus |
| Modify | `desktop/app/src/preview_worker_client.rs`, `desktop/crates/studio-engine/src/preview_state.rs`, `desktop/crates/studio-sdk/src/manifest.rs` | Validate optional responses and retain identity with frame |
| Modify | `desktop/crates/studio-project/templates/src/lib.rs`, `desktop/crates/studio-project/templates/AGENTS.md`, `desktop/crates/studio-project/src/lifecycle.rs` | Generated stable scene/title registration and canonical instructions |
| Modify | `desktop/scripts/assemble-phase-zero-sdk.py`, `desktop/scripts/package-phase-zero.py` | Package actual additive runtime/SDK declarations |
| Modify | `fframes-studio-runtime/tests/preview_worker.rs`, `desktop/app/tests/worker_roundtrip.rs`, `desktop/app/tests/managed_worker.rs` | Compatibility and managed worker gates |

Read template instructions before changes. Re-scout constructors/reexports before adding public fields; new files and hooks above are proposals, not existing symbols.

## Tasks and steps

- [x] Establish the M0–M4 baseline: run current protocol/runtime preview tests, engine scopes and Studio scoped-editing/preview tests; log pre-existing failures without weakening checks.
- [x] Trace actual svgr! tree generation, dynamic node conversion, Skia geometry and paint traversal. Build a corpus containing repeated identical subtrees, two instances of one scene type, nested translated/scaled/rotated groups, outlined text, reordered lists, opacity/clip/mask/filter, crossfade and unsupported shader/media interiors.
- [x] Compare an explicit sidecar keyed to semantic tree identities with embedding semantic SVG node IDs. The pinned converter preserves explicit g/use IDs, so an opaque unique `<g id>` wrapper plus typed sidecar is a promising candidate. Prove both compile-time nested-tree and runtime-string conversion, subtree composition, text flattening and use/marker expansion. Prefer the smallest route that joins to actual converted nodes; avoid relying on IDs a conversion discards. Prove the chosen route on real rendered frames before finalizing syntax. Record failures, preserved identities, video-coordinate bounds, paint order and pixel comparisons in a proof report under this plan's reports directory.
- [x] Implement stable scene-instance hooks and object/component/repeat registration. Detect duplicates per namespace, invalid keys, cycles and missing parents. Explicit annotations can survive text/font/style/transform edits; deleting a key invalidates its selection. Anonymous legacy scenes remain best-effort.
- [x] Build and hash the editor index from the same immutable source/build. Compute geometry from the actual frame traversal with cumulative transforms and composed scene order; include the tree/viewBox-to-video fit before the separate preview raster scaling; classify bounds as approximate or unsupported where masks, clips, filters, opacity or shaders cannot be proven. Do not hand-enter production bounds.
- [x] Add negotiated optional worker messages/fields, strict bounds/identity validation and frame association. Missing optional metadata preserves image/audio playback; malformed metadata disables exact selection visibly. An interrupted metadata request retains the previous complete bundle.
- [x] Update generated project registration/guidance and SDK packaging after the proof; build an actual managed SDK and generated worker without source overlays. Re-run old/unannotated and M2-only worker compatibility tests.

## Verification and acceptance

Run narrowest useful checks first:

```bash
cargo test --locked -p fframes-studio-protocol
cargo test --locked -p fframes-studio-runtime --test preview_worker --test editor_metadata
cargo test --locked -p fframes --test editor_identity
cargo test --locked -p fframes-skia-renderer --test svgr_corpus --test static_cache
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test worker_roundtrip --test preview_coordination
```

After compatible SDK assembly, `SDK_BUNDLE=<verified-sdk> cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test managed_worker -- --ignored` must exercise actual new metadata and legacy compatibility. The placeholder means an SDK verified by the packaging script, not a made-up directory. Record command/toolchain/digest.

Pass only when identities remain stable under edits/reordering with stable repeat keys, duplicated keys fail visibly, transforms/order match the rendered corpus, bounds use full video pixels regardless of preview scale, and old previews still work. Unsupported geometry must be classified, never passed as precise. If neither representation survives conversion, stop at this gate and replan the representation with evidence; do not proceed to fake rectangles.

## Risks, security and rollback

The source/worker is project-provided data and code; metadata is untrusted input to the app. Validate finite values, nonnegative geometry, UTF-8/control characters, relative paths, parent references and all counts before storage. Do not execute project code to build a syntax index or resolve anchors. Old workers may reject new requests: negotiate before sending. Renderer cache changes require pixel/cache regressions. Keep annotations additive so disabling the capability restores scene/range editing; disposable indexes are rebuilt, not migrated into project authority.

Next: Stage 2 only after the representation and real SDK proof pass.
