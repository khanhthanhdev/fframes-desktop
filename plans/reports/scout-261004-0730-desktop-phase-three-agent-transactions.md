# M3 contract scout

Status: DONE_WITH_CONCERNS

Scope: repository inspection for M3 planning only. No implementation or provider qualification was performed. Preserve the separate desktop Cargo workspace and existing pinned upstream GPUI. Presets, element selection and new provider promises remain outside this milestone.

## Reusable contracts

| Owner | Concrete reuse | Evidence |
| --- | --- | --- |
| M0 ACP transport | `AcpSession::spawn`, `run_prompt`, `wait_for_reply`; `SessionControl::{submit_reply,choose_permission,cancel}` | [session.rs](../../desktop/crates/studio-agent-spike/src/session.rs) |
| Process ownership | `AgentSupervisor::spawn_adapter`, `cancel_task`; `ProcessTreeManager` sub-managers, allowlisted `ChildEnvironment`, piped stderr | [supervisor.rs](../../desktop/crates/studio-agent-spike/src/supervisor.rs) |
| Project ownership | `Controller::open` locks `owner.lock`; serialized controller owns watcher, SQLite and journal | [controller.rs](../../desktop/crates/studio-engine/src/controller.rs) |
| Stale-result fences | `OperationTag` binds project, fresh open session, base source, operation and generation; reconciliation invalidates obsolete work, including failed scans | [state.rs](../../desktop/crates/studio-engine/src/state.rs), [controller.rs](../../desktop/crates/studio-engine/src/controller.rs) |
| Immutable bytes | `Checkpoints::{capture,load,draft}` stores verified content-addressed objects, performs repeat inventory checks and durable file/directory sync | [checkpoint.rs](../../desktop/crates/studio-project/src/checkpoint.rs) |
| Managed SDK builds | `materialize_with_cancel`, `MaterializedBuild` lease, SDK pin/digest and target-scoped build tree | [build_materialization.rs](../../desktop/crates/studio-engine/src/build_materialization.rs) |
| Compiler isolation | `acquire_build_lock`, `compile_portable_worker`, `launch_preview_worker`; isolated binary copied while target lock remains held | [worker_project.rs](../../desktop/app/src/worker_project.rs) |
| Preview readiness | `ReadyPreview::new`, `PreviewState::{can_install,install}`, immutable PCM leases; timeline, diagnostic, frame and audio identity checks | [preview_state.rs](../../desktop/crates/studio-engine/src/preview_state.rs) |
| Preparation/playback | `PreviewCoordinator::{build,seek,commit,cancel_build}` and `prepare`; scene boundary inspection, playhead inspection, frame and audio staging | [preview_coordinator.rs](../../desktop/app/src/preview_coordinator.rs) |

`run_prompt` waits for the correlated `session/prompt` response and validates its stop reason. Silence is not completion. Unresolved permissions reject completion. Wire messages are bounded to 1 MiB; transcript/diagnostics use bounded sanitized text. Transport tests already exercise real subprocess framing, permission handling, same-session clarification, cancellation and invalid reasons. Reuse their fixtures as regression evidence, without treating fixtures as real provider qualification.

## Gaps that planning must name

1. **Driver interface:** `driver.rs` contains DTOs and redaction, not the architecture's normalized app-owned driver/event abstraction. The transport hand-writes JSON; it discards protocol error data beyond code/message and ignores most tool/config updates. `agent_info` retains only name/version. Official SDK adoption needs a compatibility evaluation and regression gate before replacing proven framing/cancellation behavior. No source evidence establishes SDK selection or live provider qualification.
2. **Production conversation:** `AgentSpike` polls one bounded transcript string and renders it as one child, with permission buttons and reply input. It has no virtualized message model, tool cards, provider option handling or durable task transcript. Its generation guard and cleanup are useful examples, not a production task controller.
3. **Stable draft/task ownership:** spike drafts are timestamped siblings; `Controller::begin_job_observed` creates a different session/operation-named draft for every Build/Checkpoint. Neither supplies a persistent provider workspace reconciled to accepted bytes. Spike also creates a worker project and requires changed frame-zero pixels and a title anchor: those are M0 qualification conditions, unsuitable general M3 edit acceptance criteria.
4. **Task lifecycle:** `JobKind` only has Build/Checkpoint. `Candidate` is revision/tag only, and `propose` registers bytes after a succeeded job without applying them. No production edit/repair/conflict/apply/Undo state machine or task metadata exists. `Record` contains location/name/state/draft/SDK path only; review policy is absent from both it and portable `Manifest`.
5. **Tool entry points:** session creation hardcodes `mcpServers: []`; no task-scoped fframes MCP/CLI facade is connected here. M3 tools are project context, timeline, frame/strip, inspect and build status. Architecture's broader selection/style/audio/source-lookup inventory does not expand M3 scope.
6. **Validation/repair:** preparation already checks timeline boundaries and current playhead, rejects error/truncated inspection, stages a frame and prepares audio. M3 still needs representative frames plus structured compiler/inspection repair context and a visible finite repair budget. Preparing PCM alone is not loudness/placement analysis. Retain candidate/draft on failure and Stop.

## Build and tool coordination

