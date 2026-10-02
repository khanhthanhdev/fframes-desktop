# Phase 1 revisions review

The claim that all nine defects are resolved is premature. Three behavioral failures were reproduced against the current implementation, and the committed SQLite regression does not prove its stated invariant. This review leaves implementation files unchanged.

## Findings

1. **P1: Sub-manager spawn can escape parent shutdown.** In `desktop/crates/studio-bootstrap/src/process.rs:591`, spawn locks the scoped child list, checks the shared stopped flag, and creates the child before acquiring the parent list at line 609. Parent shutdown can set stopped and sweep its empty list during that interval; the spawn then publishes a live child after shutdown has returned. A concurrent reproduction using a barrier, scoped `sleep 30` spawn, and parent shutdown after 100 microseconds failed on iteration zero: spawn succeeded and parent active count was one after shutdown. The test explicitly terminated that child afterward. Serialize creation/publication with shutdown using synchronization shared by the root and all scopes, and add concurrent coverage.

2. **P1: Degraded-source reconciliation leaves checkpoint jobs active.** `desktop/crates/studio-engine/src/controller.rs:259` replaces `self.project` with the recovery inventory but returns without reconciling `record.state.source` or interrupting obsolete work. Completion sees the new revision and returns stale after changing only a cloned state. `fail_current_job` repeats the same stale completion; the caller at line 364 discards this error. Reproduction: remove the declared font at `checkpoint_observed`'s `job_started` boundary, attempt checkpoint, restore the original bytes, then retry. The first call fails with the job still Running and the retry also fails. Apply identity checks and source invalidation to recovery inventories, persist a terminal/interrupted state, and report failure-transition errors consistently.

3. **P2: Database failure during startup leaves a queued job that cannot retry after repair.** `desktop/crates/studio-engine/src/controller.rs:332` correctly advances memory after the journal commit, but SQLite failure at line 334 returns before starting the job or returning its tag. `checkpoint_observed` exits through `?` at line 357 without cleanup. Reproduction: inject a SQLite BEFORE INSERT trigger that raises FAIL at the `commit` boundary; checkpoint errors with Queued state. Drop the trigger and retry; checkpoint fails again because queue rejects an existing active job. Handle committed startup errors by settling or explicitly interrupting the job without rolling back durable state. Recovery handling must support Queued as well as Running.

4. **P2: The SQLite rollback regression can pass without injecting failure or retaining the latest checkpoint.** `desktop/crates/studio-engine/tests/recovery.rs:228` ignores chmod errors and changes permissions on an already-open database; running as root also bypasses this protection. Line 231 ignores the checkpoint result. Line 239 only asserts that accepted differs from the initial baseline, so rollback to checkpoint 1 passes. The substantive body is Unix-only. Inject an actual deterministic write failure after job startup, assert the error, and assert exact equality with checkpoint 2 both in memory and after close/reopen.

## What is verified

| Claimed fix | Review result |
| --- | --- |
| Journal commit memory synchronization | Correct placement in both methods. A separate deterministic completion-failure reproduction retained the exact latest accepted checkpoint in memory and after close/reopen. Existing regression needs replacement. |
| Worker executable isolation | Target lock, unique materialization bin copy, and launch from the copy are present. Real SDK/concurrent build execution was not rerun. |
| Absolute contained dependency rewriting | Code and targeted regression pass. |
| Executable permissions | Revision hashing and draft/copy restoration are present; targeted regression passes. |
| Worker runtime working directory | Launch uses the selected package manifest's parent. Existing workspace worker test replaces runtime media loading with compile-time media, so it does not exercise this particular runtime behavior. |
| Failed checkpoint terminal transition | Simple capture-failure test passes. Startup database failure and degraded-source completion remain defective. |
| Missing declared asset recovery | Reopen/export regression passes. Live reconciliation has the failure described above. |
| Scoped SDK verification cleanup | Sequential scoping regression passes. Parent shutdown race remains defective. |
| Compiler probe cancellation | App manager propagation and scoped probe cleanup are present. Their shutdown guarantee depends on fixing the process race. |

## Validation

- `cargo test --locked --manifest-path desktop/Cargo.toml --workspace`: 84 passed, four ignored. The ignored cases include SDK-dependent worker tests and a child fault-injection entry invoked by its parent test.
- Temporary review probes: three failed as described above; one passed, proving exact checkpoint retention after a deterministic SQLite failure at completion. SQLite test databases were backed up before adding fault-injection triggers. Probes were removed from the crate test directories after execution and retained under `/tmp/fframes-review-phase-one.rs` and `/tmp/fframes-review-shutdown-race.rs` for this session.
- `cargo fmt --all --check --manifest-path desktop/Cargo.toml`: passed after removing temporary probes. An earlier formatting run reported only the intentionally temporary probe files.
- Scoped library Clippy and `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed.
- Native UI, real SDK workers, and Windows/macOS execution were not rerun. Neither SDK_BUNDLE nor FFRAMES_SDK_BUNDLE was set.

This report records review evidence and does not supersede the project specification or execution plan.
