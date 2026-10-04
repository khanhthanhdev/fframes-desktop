---
title: "Phase 4: Audio clock and milestone qualification"
status: in-progress
---

# Phase 4: Audio clock and milestone qualification

## Context, outcome and ownership

Priority P2; implementation complete locally, qualification in progress. Depends on [worker mix contract](phase-01-start.md), [atomic preview coordination](phase-02-revision-safe-preview-coordination.md) and [timeline controls](phase-03-timeline-model-and-native-controls.md). App-owned audio output and predicted-output-clock video scheduling are implemented; physical timing and additional native platforms remain unqualified. No installers/export UI, agent transactions, new toolkit or Skia/native-player dependency.

## Verified starting points

The existing native-player uses CPAL 0.18 and keeps audio private (`/root/fframes-desktop/fframes-native-player/Cargo.toml:21`, `/root/fframes-desktop/fframes-native-player/src/audio.rs:47`). Its feeder uses the encoder AudioMixer (`:114`), reset seeks in sample space (`:87`), and callback consumes queued samples (`:189`). Reuse dependency version/design evidence, not private winit/Skia player service. Its callback currently ignores output timestamp (`:189`); queued/feed sample count is not proof of audible device position and must not become Studio's presentation clock.

Core mixer access/rate reconciliation is established in Stage 1 (`/root/fframes-desktop/fframes/src/audio_mix.rs:390`, `:565`). CI currently runs root protocol/runtime tests plus the separate desktop workspace and real fixture/managed/portable worker gates (`/root/fframes-desktop/.github/workflows/desktop.yml:125`, `:127`, `:136`, `:140`, `:142`). M1 follow-up evidence is `/root/fframes-desktop/plans/261002-0434-desktop-phase-one-foundation/phase-04-native-shell-and-foundation-verification.md:121`; start from these current tests, do not reopen resolved historical findings.

Stage 3 handoff (2026-10-03): compiled timeline/selection/thumbnail controls and provisional silent `TimelineTransport` are locally verified; replace that clock rather than adding a second playhead owner. The portable multi-scene fixture and `tests/support/preview_fixture.rs` already exist, including deterministic stereo WAV generation and an explicitly run real SDK test in `timeline_controls.rs`. Extend them for alpha/shader/audio-clock qualification rather than duplicating scaffolding. Native controls, shorter rebuild clamp and close cleanup have Linux X11 software-rendering evidence in the Stage 3 phase file; no live audio or sustained M2 measurement exists yet.

## Requirements and data flow

Data flow: selected output device/rate → Stage 1 prepared matching PCM → bounded artifact reader/resampler → fixed playback ring → CPAL callback → output-clock snapshot → engine playback position → frame-pump latest target → identity/serial guarded GPUI presentation. No callback mixing, file/network I/O, control protocol, allocation, logging or unbounded mutex wait. Reader/preparation work happens off UI/callback thread. Keep the CPAL stream on a supported owner thread; if target stream is non-Send, create/control/drop on native app thread and move only bounded commands/atomics across threads. Validate against pinned CPAL API/target trait bounds during implementation.

Proposed app `AudioService` owns stream/device and revision-bound playback epoch. Use preallocated fixed SPSC storage or equivalent bounded nonblocking ring; do not add a general audio engine. Reader holds at most 2s/1 MiB PCM and callback ring at most 250ms negotiated-rate stereo; account both limits separately and reject unsupported extreme device rates. Fill buffer before starting. Convert supported device sample formats/channels in callback without allocations, stereo front pair/mono downmix and zero other channels. Preparation uses actual device sample rate when available; on device change, either reprepare same immutable revision at new rate or resample off callback thread with a tested bounded converter. Never reinterpret PCM byte rate or AudioMap sample timebase.

