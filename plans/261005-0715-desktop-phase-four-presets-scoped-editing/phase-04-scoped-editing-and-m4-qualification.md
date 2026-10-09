---
phase: 4
title: "Scoped editing and M4 qualification"
status: in-progress
priority: P2
effort: "4-6 engineer-days"
dependencies: [1, 2, 3]
---

# Phase 4: Scoped editing and M4 qualification

## Goal

Carry the current compiled whole-project/scene/range selection and active style snapshot into a revision-frozen agent task, show honest scope/evidence in the native workflow, broaden candidate validation when edits can escape scope, and record M4 development/authentic gates separately.

## Context and key insights

- M2 already owns half-open integer-frame selections, scene-instance IDs, overlap cycling and compiled scene boundaries in `studio-engine::TimelineSelection` / `desktop/app/src/timeline_view.rs`. The selection is currently private to the view and is not passed to `AgentWorkflow::submit`.
- M3 `AgentTaskContext` is explicitly whole-project-only. `present::first_prompt` sends the brief, instructions and assets; current tool methods are the six read-only project/timeline/frame/strip/inspect/build-status operations. This phase enriches task context and existing calls, not the tool-name surface.
- `PreviewSceneInfo` gives compiled instance/name/full-name and half-open bounds but no source anchor. M4 source lookup must be deterministic, bounded and visibly best-effort; stable element identities, spans and syntax retrieval stay in M5.
- `candidate_validation::broadening` already expands inspection for Rust/Cargo/Style/Configuration changes and uncertain shared files. Preserve and test this safety behavior after adding a selected-range view; selection is never a claim that only those frames can change.
- The current M3 ledger has an authentic adapter probe but leaves full authenticated edit/recovery and other hardware/platform gates `not_run`. A fixture proves the packet and lifecycle contract, not provider compatibility.

## Requirements

- [x] Add typed `TaskScope` values for whole project, compiled scene instance and absolute frame range. Store half-open `[start,end)` indices, fps and the exact project/source/preview identity that produced the selection; validate `0 <= start <= end <= total_frames`.
- [x] Keep existing `AgentWorkflow::submit` and M3 callers as whole-project wrappers; add a scoped submit path and freeze selection at the user's submit action. Queue entries retain their scope intent and visible preview identity.
- [x] Before task start, reconcile the scope against the current accepted/displayed compiled revision. A stale scene or changed timeline is re-resolved only when exact identity can be proven; otherwise require reselection/confirmation and never silently map old coordinates to new timing.
- [x] Include the selected compiled scene/range, overlapping scenes, adjacent scene boundaries, active preset id/hash and resolved tokens, source-base identity, deterministic scene source candidates, and bounded before screenshot/strip artifact references in the initial task context.
- [x] Generate before evidence on a background worker from the same immutable source/preview identity. Attach image content only when the negotiated ACP capability permits it; otherwise retain bounded app-owned artifact references and a visible text-only limitation. Never put large inline base64 images in prompt/transcript storage.
- [x] Resolve scene source candidates by bounded exact symbol/name matching in project Rust source. Return path/symbol/hash and confidence/ambiguity; if missing or ambiguous, say so and keep the selected scene/range as the scope. Do not add automatic source spans or an AST/symbol index.
- [x] Display the pending scope in the task/queue and ghost-highlight the timeline range/scene while retaining the real current preview. Do not invent predicted result pixels; the native chat shows actual task-owned before/after thumbnails alongside the frozen revisions and frame references.
- [x] Candidate validation must cover the requested frames and adjacent scene boundaries, then use M3 change/dependency rules to broaden to whole-video inspection for Rust, Cargo, style/preset or unknown/shared changes. Report actual coverage and never label an under-covered candidate accepted.
- [x] Include before/after artifacts, changed paths, validation coverage, style snapshot id and candidate revision in the native review/outcome presentation; invalidation or task failure releases only its own artifacts.
- [x] Create an M4 qualification ledger/schema and extend the validator for development, real-SDK render, native Linux and authentic-provider gates. Keep unavailable gates `not_run`/blocked and never infer a full edit from adapter readiness.

## Architecture

Add a GPUI-free scope model to `studio-engine` and store it in the frozen `AgentTaskContext` alongside the task's base source and style snapshot hash. The native timeline exposes an immutable selection snapshot; the conversation panel submits that snapshot through the existing serialized workflow actor. Build the task packet from the selected compiled timeline, pre-prompt source and resolved style data; use existing worker/tool artifact bounds and ACP image negotiation.

