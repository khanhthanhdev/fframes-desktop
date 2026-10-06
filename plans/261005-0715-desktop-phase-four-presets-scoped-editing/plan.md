---
title: "Desktop M4: Presets and scene/range-scoped editing"
description: "Add portable versioned style presets and freeze compiled scene/range context into agent edits without expanding into M5 element selection."
status: in-progress
priority: P2
effort: "14-20 engineer-days (tentative)"
branch: main
tags: [feature, desktop, presets, agent-editing]
blockedBy: []
blocks: []
created: 2026-10-05
---

# Desktop M4: Presets and scene/range-scoped editing

## Overview

Implement the M4 requirements in the [desktop roadmap](../../docs/desktop/implementation-plan.md#L105) on top of the M0 managed project/worker boundary, M1 portable project and recovery foundation, M2 revision-safe preview/timeline/audio, and M3 agent transaction workflow. The four execution stages below subdivide M4; they do not replace roadmap milestones M5–M7.

The current checkout has the M3 development workflow and regression surfaces. Its qualification ledger reports development gates passing and an authentic adapter probe, while authentic two-edit/Undo/restart, writer containment, repair, UI/IME, physical audio and other-platform gates remain `not_run`. M4 may use those development contracts but must not convert them into broader provider or platform claims. M2 physical audio/display qualification remains separate.

See [planning validation and source checks](./reports/planning-validation.md) for concrete repository evidence and the planning-only verification performed.

## Dependencies and boundaries

| Relationship | Plan | Status |
|---|---|---|
| Builds on | [M0 feasibility](../261001-1707-desktop-phase-zero-setup/plan.md) | Implemented foundation; platform/provider gates tracked separately |
| Builds on | [M1 project and recovery](../261002-0434-desktop-phase-one-foundation/plan.md) | Implemented locally |
| Builds on | [M2 preview, timeline and audio](../261002-1457-desktop-phase-two-preview-timeline-audio/plan.md) | Implemented locally; physical/platform qualification remains open |
| Builds on | [M3 agent transactions](../261004-0730-desktop-phase-three-agent-transactions/plan.md) | Development workflow exists; authentic workflow qualification remains open |

In scope are portable preset schema/import/export, canonical typed tokens and alias resolution, project-local snapshots/overrides, a typed `Styles` runtime accessor, generated-project guidance, and whole-project/scene/half-open-range task context with before/after evidence. Preserve ordinary Rust as the source of scene timing and design structure. Presets affect video output, never Studio chrome.

Out of scope are M5 element IDs/hit testing, automatic source spans, a general Rust syntax index, vector retrieval, arbitrary CSS cascade/layout, new ACP provider drivers, export UI, simultaneous project writers, and release/platform qualification. Existing six task tools remain the only M4 tool surface; this milestone does not add M5 `selection_context`, `source_lookup` or `style_context` methods.

## Phases

| # | Phase | Status | Depends on | Tentative effort |
|---|---|---|---|---|
| 1 | [Preset schema and typed resolution](./phase-01-start.md) | Completed | M0–M3 baseline regressions | 3–4 days |
| 2 | [Preset validation and CSS import](./phase-02-preset-validation-and-css-import.md) | Completed | 1 | 3–4 days |
| 3 | [Runtime Styles and project snapshots](./phase-03-runtime-styles-and-project-snapshots.md) | Completed | 1, 2 | 4–6 days |
| 4 | [Scoped editing and M4 qualification](./phase-04-scoped-editing-and-m4-qualification.md) | Development gates complete; authentic qualification pending | 1–3; M2 timeline and M3 task workflow | 4–6 days |

One executor owns the stages sequentially. Shared schema, project mutation, controller, task-context, shell and template files transfer only after the preceding gate passes.

## M4 acceptance criteria

- [x] Versioned directory presets import/export with bounded resources, valid licenses, canonical typed values, alias/type/unit validation and a useful CSS import report.
- [x] Three original example presets vary typography, layout and motion; supported fonts/assets are portable and licensed.
- [x] The same source project renders under two presets using semantic token bindings. Local overrides survive preset reapplication and project close/reopen; explicit reset is separate. Verified with the locally assembled managed SDK and without an overlay.
- [x] Preset files and identity are included in portable source snapshots. Applying/reapplying is conflict-safe and durable, invalidates stale work and leaves the old playable preview intact until a matching replacement is ready.
- [x] Generated projects expose typed `fframes::Styles` access loaded once outside `render_frame`; generated instructions explain tokens, assets, licensing and Studio validation tools. The scaffold builds/renders on the locally assembled managed SDK.
- [x] Submitting a scene/range edit freezes its source/preview identity, compiled frame scope, overlap/boundary context, current visual evidence, active style snapshot and deterministic best-effort scene source references. A stale queued scope is refreshed or refused visibly, never silently reinterpreted.
- [x] A scene/range prompt produces a validated revision with before/after evidence. Selected scope informs task context, but Rust/style/shared-file or uncertain changes broaden validation rather than claiming the change is isolated.
- [x] Desktop/core focused tests, generated-project managed-SDK renders, bounded-resource checks, native Linux interaction and qualification-ledger validation pass; unavailable authenticated-provider, Windows and macOS gates remain explicitly `not_run`.

## Verification and handoff

Use the phase-specific test matrices and run both workspace formatting/clippy gates. The current development evidence uses a locally assembled managed SDK built from this checkout, with no overlay; it is not a published SDK release. Native Xvfb exercises scene/range scopes, before/after evidence, negotiated PNG prompt blocks and stale-scope refusal with a scripted ACP peer. Do not claim authentic provider interaction from that fixture or an adapter readiness probe; the authentic provider and non-Linux platform gates remain open.

Plan tracking remains in these phase files because no live task-management surface is connected. This planning task creates no implementation changes, provider qualification, commit, publication or release claim.

## Validation Log

### Session 1 — 2026-10-05

**Trigger:** Plan creation and accuracy validation against the M4 roadmap, M0–M3 plans and current source.\
**Questions asked:** 0. The roadmap and existing architecture constrain the implementation choices; the recommendations below remain planning decisions, not user-confirmed product requirements.

#### Verification Results

- **Tier:** Standard (Fact Checker + Contract Verifier)
- **Claims checked:** 40 (10 sampled claims per phase)
- **Verified:** 40 | **Failed:** 0 | **Unverified:** 0
- **Contract check:** `AgentTaskContext` currently has two literal construction sites (`desktop/crates/studio-engine/src/controller.rs:760` and `desktop/crates/studio-engine/src/agent_task.rs:1284`); test helpers delegate to the controller. Existing `Controller::begin_agent_task` and `AgentWorkflow::submit` remain whole-project wrappers, avoiding a mass caller migration. The six current tools are listed in `desktop/app/src/agent_tools.rs`; M4 retains that surface.
- **Failures:** None.

#### Planning decisions (recommendations, not user confirmations)

- Use a portable directory as the canonical preset exchange format; the roadmap permits directory or archive, and directory import avoids new archive/extraction risk.
- Keep preset semantics in a GPUI-free crate, and place generated-project `fframes::Styles` behind an additive feature in the published `fframes` crate.
- Treat preset application as a separate, source-fenced durable project mutation, not an accepted agent task. Preserve overrides by default and require an explicit reset.
- Freeze submitted scene/range intent against the exact displayed compiled identity. Reject or visibly refresh stale queued scopes; never retarget old frame coordinates silently.
- Keep scene source lookup bounded and best-effort; defer element selection, source spans, generalized indexing and new M5 tools.
- Enable project preset mutation only where no-clobber publication and recovery are qualified. Native Linux x64 is the M4 development target; other platforms stay `not_run` until exercised.

#### Action items and phase propagation

- No user-answer action items. Recommendations and decisions are propagated through the four phase files; implementation remains pending.

### Whole-Plan Consistency Sweep

- **Files reread:** `plan.md`, `phase-01-start.md`, `phase-02-preset-validation-and-css-import.md`, `phase-03-runtime-styles-and-project-snapshots.md`, `phase-04-scoped-editing-and-m4-qualification.md`.
- **Decision deltas checked:** 6
- **Reconciled stale references:** 3 (timeline-selection ownership, task-context/tool declaration locations, nested desktop Cargo manifest path).
- **Unresolved contradictions:** 0

<!-- slug: desktop-phase-four-presets-scoped-editing -->