Clock is media sample position actually submitted for scheduled output plus callback/device playback timestamps and measured latency, not producer/enqueued samples or callback count alone. Publish epoch, media sample start/count, callback time, predicted DAC/playback time and validity in a bounded atomic snapshot. Calculate currently audible media position from timestamp mapping, clamped to submitted interval/video end. When a backend lacks usable timestamps, document/calibrate the submitted-buffer latency estimate, expose degraded clock capability and measure residual error. Do not claim exact hardware time from an estimate. The clock epoch changes on seek/revision/device/reset, rejecting all old callback/frame completions.

Play starts only after matching PCM window/first frame are ready. Pause freezes at audible position and invalidates queued future audio/video; use stream pause/reset or bounded device-buffer drain as supported, with measured residual latency. Seek stops old epoch, clears software ring and timestamp mapping, refills at target sample, requests matching target frame and resumes prior playing intent. Paused seek/step emits no audio. Seeking prepared sequential PCM does not restart AudioMixer limiter/fades. Mute zeros output while continuing consumption/media clock when the device is active; silent revision uses output clock where available. No output device uses a monotonic app clock, visibly indicates unavailable audio and preserves playback/seek/end controls. Device loss transitions once to monotonic fallback at last audible position; reconnect uses a new epoch/primed stream without a jump. Underrun emits silence, tracks missing samples, advances the chosen output timeline consistently and asks reader for current position rather than replaying stale samples; show buffering/degraded status, never silently drift video behind audio.

Scheduler samples clock and computes desired video frame by validated fps. Permit one render plus one latest desired request; drop expired frames rather than queue catch-up. Presentation guard compares identity, seek/clock epoch and desired media time after render completes. Thumbnail work yields to playback/scrub. End freezes at exact duration with final renderable frame, drains/stops audio once, clears outstanding work and leaves playhead/time display stable. Space/Play from end restarts at zero through the same seek epoch transition; no looping feature is added.

Successful rebuild while playing stages new worker/timeline/inspection/first frame/mix/window at current/clamped playhead. Before installation, re-evaluate current audio-clock position and newest seek; prime candidate at that position. UI/device handoff is one guarded epoch transaction: quiesce old epoch, switch matching ReadyPreview and new stream/ring, acknowledge installation, then release old mix/worker/images. If stream or readiness fails before commit, keep/restart old revision at its current position. After commit, output-device loss uses fallback for the new matching identity; never pair new video with old mix. Source/build errors leave old worker/artifact alive and usable. No accepted checkpoint advancement occurs.

## Files and actions (absolute paths)

