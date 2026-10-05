# M3 failure-mode and contract review

Reviewed `plan.md` and all four phase files against transaction/preview architecture and the current engine, project inventory/checkpoint, worker compilation/coordinator and native shell contracts. Plan-only review; no tests or builds run.

Critical findings: 0. High findings: 0.

No concrete uncovered high/critical failure survived the evidence filter. Existing limitations are explicitly assigned implementation and regression gates:

- Candidate/base authority: `desktop/crates/studio-engine/src/preview_state.rs:8` derives preview identity from the operation base; `desktop/crates/studio-engine/src/preview_state.rs:324` requires that base to equal current source. Stage 2 explicitly separates candidate staging, and Stage 3 requires fresh published-candidate authorization rather than retagging the accepted base.
- Shared compiler cancellation: `desktop/app/src/preview_coordinator.rs:108` cancels its predecessor, and `desktop/app/src/preview_coordinator.rs:128` shuts down its compiler scope. Stage 2 replaces this ownership contract with shared compilation, separate subscribers and cancellation/lease tests; retaining the current behavior is not the proposed implementation.
- Recovery authority: `desktop/crates/studio-engine/src/journal.rs:21` currently has metadata-only intent/commit events; `desktop/crates/studio-engine/src/controller.rs:117` recovers pending jobs without source publication. Stage 3 explicitly adds forward/inverse file-set intent, durable progress, conflict-preserving replay and interruption tests at every durable boundary.
- Source and permission integrity: `desktop/crates/studio-project/src/revision.rs:53` currently inventories content plus an executable flag, and `desktop/crates/studio-project/src/checkpoint.rs:147` restores executable files as 0755. Stage 3 assigns mode inventory, expected-mode conflicts, immutable originals and permissions assertions to the owning project/transaction surfaces.
- Playback handoff: `desktop/app/src/studio_shell.rs:913` stages audio before installation, while `desktop/app/src/studio_shell.rs:1347` guards the UI commit against source, session, generation and readiness. Stages 3–4 explicitly retain old playback on failure, re-prime the latest playhead and require matching audio/video plus fresh epoch authorization.

Writer quiescence, detached helpers, publication interleavings, unsupported platform primitives and authentic adapter capability negotiation remain execution gates in the plan. This review does not establish those gates as passed.

Unresolved review questions: none.
