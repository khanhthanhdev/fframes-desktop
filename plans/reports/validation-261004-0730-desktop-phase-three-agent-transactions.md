# M3 planning validation

Date: 2026-10-04. Scope: planning and documentation only.

## Authority and inherited decisions

The [desktop roadmap](../../docs/desktop/implementation-plan.md) defines Phase 3 as milestone M3, not the third execution stage within M2. The [M0 feasibility record](../../docs/desktop/phase-zero-feasibility.md), [M1 plan](../261002-0434-desktop-phase-one-foundation/plan.md), [M2 plan](../261002-1457-desktop-phase-two-preview-timeline-audio/plan.md) and current source establish the implementation baseline. Actual Git status was clean at intake despite the earlier session's list of modified files. Historical claims about uncommitted implementation were not used to infer current Git state.

Keep ordinary Rust projects, the two Cargo workspaces, upstream GPUI, managed SDK and CPU preview. Default automatic Apply after validation, optional manual review, one active writer per project, retained drafts and conflict-safe Undo come from the architecture. Configurable ACP adapters and Linux development are already accepted; authenticated provider and physical/platform qualification remain open in the M0/M2 ledgers.

## Optional validation questions

Two questions were presented during authoring:

1. “For M3, should I keep the existing configurable ACP adapter approach and plan development while authenticated qualification remains open?” Options: keep configurable ACP and require a real authenticated run before claiming M3 complete; or name a first provider now and gate execution on its qualification.
2. “How many automatic repair attempts should M3 allow after a compiler or inspection failure?” Options: one, or two, before retaining the draft with diagnostics.

No response has been received as of drafting this record. The plan uses the recommended configurable approach and one repair attempt as provisional planning assumptions, not newly confirmed user decisions. Neither waives the authenticated completion gate.

## Source and contract checks

Standard verification applies to the four stages: Fact Checker and Contract Verifier. The [contract scout](scout-261004-0730-desktop-phase-three-agent-transactions.md) identifies concrete reusable symbols and their owners. The [protocol research](research-261004-0730-desktop-phase-three-acp-contract.md) records current primary sources and the explicit ACP v1 compatibility boundary.

Verified starting boundaries include ACP `run_prompt`/permission/cancel/clarification subprocess tests; `ProcessTreeManager` scopes and child environments; `OperationTag` and checkpoint-only `JobKind`; controller ownership and reconciliation; immutable checkpoint capture/load; metadata journal intent/commit; isolated SDK materialization; compiler target lock and isolated executable; `PreviewIdentity` derived from base source; bounded coordinator staging; `ReadyPreview` identity validation; and app-owned audio handoff.

The plan must add behavior, not assume it already exists: normalized production ACP events, persistent task drafts, structured task history, build sharing above the target lock, task-bound tools, candidate-versus-base identity, multi-file Apply/Undo recovery, production conversation UI and authenticated qualification. New files and test commands are proposed execution deliverables. Existing M0 exports/fixtures, M1 serialized record readers and M2 preview consumers require regression coverage when shared contracts change.

## Tracking and tooling

The plan CLI scaffolded the files. Its global index update reported an unwritable database; no global permissions or index workaround was introduced. The directory was renamed to the injected local-time naming contract. There is no live work-item management surface, so unchecked plan tasks are durable tracking authority. The worktree pointer was not written because `.git` is read-only under this session's permissions.

No build, Rust tests, GUI run, account authentication or platform qualification is claimed by this planning task. Structural/link checks and independent review are recorded after final authoring below.

## Independent review and resolution

Security and failure-mode reviewers found no uncovered high/critical issue. The assumption reviewer identified two high contract gaps, addressed within the authorized planning scope without reversing user decisions:

- Stage 1 now qualifies the selected adapter's process-group-contained writer model, assigns terminal cleanup verification to bootstrap and explicitly declines general escaped-helper detection. Unknown/detached writers block Apply qualification and draft reuse. Stage 4 records this evidence in the authentic gate.
- Stage 2 checks prepared immutable PCM and compiled timeline locally, with explicit geometry/finite-sample/duration/placement and clipping/silence diagnostics. CLI comparison is conditional. A custom-main, worker-only import with an audio-only edit is required coverage.

The [assumption review](review-assumptions-261004-0730-desktop-phase-three.md) records source evidence. These amendments fix execution contracts; exact SDK/provider pins and publication primitives remain future gates rather than factual failures in the plan.

## Whole-plan consistency sweep

All five plan files were reread. Changes were propagated through stage requirements, ownership, steps, risk responses and authentic qualification. The index preserves current-source base versus saved checkpoint versus validated candidate, one provisional repair, conditional MCP and pending authenticated/platform gates. No source publication or qualified-provider claim is made.

`ak plan validate` passes, and `ak plan parse` reports four pending stages with no completed work. Local Markdown links and all Modify paths were checked; a Modify path created in an earlier stage is treated as a valid sequential dependency. The index stays below 80 lines, every execution checkbox is unchecked, and `git diff --check` passes. There are no unresolved cross-file contradictions. Implementation begins with the Stage 1 baseline tests; this planning delivery does not execute the plan.