| Action | File | Work |
|---|---|---|
| Modify | `/root/fframes-desktop/desktop/Cargo.toml` | Add CPAL 0.18 workspace dependency preserving GPUI pin. |
| Modify | `/root/fframes-desktop/desktop/app/Cargo.toml` | App audio dependency; no native-player/Skia/winit import. |
| Modify | `/root/fframes-desktop/desktop/Cargo.lock` | Resolve native audio dependency deterministically. |
| Create (proposed) | `/root/fframes-desktop/desktop/app/src/audio_service.rs` | Device/stream owner, bounded feeder/ring, clock snapshots and cleanup. |
| Create (proposed) | `/root/fframes-desktop/desktop/crates/studio-engine/src/playback_clock.rs` | GPUI-free clock mapping, epochs, end/pause/seek and frame scheduling. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/lib.rs` | Export clock model. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/preview_state.rs` | Atomic prepared mix/install clock epoch transaction (Stage 2-created). |
| Modify | `/root/fframes-desktop/desktop/app/src/preview_coordinator.rs` | Clock-driven deadlines/drop, staged windows and old-worker release (Stage 2-created). |
| Modify | `/root/fframes-desktop/desktop/app/src/studio_shell.rs` | Audio lifecycle/mute/device state/time/control hooks and close cleanup. |
| Modify | `/root/fframes-desktop/desktop/app/src/lib.rs` | Audio module wiring. |
| Modify | `/root/fframes-desktop/desktop/app/tests/preview_coordination.rs` | Extend Stage 2 test with real atomic audio/video handoff. |
| Create (proposed) | `/root/fframes-desktop/desktop/crates/studio-engine/tests/playback_clock.rs` | Deterministic output-time/latency/epoch scheduler tests. |
| Create (proposed) | `/root/fframes-desktop/desktop/app/tests/audio_preview.rs` | Bounded PCM/ring/service/real multi-scene integration. |
| Create (proposed) | `/root/fframes-desktop/desktop/fixtures/preview-timeline-audio/src/lib.rs` | Real Video with repeated/overlapping scenes, known color/alpha frames, timed audio cues and an explicit shader capability case. |
| Create (proposed) | `/root/fframes-desktop/desktop/app/tests/support/preview_fixture.rs` | Create a real temporary portable project from the Studio template, copy fixture source and generate deterministic WAV media; expose the same fixture for native qualification. |
| Create (proposed) | `/root/fframes-desktop/desktop/scripts/qualify-m2-playback.py` | Deterministic tracked native scrub/play/rebuild/resource harness. |
| Create (proposed) | `/root/fframes-desktop/desktop/qualification/m2-results.json` | Machine-readable actual evidence with pending platform gates. |
| Create (proposed) | `/root/fframes-desktop/desktop/qualification/m2-results.schema.json` | Separate M2 evidence contract for playback, audio timing, revision replacement, pixel parity and resource gates. |
| Modify | `/root/fframes-desktop/desktop/scripts/validate-qualification.py` | Dispatch explicitly identified M2 records to their own gate/schema validation; preserve legacy M0 records and evidence-hash checks. |
| Modify | `/root/fframes-desktop/desktop/scripts/test-packaging.py` | Regress legacy qualification and reject unsupported M2 kinds, absent evidence, invalid metrics and premature qualification. |
| Modify | `/root/fframes-desktop/.github/workflows/desktop.yml` | Audio prerequisites, focused/integration/native evidence artifact gates. |
| Modify | `/root/fframes-desktop/.github/workflows/desktop-phase-zero.yml` | Preserve legacy regression and supported native CI audio build prerequisites. |

Controller owns documentation updates after verified behavior: `/root/fframes-desktop/docs/desktop/README.md`, `/root/fframes-desktop/docs/desktop/architecture.md` section 8, and `/root/fframes-desktop/docs/desktop/implementation-plan.md` M2 status. Executor supplies evidence/limitations rather than modifying controller-owned docs concurrently. No duplicate details of dependency pins or generated manifests in prose.

## Implementation steps and tasks

1. Add deterministic clock/epoch tests for device timestamp vs enqueue latency, pause/seek/end and underflow. Verify pinned CPAL callback info and stream ownership on each supported target before threading design is fixed.
2. Implement off-thread bounded artifact reading and allocation-free callback ring, then main-thread-safe stream lifecycle/sample-format/channel handling.
3. Connect playback clock to frame pump and controls; measure late-frame dropping and muted/silent/no-device modes separately.
4. Complete atomic rebuild audio/video handoff with candidate priming, pause/seek during preparation and rollback before commit.
5. Create the shared real fixture through the test support helper: scaffold an ordinary portable project with the current template, replace its Video source, write reproducible stereo WAV cues and build through the managed SDK/materialization path. Use its compiled worker and ordinary CLI in integration tests and the native harness; retain onset/frame/sample evidence. Do not require unpublished development crates to resolve outside the SDK. Compare core mix, CLI pixels, color/alpha and explicit shader capability diagnostics.
6. Add bounded native stress harness and CI gates, track all background processes with PID/command/port/worktree, reuse deterministic display/ports and stop only owned processes. Current qualification validation hardcodes M0 gates; add an explicit M2 discriminator and separate gate/schema contract, retain legacy records without conversion, require evidence hashes for every pass, and test both record routes before admitting M2 evidence.
7. Run focused then broader checks; controller reviews cross-module/public-contract changes and updates owning docs from actual evidence.

