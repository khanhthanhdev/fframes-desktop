---
phase: 2
title: "Stage 2: Portable project lifecycle and SDK binding"
status: todo
priority: P2
effort: "3-4 engineer-days (tentative)"
dependencies: [1]
---

# M1 Stage 2: Portable project lifecycle and SDK binding

## Outcome and evidence

Create/open/import a normal Rust crate with copied assets and managed SDK binding that survives relocation. The existing annotated generator writes absolute framework dependencies (`desktop/crates/studio-sdk/src/project.rs:64`) and vendor configuration (`:78`); CPU SDK generation also embeds paths (`:154`, `:187`). The current app build fixes target under source and assumes `studio-annotated-video` (`desktop/app/src/worker_project.rs:18`, `:25`). These are spike paths to preserve, not the portable product format.

## Dependencies and file ownership

Stage 1 contracts/tests pass first. One Stage 2 executor owns these changes; Stage 3 starts after lifecycle/materialization invariants pass. All paths are under `/root/fframes-desktop/`.

| Action | File | Responsibility |
|---|---|---|
| Create | `desktop/crates/studio-project/src/lifecycle.rs`, `src/assets.rs`, `tests/lifecycle.rs`, `tests/assets.rs` | Create/open/import and atomic copied assets |
| Create | `desktop/crates/studio-project/templates/{Cargo.toml,AGENTS.md,src/lib.rs,src/main.rs,src/bin/studio_worker.rs,.gitignore}` | Normal Rust scaffold/worker/instruction version |
| Create | `desktop/crates/studio-engine/src/build_materialization.rs`, `tests/materialization.rs` | App-local SDK-resolved immutable build tree |
| Modify | New crates' `src/lib.rs` and manifests; `desktop/Cargo.toml`, `desktop/Cargo.lock` | Exports/dependencies, typed Cargo parsing if needed |
| Modify | `desktop/crates/studio-sdk/src/{project.rs,environment.rs,lib.rs}` | Add SDK-bound materialized build APIs; preserve fixture/setup API |
| Modify | `desktop/app/src/worker_project.rs` | Add explicit materialized target/package/worker launch integration |
| Create | `desktop/app/tests/portable_managed_worker.rs` | Real SDK-backed portable project smoke |

## Requirements

- [x] Generated project contains ordinary Rust library, normal CLI, worker binary, media/licenses, versioned AGENTS.md and schema-versioned studio.json.
- [x] SDK/config/target/drafts never leak into portable source; no registry publication is required to use the bundled development SDK.
- [x] Import preserves dirty files, Cargo configuration and Git history; unsupported dependencies are described rather than rewritten.
- [x] Copied media remains after original asset deletion and project relocation.

## Numbered implementation steps

1. Implement read-only open: locate `studio.json` and Cargo entry, validate required Rust/media files and expose SDK availability separately. Projects remain navigable when SDK is missing/incompatible; show install/select-compatible-SDK action. Do not run Cargo or build scripts merely to open metadata.
2. Scaffold in a unique sibling staging directory. Refuse nonempty destinations, include bundled font and notice, write stable metadata/instructions and fsync before promotion. Recheck destination immediately before rename; cleanup only the owned staging directory on failure. Avoid destructive replacement of an existing folder.
3. Use stable exact version declarations matching the selected compatibility manifest for fframes/runtime/protocol in portable Cargo.toml. A lockfile is portable, with registry identity where applicable. Bundled source binding is app-local materialization: copy the immutable project inventory preserving directory structure, write SDK path dependency substitutions only into the materialized Cargo manifest and vendor config only into that tree, and isolate the resolved materialized lockfile. Never promise those development versions are already published. Explain direct standalone Cargo build prerequisites if versions are unavailable externally; Studio build uses the SDK without publication.
4. Materialize under app storage keyed by project ID, source revision, SDK hash and target. Leave SDK untouched; pass external `CARGO_TARGET_DIR` through existing SDK environment (`desktop/crates/studio-sdk/src/environment.rs:36`). Build from staging project/workspace root so embedded media paths resolve: the macro canonicalizes its supplied path against process cwd (`media-dir-macro/src/parser.rs:23`). Generated standalone projects use `media`; imported workspace projects preserve the selected workspace-relative structure and paths.
5. For imports without studio.json, inspect Cargo TOML without executing it; let user select an existing package/entry when ambiguous. Add only agreed Studio sidecar metadata/instructions, preserving existing AGENTS.md (merge explicit section or keep separate guidance reference). Preserve dirty/untracked files and Git objects; take a baseline of current working bytes, not HEAD. Never reset, stash, checkout or rewrite the user's dependency/config files. Existing projects lacking a worker bridge open with an explicit compatibility status; adding a bridge is an intentional additive import action, never hidden source rewrite.
6. Support managed standalone/workspace imports whose selected package paths remain contained. Identify external path dependencies, inherited Cargo configuration or symlinks that prevent portable materialization and report exact offending paths plus copy/relink guidance; do not claim portability until resolved. Do not silently discard imported Cargo patches/source replacement. SDK build configuration conflicts return an actionable compatibility error.
7. Copy each selected asset into project media via a same-directory temp file, streamed bytes/hash, fsync and rename; update the manifest only after successful copy. Validate containment and no-follow policy again before commit. Resolve name collisions with deterministic unique names or an explicit replace choice, never silent overwrite. On manifest commit failure keep prior manifest valid and clean only owned unreferenced temporary output; report permission/disk errors.
8. Add explicit worker target/build location parameters while keeping fixture wrappers. Existing generator callers are SDK install (`desktop/crates/studio-sdk/src/install.rs:211`) and app wrapper (`desktop/app/src/worker_project.rs:9`). App worker-launch callers are `desktop/app/src/app.rs:164`, `desktop/app/src/agent_spike.rs:141`, `:195`, `desktop/app/tests/managed_worker.rs:31`; fixture build is called by launch (`desktop/app/src/worker_project.rs:43`). Preserve all legacy command/harness input contracts and test them.
9. Exercise create → asset copy → close → rename folder → reopen with original source asset removed and SDK relocated/reselected. Compare portable bytes before/after SDK build; only intentional source/manifest/portable lock changes may affect source identity, never materialized lock/config/targets.

