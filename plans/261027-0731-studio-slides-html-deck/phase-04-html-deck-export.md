---
phase: 4
title: "HTML deck export"
status: pending
priority: P1
effort: 5d
dependencies: [1]
---

# Phase 4: HTML deck export

<!-- Updated: Red Team 2026-10-27 - split export_video into render and publish (RT3), H.264
platform scope (RT4), no-replace folder publish with retry and orphan sweep (RT10), exact JSON
encoding and CSP (RT13), thumbnails and poster limits, corrected citations (RT15) -->

## Goal

Add a second export target, **Slide deck (HTML)**, that writes a folder which plays the project
as a slide deck in any current browser, opened straight from disk, at full render quality.

## Output

```
<name>/
├─ index.html    stage, overview grid, notes panel; the deck manifest inlined as JSON
├─ style.css     theme and print stylesheet (one poster per page)
├─ app.js        navigation, exact stops, overview, notes, URL hash, fullscreen
├─ deck.mp4      H.264 + AAC with faststart, the same render as Video export
├─ slides.json   the same manifest, for tools
├─ posters/      0001.jpg … one full-size JPEG (quality 92) per stop
└─ thumbs/       0001.jpg … 320 px wide, for the overview grid
```

Manifest: `{ "version": 1, "title", "width", "height", "fps", "total_frames",
"slides": [{ "index", "start_frame", "stop_frame", "title", "notes", "poster", "thumb" }] }`.

## Context

- Video export: `EngineHandle::start_export` (`desktop/crates/studio-engine/src/engine/handle.rs:245-319`)
  freezes the source, reads `timing_for_export()` / `props_for_export()` and calls
  `export_video` (`services/export_service.rs:121-337`).
- `export_video` ends with its own publication: `begin_publication`/`finish_publication`
  (`:300-306`) move `ExportControl` to `PUBLISHED`, then it emits `ExportProgress::Complete`
  (`:332-335`). The UI treats `Complete` as terminal (`shell/engine_events.rs:942-946`) and
  Cancel is a no-op after publication (`:906-916`). So the deck cannot simply call it.
- `prepare_destination` (`:557-572`) checks only the extension and the parent; non-existence is
  enforced by the hard link in `publish_no_clobber` (`:574-576`).
- Atomic no-replace rename exists in `desktop/crates/studio-project/src/publish/mod.rs:177-194, 373`
  (Linux, macOS and Windows implementations, `probe_no_clobber`). Windows rename retry on
  sharing violations exists in `studio-harness/src/adapter_install.rs:335-347`.
- `ExportSettings::default()` is MPEG-4 Part 2 (`fframes-studio-protocol/src/export.rs:117-128`).
  H.264 maps only to `libx264` (`:74-79`), and the worker lists only formats whose encoder
  exists (`fframes-studio-runtime/src/export.rs:22-34, 287-293`). **Only the Windows SDK's
  FFmpeg has libx264** (BtbN gpl-shared, `.github/workflows/desktop.yml:194`); the Unix SDK
  FFmpeg is built without `--enable-gpl --enable-libx264` (`desktop/scripts/build-native-ffmpeg.py:21`).
- The output gets `+faststart` (`fframes/src/renderer/concatenator.rs:182`) and AAC audio. GOP
  defaults to 24 (`fframes/src/renderer/encoder.rs:243`), capped at the fps (`:198`).
- `FFmpegDecoder::decode_up_to` returns the first frame with `pts >= target`
  (`fframes-media/src/video_decoder.rs:957-1001`).
- UI: `ExportPanel` (`studio-ui/src/export/panel.rs:28-79, 295-302`), `ExportPhase`
  (`export/progress.rs:14-90`), shell state and save dialog (`shell/export_panel.rs:19-220`,
  `shell/engine_events.rs:699-926`).

## Platform scope

The deck needs H.264. Windows has it through libx264. **Phase 5 adds OpenH264 (BSD) to the macOS and Linux SDK FFmpeg** so every platform writes the same MP4/H.264 deck. Summary of that work:
- `desktop/scripts/build-native-ffmpeg.py`: build OpenH264 statically (make + nasm), then
  configure FFmpeg with `--enable-libopenh264` and its pkg-config path.
- `fframes-studio-protocol/src/export.rs`: `ExportCodec::H264` encoders become
  `["libx264", "libopenh264"]`; the worker already lists a format when any of its encoders
  exists.
- `fframes-studio-runtime/src/export.rs`: `encoder_params` sets no x264-only options
  (`preset`) for `libopenh264`; give it a bitrate.
- Verify on the macOS and Linux CI runners: the SDK reports `mp4/h264`, and an exported deck
  plays in Safari and Firefox.
Until that lands, the target shows "Slide deck export needs an H.264 encoder, which this SDK
does not include" on macOS and Linux.

## Requirements

- **Panel:** a target switch "Video | Slide deck (HTML)". Deck mode hides container and codec
  (H.264/MP4), keeps size, fixes the frame rate to the video's. It shows the slide count, an
  estimated size, and slides problems; unresolved stops are listed as skipped. Start is
  disabled with the reason when H.264 is missing or no timeline is laid out for the frozen
  build.
- **Save dialog** asks for a new folder name (default "Slides"). The folder must not exist.
- **Refactor `export_video`** into `render_verified(request, ..) -> VerifiedVideo` (render,
  verify, returns the verified temp file; emits only non-terminal progress) and
  `publish_video(verified, destination, control)`. Video export calls both, unchanged in
  behaviour; existing export tests must pass unchanged.
