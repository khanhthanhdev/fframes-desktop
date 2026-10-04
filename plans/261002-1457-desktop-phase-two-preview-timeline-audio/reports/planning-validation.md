# Phase 2 planning validation

Date: 2026-10-02. Scope: documentation and planning only. Implementation remains pending.

## Inherited decisions and validation

Questions asked: 0. The accepted M2 roadmap and existing M0/M1 decisions resolve the material product scope. The M1 validation explicitly authorizes development using recorded Linux X11 software-rendering evidence while physical GPU/display/IME, Windows/macOS interactive and authenticated ACP qualification remain open. This plan carries that decision forward without treating those gates as passed.

The proposed CPU backend, preview dimensions, PCM artifact format, cache budgets, preparation timeout and output-clock implementation are engineering defaults to verify during execution. They are not newly confirmed user decisions or measured performance promises. M3 agent transactions, M4 preset editing, M5 object retrieval and M7 export controls remain with their owning milestones.

## Source and contract verification

Tier: Standard (four stages; Fact Checker and Contract Verifier). Forty source anchors were checked, ten for each stage's starting contracts. Verified: 40. Failed: 0. These checks establish current source availability, not future behavior or runtime qualification.

| Stage | Verified starting contracts | Owning evidence |
| --- | --- | --- |
| Worker contract | v1 protocol, frame cap/header, hello, existing worker entry, persistent Previewer/CPU renderer, public inspect/scale/resolved-timeline APIs | [Protocol](../../../fframes-studio-protocol/src/lib.rs), [worker](../../../fframes-studio-runtime/src/worker.rs), [Previewer](../../../fframes/src/renderer/preview.rs) |
| Preview coordination | OperationTag, open session, source reconciliation, checkpoint meaning, controller process termination, process scopes/shared lifecycle lock, isolated executable, seek coalescing, image creation | [State](../../../desktop/crates/studio-engine/src/state.rs), [controller](../../../desktop/crates/studio-engine/src/controller.rs), [process ownership](../../../desktop/crates/studio-bootstrap/src/process.rs), [launch](../../../desktop/app/src/worker_project.rs), [client](../../../desktop/app/src/worker_client.rs), [conversion](../../../desktop/app/src/frame_image.rs) |
| Timeline and controls | compiled timeline/scene reports, half-open overlapping scene ranges, mix metadata, scene index, audio projection, client timeline request, informational video hints, existing shell and preview element | [Reports](../../../fframes/src/renderer/preview.rs), [protocol](../../../fframes-studio-protocol/src/lib.rs), [manifest](../../../desktop/crates/studio-project/src/manifest.rs), [shell](../../../desktop/app/src/studio_shell.rs), [element](../../../desktop/app/src/preview_element.rs) |
| Audio and qualification | sample-rate-aware mixer and sequential rendering, current CPAL pin, stream thread limitation, device callback, existing player's wall clock, Linux audio dependency, real portable-worker CI target, SDK runtime packaging, pending M0 qualification | [Mixer](../../../fframes/src/audio_mix.rs), [player manifest](../../../fframes-native-player/Cargo.toml), [audio](../../../fframes-native-player/src/audio.rs), [player clock](../../../fframes-native-player/src/app.rs), [CI](../../../.github/workflows/desktop.yml), [assembler](../../../desktop/scripts/assemble-phase-zero-sdk.py), [qualification](../../../docs/desktop/phase-zero-feasibility.md) |

The current native player uses a wall clock; its CPAL implementation is a reference for device handling, not proof of the audio-clock synchronization required by M2. Its private audio implementation and winit/Skia event loop are not reusable public Studio services.

The runtime entry inventory covers the project template, annotated overlay, SDK generator embedding that overlay, and in-module lifecycle test. Protocol changes also affect app worker/client/conversion/selection consumers and frame tests. The assembler packages both root crates; SDK setup, generated projects, compatibility manifests, offline checks and native CI are part of the contract handoff.

Two implementation boundaries need explicit protection: `Controller::reconcile` currently terminates all controller processes, and SDK pins use a digest of the typed compatibility manifest. M2 must preserve the displayed immutable worker when cancelling stale builds and preserve legacy digests when adding optional compatibility declarations.

## M2 acceptance coverage

| Roadmap requirement | Execution owner |
| --- | --- |
| Persistent GPUI-free worker; hello/version, timeline, frame, inspect and shutdown | Stage 1 |
| Bounded binary transport; measured conversion and presentation | Stages 1–2, final evidence in 4 |
| Compiled scene/audio tracks; UI-independent geometry and hit testing | Stage 3 |
| Play/pause, step, paused/playing scrub, zoom, range/overlap selection, thumbnails and time | Stage 3, audio integration in 4 |
| Matching revision audio and output-clock scheduling; drop late video | Stages 1 and 4 |
| Stale identity/seek rejection, safe rebuild and preserved/clamped playhead | Stages 2–4 |
| Failed compilation retains a playable successful preview | Stage 2, integrated gate in 4 |
| Repeated scrub/play remains bounded; close/reopen cleans resources | Stages 2–4 |
| Known color/alpha frames, same-backend CLI parity and visible unsupported shaders | Stages 1–2, final evidence in 4 |

## Validation evidence and limits

The CLI scaffolded the directory using UTC timestamp `261002-1457`; this matches the previous plans' naming behavior. All four generated stubs were read before replacement. `add-phase` did not regenerate the index table, so the file-owned planning table lists the four stages without advancing execution status. The plan is pinned for this worktree.

There is no live task-management tool in this session. The index and phase checklists remain the durable tracking surface; implementation rebuilds its work list from unchecked tasks.

No compilation, test suite, GUI playback, audio-device test, SDK assembly or platform qualification is claimed by this planning task. Existing M1 execution evidence is historical source material; proposed M2 tests and native qualification remain execution requirements.

`ak plan validate` passes. `ak plan parse` reports pending, four stages, 32 unchecked execution tasks and zero completed tasks. The index is under 80 lines. All 46 local Markdown links/anchors across eight documents pass, including the expanded roadmap and journal. All 66 file-ownership rows resolve to existing files or explicitly proposed files; 22 distinct new files support the required models, services, tests and qualification evidence. No new crate is proposed.

### Whole-plan consistency sweep

The controller reviewed the index and all four phase files, then checked the completed files together for stale names, unfinished stubs, dependencies, state meaning, resource bounds, ownership and acceptance coverage. Corrections preserve the roadmap heading anchor, existing snake_case Rust module convention, legacy SDK digests, PCM temporary-file accounting, explicit real fixture ownership and separate M2 qualification validation. The current validator hardcodes M0 gates; Stage 4 now owns an explicit M2 format/schema and regression coverage without converting M0 evidence.

Dependencies are sequential and acyclic. Shared-file ownership transfers between stages. Proposed tests are labeled and become runnable after creation; existing package/test targets were checked against source and CI. The source/accepted/build/displayed identities and old-worker retention remain consistent across all stages. All original M2 requirements have an execution owner. Unresolved contradictions: 0.

The plan is ready for implementation planning handoff. Baseline regressions, native audio timing and all stated execution/qualification gates still must pass before M2 can be marked complete.

## Unresolved questions

No blocking product question. Native audio timing and platform qualification require evidence during implementation.
