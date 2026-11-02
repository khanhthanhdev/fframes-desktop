---
phase: 4
title: "Media streaming"
status: pending
priority: P1
effort: "2w"
dependencies: [3]
---

# Phase 4: Media streaming

## Goal

Clients see preview frames and hear audio from the server: still frames on seek, smooth playback, and
low-resolution frames during canvas drags.

## Context

- Proposal section 4.4; spike results S2, S3 and S4.
- `desktop/crates/studio-engine/src/preview_state.rs` (`ReadyPreview`: frame, PCM, prepared audio)
- `desktop/crates/studio-engine/src/services/audio_service.rs`
- `fframes-studio-protocol` (`ScaledFrameRequest`, `FrameHeader`, audio requests)

## Requirements

- Seek: newest request wins; the server cancels stale renders by seek serial.
- Each frame is sent together with its `EditorFrameMetadata` so client hit-testing matches the image.
- Frames are rendered at the client's view size (times device pixel ratio), capped at the video size.
- Playback uses H.264 chunks when the client reports WebCodecs support, otherwise JPEG at reduced rate.
- Audio: 48 kHz stereo PCM, client audio clock is the master; the server sends ahead within a bounded
  window.
- Per-client streams: a phone at 720p and a browser at 1080p can watch at once.

## Architecture

- `studio-server/src/media/`: `stills.rs` (JPEG encode of `ScaledFrameResponse`), `playback.rs`
  (frame sequence from the worker into an FFmpeg H.264 encoder at low latency, chunks as
  `MediaRecord::H264Chunk`), `audio.rs` (reads prepared PCM through the worker's audio requests and
  chunks it), `pacing.rs` (bounded queues per client, drops stills, never drops audio).
- Client side lives in phase 6 (`player` module): canvas drawing, WebCodecs decoder, AudioWorklet.

## Related code files

- Create: `desktop/crates/studio-server/src/media/{mod.rs,stills.rs,playback.rs,audio.rs,pacing.rs}`
- Modify: `desktop/crates/studio-server/src/session_host/*` (preview commands route to media)

## Implementation steps

1. Stills with metadata and seek-serial cancellation; test that a burst of 50 seeks returns the last.
2. Audio chunks and a client clock protocol (client reports its played sample position).
3. H.264 playback stream with keyframe on every seek; JPEG fallback.
4. Drag mode: the client asks for frames at a lower scale while a gesture is active.
5. Measure on this VM: still latency, playback frame drops, audio gaps; record in
   `reports/media-measurements.md`.

## Todo

- [ ] Stills and metadata
- [ ] Audio chunks and clock
- [ ] H.264 playback and fallback
- [ ] Drag-mode frames
- [ ] Measurements

## Success criteria

- Seek to visible frame under 150 ms at 1080p on loopback with the CPU renderer for the template video.
- 60 s of playback without audio gaps; A/V offset under 40 ms.

## Risks

CPU-only rendering on this VM may not reach real time for heavy videos. Playback then degrades to the
rate the worker manages, as the desktop app does today, and the measurement report says so.
