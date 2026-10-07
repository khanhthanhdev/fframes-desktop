---
title: "Desktop M5: Canvas selection and reliable source retrieval"
description: "Select revision-matched canvas objects and ground agent edits in explicit anchors, Rust syntax, helpers, assets and style bindings."
status: in-progress
priority: P2
effort: "19–27 engineer-days (tentative)"
branch: main
tags: [feature, desktop, selection, source-retrieval]
blockedBy: []
blocks: []
created: 2026-10-06
---

# Desktop M5 canvas selection and source retrieval

## Outcome and boundaries

Click a rendered title, see its object/scene scope, retrieve its implementation, prompt a change, and receive validated before/after evidence through the existing Apply/Undo/recovery workflow. These four sequential stages subdivide **M5**, not milestones M5–M8. The implementation and Linux development qualification are complete; authenticated-provider, physical-device and cross-platform qualification remain separate.

Authority: [M5 roadmap](../../docs/desktop/implementation-plan.md#m5--canvas-selection-and-reliable-source-retrieval), [selection architecture](../../docs/desktop/architecture.md#6-selection-and-source-retrieval), and [baseline and validation record](../reports/planning-261006-2010-m5-baseline-and-validation.md). Keep ordinary Rust authoritative, both Cargo workspaces, managed SDK compatibility, upstream GPUI, immutable CPU preview/audio, shared compilation, task capabilities and conflict-safe source transactions.

| Foundation | Evidence to reuse | Limits to preserve |
|---|---|---|
| M0 | [Feasibility](../../docs/desktop/phase-zero-feasibility.md), legacy explicit-anchor selection | Static fixture bounds do not prove production frame metadata |
| M1 | [Project/recovery plan](../261002-0434-desktop-phase-one-foundation/plan.md), source inventory/checkpoint APIs | Saved bytes differ from validated task acceptance |
| M2 | [Preview plan](../261002-1457-desktop-phase-two-preview-timeline-audio/plan.md), exact preview/seek identities | Physical output and non-Linux qualification remain open |
| M3 | [Transactions plan](../261004-0730-desktop-phase-three-agent-transactions/plan.md), current implementation and [ledger](../../desktop/qualification/m3-results.json) | Older plan checkboxes lag code; authenticated full workflow remains open |
| M4 | [Presets/scopes plan](../261005-0715-desktop-phase-four-presets-scoped-editing/plan.md), [ledger](../../desktop/qualification/m4-results.json) | Development gates pass; provider and Windows/macOS qualification remain open |

Include stable scene/component/object identities, optional explicit anchors, revision-specific editor index, frame-specific transformed geometry/order, letterbox/zoom selection and overlap/group cycling, deterministic Rust syntax retrieval, the three new read-only tools, frozen element/rectangle task packets and M5 qualification. Exclude automatic span capture until independently proven, vector retrieval, arbitrary pixel editing, new providers/handoff, export/installers and unrelated prior-phase cleanup.

## Phases

| # | Phase | Status | Dependency | Effort |
|---|---|---|---|---|
| 1 | [Identity and SVG conversion proof](phase-01-start.md) | Complete | M0–M4 development baseline | 5–7d |
| 2 | [Displayed-frame canvas selection](phase-02-displayed-frame-canvas-selection.md) | Complete | 1 | 4–6d |
| 3 | [Deterministic retrieval and tools](phase-03-deterministic-source-retrieval-and-tools.md) | Complete | 1, 2 | 5–7d |
| 4 | [Element editing and qualification](phase-04-element-editing-and-m5-qualification.md) | Complete | 1–3 | 5–7d |

One executor owns stages sequentially; shared protocol/runtime/engine/shell/template files transfer at each gate. Do not parallelize edits to these owners.

## Acceptance criteria

- [x] Semantic scene/object keys survive content/style edits and distinguish repeated scenes/elements; converted geometry agrees with real rendered frames, including transforms and crossfades.
- [x] Selection uses the exact displayed image/identity/frame/seek and painted transform; late replies, reopen, deleted IDs and malformed metadata cannot select another object.
- [x] Explicit anchors resolve hash-verified UTF-8 spans and containing Rust implementations; bounded retrieval exposes helper, asset and token evidence with ambiguity/truncation labels.
- [x] GUI, CLI and MCP expose equivalent selection_context, source_lookup and style_context through existing task/revision authorization and budgets.
- [x] Click/title/prompt/validate/Apply/Undo/reopen succeeds; unsupported content offers a visibly scoped scene/rectangle task, and shared changes broaden validation.
- [x] Real managed-SDK, native Linux and resource development gates pass; unavailable authentic-provider, physical-device or platform gates remain separately pending.

## Verification and handoff

Focused stage checks, both Cargo workspace test suites, formatter/clippy checks, the real managed-SDK workflow, the native X11 selection test and resource/cleanup measurements were executed. Hash-linked evidence and prerequisites are recorded in [the M5 qualification ledger](../../desktop/qualification/m5-results.json). The ledger reports development acceptance as `pass` and full acceptance as `not_run`; authentic-provider, physical Linux input, Windows and macOS gates remain unqualified. The plan status remains in progress until those delivery/qualification boundaries are resolved.

## Validation Log

2026-10-06: Hard planning route selected because rendering identity, worker protocol, retrieval and native task contracts cross module boundaries. Scope held in full; no --yagni. Product choices are already constrained by the roadmap; there are no new product questions requiring a reply. Engineering representation remains subject to Stage 1 empirical proof. Source verification: 40 sampled claims verified; zero unresolved factual failures. Three independent review lenses found no high/critical issues. All five plan files passed the consistency sweep; the baseline report records evidence and tooling limits.

2026-10-06: Implemented all four stages. The managed-SDK starter-title render/source lookup, selected-title scripted prompt/Apply/Undo/reopen lifecycle, and native X11 semantic selection/zoom/pan/cycling/rectangle workflow passed. Added bounded source-index and hit-test measurements plus index-cache, cancellation, and teardown checks; qualification is hash-linked in the ledger. Both workspace test suites, formatting, desktop all-target Clippy, root library/bin/test Clippy, qualification negative tests, and all ledgers passed. Full/authentic qualification remains pending exactly the provider, physical-input and Windows/macOS prerequisites stated above.

2026-10-06: Remediated review findings across scope validation/labels, UTF-8 source spans/snippet windows/helper attribution, optional worker capability negotiation, duplicate scene identities, metadata bounds, task-tool schemas/errors, canvas status/modifier/redraw behavior, and qualification contracts. Added a runtime-string SVG fragment regression, restored the full repository `AGENTS.md` guidance, and refreshed the resource measurement plus hash-bound proof. Focused protocol/runtime/core/project/engine/app tests and both Rust format checks pass; all authentic-provider, physical-input and cross-platform gates remain not_run.

<!-- slug: desktop-phase-five-canvas-selection-source-retrieval -->
