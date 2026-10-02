---
phase: 4
title: "Stage 4: Native shell and M1 verification"
status: todo
priority: P2
effort: "3-4 engineer-days (tentative)"
dependencies: [1, 2, 3]
---

# M1 Stage 4: Native shell and foundation verification

## Outcome and evidence

Integrate the project engine into a native editor shell with actionable errors, SDK state, recent projects and recovery affordances. Existing GPUI spike and setup components provide actual native behavior (`desktop/app/src/app.rs:16`, `:605`; `desktop/app/src/setup_view.rs:67`, `:295`). Existing UI already dispatches blocking operations on the background executor (`desktop/app/src/app.rs:162`). Keep that boundary and preserve Phase 0 qualification routes (`desktop/app/src/main.rs:18`, `:19`).

## Dependencies and ownership

Stages 1-3 pass before UI integration; no parallel edits to engine/app shared files. One Stage 4 executor owns final integration and docs/CI. All paths below are under `/root/fframes-desktop/`.

| Action | File | Responsibility |
|---|---|---|
| Create | `desktop/app/src/{studio_shell.rs,project_view.rs}` | GPUI shell/layout, project state/actions/errors |
| Modify | `desktop/app/src/{main.rs,lib.rs,setup_view.rs,worker_project.rs}`, `desktop/app/Cargo.toml` | Normal launch, reuse setup/build APIs, engine dependencies |
| Create | `desktop/app/tests/project_foundation.rs` | Full engine-backed create/reopen/relocate/recovery flow |
| Modify | `desktop/Cargo.lock`, `.github/workflows/desktop.yml`, `.github/workflows/desktop-phase-zero.yml` | Lock update and new-crate quality gates |
| Modify | `docs/desktop/{README.md,implementation-plan.md}` | Smallest owning navigation/operational description |

Do not move spike view internals merely to share layout. Keep existing fixture/qualification code unless an integration dependency requires a focused edit.

## Requirements

- [x] Standard opaque native window contains project/assets/style navigation, preview region, timeline region and agent panel.
- [x] Create/Open/Import and asset copy actions are real; recent projects, SDK availability and recovered jobs use durable engine state.
- [x] Invalid/missing/unsupported inputs produce corrective UI actions without losing the open project or draft.
- [x] Long filesystem/hash/build/database work stays off UI thread; closed/switched-project completions are rejected.

## Numbered implementation steps

1. Add normal product launch in main while preserving explicit `spike-ui`, `--spike-ui` and `qualify-presentation` command parsing and every existing flag. Choose no-argument launch for the new shell; keep spike command behavior stable. Update help without changing qualification harness routes. Open standard opaque GPUI window using the existing pinned APIs.
2. Compose shell with a project header showing location/name, source/checkpoint revision and SDK/job status; left project/assets/style navigation; preview center; timeline region; agent panel. Show “No preview available”, “Agent editing is not available yet” and “No style preset configured” with honest disabled controls where appropriate; keep milestone identifiers in maintainer docs only. Do not wire spike buttons into advertised product agent editing/playback/presets/selection. Existing spike remains a separate development tool.
3. Wire native folder/file dialogs to actual engine commands for create/open/import and asset copy. Keep native picker cancellation neutral. Render file inventory/assets from validated metadata; style navigation can show existing files/read-only inventory with empty state, without introducing a preset model. Disable conflicting mutations while owned operations commit.
4. Reuse setup status and SDK discovery/preflight/install behavior instead of duplicating a downloader or installing dependencies on open. Show available/missing/incompatible/busy/failed setup separately from project validity. Background dispatch uses engine's operation tags; apply results on UI thread only after project/session/base/generation validation. Switching projects closes watchers and cancels/reaps owned tasks before releasing controller.
5. Persist recent-project opens only after metadata validation succeeds. Expose Locate for missing/moved paths and Remove Recent that removes metadata only. Show relink/independent-copy ambiguity instead of choosing a duplicate project ID silently.
6. Render errors with affected path/field and concrete action: fix manifest, update Studio, locate missing asset, choose compatible SDK, resolve unsupported external dependency, retry after permissions/disk repair. Keep the current project/draft available after a failed operation; never replace state with an empty successful view.
7. On restart show recovered job as Interrupted with accepted checkpoint, retained draft location and actual source status. Offer Reveal Draft/Reveal Checkpoint and explicit restore only through current-hash guarded engine operation. Do not auto-restore or provide functional M3 Apply/Undo. Missing history after cross-machine open is visible and does not prevent portable source/asset access.
8. Keep filesystem enumeration, large media copy/hash, DB and worker build off UI thread using the established background-executor pattern. UI update contains only bounded immutable state. Cancel coalesced work on close, ignore stale callbacks, and preserve ownership proof for cleanup; wait boundedly for engine close before terminating the app.
9. Add integration tests for the complete M1 lifecycle using real temporary files/app data: create with font and source, copy a real asset, close/reopen, move folder, remove original asset, relink, interrupt a real draft job, reopen, edit source externally and deliver old completion. Assert exact byte hashes, error actions and zero owned live processes.
10. Extend both explicit CI package lists to include `studio-project` and `studio-engine`: current lists live at `.github/workflows/desktop.yml:123`, `:124` and `.github/workflows/desktop-phase-zero.yml:121`, `:122`. Keep root protocol/runtime, packaging, real worker and qualification checks. CI build artifacts are not interactive/platform qualification evidence.
11. Update the smallest owning docs after reading them: M1 execution link, normal shell command, app-data/history/relocation rules and supported import limitations. Keep Phase 0 feasibility instructions/harness unchanged and its qualification status pending unless new gate evidence actually exists. Do not mark milestones completed from implementation alone.
12. Perform the native shell acceptance walk below, record platform/environment and errors, stop every owned dev/test process, and compare all M1 acceptance criteria with evidence. Keep unresolved physical display/IME/platform/auth qualification listed separately.

