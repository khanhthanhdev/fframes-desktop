---
title: "Stage 3: Recoverable promotion, Undo and restart"
status: todo
---

# Stage 3: Recoverable promotion, Undo and restart

## Context and dependencies

Depends on Stage 1 writer ownership and Stage 2 immutable candidate/report/build leases. Estimate: 6–9 engineer-days. This stage exclusively owns transaction/persistence surfaces; no parallel edits to journal, state, checkpoint or database migrations.

Existing journal has metadata-only Intent/Commit records (`desktop/crates/studio-engine/src/journal.rs:21`), with synced append at `journal.rs:122`. SQLite stores serialized project records (`desktop/crates/studio-engine/src/store.rs:10`), versioned migration at `store.rs:23`; controller reopens retained drafts without accepting pending jobs (`controller.rs:117`). Source reconciliation invalidates obsolete jobs (`controller.rs:259`). None implements multi-file publication recovery.

M1 `ProjectState::complete` accepts checkpoint bytes only equal to its source base (`desktop/crates/studio-engine/src/state.rs:270`); proposal stores a candidate without applying it (`state.rs:298`). Keep saved checkpoint acceptance distinct from new validated-task acceptance. M2 installation checks current portable source/session/generation (`desktop/crates/studio-engine/src/preview_state.rs:324`); it must receive an explicit candidate-to-published relationship, not a forged base tag.

## Requirements and data flow

- [x] Record a validated task revision separately from saved checkpoint: task/source base, prior saved history, candidate/published revisions, before/after objects, prompt summary, changed paths/modes, SDK/build key and validation-report hash.
- [x] Capture current source at task launch, including dirty imported bytes. Automatic Apply cannot restore the old checkpoint. Freeze accepted-history generation separately; if either task source or relevant history changed, stop with conflict.
- [x] Persist per-project app-local review policy with default automatic Apply after validation; manual review requires explicit Apply. Default absent legacy fields with serde, preserve old records and disclose setting does not travel with portable project.
- [x] Apply create/replace/delete/executable-mode operations through a durable file-set transaction with immutable before/after objects, path guards, expected hashes and per-operation progress. Rename is represented as delete/create. Never blanket Git reset.
- [x] Undo creates a new durable task-history transition using the guarded inverse delta, not whole-tree restore; preserve unrelated user changes, Git metadata and instructions. Overlapping external changes cause conflict.

Promotion input is a complete validation report and retained candidate build/preview lease. Under the controller/project mutation gate, reconcile project identity, current source and task source base; inspect every destination and parent without following links. Reject symlinks, path escape, nonregular files and conflicting creates/deletes/modes. Initial Apply policy on any external source change is conservative refusal with a preserved candidate; do not introduce automatic text merges. User can review/export the retained draft or launch a new task from current source. Undo permits unrelated changes outside its changed set, but must build/validate the resulting merged inventory before acceptance/install; conflicting touched files remain untouched.

A preflight scan or `owner.lock` does not exclude an editor that ignores the app's lock. Before enabling auto-apply, prove a publication protocol on the accepted Linux target with same-directory staging, durably displaced originals and **no-clobber** publication. Move the live original to a uniquely reserved recovery slot; verify the displaced file identity/hash/mode against expected before installing the staged replacement without overwriting a newly created destination. If an outside writer replaces the destination or changes a displaced open file, retain every observed variant and halt as conflict; do not delete its displaced inode. Directory identity changes or ambiguous link/rename outcomes also halt. Keep source backups durable until recovery/retention rules explicitly permit release. Ordinary filesystem writes are not globally atomic.

The engine can serialize Studio writes, but cannot guarantee exclusion of malicious arbitrary writes by an uncooperative external process. Linux primitive tests must demonstrate no silent loss for editor save-by-rename, in-place/open-descriptor writes and destination recreation at publication boundaries. If a platform cannot meet that protocol, its Apply gate stays blocked; source scan alone is not a fallback. Windows/macOS need their own primitives/evidence before their mutation support is claimed. No platform-wide security sandbox claim follows from passing these tests.

## Durable sequence and recovery policy

1. Freeze writer and verified candidate; prepare/verify before and after object inventories. Stage changed files with correct modes; sync contents/directories. Persist immutable validation report and an intent containing the complete forward/inverse file set, source/history bases and recovery paths.
2. Acquire serialized project mutation gate; verify identities/hashes immediately around each displacement/publication. Append and sync progress after each durable boundary. Exclude active internal staging/recovery paths from source inventory only by exact app-generated names; preserve arbitrary user files.
3. Verify final published inventory exactly equals validated candidate for Apply. Append durable published/accepted task commit; only then update in-memory state and SQLite projection. A metadata/append failure suspends further mutation and retains all variants for recovery.
4. Obtain a fresh install authorization naming published candidate revision and current session/generation. Re-prime existing M2 worker/first-frame/audio at latest position, reserve a new audio epoch and commit matching video/audio. Failed handoff keeps old preview and visibly labels the accepted source as awaiting preview; do not undo committed source implicitly.
5. On open, replay transaction journal before trusting normal source state. For uncommitted partial Apply/Undo, prefer rolling back the completed delta only where current files still equal transaction after bytes and displaced originals are verified. Never overwrite unknown bytes; conflicted recovery preserves current source, candidate, backups and last committed acceptance, and blocks new mutation until resolved.
6. A durable transaction commit with stale/missing SQLite projection rebuilds the projection from journal. Resume preview from immutable committed revision; pending writer/build becomes interrupted. Never automatically resume prompts/repair or infer success from old PIDs/process exits.

