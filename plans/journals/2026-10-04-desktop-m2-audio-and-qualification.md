---
title: Desktop M2 audio and local qualification
date: 2026-10-04
summary: All four M2 stages implemented locally; sustained Linux virtual-output gates pass, physical/platform qualification remains open.
---

# Desktop M2 audio and local qualification

Completed the authorized Phase 2 continuation on the existing local revision-safe worker and timeline work. App-owned CPAL output and a predicted submitted-sample clock now drive playback, with epoch-guarded atomic first-frame/audio installation, pause/seek/end/replay, mute, no-device fallback and device retry. Positioned reading, resampling and fixed SPSC storage keep allocation, locking and I/O off the audio callback. Corrupted PCM rejects preparation rather than pretending the device is unavailable.

Real SDK regressions exposed test-held materialization leases; explicitly releasing those consumers preserved the cleanup assertions. The first full native run then exposed a real last-window cleanup ordering issue and excessive full-resolution image churn. Retaining the root entity through awaited quit and fitting CPU frames to physical pane size resolved them without changing GPUI, the CPU backend or the legacy worker route.

The final 600.276649-second native/virtual-output run passes 2,001 confirmed seeks and 50 rebuilds, including five deliberate failures. Warm seek-to-image-submission p95 is 96.232490 ms, RSS slope is 1.228500 MiB/min, all sampled queue/cache bounds pass, and close leaves no owned/orphaned process or PCM/build lease. Resized, paused and failed-build scrolled states were rendered and inspected; preview/status/footer agree in paused states. The failed earlier run is retained alongside the corrected evidence.

Verification: 150 nonignored desktop tests, 17 protocol/runtime tests, 61 core tests, 14 packaging tests, explicit managed-install/relocation/legacy-worker/coordination/timeline/parity/virtual-CPAL regressions, warnings-denied desktop/runtime/core-library Clippy, root/desktop formatting and hashed M0/M2 qualification validation. CLI CPU pixels agree; sequential stereo PCM maximum difference is 0.0000460409. Core all-targets Clippy has two pre-existing warnings in the untouched yuv420 conversion benchmark.

Plans and owning docs describe actual local implementation and measured limits. Durable evidence and reproduction commands live in `desktop/qualification/evidence/linux-m2-2026-10-04/`; the ledger keeps physical A/V error/residual, unplug/drain timing and other native platforms pending. No physical GPU/IME or authenticated ACP qualification is claimed. CI wiring is prepared but not remotely run. All changes remain local, uncommitted and unpushed; no release or publication occurred.

> Historical work record — not durable authority. Prefer docs/specs/ADRs and the qualification ledger for current decisions.