## Data flow

Native dialog/action → background engine command → validated portable files/durable metadata → tagged immutable view state → GPUI render. Filesystem event → engine reconciliation → revised source/error/invalidation status → UI. App close → cancellation/watch shutdown/owned process reap → durable interrupted/completed job state. Shell regions do not create fake timeline/agent/style data.

## Verification commands

These are executor commands, not tests claimed to have run during planning. Run from repository root with documented native prerequisites and desktop pinned toolchain:

```sh
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test project_foundation
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project -p studio-engine
cargo fmt --manifest-path desktop/Cargo.toml --all --check
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-bootstrap -p studio-sdk -p studio-agent-spike -p fframes-studio
cargo clippy --locked --manifest-path desktop/Cargo.toml -p studio-project -p studio-engine -p studio-bootstrap -p studio-sdk -p studio-agent-spike -p fframes-studio --all-targets -- -D warnings
cargo test --locked -p fframes-studio-protocol -p fframes-studio-runtime
python3 desktop/scripts/test-packaging.py
python3 desktop/scripts/validate-qualification.py desktop/qualification/m0-results.json
cargo run --locked --manifest-path desktop/Cargo.toml -p fframes-studio
cargo run --locked --manifest-path desktop/Cargo.toml -p fframes-studio -- spike-ui
```

With a genuine assembled bundle at `/tmp/studio-sdk`, preserve the existing native worker gate:

```sh
SDK_BUNDLE=/tmp/studio-sdk cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test managed_worker -- --ignored
SDK_BUNDLE=/tmp/studio-sdk cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test portable_managed_worker -- --ignored
```

The existing `worker_roundtrip` ignored test requires the compiled annotated fixture (`desktop/app/tests/worker_roundtrip.rs:7`); run it through existing Desktop CI preparation or build that fixture before invoking the existing test. Do not report an ignored test as passed without running it. No browser/editor/WASM gate is needed because no editor bridge changes are planned.

## Test matrix and native acceptance

| Level | Coverage | Observable pass condition |
|---|---|---|
| Unit | Manifest/paths/revisions/state/store/journal | Focused stage suites pass including negative cases |
| Integration | Lifecycle/materialization/asset copy and recovery/external edits | Correct files/checkpoints survive relocation and child-process interruption |
| Regression | SDK/bootstrap/ACP spike/root protocol/frame tests/packaging | Existing focused commands pass and spike controls still function |
| Native end-to-end | Create → import asset → close/reopen → relocate → relink | UI displays same project/asset hashes without original asset/old folder |
| Native failures | Invalid/newer manifest, missing source/media, bad SDK, interrupted job | UI names path/problem/action and retains accepted checkpoint/draft |
| Native lifetime | Slow asset copy, project switch, close during job | Window remains responsive; stale completion ignored; all owned processes reaped |

Do native walkthrough on the supported Linux development environment; record software rendering limitations. Windows/macOS CI compile/test smoke does not qualify IME/physical presentation. Configurable adapters remain preserved with authenticated qualification deferred by the user's earlier decision.

## Risk and rollback