- [x] Timestamp/latency/epoch model distinguishes predicted output position from enqueued samples.
- [x] CPAL owner and fixed callback/ring/feeder resource limits work on Linux virtual output.
- [x] Matching sample rate/mix, mute/silent/no-device/device-loss transitions have focused tests; virtual native playback is exercised.
- [x] Play/pause/playing and paused seek/frame-step/end/reset obey one clock epoch.
- [x] Video drops late frames and thumbnails yield; metadata/epoch guards reject stale presentation.
- [x] Successful rebuild switches audio/video together; failed build/preparation preserves playable old revision.
- [x] Real multi-scene, alpha/color/CLI parity and explicit shader fallback evidence is retained.
- [x] Stress resource, local regression, process cleanup and documentation handoff gates pass with qualification limits recorded; CI wiring is prepared, not remotely executed.
- [ ] Physical A/V timing, timestamp residual and pause/drain latency are measured; actual unplug/reconnect is qualified.
- [ ] Windows/macOS native stream, device and interactive behavior are qualified.

## Verification and measurable success

Commands from repository root; native FFmpeg/CPAL development prerequisites must match current CI (Linux ALSA development headers needed when adding CPAL). Start narrow, proposed new targets after creation:

```sh
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test playback_clock
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test audio_preview
cargo test --locked -p fframes-studio-protocol -p fframes-studio-runtime
cargo test --locked --manifest-path desktop/Cargo.toml --workspace
cargo fmt --all --check
cargo fmt --manifest-path desktop/Cargo.toml --all --check
cargo clippy --locked -p fframes-studio-protocol -p fframes-studio-runtime --all-targets -- -D warnings
cargo clippy --locked --manifest-path desktop/Cargo.toml --workspace --all-targets -- -D warnings
python3 desktop/scripts/test-packaging.py
python3 desktop/scripts/validate-qualification.py desktop/qualification/m0-results.json
```

Run existing real-worker regressions explicitly after building fixture and assembling compatible SDK (existing commands, not new targets):

```sh
cargo build --release --manifest-path desktop/fixtures/annotated-video-overlay/Cargo.toml
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test worker_roundtrip -- --ignored
SDK_BUNDLE=<fresh-compatible-sdk> cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test managed_worker -- --ignored
SDK_BUNDLE=<fresh-compatible-sdk> cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test portable_managed_worker -- --ignored
```

Angle-bracket paths are placeholders to replace before running. After creation, execute proposed `audio_preview` ignored real-device/SDK scenarios explicitly, retaining measured DAC/output-clock results; a mocked clock unit test or ignored default-suite test is not live-audio evidence. Native harness CLI syntax is designed with the script; do not invent a runnable command before it exists. No editor/WASM test is required unless scope actually touches an editor bridge.

After the M2 record, schema and validator route exist, run `python3 desktop/scripts/validate-qualification.py desktop/qualification/m2-results.json` alongside the unchanged M0 record check. Validate metadata/gate structure and referenced evidence, and keep missing physical-device/platform evidence pending.

| Level | Scenarios | Mechanical gate |
|---|---|---|
| Unit | Submitted vs queued samples, callback timestamp/latency, backward/device timestamps, epoch reset, underflow, pause/seek/end, rate/channel conversion | No stale epoch accepted; checked time/range; position never exceeds duration. |
| Integration | Real sequential mix with duck/fade/offset/limiter/resampling; bounded reads; malformed descriptor/disk full; prepare cancel; rate change | Core parity within defined numerical tolerance, exact sample/byte counts, no incomplete mix installed. |
| Integration | Rebuild while playing/paused/seek, shorter/empty clip, candidate/device failure, external media edit, obsolete frames/audio callbacks | Displayed frame/report/mix identity always matches; failure retains old source-independent worker. |
| Native | Multi-scene audio playback, paused/playing scrub, frame step, zoom/range/overlap/thumbs/time, mute, unplug/reconnect, end/replay | Actual matching sound/video and correct control states, no stuck output/process. |
| Presentation | Known straight-alpha/color frames and selected-frame CLI CPU parity; shader example | Defined pixel tolerances pass; CPU shader gap visibly diagnosed. |
| Resources | 10-minute repeated scrub/play session, 2,000 seeks and 50 rebuilds with failures after warm-up | Queue/cache high-water within stage limits; no image release failures/owned orphan processes; post-close PCM/artifact leases gone. |

