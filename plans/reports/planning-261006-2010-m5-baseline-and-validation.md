# M5 planning baseline and validation

2026-10-06, working directory `/root/fframes-desktop`, timezone Asia/Ho_Chi_Minh, English. Planning only: no implementation, compilation, native process or provider session was started.

## Desired outcome and scope

Deliver the full M5 roadmap: click an object, resolve revision-matched Rust/source/style context and prompt a validated edit. Preserve prior source/recovery, preview/audio, agent/tool and style/scope contracts. No --yagni was passed. Automatic macro spans are conditional on proof; semantic vector retrieval, provider handoff and packaging/export are outside M5.

The selected hard route uses a read-only render scout and three independent review lenses. Product choices come from the existing roadmap; final macro/runtime syntax is an empirical Stage 1 gate rather than a guess or a user interview question. No optional product question was necessary.

## Previous-phase evidence

- M0 proves explicit-anchor lookup in a feasibility fixture; its static Rects are not animated frame geometry. See the [render scout](scout-261006-2010-m5-render-identities.md).
- M1 current code supplies validated inventory, checkpoints, immutable objects and recovery. Saved checkpoint bytes and accepted validated task revisions remain distinct.
- M2 ledger has PASSED revision handoff, native controls, pixel parity, resources and cleanup; output_clock is NOT_RUN and audio_modes is PENDING. These are recorded evidence states, not a claim of physical or cross-platform qualification.
- M3 current code implements the driver/workflow/tools/validation/publication/recovery route despite stale Pending entries in its older plan. Its ledger has development passes and a passing authentic adapter probe; ten other authentic gates remain not_run. Do not infer a completed provider workflow from the probe.
- M4 ledger records five development passes, including native Linux and assembled managed-SDK preset rendering. Authenticated scoped edit, Windows and macOS remain not_run; full m4_complete remains not_run.

No prior plans or ledgers are rewritten to mask stale status or incomplete authentic evidence. Qualification ledgers were inspected, not re-executed in this planning task.

## Contract consumer inventory

Confirmed against current source on 2026-10-06; repeat these searches immediately before implementation because line numbers can drift.

- AgentTaskContext has **two literal construction sites**: `desktop/crates/studio-engine/src/controller.rs:808` and `desktop/crates/studio-engine/src/agent_task.rs:1287`. Helpers in candidate_runner, promotion_handoff, candidate_validation and support/tx_fixture delegate to Controller rather than constructing additional literals.
- TaskScope has **three external literal sites**: `desktop/app/tests/scoped_editing.rs:27`, `desktop/app/tests/scoped_editing.rs:209`, `desktop/crates/studio-engine/tests/candidate_validation.rs:548`, plus constructors in `task_scope.rs`. New variants affect its validate/label/prompt/from_timeline matches; preserve old whole-project/scene/range serialized inputs.
- Tool-name/parameter/response ownership: `desktop/app/src/agent_tools.rs` (ToolCall/ToolRequest/dispatcher); `agent_tools/backend.rs` (backend exhaustive dispatch and immutable revisions); `agent_tools/client.rs:742` (run_cli and list/help); `agent_tools/mcp.rs` (tools/list/call); `agent_workflow/tools.rs` (task runtime and before evidence). Both helper binaries delegate to these modules. Tests are `desktop/app/tests/agent_tools.rs` and `build_sharing.rs`; add selection_tools rather than another tool server.
- Frame contract ownership: protocol preview.rs, runtime preview_worker.rs, app preview_worker_client.rs/coordinator.rs, engine preview_state.rs and product studio_shell.rs. The legacy spike's DisplayedSourceFrame/PreviewElement is not the product shell.
- Runtime geometry hooks belong in `fframes/src/renderer/preview.rs` using the same converted tree as pixels; occurrence propagation also crosses `scenes.rs`, `video.rs`, `fframes_context.rs` and subtree composition in `svgr.rs`.
- M4 style_context must read the captured project-local preset/tokens through existing preset resolution, never current global catalog state. A project may legitimately lack a preset or explicit token binding.

