# M3 plan review: Security Adversary and Contract Verifier

Reviewed all five completed plan files, the desktop architecture/roadmap, and the cited ACP, process ownership, source inventory, checkpoint, journal, acceptance and preview-install implementations. Read-only inspection only; no build, test or lint commands ran.

## Findings

No evidence-backed high or critical plan findings.

## Evidence supporting that result

- Stage 1 explicitly separates authenticated readiness from initialization, redacts credentials before persistence, declines unimplemented client services, and requires owned-writer quiescence before capture (`phase-01-start.md:24–30`). This correctly extends rather than assumes the handwritten v1 negotiation/completion contract (`desktop/crates/studio-agent-spike/src/session.rs:407–474`) and terminal process scopes (`desktop/crates/studio-bootstrap/src/process.rs:581–629`).
- Stage 2 binds the six-operation broker to task/project/revision authority, rejects traversal and stale tasks, keeps capabilities outside portable source and argument logs, and shares immutable compilation without giving subscribers installation authority (`phase-02-candidate-validation-and-project-tools.md:28–32`). The narrow tool surface does not claim to sandbox provider-owned coding tools or Cargo scripts.
- Stage 3 makes safe publication a prerequisite for automatic Apply, rather than treating a source scan or application lock as exclusion. Displaced originals, no-clobber publication, identity/mode checks, preserved unknown variants and conditional restart rollback cover the concrete editor race and crash scenarios (`phase-03-promotion-undo-and-recovery.md:24–39`). Linux primitive proof remains an execution gate; other platforms are not claimed qualified.
- Saved checkpoint completion currently accepts only the tagged source base (`desktop/crates/studio-engine/src/state.rs:266–292`), and M2 installation checks source/session/generation and seek identity (`desktop/crates/studio-engine/src/preview_state.rs:324–349`). The plan explicitly creates distinct validated task acceptance and candidate-to-published authorization, avoiding incompatible reuse of those existing guards.
- Existing journal Intent/Commit entries are metadata-only (`desktop/crates/studio-engine/src/journal.rs:21–24`), while synced append and failed-write suspension already exist (`journal.rs:122–155`). Stage 3 recognizes that gap and requires versioned durable file-set progress, compatible history reads and projection replay; it does not attribute multi-file recovery to the existing implementation.

## Limits

This is a plan review, not a security or provider qualification claim. Exact SDK/MCP pins, authenticated adapter behavior, detached-writer handling and publication primitives must still meet the plan's implementation gates. No unresolved product question or scope reduction is recommended.

Status: DONE
Summary: No concrete high/critical security or existing-contract blocker found in the completed M3 draft.
Concerns/Blockers: None at plan-review scope.
