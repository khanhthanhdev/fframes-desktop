---
phase: 3
title: "Runtime Styles and project snapshots"
status: completed
priority: P2
effort: "4-6 engineer-days"
dependencies: [1, 2]
---

# Phase 3: Runtime Styles and project snapshots

## Goal

Apply a preset as a portable, durable project revision; preserve overrides across reapplication/reopen; expose semantic values through generated-project `fframes::Styles`; and make preset selection/import usable in the native shell.

## Context and key insights

- `studio-project::Manifest` already stores optional preset id/hash, and `SourceInventory` includes durable style files. Keep `studio.json` v1 unless implementation proves a migration is necessary; do not duplicate compiled timeline values in the manifest.
- M3 `Controller` owns serialized source mutation and `edit_transaction` already protects Apply/Undo with hash/path checks, no-clobber publication and recovery. A preset application is a Studio-originated project mutation, not an agent task; it must not forge task acceptance or bypass the transaction boundary.
- M2 keeps the prior immutable preview/audio usable until matching timeline, frame and audio are ready. A new style revision must use that same handoff.
- A generated project depends on the published `fframes` crate, not a desktop-only crate. Implement the small typed runtime accessor in `fframes`, behind an additive `styles` feature using its existing optional JSON parser, rather than making customer projects depend on `studio-presets`. `Styles` accepts caller-provided JSON text/bytes and performs no filesystem I/O; generated projects load the project-local `style/tokens.json` with `include_str!` so the same path works in native and WASM builds.

## Requirements

- [x] Store a self-contained preset snapshot under portable `style/` files: exact preset metadata/hash, resolved canonical `style/tokens.json`, user-owned `style/overrides.json`, guide and references. Copy runtime font/media resources under a stable project-relative media path with licenses; never depend on the user's preset-store directory.
- [x] Apply/reapply/reset through a new serialized `studio-engine` project mutation API that reuses the durable no-follow/no-clobber file-set transaction and recovery semantics. Compare the captured project source before commit, retain conflicting variants, and invalidate source/build/task/preview generations after a committed change.
- [x] Do not record preset application as an agent-authored accepted task. Keep source revision, M1 saved checkpoint, M3 task history and displayed preview identities distinct; crash recovery must replay or expose the preset file-set mutation without guessing.
- [x] Default reapplication merges the newly resolved preset defaults with existing project overrides. Provide a separate explicit reset action; preserve user-authored overrides byte-for-byte where still valid and surface type-mismatch/orphaned overrides instead of silently deleting them.
- [x] Add a public additive `fframes::Styles` API behind a `styles` feature. Parse caller-provided validated resolved token JSON once in the video constructor; generated templates load `style/tokens.json` with `include_str!`, not runtime filesystem access. Provide typed color/typography/dimension/motion accessors with explicit missing/type errors and no I/O, allocation-heavy work, fallback-to-black or parsing inside `render_frame`.
- [x] Update generated Rust project templates and instructions to construct `Styles` once and use semantic tokens. Existing projects without styles continue to open/build and can choose a preset later; do not rewrite imported Rust source automatically.
- [x] Add native preset controls for bundled/imported preset selection, CSS import report, export, basic project override editing and explicit reset. Video preset styling never changes Studio chrome.
- [x] Include bundled preset resource notices and preserve all upstream licenses in projects/exports.

## Architecture

The app asks the GPUI-free `studio-presets` crate to validate, resolve and materialize a complete after-file set. The controller serializes the source-fenced mutation using the existing durable transaction/recovery primitives; only after its commit does the app schedule the ordinary revision-safe M2 preview build. Failed mutation or preview preparation preserves the previous source/history as appropriate and keeps the last playable preview; a committed source with failed preview is labeled as awaiting that revision.

The portable snapshot separates installed preset defaults, project overrides, resolved runtime values and supporting guide/resources. Runtime files are deterministic and covered by source hashing. Project font families map to bundled files; system fonts are not a reproducibility fallback. The `Styles` runtime API consumes resolved values only and is constructed outside the per-frame callback. Stage 3 modifies the shared controller/transaction boundary; there is one owner until tests for fault-injected Apply/reopen pass.

