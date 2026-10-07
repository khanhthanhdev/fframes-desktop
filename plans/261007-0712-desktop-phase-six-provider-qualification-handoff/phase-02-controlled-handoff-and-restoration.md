---
title: "Phase 2: Controlled handoff and restoration"
status: done
---

# Controlled handoff and restoration

## Overview and dependency

Priority P1. Tentative estimate 7–10 engineer-days. Depends on stage 1 profiles and evidence contracts. Extend the existing per-project actor and controller with explicit handoff and session restore states. No new orchestrator. Read [architecture ownership rules](../../docs/desktop/architecture.md#session-and-writer-ownership) and [baseline validation](../reports/planning-261007-0712-m6-baseline-and-validation.md).

## Verified control flow and lifetimes

`AgentWorkflow::open` creates a per-project actor (`desktop/app/src/agent_workflow.rs:327`); the actor owns blocking adapter work and publishes bounded snapshots. `begin_task` (`desktop/app/src/agent_workflow/actor.rs:1221`) reaches the controller's scoped task setup (`desktop/crates/studio-engine/src/controller.rs:751`), which reconciles current source, captures a source base, checks frozen scope, reserves one writer lease, seals the previous scope, then prepares the stable draft and installs the task. Current source base includes external dirty edits and is distinct from saved/accepted history; retain that distinction.

The actor starts a writer from task cwd but hardcodes `resume_session: None` (`desktop/app/src/agent_workflow/actor.rs:1541`). The existing driver already chooses negotiated resume before load, otherwise returns RestoreUnsupported (`desktop/crates/studio-agent-spike/src/driver/runtime.rs:1625`). Reuse that path. `wire_session_id` is an opaque driver value (`:2126`); the public SessionReady ID is sanitized (`:1706`). Never reconstruct a resumable ID from a display/redacted ID.

`DraftStore` is per-project and uses stable app paths (`desktop/crates/studio-engine/src/agent_task.rs:1083,1090`; `desktop/crates/studio-engine/src/app_paths.rs:55,63`). `prepare` archives retained drafts before refresh (`desktop/crates/studio-engine/src/agent_task.rs:1182`); `recover_after_restart` marks abandoned active ownership unsafe (`:1128`). This existing refresh cannot be called blindly for continued-draft handoff. `AgentTaskManager` is per-open-project/session (`:629,645`); keep new state there or in actor-owned records, never global mutable fields.

Writer generation and quiescence fencing remain authoritative (`desktop/crates/studio-engine/src/agent_task.rs:764,798,842,876`). Process scopes seal and observe before reuse (`desktop/crates/studio-bootstrap/src/process.rs:1178,1187,1196`). Shutdown returns verified termination; a prompt completion/cancellation result alone cannot authorize a replacement writer.

## Requirements and data flow

Switch intent → actor retains and pauses outgoing queue → switch-specific completion/stop → outstanding permissions/input/tool capabilities closed → existing scope sealed/reaped/observed → in-flight publication/Undo settled → existing tool snapshot gate → immutable outgoing draft capture plus current source/accepted identity → user chooses continue draft or restart from accepted → new task/writer generation and fresh tool grants → context refresh → new provider session → existing validation/capture/Apply/Undo path.

Preserve outgoing session and both versions until the user explicitly chooses. Preserve all observed variants on external edits/conflicts; compare the user-selected revision again before launch. A continued draft becomes the incoming working copy but does not change accepted source. Its ultimate candidate still validates and publishes against the live source-base/history fences, including outgoing draft changes. Stale selection is visibly invalidated rather than rebound to another element. Restart-from-accepted is explicit; if current source differs, preserve and present that source variant too.

Session restoration record → compatibility/ownership verification → same stable cwd → current context refresh → negotiated resume/load → replay boundary → fresh task. Unsupported/expired/rejected restoration retains the outgoing record and displays a fresh-session fallback with current explicit context. Never auto-resend an interrupted prompt or auto-apply a recovered candidate. Opening a project launches no adapter; the user initiates continuation.

Persist a bounded versioned app-local session manifest with provider/profile ID, opaque native ID, project/draft identity, exact launch/dependency fingerprint, last source/accepted revision, context schema, capability snapshot, transcript boundary and ended/unsafe state. Use owner-only atomic storage; redact IDs from logs/UI/evidence. Future manifest versions are read-only/ineligible to launch. Keep provider credentials in provider storage/environment.

Bounded context contains instruction-bundle identity, selected source/draft revision, task brief/outcome, frozen element/rectangle/scene/range scope, style snapshot and unresolved diagnostics. Reuse M4/M5 retrieval/tools; do not copy full transcript or repository. Proposed envelope maximum: 64 KiB UTF-8 text, within existing 256 KiB tool reply cap; apply existing image/artifact budgets and negotiated image support. Reserve required identity fields, truncate optional sections deterministically and report omissions. Refresh/reset incompatibility creates a new session instead of misleading continuity.

Queued briefs remain explicitly owned by the outgoing provider/session and their frozen revision. Do not silently retarget; allow cancel or an explicit transfer/re-freeze choice after handoff. Duplicate restore/replay events and stale permission replies are keyed by provider/session/task/writer epoch and sequence boundary; old events cannot affect the incoming task.

## File ownership

Paths are under `/root/fframes-desktop/`. Stage 2 owns all listed files sequentially; stage 3 receives workflow/UI integration files after this gate.

| Action | Exact path | Purpose |
|---|---|---|
| Modify | `desktop/app/src/agent_workflow.rs` | Typed switch/continue/restore commands |
| Modify | `desktop/app/src/agent_workflow/actor.rs` | Existing actor handoff and restore state machine |
| Modify | `desktop/app/src/agent_workflow/jobs.rs` | Existing job settlement and tool snapshot gate integration |
| Modify | `desktop/app/src/agent_workflow/model.rs` | Bounded immutable session/handoff views and queue ownership |
| Modify | `desktop/app/src/agent_workflow/tools.rs` | Revoke outgoing grants; reissue per task/writer epoch |
| Modify | `desktop/app/src/agent_workflow/log.rs` | Replay boundaries and continuity notices without changing old row semantics |
| Modify | `desktop/crates/studio-engine/src/controller.rs` | Explicit draft-choice task preparation and source/history fences |
| Modify | `desktop/crates/studio-engine/src/agent_task.rs` | Existing lease/retention model extension |
| Modify | `desktop/crates/studio-engine/src/app_paths.rs` | App-local session path |
| Modify | `desktop/crates/studio-agent-spike/src/driver/runtime.rs` | Minimal opaque-ID extraction/replay exposure only if existing API is insufficient |
| Create | `desktop/app/src/agent_workflow/session-store.rs` | Versioned durable manifest; explicit Rust path attribute |
| Create | `desktop/app/tests/provider_handoff.rs` | End-to-end actor handoff/restore fixture scenarios |
| Create | `desktop/crates/studio-engine/tests/provider_handoff.rs` | Retention, source fences and unsafe restart tests |
| Modify | `desktop/app/tests/support/acp-agent.py` | Deterministic restore/switch failure injection |

No changes to transaction publication algorithm or bootstrap process ownership unless a demonstrated regression requires them. No deletions or portable project schema changes.

## Implementation steps

1. Add failure-first tests for overlapping writers, continued unsaved draft loss, changed source during choice, delayed permissions/events and unsupported restoration. Trace task start, shutdown, candidate capture, log replay and queue advancement before editing.
2. Add durable session records independent of transcript rows. Preserve eligible opaque IDs exactly through the existing `wire_session_id()` API; an ID requiring credential redaction is deliberately unavailable and disables restoration with a visible notice. Never bypass that safeguard or reconstruct an ID from sanitized events. Store only after durable session-ready acknowledgement. A save failure visibly disables persistence, never claims restart continuity. Retain existing logs and use migration/version checks before mutation.
3. Add switch state to the existing actor. Retain the bounded outgoing queue and latch switch-pending before stopping/finalizing the task; suppress automatic `start_next`. Use a switch-specific stop path because ordinary Stop clears queued briefs. Keep ordinary user Stop behavior. Close replies, revoke capabilities, stop or wait for completion and use verified scope teardown. If any ownership/termination fact is unknown, block handoff and retain the draft; no incoming process may launch.
4. Wait for any in-flight pipeline, uncancellable Apply or Undo and consume its committed engine result before revision capture or choice. Preserve the cannot-cancel notice; recompute source/accepted/history and invalidate an earlier choice. A delayed/failed preview acknowledgement cannot undo accepted source or indefinitely block source continuation. Capture safe stopped draft and current accepted/source variants by existing revision storage. Reuse the existing `WriterGate` around capture and stable-cwd materialization: revocation alone does not drain an already executing tool snapshot. Completed immutable tools stay bound to the outgoing revision and cannot become incoming evidence. Expose an explicit choice; fence it on project/session/source/history and retained snapshot IDs. Add a cause-specific preparation route that materializes the chosen immutable snapshot at the stable cwd only after ownership proof, retaining archived bytes.
5. Assemble deterministic bounded context through existing scope/style/source tools. Obtain a fresh task identity/writer epoch and issue fresh grants; retired capability files and old MCP processes cannot act for the new task. Revalidate selection against chosen revision; show unresolved scope when it no longer matches.
6. Wire eligible manifest IDs into the driver's existing resume/load configuration at the same cwd. Recheck profile closure and negotiated capabilities at launch. Deduplicate replay, block stale replies and refresh context before Send; unsupported/load errors expose new-session fallback rather than automatic repeated retries.
7. Continue using existing candidate validation, single repair budget, durable Apply/Undo and preview promotion. Test continued-draft delta includes all retained edits without falsely advancing accepted history; project close/reopen keeps records and no process owner runs on the UI thread.

## Validation and matrix

```bash
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test provider_handoff
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test task_recovery
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test edit_transaction
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-agent-spike --test acp_v1
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test provider_handoff
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test agent_workflow
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test agent_tools
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test scoped_editing
```

New provider_handoff targets are proposed; all other targets exist. Unit tests cover bounded manifest migration/atomic failure and UTF-8 truncation. Integration tests cover accepted/dirty-source/unsaved draft choice, failed/cancelled turns, repair, unknown or detached writer, external change, late permission, retired tool token, two retained queued briefs without automatic successor launch, switch at Apply commit and during Undo, delayed/failed preview acknowledgement, a paused tool-snapshot race against stable-cwd replacement, supported load/resume, unavailable/expired IDs, changed adapter closure, missing cwd, interrupted save and replay deduplication. Existing transaction/Undo/reopen tests must continue passing. Fixture evidence is development-only.

## Todo and success criteria

- [x] No incoming writer starts until all outgoing process ownership checks pass.
- [x] Both draft and accepted/source variants remain retrievable before and after a choice or failure.
- [x] Continued draft validates/publishes through existing fences without silent reconciliation loss.
- [x] Opaque IDs round trip; supported restart uses identical cwd and fresh grants.
- [x] Unsupported restoration is visibly a new session with bounded refreshed context.
- [x] Stale replies/events/tokens and queued briefs cannot cross provider epochs.
- [x] Crash/reopen does not resend work, overwrite unknown files or auto-publish candidates.

## Risk, security and rollback

Medium likelihood × critical impact: escaped/unknown writers; block capture/switch, retain variants, require existing writer-gone recovery evidence. Medium × high: refresh destroys an unsaved draft; immutable capture and explicit fenced choice precede preparation. Medium × high: stale native memory/tools; compatibility checks, context refresh and fresh capabilities precede continuation. Medium × high: replay/persistence bugs; atomic manifest writes, exact opaque IDs, sequence dedupe and failure injection. Medium × medium: unbounded context/logs; use existing resident budgets and measured envelope limits.

Rollback disables handoff/restore commands and reverts to fresh sessions. Retain manifests, immutable snapshots and archives; old builds ignore unsupported manifest versions. Never roll source back outside guarded existing Apply/Undo. If shutdown is unproven, rollback still blocks writer reuse. Stage 3 starts after the full development safety matrix passes; authentic provider safety is qualified separately.
