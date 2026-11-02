---
phase: 6
title: "fframes-kit in the managed SDK"
status: pending
priority: P1
effort: 3d
dependencies: []
---

# Phase 6: fframes-kit in the managed SDK

## Goal

Let any Studio-built project depend on `fframes-kit` (crates.io) and `fframes-studio-kit`, build
offline against the managed SDK, and share one `fframes` with the SDK, so kit components and
the Props panel work in ordinary Studio projects.

## Context

- fframes-kit 0.1.0 and fframes-kit-macros 0.1.0 are on crates.io (published 2026-10-27). The
  kit depends on `fframes = "1.3.0"` from crates.io.
- Managed builds copy the project into staging, bind `fframes`, `fframes-studio-runtime` and
  `fframes-studio-protocol` to the SDK's framework paths with an exact-version check
  (`desktop/crates/studio-engine/src/build_materialization.rs:388-480`), refuse `[patch]` and
  `[replace]` in projects (`:400-407`), and replace crates.io with the SDK's `framework/vendor`
  folder (`:343-356`).
- The SDK vendors only the graph of its qualification worker (`worker_manifest`,
  `desktop/scripts/assemble-phase-zero-sdk.py:219-223, 365`), and copies the framework `CRATES`
  (`:20, 352-353`) into one framework workspace (`write_framework`, `:206-212`).
- Without a patch, a vendored kit would pull a second, crates.io copy of `fframes`, whose types
  do not match the project's SDK `fframes`.
- `fframes-studio-kit` (this repository) is its own workspace with a git dependency on the kit
  and `[patch.crates-io] fframes = { path = "../fframes" }` (`fframes-studio-kit/Cargo.toml`).
- `desktop/fixtures/kit-props` uses path and git dependencies and is built as a plain Cargo
  project (`desktop/app/tests/kit_props_roundtrip.rs`).

## Requirements

- **fframes-studio-kit**
  - Depends on `fframes-kit = { version = "=0.1.0", features = ["studio"] }` from crates.io.
  - Becomes a member of the root workspace, versioned with `fframes-studio-runtime` (0.2.0), so
    Studio binds it to the SDK with the runtime version like the other Studio crates. The root
    `Cargo.toml` gains `[patch.crates-io] fframes = { path = "fframes" }` so the kit uses this
    repository's `fframes`.
- **SDK assembly** (`assemble-phase-zero-sdk.py`)
  - `CRATES` gains `fframes-studio-kit`; the framework workspace gets the same
    `[patch.crates-io] fframes = { path = "fframes" }`.
  - The qualification worker depends on `fframes-kit` with all features except `skia`, and on
    `fframes-studio-kit`, with the same patch, so `cargo vendor` includes the kit and every
    optional dependency a project may enable (Typst, rapier, chematic, ...). Its qualification
    run renders one kit component, proving the kit builds offline from the archive.
  - The SDK manifest gains `kit_version` (`#[serde(default)]` in
    `desktop/crates/studio-sdk/src/manifest.rs`, so older manifests still load).
  - Record the SDK size before and after; if the kit's optional dependencies add more than
    150 MB, vendor `latex` separately and document it.
- **Materialization** (`build_materialization.rs`)
  - Bind `fframes-studio-kit` like `fframes-studio-runtime` (exact runtime version).
  - Check `fframes-kit`'s requirement against `kit_version` and report "This SDK includes
    fframes-kit X; set `fframes-kit = "=X"`" instead of a Cargo resolution error. Git or path
    dependencies on the kit get the same diagnostic with the crates.io line to use.
  - Insert `[patch.crates-io] fframes = { path = <SDK framework fframes> }` into the copied root
    manifest (the staging copy, never the user's file), so crates.io crates that depend on
    `fframes` use the SDK's.
- **Fixture:** `desktop/fixtures/kit-props` switches to the crates.io kit and becomes a managed
  project (`studio.json`), so tests build it with the SDK.
- **Docs:** `docs/desktop/README.md` and the template `AGENTS.md` say how to add the kit
  (`fframes-kit = "=X"`, `fframes-studio-kit = "=0.2.0"`, worker hooks).

## Files

- Modify: `Cargo.toml` (root: member, patch), `fframes-studio-kit/Cargo.toml`,
  `desktop/scripts/assemble-phase-zero-sdk.py`, `desktop/crates/studio-sdk/src/manifest.rs`,
  `desktop/crates/studio-engine/src/build_materialization.rs`,
  `desktop/fixtures/kit-props/{Cargo.toml,studio.json}`, `desktop/app/tests/kit_props_roundtrip.rs`,
  `docs/desktop/README.md`, `desktop/crates/studio-project/templates/AGENTS.md`.

## Tasks & Steps

1. `fframes-studio-kit` on the crates.io kit inside the root workspace; its tests pass.
2. SDK assembly: crate list, patch, worker dependencies, `kit_version`, kit qualification frame.
3. Materialization: bind, version diagnostic, staging patch.
4. Fixture and roundtrip test through the managed SDK.
5. Docs.

## Verification

- Unit tests in `build_materialization.rs`: a manifest with `fframes-kit = "=0.1.0"` binds and
  gets the staging patch; a wrong kit version and a git kit dependency give the diagnostic; the
  user's `Cargo.toml` bytes are unchanged; a user `[patch]` is still refused.
- `cargo tree` in a materialized kit project shows exactly one `fframes` (the SDK path).
- SDK build on Windows (`build-phase-zero-sdk.ps1`) completes its offline qualification with
  the kit frame; the ignored managed-worker tests pass with `SDK_BUNDLE` set.
- `kit_props_roundtrip` passes through the managed SDK.
- `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-sdk -p studio-engine` and
  `cargo test -p fframes-studio-kit`.

## Success criteria

- [ ] A new Studio project that adds `fframes-kit` builds offline and renders kit components.
- [ ] The Props panel edits kit components in a managed project.
- [ ] Projects without the kit build exactly as before.

## Risks

- **Kit version drift:** each SDK ships one kit version; kit upgrades go with SDK releases
  (the existing "Use installed SDK" flow explains a mismatch).
- **SDK size** from the kit's optional dependencies; measured in step 2.
