---
phase: 8
title: "fframes-kit stop suggestions"
status: in_progress
priority: P2
effort: 5d
dependencies: [1, 2, 6, 7]
---

# Phase 8: fframes-kit stop suggestions

<!-- Updated: Red Team 2026-10-27 - suggestion identity and spacing (RT7), project scope (RT8),
bounded untrusted input (RT11), chunked cancellable scan (RT12) -->

## Goal

Let fframes-kit components suggest slide stops and speaker notes while Studio renders, so a
deck built from kit components gets sensible stops (one per bullet, step or chapter) with one
click. Suggestions are never applied silently: the person accepts them in the Slides tab.

## Implementation — 2026-10-28

- Implemented locally in both repositories; fframes-kit 0.3.0 is prepared but unpublished.
  The release, registry lockfile regeneration and SDK rebuild/install are still gates.
- Kit suggestions are lazy, keyed by caller/key/root start, and suppressed under Freeze
  and Loop. Sub-frame views scale the frame origin with FPS to preserve root seconds.
- The worker evaluates the SVG tree through inspection, without rasterizing preview
  pixels. Batches carry a reset flag so each scan starts with fresh observations.
- Studio scans in bounded batches between seeks, supports cancellation, and fences
  results and acceptance against preview identity, timing generation and props generation.
  Acceptance uses the normal undoable slides edits; repeated acceptance is idempotent.
- Root-video suggestions map through real clips, or through a virtual `"video"` clip only
  for timelines without scenes. Reveals round up to the first complete frame, using the
  template's floating-point tolerance. Existing workers without the capability hide the UI.
- The Suggested section shows proposed notes, individual acceptance, Accept all, progress,
  cancellation and refusal reasons. Light and dark gallery captures were inspected.

## Scope

After phase 6, Studio-built projects can depend on fframes-kit and `fframes-studio-kit`, and
the phase 7 slide-deck template wires `PropsStore` hooks into its worker. This phase serves
every project with those hooks: the phase 7 template, kit projects that add the hooks, and
plain Cargo kit projects such as `desktop/fixtures/kit-props`. Other projects and older
workers behave as before: the scan button is hidden when the worker lacks the capability.

The kit change ships as fframes-kit **0.3.0** on crates.io (0.2.0 is the phase 7 slides release); the SDK then vendors 0.3.0 (phase 6 process).

## Context

- fframes-kit (github.com/khanhthanhdev/fframes-kit, on crates.io; paths checked at `01f5b326`) has no stop or notes
  concept. Paths below are in `fframes-kit/src/`.
- `PropsSource` (`props.rs:135-155`, `studio` feature) is on every `Ui` (`ui.rs:42-43`), shared
  through `shifted` (`ui.rs:158-170`), gated by `listening()`. Studio implements it in
  `fframes-studio-kit/src/lib.rs` (`PropsView`, `PropsStore`, `hooks()` at `:113-119`; video-level
  views use the scene key `"video"`, `:33-34, 94-96`).
- Components have no start time (`time.rs:4-5`); sub-frame `Ui`s (`ui.rs:175-198`) and
  `Freeze`/`Loop` do not map local time to timeline time. Components report only while
  rendered (`time.rs:53, 92`); `TransitionSeries` lays out all items every frame
  (`transitions/series.rs:97-141, 179`).
- `LaggedStart` has no key field and defaults to 50 ms between children
  (`animate/group.rs:14-20, 56-58`).
- Protocol limits: control messages are capped at 1 MiB and a message over it, or one that
  does not parse, stops the worker (`desktop/crates/studio-engine/src/services/worker_client.rs:344-353`,
  `preview_worker_client.rs:364-369`); `serde_json` turns a NaN `f32` into `null`, which does not
  parse back. Preview requests time out at 30 s and a timeout stops the worker
  (`worker_client.rs:112`).
- The displayed worker is served by one synchronous pump (`preview_coordinator.rs:837-845,
  974-985`); a long request blocks seeks and retimes.
- Optional capabilities follow `editor_frame_v1` (`fframes-studio-protocol/src/preview.rs:27`,
  `preview_worker_client.rs:117-122`, `fframes-studio-runtime/src/preview_worker.rs:386, 523`).
- No changes to the `fframes/` core crate.

## API addition (fframes-kit, `studio` feature)

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct SuggestedStop {
    pub seconds: f32,          // root Ui time: scene seconds for Ui::for_scene, video seconds for Ui::new
    pub label: Option<String>,
    pub notes: Option<String>,
}

pub trait PropsSource: Send + Sync {
    // existing methods unchanged
    fn suggest_stops(&self, origin: &str, stops: &[SuggestedStop]) { let _ = (origin, stops); }
}

