---
phase: 4
title: "Element editing and M5 qualification"
status: complete
priority: P2
effort: "5–7 engineer-days"
dependencies: [1, 2, 3]
---

# Phase 4: Element editing and M5 qualification

## Goal and context

Complete title/object and fallback rectangle prompts through frozen task packets, bounded context, validation, Apply/Undo/reopen and evidence-backed qualification. Read [M5 roadmap](../../docs/desktop/implementation-plan.md#m5--canvas-selection-and-reliable-source-retrieval), [M4 scoped editing](../261005-0715-desktop-phase-four-presets-scoped-editing/phase-04-scoped-editing-and-m4-qualification.md) and [baseline](../reports/planning-261006-2010-m5-baseline-and-validation.md). Reuse the implemented workflow; no new agent driver, source writer or publication path.

## Requirements and architecture

Extend TaskScope with typed element and rectangle variants. An element packet freezes full PreviewIdentity, displayed frame/seek/index hash, semantic scene/component/object/repeat tuple, geometry/support label, half-open scene/range coverage, exact or uncertain source references, syntax context, style hashes and bounded before evidence. A rectangle freezes video-pixel coordinates plus frame/time/scene choice; it carries no fabricated source ID. Existing whole-project/scene/range constructors and default serialization remain compatible.

The canvas selection is pending UI state until composer submission. Capture/reconcile immutable source, style, assets and displayed identity before starting the writer. Reconcile queued selections again at start; stale/deleted IDs require visible reselection or an explicit fallback, never a silent remap. A base packet remains available through task-bound tools while the agent edits the draft. Retrieve exact snippets from the captured base, not live changing files.

Use existing ACP image-capability negotiation: attach bounded real frame/crop/strip evidence only when supported; otherwise provide app-owned artifact references with a visible limitation. A crop is derived from the same full frame and retains its video-coordinate rectangle; show full-frame evidence for context and do not claim crop-only edits are isolated. Keep large base64 outside stored prompt/transcript text. Do not draw a predicted result; before/after thumbnails describe actual base/candidate revisions.

Candidate validation retains requested displayed frame and selected scene/range plus adjacent boundaries, then broadens for Rust/helpers/Cargo/style/assets/unknown changes. If the candidate removes the object, reports its deletion and validates the requested change using scene/frame evidence; if it cannot cover the required frame/time range, keep the candidate incomplete for review/repair. IDs describe context, not an edit whitelist. Existing at-most-one repair, single writer, AutoApply/manual review, no-clobber Apply, safe Undo, crash replay and fresh playback promotion authorization remain authoritative.

## Files and ownership

Paths are relative to `/root/fframes-desktop`; this final sequential stage owns these files.

| Action | Files | Purpose |
|---|---|---|
| Modify | `desktop/crates/studio-engine/src/task_scope.rs`, `desktop/crates/studio-engine/src/agent_task.rs`, `desktop/crates/studio-engine/src/controller.rs`, `desktop/crates/studio-engine/src/candidate_validation.rs` | Typed object/rectangle scope, stale guards and coverage |
| Modify | `desktop/crates/studio-engine/tests/agent_task_scope.rs`, `desktop/crates/studio-engine/tests/candidate_validation.rs` | Packet and broadened coverage regressions |
| Modify | `desktop/app/src/agent_workflow.rs`, `desktop/app/src/agent_workflow/model.rs`, `desktop/app/src/agent_workflow/actor.rs`, `desktop/app/src/agent_workflow/present.rs`, `desktop/app/src/agent_workflow/tools.rs` | Freeze/reconcile queued packets and evidence through existing lifecycle |
| Modify | `desktop/app/src/studio_shell.rs`, `desktop/app/src/canvas_view.rs`, `desktop/app/src/conversation_panel.rs`, `desktop/app/src/conversation_panel/controls.rs` | Visible object/rectangle scope, clear/expand/reselect and actual evidence |
| Modify | `desktop/app/tests/scoped_editing.rs`, `desktop/app/tests/agent_workflow.rs`, `desktop/app/tests/agent_workflow_ui.rs`, `desktop/app/tests/x11_shell.rs` | Preserve older constructors and task/native behavior |
| Create | `desktop/app/tests/element_editing.rs`, `desktop/app/tests/real_sdk_selection.rs` | Full title edit and managed SDK conversion workflow |
| Create | `desktop/qualification/m5-results.json`, `desktop/qualification/m5-results.schema.json` | Machine-readable development/authentic gates and evidence hashes |
| Create | `desktop/scripts/qualify-m5-selection.py`, `desktop/scripts/test-qualification-m5.py` | Real SDK/native development runner and false-pass validator tests |
| Modify | `desktop/scripts/validate-qualification.py` | M5 gate-specific contracts and default ledger list |
| Modify after proof | `docs/desktop/architecture.md`, `docs/desktop/README.md`, `docs/desktop/implementation-plan.md` | Actual syntax/tool behavior, fallback and evidence limits |

Keep new fixtures under tests or qualification evidence; they must exercise actual rendering/source transactions and cannot substitute for provider qualification.

## Tasks and steps

- [x] Add optional object/rectangle packet fields/variants with strict identity and bounds validation, backward defaults and reexports. Update the two AgentTaskContext literal sites and the three external TaskScope literal sites listed in the baseline; re-run constructor searches before editing. Preserve wrapper APIs and old serialized scope reads.
- [x] Connect canvas pending scope to composer and workflow queue; resolve immutable anchors/index/style/evidence before writer launch. Test source changes between click, submit, queue start and capture, plus close/reopen and preview promotion. Refuse stale packets before refreshing drafts or granting a writer.
- [x] Attach bounded syntax/token/asset context and actual image evidence; give the agent the new tool route for more context. Show missing anchors, approximation, truncation, image transport gaps and unsupported geometry in the scope/result UI. Keep composer typing/IME and existing timeline keyboard controls usable.
- [x] Extend validation coverage for frozen displayed frame/scene/boundaries and broadened dependencies. Test shared helper/token changes, retiming/shortened candidates, deleted IDs, deliberate compiler failure/one repair, Stop and process failure. Preserve prior playback, retained drafts and source/Git history on failure.
- [x] Prove a generated annotated title edit with a verified managed SDK: before source/frame/index, selected tuple/source span, candidate edit/render, accepted task revision, Undo and reopen. Use real compiled metadata rather than feasibility-fixture Rects. Run native Linux UI cases for repeat/group/crossfade selection, rectangle fallback, zoom/pan and stale queue refusal.
- [x] Add the M5 schema, runner and validator with gate-specific measurements, artifact hashes and negative tests. Discover/retain existing qualification command contracts; never overwrite M2–M4 evidence or promote their pending gates.
- [x] Measure retrieval and hit-test latency, index/metadata memory, cache eviction, seek/rebuild races and teardown. Record actual dimensions/object/file counts and test command/toolchain/SDK identity; cite proposed roadmap performance targets as targets, not measured passes.
- [x] After implementation gates pass, document the chosen registration syntax, tool inputs/outputs, identity lifetimes and fallback behavior on the smallest owning docs surfaces. Link to machine-owned schema/tests instead of copying contract fields. Record remaining provider/platform prerequisites and do not relabel prior milestones complete.

## Test matrix and qualification

| Case | Required assertion |
|---|---|
| Title click/prompt | Tuple, source span and before pixels agree; edit changes the intended implementation; validated after evidence names the candidate |
| Repeated scene type/reference and repeated SVG elements | Distinct stable keys; no collapse by type/cache hash/list order |
| Groups, transforms, zoom, crossfade | Painted/video coordinate agreement, global topmost order, ancestor/overlap cycling |
| Late frame/metadata, changed source/style/assets, reopen | Wrong identity is refused; no source lookup using unrelated pixels |
| Missing/duplicate/deleted IDs or anchors | Explicit error/uncertainty; scene/rectangle fallback without invented exact source |
| Shared helpers/tokens and macros/cfg gaps | Broader context and validation; no unsupported isolation claim |
| Too-large source/metadata/snippet/image or cancellation | Bounded memory/replies, explicit truncation/error and owned cleanup |
| Apply/Undo/restart and failure paths | Existing source/permission/history/conflict fences and matching preview/audio handoff survive |
| Old unannotated/M2-only projects | Preview plus whole-project/scene/range editing remain usable |

Development gates: conversion identity/geometry, syntax/tool parity, stale selection/refusal, complete real-SDK title edit, native Linux selection, resources/cleanup. Authentic gates: authenticated-provider element edit/Undo/reopen and writer cleanup, physical display/input, Windows, macOS. The M5 ledger may claim development complete while full qualification stays not_run; fixture-only runs must be labelled development. A probe or compiled test cannot pass an authentic gate.

## Verification and success criteria

```bash
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test agent_task_scope --test candidate_validation
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test element_editing --test scoped_editing --test agent_tools --test selection_tools --test agent_workflow --test promotion_handoff
cargo fmt --all --check
cargo fmt --all --manifest-path desktop/Cargo.toml --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked --manifest-path desktop/Cargo.toml --workspace --all-targets -- -D warnings
python3 desktop/scripts/test-qualification-m5.py
python3 desktop/scripts/validate-qualification.py
```

For the proposed real-SDK test, define and document the same `SDK_BUNDLE` convention used by existing managed-worker tests, then run `SDK_BUNDLE=<verified-sdk> cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test real_sdk_selection -- --ignored`. Native test invocation is discovered from the existing X11 harness; respect process ownership and cleanup. Broaden to both workspace test suites after focused tests because public runtime/protocol/scoped-task contracts changed; report unsupported environment dependencies rather than weakening tests.

Pass development only when every M5 acceptance case has hash-linked evidence and validator negative tests reject false claims. Missing provider credentials/hardware/OS produce a pending authentic gate, not a fixture substitution. The planning task does not run these implementation gates.

## Risks, security and rollback

Selection or retrieval must not bypass M3 source publication or read outside captured source. Keep task capabilities secret and short-lived; evidence/transcripts may contain authored project text, so do not publish them or include credential/private-path data in qualification records. No database migration is planned; if one becomes necessary, back up with the existing SQLite backup API before any change. Roll back by disabling optional metadata/tools and new scopes, while retaining M3 journals and M4 portable styles. Never rewrite accepted history to hide an unsuccessful new feature.