- **`EngineHandle::start_deck_export(session, destination_dir, settings, sdk, builds)`**
  returns the same `ExportJob` with one `ExportControl` for the whole job:
  1. Freeze the source and resolve stops from the engine's latest laid-out timeline, which
     must belong to the frozen build revision and the exported timing generation; otherwise
     refuse with "Wait for the preview to update".
  2. Sweep sibling `.fframes-deck-*` folders whose `owner.json` names a process that is no
     longer running. Create `.fframes-deck-<uuid>` next to the destination with `owner.json`
     (pid, start time).
  3. `render_verified` into `<staging>/deck.mp4` with H.264 settings.
  4. Decode stop frames in one forward pass; stream each to `posters/NNNN.jpg` and
     `thumbs/NNNN.jpg` (own encoder in `deck/posters.rs`; the agent tools' private `png` helper
     is not reused). Progress `Posters { done, total }`; Cancel works between frames.
  5. Write `slides.json`, `index.html`, `style.css`, `app.js`.
  6. Drop the decoder, check the cancel fence, enter publication, then publish the staging
     folder with the `studio_project::publish` no-replace rename, retrying Windows sharing
     violations for up to 10 s. If the destination now exists, fail and leave it untouched.
     Refuse up front where `probe_no_clobber` says no-replace is not guaranteed.
  7. Emit `Complete { destination }` only after the folder rename. On failure or cancel,
     delete the staging folder.
- **Manifest encoding:** `serde_json` output with `<`→`\u003c`, `>`→`\u003e`, `&`→`\u0026`,
  U+2028→`\u2028`, U+2029→`\u2029`; no HTML entity escaping. The data block is the only
  generated text in `index.html`; the page title and all titles and notes are set from
  `app.js` with `textContent`.
- **CSP:** `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; media-src 'self'">`,
  verified under `file://` in Chrome, Edge, Firefox and Safari. If a browser treats `file:`
  as not matching `'self'`, use explicit relative-file-friendly sources there rather than
  dropping the policy, and record the result.
- **Player (`app.js`, no dependencies, no network):**
  - Starts held on frame 0; the first input starts playback (browser autoplay rules).
  - Next plays to the stop. A `requestVideoFrameCallback` loop (fallback:
    `requestAnimationFrame` + `currentTime`) pauses when `metadata.mediaTime` reaches the stop,
    then sets `currentTime = (stop_frame + 0.5) / fps` so the held picture is the stop frame.
  - Previous shows the previous stop's poster at once and seeks the video behind it.
  - Keys: → Space PageDown click = next, ← PageUp = previous, Home End, F fullscreen,
    O overview (thumbnails with `loading="lazy" decoding="async"`), S notes panel, `#N` in the
    URL, Esc closes panels.
  - Print stylesheet: one poster per page with title and notes.
- Templates live in `desktop/crates/studio-engine/src/deck/template/` (`include_str!`).

## Files

- Create: `desktop/crates/studio-engine/src/deck/{mod.rs,manifest.rs,posters.rs,publish.rs}`,
  `desktop/crates/studio-engine/src/deck/template/{index.html,style.css,app.js}`.
- Modify: `desktop/crates/studio-engine/src/services/export_service.rs` (split, new progress
  variants `Posters` and `WritingDeck`), `engine/handle.rs` (`start_deck_export`), `lib.rs`.
- Modify: `desktop/crates/studio-ui/src/export/{panel.rs,progress.rs,settings.rs}`,
  `shell/export_panel.rs`, `shell/engine_events.rs` (`Picker::ExportDeck`, folder dialog).
- Create: `desktop/crates/studio-engine/tests/deck_export.rs`.

## Tasks & Steps

1. Split `export_video`; existing export unit and integration tests pass unchanged.
2. Manifest builder (pure) with the exact encoding, and its tests.
3. Templates; test `app.js` in a browser on a hand-made manifest and a short MP4 first.
4. Poster and thumbnail extraction in one pass.
5. Staged folder publish: owner file, sweep, no-replace rename with retry, cleanup.
6. `start_deck_export` and progress.
7. Panel target switch, folder dialog, progress stages, platform message.

## Verification

- Unit tests: manifest inertness for `</script>`, `<!--<script>` and U+2028 in notes, and a
  round trip (text in the DOM equals the source text); stop to time conversion; publish
  refuses an existing (also empty) destination and leaves it untouched; cancel during posters
  deletes staging and is honoured; a stale `.fframes-deck-*` with a dead owner is swept, one
  with a live owner is kept.
- Ignored real-SDK test (Windows runner, like `native-export.rs`): export Signal Lab; decode
  `deck.mp4` at every `stop_frame` and compare with the poster (mean absolute difference below
  a small bound, which also catches pts shifts from B-frames); `total_frames` equals the
  decoded frame count.
- Browser check on the folder via `file://` (`agent-browser` or Chrome DevTools): press → N
  times; after each hold `video.currentTime` equals `(stop_frame + 0.5) / fps` within half a
  frame; no CSP violations in the console. Capture and inspect stage, overview and notes.
- `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine -p studio-ui`, clippy.

## Success criteria

- [ ] On Windows, an exported folder opened from disk in Chrome, Edge, Firefox and Safari plays
      every slide and holds on its stop frame.
- [ ] The export never overwrites an existing folder; a cancelled or failed export leaves no
      staging folder, and a crashed one is swept by the next deck export.
- [ ] Shader and 3D projects look the same in the deck as in Video export.
- [ ] Notes and titles containing markup display literally and never run.

## Risks

- **pts shifts** (B-frames, edit lists) would make posters or holds one frame off; the
  real-SDK comparison test catches it. If it fails, disable B-frames for deck exports through
  the existing `encoder_params`.
- **Browser frame accuracy** differs; the final `currentTime` set keeps the held frame exact.
