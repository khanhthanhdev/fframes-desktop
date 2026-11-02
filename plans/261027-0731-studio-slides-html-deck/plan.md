---
title: "Studio slides: present mode and HTML deck export"
description: "Let fframes Studio (GPUI desktop) mark slide stops on a video, play it as a slide deck with its animations, and export a self-contained HTML/CSS/JS deck on every platform, with fframes-kit providing slide layouts, a slide-deck template and stop suggestions."
status: in_progress
priority: P1
effort: 30d
branch: main
tags: [feature, desktop, studio, export, fframes-kit]
blockedBy: []
blocks: []
created: 2026-10-27
---

# Studio slides: present mode and HTML deck export

## Overview

A slide is the part of the video that plays from one **stop** to the next and then holds. Studio
stores stops and speaker notes in a new run-time document `slides.json`, shows them in a Slides
tab and on the timeline, presents them full screen with audio, and exports a folder
(`index.html`, `style.css`, `app.js`, `deck.mp4`, `slides.json`, `posters/`) that plays in any
browser from disk. The export renders through the same worker and Skia pipeline as Video export,
so shaders and 3D look identical.

fframes-kit owns how slides look: a `slides` module (`Slide`, `Deck`, layouts with bullet
reveals) that any fframes video can use, and a **Slide deck** project template in Studio and
`cargo fframes new` built on it. For that, managed Studio projects must be able to depend on the
kit, and the macOS/Linux SDK needs an H.264 encoder so decks export on every platform. Kit
components can also suggest stops and notes.

## Scope and boundaries

- **In:** `desktop/`, `fframes-studio-protocol/`, `fframes-studio-runtime/`, `fframes-studio-kit/`,
  `cargo-fframes/`, the SDK scripts and CI, the root `Cargo.toml` (workspace member and patch),
  and the fframes-kit repository (github.com/khanhthanhdev/fframes-kit) for phases 7 and 8.
- **Not touched:** the web editor (`fframes-editor/`, `fframes-editor-controller/`) and the
  fframes core crate (`fframes/`). `timeline.json` keeps its format.
- Scope mode: HOLD SCOPE. Requested: slides in the GPUI app, playing slides with their
  animation, HTML/CSS/JS export, fframes-kit support, a kit slides template that fframes and
  Studio use, and fixes for the two limits (H.264 on macOS/Linux, kit in Studio projects).

## Done outside the phases

- 2026-10-27: fframes-kit 0.1.0 and fframes-kit-macros 0.1.0 published to crates.io (from
  branch `khanhthanhdev/feat/website`, crate code equal to `main` but one small file).

## Phases

| # | Phase | Depends on | Effort | Status |
|---|-------|-----------|--------|--------|
| 1 | [Slides document and engine session](./phase-01-slides-document.md) | – | 3d | Implemented, tested |
| 2 | [Slides tab and timeline markers](./phase-02-slides-tab-and-timeline-markers.md) | 1 | 3d | Implemented, tested |
| 3 | [Present mode](./phase-03-present-mode.md) | 1 | 4d | Implemented; audio-device verification remains |
| 4 | [HTML deck export](./phase-04-html-deck-export.md) | 1 | 5d | Implemented; real-SDK and browser matrix remain |
| 5 | [OpenH264 in the macOS and Linux SDK](./phase-05-openh264-macos-linux-sdk.md) | – | 2d | Implemented; native SDK qualification remains |
| 6 | [fframes-kit in the managed SDK](./phase-06-fframes-kit-in-managed-sdk.md) | – | 3d | Implemented; SDK rebuild/install remains |
| 7 | [fframes-kit slides and the slide-deck template](./phase-07-kit-slides-and-deck-template.md) | 1, 6 | 5d | Implemented; kit 0.2.0 published |
| 8 | [fframes-kit stop suggestions](./phase-08-fframes-kit-stop-suggestions.md) | 1, 2, 6, 7 | 5d | Implemented locally, tested; kit 0.3.0 publication remains |

### Continuation — 2026-10-28

- Phase 8 worker, protocol and store implemented. Public worker tests reject invalid frames,
  stale timing, NaN, overlong notes and oversized encoded responses while keeping preview alive.
- Real template scan finds the six bullet stops at scene frames 27, 45 and 63 in both
  `agenda` and `edit`. Plain-Cargo kit worker roundtrip passes with notes, repeat scans and
  a seek between batches; the existing props edits and pixel checks still pass.
