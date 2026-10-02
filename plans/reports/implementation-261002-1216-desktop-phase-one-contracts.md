# Desktop foundation Stage 1 implementation

Implemented the two GPUI-free model crates and registered them in the existing desktop workspace. Stage 1 is complete; lifecycle, persistence and native shell stages remain pending. Existing Phase 0 code and worker wire types were unchanged.

`studio-project` provides version-gated read-only metadata parsing, bounded semantic validation, portable path validation, link/special-file errors and deterministic streamed source hashing. Declared assets and Cargo entries cannot use excluded cache/build/Git paths. `studio-engine` provides per-open-project source/checkpoint/candidate/build state, fresh session tokens, explicit job transitions and compare-before-install completion guards. Source reconciliation retains accepted checkpoints and rejects stale results, including edits followed by restoring identical bytes.

## Verification

All commands passed on Linux:

- `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project --test manifest`: 5 tests.
- `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project --test revision`: 3 tests.
- `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test state`: 6 tests.
- `cargo clippy --locked --manifest-path desktop/Cargo.toml -p studio-project -p studio-engine --all-targets -- -D warnings`.
- `cargo fmt --manifest-path desktop/Cargo.toml --all --check` and `git diff --check`.

The initial formatting check identified unformatted new files, which were formatted. A portability regression test then exposed an unconnected reserved-name predicate; it was fixed and the matrix rerun successfully. The scoped simplification/review pass removed duplicated identity checks and added the excluded-path regression. No checks were weakened.

## Handoff and limitations

The [architecture contract](../../docs/desktop/architecture.md#implemented-portable-foundation-contracts) describes lifetime, inventory exclusions and completion invariants. A scan is not an atomic filesystem snapshot; later filesystem orchestration must serialize mutation/installation and supply freshly reconciled identity. No persistence, source copying, migration or process execution occurs in these models.

Unix descriptor-relative no-follow opens were exercised on Linux. Windows static reparse points are rejected, but intermediate-directory replacement protection is weaker than Unix and remains unqualified. Existing physical GPU/display/IME, Windows/macOS interactive and authenticated adapter qualification gates remain open. No consumer release claim or commit was made.

No blocking product questions remain for Stage 2.