## Files to create or modify

All paths are rooted at `/root/fframes-desktop/`.

| Action | Path | Responsibility |
|---|---|---|
| Create | `fframes/src/styles.rs` | Feature-gated public typed `Styles` parser/accessor and structured errors. |
| Modify | `fframes/src/lib.rs`, `fframes/Cargo.toml` | Export `Styles`; add additive `styles = ["dep:serde_json"]` feature without changing defaults. |
| Create | `fframes/tests/styles.rs` | Feature-gated type, malformed data, color-alpha and token lookup tests. |
| Modify | `desktop/Cargo.toml`, `desktop/Cargo.lock`, `desktop/crates/studio-engine/Cargo.toml`, `desktop/app/Cargo.toml` | Add the preset package to the engine/app dependency graph. |
| Create | `desktop/crates/studio-engine/src/preset_state.rs` | Source-fenced, durable Studio-originated preset mutation contract. |
| Modify | `desktop/crates/studio-engine/src/controller.rs`, `lib.rs`, `edit_transaction.rs`, `journal.rs` | Serialized mutation/recovery, source reconciliation and explicit provenance without task-history forgery. |
| Modify | `desktop/crates/studio-project/src/lifecycle.rs`, `lib.rs` | Optional preset-aware creation/materialization while preserving existing constructors. |
| Create | `desktop/crates/studio-engine/tests/preset_state.rs` | Apply/reapply/reset, conflict and crash-recovery boundaries. |
| Create | `desktop/app/src/preset_panel.rs` | Preset list/detail, import/export/CSS report and override actions. |
| Modify | `desktop/app/src/studio_shell.rs`, `desktop/app/src/lib.rs` | Route preset UI and committed preset revisions through existing preview coordination. |
| Create | `desktop/app/tests/preset_workflow.rs` | Native/controller lifecycle and old-preview-on-failure integration tests. |
| Create | `desktop/scripts/test-preset-relocation.py` | Managed-SDK two-preset render and relocate/reopen reproducibility check. |
| Modify | `desktop/crates/studio-project/templates/Cargo.toml`, `templates/src/lib.rs`, `templates/AGENTS.md` | Enable `fframes/styles`, construct the helper once, and teach semantic token/media use. |

## Tasks & steps

### Task 3.1 — Establish mutation and recovery baselines

- **Goal:** Record the current M2/M3 behavior and journal format before adding Studio-originated preset changes.
- **Target files and symbols:** No source edits. Existing targets: `desktop/crates/studio-engine/tests/edit_transaction.rs`, `desktop/crates/studio-engine/tests/task_recovery.rs`, `desktop/crates/studio-engine/tests/recovery.rs`; `desktop/app/tests/project_foundation.rs`, `desktop/app/tests/preview_coordination.rs`, `desktop/app/tests/audio_preview.rs`.
- **Steps:** Run the focused tests; inspect `TaskEvent` in `desktop/crates/studio-engine/src/edit_transaction.rs`, the durable event/recovery path in `desktop/crates/studio-engine/src/journal.rs`, and record the current no-clobber platform gate.
- **Success criteria:** Focused baseline suites pass and existing journal records remain understood and unchanged.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test edit_transaction --test task_recovery --test recovery` and `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test project_foundation --test preview_coordination --test audio_preview` each exit 0 and print `test result: ok`.

### Task 3.2 — Materialize project-local preset snapshots transactionally

- **Goal:** Apply/reapply/reset a preset as a source-fenced durable file-set mutation, preserving overrides and recoverable provenance independently of agent task history.
- **Target files and symbols:** New `desktop/crates/studio-engine/src/preset_state.rs`; `desktop/crates/studio-engine/src/controller.rs`, `lib.rs`, `edit_transaction.rs`, `journal.rs`; `desktop/crates/studio-project/src/lifecycle.rs`, `lib.rs`; new `desktop/crates/studio-engine/tests/preset_state.rs`.
- **Steps:** Resolve a complete after-file set through `studio-presets`; preserve valid overrides on reapply and remove them only on explicit reset; compare captured source revision before commit; add an explicit journal record without reinterpreting old records; use existing no-follow/no-clobber publication; fault-inject each durable boundary; invalidate only after a committed source revision.
- **Success criteria:** Apply/reapply/reset, conflicting external changes and every injected crash boundary either recover to the old or new complete file set; task acceptance/history is untouched; unqualified platforms cannot publish preset mutations.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test preset_state --test edit_transaction --test task_recovery --test recovery` exits 0 and prints `test result: ok`.

