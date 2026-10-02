---
phase: 3
title: "Stage 3: Durable state, revisions and interrupted recovery"
status: todo
priority: P2
effort: "4-6 engineer-days (tentative)"
dependencies: [1, 2]
---

# M1 Stage 3: Durable state, revisions and interrupted recovery

## Outcome and evidence

Persist recent projects, checkpoint/draft/job metadata and recover without overwriting external changes. Architecture requires an app-local database and append-only lifecycle journal (`docs/desktop/architecture.md:369`, `:371`), and imported dirty source preservation (`:134`). Existing source revision and draft copy reject symlinks but skip every `frames` basename and buffer full file contents (`desktop/crates/studio-agent-spike/src/session.rs:560`, `:588`); product policy must not inherit those accidental exclusions.

## Dependencies and ownership

Stages 1-2 pass. One Stage 3 executor owns this stage; Stage 4 waits on fault-injection/recovery gates. Controller/store lifetime is per open session with one owned operation writer per project; no mutable process-wide source identity. The database has one engine-owned connection/serialized persistence queue, not a UI-thread connection.

All files are under `/root/fframes-desktop/`.

| Action | File | Responsibility |
|---|---|---|
| Create | `desktop/crates/studio-engine/src/{controller.rs,store.rs,journal.rs,recovery.rs,app_paths.rs}` | Controller, transactional persistence/replay, OS storage |
| Create | `desktop/crates/studio-project/src/checkpoint.rs` | Immutable source manifests and streaming object storage |
| Create | `desktop/crates/studio-engine/tests/{recovery.rs,external_changes.rs,relocation.rs}` | Kill/reopen boundary, stale completion and relink tests |
| Modify | New crates' manifests/exports; `desktop/Cargo.toml`, `desktop/Cargo.lock` | SQLite and filesystem notification dependencies |
| Modify | `desktop/crates/studio-agent-spike/src/session.rs`, `Cargo.toml` | Delegate source inventory/copy to shared product policy while preserving fixture compatibility |

## Requirements

- [x] App-local DB and append-only task journal record typed state without credentials or provider auth values.
- [x] Interrupted jobs retain accepted checkpoint and draft; completed process exit never implies validated/accepted source.
- [x] External edits create a new source identity, invalidate derived results and cannot be clobbered by late completion or recovery.
- [x] Relocation/relink and duplicate project IDs are handled explicitly.

## Numbered implementation steps

