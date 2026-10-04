# Linux M2 local implementation evidence

All implementation is local, uncommitted and unpushed. The actual GPUI executable and installed SDK ran on Linux x86_64, X11/Xvfb software rendering and a private 48 kHz stereo PulseAudio null sink. This qualifies the recorded control/resource checks, not physical DAC/display timing, GPU/IME behavior, Windows/macOS or a release. No remote CI workflow was triggered.

## Sustained run passes after two fixes

The full run lasted 600.276649 seconds, with 2,001 confirmed seeks and 50 paused/playing rebuilds, including five deliberate compile failures. Warm paused seek p95 was 96.232490 ms **to image submission**, not physical presentation. Host RSS slope was 1.228500 MiB/min, below the 2 MiB/min gate. The fixture is 1280×720 at 30 fps; CPU frames fit the pane's physical pixels rather than rendering at full fixture resolution. The default pane was 718×132 pixels, scale 0.183333; the failed-build banner reduced its available height.

Every sampled queue/cache bound passed: one render request, at most 12 thumbnail requests, thumbnail high-water 14 entries / 806,400 bytes, one resident/queued main image. The pump recorded 10,358 renders, 13 late frames and 27.619331 ms maximum render time. Graceful window close left no owned processes, orphaned children or PCM/build leases; image release failures were zero. Compilation caches are not active materialization leases.

`raw-measurements.json` records measurements and process/cleanup inventories; `telemetry-samples.json` preserves the sampled revision, epoch, audio, queue and image observations. Physical A/V error and timestamp residual are null in the raw record and absent from the qualification ledger. Final audio metrics may be null at the end boundary; earlier samples and the real CPAL test record active output.

`before-adaptive-raw.json` preserves the earlier failed full run: RSS slope 4.986848 MiB/min and one retained PCM/build lease despite process exit. The fixes retain the root entity through awaited quit cleanup and render CPU frames at pane resolution. That failed run is not counted as passing evidence.

## Native screenshots and integration evidence

- `native-paused.png` was inspected: preview, cursor and footer all show frame 100 / 3.333 s, without stretching; controls, tracks and shader notice are visible. The thumbnail row is still warming immediately after its last rebuild; the later resized and scrolled captures show populated thumbnails.
- `native-resized-paused.png` and `result.json` record resize from scale 0.183333 to 0.529688 while preserving generation and position. The inspected paused image/status/footer agree at frame 70 / 2.333 s. The rightmost ruler label is partially clipped at the horizontally cropped viewport edge; preview and transport are not clipped.
- `native-window.png` is the final stress capture, with the deliberate build failure still reported and prior preview playing. Its short window has a scrollable center pane; lower timeline content is initially outside the viewport. Frame labels during playback do not prove physical presentation timing.
- `native-failed-scrolled.png` and `scroll-result.json` verify real wheel input exposes the complete lower timeline, thumbnails and frame 99 / 3.300 s footer in that short-window failed-build state. The error banner remains visible, the prior generation remains paused/playable and close leaves no PCM/build lease. Content above the viewport is intentionally scrolled out, not inaccessible.
- `parity.json` and `m2-parity-final.log` record exact ordinary CLI CPU RGBA parity at frames 0 and 137, independent dynamic geometry checks after thumbnail/full rendering and at same-size frame 19, straight alpha 128, color swatches and the explicit shader gap. Prepared PCM versus sequential 44.1 kHz stereo CLI WAV has 242,550 frames, maximum difference 0.0000460409 and mean difference 0.000009305. Host resampling produces 264,000 stereo frames at 48 kHz.
- `m2-cpal-final.log` records the explicitly executed real virtual-output regression: submitted-sample clock, mute consumption, pause, superseded seeks, no-device fallback, corrupted PCM rejection and cleanup.
- `m2-real-retry.log` records passing real failed-build/invalid-source coordination. It also retains a subsequent historical timeline test failure caused by a test-held lease. `m2-timeline-final.log` is the corrected passing timeline rerun after explicitly releasing that test consumer; cleanup assertions remain unchanged.
- Remaining logs record 150 nonignored desktop tests, 17 protocol/runtime tests, 61 compile-time-tree core tests, 14 packaging tests, legacy worker roundtrip and warnings-denied desktop/root-runtime/core-library Clippy. Managed install and source/SDK relocation were also executed. Root and desktop formatting pass. Core all-targets Clippy still has two pre-existing warnings in the untouched `yuv420_conversion` benchmark; that broader gate is not reported as green.

## Reproduction from the repository root

Use a fresh disposable directory. SDK assembly and the first real worker compilation are substantial; the native driver rejects an occupied X display and restores the fixture source after deliberate failures.

```bash
scratch="$(mktemp -d /tmp/studio-m2-XXXXXX)"
python3 desktop/scripts/assemble-phase-zero-sdk.py --out "$scratch/sdk"
SDK_BUNDLE="$scratch/sdk" \
  AUDIO_PREVIEW_FIXTURE_ROOT="$scratch/video" \
  AUDIO_PREVIEW_SDK_HOME="$scratch/home/.fframes/sdk" \
  AUDIO_PREVIEW_EVIDENCE="$scratch/parity.json" \
  cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio \
    --test audio_preview real_sdk_frame_audio_and_retained_source -- --ignored
cargo build --locked --manifest-path desktop/Cargo.toml -p fframes-studio --bin fframes-studio
FFRAMES_SDK_BUNDLE="$scratch/sdk" \
  python3 desktop/scripts/qualify-m2-playback.py \
    --application desktop/target/debug/fframes-studio \
    --project "$scratch/video" --sdk-home "$scratch/home" \
    --out "$scratch/native" --virtual-audio --inject-build-failures
python3 desktop/scripts/validate-qualification.py desktop/qualification/m2-results.json
```

The recorded run used `/tmp/studio-m2-sdk-stage3`, `/tmp/studio-m2-native-final`, `.amp/in/stage3-home` and `.amp/in/m2-qualified-run`, with otherwise identical driver defaults. The ledger hashes its gate evidence; these records should not be reformatted or substituted for measurements on another system.
