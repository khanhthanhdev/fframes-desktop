# M6 failure-mode review

Reviewed 2026-10-07 (Asia/Saigon). Read all four M6 plan files and traced actor shutdown, process observations, draft restart/preparation, tool revocation/capture and publication settlement. No plan edits, execution tests or lint/build review. Source behavior supports the overall design; three medium execution ambiguities should be tightened. No Critical or High finding established.

## Findings

### Medium: Handoff must not invoke ordinary Stop before preserving the outgoing queue

**Phase:** 2 implementation step 3 and phase 3 picker integration. The plan preserves outgoing queued briefs and offers cancel or explicit transfer, but says to request Stop without naming the existing Stop side effect.

**Evidence:** `desktop/app/src/agent_workflow/actor.rs:2276–2281` immediately decrements pending and clears the entire queue before stopping the task. `:3441` starts the next brief after finalize. Phase 2's plan requires outgoing queue ownership at `phase-02-controlled-handoff-and-restoration.md:34` and a queue freeze at `:62`.

**Scenario:** A user switches while one brief runs and two follow-ups wait. Reusing `stop()` to reap the old writer silently removes both follow-ups before the handoff choice can offer transfer. Conversely, reusing finalize without the switch freeze can launch one queued writer before the switch finishes.

**Smallest fix:** Name a switch-specific stop path that first retains the bounded queue and latches handoff-pending before finalize, suppresses `start_next`, and does not use ordinary Stop's queue-clear behavior. Add a fixture with two outgoing queued briefs asserting they survive until explicit cancel/transfer and no successor starts automatically. Ordinary user Stop remains unchanged.

### Medium: Revision choice must await publication/Undo settlement, not only writer teardown

**Phase:** 2 handoff state machine and phase 3 UI state race matrix. The plan's source/history fences and existing publication protections are appropriate, but it does not explicitly order switch against an uncancellable in-flight Apply or Undo.

**Evidence:** `desktop/app/src/agent_workflow/jobs.rs:105–128` makes cancellation fail once commit begins. `desktop/app/src/agent_workflow/actor.rs:2338–2344` reports that publication cannot stop. `:2742–2757` installs the committed outcome even when Stop raced it. `:2283–2291` has analogous Undo handling. Existing project close explicitly joins jobs and drains committed results before ending ownership (`:3454–3484`), whereas the planned switch flow at phase 2 `:24` only states completion/Stop and process teardown before capture.

**Scenario:** Switch arrives after Apply entered commit. The provider writer is already stopped, so a writer-only barrier passes while publication updates source/history. A displayed variant choice or persisted session/context captured before settlement names a revision that is no longer current. Fences may reject it safely, but the user is offered an avoidably stale or misleading accepted version.

**Smallest fix:** Specify switch-pending waits for the identified pipeline/publication/Undo job and its committed engine result before immutable variant capture. Preserve the existing cannot-cancel notice. Recompute accepted/source/history and invalidate any earlier choice. Add deterministic switch-at-begin-commit and switch-during-Undo tests, including delayed preview acknowledgement; preview failure must not revert accepted source or block safe source continuation indefinitely.

### Medium: Handoff capture/materialization must reuse the tool snapshot gate

**Phase:** 2 implementation steps 3–5. Revoking old grants and issuing fresh ones is necessary, but revocation alone does not synchronously stop an already executing tool snapshot.

**Evidence:** `desktop/app/src/agent_tools/broker.rs:653–670` revokes flags/files and wakes workers; it does not wait for executing calls to finish. `desktop/app/src/agent_tools/backend.rs:166–189` defines the short snapshot `WriterGate`. Existing capture explicitly acquires that gate after reaping the provider (`desktop/app/src/agent_workflow/jobs.rs:287–302`). `desktop/app/src/agent_workflow/tools.rs:153–156` unregisters old task backend state on end. Phase 2 names revocation and retained revision capture at `:62–64` but omits this serialization boundary.

**Scenario:** An old authorized read-only tool is capturing the stable draft as switch revokes its grant. The new preparation route materializes a chosen snapshot at the same cwd while the old snapshot is still reading it. No overlapping provider writer is needed to produce mixed tool bytes or contention with the incoming task.

