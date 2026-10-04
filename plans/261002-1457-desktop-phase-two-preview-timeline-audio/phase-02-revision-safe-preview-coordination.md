---
title: "Phase 2: Revision-safe preview coordination"
status: completed
---

# Phase 2: Revision-safe preview coordination

## Context, outcome and ownership

Priority P2; locally completed. Depends on [Stage 1 contract](phase-01-start.md). One Stage 2 executor owns all listed files; shared protocol/runtime edits transfer after Stage 1 closes. Deliver source-safe asynchronous builds, bounded frame requests and atomic ready-preview installation in the normal native shell. Audio is prepared/staged here; live device ownership/scheduling arrives in Stage 4. No M3 Apply/Undo, accepted-source promotion or new crates.

## Verified starting points

- `Controller::reconcile` terminates all controller processes on invalid/changed source (`/root/fframes-desktop/desktop/crates/studio-engine/src/controller.rs:249`, `:278`, `:292`); cancel also does so (`:405`, `:409`). This would destroy last-good playback unless ownership is split. Existing `ProcessTreeManager::sub_manager` scopes child cancellation while parent shutdown remains terminal (`/root/fframes-desktop/desktop/crates/studio-bootstrap/src/process.rs:582`, `:657`).
- Build materialization copies source into an isolated tree, verifies source hashes and binds the managed SDK (`/root/fframes-desktop/desktop/crates/studio-engine/src/build_materialization.rs:30`, `:87`). `launch_portable_worker` locks the shared target with blocking `lock` (`/root/fframes-desktop/desktop/app/src/worker_project.rs:8`, `:27`), copies the binary and uses the materialized package cwd (`:67`). Retain the tree/media as long as its worker/audio needs them.
- `OperationTag` already has project/session/base/operation/generation and `ProjectState` is per controller (`/root/fframes-desktop/desktop/crates/studio-engine/src/state.rs:20`, `:86`). `Controller::complete` reconciles and validates tagged completion (`/root/fframes-desktop/desktop/crates/studio-engine/src/controller.rs:367`); neither completion nor accepted checkpoint is preview readiness.
- Legacy `request_timeline_inner` returns a report without identity validation (`/root/fframes-desktop/desktop/app/src/worker_client.rs:348`). Image conversion/presentation exist (`/root/fframes-desktop/desktop/app/src/frame_image.rs:20`, `:169`, `:216`). Existing `PreviewElement` is typed to spike app (`/root/fframes-desktop/desktop/app/src/preview_element.rs:10`); normal shell owns a serialized backend (`/root/fframes-desktop/desktop/app/src/studio_shell.rs:67`, `:351`).

## Requirements and data flow

Proposed engine `PreviewState` is ephemeral per open session and separate from durable source/checkpoint/job state. Do not serialize PreviewIdentity into portable project schema or advance accepted checkpoint on preview installation; derived preview state is rebuilt on reopen. States distinguish absent/building/preparing/ready/displayed/error; retain immutable displayed identity even when source is newer or unreadable. Compare identity and operation tag before every install; a monotonic seek serial distinguishes successive requests to the same worker, and checked counter exhaustion fails safely. A source-generation advance does not invalidate requests legitimately targeting the displayed older worker.

Data flow: current source inventory + BuildJob tag → verified materialized copy → shared-target build lock → isolated binary/media → candidate worker → negotiated hello/timeline/inspect/first frame/audio artifact → ReadyPreview → UI installation. Source is reconciled before materialization, completion and installation. BuildJob success records compiled source only; install is a separate guarded transition. Candidate errors show diagnostics alongside the last-good revision label. No failure writes portable source or accepted checkpoint.

Use controller/app root process owner plus separate build-operation and candidate/displayed-worker submanagers. Source reconcile/cancel shuts down only affected operation/candidate scopes; parent close/project switch/app exit shuts down every owned scope. Candidate and displayed worker can coexist; exactly one compiler, one candidate and one displayed worker per session. Replace queued build with latest source. Terminal scope shutdown prevents late spawns; do not reuse shutdown manager for retries. Make shared-target lock acquisition cancellable using nonblocking try-lock + short bounded polling off UI thread, deadline, token checks before/after lock and before spawn; never release another build's lock. Retain current lock semantics through binary copy.