`PreviewCoordinator::build` always cancels the previous request. `compile_portable_worker` always materializes a fresh temporary tree and invokes Cargo; the cancellable target file lock serializes shared target mutation but does not deduplicate same-revision requests. Preserve this lock and binary isolation.

Introduce one app-owned build service through which UI preparation and MCP/CLI requests acquire an in-flight or retained result. Key compilation by project, immutable source revision, compatibility digest/target and relevant build entry/options; keep operation/session/generation identities on each subscriber's result. An artifact can be shared across task requests without sharing obsolete installation authority. Share compilation before launching separate worker consumers where necessary; do not make a tool query steal the displayed worker's render lane.

Retain `Arc<MaterializedBuild>` until worker/audio/artifact consumers release it. Separate subscriber cancellation from underlying build cancellation: one tool's cancellation must not terminate a build still required by UI validation. Stop/project close must cancel all task consumers and reap owned children. Limit tool queues, output and image artifact sizes; expose revision and diagnostic structure. Never add arbitrary-shell or unrestricted filesystem tools.

## Promotion and Undo safety

- **Accepted is not validated:** `ProjectState` explicitly defines accepted as saved checkpoint bytes; `complete(Built)` only records built and does not accept. `complete(Checkpointed)` accepts only `tag.base_source`. A task candidate differs from this base, so do not repurpose checkpoint completion or claim successful compilation promotes it.
- **Two revisions must remain distinct:** current `preview_identity(tag)` sets source revision from `tag.base_source`, and `PreviewState::can_install` requires that base equal current portable source. Task accepted-base identity and candidate artifact identity need an explicit relationship. Forging a tag to make candidate bytes look like the accepted base bypasses the conflict contract.
- **Journal is metadata-only today:** journal events are Intent(Record)/Commit. `Controller::persist` writes intent/commit before SQLite, and replay can recover metadata; it does not encode source mutations or recover partially applied multi-file changes. New apply/Undo transaction data must include before/after object references, path operations and durable progress, with schema compatibility and crash recovery tests.
- **Protect external edits:** reconcile accepted base and current inventory immediately before Apply/Undo, then validate expected content hashes for each write/delete/create. Either preserve unrelated changes through a verified merge or surface conflict. Do not overwrite external edits or resurrect stale work after edit-back. Include executable-bit changes and new/deleted files; use project path guards and retain immutable before/after snapshots.
- **Undo is a durable revision:** record predecessor and task revision linkage; Undo must use the same guarded recoverable file-set mechanism, advance accepted only after durable completion, and survive restart. Existing Restore exports a checkpoint as a copy and must keep that safety behavior. It is not in-place Undo.
- **Freeze after writer ownership ends:** spike drops `AcpSession` before rebuilding. A production persistent session needs an explicit guarantee that task writers and background descendants are quiescent before snapshot capture; protocol completion alone does not prove detached writers stopped. Reconcile/snapshot checks should reject changing bytes rather than compile a mutable workspace.
- **Recovery ordering:** replay currently retains an uncommitted active-job draft without advancing accepted. Extend this invariant for interrupted promotion/Undo: opening must resolve durable file-set progress before trusting current source or changing accepted. Disk failure must halt journal appends until recovery, preserving the existing failed-writer guard.

## Minimal module/file ownership

1. `studio-agent-spike/src/{driver,session,supervisor}.rs`: normalize driver events/capabilities and evaluate official SDK integration while keeping process supervision separate. Retain qualification entry points; a dedicated production agent crate is optional, not required merely to rename the spike.
2. `studio-engine/src/agent-task.rs` (new): task packet/state, stable draft identity, finite repair policy and operation fences. `controller.rs` remains the serialized owner; extend `state.rs`/`store.rs` only for necessary persisted contracts.
3. `studio-engine/src/edit-transaction.rs` (new), plus `journal.rs` and `studio-project/src/checkpoint.rs`: verified file-set promotion/Undo and crash recovery, reusing durable object storage. One owner for these shared recovery/schema surfaces.
4. `desktop/app/src/build-service.rs` (new), `worker_project.rs`, `preview_coordinator.rs`: keyed compiler sharing and result/artifact leases; preserve existing latest-wins seek/display behavior. One owner for build integration until its API settles.
5. `desktop/app/src/agent-tools.rs` (new): one typed task-scoped dispatcher with MCP and local CLI adapters. Depend on the build service, not direct compiler entry points.
6. `desktop/app/src/conversation-panel.rs` (new), `studio_shell.rs`: virtualized/batched conversation and serialized task commands, review setting, Apply/Undo, permissions/questions, options and Stop. Integrate staged preview installation with transaction acceptance; preserve M2 audio/seek guards.

Owning architecture sections 4/5/9 and implementation-plan M3 describe the required outcome. Tests should start with transaction/state and transport boundaries, then dedup/cancellation, then native two-edit/Undo/restart acceptance. No test or build was run for this read-only scout.

Unresolved planning decisions: official ACP SDK candidate and compatibility gate; location/default/migration of project review policy; exact finite repair budget; deterministic reconciliation/recovery policy for interrupted Apply/Undo. These require explicit plan decisions, not inferred provider promises.