**Smallest fix:** Explicitly reuse the existing bounded tool snapshot gate for outgoing snapshot capture and stable-cwd replacement, with revocation before the gate and task unregister after safe settlement. Old immutable build work can finish with its old revision identity; it must not be treated as new-task evidence. Add a paused tool-snapshot race test rather than introducing a new broker supervisor or waiting for every immutable build.

## Existing recovery protections confirmed

- Driver shutdown latches cancellation, closes requests and reports teardown rather than treating prompt cancellation as proof (`desktop/crates/studio-agent-spike/src/driver/runtime.rs:2461–2518`). Actor propagates sticky escaped/unqualified ownership before finishing (`desktop/app/src/agent_workflow/actor.rs:3308–3336`). Controller seals, observes escapes before killing and records termination (`desktop/crates/studio-engine/src/controller.rs:1079–1092`). The plan correctly preserves these protections.
- An interrupted active draft becomes UnsafeWriter on reopen (`desktop/crates/studio-engine/src/controller.rs:272–277`; `desktop/crates/studio-engine/src/agent_task.rs:1128–1138`). Preparation refuses Active/UnsafeWriter and archives Retained drafts (`:1182–1205`). Session manifests must remain subordinate to these facts; an ended session field alone must never unlock a draft.
- Accepted-draft preparation currently removes the accepted working directory, while retained drafts are archived (`desktop/crates/studio-engine/src/agent_task.rs:1201–1210`). The plan correctly requests a dedicated explicit-choice route rather than reusing refresh blindly.
- Opaque session restoration already has an API that refuses IDs requiring redaction (`desktop/crates/studio-agent-spike/src/driver/runtime.rs:2124–2129`); the updated phase 2 step correctly preserves that limitation.

## Ten stage 3 claims verified

| Claim | Verification |
|---|---|
| Native saved adapter config/open belongs to host | `desktop/app/src/conversation_panel/host.rs:264,270,449–477` loads/resolves saved config and opens workflow |
| Global PATH is not consulted by host | `desktop/app/src/conversation_panel/host.rs:426–430` explicitly sets managed adapter directory and no GUI PATH |
| Workflow command methods enqueue | `desktop/app/src/agent_workflow.rs:525–527,571–575,713–720` sends actor messages |
| Permission replies to closed cards fail | `desktop/app/tests/agent_workflow_ui.rs:483` exercises answer-once and late reply behavior; driver `runtime.rs:2416–2422` uses correlated permission handling |
| Existing video CLI harness uses managed environment | `desktop/app/tests/real_sdk_selection.rs:20–35` launches project Cargo offline with SDK child environment |
| Existing managed selection test requires SDK_BUNDLE | `desktop/app/tests/real_sdk_selection.rs:38–45` ignored prerequisite and exact environment read |
| Native canvas X11 test needs SDK_BUNDLE and native helpers | `desktop/app/tests/x11_shell.rs:1013–1015` names SDK_BUNDLE, Xvfb, xdotool, xwd and ffmpeg |
| Package name and helper binaries exist | `desktop/app/Cargo.toml:2,7,11–20` names fframes-studio default run, studio-tools and studio-mcp |
| All nine named Studio tools exist | `desktop/app/src/agent_tools.rs:187–237` defines project_context, timeline, render_frame, render_strip, inspect, build_status, selection_context, source_lookup and style_context |
| Conversation resident bounds are 400 rows/4 MiB | `desktop/app/src/agent_workflow/model.rs:26–28` defines bounds; `log.rs:48–59` adopts them by default |

Proposed provider_handoff, provider_profiles and real_sdk_provider_handoff targets are correctly labelled new. Backend CLI export is correctly separated from M7 native export/installer release acceptance. The plan does not predeclare authenticated provider success or choose two winners without evidence.

Status: DONE_WITH_CONCERNS
Summary: Source-backed review found no Critical/High issue; three medium handoff-ordering details need explicit execution language and regression cases.
Concerns: Queue preservation before Stop, publication/Undo settlement before choices, and tool-snapshot gate reuse before stable cwd mutation.