High (medium likelihood × high impact): stale asynchronous UI callback attaches wrong project result. Mitigate session/base/generation guards and switch/close tests. High (medium × high): shell routing breaks qualification harness. Mitigate explicit command/flag regressions and spike walkthrough. Medium: unsupported native dialog/input behavior; rely on pinned GPUI patterns and report target-specific blockers.

Rollback normal shell routing to spike while preserving portable projects/app history and refusing unsupported newer schemas. Revert CI/docs with integration change if needed; retain evidence/history. Stop only processes this task owns, never unrelated user/OS sessions.

## Execution checklist

- [x] Native shell and actual project/SDK/recent/recovery actions implemented.
- [x] Empty states honestly identify later milestones.
- [x] All scoped automated gates and native M1 acceptance pass.
- [x] Docs/CI include new crates and preserve M0 routes/evidence limits.
- [x] Final review checks no external-source overwrite and no orphan owned processes.

## Execution evidence — 2026-10-02

All 78 nonignored desktop tests pass across `studio-project`, `studio-engine`, bootstrap, SDK, agent spike and app. Desktop formatting, locked build and all-target Clippy with warnings denied pass. Root protocol/runtime: 10 tests passed; packaging: 5 passed; qualification-record validation passed while explicitly retaining pending gates. Explicit ignored tests were actually executed: `portable_managed_worker` (70.56 seconds), `managed_worker` (68.95 seconds), and `worker_roundtrip` (0.14 seconds). Both desktop CI workflows include the new crates and real portable-worker test.

Native environment: Linux x64, pinned upstream GPUI, Xvfb `:91` at 1440×1000, 1280×800 product window, software GL, disposable HOME/app data and real local SDK bundle. Exercised Create, native asset chooser/copy, close/reopen, rename/Locate with the original asset removed, project/assets/style navigation, Rust import, SDK installation, newer-schema rejection, missing asset, invalid SDK selection and interrupted recovery. Native import's nine existing files retained exact SHA-256 hashes. Tab/Shift-Tab visible focus and Enter-to-open chooser were exercised; inspection caught missing focus navigation and it was repaired. Asset/SDK diagnostics now identify the path and specific corrective action. Recovery displays changed source separately from checkpoint, retained draft and reveal/restore-copy controls. Final screenshot was inspected with no clipping/overlap/error: `.amp/in/artifacts/studio-foundation-final.png`.

Setup/navigation remained responsive during extraction. Closed the window while setup displayed “Operation in progress”; the tracked native harness exited successfully and process inspection found no surviving Studio, owned Xvfb, SDK compiler or worker. Chunked asset cancellation and terminal process shutdown/late-spawn rejection are additionally covered by automated tests; a compiler was not running at the exact native quit capture. Native captures of busy setup, SDK ready, recovery, missing asset and invalid SDK were inspected rather than treated as screenshots-only proof.

The unchanged Phase 0 native route also ran through `qualify-linux-native.py` on `:92`: 2000 verified renders, 1000 confirmed presentations, real keyboard text and selection of `intro.title`, zero release failures, queue/current-image high water 1. Retained JSON/logs/screenshots are under `.amp/in/artifacts/m0-regression/`; software-rendered host RSS reached about 1.07 GB and the record preserves all samples. This run used the current UID with a fresh HOME, not an isolated account or disabled-network claim. Its application/SDK hashes are recorded. The last edits after that run affect product focus and error diagnostics only; the qualification route and renderer remained unchanged.

Docs describe actual launch/storage/import/recovery limits. Checkpoints save bytes; **Restore as copy** does not implement M3 Apply/Undo. Preview/playback, compiled timeline, agent editing and presets are explicitly unavailable in the product shell. Physical GPU/display/IME, Windows/macOS interactive and authenticated configurable ACP gates remain pending. No commit, push, deployment or release qualification is claimed.

Follow-up revision gate: `cargo test --locked --workspace` now passes 98 tests with four ignored entries; all-target Clippy with warnings denied and formatting checks pass. `SDK_BUNDLE=/tmp/studio-sdk-native-verified cargo test --locked -p fframes-studio --test portable_managed_worker -- --ignored --nocapture` passed in 72.22 seconds. Its contained workspace now actually reads package-relative runtime media and validates copied asset bytes as well as compiling workspace-relative embedded fonts. A direct shell backend test exports the selected saved checkpoint despite newer accepted source and an unreadable live manifest. This follow-up changes recovery semantics, not visual layout; native GUI interaction and the other ignored worker gates were not rerun. Earlier native/platform evidence is historical, not renewed qualification. Latest fixes and compatibility boundaries are recorded in the [revision journal](../journals/2026-10-02-desktop-m1-phase-1-revision-defects.md).