Capture materialization lifetime in proposed `PreviewLease`, including immutable root/binary/media and artifact references. Hash/copy checks already implemented remain mandatory; reads during playback use the captured tree rather than live project paths. Prevent cache pruning while displayed/candidate/request leases exist. An obsolete build finishes or cancels without becoming installed; clean it only after process reap and lease release.

`ReadyPreview` carries one identity, validated timeline, critical inspection result, first scaled frame at current/clamped position, matching prepared mix descriptor and bounded cached PCM window. At commit, recheck source/session/tag and playhead seek serial. If user scrubbed during preparation, render the newest position before switching; do not briefly display the old preparation frame. Atomically replace timeline/frame/mix/playhead/selection on UI update; retain old worker until image/audio references release. Stage 4 adds output-stream switch acknowledgment before old audio is destroyed. Empty videos have no renderable frame, stopped playhead and explicit empty state; critical missing-media/render errors block readiness.

The frame pump serializes requests to each worker, with one active render and one replaceable latest desired request. Monotonic serial is assigned at user intent before dispatch, not when output arrives. Paused seeks coalesce; playing deadlines will use Stage 4 clock. Thumbnail/inspect/audio-read requests have typed completion destinations and lower priority; they never install the main preview image. Old-scale, old-seek and old-generation responses are discarded after validation/drain without updating timeline/playhead/image. Normal shell background queue is bounded; keep expensive build/hash/I/O away from its mutex and UI thread so Cancel/project switch still works.

Proposed bounds: two workers, one compiler; one frame active + one desired; two decoded main-frame buffers; one current GPUI image + at most one retiring image; 64 thumbnail entries / 16 MiB decoded budget (Stage 3); PCM window at most 2s / 1 MiB; bounded event channel capacity 2 with latest presentation replacing prior snapshot. Reuse explicit `Window::drop_image` behavior through image manager, update evicted IDs once and track release failures. Do not confuse Rust Arc drops with GPUI texture disposal. Image install must run on UI thread and include identity/serial guard.

## Files and actions (absolute paths)

| Action | File | Work |
|---|---|---|
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/controller.rs` | Source/job cancellation scopes and guarded preview readiness integration. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/state.rs` | Reuse tags; keep checkpoint/build identities distinct from preview. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/lib.rs` | Export GPUI-free preview model. |
| Create (proposed) | `/root/fframes-desktop/desktop/crates/studio-engine/src/preview_state.rs` | Immutable identity/ready/install/seek transitions; follow existing Rust module convention. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/build_materialization.rs` | Returned tree lease/cleanup ownership without changing portable source. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/Cargo.toml` | Protocol value dependency through existing desktop workspace. |
| Modify | `/root/fframes-desktop/desktop/app/src/worker_project.rs` | Cancellable lock/build launch, preview flags, retained materialization. |
| Modify | `/root/fframes-desktop/desktop/app/src/worker_client.rs` | Shared bounded control/bulk/deadline primitives; retain legacy API wrappers. |
| Create (proposed) | `/root/fframes-desktop/desktop/app/src/preview_worker_client.rs` | Strict negotiated M2 identity/timeline/inspect/audio client and destination tags. |
| Create (proposed) | `/root/fframes-desktop/desktop/app/src/preview_coordinator.rs` | Background build/candidate/frame pump and lease ownership. |
| Modify | `/root/fframes-desktop/desktop/app/src/frame_image.rs` | Explicit bounded image retirement/instrumentation, same color conversion. |
| Modify | `/root/fframes-desktop/desktop/app/src/preview_element.rs` | Reusable shell presentation callback while preserving spike adapter. |
| Modify | `/root/fframes-desktop/desktop/app/src/studio_shell.rs` | Show real preview/build/error/prior-revision state with bounded async updates. |
| Modify | `/root/fframes-desktop/desktop/app/src/lib.rs` | Module/export wiring using existing snake_case Rust module paths. |
| Modify | `/root/fframes-desktop/desktop/app/src/app.rs` | Adapt spike PreviewElement call sites; preserve qualification route. |
| Modify | `/root/fframes-desktop/desktop/app/tests/frame_image.rs` | Conversion/retirement regression. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/tests/external_changes.rs` | Scoped cancellation preserves old displayed worker. |
| Create (proposed) | `/root/fframes-desktop/desktop/crates/studio-engine/tests/preview_state.rs` | State/identity race matrix. |
| Create (proposed) | `/root/fframes-desktop/desktop/app/tests/preview_coordination.rs` | Real worker/build failure/immutable media/cleanup scenarios. |

