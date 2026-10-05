# Assumption and contract review: Desktop M3 plan

Status: DONE

Reviewed `plan.md` and all four phase files against existing project, engine, agent and preview contracts. Source inspection only; no lint, build or tests run. Entire M3 scope remains required. Two high findings require plan amendments before implementation; no additional critical findings established.

## 1. High — detached-writer refusal lacks a mechanism that can observe escaped descendants

**Plan location:** Stage 1, Requirements and data flow, [phase-01-start.md:28](../261004-0730-desktop-phase-three-agent-transactions/phase-01-start.md). Gate also requires unproven quiescence to block capture. Stage 1 files/steps assign supervision to the agent package and reuse the bootstrap owner without assigning stronger process ownership.

**Source evidence:** [process.rs:243](../../desktop/crates/studio-bootstrap/src/process.rs) probes a Unix process group with `kill(-pgid, 0)`; [process.rs:318](../../desktop/crates/studio-bootstrap/src/process.rs) returns successful cleanup once the root exited and that group disappeared. [process.rs:420](../../desktop/crates/studio-bootstrap/src/process.rs) starts a process group, not a containment mechanism. No setsid-escape observer or cgroup ownership exists in these paths. After sending SIGKILL, [process.rs:342](../../desktop/crates/studio-bootstrap/src/process.rs) waits for the direct child and returns, without a post-kill proof that all owned group descendants have ceased execution.

**Concrete failure:** An adapter tool starts a conventional detached helper using a new session/process group and closes inherited pipes; the helper retains draft write access. The adapter reports end_turn and exits. The tracked group disappears, so the current owner sees no writer and cannot distinguish this case from actual quiescence. A later helper edit may race immutable capture or refresh the stable draft for the next task. Repeated source scans can catch some changes but cannot implement the plan's promised detection of unsupported detached writers.

**Required amendment:** Define the exact Linux quiescence mechanism and evidence gate before calling it verified. Assign bootstrap ownership changes if adding containment (for example a task-owned delegated cgroup whose membership survives setsid), account for permissions/unavailability, and verify terminal membership after cleanup. If using only process groups for a specifically qualified adapter, explicitly bound the guarantee to qualified owned descendants and document detached-writer behavior as a qualification constraint; do not claim general detached-writer detection. Fresh sessions alone cannot repair an unknown writer that still holds the same draft cwd. Keep fail-closed capture when the selected mechanism cannot establish its stated guarantee, and add an escaped-helper scenario to the execution matrix.

This finding concerns ordinary detached subprocess behavior, not a demand to sandbox malicious agents. The plan already correctly says workflow ownership is not a security sandbox; it still needs an executable definition of the stronger writer proof it requires.

## 2. High — required audio validation assumes an analysis CLI that the accepted project contract does not require

**Plan location:** Stage 2, Requirements and data flow, [phase-02-candidate-validation-and-project-tools.md:24](../261004-0730-desktop-phase-three-agent-transactions/phase-02-candidate-validation-and-project-tools.md); steps 2/6 and gate require validation before promotion. Its file ownership says no root runtime/protocol API changes are planned.

**Source evidence:** Imported projects are accepted by [lifecycle.rs:324](../../desktop/crates/studio-project/src/lifecycle.rs) with package/Rust-entry/inventory checks; no fframes analysis CLI contract is verified. [lifecycle.rs:121](../../desktop/crates/studio-project/src/lifecycle.rs) recognizes the worker bridge independently. The generated template's main does use `fframes::cli`, but that establishes only template behavior, not an import invariant. [worker_project.rs:100](../../desktop/app/src/worker_project.rs) compiles the selected worker target only. [preview.rs:226](../../fframes-studio-protocol/src/preview.rs) provides timeline/PrepareAudio/ReadAudio requests and no analysis RPC; [preview_worker_client.rs:319](../../desktop/app/src/preview_worker_client.rs) exposes prepared PCM, not integrated loudness analysis.

**Concrete failure:** An existing compatible imported project has a valid library and preview worker plus a custom main that renders directly, without `fframes::cli`. An audio-only agent edit compiles and prepares valid PCM. Stage 2 then cannot invoke the assumed `audio analyze/at` route, so required validation is either skipped, falsely reported, or the legitimate project is blocked for an undocumented additional entry-point requirement. Building an extra CLI ad hoc also needs a declared artifact/key/ownership route rather than bypassing compiler sharing.

**Required amendment:** Choose and specify an audio-validation route that works for accepted worker-only/custom-main imports. The minimal desktop-owned route can check placement/mix against compiled timeline metadata and analyze the already prepared immutable PCM through an explicitly selected local analyzer, with clear metrics/limits; it need not enlarge the six tools or alter the worker protocol. Alternatively explicitly own an additive analysis bridge/build target and its compatibility/cache contract. Keep existing project CLI comparison as a conditional parity check where the CLI exists. Add an imported custom-main, audio-only edit case to the execution matrix. Do not silently add a new portable project CLI requirement.

## Verified decisions that do not warrant reversal

- Candidate/base separation is explicit across all stages, preserving M1 checkpoint semantics and requiring fresh published-preview authorization; existing `preview_identity`/`can_install` contracts justify these planned changes.
- Existing target locking is correctly described as serialization rather than deduplication; Stage 2 assigns sharing, subscriber cancellation and build leases above it.
- Stage 3 acknowledges external editors ignore `owner.lock`, retains displaced originals and requires no-clobber primitive evidence. It gates unsupported platforms instead of treating inventory scans as exclusion. No blanket filesystem atomicity claim remains.
- Undo explicitly validates an inverse-delta merged candidate and preserves unrelated edits; it does not reuse Restore-as-copy or force a historical whole-tree restore.
- Stage 4 leaves authenticated-provider and native/platform qualification open; fixtures and software rendering do not close these gates.

Unresolved questions: exact Linux writer-proof mechanism and unavailable-containment behavior; audio analyzer/bridge choice and required metrics. Exact SDK/provider/MCP pins remain declared execution gates, not verified facts.

## Controller amendment verification

Both findings are **resolved at the planning level** after reading the amended Stage 1, Stage 2 and Stage 4 wording. Historical findings above remain as review evidence; they are no longer open blockers.

1. Stage 1 now names a specifically qualified Linux adapter whose descendants stay in its task process group, assigns `studio-bootstrap/src/process.rs` terminal group verification, acknowledges setsid escapes cannot be observed generally, and blocks Apply/capture/draft reuse for detached or unknown ownership. Its escaped-helper qualification failure and post-kill group verification scenarios match the source limitation. Stage 4 records the writer model and terminal cleanup in authentic evidence. This resolves the missing executable boundary without claiming a security sandbox; implementation and provider qualification remain gates.
2. Stage 2 now requires bounded local checks of immutable prepared PCM and compiled timeline, including revision, sample/channel geometry, finite samples, duration/placement and clipping/silence diagnostics. It makes project CLI comparisons conditional and explicitly tests a custom-main import without a video CLI and an audio-only edit. This removes the ungrounded import prerequisite while retaining required audio validation and the six-tool facade.

No additional broad review or tests/builds were performed for this verification. Remaining SDK/provider/MCP pins are acknowledged execution prerequisites.