- Engine scans interleave seeks, cancel immediately, and fence stale replies and queued
  acceptance after preview, timing or props changes. Explicit acceptance is undoable and
  idempotent. Root-video suggestions also resolve for videos without scenes. The desktop
  project/engine/UI/app suite passes: 1367 tests, with 12 opt-in tests/doctests ignored.
  The explicitly run real kit-worker roundtrip also passes, including root-video mapping,
  frame rounding and notes. Desktop all-targets Clippy passes with warnings denied.
- Proposed notes and individual/whole-scan acceptance are visible in the Suggested section;
  scan progress, cancellation and older-worker fallback were rendered in both themes.
- SDK assembly now converts versioned external workspace paths to registry dependencies,
  retaining internal framework paths. Its regression test passes. Full packaging tests still
  fail the pre-existing SDK manifest cargo-fframes 1.2.0 versus source 1.3.0 mismatch.
- Kit 0.3.0 is unpublished; root and fixture checks use a command-line Cargo path patch to
  the sibling kit checkout. Their lockfiles currently reflect that local validation graph.
  After publication, regenerate them without the patch to record registry checksums.
- No publication uses the credential exposed in the preceding thread. Rotate it before
  publishing. SDK rebuild/install and macOS/Linux native qualification remain release gates.

```
 1 ──┬── 2 ──────────────┐
     ├── 3               │
     ├── 4 ◀── 5 (other platforms)
     └───────── 7 ◀── 6  │
                 └────── 8
```

Order: phases 1, 5 and 6 can start at once (separate files). After 1: phases 2, 3 and 4 in
parallel. Phase 7 needs 1 (the template ships a `slides.json`) and 6 (the SDK builds kit
projects). Phase 8 is last. Phase 4 alone gives a shareable deck on Windows; with 5 on every
platform.

## Releases

| Release | Contains | Phase |
| --- | --- | --- |
| fframes-kit 0.2.0 | `slides` module, `BulletedList` reveal | 7 |
| fframes-kit 0.3.0 | `suggest_stops` API and callers | 8 |
| SDK | OpenH264 (macOS/Linux), kit + `fframes-studio-kit` vendored | 5, 6 (kit version bumped in 7, 8) |

## Key decisions

| Decision | Choice | Why |
| --- | --- | --- |
| Where stops live | New `slides.json`, a run-time document like `props.json` | `timeline.json` uses `deny_unknown_fields` and `version == 1` in core (`fframes/src/timing.rs`), so it cannot hold stops without a core change |
| Stop identity | `(scene key, scene frame or "end")` | Scenes keep their own frame numbers when trimmed; `"end"` follows trims |
| Default stops | `"end"` of every clip, placed before any cross-fade into the next clip | A deck works for every existing project with no setup |
| Timeline the engine resolves against | The one the person sees, including retimes (new `PreviewNews::retimed`) | Today only new builds reach the engine (`engine/handle.rs:552-557`) |
| Undo | One history for timing and slides gestures | Ctrl+Z in the Timeline context must undo a marker drag, not a trim |
| Slide posters | Decoded from the exported `deck.mp4`; JPEG posters plus 320 px thumbnails | The preview worker caps frames at 1280×720 (`fframes-studio-protocol/src/preview.rs:6-7`) |
| Video codec | H.264 in MP4 everywhere: libx264 on Windows, OpenH264 added to the macOS/Linux SDK (phase 5) | Browsers do not play MPEG-4 Part 2; MP4/H.264 is the most compatible format (Safari, iOS, PowerPoint); OpenH264 is BSD and builds with make + nasm |
| Deck manifest | Inlined as JSON with `<`, `>`, `&`, U+2028/9 escaped as `\u` sequences, plus a CSP | `fetch()` of a local file is blocked from disk; notes are untrusted text |
| Folder publish | Staging folder renamed with the existing no-replace rename, owner file and orphan sweep | `std::fs::rename` replaces empty folders on Unix |
| Present mode | Presentation stage inside the player section, own focus and key context, full screen | Frames notify only the player section; a second window would need its own frame images |
| Slide visuals | fframes-kit `slides` module; Studio and `cargo fframes new` templates use it | Decks also render as plain fframes videos; Studio stays layout-agnostic |
| Kit in Studio projects | Kit from crates.io, vendored into the SDK; materialization patches `fframes` to the SDK copy in the staging manifest | One `fframes` in the graph; projects still may not use `[patch]` |
| Present resolution | Uses the existing 1280×720-capped preview | Raising the cap changes the preview handshake; the HTML deck is the full-quality output |

