---
title: "Revision-pinned native MP4 export"
status: in-progress
priority: P1
effort: "8–12 engineer-days"
---

# Stage 3: Revision-pinned native MP4 export

## Context and outcome

Depends on stages 1–2. Read [plan](plan.md), build_service, worker_project, controller/checkpoints, project templates, root fframes-studio-runtime/protocol, and fframes/src/renderer/{cli,renderer,encoder,fframes_logger}.rs. Add native export controls using existing rendering/encoding behavior without rendering from mutable live source.

## Verified starting point and architecture

Controller::export_checkpoint exports portable source, not video. BuildService deduplicates exact build keys and keeps leased MaterializedBuilds; CompileRequest.retained supports retained source, but compile_portable_worker_via supplies None. BuildProfile has only Debug and CargoCompiler accepts default-feature CPU workers. PreviewRequest has no export operation. CLI render/JSON progress and encoder controls exist separately.

Use an additive export entry in the project worker binary, dispatched before preview's required frame-port/generation arguments. The export process constructs the same Video/media/styles and calls the existing renderer/encoder with bounded progress logging. Run it separately from the displayed preview process. The app owns cancellation, temporary destination and final publication. Do not add export to the preview request loop or assume imported worker binaries support it.

Flow: explicitly selected validated/published source revision + immutable assets/style snapshot → retained build request with exact SDK/environment/backend → compiled artifact lease → owned export process → same-directory temporary MP4 → container/decode verification → guarded atomic publication.

## File ownership

| Action | Files under repository root | Purpose |
|---|---|---|
| Create | desktop/crates/studio-engine/src/export-job.rs; desktop/app/src/export-service.rs, export-view.rs; desktop/app/tests/native-export.rs | Durable export state, service/UI and failure coverage |
| Modify | desktop/crates/studio-engine/src/controller.rs, journal.rs, lib.rs; desktop/app/src/lib.rs, studio_shell.rs, teardown.rs | Accepted capture, job recovery and close cleanup |
| Modify | desktop/app/src/build_service.rs, worker_project.rs; desktop/crates/studio-sdk/src/project.rs | Retained-source subscriptions and actual build support |
| Create | fframes-studio-protocol/src/export.rs; fframes-studio-runtime/src/export-worker.rs | Negotiated request/progress/completion contract and render bridge |
| Modify | fframes-studio-protocol/src/lib.rs; fframes-studio-runtime/src/lib.rs | Additive exports only |
| Modify | desktop/crates/studio-project/templates/src/bin/studio_worker.rs; desktop/crates/studio-project/src/lifecycle.rs; desktop/fixtures/annotated-video-overlay/src/main.rs | Shared project export entry and matching qualification fixture |
| Modify | desktop/scripts/assemble-phase-zero-sdk.py; desktop/app/tests/build_sharing.rs, managed_worker.rs | Published SDK capability and dedup/real-worker regression |
| Modify if needed | fframes/src/renderer/fframes_logger.rs | Only an evidenced gap in structured progress; preserve CLI output |
| Modify | docs/desktop/architecture.md | Export identity/publication/recovery contract |

No deletions. Reuse existing module conventions; proposed kebab-case Rust modules need explicit path attributes, as used by M6.

## Requirements and implementation

