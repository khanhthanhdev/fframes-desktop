---
title: "Desktop M3: Recoverable agent edit transactions"
description: "Connect a configurable ACP v1 agent to a stable draft, validate immutable changes, and apply undoable task revisions with native recovery."
status: in-progress
priority: P2
effort: "22–31 engineer-days (tentative)"
branch: main
tags: [feature, desktop, agent, acp, recovery]
blockedBy: []
blocks: []
created: 2026-10-04
---

# Desktop M3 agent transactions

## Outcome and boundaries

Enter a brief, watch one authenticated agent produce a playable validated video, request a second edit, Undo it, and reopen with the accepted task revision intact. Compilation/inspection failure gets structured context and at most one automatic repair attempt; Stop or provider failure retains the draft without leaving a writer. These four stages subdivide **M3**, not milestones M3–M6.

Reuse [M3 roadmap](../../docs/desktop/implementation-plan.md), [transaction architecture](../../docs/desktop/architecture.md#5-the-edit-transaction), [M0 feasibility](../../docs/desktop/phase-zero-feasibility.md), [M1](../261002-0434-desktop-phase-one-foundation/plan.md) and [M2](../261002-1457-desktop-phase-two-preview-timeline-audio/plan.md). Existing Linux X11 development evidence is accepted; physical GPU/display/IME/audio, Windows/macOS and authenticated-provider qualification remain open. M3 planning does not close those gates.

Keep both Cargo workspaces, managed SDK, CPU preview, upstream GPUI pin, GPUI-free engine and legacy spike/worker routes. Implement the official Rust ACP SDK with explicit v1 negotiation inside the existing agent package. Continue configurable adapter executables; the first qualified provider is an execution gate. Default automatic Apply after required validation and optional manual review are inherited architecture. One repair attempt is a provisional engineering default, not a newly confirmed user choice.

Included: discovery/auth feedback; bounded virtualized conversation, tool cards, permissions/questions/options/Stop; stable app-owned draft; immutable candidate validation; shared project-context/timeline/frame/strip/inspect/build-status CLI and supported MCP facade; recoverable multi-file Apply, safe Undo and task-journal recovery. Excluded: M4 presets/scoped task packets, M5 object retrieval/selection, M6 multiple-provider handoff and M7 export.

## Sequence and ownership

| # | Stage | Depends on | Effort | Status |
|---|---|---|---|---|
| 1 | [ACP driver, task and draft contracts](phase-01-start.md) | M0–M2 implementation and regressions | 5–7d | Pending |
| 2 | [Candidate validation and shared project tools](phase-02-candidate-validation-and-project-tools.md) | 1 | 6–8d | Pending |
| 3 | [Promotion, Undo and recovery](phase-03-promotion-undo-and-recovery.md) | 1, 2 | 6–9d | Pending |
| 4 | [Native workflow and authentic qualification](phase-04-native-agent-workflow-and-qualification.md) | 1, 2, 3 | 5–7d | Pending |

One executor owns each stage; shared files transfer after the preceding gate. Do not run stages concurrently. Phase files enumerate existing modifications, proposed creates, exact package checks, failure responses and rollback. Every execution checkbox remains unchecked.

## Data and authority

Current source plus prior saved history and brief enter a task. Capture the source base independently of the old checkpoint. One owned writer edits a stable draft. Authoritative completion followed by verified writer quiescence permits immutable capture. Shared compilation and inspection create a revision-bound validation report and staged matching video/audio. Journaled, conflict-checked publication creates a validated task history entry; the M2 handoff installs matching playback. Undo applies an inverse file set with the same conflict and recovery rules.

M1 saved checkpoints prove bytes only. M3 validated acceptance separately names task, source base, prior history, candidate revision, SDK/build key, validation evidence and published revision. Do not force promotion through M1 checkpoint completion or relabel a candidate as its accepted base. Source scans and watcher hints do not isolate mutations.

## Acceptance

- [ ] ACP v1 negotiation, authenticated completion, permission/question/options handling, Stop and process cleanup are proven with the selected adapter; fixtures cannot qualify it.
- [ ] A stable draft has one writer; every build uses quiesced immutable bytes and bounded revision-bound validation, with one visible repair attempt maximum.
- [ ] GUI, CLI and supported MCP calls share compilation for equal source/SDK/target/backend keys; cancelling one subscriber cannot kill another subscriber's work.
- [ ] Default Apply and optional review create recoverable task revisions; external edits, dirty imports, Git history and executable permissions survive Apply/Undo/restart conflicts.
- [ ] Failed/cancelled work preserves old playback; successful promotion installs matching video/audio with existing position/selection and epoch checks.
- [ ] Authentic first edit, second edit, Undo and restart pass, alongside repair, cancellation, crash-boundary and resource tests; missing credentials leave qualification pending.

## Evidence and open gates

[Protocol research](../reports/research-261004-0730-desktop-phase-three-acp-contract.md) and [contract scout](../reports/scout-261004-0730-desktop-phase-three-agent-transactions.md) distinguish implemented foundations from qualification. Exact ACP/MCP pins, Linux no-clobber publication primitives and authenticated provider access are execution gates. No blocking product question; re-estimate effort if those gates expose incompatible behavior.

## Review and validation

Three independent lenses reviewed the draft: [security](../reports/review-security-261004-0730-desktop-phase-three.md), [assumptions](../reports/review-assumptions-261004-0730-desktop-phase-three.md) and [failure modes](../reports/review-failures-261004-0730-desktop-phase-three.md). Two high assumption findings were addressed: qualify the adapter's process-group writer boundary and terminal cleanup without claiming escaped-writer detection (Stages 1/4); validate prepared PCM/timeline locally for worker-only imports (Stage 2). No user-selected scope or policy was reversed.

The [validation record](../reports/validation-261004-0730-desktop-phase-three-agent-transactions.md) records optional questions, source/consumer checks, CLI limitations and the final consistency sweep. No reply to the two optional questions was received; configurable ACP and one repair remain stated planning defaults. Source execution, authentic qualification and all checkboxes remain pending. Plan files track tasks because no live work-item surface is available.

Next execution: `/ak:cook /root/fframes-desktop/plans/261004-0730-desktop-phase-three-agent-transactions/plan.md`. Begin with Stage 1 baseline regression checks; this planning task does not authorize implementation.

## Windows development run (2026-10-10)

On 2026-10-10 a Windows Server 2022 x64 VM (virtual RDP display, no GPU) ran the Windows development checks: the full desktop workspace suite (79 test targets) passes on MSVC, the Windows SDK assembles with its offline double build, and the packaged app installs the managed SDK into a fresh home, compiles a worker, presents 1,000 frames and accepts native SendInput typing and preview selection with no leftover processes. Results and evidence are in the [M0 ledger](../../desktop/qualification/m0-results.json) under `additional_platforms`. `auth_windows` in the [M3 ledger](../../desktop/qualification/m3-results.json) stays `not_run`: no authenticated ACP adapter was available on the Windows host.

<!-- slug: desktop-phase-three-agent-transactions -->
