---
phase: 2
title: "Displayed-frame canvas selection"
status: complete
priority: P2
effort: "4–6 engineer-days"
dependencies: [1]
---

# Phase 2: Displayed-frame canvas selection

## Goal and context

Let the native shell select topmost objects and cycle overlapping objects/groups using metadata from its actual displayed frame. Read [Stage 1](phase-01-start.md), [render scout](../reports/scout-261006-2010-m5-render-identities.md) and `desktop/app/src/studio_shell.rs` preview installation/rendering. The legacy `SelectionSpike` is evidence for simple letterboxing only; `PreviewElement` paints the spike, while the product shell currently uses `gpui::img`.

## Requirements and architecture

Store one displayed bundle: image lease, PreviewIdentity, frame index, seek serial, frame/index digest, and validated geometry. Bind selection to this bundle, not the moving transport playhead or newest network reply. Retain the last complete image while a request is pending; disable selection if its identity cannot be matched to current source. Missing metadata leaves scene/range prompts available.

A GPUI-free hit-test model consumes validated objects and the inverse of the exact paint transform. The shell supplies measured image bounds, letterboxing, window DPI conversion and preview zoom/pan. Fit, zoom around pointer, pan and reset-to-fit must use that same transform for pixels and overlays. Video coordinates remain independent of scaled raster dimensions. Never multiply GPUI logical pointer coordinates by DPI twice.

Topmost supported leaf wins by global traversal rank. A deterministic overlap list includes ancestors/groups, distinguishes repeated instance keys and excludes invisible/unsupported internals. Cycle through the list with an accessible control/key; ties use stable identity ordering. Highlight the selected object, show its name/scene/frame and approximate-bounds status, and allow removing/expanding scope. Bounding boxes do not claim exact shape picking.

Clicking while playing pauses at the displayed frame before freezing selection; a seek/rebuild invalidates frame geometry and requires fresh metadata. Preserve a semantic selection across a new frame only after re-resolving the full tuple in matching metadata. Deleted IDs clear selection with a message. Never silently bind by name, index or nearest rectangle. Keep composer focus and existing timeline shortcuts intact.

When no supported semantic object is available, offer choosing an active scene or dragging a video-coordinate rectangle on the frozen frame. The rectangle is a prompt scope with screenshot/frame/time evidence, not a source anchor. Outside-image clicks do not select. Overlapping scenes/crossfades remain explicit choices.

## Files and ownership

Paths are relative to `/root/fframes-desktop`; this sequential stage owns the following changes.

| Action | Files | Purpose |
|---|---|---|
| Create | `desktop/crates/studio-engine/src/canvas_selection.rs`, `desktop/crates/studio-engine/tests/canvas_selection.rs` | Validated geometry, mapping, overlap/group cycling and rectangle model |
| Modify | `desktop/crates/studio-engine/src/lib.rs`, `desktop/crates/studio-engine/src/preview_state.rs` | Reexports and displayed-bundle identity fencing |
| Modify | `desktop/app/src/preview_coordinator.rs`, `desktop/app/src/preview_worker_client.rs`, `desktop/app/src/frame_image.rs` | Pair/retain metadata with exact image; discard stale lanes |
| Create | `desktop/app/src/canvas_view.rs` | Paint matching image, transform and selection overlays; native pointer/keyboard controls |
| Modify | `desktop/app/src/lib.rs`, `desktop/app/src/studio_shell.rs`, `desktop/app/src/timeline_view.rs` | Connect canvas to real preview and explicit scene fallback |
| Create | `desktop/app/tests/canvas_selection.rs` | Display/seek/install races and scope state |
| Modify | `desktop/app/tests/preview_coordination.rs`, `desktop/app/tests/frame_image.rs`, `desktop/app/tests/x11_shell.rs` | Preserve preview resources and native input behavior |

Retain the legacy spike route; share a proven mapping helper only if it reduces duplication without importing legacy identity assumptions.

## Tasks and steps

- [x] Define finite invertible viewport transforms, half-open rectangle containment/clamping, hit results, ancestor relationships, selected semantic tuple and support reasons. Test portrait/landscape, resize, DPI, fit/zoom/pan, noninvertible transforms and letterbox boundaries.
- [x] Transfer metadata with accepted preview frames and promotion frames through coordinator and shell. Handle thumbnail lanes separately. Compare full open-session/source/generation/frame/seek/index identity before installing or selecting; preserve old pixels on failure without exposing unrelated new metadata.
- [x] Render the image and overlay through one measured content transform. Implement pointer-centered zoom, pan and Fit without rebuilding source. Reuse negotiated preview scale/resource limits rather than creating an unbounded raster cache. Add bounded object/metadata retention: displayed plus pending bundle only, released when image leases retire.
- [x] Implement topmost selection, ancestor/overlap cycling, scope clear/expand and visible approximate/unsupported labels. Pause at displayed frame on click. Snapshot a selection independently of subsequent UI playhead changes; changing frame/source clears or proves a re-resolution before showing an exact selection.
- [x] Implement scene/rectangle fallback with active-scene choice and drag rectangle clamped to full video coordinates. Give legacy projects a visible selection-unavailable reason; do not synthesize IDs or source refs from coordinates.
- [x] Extend native Linux interaction tests with actual title geometry, transformed nested groups, crossfade cycling, zoom/pan and stale replies. Check keyboard focus, composer editing, existing scene/range controls and preview/audio handoff.

## Verification and success criteria

```bash
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test canvas_selection --test preview_state
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test canvas_selection --test preview_coordination --test frame_image --test timeline_controls
```

Extend `desktop/app/tests/x11_shell.rs` and discover its existing ignored/native prerequisites before invoking the focused new case. Use an owned Xvfb process only; track PID/display/worktree and stop it at test end. A headless test cannot qualify physical display behavior.

Pass when the highlighted bounds follow painted pixels across fit/zoom/pan/DPI, topmost/cycle results match traversal, repeated instances remain distinct, source/frame/session changes invalidate safely, rectangle scope is visibly nonsemantic, and old preview/audio/timeline behavior remains intact. Delayed metadata, swapped images and project reopen must all refuse the wrong tuple.

## Risks, security and rollback

Unknown masks/clips/filter regions may make bounds overinclusive: preserve approximation/support flags and allow scene/rectangle fallback. Reject malicious cycles/deep ancestry and nonfinite geometry before hit testing; cap candidate lists at Stage 1 object limits. Keep mapping/hit tests off filesystem/compiler paths. Roll back by disabling canvas object selection and keeping scene/range preview controls; no source or database changes are required here.

Next: Stage 3 indexes only immutable source and uses this selection as a revision-bound query.
