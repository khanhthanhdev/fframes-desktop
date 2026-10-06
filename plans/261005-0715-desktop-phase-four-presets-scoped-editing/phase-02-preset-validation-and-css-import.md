---
phase: 2
title: "Preset validation and CSS import"
status: completed
priority: P2
effort: "3-4 engineer-days"
dependencies: [1]
---

# Phase 2: Preset validation and CSS import

## Goal

Safely import/export usable directory presets, provide a truthful partial CSS custom-property importer, and ship three original presets whose resources and licensing are verified.

## Context and key insights

- Phase 1 supplies schema v1, typed tokens, resolution and canonical hashes. The [architecture](../../docs/desktop/architecture.md#L298) permits a portable directory or archive; this plan deliberately chooses the simpler directory form for M4 and does not add an archive dependency.
- Existing project assets use `studio_project::ProjectPath` and no-follow opens. `studio-project` inventories style files and rejects links; preset import must preserve the same path/containment policy rather than extracting arbitrary paths.
- CSS is only an import format. It must not become a browser cascade engine or imply that selectors/component styles were interpreted.

## Requirements

- [x] Import/export a versioned directory with `preset.json`, canonical token data, guide, declared assets/fonts/examples and license notices. Verify resource hashes and all `ProjectPath`s before publishing an imported preset.
- [x] Bound input bytes, file count, nesting, names and resource sizes. Reject symlinks, special files, path traversal, duplicate paths, hash mismatches and unsupported schema versions without partial output.
- [x] Validate that every declared font family maps to a bundled font file with explicit license metadata and that reusable media exists, is a supported type and stays within bounds. Missing resources are actionable failures, not silent system-font substitutions.
- [x] Emit a CSS import report with line/variable, mapping, normalized result and accepted/unsupported/rejected status. Parse only documented `:root` custom-property declarations mapped to canonical token names; report ordinary selectors/properties, functions, `!important`, unsupported units and unmapped variables.
- [x] Support an explicit subset: hex color literals, quoted family strings, bounded numeric/px/rem dimensions, unitless weights, and duration values with declared units. Require design dimensions/base font size for conversions; never approximate `calc()`, `var()`, URL, gradients or cascade behavior.
- [x] Add three original example presets with distinct typography, layout and motion. Include license notices for every bundled font/image/example; prefer existing bundled fonts when they satisfy the designs and add third-party resources only with recorded compatible licenses.

## Architecture

`studio-presets` owns directory inspection/import/export and the CSS parser/report. Validate the complete source tree before writing to a unique sibling staging directory, copy only regular files through bounded streams, verify hashes after copy, sync, then publish the staged directory without replacing an existing preset. Export uses the same schema/path/hash checks. No user project files are written yet.

The CSS importer accepts either a `:root { ... }` block or a raw declarations block under an explicit mapping table from CSS variable to canonical token. It does not evaluate selectors, inheritance, custom-property references or CSS functions. Preserve each rejected declaration and reason in the report so the caller can correct the source.

## Files to create or modify

All paths are rooted at `/root/fframes-desktop/`.

| Action | Path | Responsibility |
|---|---|---|
| Create | `desktop/crates/studio-presets/src/package.rs` | Bounded no-follow directory import/export, resource validation and staged publication. |
| Create | `desktop/crates/studio-presets/src/css_import.rs` | Allowlisted CSS token parser and line-oriented import report. |
| Create | `desktop/crates/studio-presets/src/builtin.rs` | Compile-time registry for the bundled preset directory files. |
| Create | `desktop/crates/studio-presets/tests/package.rs`, `tests/css_import.rs` | Containment, licensing/resource, limit and parser matrix. |
| Modify | `desktop/crates/studio-presets/src/lib.rs`, `desktop/crates/studio-presets/Cargo.toml`, `desktop/Cargo.lock` | Export stable APIs and use only dependencies required by the directory contract, including the existing locked `ttf-parser` version for bundled font-family verification. |
| Create | `desktop/crates/studio-presets/builtins/editorial/`, `builtins/pulse/`, `builtins/quiet-motion/` | Three original versioned bundles, guides, examples and license files embedded into the app. |
| Create | `desktop/crates/studio-presets/builtins/README.md` | Example provenance, font/image license inventory and regeneration notes. |

## Tasks & steps

### Task 2.1 — Build bounded directory import/export

- **Goal:** Import and export schema-v1 preset directories without following links, escaping the selected root, partially publishing, or replacing an existing preset.
- **Target files and symbols:** New `desktop/crates/studio-presets/src/package.rs`; `desktop/crates/studio-presets/src/lib.rs`; new `desktop/crates/studio-presets/tests/package.rs`.
- **Steps:** Validate every relative path and regular-file type before copying; enforce byte/file/depth/name/resource limits; stage beside the destination; verify the completed package; publish without replacement; on failure remove only the unique importer-owned staging directory.
- **Success criteria:** A valid directory round-trips; traversal, symlink, special-file, duplicate-path, limit and pre-existing-destination cases fail without modifying the store.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets --test package` exits 0 and prints `test result: ok`.

### Task 2.2 — Verify resources, fonts, media and licenses

- **Goal:** Establish that every declared resource exists, is supported, bounded, hash-matched and accompanied by the license notice that will travel with it.
- **Target files and symbols:** `desktop/crates/studio-presets/src/package.rs`; `desktop/crates/studio-presets/src/model.rs`; `desktop/crates/studio-presets/Cargo.toml`; `desktop/crates/studio-presets/tests/package.rs`.
- **Steps:** Verify canonical `ProjectPath`s and hashes; use the already lockfile-resolved `ttf-parser` 0.25.1 dependency to inspect bundled font names and match each declared family to its font file; validate supported media signatures; reject missing or mismatched font/resource declarations; include license files/notices in canonical package hashing; add corrupt, unsupported, absent and oversized resource fixtures.
- **Success criteria:** Every invalid resource case has an explicit diagnostic and publishes no package; valid resource digests and license notices participate in the package hash.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets --test package` exits 0 and prints `test result: ok`.

### Task 2.3 — Implement the explicitly limited CSS importer

- **Goal:** Convert only the documented custom-property subset and produce a per-declaration report for all accepted, unsupported and rejected values.
- **Target files and symbols:** New `desktop/crates/studio-presets/src/css_import.rs`; `desktop/crates/studio-presets/src/lib.rs`; new `desktop/crates/studio-presets/tests/css_import.rs`.
- **Steps:** Parse only raw declarations or a `:root` block; map only documented variable names; normalize supported literals/units using explicit design context; preserve source line, variable, mapping, normalized value/status and reason; add positive and negative fixtures for selectors, functions, `!important`, unknown variables and units.
- **Success criteria:** Every input declaration appears in the report; only allowlisted forms yield tokens; no cascade/function/selector is evaluated or silently discarded.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets --test css_import` exits 0 and prints `test result: ok`.

### Task 2.4 — Add and audit the three bundled presets

- **Goal:** Ship three offline-validating, original preset bundles with distinct supported typography/layout/motion examples and complete resource notices.
- **Target files and symbols:** `desktop/crates/studio-presets/src/builtin.rs`; `desktop/crates/studio-presets/builtins/editorial/`; `desktop/crates/studio-presets/builtins/pulse/`; `desktop/crates/studio-presets/builtins/quiet-motion/`; `desktop/crates/studio-presets/builtins/README.md`.
- **Steps:** Bind all example styles to semantic tokens; keep resources inside each bundle; record provenance/license notices and hashes; register the bundles for read-only built-in access.
- **Success criteria:** All three bundles validate offline; their normalized typography, layout and motion token sets differ; every included resource has a matching license notice.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets` exits 0 and prints `test result: ok`; the built-in registry test reports exactly three validated bundles.

### Task 2.5 — Close import/export and CSS stage gate

- **Goal:** Prove deterministic package round trips and complete rejection/report behavior before project mutation is introduced.
- **Target files and symbols:** `desktop/crates/studio-presets/tests/package.rs`; `desktop/crates/studio-presets/tests/css_import.rs`; `desktop/crates/studio-presets/builtins/README.md`.
- **Steps:** Run package, CSS and built-in suites; compare canonical hashes across import/export; verify invalid imports leave a pre-existing store snapshot byte-identical; run desktop formatting.
- **Success criteria:** All checks pass; no project source files are modified by this phase; every unsupported CSS declaration has a report entry.
- **Verify:** `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets` exits 0 and prints `test result: ok`; `cargo fmt --manifest-path desktop/Cargo.toml --all --check` exits 0.

## Verification

```sh
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-presets
cargo test --locked --manifest-path desktop/Cargo.toml -p studio-project --test assets
cargo fmt --manifest-path desktop/Cargo.toml --all --check
```

Gate: all three bundles validate offline; a clean round trip preserves hashes and licenses; malformed/oversized/linked/traversal packages publish nothing; CSS supported values normalize exactly and every unsupported declaration is explained in the report.

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
| CSS subset appears broader than it is | Show the per-declaration report, keep the allowlist explicit and reject unsupported constructs. |
| Font family resolves differently on another machine | Bundle and verify the declared font, retain its license, and report missing/corrupt files instead of claiming reproducibility. |
| Third-party preset content has incompatible terms | Prefer original vector/reference assets; block adding any font/image without a checked license notice. |
| Interrupted import leaves partial package | Stage beside destination, sync and publish only after complete verification; clean only owned staging output. |

Rollback removes the new import/export route and example registrations. It does not modify project files, global fonts or existing application preferences.

## Todo list

- [x] Implement safe directory package import/export and resource validation.
- [x] Implement the bounded CSS importer and complete import report.
- [x] Add three original, licensed example presets and pass offline round-trip tests.

## Success criteria

Preset packages are portable and offline-validatable, CSS import never pretends to understand full CSS, all three examples have materially distinct supported styling, and no failed import leaves a published partial bundle.
