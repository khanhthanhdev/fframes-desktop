# Planning validation

**Plan:** Desktop M4 — presets and scoped editing\
**Date:** 2026-10-05\
**Scope:** Planning only. No application or Rust implementation was changed.

## Source checks

- Compared Phase 04 in `docs/desktop/implementation-plan.md` with `docs/desktop/implementation-plan-M0.md`, `implementation-plan-M1.md`, `implementation-plan-M2.md`, and `implementation-plan-M3.md`.
- Confirmed the current manifest remains `desktop/crates/studio-project/schemas/studio.schema.json`; its optional `preset` field already has `id` and `sha256`, so the plan does not introduce an unnecessary manifest version bump.
- Confirmed `TimelineSelection` lives in `desktop/crates/studio-engine/src/timeline.rs`, is held by `desktop/app/src/timeline_view.rs`, and is not submitted through `AgentWorkflow::submit`. The new scoped-task contract therefore requires explicit immutable submitted scope.
- Confirmed M3 `AgentTaskContext` in `desktop/crates/studio-engine/src/agent_task.rs` contains whole-project source/assets/instructions and has no scope field. The existing six read-only tools (`project_context`, `timeline`, `render_frame`, `render_strip`, `inspect`, `build_status`) are declared in `desktop/app/src/agent_tools.rs`.
- Confirmed candidate validation includes broader inspection paths in `desktop/crates/studio-engine/src/candidate_validation.rs`; the plan retains inspection for shared Rust/Cargo/style/config changes.
- Confirmed the current runtime publication path is in `desktop/crates/studio-engine/src/edit_transaction.rs` and `journal.rs`; preset source mutation is planned as a separate durable project mutation, not as task acceptance.
- Read `desktop/qualification/m3-results.json` and `desktop/qualification/m3-results.md`: development checks and the authentic adapter probe are recorded as passing; authenticated edit/recovery, broader repair, writer containment, physical audio, UI/IME, and other-platform checks remain `not_run`. M4 preserves this distinction and does not claim those gates.

## Plan structure and validation

- The plan is split into four ordered implementation phases with explicit scope, file targets, dependencies, acceptance criteria, and checks.
- Checked 35 currently-existing `Modify` paths from the phase file tables. Thirty-three exist now; the two not yet present are `desktop/crates/studio-presets/src/lib.rs` and `desktop/crates/studio-presets/Cargo.toml`, which Phase 1 creates before Phase 2 modifies them.
- `ak plan reindex --apply --json` indexed the plan with four phases.
- `ak plan validate plans/261005-0715-desktop-phase-four-presets-scoped-editing` passed.
- `ak plan parse plans/261005-0715-desktop-phase-four-presets-scoped-editing --json` reported four pending phases and 44 unchecked tasks, as expected for an unstarted plan.
- Relative Markdown links and named existing repository paths were checked; `git diff --check` passed.
- No Rust, desktop integration, or UI tests were run because this task produces an implementation plan only. Relevant implementation and qualification tests are specified in the phase plans.

## Important qualification constraint

Preset schema validation and preview rendering may be developed portably, but applying preset changes to a project must remain disabled on any OS/filesystem combination whose no-clobber publication and crash-recovery gates are not qualified. The M4 development target is native Linux x64. macOS and Windows results stay `not_run` until exercised.
