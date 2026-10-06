---
phase: 1
title: "Preset schema and typed resolution"
status: completed
priority: P2
effort: "3-4 engineer-days"
dependencies: []
---

# Phase 1: Preset schema and typed resolution

## Goal

Define a GPUI-free, versioned preset/token contract and deterministic resolver that can validate and snapshot video styles without changing renderer or project behavior yet.

## Context and key insights

- The [M4 roadmap](../../docs/desktop/implementation-plan.md#L105) requires typed tokens, aliases, overrides and snapshots. [Architecture sections 3 and 7](../../docs/desktop/architecture.md#L101) define portable `style/` files, precedence and the allowed token families.
- `studio.json` v1 already has an optional `PresetReference { id, sha256 }` in `desktop/crates/studio-project/src/manifest.rs`; do not bump its version just to add the preset engine. `studio-project` owns project paths/source inventory, while the new package owns preset semantics.
- `desktop/crates/studio-engine` owns controller/task state and `fframes` is a separate root workspace. Keep this phase's schema independent of GPUI, filesystem mutation and renderer API. Project persistence and runtime access are Phase 3.

## Requirements

- [x] Add `studio-presets` as a GPUI-free desktop-workspace crate with explicit schema and token-schema versions. Reject unsupported newer versions before interpreting fields.
- [x] Model canonical semantic tokens for colors, typography, spacing, radii, stroke widths, supported shadows and motion (durations, easing/spring parameters). Values carry explicit types/units; reject non-finite numbers and unsupported units.
- [x] Support literal values and aliases. Resolve aliases deterministically; reject missing references, cycles, cross-type aliases and duplicate canonical names with field-specific diagnostics.
- [x] Resolve precedence as preset defaults → project overrides → optional scene/element overrides, without requiring M5 selection APIs. Preserve unknown values only where the declared schema explicitly permits extensions; never silently coerce types.
- [x] Define a stable canonical serialization/hash over normalized metadata, token values and referenced resource digests; equivalent inputs produce the same hash, while any material resource/token change changes it.
- [x] Keep all identifiers, keys, nesting, token count, strings and serialized bytes bounded. Preset data is untrusted input.

## Architecture

Create a `studio-presets` model/resolution boundary that consumes validated plain data and returns resolved typed values plus structured diagnostics. Reuse `studio_project::ProjectPath` for portable resource paths, but do not make `studio-project` depend on preset code. A preset directory is the canonical exchange form; archive extraction is not introduced in this phase. Resolution is pure: the caller supplies a preset, override layer and optional scene layer, and receives an immutable resolved snapshot/hash.

Token names are semantic paths such as `color.accent` and `typography.title`; literal values are tagged by kind. `px` and `rem` are converted only with explicit design dimensions/base typography. CSS-specific parsing and filesystem verification are deferred to Phase 2. Define shadow/easing variants only where fframes can represent them; unsupported renderer features yield an import/validation diagnostic instead of a fake rendering guarantee.

## Files to create or modify

All paths are rooted at `/root/fframes-desktop/`.

| Action | Path | Responsibility |
|---|---|---|
| Create | `desktop/crates/studio-presets/Cargo.toml` | GPUI-free package with minimal serde/hash dependencies and the one-way `studio-project` dependency for `ProjectPath`. |
| Create | `desktop/crates/studio-presets/src/lib.rs` | Public versioned preset/resolution surface. |
| Create | `desktop/crates/studio-presets/src/model.rs` | Manifest, typed token values, aliases, resources and diagnostics. |
| Create | `desktop/crates/studio-presets/src/resolve.rs` | Bounds/type checks, alias graph and precedence resolver. |
| Create | `desktop/crates/studio-presets/tests/schema.rs`, `tests/resolve.rs` | Parse, bounds, alias and deterministic-resolution matrix. |
| Modify | `desktop/Cargo.toml`, `desktop/Cargo.lock` | Register the new package and lock its dependencies. |

## Tasks & steps

### Task 1.1 — Establish the M0–M3 regression baseline

- **Goal:** Confirm the existing project, engine, app workflow, protocol/runtime and timeline test suites pass before adding the preset contract.
- **Target files and symbols:** No source edits. Existing targets: `desktop/crates/studio-project`, `desktop/crates/studio-engine`, `desktop/app/tests/project_foundation.rs`, `desktop/app/tests/agent_workflow.rs`, `desktop/app/tests/timeline_controls.rs`, plus packages `fframes-studio-runtime` and `fframes-studio-protocol`.
- **Steps:** Run each command below and record its exit status before changing code.
- **Success criteria:** Every command exits 0; no baseline test is deleted, skipped, or reclassified.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project -p studio-engine` exits 0 and prints `test result: ok`; `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test project_foundation --test agent_workflow --test timeline_controls` exits 0 and prints `test result: ok`; `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio-runtime -p fframes-studio-protocol` exits 0 and prints `test result: ok`.

### Task 1.2 — Define the versioned preset and token schema

- **Goal:** Parse bounded schema-v1 presets into typed values and reject unsupported versions or unrecognized trust-boundary fields with structured diagnostics.
- **Target files and symbols:** `desktop/crates/studio-presets/Cargo.toml`; `desktop/crates/studio-presets/src/lib.rs`; new `desktop/crates/studio-presets/src/model.rs`; `desktop/Cargo.toml` and `desktop/Cargo.lock`.
- **Steps:** Register the GPUI-free crate and depend one-way on `studio-project` for its existing `ProjectPath` type; define versioned metadata, resource declarations, typed token variants, bounded parse entry point and error types; use `deny_unknown_fields` at external trust boundaries; add unit tests for accepted v1 and rejected future/malformed versions.
- **Success criteria:** Schema tests establish the exact serialized v1 shape; malformed and future versions return typed errors without panic; the crate dependency tree does not include GPUI or a renderer.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets` and `cargo tree --locked --manifest-path desktop/Cargo.toml -p studio-presets` each exit 0 and print the package results/tree; `test -z "$(cargo tree --locked --manifest-path desktop/Cargo.toml -p studio-presets | grep -Ei 'gpui|renderer' || true)"` exits 0, proving neither dependency appears.

### Task 1.3 — Implement deterministic token resolution and hashing

- **Goal:** Resolve literal/alias tokens and preset → project → scene/element precedence into an immutable typed snapshot with a stable hash.
- **Target files and symbols:** `desktop/crates/studio-presets/src/resolve.rs`; `desktop/crates/studio-presets/src/model.rs`; new `desktop/crates/studio-presets/tests/resolve.rs`.
- **Steps:** Validate names/types/units and finite numeric values; detect duplicate names, missing references, cycles and cross-type aliases; normalize canonical units and numeric serialization; apply precedence without coercion; hash canonical metadata/tokens/resources.
- **Success criteria:** Equal normalized inputs produce identical hashes; changing any material token/resource changes the hash; every invalid alias/type/unit case returns a field-specific diagnostic.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets` exits 0 and prints `test result: ok`.

### Task 1.4 — Close the schema stage gate

- **Goal:** Prove the new contract passes its validation matrix and does not regress desktop consumers.
- **Target files and symbols:** `desktop/crates/studio-presets/tests/schema.rs`, `desktop/crates/studio-presets/tests/resolve.rs`; `desktop/Cargo.toml`, `desktop/Cargo.lock`.
- **Steps:** Complete table/property cases for all token kinds, bounds, units, aliases, deterministic hashes and precedence; run the package and baseline compatibility commands from Task 1.1; format the desktop workspace.
- **Success criteria:** All new and baseline tests pass, formatting is clean, and import/export/project mutation remains absent from this phase.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets -p studio-project -p studio-engine -p fframes-studio` and `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio-runtime -p fframes-studio-protocol` each exit 0 and print `test result: ok`; `cargo fmt --manifest-path desktop/Cargo.toml --all --check` exits 0.

## Verification

```sh
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project -p studio-engine
cargo fmt --manifest-path desktop/Cargo.toml --all --check
```

Gate: invalid or unsupported input returns structured errors without panic; aliases resolve to the expected typed values; default/override/scene precedence is deterministic; hashes are stable and resource-sensitive; the new crate has no GPUI or renderer dependency.

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

## Risks and rollback

| Risk | Response |
|---|---|
| Token units or numeric normalization are ambiguous | Keep canonical units explicit, require declared conversion context, and reject ambiguous values rather than guessing. |
| Preset schema becomes coupled to the desktop project format | Keep preset versioning in `studio-presets`; the existing project reference remains an opaque id/hash. |
| Token type set claims unsupported renderer behavior | Validate against current fframes values; mark unsupported shadow/motion forms instead of emitting approximate values. |

Rollback removes only the new crate registration and package files. Existing `studio.json`, source inventory and M0–M3 task/preview contracts remain unchanged.

## Todo list

- [x] Implement and test schema v1 and token types.
- [x] Implement pure override/alias resolution and canonical hashing.
- [x] Pass the package and baseline gates before transferring the schema to Phase 2.

## Success criteria

One small, versioned contract represents every M4 token family needed by the selected examples, rejects malformed inputs precisely, and can be consumed by import, project-snapshot and runtime layers without depending on UI or filesystem code.
