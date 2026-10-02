# Phase 0 plan review: security adversary and contract verification

Date: 2026-10-01
Scope: `plans/261001-1707-desktop-phase-zero-setup/plan.md` and `phase-01-start.md`, `phase-02-managed-sdk-and-project-setup.md`, `phase-03-feasibility-and-platform-qualification.md`.

## Result

The plan is materially aligned with the repository contracts: it keeps GPUI outside the core workspace, treats the worker as crash isolation rather than a sandbox, requires app-local child environments, preserves the generator’s current GPL/H.264 profile, and makes clean-account/offline evidence a target gate. The findings below are concrete implementation blockers to close during Phase 2/3; they do not require broadening M0 into release signing or a general legal approval process.

## Prioritized findings

### High — `FFMPEG_BINARIES_URL=file://` does not prevent an untracked source download

Evidence: Phase 2 says the manifest resolves `FFMPEG_BINARIES_URL` to a verified local artifact “so the FFmpeg build script cannot fall back to an untracked download” ([phase-02-managed-sdk-and-project-setup.md:90](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L90)). The published wrapper does the opposite when local installation fails: `try_install` returns `false` after a missing/corrupt local archive and logs “compiling from source” ([`build_prebuilt.rs:82-109`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build_prebuilt.rs)); `build.rs` then clones `https://github.com/FFmpeg/FFmpeg`, release branch `9.0`, and runs configure/make/install ([`build.rs:163-187`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs), [`build.rs:782-801`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)). `CARGO_NET_OFFLINE` does not constrain that build script; the plan correctly acknowledges this for project B but does not apply the boundary to project A ([phase-02:90,121](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L90)).

Failure: a missing or damaged prebuilt archive can cause project A’s Cargo build to clone and compile a different FFmpeg source tree. That can consume network, alter the feature/codec result, or fail late after the user accepted a supposedly self-contained SDK.

Fix: run project A’s build under the same network-denied child boundary as project B after setup has staged all artifacts, or add a fail-closed build wrapper that proves the exact cache archive/key before Cargo starts and detects any `git`/`curl` FFmpeg child. Treat any source fallback as `BLOCKED`; do not rely on a local URL override as a policy control. Keep the existing source fallback available only as an explicitly separate developer mode.

### High — bootstrap checksum trust origin and elevated package route need an explicit root

