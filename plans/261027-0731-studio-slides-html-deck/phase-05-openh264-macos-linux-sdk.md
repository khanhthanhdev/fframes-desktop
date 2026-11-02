---
phase: 5
title: "OpenH264 in the macOS and Linux SDK"
status: pending
priority: P2
effort: 2d
dependencies: []
---

# Phase 5: OpenH264 in the macOS and Linux SDK

## Goal

Give the macOS and Linux managed SDK an H.264 encoder, so Video export can write MP4/H.264
and the HTML deck export (phase 4) works on every platform, with the same output as Windows.

## Context

- The Unix SDK FFmpeg is configured with `--disable-autodetect` and no external libraries
  (`desktop/scripts/build-native-ffmpeg.py:21`); CI calls it from
  `.github/workflows/desktop.yml:94, 183`. Windows uses BtbN `win64-gpl-shared`, which has
  libx264 (`desktop.yml:194`).
- `ExportCodec::H264` maps only to `["libx264"]`
  (`fframes-studio-protocol/src/export.rs:74-79`); the worker lists a format only when one of
  its encoders exists (`fframes-studio-runtime/src/export.rs:22-34, 287-293`), and
  `encoder_params` sets `preset=veryfast` for libx264 (`:305-311`).
- Decision (2026-10-27): OpenH264 (BSD). Same MP4/H.264 output as Windows, the most compatible
  format (Safari, iOS, PowerPoint), no GPL switch. Building it ourselves is in the same patent
  position as the libx264 Windows ships today; Cisco's royalty coverage applies only to its
  own binaries.

## Requirements

- `build-native-ffmpeg.py` downloads a pinned OpenH264 release (tag and SHA-256 in the script),
  builds it static with `make` (needs `nasm` on x86_64; arm64 macOS uses its NEON assembly),
  installs it into a private prefix, and configures FFmpeg with `--enable-libopenh264` and
  `PKG_CONFIG_PATH` pointing at that prefix. `--disable-autodetect` stays.
- `ExportCodec::H264.encoders()` returns `["libx264", "libopenh264"]` (preference order).
- `encoder_params` passes no x264 options to libopenh264; it sets the bitrate from
  `video_bitrate` and `allow_skip_frames=0` so every frame is encoded (frame-exact decks).
- The SDK manifest records the OpenH264 version and its license file
  (`desktop/packaging/sdk/`), and the SDK ships OpenH264's `LICENSE` next to FFmpeg's.
- CI installs `nasm` on the Linux and macOS runners.

## Files

- Modify: `desktop/scripts/build-native-ffmpeg.py`, `.github/workflows/desktop.yml`,
  `desktop/packaging/sdk/phase-zero-sdk.json` (and its license list),
  `fframes-studio-protocol/src/export.rs`, `fframes-studio-runtime/src/export.rs`.

## Tasks & Steps

1. OpenH264 build step and FFmpeg configure flags.
2. Encoder list and `encoder_params`.
3. License and manifest entries.
4. CI: nasm, then build the SDK on macOS arm64 and Linux x86_64.

## Verification

- Protocol unit test: `H264.encoders()` order; existing `export.rs` tests unchanged.
- `fframes-studio-runtime/tests/export.rs` `every_reported_format_exports_at_the_requested_size_and_rate`
  on the macOS and Linux runners now covers `mp4/h264` (the worker reports it).
- `ffprobe` on an exported file: codec `h264`, profile constrained baseline or main, frame count
  equals the video's; the file plays in Safari (macOS runner) and Firefox.

## Success criteria

- [ ] The macOS and Linux SDKs report `mp4/h264` in export capabilities.
- [ ] A deck exported on macOS plays in Safari and holds on its stop frames.

## Risks

- **Quality per bitrate** is lower than x264; raise the bitrate for OpenH264 if the deck
  comparison test in phase 4 shows visible artifacts.
- **Build tool on runners:** a missing `nasm` fails the SDK build early with a clear message.