Proposed performance qualification defaults: 1280×720 30 fps real fixture; paused seek p95 ≤150ms after warm-up; A/V error ≤one video frame plus measured timestamp/latency residual, with no cumulative growth over 10 minutes. Record achieved FPS/render latency/drop counts, output latency/underruns and hardware/environment. Track host RSS after warm-up; proposed slope ≤2 MiB/min with input media held constant, plus fixed cache high-water checks. These are engineering gates to measure, not inherited user promises. If CPU workload misses them, profile/downscale within contract and report limits; do not silently replace framework/backend or pretend dropped frames establish performance. GPUI and renderer acceleration remain separate.

## Risks, security, rollback and finish

High (medium × high): callback timestamp is mistaken for feeder position, creating drift. Mitigate explicit output-time model, synthetic latency tests and real-device measurements. High (medium × high): non-Send stream or blocking callback fails on another OS. Mitigate native-owner lifecycle, nonblocking ring and native CI compilation/target tests. High (medium × high): rebuild mixes epochs/revisions. Mitigate precommit prime, guarded atomic installation and old-resource retention until acknowledgment. High (medium × high): audio device failure destroys old playable preview. Mitigate monotonic fallback and resumable precommit rollback. Medium: physical timing proof unavailable in software-rendered/headless environment; retain pending evidence instead of broad qualification claims.

Callback reads only validated PCM in preallocated storage; no worker filenames/shell operations or secrets enter output code. On close, stop stream before releasing ring/artifact, cancel reader/pump, reap owned workers/builds and release GPUI images; cleanup after a timeout must remain bounded and attributable.

Rollback live-audio route to clearly labeled muted/no-device monotonic preview; retain Stage 2/3 functionality and existing spike. Revert CPAL/CI dependency additions together if needed, preserve all project/history/SDK bytes and qualification evidence. M2 is complete only when all original roadmap bullets and measurable gates have implementation evidence. Keep physical GPU/IME, macOS/Windows interaction and authenticated ACP qualification open unless independently proven; no consumer release, export UI, commit or publication is implied.

## Unresolved questions

No blocking product question. Actual native output latency/target performance and pending platform qualification require execution evidence, not renewed approval of the accepted Linux development path.

## Local implementation and verification — 2026-10-03

`PlaybackClock` replaces the provisional TimelineTransport. It maps submitted sample intervals against predicted callback/playback time, rejects stale/backward epochs, handles pause/seek/end/replay and exposes a monotonic no-device fallback. A dedicated CPAL 0.18.2 owner controls native streams; a 64 KiB positioned reader and tested windowed-sinc resampler feed an atomic SPSC ring capped at 250 ms (PCM plus per-frame metadata are accounted separately). Callback tests count allocations, distinguish asymmetric channel conversions, force underrun/stale packets and preserve the final EOF timestamp anchor.

The host retains a validated open PCM file with its materialization lease. Source corruption is a failed preparation, not an unavailable-device fallback. New readiness is published only for the wanted epoch; mute consumes the cursor, pause/seek invalidates output, and the shell waits for matching first-frame/audio readiness before activation. Candidate epoch reservation also advances the active clock so a concurrent seek cannot collide with a candidate callback. Cancellation and rejected precommit readiness resume the old revision without advancing its checkpoint.