Existing legacy consumer inventory to preserve: spike app `/root/fframes-desktop/desktop/app/src/app.rs:30`; agent pipeline `/root/fframes-desktop/desktop/app/src/agent_spike.rs:141`, `:149`, `:195`, `:205`, `:218` (both launches, both renders and metadata); selection contract `/root/fframes-desktop/desktop/app/src/selection_spike.rs:1`; public re-export `/root/fframes-desktop/desktop/app/src/lib.rs:16`; tests `/root/fframes-desktop/desktop/app/tests/worker_roundtrip.rs:1`, `/root/fframes-desktop/desktop/app/tests/managed_worker.rs:1`, `/root/fframes-desktop/desktop/app/tests/portable_managed_worker.rs:1`, `/root/fframes-desktop/desktop/app/tests/frame_image.rs:1`. New preview methods must not silently alter their v1 framing. No process-manager rewrite unless scoped-owner regression reveals a real gap.

## Implementation steps and tasks

1. Add state tests proving accepted checkpoint/build success never implies installed preview; characterize reconcile's current all-process cancellation.
2. Split operation/candidate/displayed process ownership using existing submanagers; test invalid-source paths, cancel and terminal close separately.
3. Make lock waits and build launch cancellable, retain immutable tree/media lease and isolate binary. Keep every process and artifact attached to its session owner.
4. Add strict M2 client checks and bounded frame pump/destination queues. Maintain legacy methods for spike/tests.
5. Prepare ReadyPreview and implement latest-seek guarded installation in native shell; don't hold backend mutex across long blocking work.
6. Refactor PreviewElement/image lifetime only as needed to reuse real rendering in shell; verify known alpha/color and stale-response behavior.
7. Test source changes at every boundary, real broken compilation and cancelled lock wait, then hand atomic mix lease to Stage 4.

- [x] Preview identity/ready/displayed state is separate from checkpoint and BuildJob success.
- [x] Reconcile/cancel preserve older immutable worker while close/switch reap all owned children.
- [x] Build/materialization/target lock cancellation is bounded and source-safe.
- [x] Candidate tree/binary/media/artifacts remain leased until final consumer release.
- [x] M2 client validates hello/timeline/inspect/frame/audio and classifies bulk destinations.
- [x] Frame/event queues are bounded and same-worker stale seek/scale responses never install.
- [x] Native shell switches ready preview atomically and labels old preview during failures.
- [x] Color/alpha, real failed build, race and resource gates pass; spike route remains compatible.

## Verification and measurable success

Narrow existing regression commands from root:

```sh
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test state --test external_changes --test materialization
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test frame_image
```

Proposed targets after creation: `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test preview_state` and `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test preview_coordination`. Mark real SDK/device requirements explicitly in the latter; run ignored scenarios explicitly with a fresh compatible `SDK_BUNDLE` as in Stage 4, not as default-suite passes.

Unit matrix: old session/project/revision/generation; same-worker seek/scale (an in-flight older seek may complete/drain, but only the latest intent may become displayed); counter exhaustion; empty timeline; critical inspect/audio failure; source edit-back; successful build before readiness; immutable prior display. Integration matrix: edits at materialization/build/hello/timeline/frame/audio/install, blocked lock cancel, candidate crash, failed compilation, old media modified/deleted, switch/close during candidate prep, late background spawn, thumbnail reply during scrub. Done means no wrong-identity result installs, prior video stays seekable after each failure, stale work is reaped, and configured queue/image/artifact high-water limits are enforced. Stage 4 measures sustained playback and live audio release.

## Risks, security, rollback and next

High (medium × high): reconcile or cancel destroys last-good worker. Mitigate scoped-owner process tests, including invalid-source branch. High (medium × high): old request within same generation overwrites new seek. Mitigate intent-time serial and final UI guard. High (medium × high): live media leaks into old revision. Mitigate retained copied-tree cwd/leases and edit/delete asset tests. High (medium × high): shell mutex blocks Cancel. Mitigate dedicated coordinator and bounded command/event channels. Medium: target lock on non-Unix behaves differently; exercise cancellation on native CI.