1. Add SQLite through `rusqlite` with bundled SQLite for predictable native builds; this dependency supplies atomic metadata transactions and schema migrations demanded by the architecture. Check compatibility with the pinned compiler/GPUI graph when updating the lock. Use a small `notify` dependency for filesystem events; treat events as hints and content reconciliation as authority. Avoid custom DB or polling large media on the UI thread.
2. Define app-data roots using platform conventions: Linux `${XDG_DATA_HOME:-~/.local/share}/fframes-studio`, macOS `~/Library/Application Support/fframes-studio`, Windows `%LOCALAPPDATA%/fframes-studio`; cache may use OS cache root. Inject roots in tests. Resolve through existing GPUI platform facilities if sufficient or a narrow platform-path helper; error if home/platform data unavailable, never fall back to cwd. Store `studio.sqlite3`, per-project journals, immutable objects/checkpoints, stable draft directories and materialized builds outside portable source. Set private directory/file permissions where supported.
3. Add versioned SQL migrations and metadata for project ID/location/recent-open, SDK selection, accepted/source/candidate revisions and tagged jobs. App paths are stored only locally. Start transactions before updating related rows; migrate with backup, refuse newer DB versions without source mutation. DB corruption preserves project/checkpoints/journal and produces repair/rebuild-index action, never wipes data silently.
4. Implement content-addressed file objects and immutable checkpoint manifests via temp write → sync → atomic rename → directory sync where supported. Verify hashes on recovery. Commit a checkpoint reference only after all objects are durable. Stream assets and deduplicate unchanged objects. Never prune accepted/recovery/draft references; bounded retention only removes unreferenced caches/old objects after a successful integrity scan, and disk exhaustion produces actionable failure.
5. Journal each operation with monotonically increasing sequence, transaction ID, project/session/base/candidate identity and stage. Write/sync durable intent, publish immutable files, append/sync commit, then update SQLite projection transactionally. Journal is recovery authority for lifecycle, database is transactional lookup/projection; recovery replays committed idempotent events. Detect truncated final records and quarantine that tail; malformed interior records stop recovery with diagnostics instead of skipping history.
6. Implement checkpoint/draft job boundaries only: capture current accepted baseline on initial create/import; create a stable app-owned draft with base checkpoint; persist queued/running before launching a job. Retain draft and mark interrupted on cancel/crash. Success validates tagged completion and checkpoint durability before updating accepted metadata. Do not implement M3 agent apply/repair/Undo or multi-file source promotion. A simulated job in tests writes real retained draft/checkpoint files; it is a fault injection fixture, never fake product progress.
7. Reconcile source at open, before job launch, before consuming any completion and on debounced filesystem events. Track create/delete/rename/media/instruction changes. Cancel/supersede affected operations and invalidate candidate/build/index identities; retain last accepted checkpoint and last successful presentation as explicitly older revisions. Rescan after dropped/overflowed events. Coalesce hash work off UI thread; recheck source before exposing results, because watcher delivery alone cannot enforce races.
8. Centralize draft/hash policy in studio-project; keep re-export/wrapper compatibility for spike APIs rather than duplicating policy. Preserve public wrapper signatures. Current production callers are `desktop/app/src/worker_project.rs:20`, `:22`; selection guard `desktop/app/src/app.rs:540`; draft copy `desktop/app/src/agent_spike.rs:136`; agent hashes `:157`, `:175`, `:191`, `:202` in that same file. Regression consumers are `desktop/app/tests/worker_roundtrip.rs:40` and session tests `desktop/crates/studio-agent-spike/src/session.rs:654`, `:657`, `:659`, `:722`, `:729`, `:762`. Re-grep at implementation to catch added callers. Validate legitimate nested `frames` source survives copying and root caches stay excluded. Avoid changing fixed fixture hashes without updating grounded assertions.
9. Relink by project ID plus validated folder/content identity. Missing recent path stays listed with Locate/Remove Recent actions. When a moved original no longer exists, relink app-local history after validation. If two existing folders share an ID, ask whether to use an independent copy or the existing project; never attach concurrent edits silently to the same mutable state. Reopening on a different machine reconstructs source baseline from portable files and shows history unavailable; source/media portability must not depend on local history.
10. Recovery order: acquire per-project ownership lock; inspect journal/object integrity; reconcile actual source; replay committed metadata; mark unfinished jobs interrupted; reap only verifiably owned processes; load accepted checkpoint and draft; publish recovered UI state. Never blindly restore accepted bytes over current source. If current bytes differ, retain them as source revision, keep accepted checkpoint separately and show conflict/restore choice. Explicit restore requires current-hash checks and preservation of changed files.
11. Persist process ownership session token/PID/start identity where needed. Use existing process manager for live cleanup (`desktop/crates/studio-bootstrap/src/process.rs:559`, `:586`). On restart do not kill arbitrary PIDs because a number was reused; without ownership proof mark interrupted and report possible orphan. Close cancels/reaps only owned descendants and awaits bounded completion, then closes DB/watch resources.
12. Test every durable boundary with an actual child-process kill/restart against a temp app root, plus external modification during each boundary. Replay twice must be idempotent and source bytes untouched. Check DB migrations, journal truncation/corruption, missing object, duplicate IDs, moved folder, two controllers, stale completions and disk failure.

## Data flow and compatibility

