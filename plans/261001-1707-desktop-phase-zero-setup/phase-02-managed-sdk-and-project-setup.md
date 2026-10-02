---
phase: 2
title: "Build managed SDK and project setup"
status: pending
priority: P1
effort: "3-4 engineer days"
dependencies: [1]
---

# Phase 2: Build managed SDK and project setup

## Goal

Extend the minimal app into an end-to-end setup surface: diagnose host prerequisites, let the user review any administrative install, transactionally install a checksum-pinned app SDK, generate a real CPU fframes project outside the repository, compile and render it, then compile a second fresh project offline without developer-repository or home Cargo caches.

## Context and decisions

- Native Rust compilation still requires a compiler, linker and OS/native SDK pieces; installing the GPUI app cannot remove them ([architecture.md:355-365](../../docs/desktop/architecture.md#L355)). The supported M0 approach is guided host prerequisites plus app-managed artifacts ([SDK report:107-114](../reports/researcher-261001-1707-sdk-ffmpeg-phase-zero.md#L107)).
- The app manages the pinned Rust/Cargo homes, exact fframes/cargo-fframes release, vendored registry/source inputs, template/font assets, FFmpeg/Skia artifacts used by the chosen profile, compatibility data and notices. The host supplies what cannot yet be redistributed or safely isolated: OS linker/SDK and native graphics/runtime support.
- The existing generator creates a standalone workspace outside this repository and pins the fframes release matching `cargo-fframes` ([cargo-fframes main.rs:452-477](../../cargo-fframes/src/main.rs#L452)). Its generated codec dependency is guarded off on Windows and currently requests only `h264` plus `libav-agree-gpl` on non-Windows ([cargo-fframes main.rs:498-507](../../cargo-fframes/src/main.rs#L498)). Use this real CPU generator output for M0; do not use `examples/hello-world`, whose workspace feature unification and unconditional example choices are not a Windows clean-user contract.
- Do not silently replace that GPL/H.264 choice with an LGPL product profile in this phase. Record codec/license/profile qualification as a separate result. If the exact feature-key archive or obligations cannot be satisfied, managed compilation is blocked until the user/product owner accepts a generator/profile change.
- `fframes-media` links feature-matching static FFmpeg on non-Windows and shared FFmpeg on Windows ([fframes-media/Cargo.toml:34-46](../../fframes-media/Cargo.toml#L34)). Windows must have FFmpeg 9 headers, import libraries and DLLs through `FFMPEG_DIR`, `LIBCLANG_PATH`, and the child-only DLL search `PATH` at both proc-macro build and runtime ([README.md:260-277](../../README.md#L260), [main.yml:338-348](../../.github/workflows/main.yml#L338)). An `ffmpeg` CLI alone does not satisfy this.
- `ffmpeg-sys-fframes` can download binaries during its build script, but the wrapper does not checksum them. Preseed its app-local cache from the checked manifest and force the normal path to local verified inputs ([SDK report:31-35](../reports/researcher-261001-1707-sdk-ffmpeg-phase-zero.md#L31)). M0 may trust a checked-in checksum manifest; signing and update trust machinery remain M7.

## Files to create or modify

| Action | Path | Ownership and purpose |
|---|---|---|
| Modify | `desktop/Cargo.toml` and `desktop/Cargo.lock` | Add only the `studio-sdk` crate and its resolved dependencies. |
| Create | `desktop/crates/studio-sdk/Cargo.toml` | SDK/setup library; depends on `studio-bootstrap`, remains GPUI-free. |
| Create | `desktop/crates/studio-sdk/src/{lib,manifest,doctor,install,environment,project}.rs` and `src/bin/studio_setup.rs` | Manifest validation, read-only probes, staged install, child environment, generator/build/render orchestration, plus a GPUI-free prebuilt setup/doctor companion. |
| Create | `desktop/crates/studio-sdk/tests/{manifest,install_recovery,environment}.rs` | Schema/error, corrupt download, cancel/resume, rollback and environment-isolation checks. |
| Modify | `desktop/app/Cargo.toml`, `src/app.rs`, and `src/main.rs` | Add a minimal setup screen and wire background setup events to GPUI. |
| Create | `desktop/app/src/setup_view.rs` | Visible states: Checking, Prerequisites needed, Installing, SDK ready, Creating project, Building, Rendered, Failed/Cancelled. |
| Create | `desktop/packaging/sdk/compatibility.schema.json` | Single owning schema for app/SDK/target/native feature compatibility. |
| Create | `desktop/packaging/sdk/phase-zero-sdk.json` | Exact candidate and artifact records; no moving URLs, missing checksums, or placeholder values may merge. |
| Create | `desktop/packaging/sdk/notices/` | Licenses/notices and exact source/configuration references for redistributed artifacts. |
| Create | `desktop/scripts/studio-dev-setup.sh` and `studio-dev-setup.ps1` | Tiny bootstrap wrappers that checksum-resolve the native GPUI-free `studio_setup` companion, then call the same manifest/doctor implementation; they do not duplicate probes or mutate global setup. |
| Create | `desktop/scripts/build-phase-zero-sdk.sh` and `build-phase-zero-sdk.ps1` | Reproducible artifact assembly/checksum scripts for native CI; they never download an unlisted runtime input. |
| Create | `desktop/packaging/app/{linux,windows,macos}/` | Minimal per-target launchable artifact recipes containing the app binary, manifest, notices and runtime files needed for clean-account testing. |

The generated test projects and installed SDK live in temporary/clean-user app-data locations during qualification, not in the repository. No Node or pnpm dependency belongs in the SDK or setup flow unless the ACP adapter selected in Phase 3 explicitly requires a managed runtime; the GPUI UI and fframes generator are Rust binaries.

## Compatibility manifest contract

The JSON schema is the only owner of compatibility fields. It requires:

- `schema_version`, `sdk_id`, compatible app range, target triple, architecture and minimum OS/glibc baseline;
- exact Rust toolchain and component hashes; exact `cargo-fframes` and fframes versions; template revision/hash;
- exact runtime/protocol crate versions, source hashes and standalone normalized manifests when Phase 3 adds the worker; SDK-relative Cargo patch/vendor configuration and every native build-script input/patch digest;
- GPUI commit for the prebuilt app, worker protocol range, frame schema version and supported project feature set;
- immutable artifact URL or offline-bundle path, byte size and SHA-256 for every archive; extraction root and expected files;
- FFmpeg 9 tag/key, link mode, headers/libs/bin layout, codec Cargo features, ABI, `extralibs.txt` dependencies and license/notice references;
- required host probes, minimum versions, user-reviewed package/install instructions, disk-space estimate and app-local environment variables;
- platform runtime files and native capability flags. A precompiled Rust artifact is explicitly a cache for this exact compiler/target/features tuple, never a stable plugin ABI.

Reject unknown schema versions, target mismatches, duplicate paths, absolute/archive traversal paths, missing hashes, moving `latest` URLs, incompatible protocol ranges, and feature/ABI mismatches before download. Preserve the manifest with every evidence run.

## Cross-platform prerequisite matrix

| Target | App-owned, checksum-pinned | Host/admin prerequisite shown before elevation | Doctor pass condition | Runtime qualification boundary |
|---|---|---|---|---|
| Ubuntu 24.04 x64 baseline | Rust/Cargo homes; cargo-fframes/fframes/vendor inputs; exact static FFmpeg feature archive/cache; template/font/notices; app binary | Candidate list: `build-essential cmake clang libclang-dev lld llvm pkg-config ninja-build yasm nasm libfontconfig-dev libwayland-dev libx11-xcb-dev libxkbcommon-x11-dev libvulkan1 libasound2-dev libx264-dev`. Keep only packages proved by the clean build and FFmpeg `extralibs.txt`; the `ffmpeg` CLI is optional diagnostics, not linkage. | Correct x86_64/glibc baseline; compiler/linker/libclang load probe; exact libraries resolved; X11 or Wayland and compatible graphics driver/backend available | Qualifies Ubuntu 24.04 only. Test X11 and Wayland. Static FFmpeg is accepted only after `extralibs.txt` dependencies resolve. Other distros remain unadvertised. |
| Windows 11 x64, MSVC | Rust/Cargo homes; exact FFmpeg 9 x64 shared archive with headers, `.lib` and DLLs; cargo/vendor/template/notices; app `.exe` and redistributable app DLLs | Visual Studio 2022 Build Tools Desktop C++ workload, MSVC v143 x64/x86 tools, CMake/Spectre components required by the qualified GPUI revision, Windows 10/11 SDK (record exact version); VC++ Runtime for packaged binaries; LLVM/libclang 9+ if not bundled under the SDK | `cl.exe`, `link.exe`, `rc.exe`, CMake, SDK roots and runtime found from a VS developer environment; bindgen probe works; every FFmpeg import library/header matches and DLL load succeeds | `x86_64-pc-windows-msvc` only. MinGW/MSYS2 is not an alternate supported ABI. FFmpeg `bin` is prepended only in child environments at build and run. |
| macOS arm64 | Rust/Cargo homes; cargo/vendor/template/font/notices; exact static FFmpeg feature archive/cache; minimal `.app` test bundle | Full/current supported Xcode + Command Line Tools, selected developer directory, accepted license, macOS SDK/linker; CMake; Metal toolchain component when `xcrun --find metal` fails; libclang/LLVM route selected and recorded | Native `aarch64-apple-darwin`; `xcode-select`, SDK, clang/linker, Metal and libclang probes pass; FFmpeg static/external libs resolve | Apple Silicon only. GPUI glyphs must render with `font-kit`; Metal GUI presentation and CPU fframes rendering are separate gates. Signing/notarization remains M7. |

Doctor is read-only and has two explicit stages. `HostPreflight` checks host/admin prerequisites and installer access; absent app-owned Rust/FFmpeg artifacts are `Installable`, not host blockers. `CandidateSdkVerify` runs after extraction and checks the SDK toolchain, headers, libraries, DLLs, caches, bindgen/link/load probes and exact feature/ABI tuple before promotion. Every probe declares its owner and stage. A clean account without a system FFmpeg installation must progress from host-ready to SDK-ready.

The app reports observed paths/versions and a copyable reviewed package list. OS installation uses a built-in allowlisted executable and structured arguments/package IDs, never a shell command authorized by a downloaded manifest. Display the exact action before the user accepts elevation, then rerun host preflight. The app and bootstrap wrappers embed the checked manifest bytes or digest; an external manifest must match that root before it can authorize executable downloads or installer actions. This is the internal M0 trust baseline; M7 owns release signatures.

The initial bootstrap cannot assume GPUI or a Rust build environment. Native CI therefore packages a small GPUI-free `studio_setup` binary next to the app. The shell/PowerShell wrappers need only the documented platform fetch, SHA-256 and archive-extraction primitives, resolve the exact companion from the same checked manifest, verify it, and execute it. Once the app launches, it calls the same `studio-sdk` library. If graphics prevent GPUI startup, the companion still reports the missing graphics/runtime prerequisite. A source-only developer without the companion must first provide the documented minimum Rust/compiler entry environment; that is a developer route, not the clean-user product flow.

## Setup state machine and data flow

```text
Launch minimal app
  → read embedded compatibility manifest
  → HostPreflight (no mutation; app-owned artifacts may be Installable)
  → [blocked] show exact prerequisite + reviewed native install action
  → HostPreflight again
  → download/resume each SDK artifact to transaction staging
  → SHA-256 + archive-layout validation
  → extract without traversal into versioned candidate
  → CandidateSdkVerify: toolchain, bindgen/link/load and exact inputs
  → atomically update app-local active-SDK pointer
  → generate into a unique sibling staging directory outside the repo
  → validate/build/render with explicit app-local environment
  → atomically promote to the still-absent chosen directory, then display
```

Downloads use per-artifact locks, progress and cancellation. Cancel/failure removes only the transaction staging directory, preserves verified cache entries and the prior active SDK, and returns to a resumable state. Corrupt input is quarantined with expected/actual checksum. Extraction validates normalized paths, per-file/aggregate sizes and entry types before writing; reject devices/FIFOs and escaping links. Permit a necessary SDK symlink or hardlink only when explicitly declared in the checked manifest, with its target contained in the new candidate root and verified after extraction. Reject writes through existing links and test link chains, traversal and replacement races. The active directory is immutable after promotion; retain one previous SDK for rollback.

Project creation has its own app-local transaction marker outside the staging tree. Generate, validate, compile and render before atomic same-filesystem promotion; recheck that the final destination is absent and never merge into a user's existing directory. On interruption, offer resume or cleanup of only the owned stage. After promotion, rerender from the relocated project and resolve embedded media paths again. A cancelled build of an existing project retains its source and only invalidates that build output.

Every build child receives explicit `RUSTUP_HOME`, `CARGO_HOME`, `CARGO_TARGET_DIR`, toolchain path, explicit `RUSTUP_TOOLCHAIN` matching the manifest (or a direct SDK toolchain Cargo path), linker/SDK environment, and `FFMPEG_BINARIES_CACHE`. Resolve `FFMPEG_BINARIES_URL` to the verified local artifact; this is an input route, not a fail-closed control, because wrapper 9.0.0 can fall back to a network source build. The qualified SDK must reject that fallback: use an upstream fail-closed option if verified, otherwise carry a minimal checksum-recorded SDK-local wrapper patch that returns an error on unavailable/invalid prebuilt inputs. Keep explicit source builds as a separate developer route. Test missing/corrupt archives and wrong feature keys; none may start a source download. No global dependency override is permitted.

Windows additionally receives `FFMPEG_DIR`, `LIBCLANG_PATH` and a child-only `PATH` beginning with FFmpeg DLLs and resolved VS tools. Builds use `CARGO_NET_OFFLINE=true` and SDK source configuration after setup has staged all inputs. Cargo offline mode alone does not constrain arbitrary build scripts: qualify both project A and fresh project B with OS/VM network access disabled during compilation. Document that boundary separately from process crash isolation. Do not inherit developer `RUSTFLAGS`, Cargo config, registry credentials, repository target directory, or home Cargo/rustup caches unless explicitly allowed. Evidence contains only safe allowlisted values; secret-bearing values are excluded before persistence.

## Tasks and steps

1. **Specify and validate the compatibility manifest.**
   - [x] Implement the schema and parser with the fields and rejection rules above; embed a manifest digest into the minimal app artifact.
   - [x] Enumerate the exact FFmpeg 9 archive key produced by the generated project features on Linux/macOS, inspect `extralibs.txt`, and either supply every external dependency or stop the profile as blocked.
   - [x] Pin the Windows FFmpeg 9 shared asset to an immutable release identifier and SHA-256 rather than the current README's example `latest` filename.
   - [x] Record the current GPL/H.264 generator profile and its notices/source obligations as evidence. Do not turn this investigation into a product licensing decision.

2. **Implement a read-only doctor and guided host setup.**
   - [x] Implement owner/stage-aware host preflight and candidate SDK verification. Probe host target/OS, disk, linker/OS SDK, CMake, libclang, GPUI runtime and package versions first; probe app-owned Rust/FFmpeg and link/load behavior after extraction. Test a host with no system FFmpeg.
   - [x] Drive the same doctor from GPUI and the prebuilt `studio_setup` companion; `studio-dev-setup.sh` and `studio-dev-setup.ps1` verify/launch that companion and may not reimplement platform detection.
   - [x] Show the exact package list/installer source and why each item is required. Require an explicit click before an elevated OS action, then rerun doctor and preserve actionable stderr on failure.
   - [x] Ensure no path writes to global rustup/Cargo state and no Node/pnpm is installed in this phase.

3. **Build the transactional SDK installer.**
   - [x] Implement resumable staging, checksum validation, safe extraction, per-artifact lock, progress events, cancellation, retry, quarantine, atomic promotion and previous-SDK rollback.
   - [x] Preseed vendored sources/native binary caches and qualify the SDK-local fail-closed FFmpeg wrapper route; absent/corrupt archives and wrong feature keys fail before a source download. Record any minimal patch with upstream version, diff and digest in the SDK manifest.
   - [x] Run bindgen, link and runtime-load probes against the promoted candidate before switching the active pointer.
   - [x] Produce one online bundle and one equivalent offline bundle per native target with checksums and notices; package the already-built GPUI app into a minimally launchable native artifact for clean-account testing.

4. **Implement app-driven project creation/build/render.**
   - [x] From GPUI, run the packaged generator in noninteractive CPU mode to an owned sibling staging directory outside both workspaces; persist a transaction marker and reserve the user's absent final path. Validate/build/render before atomic promotion, then prove media still resolves at the final path.
   - [x] Verify the generated manifest's target guards and exact SDK-compatible fframes version before building.
   - [x] Build with an isolated project/revision target directory, show bounded stdout/stderr progress, support Cancel through the owned process tree, and retain a complete redacted command/environment manifest.
   - [x] Invoke the generated CLI's existing `frame` operation to render one PNG, load it into GPUI, and report cold build, warm build, render, conversion/upload time and SDK/bundle sizes. This bootstraps setup evidence; Phase 3 replaces CLI-per-frame preview with a persistent worker.

5. **Prove clean-account and offline reuse.**
   - [x] On each native target, start from a new OS account/VM with no repository checkout and empty user Cargo/rustup caches; install only the reviewed host prerequisites and the Phase 0 app artifact.
   - [x] Complete doctor → SDK install → project A generate/build/frame entirely in the app.
   - [x] Disable network access at the OS/VM boundary after downloads for both project A's build and fresh project B's build. Project B uses a new source path and empty target directory with `CARGO_NET_OFFLINE=true`; warm artifacts or Cargo offline mode alone do not count.
   - [x] Interrupt generation before each write/promotion boundary, restart and resume or clean only the owned stage. A destination created externally during staging must be preserved and block promotion.
   - [x] Cancel once during download and once during compile, relaunch, verify the previous SDK remains active and no owned process remains, then resume successfully.

## Verification

The following are proposed commands implemented by this phase for CI/debug parity with the app. The clean-user gate must still be completed through the GPUI flow.

```bash
cargo +1.98.1 test --locked --manifest-path desktop/Cargo.toml -p studio-sdk -p fframes-studio
cargo +1.98.1 run --locked --manifest-path desktop/Cargo.toml -p fframes-studio -- doctor --json
cargo +1.98.1 run --locked --manifest-path desktop/Cargo.toml -p fframes-studio -- install-sdk --manifest desktop/packaging/sdk/phase-zero-sdk.json
cargo +1.98.1 run --locked --manifest-path desktop/Cargo.toml -p fframes-studio -- new-project --name phase-zero-video --backend cpu --dir <clean-user-project-dir>
cargo +1.98.1 run --locked --manifest-path desktop/Cargo.toml -p fframes-studio -- build-project <clean-user-project-dir> --offline
```

The last four command names are contracts to implement, not existing repository commands. Tests use local fixtures/fault injection for interrupted downloads and invalid archives; they do not substitute fake artifacts for the native qualification. Native evidence records package versions, manifest digest, environment allowlist, checksum results, process cleanup, exact Cargo invocation, dynamic dependencies and whether the build contacted the network.

Also run unchanged root checks after implementation:

```bash
cargo metadata --locked --no-deps
cargo test -p cargo-fframes
cargo test -p fframes -p fframes-media
```

## Pass/fail criteria

- [x] One app click path visibly reaches SDK ready, creates an actual generator project outside the repo, compiles it and displays its actual rendered frame on each target.
- [x] Project B builds offline from fresh source and a fresh target directory using only the app SDK and host prerequisites; no developer checkout, user Cargo cache, global rustup mutation or network access participates.
- [x] Windows build and run both load the manifest's FFmpeg DLLs; Linux/macOS static/external libraries match the exact feature key and ABI.
- [x] Download/compile cancellation is idempotent and reaps the owned process tree; corrupt or partial installs cannot become active.
- [x] Every target result names its remaining user/admin prerequisite. A blocked result is honest evidence but does not count as a supported local-build target.

Stop and replan before Phase 3 if the exact generated project cannot build from a controlled offline bundle, the SDK depends on an unverified build-script download, Windows headers/import libraries/DLLs disagree, the non-Windows FFmpeg archive/external libraries cannot be reproduced, or a required OS SDK cannot be installed through a user-visible supported route. The fallback decision may be a narrower advertised platform or separately planned remote build; it cannot be an unproven terminal-free claim.

Any change to the requested three-platform scope requires an explicit user decision; a failed target remains an unmet gate until repaired or that change is accepted. M0's Windows route is the checksum-pinned shared FFmpeg 9 `FFMPEG_DIR` archive, with source-codec features disabled. vcpkg and MSYS2 source builds are not alternate implicit routes; a missing archive/layout fails candidate verification instead of selecting them.

## Risks, mitigation, and rollback

| Risk | Likelihood × impact | Signal / mitigation | Rollback |
|---|---|---|---|
| Native dependencies escape the SDK | High × High | Empty-home/offline project B plus system/dynamic dependency inventory. | Mark target blocked; retain prior SDK and publish exact guided prerequisite. |
| Untrusted/corrupt archive writes executable files | Medium × High | Checked-in SHA-256 manifest, size/path checks, staging and quarantine. | Delete only staging/quarantine; never switch active SDK. M7 adds signed update trust. |
| Feature archive ABI differs from generator | Medium × High | Compare target/features/key, headers, libraries and load/link probe before activation. | Reject bundle and rebuild the exact tuple; no feature substitution. |
| Administrative install surprises users | Medium × High | Read-only doctor, exact package list, explicit review/elevation, rerun probe. | Leave state at Prerequisites needed with manual instructions. |
| Build cancellation leaves locks/processes | Medium × High | Exact process-group ownership, bounded drain and restart tests. | Kill only owned group, discard revision target directory, preserve project source/SDK. |
| Current GPL/H.264 generator profile is unsuitable for distribution | Medium × High | Preserve profile facts/notices and request a product/legal decision outside this phase. | Keep artifact internal to feasibility; do not advertise or ship it. |

## Backward compatibility and rollback

The setup service adds no public fframes contract and uses the current generator unchanged. Existing CLI users continue to use their own toolchains. Projects store only the SDK ID; installed SDK state remains in app data. Removing the desktop workspace and app-data SDK leaves generated projects as ordinary Rust crates. Rollback switches the app-local pointer to the previously validated immutable SDK; never edit an active SDK in place.

## Next phase entry criteria

- [x] Phase 1 GPUI gate remains passing with the setup view attached.
- [x] All three native targets have a clean-account result and an offline project-B result, either pass or explicit blocker.
- [x] The initial development target has passed setup and real frame display.
- [x] App, SDK and project manifests record exact versions, features, ABI and checksums needed to reproduce Phase 3.