For a scene scope, include the instance id/name, `[start,end)`, overlapping active instances and previous/next boundary frames. For a range scope, include the selected interval and each scene intersecting it plus neighboring scene endpoints. Whole-project remains the fallback for imported projects or absent selection. Current selection uses the displayed worker identity; queued selections whose source/worker identity changed become visibly stale and must be reselected or explicitly rebound to a newly compiled exact scene instance.

Keep M3's validation broadening invariant. Scope narrows the agent's initial attention and adds mandatory selected/boundary samples; it does not narrow required safety inspection for changed shared helpers, style tokens, media/configuration, unknown files or ambiguous dependencies. The validation report identifies requested scope separately from actual inspected/rendered coverage.

## Files to create or modify

All paths are rooted at `/root/fframes-desktop/`.

| Action | Path | Responsibility |
|---|---|---|
| Create | `desktop/crates/studio-engine/src/task_scope.rs` | Validated source/preview-bound scope packet and deterministic scene source candidates. |
| Modify | `desktop/crates/studio-engine/src/agent_task.rs`, `desktop/crates/studio-engine/src/controller.rs`, `desktop/crates/studio-engine/src/lib.rs` | Add scoped task start/context while preserving whole-project wrapper and M3 fences. |
| Create | `desktop/crates/studio-engine/tests/agent_task_scope.rs` | Scope serialization, half-open boundaries, stale identity and fallback tests. |
| Modify | `desktop/crates/studio-engine/src/candidate_validation.rs`, `desktop/crates/studio-engine/tests/candidate_validation.rs` | Selected/boundary coverage plus unchanged shared-file broadening rules. |
| Modify | `desktop/app/src/timeline_view.rs`, `desktop/app/src/studio_shell.rs` | Expose immutable selection/preview identity and visible pending-scope overlay. |
| Modify | `desktop/app/src/conversation_panel.rs`, `desktop/app/src/conversation_panel/controls.rs` | Submit scoped briefs, show frozen scope/status, preserve focus/input behavior. |
| Modify | `desktop/app/tests/x11_shell.rs` | Exercise selected scope, stale-scope and before/after states through the native shell on owned Xvfb. |
| Modify | `desktop/app/src/agent_workflow.rs`, `desktop/app/src/agent_workflow/actor.rs`, `desktop/app/src/agent_workflow/model.rs`, `desktop/app/src/agent_workflow/present.rs` | Carry scope through queue/task snapshots and construct bounded initial prompt/evidence. |
| Modify | `desktop/app/src/agent_workflow/tools.rs`, `desktop/app/src/agent_tools/backend.rs` | Bind existing tool evidence/artifact references to the frozen scope/revision without adding methods. |
| Create | `desktop/app/tests/scoped_editing.rs` | Submit/queue/stale-scope, evidence, review and UI-state integration tests. |
| Create | `desktop/qualification/m4-results.json`, `desktop/qualification/m4-results.schema.json` | M4 pass/fail/blocked/not-run ledger with hashed evidence. |
| Modify | `desktop/scripts/validate-qualification.py` | Validate M4 gates and reject missing hashes, false passes and leaked credentials. |
| Create | `desktop/scripts/test-qualification-m4.py` | Regression tests for M4 ledger validation. |
| Modify | `docs/desktop/architecture.md`, `docs/desktop/README.md` | Document implemented scope packet, preset behavior and evidence limits after gates pass. |

## Tasks & steps

### Task 4.1 — Define immutable, half-open task scope