## Data flow and compatibility

Create/import request → validated metadata/tree → portable files. Asset source stream → validated temp copy → project-relative asset/manifest entry. Portable snapshot + SDK manifest → app-local build materialization + target/config → existing process supervisor/Cargo → tagged build result. Original project and SDK stay unchanged during builds. Legacy spike scaffolding remains available for M0 qualification; no wire version change.

## Verification and measurable gate

```sh
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project --test lifecycle
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project --test assets
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine --test materialization
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-sdk
```

After assembling a real SDK at `/tmp/studio-sdk` using the documented Phase 0 procedure:

```sh
SDK_BUNDLE=/tmp/studio-sdk cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test portable_managed_worker -- --ignored
SDK_BUNDLE=/tmp/studio-sdk cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test managed_worker -- --ignored
```

Planned integration target is ignored only for explicit SDK prerequisite, fails if invoked with a bad bundle. Matrix: nonempty destination race; missing/invalid/newer manifest; missing assets; dirty imported Git/untracked source; duplicate asset names; interrupted copy; Unicode/spaces; contained workspace embedded media; external dependency error; SDK missing/incompatible/relocated; portable files unchanged after real CPU worker build/render. Gate: relocation opens with identical copied asset hashes and real worker builds via managed SDK; original dirty import bytes/Git history remain identical.

## Risks and rollback

High (high likelihood × high impact): Cargo overlay resolves a different dependency graph or wrong media root. Mitigate explicit SDK version checks, materialized manifest/lock separation, offline real compile/render and cwd regression fixture. High (medium × high): import/copy overwrites user source. Mitigate nonempty checks, hash checks and mutation tests. Medium: unsupported external dependencies prevent managed build; keep open/navigation usable and report limitation.

Rollback new product generator/materializer while retaining spike wrappers. Remove only owned failed staging/build trees; keep portable projects/assets. Do not restore source from Git or erase imported metadata without preserving a backup.

## Execution checklist

- [x] Create/open/import and copied asset behavior complete.
- [x] Portable dependency declarations and app-local SDK materialization complete.
- [x] Scoped unit/integration tests and real SDK relocation flow pass.
- [x] Stage 3 receives final identity and app-storage layout contracts.

## Execution evidence — 2026-10-02

Lifecycle/assets/materialization/SDK suites passed. Import preserves a real temporary Git commit, its dirty diff, untracked bytes and existing instructions. Asset tests cover deterministic collisions, source/manifest/directory mutation before publication and cancellation between chunks. Failed import validates source before publishing its sidecar.

Explicitly executed `portable_managed_worker` with `SDK_BUNDLE=/tmp/studio-sdk-native-verified`: passed (70.56 seconds). It installs a real SDK, copies/removes an original asset, relocates project and SDK, builds/renders both a standalone project and a contained workspace with compile-time embedded DM Sans. Pixel bytes and generation are asserted; portable inventories remain exact after both builds. The expanded fixture needed syntax/media-type/trait-import corrections before its successful run; these were fixture failures, not waived checks.

Build copies bind captured manifests, preserve workspace-relative media cwd, reject conflicts/external dependencies and leave source/SDK unchanged. Native asset copy, folder relocation and Locate also succeeded with the original asset removed; copied asset SHA-256 was `d738528acd920052e1a0cce1da8fb387198d4725d63aa96a10aee9d815f4df88`. Stage 3 uses app-local objects/drafts/journals keyed by project identity, independent of folder location.