## Not in scope (offered earlier, not requested)

Presenter view on a second display, "loop while waiting" holds, clickable HTML overlays, PPTX
and PDF exporters (the deck prints to PDF through its print stylesheet), interactive WASM export.

## Red Team Review

### Session — 2026-10-27

Three hostile reviewers (Assumption Destroyer, Failure Mode Analyst, Security Adversary), each
backing findings with file:line evidence. **Findings:** 25 raised, deduplicated to 15, all
accepted. **Severity:** 1 Critical, 9 High, 5 Medium.

| # | Finding | Severity | Applied to |
|---|---------|----------|------------|
| RT1 | Timing edits never reach the engine (`retimed` stays in the UI), so stops go stale and export refuses forever | Critical | Phase 1, 4 |
| RT2 | Resolution ignored scene overlaps; default stops landed inside cross-fades; no fallback for workers without clip data | High | Phase 1 |
| RT3 | Reusing `export_video` emits terminal `Complete` and disables Cancel before posters exist | High | Phase 4 |
| RT4 | H.264 (libx264) exists only in the Windows SDK | High | Phase 4, 5 |
| RT5 | A separate present layout freezes during playback; the `PLAYER` key context wins over a `PRESENT` one; fullscreen is async | High | Phase 3 |
| RT6 | Audio keeps playing past `play_until` until the UI notices | Medium | Phase 3 |
| RT7 | Promotions and retimes restart playback while presenting | High | Phase 3 |
| RT8 | Ctrl+Z and header undo only touch the timing history, so they undo a trim instead of a marker drag | High | Phase 1, 2 |
| RT9 | Agent-written `slides.json` skips candidate validation; later note edits block task undo | High | Phase 1 |
| RT10 | Folder rename is not no-clobber on Unix, has no Windows retry, and crashes leave staging folders | Medium | Phase 4 |
| RT11 | Suggestions from project code are unbounded; NaN or >1 MiB responses stop the preview worker | High | Phase 8 |
| RT12 | One long scan request blocks the preview pump and exceeds the 30 s worker timeout | High | Phase 8 |
| RT13 | Manifest escaping was unspecified: either garbled text or `</script>` injection; no CSP | High | Phase 4 |
| RT14 | Trims silently make clip-end stops unresolved and block export | Medium | Phase 1, 2 |
| RT15 | Inconsistent limits (1000 × 8 KiB notes > 512 KiB), 1000 full-size PNG posters, wrong GOP and `prepare_destination` citations; kit identity collisions and kit not usable by managed projects | Medium | Phase 1, 4, 6, 8 |

### Feasibility

Verified buildable on the current code: GPUI fork fullscreen on Windows, a play bound in
`PlaybackClock`, optional worker capabilities, run-time documents outside the build revision,
frame-exact decoding with `FFmpegDecoder`, faststart AAC MP4, no-replace rename helpers, and
every kit hook point. The two limits found in review are now phases: phase 5 adds H.264 to the
macOS/Linux SDK, phase 6 lets managed projects use fframes-kit. Not yet proven, covered by
tests: whether x264 B-frames shift frame timestamps (phase 4), the SDK size added by the kit's
optional dependencies (phase 6), and whether Cargo accepts the staging `[patch]` against
vendored sources offline (phase 6 unit and SDK tests).

### Whole-Plan Consistency Sweep

All phase files were rewritten or edited for the decisions above; stop limits are 300 stops,
512 KiB, notes 8 KiB, titles 256 B everywhere; posters are JPEG in every phase; no phase still
says resolution uses `scene_frames - trim_end_frames` or that the deck export forwards
`export_video` progress. No unresolved contradictions.

## Verification (whole plan)

- `cargo fmt --manifest-path desktop/Cargo.toml --all --check`
- `cargo clippy --locked --manifest-path desktop/Cargo.toml --all-targets -- -D warnings`
- `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project -p studio-engine -p studio-ui -p fframes-studio`
- `cargo test -p fframes-studio-protocol -p fframes-studio-runtime` (root workspace)
- Real-SDK ignored tests as in `.github/workflows/desktop-self-hosted.yml`.

<!-- slug: studio-slides-html-deck -->
