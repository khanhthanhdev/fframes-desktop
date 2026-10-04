---
title: "Phase 3: Timeline model and native controls"
status: completed
---

# Phase 3: Timeline model and native controls

## Context, outcome and ownership

Priority P2; locally completed. Depends on [Stage 1 protocol](phase-01-start.md) and [Stage 2 installed-preview model](phase-02-revision-safe-preview-coordination.md). One Stage 3 executor owns listed files after prior gates; Stage 4 waits on geometry/control tests. Deliver compiled scene/audio tracks, play/pause, frame step, scrub, zoom/scroll, range and overlapping-scene selection, bounded thumbnails and time display. All arithmetic is GPUI-free. No source timeline mutation, drag-to-agent task, object selection or preset features; those belong to later roadmap milestones.

## Verified starting points

Compiled authority is `TimelineReport` and half-open scene report endpoints (`/root/fframes-desktop/fframes/src/renderer/preview.rs:177`, `:188`, `:193`, `:194`); audio placements carry `TrackMix` (`:200`, `:204`). Existing protocol `TimelineResponse` carries fps/frames/dimensions/scenes/audio (`/root/fframes-desktop/fframes-studio-protocol/src/lib.rs:235`), but Stage 1 adds validated identity and fuller mix projection. Native shell is `/root/fframes-desktop/desktop/app/src/studio_shell.rs:618`; existing engine module surface is `/root/fframes-desktop/desktop/crates/studio-engine/src/lib.rs:1`. Reuse engine instead of creating a timeline crate.

## Requirements and architecture

Data flow: installed ReadyPreview timeline → validated engine timeline model → viewport geometry/hit test/selection → immutable shell presentation → tagged playhead intent → Stage 2 frame pump. Tracks are derived only from the compiled report matching displayed identity, never current source's speculative durations. Build in progress labels the old report; a failed build cannot clear it. Stage 2 switches report/frame/audio together; rebuilding with fewer frames clamps playhead/range, discards unavailable scene instance IDs and bounds scroll.

Use integer frame indices for playhead/scene/range selection, checked conversion to seconds using validated finite positive fps. Selectable ranges are `[start, end)` with `0 <= start <= end <= total_frames`; last renderable frame is `total_frames - 1`. A timeline cursor can sit at exact video end for stopped time display while rendering the final valid frame. Empty/zero-duration video shows stopped zero time and disables frame/selection commands safely. Audio placements retain sample-accurate seconds/rate metadata; do not quantize their displayed placement to scene frames or rewrite audio data.

Proposed `TimelineModel`, `TimelineViewport` and `TimelineSelection` stay in existing GPUI-free studio-engine. Viewport contains finite positive pixels-per-second, visible width and bounded scroll; pointer x maps through track origin/scroll/zoom and clamps to timeline. Reject NaN/infinity/overflow and do not divide by zero for empty/very short clips. Zoom around pointer/current playhead preserves its screen coordinate; scroll clamps after resize/rebuild. Geometry returns visible scene/audio rectangles, ticks and overlapping hits in deterministic track order. Rendering consumes rectangles only; UI has no duplicate time conversions.

Each compiled scene instance has revision-scoped ID independent of display type name/index ambiguity. Hit-testing returns all half-open overlapping scene instances at x/y; click selects deterministic topmost instance, cycling or a native list chooses the others. Preserve range alongside explicit selected scene ID; repeated type/name scenes remain distinct. Dragging empty time selects a normalized range in either direction; scene selection uses compiled endpoints. No resize/reorder/trim mutation is enabled in M2. Selection is valid only against displayed identity and does not imply source-edit isolation.

Implement keyboard-focus-aware actions: Space toggles playback, arrows step one frame only when timeline/preview focused, Home/End seek start/end, pointer drag scrubs while retaining prior playing/paused intent. Text inputs retain editing keys. Paused step requests latest frame and leaves audio paused; playing scrub resets clock/serial in Stage 4. Time display shows seconds and frame index using report fps; avoid an invented SMPTE drop-frame contract. Native controls communicate disabled/buffering/building/unsupported shader states and have labels/focus cues.

Thumbnails use scaled frame operations on the same displayed worker with distinct request destination/cache key `(identity, backend, scale, frame_index, media_hash)`. Schedule only visible samples and neighbors; main preview seek always wins, one thumbnail request can be in flight, queued obsolete thumbnails are replaced. Use LRU + explicit byte/entry accounting: proposed 64 entries/16 MiB decoded images, visible slot count capped, no one-thumbnail-per-video-frame growth. Reuse image conversion and explicit GPUI eviction. Worker generation/source/media changes invalidate thumbnails even if frame numbers match. Thumbnail replies cannot overwrite main preview image or playhead. Audio tracks are required; waveform decoration is not a separate M2 feature and must not add a second authority.

## Files and actions (absolute paths)