impl Ui<'_, '_, '_> {
    /// Converts local seconds to root time; does nothing unless a source listens.
    /// A no-op without `studio`, so call sites need no cfg.
    #[track_caller]
    pub fn suggest_stops(&self, stops: impl FnOnce() -> Vec<SuggestedStop>);
}
```

- `Ui` gains `pub(crate) origin: usize` (frames): 0 in `new`/`for_scene`, increased by
  `shifted`, kept by sub-frame `Ui`s; `Freeze` and `Loop` set a "no stops" flag.
- The origin string is built from `Location::caller()` plus the component key when it has one
  plus the root-time start of the reporting `Ui`, so two `LaggedStart`s in one scene never
  replace each other's suggestions and repeated frames of the same one do.
- Kit-side spacing: a component never suggests two stops less than 0.4 s apart (it keeps the
  later one), so `LaggedStart` defaults do not produce a stop per frame.

## Studio side

- **Bounds, enforced three times** (kit store, worker before sending, engine before
  resolving): finite `seconds >= 0`; at most 64 stops per origin and 300 in total
  (`MAX_SUGGESTED_STOPS` in `fframes-studio-protocol/src/preview.rs` next to
  `MAX_DIAGNOSTICS`); labels at most 256 bytes, notes at most 8 KiB. Excess is dropped and the
  response says `truncated: true`. If the encoded response would still exceed the message cap,
  the worker answers `PreviewResponse::Error`, which keeps it alive.
- `fframes-studio-kit`: `PropsStore` keeps
  `suggested: Mutex<BTreeMap<(scene, origin), Vec<SuggestedStop>>>` (replace semantics) and
  `suggested_stops()`. `PropsHooks` (`fframes-studio-runtime/src/props.rs:19-48`) gains an
  optional `suggestions` closure. Suggestions from a `"video"` view are video seconds; the
  engine maps them through the laid-out timeline to a scene and frame instead of treating
  `"video"` as a scene key.
- **Protocol:** capability `suggested_stops_v1`; request
  `SuggestStops { frames: Vec<usize>, timing_generation }` renders at most 16 frames at the
  smallest preview scale while listening and returns the suggestions collected so far.
- **Scan, driven by the engine:** samples the first frame of every clip and then every 0.5 s,
  capped at 2000 samples (reported when hit). It sends batches of 16 between the pump's seek
  intents, so scrubbing stays responsive; it is cancellable, shows progress in the Slides tab,
  and drops its result if the timing generation changes mid-scan.
- Slides tab: a "Suggested" section listing suggestions not yet in `slides.json`, each with
  Accept, and "Accept all" (refused with the reason when the result would exceed the phase 1
  document limits).

## Files

- fframes-kit repository: `src/props.rs`, `src/ui.rs`, `src/time.rs` (`Freeze`, `Loop`, `Series`),
  `src/animate/group.rs`, `src/transitions/series.rs`, `src/overlay/chapters.rs`,
  `src/kits/cs/keyed.rs`, `src/voiceover/script.rs` (`suggest_stops(&self, ui)` helper),
  `src/slides/slide.rs` (`Slide` reports `reveal_times()` as suggestions), docs, gallery scene `SlideStops`.
- This repository: `fframes-studio-kit/src/lib.rs`, `fframes-studio-kit/Cargo.toml` (new kit
  rev), `fframes-studio-kit/tests/props.rs`, `fframes-studio-runtime/src/{props.rs,preview_worker.rs}`,
  `fframes-studio-protocol/src/preview.rs`,
  `desktop/crates/studio-engine/src/services/{preview_worker_client.rs,preview_coordinator.rs}`,
  `desktop/crates/studio-engine/src/slides/suggest.rs` (create),
  `desktop/crates/studio-ui/src/slides_panel.rs`, `desktop/fixtures/kit-props/`.

## Tasks & Steps

1. Kit: `SuggestedStop`, `PropsSource::suggest_stops`, `Ui::origin`, `Ui::suggest_stops`,
   spacing; tests that a stop inside nested `Sequence`s reports root time, that two
   `LaggedStart`s keep separate suggestions, and that `Freeze`/`Loop` report nothing.
2. Kit callers: `LaggedStart`/`Succession`, `TransitionSeries` (sequence ends), `Series`,
   `ChapterList` (titles), cs step views (`note`), `VoiceoverScript` helper, `BulletedList` and `Slide` bullets (reveal timing from phase 7), one stop per item.
3. Publish fframes-kit 0.3.0 (and the macros if changed); bump the kit version in
   `fframes-studio-kit`, the kit-props fixture, the SDK vendor graph and the phase 7 template.
4. `PropsStore` suggestions with bounds, `PropsHooks` closure, worker request, capability.
5. Engine batched scan, mapping, resolution; Slides tab Suggested section.

## Verification

- Kit: `cargo test -p fframes-kit --features studio` and without `studio`; the `SlideStops`
  gallery scene renders unchanged without a listening source.
- `fframes-studio-kit` tests (own workspace): `LaggedStart` with three children in a scene
  offset by 2 s reports scene-local stops at the right seconds; a second render does not
  duplicate; 1000 suggested stops with 10 KiB notes and a NaN are truncated, flagged and
  never break serialization.
- Plain Cargo integration test with the kit-props fixture (the existing
  `desktop/app/tests/kit_props_roundtrip.rs` pattern): scan returns the expected stops,
  accepting them writes `slides.json`, and seeks issued during the scan are answered.
- Workspace commands from `plan.md`.

## Success criteria

- [ ] A kit deck with a bullet list and a transition series gets one suggestion per bullet and
      per section after one scan.
- [ ] No scan input, however large or malformed, stops the preview worker.
- [ ] Projects and workers without the capability behave as before; no change under `fframes/`.

## Risks

- **Sampling misses** components shown for less than 0.5 s; clip starts are always sampled and
  `TransitionSeries` reports all boundaries. Accept for v1.
- **Two repositories:** the Studio side stays behind the capability so it can merge first.