Explicit real SDK regressions pass for failed-build/re-prime/cancel/shorter-worker coordination, compiled timeline/thumbnail destinations, ordinary CLI RGBA at frames 0 and 137, same-size dynamic geometry at frame 19, straight-alpha/color swatches and sequential 44.1 kHz WAV/PCM parity. Maximum PCM difference is 0.0000460409 and mean difference is 0.000009305; the output has 242,550 stereo frames. The real CPAL regression ran against private virtual output and passed output snapshots, mute consumption, pause, superseded seeks, fallback, corrupted PCM rejection and teardown. Managed installation, source/SDK relocation and legacy worker crash/restart tests also pass explicitly.

The native harness uses real pointer/key input, checks identity/epoch and every sampled queue/cache bound, measures seek-to-image-submission latency and RSS, and strictly inventories close cleanup. Image submission is not physical presentation. Its initial full run completed 2,001 seeks and 50 rebuilds (five deliberate failures), but failed RSS at 4.99 MiB/min and left one final PCM/build lease on window close. That evidence is preserved, not relabeled as a pass. The final window previously released the shell before its weak quit hook could run; the app now retains the root through awaited teardown. CPU frame scale now follows the pane's physical pixels, preserving aspect ratio and the 1280×720/no-upscale caps rather than repeatedly allocating full-size pixels for a small pane.

The subsequent short native run passes controls and clean window-close teardown with no owned process or PCM/materialization lease. Its inspected paused image, cursor and footer all show frame 100 / 3.333 s, and shader fallback is visible. It is a smoke check, not sustained RSS qualification. Final sustained measurements are recorded in the M2 qualification ledger after their own run. CI changes are prepared locally; no remote workflow was triggered. Physical DAC/presentation timing, hardware unplug/drain latency, other native platforms, GPU/display/IME and authenticated ACP remain pending. All changes remain local, uncommitted and unpushed.

## Final sustained evidence — 2026-10-04

The corrected full native run passes: 600.276649 seconds, 2,001 confirmed seeks, 50 paused/playing rebuilds and five deliberate failures. Warm seek-to-image-submission p95 is 96.232490 ms and host RSS slope is 1.228500 MiB/min, within the 150 ms / 2 MiB/min gates. The 1280×720 / 30 fps fixture renders at pane-sized CPU resolution (default 718×132 physical extent, scale 0.183333), with unchanged aspect ratio and protocol caps. Every sampled bound passes: render queue high-water 1, thumbnail queue 12, decoded-thumbnail high-water 14 entries / 806,400 bytes and one resident/queued main image. The pump records 10,358 renders and 13 late frames; these are not physical presentation measurements.

Graceful close reaps every owned process and observed child, releases PCM/build leases and reports zero image release failures. An isolated native resize changes scale from 0.183333 to 0.529688 without changing generation or playhead. Its inspected paused image, status and footer agree at frame 70 / 2.333 s. At the default short window with an error banner, real wheel input exposes the complete lower timeline, thumbnails and footer while the failed-build banner and playable prior revision remain visible. Both additional sessions close cleanly.

The [hashed qualification ledger](../../desktop/qualification/m2-results.json) and [retained evidence/reproduction commands](../../desktop/qualification/evidence/linux-m2-2026-10-04/README.md) distinguish these passes from physical timing and platform gates. The earlier failed full run remains in the evidence set. Final checks cover 150 nonignored desktop tests, 17 protocol/runtime tests, 61 core tests, 14 packaging tests, explicit real SDK/virtual CPAL regressions, warnings-denied desktop/runtime/core-library Clippy and both workspace formatting checks. Core all-targets Clippy retains two pre-existing warnings in the untouched yuv420 conversion benchmark.

All four M2 stages are implemented and locally verified. This phase remains `in-progress` only for physical DAC/presentation, unplug/drain timing and Windows/macOS qualification. No A/V error or timestamp residual was invented; no publication or remote CI run occurred.
