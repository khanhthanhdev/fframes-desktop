---
title: Desktop M1 Phase 1 revision defects resolution
date: 2026-10-02
summary: Resolved 9 recovery, isolation, and process supervision defects identified in review
---

# Desktop M1 Phase 1 revision defects resolution

Resolved all 9 review defects across recovery, isolation, and process supervision:

1. **Journal commit memory synchronization**: `Controller::persist` and `begin_job_observed` advance in-memory `self.record` immediately upon durable `journal.commit()`, preventing stale in-memory state from overwriting durable journal checkpoints on subsequent operations if SQLite save fails.
2. **Worker executable isolation**: `launch_portable_worker` synchronizes cargo build using a target directory file lock, copies the compiled binary into `build.isolated_bin_dir` (a private per-materialization path), and sets executable permissions before releasing the lock, eliminating race conditions with other builds sharing the target directory.
3. **Contained dependency path rewriting**: `build_materialization::bind` detects absolute paths in dependencies and rewrites contained paths to `copied_root.join(&relative)`, preventing materialized manifests from referencing live project sources.
4. **Executable permissions tracking and restoration**: `SourceFile` records `executable: bool`. Both `SourceInventory::scan` and `verify` include the executable bit in the revision SHA-256 calculation so mode changes invalidate revision identity. `Checkpoints::draft` and `copy_draft` restore `0o755` permissions on Unix.
5. **Workspace worker runtime directory**: `launch_portable_worker` passes `build.manifest.parent().unwrap_or(&build.root)` as runtime cwd to `spawn_worker`, allowing runtime `read_folder("media")` to resolve package-relative media while keeping cargo build at workspace root.
6. **Failed checkpoint terminal transition**: `checkpoint` catches capture and completion failures and persists a `JobResult::Failed` terminal transition via `fail_current_job`, preventing the job from being stuck in `JobState::Running` and allowing retry without restart. If persistence fails, `recovery_notice` records required recovery.
7. **Missing asset checkpoint recovery**: Added `studio_project::open_for_recovery` and updated `Controller::open` to fall back to recovery state when source is degraded (e.g. missing declared font). Exposes `export_checkpoint` and allows `restore_as_copy` to recover intact checkpoints from history without requiring healthy current source.
8. **Scoped SDK verification cleanup**: `ProcessTreeManager::sub_manager` creates scoped managers that publish children to parent for app-wide shutdown while scoping `terminate_all` to verification children only. `install.rs` uses `sub_manager` to prevent sweeping unrelated children.
9. **Compiler probe cancellation**: `Doctor::verify_candidate_sdk_with_processes` and `probe_bundled_compiler` propagate app-owned `ProcessTreeManager` and scope probe cleanup via `sub_manager`, ensuring app cancellation terminates compiler probes.

Verification: All 84 desktop tests passed, including targeted regression tests for database failure rollback, failed checkpoint retry, missing asset recovery, absolute contained dependency rewriting, executable permission revision invalidation/restoration, and sub-manager scoped process cleanup. `cargo clippy --all-targets -- -D warnings` and `cargo fmt` passed with zero warnings or diffs.

## Follow-up review and repair

The earlier passing suite did not prove all nine claims: review reproduced a scoped-spawn shutdown race, live degraded-source failure, and queued-job deadlock after SQLite failure. The chmod-based SQLite test could pass without injecting failure. Further inspection found unversioned inventory compatibility and one unowned setup compiler-probe call.

- Root and scopes now share a lifecycle lock. Spawn/publication and terminal cleanup serialize; scoped cleanup preserves unrelated children and retains failed-cleanup ownership. The concurrent test inspects the first shutdown before a cleanup sweep can hide the race.
- Reconciliation returns errors for unreadable/degraded source and changed identity, interrupts old work, updates matching recovery inventory, and preserves accepted/draft. Edit-back cannot resurrect a tag, including after failed SQLite invalidation projection.
- Startup failures settle queued/running jobs. Pre-commit draft failure retains its attempt and advances operation identity so retry cannot collide with the old directory. Journal-committed memory remains authoritative if projection fails.
- New inventories declare version 1; both historical unversioned formats load, draft, reopen, relocate and export without rewriting manifests. Unknown versions and unprotected executable metadata fail verification. The oldest format cannot recover executable bits it never recorded.
- SQLite triggers now inject actual cross-platform write failures. Tests assert the injected error, exact latest accepted checkpoint, stale database projection, repaired close/reopen, and exported checkpoint bytes.
- Setup preflight propagates its app process manager. The shell's restore command exports the checkpoint selected before the picker independently of damaged current source; source-sensitive restore retains strict comparison guards.

Final commands from `desktop`: `cargo test --locked --workspace` (98 passed, four ignored), `cargo clippy --locked --workspace --all-targets -- -D warnings`, and `cargo fmt --all --check` passed. The explicitly invoked SDK-backed `portable_managed_worker` passed in 72.22 seconds with source/SDK relocation, package-relative runtime media bytes and workspace-root compile-time media. The ignored fault-injection entry is exercised by its parent crash-boundary test; other ignored native worker gates and native GUI interaction were not rerun in this follow-up. Shell command semantics have a direct backend regression. Changes remain local and uncommitted; platform/authentication qualification remains pending.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