- **Goal:** Represent whole-project, compiled-scene and absolute-frame-range scopes bound to the exact project/source/preview that generated them.
- **Target files and symbols:** New `desktop/crates/studio-engine/src/task_scope.rs`; `desktop/crates/studio-engine/src/lib.rs`; new `desktop/crates/studio-engine/tests/agent_task_scope.rs`.
- **Steps:** Define typed whole-project/scene/range variants; store `[start,end)`, fps, total frames, scene instance identity and project/source/preview identity; validate bounds and non-finite fps; test overlaps, empty ranges, end exclusivity and stale identities.
- **Success criteria:** Scope serialization round-trips; invalid/missing identities and out-of-bounds ranges return explicit errors; empty ranges are represented without accidentally rendering an adjacent frame.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test agent_task_scope` exits 0 and prints `test result: ok`.

### Task 4.2 — Freeze scope through submit, queue and task context

- **Goal:** Preserve the selected scope from the user's submit action through serialized task creation and reject stale queued scopes rather than remapping silently.
- **Target files and symbols:** `desktop/crates/studio-engine/src/agent_task.rs`, `desktop/crates/studio-engine/src/controller.rs`, `desktop/crates/studio-engine/src/lib.rs`; `desktop/app/src/timeline_view.rs`, `desktop/app/src/studio_shell.rs`, `desktop/app/src/conversation_panel.rs`, `desktop/app/src/conversation_panel/controls.rs`; `desktop/app/src/agent_workflow.rs`, `desktop/app/src/agent_workflow/actor.rs`, `desktop/app/src/agent_workflow/model.rs`.
- **Steps:** Keep the existing `Controller::begin_agent_task` and `AgentWorkflow::submit` signatures as whole-project wrappers; add distinct scoped entry points and route the conversation submit control through the scoped path. Capture immutable selection/preview identity in the queue entry; on start reconcile against exact source/compiled identity; require visible reselection/confirmation if identity cannot be proven; include active style snapshot identity. The current `AgentTaskContext` literal construction sites are `desktop/crates/studio-engine/src/controller.rs:760` and `desktop/crates/studio-engine/src/agent_task.rs:1284`; update these two sites and re-run `grep -R -n 'AgentTaskContext {' desktop --include='*.rs'` immediately before editing to catch drift. The test helpers at `desktop/app/tests/promotion_handoff.rs:89`, `desktop/app/tests/candidate_runner.rs:96`, `desktop/crates/studio-engine/tests/candidate_validation.rs:62` and `desktop/crates/studio-engine/tests/support/tx_fixture.rs:109` delegate to the controller and are not additional struct literal constructors.
- **Success criteria:** Whole-project callers remain behaviorally unchanged; queued context retains its original identity; stale timelines are refused or explicitly rebound only after exact identity matching.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test agent_task_scope --test agent_task` and `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test agent_workflow --test scoped_editing` exit 0 and print `test result: ok`.

### Task 4.3 — Build bounded before-evidence and scene source candidates

- **Goal:** Attach truthful, revision-matched baseline artifacts and best-effort deterministic scene source candidates to scoped task context.
- **Target files and symbols:** `desktop/crates/studio-engine/src/task_scope.rs`; `desktop/app/src/agent_workflow/model.rs`, `desktop/app/src/agent_workflow/present.rs`, `desktop/app/src/agent_workflow/tools.rs`, `desktop/app/src/agent_tools/backend.rs`; `desktop/app/tests/scoped_editing.rs`.
- **Steps:** Generate selected/boundary screenshots or strips off the UI thread; bind artifacts and style/source hashes to the frozen task identity; enforce existing size/TTL limits and ACP image negotiation; search bounded Rust source by exact scene name/full name; return path/symbol/hash/confidence/ambiguity without claiming spans or exact selection; clean only task-owned artifacts on cancellation/close.
- **Success criteria:** Every attached artifact matches the task revision; unsupported image transport yields a visible text-only limitation; missing/ambiguous source candidates are represented as such; no inline base64 or outside-project path leaks into transcript context.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test scoped_editing` exits 0 and prints `test result: ok`.

### Task 4.4 — Present scope and before/after review in the native shell

- **Goal:** Show queued/running scope and actual before/candidate evidence while retaining the current real preview and existing conversation input behavior.
- **Target files and symbols:** `desktop/app/src/timeline_view.rs`, `desktop/app/src/studio_shell.rs`, `desktop/app/src/conversation_panel.rs`, `desktop/app/src/conversation_panel/controls.rs`; `desktop/app/tests/scoped_editing.rs`, `desktop/app/tests/timeline_controls.rs`, `desktop/app/tests/x11_shell.rs`.
- **Steps:** Add pending scene/range badge and ghost highlight only; retain current preview pixels until an actual candidate exists; show before/after artifacts and source/style revision; exercise reverse drag, overlaps, rebuild during edit, stale queued selection, keyboard and IME behavior.
- **Success criteria:** UI tests distinguish requested scope, frozen before revision, candidate revision and actual coverage; no predicted/fabricated preview pixels appear; existing focus and IME tests still pass.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test scoped_editing --test timeline_controls --test agent_workflow_ui` exits 0 and prints `test result: ok`; with `SDK_ACTIVE` exported as the path to an installed `.fframes/sdk/active` directory and `Xvfb`/`xdotool` on `PATH`, `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test x11_shell -- --ignored --nocapture` exits 0 and prints the native shell test as passed.

### Task 4.5 — Preserve safety broadening and report actual coverage