Portable source → inventory/hash → immutable objects/checkpoint. Controller job intent → durable journal → operation files → durable completion journal → SQLite state projection → UI. Watch event → source reconciliation → new revision/invalidation. Restart → ownership/integrity/replay → interrupted job + retained draft + accepted checkpoint + actual current source. Imported Git stays outside app history management.

No project schema migration is needed merely for a DB upgrade. Initial project baseline means captured source, not a compiled/playable acceptance claim. Authentication remains provider-managed and outside journals.

## Verification and measurable gate

```sh
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test recovery
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test external_changes
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test relocation
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-agent-spike
```

Unit matrix: transition sequence, schema upgrade/newer refusal, journal decoder/truncated tail/interior corruption, object integrity and retention roots. Integration matrix: actual interrupted draft job; intent/object/commit/DB crash boundaries; replay twice; external edit preserves bytes; watcher overflow rescans; stale base/session/generation rejected; duplicate project IDs separated; permission/disk full leaves prior checkpoint; no cross-project cleanup. Gate: reopened job is interrupted, accepted snapshot/draft hashes match precrash hashes, externally modified source remains exact, and all stale results are rejected without source writes.

## Risks and rollback

High (medium × high): journal/DB disagreement falsely marks job accepted. Mitigate durable ordering, idempotent replay and process-kill boundary tests. High (medium × high): recovery overwrites manual changes. Mitigate source-first reconciliation and no automatic restoration. High (medium × high): restart kills a reused PID. Mitigate session/start identity and conservative cleanup. Medium: media hashing/retention costs; stream/deduplicate and move work off UI thread.

Rollback controller adoption while retaining app data. Back up DB/journal before migrations; old apps must refuse newer schema, not downgrade it. Portable source remains openable independently. Never delete drafts/checkpoints to make rollback appear successful.

## Execution checklist

- [x] App paths, DB migrations, durable journal and checkpoints complete.
- [x] External changes, tagged guards and relink semantics complete.
- [x] All crash-boundary tests pass with no source overwrite.
- [x] Stage 4 receives recovery/UI state contract and cleanup ownership evidence.

## Execution evidence — 2026-10-02

Engine state/recovery/external-change/relocation suites passed. Recovery kills a real child at durable intent, object capture, draft publication, commit, SQLite projection and running boundaries, edits live source at each boundary, then reopens twice. Accepted checkpoint identity, published drafts, interrupted state and exact external bytes survive; stale completions are rejected and no owned process remains. Two-controller exclusion, moved folders and duplicate IDs have explicit tests.

SQLite migration preserves a synced backup and unrelated tables; newer databases and missing/corrupt objects refuse recovery without source mutation. Journal tests cover quarantined partial tails, fatal interior corruption, 1 MiB entry limits and Linux `/dev/full` write failure; poisoned writers refuse further appends until reopen. Watcher hints are retained until reconciliation starts, and completions always rescan source.

The product recovery action is **Restore as copy**: it exports the saved checkpoint selected before the picker opens under a fresh project ID, even if current source becomes unreadable. It never replaces current source or imported Git. The source-sensitive controller `restore_as_copy` API retains a strict successful-reconciliation/expected-source guard; the product recovery action uses independent `export_checkpoint`. Objects/drafts are retained conservatively; no history pruning is introduced. Stage 4 receives bounded immutable project presentation, interrupted/draft/recovery-notice fields and terminal shared process-owner shutdown, tested against late background spawns.

Follow-up revision verification: deterministic SQLite triggers prove exact newest-checkpoint retention after a committed checkpoint fails projection, and startup failure at queued/running boundaries leaves retryable failed state with its draft retained. Failed draft publication cannot reuse its operation/directory. Invalid JSON, invalid Cargo, deleted declared assets and changed project identity reject completion, invalidate old work even after edit-back, preserve exportable history, and allow retry after repair. Failed invalidation projection cannot resurrect the old tag. Both earlier unversioned checkpoint formats reopen, relocate and export without manifest rewriting; unknown formats and unprotected executable metadata are rejected.