Undo verifies expected task-after bytes only on touched paths, captures current full source, forms inverse-delta candidate preserving unrelated edits and validates it through Stage 2. New task provenance points to the undone revision and resulting inventory, which may differ from the historical predecessor when unrelated edits exist. Source/validation/history fences are rechecked before publication. No-op/disabled Undo has an actionable reason.

## Files owned

| Action | Absolute path | Responsibility |
|---|---|---|
| Create | `/root/fframes-desktop/desktop/crates/studio-engine/src/edit-transaction.rs` | File-set Apply/Undo, durable progress and replay/conflict decisions. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/journal.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/src/store.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/src/controller.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/src/state.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/src/lib.rs` | Versioned task journal/projection, policy defaults, legacy state compatibility and serialized mutations. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/agent-task.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/src/preview_state.rs` | Validated acceptance/install authorization with distinct candidate/base identity. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-project/src/checkpoint.rs`, `/root/fframes-desktop/desktop/crates/studio-project/src/paths.rs`, `/root/fframes-desktop/desktop/crates/studio-project/src/revision.rs` | Reuse verified objects; safe no-follow staging, mode inventory and exact internal-path exclusion. |
| Modify | `/root/fframes-desktop/desktop/app/src/preview_coordinator.rs`, `/root/fframes-desktop/desktop/app/src/studio_shell.rs` | Staged candidate to published preview authorization and guarded handoff. |
| Create | `/root/fframes-desktop/desktop/crates/studio-engine/tests/edit-transaction.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/tests/task-recovery.rs` | Apply/Undo/interleaving and every durable boundary. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/tests/recovery.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/tests/external_changes.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/tests/preview_state.rs` | Legacy recovery/stale fences remain effective. |

Existing state constructor production site is `desktop/crates/studio-engine/src/controller.rs:99`; Record construction is in that controller open path. Project state remains per open session, Store per serialized backend (`store.rs:18`), and no new global mutable task state is allowed. Keep previous JSON journal records readable; new events use an explicit format version. Do not rewrite old immutable checkpoint inventories; both unversioned historical formats remain readable.

## Migration, tests and commands

Before schema/data migration, create a consistent SQLite backup using SQLite backup semantics (including WAL state), sync it and retain unknown tables. Add optional/defaulted task/policy fields and additive projection tables. A newer unsupported format opens in diagnostic/read-only mode without mutating source/history. Preserve M1 restore-as-copy, relocation, duplicate-ID, invalid-source and recent-list behavior.

Unit tests cover state/acceptance distinction, review default, forward/inverse delta and path/mode conflicts. Integration matrix covers multi-file creates/deletes/replacements, dirty import, unrelated Undo edits, touched-file conflicts, same-file TOCTOU, link/directory swaps, open descriptors, disk full, corrupt/missing objects, lost projection, truncated tail/interior journal corruption, and repeated idempotent recovery. Inject interruption before/after every intent, staged sync, displacement, publication, progress append, final inventory, commit and database boundary. For each assert bytes/permissions/Git unchanged outside task, acceptance never advances without durable commit, and all unresolved variants survive.

Create proposed tests first, then run `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test edit-transaction --test task-recovery`. Broaden to `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine -p studio-project` and `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test project_foundation --test preview_coordination`. Prove source commit/preview handoff failure, latest seek/epoch rejection and matching audio/video under real SDK tests; record skipped prerequisites separately.

## Risks, response and rollback

| Likelihood × impact | Failure and pre-decided response |
|---|---|
| High × high | External writer races: displaced originals plus no-clobber publication; retain unknown variants, halt conflict, no unsafe scan-only fallback. |
| Medium × high | Crash/disk failure mid-file set: synced progress and immutable objects; conditional rollback on restart, no overwrite of unknown bytes. |
| Medium × high | Migration/journal incompatibility: consistent backup, additive version/defaults, refuse newer format rather than downgrade-write. |
| Medium × high | Source committed but preview not ready: retain old playable preview with explicit revision label; rebuild committed revision, never retag old pixels/audio. |

Implementation rollback disables new Apply/Undo but does not erase task history or attempt a blanket source reversal. Resolve pending mutations with the current transaction reader first. Restore an older database backup only through explicit compatible recovery while preserving new journal/backups; older binaries must not write newer history. Retain last playable M2 revision and independent checkpoint exports.

## Gate

- [x] Linux mutation primitives and interleaving tests prove conflict preservation before auto-apply is enabled.
- [x] Apply/Undo are durable, validated task revisions; current source and saved checkpoint remain semantically distinct.
- [x] Every crash boundary recovers idempotently or exposes preserved conflict; restart never silently replaces external edits.
- [x] Matching video/audio handoff keeps old playback on failure and never installs a candidate using accepted-base identity.
