# M6 planning baseline

Date: 2026-10-07. Workspace: `/root/fframes-desktop`. Language: English. Timezone: Asia/Saigon.

## Outcome and scope

Plan the full M6 provider qualification and controlled handoff milestone in the desktop roadmap. Hold scope; no scope-cutting flag was passed. Reuse the implemented M0–M5 development foundation, retain ordinary Rust projects and the current upstream GPUI stack, and keep installer/native export delivery in M7. No implementation, adapter installation or authenticated provider run is performed by this planning task.

## Verified baseline

- [M5 plan](../261006-2010-desktop-phase-five-canvas-selection-source-retrieval/plan.md) and [M5 ledger](../../desktop/qualification/m5-results.json) show Linux development acceptance passing, with authenticated-provider, physical-device and Windows/macOS evidence still unrun. The older M3 roadmap narrative is not current implementation authority.
- [Agent workflow](../../desktop/app/src/agent_workflow.rs) already owns task transactions, bounded transcript/history, tool capability lifetime, candidate validation, Apply/Undo and interrupted recovery. Extend that owner rather than introducing a second orchestrator.
- [Workflow actor](../../desktop/app/src/agent_workflow/actor.rs) currently sets `resume_session: None` when launching tasks. [ACP runtime](../../desktop/crates/studio-agent-spike/src/driver/runtime.rs) already selects negotiated resume/load and reports unsupported restoration explicitly. Persistence and safe context refresh belong at the workflow/controller boundary.
- [Discovery](../../desktop/crates/studio-agent-spike/src/discovery.rs) resolves explicit executable/runtime paths, forwards named auth variables and distinguishes initialized, auth-unknown, auth-required and ready states. Readiness is not end-to-end qualification.
- [Architecture](../../docs/desktop/architecture.md#session-and-writer-ownership) requires verified stopped writers, stable draft cwd, preserved accepted/draft variants, explicit handoff context and no claim of portable provider-native memory.
- [Qualification validator](../../desktop/scripts/validate-qualification.py) already enforces evidence hashes, authentic versus development records, redaction and per-gate contracts for M0–M5. M6 needs an additive contract and negative tests, not a second permissive evidence path.

## Tooling record

The live `ak plan create` and `ak plan add-phase` commands successfully scaffolded the plan and three execution stages. The CLI initially used UTC for the directory timestamp; the directory was moved to the injected session naming path `261007-0712-desktop-phase-six-provider-qualification-handoff` before further phase operations. The create command reported that its optional global plan-store index database could not be written outside the sandbox. Repository files remain canonical; no skill code, global database or configuration was changed. No live task-management tool is exposed in this session, so phase checkboxes provide durable task tracking.

The live CLI parser discovers phase files independently of the static scaffold table; live help exposes no table regeneration operation. The plan table was filled with the three scaffolded phases and their dependencies. This does not modify a CLI-owned index.

## Validation and review results

Hard planning mode was selected for provider/runtime, transaction, persistence and UI contracts. Scope remains the complete roadmap milestone. No new product decision required an interview; runtime unknowns are explicit qualification probes, not guessed compatibility. No code or authentic adapter tests were run during planning.

- [Provider research](research-261007-0712-desktop-phase-six-providers.md) records current primary sources, candidate distribution versions and unqualified behavior. Recheck and pin the actual dependency closure during execution.
- [Security review](review-security-261007-0712-desktop-phase-six.md) found zero Critical/High/Medium issues. The existing exclusion of credential-bearing wire session IDs is explicitly preserved.
- [Assumption review](review-assumptions-261007-0712-desktop-phase-six.md) verified 14 Stage 1 and 18 Stage 2 claims, including consumer contracts. No unresolved findings remain.
- [Failure review](review-failures-261007-0712-desktop-phase-six.md) verified 10 Stage 3 claims and identified three Medium execution ambiguities. Accepted all three within the already requested behavior: retain queue before switch-specific stop, await publication/Undo settlement before variant choice, and reuse the existing tool snapshot gate during capture/materialization. Added explicit regression scenarios to Stage 2 and the UI wait/queue state to Stage 3. No Critical/High finding was established.

Source claim tier: Standard (Fact Checker and Contract Verifier). Claims checked: 42; verified: 42; failed: 0; unresolved: 0. Reviews cite the owning source and caller paths. Distribution declarations remain research observations, not authentic compatibility claims.

### Whole-plan consistency sweep

Reread `plan.md` and all three execution stages after review edits. Checked four engineering deltas (queue retention, publication settlement, tool snapshot gate, safe opaque-ID persistence) across steps, success criteria, tests and UI. There are no unresolved contradictions. Dependency order is sequential and acyclic; shared owners transfer after the prior gate. Later-stage modifications to the new M6 validator test and ledger correctly depend on Stage 1 creation. An initial existence-only check flagged those two future files; the corrected ownership-aware check confirms their explicit dependency and passes.

`ak plan validate` passes; `ak plan parse` reports three pending stages and twenty unchecked execution tasks. Local Markdown links resolve and all existing modification targets exist; new targets are labelled as proposed. The root roadmap adds only the M6 execution link and qualification boundary. No live runtime task tool is exposed, so durable plan checkboxes retain task state. A journal entry was created through the live `ak journal create` CLI under `plans/journals/`.

Next execution entry: [M6 plan](../261007-0712-desktop-phase-six-provider-qualification-handoff/plan.md). The optional global plan-store index warning does not block file-based execution. Authentication, platform/device access and provider-specific capabilities remain explicit execution prerequisites; no connector is newly qualified by planning.