## Verification results

Standard tier: Fact Checker and Contract Verifier, ten factual samples per stage. **40 verified, zero unresolved failures, zero unverified existing-code claims.** One initial sample incorrectly searched the thin studio_tools binary for the build_status method string; source inspection showed CLI ownership in agent_tools/client.rs, and the sample was corrected to run_cli. No behavior/plan requirement changed from that correction. Proposed APIs, tests and files are labelled proposed and await implementation/proof, not counted as existing facts.

| Stage | Evidence | Sampled fact | Result |
|---|---|---|---|
| 1 | `fframes/src/scenes.rs:47` | `pub trait Scene` | VERIFIED |
| 1 | `fframes/src/svgr.rs:16` | `into_svg_tree` | VERIFIED |
| 1 | `fframes/src/renderer/preview.rs:451` | `render_inspected` | VERIFIED |
| 1 | `fframes-studio-runtime/src/anchors.rs:8` | `pub struct ElementRegistration` | VERIFIED |
| 1 | `fframes-studio-protocol/src/lib.rs:281` | `pub struct ElementMetadata` | VERIFIED |
| 1 | `fframes-studio-protocol/src/preview.rs:21` | `pub struct PreviewIdentity` | VERIFIED |
| 1 | `fframes-studio-protocol/src/preview.rs:110` | `pub struct ScaledFrameResponse` | VERIFIED |
| 1 | `fframes-studio-runtime/src/preview_worker.rs:219` | `instance_id: format!` | VERIFIED |
| 1 | `desktop/crates/studio-sdk/src/manifest.rs:89` | `preview_capabilities` | VERIFIED |
| 1 | `desktop/crates/studio-project/templates/src/lib.rs:1` | `Styles` | VERIFIED |
| 2 | `desktop/app/src/selection_spike.rs:111` | `map_viewport_to_canvas` | VERIFIED |
| 2 | `desktop/app/src/preview_element.rs:10` | `pub struct PreviewElement` | VERIFIED |
| 2 | `desktop/app/src/studio_shell.rs:2534` | `gpui::img` | VERIFIED |
| 2 | `desktop/crates/studio-engine/src/preview_state.rs:69` | `pub struct PreviewFrame` | VERIFIED |
| 2 | `desktop/app/src/preview_coordinator.rs:242` | `pub struct PreviewCoordinator` | VERIFIED |
| 2 | `desktop/app/src/preview_worker_client.rs:6` | `pub struct PreviewWorkerClient` | VERIFIED |
| 2 | `desktop/app/src/frame_image.rs:178` | `ImagePresentationManager` | VERIFIED |
| 2 | `desktop/app/src/timeline_view.rs:24` | `pub struct TimelineView` | VERIFIED |
| 2 | `desktop/app/tests/preview_coordination.rs:18` | `#[test]` | VERIFIED |
| 2 | `desktop/app/tests/x11_shell.rs:283` | `#[test]` | VERIFIED |
| 3 | `desktop/crates/studio-engine/src/task_scope.rs:397` | `resolve_scene_sources` | VERIFIED |
| 3 | `svgr-macro/Cargo.toml:15` | `syn =` | VERIFIED |
| 3 | `desktop/app/src/agent_tools.rs:183` | `pub enum ToolCall` | VERIFIED |
| 3 | `desktop/app/src/agent_tools.rs:94` | `pub enum BoundRevision` | VERIFIED |
| 3 | `desktop/app/src/agent_tools.rs:38` | `MAX_TEXT_REPLY_BYTES` | VERIFIED |
| 3 | `desktop/app/src/agent_tools/backend.rs:87` | `pub struct WriterGate` | VERIFIED |
| 3 | `desktop/app/src/agent_tools/backend.rs:484` | `pub struct ProjectToolBackend` | VERIFIED |
| 3 | `desktop/app/src/agent_tools/mcp.rs:18` | `tools/list` | VERIFIED |
| 3 | `desktop/app/src/agent_tools/client.rs:742` | `pub fn run_cli` | VERIFIED |
| 3 | `desktop/app/src/agent_workflow/tools.rs:48` | `ToolRuntime` | VERIFIED |
| 4 | `desktop/crates/studio-engine/src/task_scope.rs:93` | `pub struct TaskScope` | VERIFIED |
| 4 | `desktop/crates/studio-engine/src/controller.rs:731` | `begin_agent_task_scoped` | VERIFIED |
| 4 | `desktop/crates/studio-engine/src/agent_task.rs:87` | `pub struct AgentTaskContext` | VERIFIED |
| 4 | `desktop/crates/studio-engine/src/candidate_validation.rs:1020` | `broadening` | VERIFIED |
| 4 | `desktop/app/src/agent_workflow/present.rs:424` | `first_prompt` | VERIFIED |
| 4 | `desktop/app/src/agent_workflow/actor.rs:7` | `scope` | VERIFIED |
| 4 | `desktop/app/tests/scoped_editing.rs:19` | `TaskScope {` | VERIFIED |
| 4 | `desktop/app/tests/promotion_handoff.rs:264` | `#[test]` | VERIFIED |
| 4 | `desktop/scripts/validate-qualification.py:669` | `def validate_m4` | VERIFIED |
| 4 | `desktop/qualification/m4-results.json:77` | `auth_provider_scoped_edit` | VERIFIED |