| Action | File | Work |
|---|---|---|
| Create (proposed) | `/root/fframes-desktop/desktop/crates/studio-engine/src/timeline.rs` | Validated compiled model, viewport arithmetic, hits, ranges and ticks. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/preview_state.rs` | Stage 2-created model: clamp selections/playhead on ready install. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/lib.rs` | Export timeline model without GPUI. |
| Create (proposed) | `/root/fframes-desktop/desktop/app/src/timeline_view.rs` | Native tracks/ticks/playhead/hit-list and input gestures. |
| Create (proposed) | `/root/fframes-desktop/desktop/app/src/thumbnail_cache.rs` | Bounded revision/media-keyed LRU with image disposal. |
| Modify | `/root/fframes-desktop/desktop/app/src/studio_shell.rs` | Native transport controls/time/focus and timeline rendering. |
| Modify | `/root/fframes-desktop/desktop/app/src/preview_coordinator.rs` | Stage 2-created pump: thumbnail scheduling and destination separation. |
| Modify | `/root/fframes-desktop/desktop/app/src/frame_image.rs` | Reuse conversion/disposal for bounded thumbnail images. |
| Modify | `/root/fframes-desktop/desktop/app/src/lib.rs` | Wire native timeline/cache modules. |
| Create (proposed) | `/root/fframes-desktop/desktop/crates/studio-engine/tests/timeline.rs` | Geometry, overlapping/repeated scenes and selection matrix. |
| Create (proposed) | `/root/fframes-desktop/desktop/app/tests/timeline_controls.rs` | Control-intent and bounded thumbnail integration tests. |

No core TimelineReport schema change is planned; if Stage 1 missed metadata, transfer protocol/runtime ownership sequentially for the smallest fix and rerun its contract gate. New file paths listed as Stage 2-created are proposed, not existing today.

## Implementation steps and tasks

1. Build real compiled multi-scene/overlap/repeated-scene report fixtures; add geometry acceptance tests before UI wiring.
2. Implement engine validation and frame/second/x conversions, zoom anchor, scroll, overlap list, normalized range and rebuild clamp.
3. Render visible scene/audio tracks and ticks from engine geometry. Ensure stale source is visibly distinct from installed compiled timeline.
4. Wire keyboard/pointer/playback intents to coordinator with monotonically newer seek serial; reserve CPAL clock/reset connection for Stage 4.
5. Add visible-only thumbnail scheduler/cache and explicit texture eviction; give scrub/playback priority over thumbnails.
6. Walk native controls and focus; record overlap/zoom/end/short-video evidence and pass geometry/control/cache gates.

- [x] Compiled scene/audio tracks remain tied to displayed preview identity.
- [x] GPUI-free geometry validates timebases, half-open boundaries, finite values and empty clips.
- [x] Zoom/scroll/hit tests and forward/reverse range selection have deterministic tests.
- [x] Repeated/overlapping scene instances can all be selected explicitly.
- [x] Play/pause/frame-step/paused and playing scrub/time display are wired with focus-safe actions.
- [x] Rebuild preserves/clamps playhead/range/scroll and rejects stale selected IDs.
- [x] Visible-only thumbnails enforce cache limits, priority and image release.
- [x] Engine/control tests and native interaction evidence pass before audio clock stage.

## Verification and measurable success

Existing narrow baseline: `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine`. Proposed targets after creation:

```sh
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test timeline
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test timeline_controls
```

Unit matrix: frame/seconds/x round-trip within half-frame tolerance; nonzero scroll; pointer-anchored zoom; min/max zoom; NaN/infinity; huge clips/overflow; empty and one-frame video; scene endpoints `[start,end)`; exact overlap boundary; repeated scene names; reverse/empty range; clamp on shorter rebuild. Integration matrix: latest scrub vs thumbnail response, cache entry/byte eviction, stale scale/revision/media key, resize while scrubbing, keyboard focus in text input, preserving paused state, end cursor/final image distinction. Native matrix: scene/audio rows show compiled timing, cycle overlap scenes, zoom/pan without pointer jump, scrub/step/time feedback and disabled empty state.

Done means arithmetic is entirely engine-owned, all M2 controls are actionable, selected IDs always exist in displayed report, latest main frame never comes from thumbnail/old seek and cache high-water remains at limits. Stage 4 qualifies actual audio-clock behavior; the Stage 3 execution evidence below covers provisional silent playback only.

## Risks, security, rollback and next

High (medium × high): overlap/time arithmetic selects wrong scene/revision. Mitigate pure engine tests with repeated scenes and revision guards. High (medium × high): thumbnails starve scrubbing or leak GPU images. Mitigate low-priority single-flight work, byte/entry limits, explicit disposal and high-water metrics. Medium: platform focus/key behavior varies; native interaction qualification remains target-specific.