- **Goal:** Require selected and boundary samples while preserving M3 whole-video inspection for changes that can escape the selected scope.
- **Target files and symbols:** `desktop/crates/studio-engine/src/candidate_validation.rs`; `desktop/crates/studio-engine/tests/candidate_validation.rs`; `desktop/app/tests/scoped_editing.rs`.
- **Steps:** Add scope-aware requested samples and adjacent scene boundaries; preserve broadening for shared Rust/Cargo/style/preset/media/configuration/unknown changes, truncated diffs and uncertain dependencies; expose requested versus inspected/rendered coverage in the validation report.
- **Success criteria:** Isolated scoped changes validate selected and boundary frames; shared/uncertain changes trigger existing broader inspection; no candidate is accepted when required coverage is missing.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test candidate_validation --test agent_task_scope` and `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test scoped_editing` each exit 0 and print `test result: ok`.

### Task 4.6 — Add M4 ledger validation and evidence rules

- **Goal:** Record M4 development, managed-SDK, native Linux and authentic-provider gates without turning missing evidence into a pass.
- **Target files and symbols:** New `desktop/qualification/m4-results.json`, new `desktop/qualification/m4-results.schema.json`; `desktop/scripts/validate-qualification.py`; new `desktop/scripts/test-qualification-m4.py`.
- **Steps:** Define `pass`/`fail`/`blocked`/`not_run` gate states and evidence hashes; require evidence for passes; reject credentials/secrets; keep authenticated provider and untested platform gates `not_run` or `blocked`; add validator regression fixtures.
- **Success criteria:** Missing/incorrect hashes, false passes and credential-like values are rejected; an all-not-run initial ledger validates and claims no unperformed qualification.
- **Verify:** `python3 desktop/scripts/test-qualification-m4.py` exits 0 and prints its success summary; `python3 desktop/scripts/validate-qualification.py desktop/qualification/m3-results.json` exits 0, preserving M3 ledger compatibility; `python3 desktop/scripts/validate-qualification.py desktop/qualification/m4-results.json` exits 0 for the initial evidence-complete ledger.

### Task 4.7 — Run integrated Linux and managed-SDK qualification

- **Goal:** Demonstrate scene/range scoping, revision coherence, safe validation broadening, preset context, and regression compatibility on the supported M4 development target.
- **Target files and symbols:** `desktop/app/tests/scoped_editing.rs`; `desktop/crates/studio-engine/tests/agent_task_scope.rs`, `desktop/crates/studio-engine/tests/candidate_validation.rs`; `desktop/qualification/m4-results.json`; `desktop/scripts/test-preset-relocation.py`; `desktop/app/tests/x11_shell.rs`.
- **Steps:** Build a fresh managed-SDK project; submit whole-project, scene, overlap and range tasks; exercise stale queue, failed build, candidate before/after evidence and source/style identity; verify relocation and two-preset render; run focused M0–M3 suites, Clippy, formatting and Linux/Xvfb UI checks; record authentic provider, display/audio and other-platform outcomes separately.
- **Success criteria:** Linux development gates pass with hashed evidence; source/preview/style identities agree; stale scopes never retarget silently; M3 broadening remains active; unexercised provider/platform/physical gates stay `not_run`.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets -p studio-project -p studio-engine -p fframes-studio` and `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio-protocol -p fframes-studio-runtime` each exit 0 and print `test result: ok`; `cargo test --locked -p fframes --features styles` exits 0 and prints `test result: ok`; `cargo clippy --locked --manifest-path desktop/Cargo.toml --workspace --all-targets -- -D warnings` and `cargo clippy --locked --manifest-path Cargo.toml -p fframes --lib -- -D warnings` each exit 0; `cargo fmt --manifest-path desktop/Cargo.toml --all --check` and `cargo fmt --manifest-path Cargo.toml --all --check` each exit 0; `python3 desktop/scripts/test-qualification-m4.py` and `python3 desktop/scripts/test-preset-relocation.py` exit 0; with `SDK_ACTIVE` exported as the path to an installed `.fframes/sdk/active` directory and `Xvfb`/`xdotool` on `PATH`, `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test x11_shell -- --ignored --nocapture` exits 0; `python3 desktop/scripts/validate-qualification.py desktop/qualification/m4-results.json` reports all claimed development gates `pass` and all unexercised gates `not_run`/`blocked`.

## Verification