### Task 3.3 — Add the feature-gated `fframes::Styles` runtime API

- **Goal:** Let generated videos access already-parsed typed style tokens with no file I/O or parsing in `render_frame`.
- **Target files and symbols:** New `fframes/src/styles.rs`; `fframes/src/lib.rs`; `fframes/Cargo.toml`; new `fframes/tests/styles.rs`; `desktop/crates/studio-project/templates/Cargo.toml`, `desktop/crates/studio-project/templates/src/lib.rs`, `desktop/crates/studio-project/templates/AGENTS.md`.
- **Steps:** Add an additive `styles` feature using the existing optional JSON dependency; implement structured missing/type/invalid-data errors over caller-provided JSON text/bytes; update the generated template to load `style/tokens.json` with `include_str!` and parse once at construction; add a feature-on native/WASM compile fixture and preserve builds without the feature.
- **Success criteria:** Feature-enabled fixture reads the expected tokens; missing/wrong-type values return errors; existing default builds are unchanged; no parsing or I/O is introduced in the per-frame callback.
- **Verify:** `cargo test --locked -p fframes --features styles --test styles` and `cargo test --locked -p fframes --lib` each exit 0 and print `test result: ok`; `cargo check --locked --manifest-path Cargo.toml -p fframes --features styles --target wasm32-unknown-unknown` exits 0.

### Task 3.4 — Integrate native preset controls and revision-safe preview handoff

- **Goal:** Allow bundled/imported selection, CSS report review, export, override editing and explicit reset while keeping video styles separate from Studio chrome.
- **Target files and symbols:** New `desktop/app/src/preset_panel.rs`; `desktop/app/src/studio_shell.rs`, `desktop/app/src/lib.rs`; new `desktop/app/tests/preset_workflow.rs`.
- **Steps:** Route preset operations through the controller API, never filesystem mutation in GPUI; request a matching M2 preview only after commit; retain and label the previous playable preview on failure; expose whether source committed or was retained as a conflict.
- **Success criteria:** UI integration tests cover selection, CSS report, override/reapply/reset, source conflict, failed preview, and old-preview retention; no preset styling changes Studio chrome.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test preset_workflow` exits 0 and prints `test result: ok`.

### Task 3.5 — Close snapshot, relocation and platform gates

- **Goal:** Demonstrate that preset styles are reproducible from the project snapshot and that writes are enabled only on qualified OS/filesystem combinations.
- **Target files and symbols:** `desktop/crates/studio-engine/tests/preset_state.rs`; `desktop/app/tests/preset_workflow.rs`; `desktop/crates/studio-project/templates/`; new `desktop/scripts/test-preset-relocation.py`; `desktop/qualification/` evidence for the existing no-clobber/recovery probe.
- **Steps:** Render identical source under two presets; reapply with overrides; close/reopen and relocate the project; compare preset/token/font/resource hashes and output; test legacy projects without styles; run the existing publication and recovery probes on the target; leave other targets disabled and `not_run` unless measured.
- **Success criteria:** Two preset snapshots yield the expected distinct output; overrides survive reopen; relocated resources resolve identically; a failed or unqualified publication gate leaves mutation disabled and the previous playable preview intact.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets -p studio-project -p studio-engine -p fframes-studio` exits 0 and prints `test result: ok`; `cargo fmt --manifest-path Cargo.toml --all --check` and `cargo fmt --manifest-path desktop/Cargo.toml --all --check` each exit 0; `python3 desktop/scripts/test-preset-relocation.py` exits 0 and reports matching token/font/resource hashes before and after relocation.