1. Define export acceptance independently of ProjectState::accepted(), which is the M1 saved checkpoint and is deliberately unchanged by Apply/Undo. Default to the latest committed published task revision matching current source and its validation evidence, or the current successfully compiled/validated source for a project without a task revision. Capture that revision's immutable checkpoint while holding the controller briefly, then freeze source, lockfile, assets, style snapshot and media bytes off-thread with before/after identity verification. If source is newer/unvalidated or the preview shows an older revision, label the identities and offer validation or an explicit prior validated revision; never silently export the M1 saved checkpoint. A damaged live checkout must not prevent exporting an intact accepted snapshot. Define explicitly which external runtime assets must be copied or rejected; revision IDs alone do not freeze media.
2. Persist job ID, accepted revision, SDK/compatibility/environment digests, backend, video metadata, codec/quality, final/temp paths and publication phase in app-local state. Portable studio.json/history semantics stay compatible.
3. Subscribe using CompileRequest.retained and the existing process-wide BuildService. Retain materialization and binary/SDK leases through verification/publication. Add export subscriber identity. Reuse a preview build only when byte-changing inputs and export capability agree; codec/quality/output belongs in the render request unless it affects compilation.
4. Add negotiated export hello/request/progress/completion with bounded JSON and diagnostics. Use the root runtime and actual renderer API. Child writes only the job's temporary destination; app completion validates identity, process success, closed muxer/trailer, container streams/duration and representative decode.
5. Expose native destination picker, revision label, qualified MP4 codec/quality choices, progress and cancel. Default CPU export must match CPU preview/fonts/audio. Detect shader/backend gaps and visibly block an unsupported render rather than silently dropping layers. Additional advertised backend/profile support requires real compiler/SDK capability, not only a new BuildKey value.
6. Begin with one active export and a bounded FIFO queue; capture revision when queued, show queued/running/interrupted states and cancellation. A later edit/Undo never changes an existing job. Admission enforces retained-build/disk/resource budgets.
7. Create temp output on the destination filesystem using an exclusive unpredictable owned name. New destinations use no-clobber publication. Replacement requires an explicit native overwrite choice; recheck destination identity before atomic replace, preserve preexisting bytes on any failure and handle Windows open-handle/locked-file semantics. Flush data/metadata as required.
8. Give each job a scoped ProcessTreeManager; cancel detaches its build subscriber and reaps its renderer without killing another preview/subscriber. Cancel-before-publication is fenced; after committed publication report completion. Close cancels queued/active jobs; startup reconciles publishing intent and marks incomplete jobs interrupted, never resumes mutable rendering silently.

## Checklist and success criteria

- [x] Linux development path: the generated worker exported MP4 with video/AAC audio, the worker decoded representative video/audio, `ffprobe` confirmed both streams, and tests verified the captured revision survives a later live-source edit, cancellation during rendering leaves no final/temp file, and a destination created during rendering is preserved by no-clobber publication. The app independently opens output, decodes representative first/middle/final video frames and bounded audio samples before publication; a generated valid/truncated MP4 test covers this path. The actual release-candidate/installed-target gate remains open.
- [ ] Apply without Save exports the published edit; Undo then export uses the reverted published source, independently of the M1 saved checkpoint.
- [ ] Edit/Undo/external source or media changes during export do not alter frozen output.
- [ ] Equal supported preview/export requests compile once; differing compilation inputs never collide.
- [ ] FIFO admission is bounded; UI progress/cancel remains responsive.
- [ ] Crash/write/full-disk/cancel/publish races retain old final bytes and never expose a partial final MP4.
- [ ] Close/restart cleans only owned temp files/processes and recovers publication intent safely.
- [ ] Preview/CLI/export representative pixels and decoded audio agree within recorded codec tolerances.

## Validation

On Linux, run `SDK_BUNDLE="$PWD/.amp/in/artifacts/m7-local-sdk" cargo test --offline --manifest-path desktop/Cargo.toml -p fframes-studio --test native-export linux_export_renders_the_frozen_saved_revision_to_decodable_video_and_audio -- --ignored`; then existing build_sharing/promotion_handoff/recovery tests and `cargo test --locked -p fframes-studio-protocol -p fframes-studio-runtime`. The Linux development SDK export smoke passes actual cancellation during rendering, destination-creation race, real-worker video/audio decode and generated-project `frame`, `inspect`, and `strip`; it is not an M7 release-gate pass because it does not bind a signed candidate package/feed and does not establish codec redistribution. Still test crash/full-disk recovery, cancellation around the commit boundary, and destination races on all native OSes. Deterministic process fixtures cover faults but cannot prove codec output. Run scoped Clippy/format; check impacted WASM/shared API compilation only if a shared fframes API changes.

## Risk, security and rollback

Native project workers execute user Rust under the existing local trust model; do not claim sandboxing. Never delete user destinations or scan broad temp trees. Bound queued snapshots and native encoder threads. Disable export on incompatible legacy workers with actionable guidance; preserve existing preview/edit/source APIs. Failed job rolls back only its owned temp state.

## Next step

Stage 4 packages and updates the complete setup/export path.
