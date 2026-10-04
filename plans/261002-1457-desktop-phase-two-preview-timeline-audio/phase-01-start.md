---
title: "Phase 1: Worker preview contract"
status: completed
---

# Phase 1: Worker preview contract

## Context, outcome and ownership

Priority P2; implemented and locally verified. Depends on current M0/M1 baseline. The worker contract and generated SDK gate are available to Stages 2–4. Read [M2 scope](../../docs/desktop/implementation-plan.md#3-milestones-and-acceptance-criteria), [architecture section 8](../../docs/desktop/architecture.md#8-preview-timeline-and-audio), [M1 evidence](../261002-0434-desktop-phase-one-foundation/phase-04-native-shell-and-foundation-verification.md).

Deliver a GPUI-free persistent worker with trustworthy metadata, inspection, scaled pixels and immutable prepared audio. Preserve existing worker entry/public v1 structs and the M0 spike. No export operation/UI, new renderer/framework, generalized source registration or streaming mix preparation is required.

## Verified starting points

- Existing `serve_worker` holds `Previewer` and CPU renderer across requests, but hello metadata is hardcoded and requests are not identity-validated (`/root/fframes-desktop/fframes-studio-runtime/src/worker.rs:94`, `:103`, `:119`). Current protocol is v1 with 64 MiB frame cap (`/root/fframes-desktop/fframes-studio-protocol/src/lib.rs:4`, `:5`); preserve that legacy contract.
- Core access already exists: resolved timeline/media/options/scale/inspect at `/root/fframes-desktop/fframes/src/renderer/preview.rs:306`, `:358`, `:362`, `:372`, `:480`. Use these, with no new context accessor. Core track report carries mix options (`:200`), currently dropped by worker timeline projection (`/root/fframes-desktop/fframes-studio-runtime/src/worker.rs:149`).
- `AudioMixer::new_rescaled` reconciles timeline/output rates and `render` maintains sequential mix state (`/root/fframes-desktop/fframes/src/audio_mix.rs:390`, `:565`). Avoid `render_all` (`:618`) for unbounded projects.
- Runtime external entry consumers are template `/root/fframes-desktop/desktop/crates/studio-project/templates/src/bin/studio_worker.rs:16`, overlay `/root/fframes-desktop/desktop/fixtures/annotated-video-overlay/src/main.rs:158`, and generated overlay source embedded by `/root/fframes-desktop/desktop/crates/studio-sdk/src/project.rs:73`; in-module lifecycle regression is `/root/fframes-desktop/fframes-studio-runtime/src/worker.rs:281`.

## Requirements and architecture

Define proposed `PreviewIdentity { project_id, open_session, source_revision, worker_generation }` as a protocol value with no engine dependency. Every M2 command, response, diagnostic and bulk descriptor includes this identity and monotonic request ID; frame requests additionally carry seek serial. One immutable identity is bound at launch, not copied from arbitrary caller requests. Reject wrong version/identity/request/range before rendering, allocation or bulk writes; echoed errors remain attributable.

Retain legacy `serve_worker` and v1 request/response shapes. Introduce proposed explicit `serve_preview_worker` and preview protocol module with capability negotiation (`preview_identity_v1`, `scaled_frame_v1`, `inspect_v1`, `prepared_audio_v1`) and a fixed negotiated contract version. Do not globally change `CURRENT_PROTOCOL_VERSION`. New hello has offered/supported versions, identity, actual framework/runtime/SDK versions, selected backend, sizes, sample formats/rates and capability gaps. Missing capabilities fail with an actionable SDK/bridge diagnostic; the M0 app route continues to use the legacy entry. Imported projects are never silently rewritten to adopt M2. SDK compatibility digest (`/root/fframes-desktop/desktop/crates/studio-sdk/src/manifest.rs:154`) is a portable project pin: absent optional fields must be omitted from legacy typed serialization, preserving exact old JSON digest. Test a checked-in legacy manifest/digest before changing fields. New capabilities/packages get new SDK IDs, pins and checksums; never mutate existing installed SDKs or project pins.

Data flow: tagged command → validation → persistent Previewer/renderer or chunked AudioMixer → tagged control descriptor → bounded binary payload → app validation. Use existing length-prefixed control and loopback connection. M2 binary records add explicit kind/identity/request/offset/length framing so frame bytes cannot be interpreted as audio. Only one serialized bulk write is active; receiver drains a validated record completely or closes/reaps the connection. Keep logs on bounded stderr.

Scaled frame requests use finite positive bounded scale/output dimensions through `set_scale`; original timeline dimensions remain unchanged. Validate checked stride/payload arithmetic and actual requested frame/size; preserve straight RGBA/sRGB contract. Preview maximum is proposed 1280×720, aspect-preserving, no upscaling; full-size reference/CLI tests may request within the existing frame cap. Inspect explicitly bounded frame lists/ranges (including scene and overlap boundaries) and structured severity/capability diagnostics; clamp work and diagnostics rather than serialize an unlimited report.

Prepared audio is whole-revision stereo interleaved little-endian f32 PCM at a negotiated output sample rate (48 kHz when no device). Use the exact resolved audio map, media provider and `AudioMixOptions`, with `new_rescaled` if needed. Render sequential fixed chunks from sample zero through video end, preserving limiter/lookahead, ducking, fades, offsets, pan/resampling and de-click behavior. Write to an internal temporary disk artifact, finalize atomically, and expose an opaque ID + identity/rate/channels/sample_count/byte_count/checksum descriptor. An explicit silent descriptor is valid only for an actually silent/no-audio revision; missing files/mix errors are diagnostics, never disguised silence.

Proposed bounds: 1 MiB control messages; 64 MiB frame records; 256 KiB audio reads; 4 GiB prepared audio per revision with checked upfront size; two retained artifacts (displayed/candidate), total prepared PCM disk budget 8 GiB including temporary files; retain PCM on the worker side and send only bounded playback windows to the app; do not duplicate full disk artifacts in the app, and count temporary overlap explicitly; preparation deadline 120s configurable for qualification. Exceeding bounds fails preparation while preserving old preview; no whole-mix Vec or shared-memory adoption. Support bounded offset reads and cancel/release. A dedicated bounded control reader (capacity 2) permits the main worker to process cancellation/shutdown between fixed mixing chunks; no core Video/media is transferred to callback/UI threads. On blocking I/O/deadline, client closes/reaps worker. Cleanup temp/final artifacts on failure/release/exit; expose progress without unbounded events.

## Files and actions (absolute paths)

| Action | File | Work |
|---|---|---|
| Modify | `/root/fframes-desktop/fframes-studio-protocol/src/lib.rs` | Re-export additive preview contract while preserving legacy types/tests. |
| Create (proposed) | `/root/fframes-desktop/fframes-studio-protocol/src/preview.rs` | Identity, negotiated operations, bulk/audio headers and checked validation. |
| Modify | `/root/fframes-desktop/fframes-studio-runtime/src/worker.rs` | Preserve legacy serving wrapper; shared bounded transport primitives. |
| Modify | `/root/fframes-desktop/fframes-studio-runtime/src/lib.rs` | Export explicit preview entry/config. |
| Create (proposed) | `/root/fframes-desktop/fframes-studio-runtime/src/preview_worker.rs` | Persistent preview operations and chunked disk mix lifecycle; follow existing Rust module convention. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-project/templates/src/bin/studio_worker.rs` | Explicit M2 launch identity/config with compatible legacy mode. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-project/templates/Cargo.toml` | Compatible bridge dependency pins if API packaging requires new versions. |
| Modify | `/root/fframes-desktop/desktop/fixtures/annotated-video-overlay/src/main.rs` | Preserve legacy worker path; add explicit preview mode for SDK qualification. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-sdk/src/project.rs` | Keep embedded fixture generator compatible; add launch contract checks. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-sdk/src/manifest.rs` | Default missing worker-version/capability declarations to legacy v1; omit absent optional fields when serializing to preserve legacy canonical digests; validate supported range. |
| Modify | `/root/fframes-desktop/desktop/packaging/sdk/phase-zero-sdk.json` | Declare actual supported contracts/capabilities; retain existing SDK compatibility. |
| Modify | `/root/fframes-desktop/desktop/scripts/assemble-phase-zero-sdk.py` | Package new module graph, verify both legacy and M2 real-worker operations offline. |
| Modify | `/root/fframes-desktop/desktop/scripts/test-packaging.py` | Compatibility-manifest/artifact regression coverage. |
| Create (proposed) | `/root/fframes-desktop/fframes-studio-runtime/tests/preview_worker.rs` | Real Video/scene/audio protocol and artifact integration. |

Root `/root/fframes-desktop/Cargo.toml`, `/root/fframes-desktop/Cargo.lock`, protocol/runtime Cargo manifests and `/root/fframes-desktop/desktop/Cargo.lock` change only if dependency/version updates are necessary. Keep project manifest schema/portable files untouched. Stage 2 owns app consumers; Stage 4 owns CI integration. Re-grep constructor/entry consumers before executing; this is the verified inventory, not permission to omit added callers.

## Implementation steps and tasks

1. Run current protocol/runtime/SDK/project regressions. Add additive contract tests first; enumerate serialized constructors and compatibility manifests before changing shared values.
2. Implement preview identity/capability negotiation and validation without altering legacy entry behavior. Derive build versions from compiled/package/SDK data instead of placeholder strings.
3. Implement persistent timeline projection, scales and bounded inspection, including mix metadata and stable scene-instance IDs scoped to revision (repeated scene types remain distinct).
4. Implement binary record framing and chunked PCM artifact preparation/read/cancel/release; verify requested and timeline sample rates consistently.
5. Update template/fixture/generator/assembler and SDK compatibility declarations together. Keep explicit old bridge mode tested and surface unsupported old project bridge in product setup.
6. Exercise real scenes/audio and compare prepared PCM against the same mixer path; document error/cancel behavior and measurements for Stage 2.

- [x] Current baseline is recorded; legacy v1 fixtures and tests remain unchanged in behavior.
- [x] Identity/version/capability negotiation and attributable errors reject malformed/stale input.
- [x] Persistent timeline exposes all scene instances, overlap ranges and audio mix metadata.
- [x] Scaled frames and bounded structured inspection validate arithmetic and request identity.
- [x] Tagged bulk transport bounds/corruption/deadline behavior are tested.
- [x] Sequential disk preparation matches AudioMixer and supports bounded reads/cancel/release.
- [x] Template, overlay, generated source, SDK pins/manifest and assembler stay compatible.
- [x] Runtime/protocol/offline generated-worker gate and bounded resource regression evidence pass.

## Verification and measurable success

Run from `/root/fframes-desktop`; prerequisites match existing native FFmpeg/SDK setup. Narrow first:

```sh
cargo test --locked -p fframes-studio-protocol -p fframes-studio-runtime
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-sdk -p studio-project
python3 desktop/scripts/test-packaging.py
```

Proposed integration target, runnable only after creation: `cargo test --locked -p fframes-studio-runtime --test preview_worker`. SDK assembly uses existing `python3 desktop/scripts/assemble-phase-zero-sdk.py --out <fresh-sdk-dir>` with native prerequisites; never overwrite an installed SDK. The existing M0 worker tests are explicitly executed after fixture/SDK preparation in Stage 4.

Matrix: unit serialization/legacy JSON/unsupported-version/overflow/NaN/size/identity; runtime integration repeated scenes, overlap endpoints, out-of-range frame, unknown operation, panic, missing media, bounded diagnostics, alpha/scale, audio fade/duck/limiter/offset/resampling parity, final sample count and checksum; transport integration truncated/oversized/reordered record, stall and disconnect; resource integration cancel during preparation, disk full, release twice and shutdown. Done means matching identity across all operations, bytes within stated bounds, no incomplete artifact presented, and legacy v1 regressions pass. No test execution is claimed here.

## Risks, security, rollback and next

High (medium likelihood × high impact): protocol migration breaks generated projects. Mitigate explicit additive entry, legacy default SDK fields, source-preserving diagnostics and old-mode regressions. High (medium × high): sample-rate/limiter mismatch changes audible mix. Mitigate sequential full mix and same-mixer numerical tests; never reset limiter at transport-chunk boundaries. High (medium × high): cancellation blocked in mix/I/O. Mitigate bounded chunks/control reader, preparation budget and supervised kill cleanup. Medium: disk artifacts exceed budget; check required bytes/free space early, enforce written-byte and total-cache accounting, handle mid-write disk exhaustion and retain old preview.

Project code/build scripts execute code; process separation remains no security sandbox. Do not accept worker-supplied absolute paths for app reads. Artifact IDs resolve only within the revision's private app/runtime cache; validate offsets, sums and lengths before allocation. Treat diagnostics as bounded untrusted text.

Rollback additive preview entry/template/manifest/assembler as one compatible set; legacy serving remains available. Delete only unreferenced new cache artifacts; never remove portable source/checkpoints or old SDK. Transfer listed shared files and verified contract to Stage 2 only after this gate.

## Local implementation evidence — 2026-10-03

The additive preview entry is implemented and locally verified without changing the legacy protocol version or canonical legacy SDK manifest digest. It retains the concrete Video/media/CPU renderer, validates revision identity and request counters, returns compiled scene/audio reports and bounded inspection, and prepares stereo PCM sequentially into leased disk artifacts. Templates, overlay generation and the assembler negotiate the new capability explicitly; existing pins are not rewritten.

The fresh `/tmp/studio-m2-sdk-stage3` assembly built two offline generated workers and exercised its preview probe. Root protocol/runtime tests pass (17 tests); managed-worker installation, portable SDK/source relocation and the legacy annotated-worker frame/anchor/crash/restart regressions were run explicitly and pass. Real SDK parity checks cover full RGBA equality at frames 0 and 137, thumbnail → main scale changes, same-size dynamic geometry at frame 19, exact alpha/color swatches and the explicit CPU shader capability gap. Prepared 44.1 kHz PCM matches the ordinary CLI WAV within independent 16-bit quantization/dither tolerances, with 242,550 stereo frames and the specified sample onset. Host 48 kHz preparation has 264,000 stereo frames and 2,112,000 bytes.

Stage 2/4 regressions verify retained reads after worker retirement, stale-result rejection and release of the final PCM/materialization consumers. Tests explicitly drop their final ReadyPreview handles before asserting cleanup; retaining those handles is intentionally a live lease, not a leak. The sustained app resource and physical timing gates remain owned by Stage 4. Fault and scheduling coverage is representative, not exhaustive disk-full or model-checking evidence. All changes remain local, uncommitted and unpushed.