## Verification

```sh
cargo test --locked --manifest-path Cargo.toml -p fframes --features styles
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets -p studio-project -p studio-engine
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test preset_state
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test preset_workflow
cargo fmt --manifest-path Cargo.toml --all --check
cargo fmt --manifest-path desktop/Cargo.toml --all --check
```

Then build a fresh generated project with the managed SDK, render one identical source revision under two presets and compare output hashes/pixels; relocate/reopen it and verify identical token/font/resource hashes. Use the app's UI/preview test workflow to inspect the selected-preset and explicit-reset states. M3 task recovery/Undo and M2 audio/video handoff regressions must still pass.

## Failure Protocol
If any Verify step does not meet its stated pass condition, STOP this phase.
Do not retry blindly or infer a pass from partial output. Use an available
advisor agent for independent, bounded analysis when useful, and provide:
- the phase and task id,
- what you attempted (the steps you ran),
- the exact command and its full output,
- the pass condition it failed to meet.
Apply any relevant guidance, then re-run the Verify step. If no suitable advisor
is available, STOP and report the same failure evidence to the user.

## Platform and qualification boundary

Preset schema validation and preview rendering may be developed portably, but applying preset changes to a project must remain disabled on any OS/filesystem combination whose no-clobber publication and crash-recovery gates are not qualified. M4 development qualification targets native Linux x64. macOS and Windows remain `not_run` unless independently exercised; their mutation controls stay disabled until their publication and recovery probes pass.

## Risks and rollback

| Risk | Response |
|---|---|
| Preset mutation bypasses accepted-task or external-writer safety | Reuse controller-owned durable transaction code, add source provenance distinct from agent task history, and halt on any hash/inode conflict. |
| Source commits but preview does not | Preserve old playback with an explicit awaiting-preview label; rebuild committed source normally, never undo it implicitly. |
| User override lost during preset switch | Keep overrides as a distinct layer, test reapply/reopen and require explicit reset to remove them. |
| Runtime helper causes per-frame work or panic | Parse once in constructor, return typed `Result`/lookup errors, and test no I/O in render path. |
| Existing projects fail due missing style files | Treat style snapshot as optional for old imports; provide actionable fallback and leave their source untouched until the user applies a preset. |

Rollback disables preset mutation/UI routing and the new feature in generated templates; preserve already-created style files and journal records for newer-compatible recovery. Do not delete snapshots or roll source back with Git.

## Caveat

The relocation script passes against a locally assembled native managed SDK from this checkout. `desktop/scripts/test-preset-relocation.py --sdk <assembled SDK> --out <fresh directory>` reports `fframes::Styles` present and runs without the overlay branch; the checked-in pre-M4 scaffold also builds/renders without source or Cargo rewrites. This proves the development SDK assembly path, not publication of a new SDK release.

## Todo list

- [x] Implement source-fenced preset snapshot Apply/reapply/reset with durable recovery.
- [x] Add feature-gated typed `fframes::Styles` and generated-project guidance.
- [x] Integrate native preset controls and prove two-preset/override/reopen/relocation behavior.
- [x] Pass M3 transaction/recovery and M2 preview/audio regression suites.

Verification notes (2026-10-06, native Linux x64): focused/full desktop suites, both formatting gates, desktop and `fframes` Clippy, and the styles-enabled `fframes` tests pass. The preset relocation script passes against the locally assembled native managed SDK without overlay: editorial/pulse output differs, override pixels match across reopen/relocation, token/font/resource hashes match, and the pre-M4 scaffold builds/renders unchanged. Native preset controls have model/controller coverage. Windows and macOS remain `not_run`; project mutation remains disabled there.

## Success criteria

The same Rust source renders with two semantic token snapshots, project overrides remain stable until explicit reset, a preset operation is a recoverable project-source revision rather than an agent task, and the previously displayed video/audio survives any failed style build or transaction.