```sh
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test agent_task_scope --test candidate_validation
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test scoped_editing --test agent_workflow --test timeline_controls
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project -p studio-engine -p fframes-studio
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio-protocol -p fframes-studio-runtime
cargo clippy --locked --manifest-path desktop/Cargo.toml --workspace --all-targets -- -D warnings
cargo clippy --locked --manifest-path Cargo.toml -p fframes --lib -- -D warnings
cargo fmt --manifest-path desktop/Cargo.toml --all --check
cargo fmt --manifest-path Cargo.toml --all --check
python3 desktop/scripts/test-packaging.py
python3 desktop/scripts/test-qualification-m4.py
python3 desktop/scripts/validate-qualification.py desktop/qualification/m4-results.json
```

Run the native shell under the existing Linux X11/Xvfb workflow and inspect representative whole-project, selected-scene, selected-range, overlap, stale-scope and failed-build states. Use the managed SDK to prove the same source renders in two preset snapshots and M4 scope context is revision-matched. Record process/artifact cleanup and exact coverage. Physical display/audio, Windows/macOS, authenticated provider edit/repair and provider image handling remain separate evidence gates unless actually exercised.

## Failure Protocol
If a Verify step fails, stop the affected gate, preserve the exact command/output and
investigate the specific failure before changing code or updating evidence. Use an
available advisor agent for an independent, bounded analysis when useful; do not blindly
retry, infer a pass from partial output, or use an unavailable advisor as a gate.

## Risks and rollback

| Risk | Response |
|---|---|
| Selected scope is stale after queued task or rebuild | Bind to project/source/preview identity; re-resolve only exact scene identity on matching compiled source; otherwise ask for reselection and preserve the brief without launching an incorrectly scoped task. |
| Agent changes shared helper or preset token and affects outside frames | Keep broadening for Rust/Cargo/style/shared or unknown changes; show full-project coverage requirement and never imply scope isolation. |
| Screenshot/source refs cross task revisions or leak project paths | Bind refs to task/project/revision, keep them app-owned/bounded, redact outside-project paths and expire/release on close. |
| Best-effort source search claims an exact implementation incorrectly | Return ambiguity/confidence and all bounded candidates; no automatic spans or exact-selection claim in M4. |
| M3 provider qualification is mistaken for M4 success | Separate M4 development and authentic gates; adapter probe alone is not an edit workflow. |

Rollback keeps whole-project submit, existing timeline selection and M3 validation behavior. Disable scoped submission/overlay without removing saved preset snapshots or task evidence; do not rewrite source/history or clear the last preview.

## Implementation notes

- Scoped workflow tests: `desktop/app/tests/scoped_editing.rs` (shared harness `tests/support/workflow_world.rs`, also used by `agent_workflow.rs`), unit tests in `agent_workflow/present.rs`, `studio-engine` `agent_task_scope.rs` / `candidate_validation.rs`, and native `x11_shell` tests (scene, range, before/after review, stale queued scope).
- The task packet's first prompt carries the style snapshot identity and a bounded list of resolved tokens read from the working copy's `style/tokens.json`.
- Native evidence is the scripted ACP peer on Xvfb; prompt PNG blocks are capability-negotiated and bounded, while before/after review previews are decoded locally from task-owned PNG artifacts. The telemetry records only preview counts, not paths or pixels. The scripted peer is not provider qualification.
- The managed-SDK preset render passes against a locally assembled SDK built from this checkout, directly and without the overlay branch. This is development evidence, not a published SDK release. The Linux scope UI test verifies scene evidence while ManualReview is pending and verifies that the default AutoApply terminal snapshot already contains both decoded before/after thumbnails before finalization releases task artifacts. The terminal panel distinguishes the last task's frozen scope from the next submission scope.
- Authenticated-provider, Windows and macOS gates remain `not_run`; therefore the ledger's `m4_complete` remains `not_run` even though the M4 implementation and Linux development gates are complete.
- Unrelated and pre-existing at HEAD: five `M3LedgerTests` failures in `desktop/scripts/test-packaging.py` (identical names on HEAD and in this tree after the default-ledger test was updated for the M4 ledger).

## Todo list

- [x] Add and test revision-bound scope packet and stale-queue behavior.
- [x] Wire scene/range submit, style/visual context and bounded before/after review to the native workflow.
- [x] Preserve whole-project broadening for shared/uncertain edits and test actual coverage.
- [x] Pass M4 ledger/native/managed-SDK development gates and update docs with only measured evidence; authentic-provider and non-Linux gates remain explicitly `not_run`.

## Success criteria

A user can select a compiled scene or half-open frame range, prompt a change, see the exact frozen scope and current before evidence, then review a validated candidate with matching source/style revisions and actual coverage. Shared changes are inspected more broadly; stale scopes never target a different timeline silently; fixture success is not reported as provider qualification.