Treat worker report names/paths as bounded display data; no filesystem actions arise from a track click. Range selection does not mutate source. Rollback native timeline route to M1 placeholder and retain Stage 2 preview/coordinator; drop thumbnail images/artifacts on detach, preserve source/history. Stage 4 owns shared shell/pump/image files only after this gate, then replaces provisional wall-clock playback with app audio output scheduling.

## Local implementation evidence — 2026-10-03

Stage 3 is implemented locally, uncommitted/unpushed. Compiled scene/audio placement, packed overlap lanes, sample-accurate audio geometry, cursor/end/range arithmetic, viewport zoom/scroll and selection live in `studio-engine`. Native controls use a retained timeline entity and focus-local keys. `TimelineTransport` supplies explicitly labeled **silent monotonic playback**, not live audio or an output clock. Successful installs clamp position/range/scroll and invalidate revision-scoped scene IDs and thumbnails.

Thumbnail sampling/queueing is capped at 12 visible requests, one in flight and one completion; main preview seeks dispatch first. The revision/backend/scale/frame/media keyed LRU enforces 64 entries/16 MiB decoded bytes, and eviction/detach explicitly releases window images. Engine review also fixed audio rows shifting upward when overlapping scene lanes left the horizontal viewport; the regression pans from a two-lane overlap to a lone later scene.

Native verification exposed a tiny-skia static-raster cache scale bug. `CpuFrameRenderer` now clears static pixels when output dimensions change, preserving same-size reuse. A compile-time static, half-opacity group regression fails before the fix and passes after it, verifying main → thumbnail → main pixels at both inside/outside coordinates. A fresh SDK was assembled and installed into an isolated home so native thumbnails exercised the corrected framework.

- Final desktop workspace: **131 nonignored tests pass**, including ten timeline tests and six cache tests; workspace/all-targets Clippy with `-D warnings` passes. Core: 61 tests with `compile-time-svgtree` and library/test Clippy pass. Root protocol/runtime: 17 tests pass. Packaging: six tests pass. All-targets core Clippy remains red on two pre-existing warnings in the unchanged `yuv420_conversion` benchmark; no unrelated benchmark cleanup was made.
- Fresh SDK assembly passed two offline worker builds and its preview probe. Explicit real `timeline_controls` integration passed both SDK-bundle installation and installed-SDK routes. It independently checks 165 frames, compiled ranges `(0,60)`, `(45,135)`, `(105,165)`, repeated IDs, overlap cycling, first audio onset `5513/44100` seconds, thumbnail/main-frame separation, latest seek 137, unchanged checkpoint and worker/materialization cleanup. The existing real Stage 2 failed-build/re-prime/cancel regression also passed explicitly during this work.
- Normal Linux X11/Xvfb/software-rendered shell: verified ruler seek 51 → cycle Repeated `[0..60)` → cycle Overlay `[45..135)` without moving playhead; Right → paused frame 52; reverse Shift-drag → `[63..116)`; anchored zoom and PageDown pan; stopped end cursor 165 with final rendered frame 164; Space restart; playing scrub resumed at frame 98 with Pause visible, advanced to frame 120 and paused with exact matching pixels/time; resized 1160×760 remains navigable. Shorter rebuild clamps cursor 165 → 75, range `[63..116)` → `[63..75)` and renders frame 74. Default 1280×800 layout retains the full thumbnail/time footer. Close clears controls/tracks/images and leaves no leased `build-*` tree or owned preview worker.
- Inspected screenshots: `.amp/in/artifacts/stage3-timeline-final.png`, `stage3-{overlap-first,overlap-second,stepped,range,zoom,pan,end,playing,resized,short-rebuild,scrub-resumed,scrub-progress-paused,closed}.png`. Logs: `stage3-{final-workspace-tests,final-clippy,core-tests,core-targeted-clippy,protocol-runtime,packaging,sdk-assembly,fresh-sdk-tracks,final-real-tracks,real-revision-regression,native-cleanup}.log`. Review artifacts are local and excluded from Git.

The reusable portable fixture/helper listed in Stage 4 already exists, with repeated/overlapping scenes, a moving rectangle and deterministic stereo WAV cues. Reproduce its real test with `SDK_ACTIVE=<installed-sdk> cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test timeline_controls real_compiled -- --ignored --nocapture`, or supply `SDK_BUNDLE=<fresh-bundle>`. Optional `TIMELINE_NATIVE_ROOT`, `TIMELINE_NATIVE_DATA` and `TIMELINE_NATIVE_SDK_HOME` retain isolated native inputs. Project roots must be fresh and outside the repository to avoid inherited Cargo configuration.

Stage 4 remains open: live output/device ownership, bounded audio callback/reader, audible-position epochs and atomic audio/video handoff, CLI/color/alpha/shader qualification, 10-minute/2,000-seek/50-rebuild resource measurements and CI M2 evidence. Fixed cache bounds and short native sessions do not establish those sustained gates. Physical GPU/display/IME, Windows/macOS native behavior and authenticated ACP are still unqualified. No commit, publication or completed M2 is implied.