## Review and consistency

Three independent read-only lenses completed: Security Adversary, Failure Mode Analyst and Assumption Destroyer, each checking factual claims/contracts against source. **Zero high/critical findings; zero accepted/rejected behavioral changes; no product decision was reversed.** The only reported concern was the then-unwritten baseline report; this report now exists and its constructor counts were checked against source. No approval question was needed because there are no findings to apply.

Evidence examined includes `agent_tools/backend.rs:612` task-base versus writer-gated draft access, `studio_shell.rs:2200`/`:2285` promotion and normal image installation, `scenes.rs:69` repeated scene references, `preview_worker.rs:219` revision-local scene IDs, `renderer/preview.rs:80` output fit, `agent_workflow/actor.rs:1221` queued scope checks and `candidate_validation.rs:1185` incomplete requested-frame coverage. The plan addresses these owners rather than inventing already-implemented M5 behavior.

Whole-plan consistency sweep: all five plan files reread, links/relative owning files checked, proposed new files distinguished from existing modification paths, Stage 1 representation proof matched to Stages 2–4 consumers, base/draft/candidate revision semantics checked throughout, and M0–M4 qualification limits reconciled. No unresolved contradictions. No implementation tests/builds were run.

## Tooling limits and task tracking

`ak plan create` succeeded in creating canonical files, but reported that its global plan-store index could not be written (`unable to open database file (14)`). The error's suggested global-home write requires sandbox permission; no escalation is needed to author/review canonical project files. No skill code was changed. The CLI timestamp was UTC; the scaffold directory was renamed to the session's configured `261006-2010` local naming convention before phase creation.

`ak plan add-phase` succeeded for Stages 2–4. All five generated files were read before replacement. No live task-management surface is connected; phase checkboxes are durable execution tracking. `ak plan validate` passed the authored four-stage structure. Final checks passed: ak plan validate, links across all seven plan/report files, five frontmatter blocks, modification owners including prior-stage-created files, and git diff --check. The first ownership sweep treated canvas_view.rs as missing; the dependency-aware sweep confirmed it is created in Stage 2 before Stage 4 modifies it. The journal was created successfully through the available CLI because no journal skill is installed.

## Handoff

Execute [the plan](../261006-2010-desktop-phase-five-canvas-selection-source-retrieval/plan.md) with ak-cook. Begin with Stage 1 regression and empirical ID-conversion proof; keep authentic/platform gates open until measured.

Unresolved questions: none requiring product input. The annotation representation and parser span accuracy are explicit engineering proof gates, and performance ceilings remain proposed until measured.