Evidence: Phase 2 requires a checked manifest, SHA-256, and a companion executable ([phase-02:35-39,68,94-98](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L35), [phase-02:68](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L68)); the setup flow reads an embedded manifest ([phase-02:72-82](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L72)), but the wrapper description says it resolves the companion from the “same checked manifest” ([phase-02:68](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L68)). The doctor may open a platform installer or launch a package-manager command after review/elevation ([phase-02:66](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L66)). The repository’s current CI downloads a moving `latest` BtbN URL without a checksum ([`.github/workflows/main.yml:338-348`](../../.github/workflows/main.yml#L338-L348)); the plan correctly requires replacing it with an immutable asset and SHA-256 ([phase-02:97](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L97)).

Failure: a checksum authenticates bytes only relative to the manifest. If a wrapper accepts a separately downloaded/replaced manifest, or invokes a shell-rendered package command whose arguments came from that manifest, a tampered manifest can authorize an executable companion or an elevated install action. M0 does not need release signatures, but it does need a non-network trust root for the internal spike.

Fix: embed the manifest bytes or digest in the app/companion build and require the external copy to match; never fetch the manifest used to authorize the companion or elevation. Store package actions as an allowlisted executable plus structured arguments/package IDs, invoke without a shell, display the exact arguments, and reject URLs/commands outside the embedded target record. Keep SHA-256 as the M0 integrity baseline; leave signed manifests/installers for M7 as the plan states.

### High — archive extraction must reject links and special entries before promotion

Evidence: Phase 2 promises “safe extraction,” traversal/size checks and quarantine ([phase-02:78-88,106-110](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L78)), but the underlying wrapper extracts with external `tar -xzf` before it checks only that `lib`, `include`, and `extralibs.txt` exist ([`build_prebuilt.rs:129-145`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build_prebuilt.rs)). The active directory is then treated as immutable after promotion ([phase-02:88](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L88)).

Failure: an archive with a symlink, hardlink, device, or other special entry can cause extraction-time writes or later probes to reach outside the candidate root, or can leave a path that changes after validation. This matters because the SDK contains executable companions, headers, libraries and DLLs that the app will run.

Fix: make the app extractor inspect every archive entry before writing; allow regular files/directories only, reject symlink/hardlink/device/FIFO entries, reject absolute and normalized `..` paths, enforce per-file and aggregate limits, extract into a new directory on the same filesystem, then apply read-only permissions and atomically promote. Do not delegate the trust boundary to the wrapper’s `tar` extraction. Add link-entry and replacement-race fixtures to the existing invalid-archive tests.

### High — “exact process group” is insufficient for Windows Cargo/build-script cancellation

Evidence: Phase 1 and Phase 2 require exact process-group ownership and clean cancellation ([phase-01:69-70](../261001-1707-desktop-phase-zero-setup/phase-01-start.md#L69), [phase-02:115,122,151,164](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L115)); Phase 3 repeats “cross-platform worker/agent process groups” ([phase-03:32-35](../261001-1707-desktop-phase-zero-setup/phase-03-feasibility-and-platform-qualification.md#L32)). The FFmpeg build script nests children: `curl` for archives ([`build_prebuilt.rs:149-169`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build_prebuilt.rs)), `git` for source ([`build.rs:163-180`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)), `sh` for Windows configure ([`build.rs:332-350`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)), and `make`/`make install` ([`build.rs:782-801`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)).

Failure: terminating Cargo or the immediate build child can leave a descendant running, retaining the SDK/project target directory or continuing a network/build write after the UI reports cancellation. Unix process-group handling does not establish equivalent Windows tree ownership.

Fix: make the Windows supervisor assign every build/download/adapter/worker descendant to a Job Object with kill-on-close, and use a process group/session plus descendant verification on Unix. Track command/PID/job identity, close stdin and pipes, enforce a bounded drain, then verify no owned descendant remains before deleting staging or reporting cancellation. Keep the existing unrelated-sentinel test and add nested `curl`/`git`/shell fixture coverage.

### Medium — adapter credential handling is underspecified for provider secrets

Evidence: architecture places authentication credentials in provider-managed or OS credential storage ([architecture.md:119](../../docs/desktop/architecture.md#L119)); Phase 3 says credentials are not copied to project/app logs, but only promises “redacts known secret fields” while retaining structured errors and bounded stderr ([phase-03:85-90](../261001-1707-desktop-phase-zero-setup/phase-03-feasibility-and-platform-qualification.md#L85)). Phase 2 also requires a redacted command/environment manifest ([phase-02:115](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L115)). The plan explicitly recognizes that adapters are separate installs/processes, while process separation is not a sandbox ([architecture.md:66-70](../../docs/desktop/architecture.md#L66)).

Failure: a qualified adapter can receive an API token through an environment variable or emit it in stderr/protocol diagnostics under a provider-specific field name. “Known field” redaction and a saved environment manifest can then persist the secret in app data or a run report.

Fix: make adapter environments deny-by-default, with a provider-specific credential handle or explicit secret injection that is never included in environment snapshots. Persist only variable names and a secret-present bit; never capture raw environment values. Redact before bounded logs are written, not only when a report is rendered, and add a fixture that emits an unknown secret-shaped field and an env token to prove it cannot reach evidence artifacts.

## Verified contracts and no finding

- The plan correctly keeps the desktop workspace isolated and prevents GPUI from entering the root workspace; Phase 1’s metadata checks match the current root resolver-2 workspace ([`Cargo.toml:1-51`](../../Cargo.toml#L1-L51)).
- The plan correctly treats the renderer worker as crash isolation, not security isolation. User project build scripts, procedural macros, constructors and agent tools execute code ([architecture.md:66-70](../../docs/desktop/architecture.md#L66)). The UI disclosure requirement is appropriate for M0.
- Windows setup facts are accurate: fframes uses a shared FFmpeg 9 through `FFMPEG_DIR`/vcpkg and requires LLVM/libclang; the FFmpeg `bin` directory must be available during build/test and runtime ([`README.md:263-277`](../../README.md#L263-L277), [`build.rs:1213-1244`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs), [`.github/workflows/main.yml:338-348`](../../.github/workflows/main.yml#L338-L348)).
- Linux/macOS static linking is correctly qualified as potentially dependent on external codec libraries: the wrapper preserves `EXTRALIBS` and resolves missing external search paths through pkg-config ([`build.rs:1142-1208`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)); Phase 2 requires inspecting `extralibs.txt` and either supplying dependencies or blocking the profile ([phase-02:96](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L96)).
- The generator claim is accurate: `cargo-fframes` adds only `h264` and `libav-agree-gpl` under `cfg(not(windows))` ([`cargo-fframes/src/main.rs:498-507`](../../cargo-fframes/src/main.rs#L498-L507)). The plan preserves this measured profile instead of silently changing codec scope.
- Offline qualification is framed correctly as a fresh project and fresh target directory with network disabled, rather than trusting `CARGO_NET_OFFLINE` alone ([phase-02:118-123](../261001-1707-desktop-phase-zero-setup/phase-02-managed-sdk-and-project-setup.md#L118)). The High finding above extends the same fail-closed boundary to project A.
- Source-anchor validation has concrete revision, hash, path-containment, marker-uniqueness and byte-boundary checks ([phase-03:67-81](../261001-1707-desktop-phase-zero-setup/phase-03-feasibility-and-platform-qualification.md#L67)), so no additional anchor concern is warranted.

## Review disposition

Prioritize the four High findings before declaring Phase 2’s setup contract implemented. The provider-secret Medium finding belongs in the Phase 3 adapter supervisor/evidence implementation. With those fixes, the plan’s existing clean-account, offline, ABI/load, cancellation, and rollback gates are adequate to produce an evidence-backed Linux-first decision while leaving blocked Windows/macOS targets unadvertised.

Status: DONE_WITH_CONCERNS
Summary: Verified the stable plan against the fframes manifests, generator, CI, and published FFmpeg wrapper. Found four High concrete blockers (FFmpeg fallback, bootstrap trust/elevation root, archive link extraction, and Windows descendant cancellation) plus one Medium provider-secret evidence gap.
Concerns/Blockers: Phase 2 should fail closed against wrapper source fallback and define the initial manifest trust root; Phase 3 should specify Windows Job Objects and deny-by-default adapter secret handling.
