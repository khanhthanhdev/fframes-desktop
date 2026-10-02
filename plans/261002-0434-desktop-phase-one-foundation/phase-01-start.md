---
phase: 1
title: "Stage 1: Portable models and project state contract"
status: todo
priority: P2
effort: "2-3 engineer-days (tentative)"
dependencies: []
---

# M1 Stage 1: Portable models and project state contract

## Outcome and evidence

Define the smallest typed project and controller contracts without changing renderer transport. M1 requires portable metadata and distinct source/accepted/candidate/job state (`docs/desktop/implementation-plan.md:54`, `:56`). Portable authority is Rust/media/configuration, not duplicated timeline values (`docs/desktop/architecture.md:119`, `:130`). Existing desktop workspace already exists (`desktop/Cargo.toml:1`); root protocol version is already 1 (`fframes-studio-protocol/src/lib.rs:4`).

## Dependencies and ownership

Begin under the user-confirmed development decision in the index. Single Stage 1 executor; Stage 2 waits for this model/test gate. New model instances are per open-project controller, never process-global mutable revision state. SDK environment instances already bind a specific root/target (`desktop/crates/studio-sdk/src/environment.rs:5`, `:13`).

All paths below are under `/root/fframes-desktop/`.

| Action | File | Responsibility |
|---|---|---|
| Create | `desktop/crates/studio-project/Cargo.toml`, `src/lib.rs`, `src/manifest.rs`, `src/paths.rs`, `src/revision.rs` | Portable schema, validation, source inventory/hash policy |
| Create | `desktop/crates/studio-project/tests/manifest.rs`, `tests/revision.rs` | Format, containment and identity checks |
| Create | `desktop/crates/studio-engine/Cargo.toml`, `src/lib.rs`, `src/state.rs`, `tests/state.rs` | Controller state and tagged operation contracts |
| Modify | `desktop/Cargo.toml`, `desktop/Cargo.lock` | Register two crates/dependencies, regenerate lock normally |

Planned symbols/files have no current source citation; these names are proposed ownership, not claims that they exist.

## Requirements

- [x] Schema version, stable project ID, display metadata, SDK compatibility pin, Cargo package/worker target, project-relative assets and generated-instruction version are typed.
- [x] Portable metadata excludes absolute SDK/app-data paths, credentials, sessions and build output. Optional preset reference is opaque metadata only; no preset schema/engine in M1.
- [x] Controller exposes source revision, accepted checkpoint revision, optional candidate revision and explicit job state plus project/open-session identity and generation.

## Numbered implementation steps

1. Add the two workspace members using existing serde/sha2/thiserror conventions. Keep `studio-project` independent of GPUI/SDK/processes; `studio-engine` depends on project models and existing bootstrap/SDK only where orchestration needs them. Do not create a desktop protocol clone or edit root worker wire types.
2. Define version 1 `studio.json`. Store entry package/target explicitly instead of assuming the fixture executable. Validate identifiers, dimensions/FPS if present, bounded strings/list sizes, asset references and SDK pin. Rust remains timing authority. Define a structured error carrying file/field/reason/action.
3. Implement parse then semantic validation. Reject malformed JSON, unsupported older versions without a registered migration, and newer versions with “Update Studio”; never rewrite unknown versions. Preserve an original copy before any future supported migration and make migrations explicit/idempotent. Do not parse a partial unsupported file into writable state.
4. Define project-relative paths: reject absolute paths, drive/UNC prefixes, parent traversal and NUL; canonicalize existing descendants and verify containment. Reject symlinked source/media components, including links that appear between enumeration and copying where the platform supports no-follow opens. Return a specific unsupported-link error rather than following it. Keep filesystem-native paths internally; reject non-round-trippable portable names explicitly rather than lossy hashing.
5. Define deterministic source inventory: normalized relative path plus streaming file content digest, stable path order, explicit file kind. Include Cargo files/lock, Rust, media, instructions and durable style files. Exclude root `.git`, root `target` and specified `.fframes/context`/`.fframes/cache`; do not ignore arbitrary nested `frames` folders. Asset bytes must affect revision. Bound buffers while hashing large media.
6. Define state transitions: opening → ready/error; job queued/running/cancel-requested/succeeded/failed/interrupted. Source revision tracks actual current files; accepted revision is a persisted baseline/checkpoint, not proof of compilation; candidate is immutable proposed output and cannot imply acceptance. Each completion carries project ID, open-session token, base source revision, operation ID and generation.
7. Specify compare-before-install completion rule: reconcile current source first; accept a result only if all tags match and no later source version exists. An edit invalidates candidate/build/cache identities, retains the accepted snapshot and discards late results without writing source. M1 implements guards and checkpoint jobs, not agent transactions.
8. Write meaningful round-trip, malformed/newer schema, path/symlink, deterministic revision and transition tests before publishing these contracts. Test isolation across two simultaneous project instances and close/reopen token changes.

## Data flow

`studio.json` bytes → version check → typed manifest/path validation → portable project descriptor. Source tree → filtered streaming inventory → content revision. Controller command + current descriptor → tagged operation/state transition → result guard → immutable UI state. No renderer IPC or source mutation occurs from parsing.

## Verification and measurable gate

Run from repository root after implementation (these are instructions, not executed evidence):

```sh
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project --test manifest
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project --test revision
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test state
cargo fmt --manifest-path desktop/Cargo.toml --all --check
```

Unit matrix: valid round-trip; missing required fields; malformed and newer version no rewrite; asset traversal/absolute/UNC/symlink rejection; source reorder yields same hash; source/asset edit yields new hash; caches do not; real nested `frames` does; wrong project/session/base/generation completion rejected. Gate: all pass and model docs state lifetime and transition invariants.

## Risk and rollback

High (medium likelihood × high impact): incorrect exclusions lose source or alias two revisions. Mitigate exact root-relative exclusions and asset/nested-directory tests. High (medium × high): shared controller state leaks between projects. Mitigate per-session controller construction and two-project tests. Medium: strict link rejection limits imported projects; preserve files and give an actionable unsupported-path message.

Rollback registrations/new crates in a focused revert before adoption; leave existing SDK/spike paths intact. No user data migrations run in this stage.

## Execution checklist

- [x] Implement schema/path and source revision contracts.
- [x] Implement state transitions and completion guards.
- [x] Pass focused matrix and record evidence.
- [x] Hand frozen contracts to Stage 2; Stage 2 remains unstarted.

## Execution evidence — 2026-10-02

Stage 1 contracts are implemented in `studio-project` and `studio-engine`. The three specified locked integration commands passed: manifest 5 tests, revision 3 tests and state 6 tests. Workspace formatting and scoped clippy with warnings denied passed. The simplification/review pass shared identity guards, aligned imports and fixed excluded declared paths; no source mutation or renderer transport was introduced.

See [implementation report](../reports/implementation-261002-1216-desktop-phase-one-contracts.md). Contracts and invariants are documented in [architecture](../../docs/desktop/architecture.md#implemented-portable-foundation-contracts). Stage 2 may build lifecycle/materialization on these APIs. Source scanning is not atomic; the filesystem owner must serialize mutation and installation and reconcile before completion. Unix no-follow descendant opens were verified on Linux. Windows reparse-point handling is implemented but intermediate-directory race protection and Windows/macOS platform behavior remain unqualified under the accepted development decision.
