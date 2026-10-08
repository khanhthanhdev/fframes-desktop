---
title: "Release contracts and native package baseline"
status: in-progress
priority: P1
effort: "5–8 engineer-days"
---

# Stage 1: Release contracts and native package baseline

## Context and outcome

Read [plan](plan.md), [baseline](../reports/planning-261007-2054-m7-baseline-and-validation.md), [installation architecture](../../docs/desktop/architecture.md#10-installation-builds-and-updates), desktop CI and packaging scripts. Establish versioned release contracts and native runtime layouts used by stages 2–5. This is an internal artifact gate.

## Verified starting point

CompatibilityManifest records targets, toolchain, prerequisites and preview capabilities, but compatible_app_range is not enforced. Phase-zero packaging verifies SDK hashes/helper adjacency and emits ZIPs/minimal macOS bundles; launch scripts pass spike-ui. The release workflow publishes these, including an existing-run route. A separate unsigned Linux x64 .deb candidate builder now packages the product-shell binary/helpers, third-party notices, desktop entry and a launcher under /opt, derives direct shared-library dependencies with dpkg-shlibdeps, and excludes the separately delivered SDK. Its extracted clean-HOME Xvfb launch smoke passes; this remains internal package evidence and does not replace apt install/update/uninstall or clean-account qualification. Assembler provenance leaves clean-account qualification pending.

## Requirements and design

- Bind release evidence to exact source commit, target, version, SDK digest, protocols, runtime inventory, minimum OS and artifact bytes.
- Add a release schema and signed envelope with versioned canonical payload bytes, immutable artifact URLs/hashes/sizes, app↔SDK ranges, export capabilities, sequence/version and key ID. OS signatures do not replace metadata authentication. Pin verification keys in the app; reject unknown keys and downgrade/replay except explicit recovery to a locally verified previous pair.
- Preserve legacy manifest/worker reading. Negotiate export separately; old SDKs support their existing path without falsely promising native export. Parse and enforce app compatibility before setup/build/update.
- Package Apple Silicon .app/DMG, per-user Windows installer, Linux x64 AppImage and deb. Native tests determine OS/distro baselines. Keep app installer small; SDK downloads and offline bundles are separate.
- Inventory actual libraries, fonts, adapter runtimes and codec/source obligations. Windows currently fetches moving GPL-shared FFmpeg while Unix uses pinned static configuration. Use immutable verified inputs and prove a default MP4 video/audio encoder through a real render.

## File ownership

All paths are relative to the repository root. Stage 1 owns these files; transfer shared ownership sequentially. No deletions.

| Action | Files | Purpose |
|---|---|---|
| Modify | desktop/crates/studio-sdk/src/manifest.rs, lib.rs; desktop/packaging/sdk/compatibility.schema.json | Enforce compatibility; preserve old manifests |
| Create | desktop/crates/studio-sdk/src/release-manifest.rs; desktop/packaging/release-manifest.schema.json; release-policy.json | Signed metadata and target/codec policy |
| Modify | desktop/scripts/assemble-phase-zero-sdk.py, build-native-ffmpeg.py, package-phase-zero.py, test-packaging.py | Reuse assembly/layout; retain honest spike mode |
| Create | desktop/scripts/package-release.py | Consumer layout using existing helpers |
| Create | desktop/scripts/package-linux-deb.py | Unsigned Linux x64 Debian package candidate; signing and installed package-manager qualification stay separate |
| Modify | .github/workflows/desktop.yml, desktop-phase-zero.yml, desktop-release.yml | Native candidates and fail-closed publication |
| Create | desktop/qualification/m7-results.schema.json, m7-results.json; desktop/scripts/test-qualification-m7.py | Explicit automated/authentic/physical/release gates |
| Modify | desktop/scripts/validate-qualification.py; docs/desktop/architecture.md | Owning validation and architecture |

## Implementation steps

1. Preserve dirty M6 work; recheck handoff/profile/UI checks and latest evidence hashes. Record commit plus worktree digest. Do not use historical plan status as implementation evidence.
2. Add meaningful manifest negative tests: malformed ranges, wrong target/app/SDK/protocol, missing codecs, unknown signer, replay and legacy reading. Enumerate manifest consumers in setup/shell, materialization, SDK CLI, assembler, schema and tests before changing fields.
3. Add consumer packaging over existing helpers. Resolve resources from executable location, preserve adjacent studio-tools/studio-mcp, launch studio consistently, use accurate versions/identifiers/icons and per-user writable paths.
4. Build all native targets early. Capture loader dependencies, artifact size/hashes, compiler/linker/OS SDK provenance and generated notices. Prove tiny MP4 video/audio encode and decode, not just an encoder name.
5. Add hash-bound M7 evidence validation that cannot satisfy authentic/physical gates with mock agents or Xvfb. Missing access produces blocked/not_run.
6. Gate both fresh and reused-run publication on matching candidate commit/tag/artifact identity and complete qualification. Ordinary core release tags must not accidentally publish unqualified Studio packages. Keep internal artifacts available and prohibit conflicting replacement of released versions.

## Checklist and success criteria

- [x] Compatibility/trust/evidence schemas pass legacy and negative tests.
- [x] Native matrix yields inventoried product-shell layouts or explicit target blockers.
- [ ] Fonts, runtime libraries and helpers load outside checkout/build directories.
- [ ] Actual bundled codec/configuration and source/license obligations are resolved before publication.
- [x] Neither tag nor reused-artifact route bypasses qualification or overwrites released bytes.

## Validation

First run `python3 desktop/scripts/test-packaging.py` and `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-sdk`; then the new `python3 desktop/scripts/test-qualification-m7.py`, M6 evidence contract checks and root protocol tests. Native CI builds/renders the small annotated fixture and inspects dependencies on every target.

## Risk, security and rollback

Metadata authorizes executable download/native compilation. Protect signing keys in CI secrets, never evidence/projects. Unresolved codec redistribution, linker or OS SDK availability blocks that target's release; record guided prerequisites instead of claiming a fully managed toolchain. Keep development packaging and prior artifacts available.

Recheck current requirements at execution: [Apple notarization](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow), [Microsoft signing](https://learn.microsoft.com/en-us/windows/win32/appxpkg/how-to-sign-a-package-using-signtool), [FFmpeg distribution considerations](https://www.ffmpeg.org/legal.html). These sources guide checks; they do not establish this project's qualification.

## Next step

Before transferring SDK/setup ownership to stage 2, resolve or explicitly exclude HEVC's undeclared host `libx265.so.199` dependency, finish source/license review, and verify runtime/font/helper behavior from installed locations. The current Linux smoke proves an HEVC/AAC encode/decode only on a host that supplies `libx265.so.199`; it does not pass those gates.