Validate worker metadata and payload lengths before trusting them; app resolves cache paths itself. Maintain original project trust boundary, no arbitrary worker path reads or unbounded logs. Cleanup only owned process/cache trees; parent shutdown cannot touch user processes.

Rollback product preview routing to M1 honest empty state while retaining legacy spike; revert coordination/client changes together. Preserve source/history/SDK and displayed lease until teardown completes. Transfer shared app/engine files to Stage 3 only after state, cancellation and real failed-build gates pass.

## Local implementation evidence — 2026-10-02

Phase 2 is implemented locally, uncommitted. Missing Stage 1 prerequisites were added: additive negotiated preview transport, persistent CPU preview, bounded inspection and disk-backed chunked PCM, explicit template/overlay entry, and a fresh SDK preview probe. This does not close every Stage 1 performance/platform gate or implement Stages 3–4. Legacy framing and manifest digests remain compatible; preview installation never promotes the checkpoint.

- Desktop workspace: 113 nonignored tests pass; workspace/all-targets Clippy with `-D warnings`, both workspace format checks and `git diff --check` pass. Root protocol/runtime: 17 tests pass and targeted Clippy passes. Packaging: six tests pass.
- Fresh `/tmp/studio-m2-sdk-phase2`: assembler performed two offline worker builds and the full preview-contract probe before setting `preview_contract_verified`. Explicit ignored `preview_coordination`, `managed_worker`, `portable_managed_worker` and `worker_roundtrip` tests pass with this bundle/native fixture.
- Real coordination proves compile failure, deleted live font, latest-wins seeks, stale readiness rejection/re-prime, shorter revision clamping, exact PCM offset, unchanged checkpoint, post-commit cancellation and process/tree cleanup. Unit/client tests cover identity/session/generation/seek/scale/counter failures, malformed/truncated bulk records, inspection/audio readiness and bounded lock/copy cancellation.
- Normal native shell exercised on Linux X11/Xvfb, software rendering, 1280×800: build → frame 17 → broken compile → old revision seek to 43 → shorter rebuild clamps to 30/30 → cancel → previous frame 29 → focused Home/End → Close project. Close reaped the worker, removed every leased `build-*` tree and cleared the image. Inspection found and fixed read/access watcher events blocking the install fence; its regression fails before the fix and passes after it.
- Inspected screenshots/logs: `.amp/in/artifacts/phase2-preview-{ready,prior-seek,shorter,cancel-step,keyboard-home,keyboard-end,closed}.png`, `phase2-desktop-tests-final.log`, `phase2-desktop-clippy-final.log`, `phase2-real-coordination-final.log`, `phase2-legacy-workers.log`, `phase2-root-tests.log`, `phase2-root-clippy.log`, `phase2-sdk-assembly.log`, `phase2-watcher-{before,after}.log`. Artifacts are local and excluded from Git.

Stage 3 receives the validated compiled timeline and intent-time seek serial; Stage 4 receives matching prepared PCM and the retained worker/materialization lease. Live device switching, sustained playback/thumbnail cache measurements, physical GPU/display/IME and Windows/macOS native qualification remain open. The executed races are representative, not exhaustive scheduling/model-checking evidence.

### Review fixes — 2026-10-03

The installation frame is now inspected in initial preparation and every re-prime, in addition to scene boundaries. Critical or incomplete diagnostics reject the candidate; boundary findings remain bounded and do not accumulate obsolete playhead findings. Candidate re-priming uses the single background preparation lane, leaving displayed-worker seeks independent. Completed preparation publishes readiness only for the current seek/scale, and cancelled work cannot publish into a newer build.

The expanded real `preview_coordination` regression first failed on a missing image used only at the installation frame, then (with inspection fixed) on a blocked last-good seek. It now passes explicitly with `SDK_BUNDLE=/tmp/studio-m2-sdk-phase2`, covering initial/re-primed missing media, a gated candidate while the displayed worker delivers a newer seek, latest readiness after release, cancellation during the gate, and existing checkpoint/process/tree cleanup assertions. All 115 nonignored desktop tests, workspace/all-targets Clippy with `-D warnings`, desktop format and diff checks pass. Evidence: `.amp/in/artifacts/preview-fix-{regression-before,regression-inspection-only,real-regression,desktop-tests,clippy}.log`. These follow-up checks ran on Linux; no new native UI or other-platform qualification is claimed. Changes remain local and uncommitted.
