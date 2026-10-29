> This is a historical record, not the current design; see [README.md](README.md) and [architecture.md](architecture.md) for Studio as built.

# Desktop feasibility and native qualification

Date: 2026-10-01. Status: implementation available; Phase Zero qualification **PENDING**.

The isolated desktop workspace connects managed setup, an external annotated renderer, image presentation, source inspection and a configurable ACP adapter session. Linux X11 software rendering had recorded unprivileged, network-isolated install/build, presentation, keyboard and source-click evidence (see [Qualification history](#qualification-history)). Physical-display/IME, Windows/macOS and authenticated provider qualification remain open. A software-rendered X11 run does not qualify every native platform.

## M6 provider and handoff qualification

The native provider registry, experimental picker, stopped-writer handoff and guarded session-manifest restore are implemented with fixture-backed development coverage. Their user-visible availability is not a provider qualification: [M6 results](https://github.com/khanhthanhdev/fframes-desktop/blob/037b391e80690e85ca8329d14129dc97b746eadc/desktop/qualification/m6-results.json) currently rank all four connectors as **insufficient evidence**. On the recorded host, the Claude, Codex, Pi and Antigravity ACP executables were not discoverable; their authenticated workflow gates remain `not_run`, while Windows, macOS and physical-device gates remain `blocked` on unavailable hosts/hardware.

The M6 runner, its ledger and their validator were removed with the qualification harness (see [Qualification history](#qualification-history)); the fixture-backed tests run in [Desktop self-hosted validation](../../.github/workflows/desktop-self-hosted.yml).

Compiler, GPUI and framework pins are owned by [the desktop workspace](../../desktop/Cargo.toml), [toolchain](../../desktop/rust-toolchain.toml) and [SDK manifest](../../desktop/packaging/sdk/phase-zero-sdk.json). Worker protocol and ACP protocol versions are both `1`; their transports differ. Worker control uses length-prefixed JSON over stdin/stdout, frames use a dedicated loopback TCP stream, and bounded stderr carries diagnostics. ACP uses newline-delimited JSON-RPC over adapter stdio.

## Run the native spike

```sh
cargo run --locked --manifest-path desktop/Cargo.toml -p fframes-studio -- spike-ui
```

Complete setup with an assembled SDK. Connect Worker builds the bundled annotated source with that SDK and launches the compiled executable. Step Worker Frame requests real pixels and metadata; Force Crash and Restart exercise process isolation. The last image survives a worker failure.

Worker control writes, response reads and binary payload reads share a request deadline. Expiry kills the owned process tree and rejects the response. GUI launches, restarts and agent candidates use one increasing generation allocator. Binary frame identity and geometry are validated before allocating their payload.

Click the marked title in a worker frame to inspect its Rust snippet. Preview bounds use contain-fit letterboxing. Selection uses metadata retained with the painted frame and rejects a changed project revision, worker generation, file hash, escaping path, ambiguous marker or invalid span. Edit another project file after rendering to check revision rejection; rebuild and select again.

## Configure an ACP adapter

The adapter field takes JSON with `executable`, `args` and `auth_env_names`, for example:

```json
{"executable":"/absolute/path/to/adapter","args":[],"auth_env_names":[]}
```

Use the adapter's own authenticated account/configuration. `auth_env_names` lists environment variable names to inherit when needed; never put secret values in this field. The UI supports streamed progress, permission choices, authoritative prompt completion, Stop and retained drafts. If a completed turn has made no source changes, the adapter and draft stay open for clarification: enter the answer in the prompt field and use Send Reply. It sends another prompt in the same session; duplicate, late or cancelled replies are rejected. Waiting has a deadline and Stop reaps the adapter. The adapter must provide its own filesystem/terminal tools: this minimal client advertises no client-owned tools.

Run Agent Edit builds/renders a baseline in an isolated draft, starts the configured adapter, then reaps it before rebuilding. A candidate is presented only after source and frame pixels change and the refreshed title anchor validates. Build failures and cancellation retain the draft and previous preview. Process isolation is not a security sandbox: project build scripts and adapter tools execute code. Candidate drafts stay under the project parent for inspection.

The user selected configurable adapters and deferred qualification. Protocol subprocess tests are transport evidence only. A qualified adapter needs a recorded executable/version/hash, authentication route, streamed edit, permission resolution, authoritative completion, changed Rust build/frame/anchor, and a second task cancelled while GPUI stays responsive and all owned descendants exit.

## Assemble and package

Desktop builds use core fframes crates from this checkout; they do not require publishing to crates.io or npm. The inherited [core release job](../../.github/workflows/main.yml) runs only in `dmtrKovalenko/fframes`. Forks need no upstream publishing credentials or release tags to build Studio.

[Desktop self-hosted validation](../../.github/workflows/desktop-self-hosted.yml) runs the tests and lint on the self-hosted Linux and Windows runners for every push, including changes to the local framework crates, and builds the Windows test build for pushes to `main`; it never runs for pull requests, because the repository is public. The real-worker integration and packaging run when the workflow is started by hand with **full** ticked. [Desktop CI](../../.github/workflows/desktop.yml) runs the tests and lint on hosted Ubuntu for pull requests from other repositories. See [Self-hosted runners](self-hosted-runners.md) for the machine setup and how to download the `studio-test-<target>` builds.

On Unix, build the pinned official FFmpeg source and supply its real static install. Windows uses an FFmpeg 9 shared install and a native MSVC build environment.

```sh
python3 desktop/scripts/build-native-ffmpeg.py --source /tmp/studio-ffmpeg-source --out /tmp/studio-ffmpeg-install
python3 desktop/scripts/assemble-phase-zero-sdk.py --out /tmp/studio-sdk --ffmpeg-root /tmp/studio-ffmpeg-install
python3 desktop/scripts/package-phase-zero.py --out /tmp/studio-package --sdk-bundle /tmp/studio-sdk
```

Use fresh output directories. The [shell wrapper](../../desktop/scripts/build-phase-zero-sdk.sh) forwards assembler arguments; the [PowerShell wrapper](../../desktop/scripts/build-phase-zero-sdk.ps1) accepts `-Out`, `-FfmpegRoot` and optional `-Vendor`. Assembly copies the actual native Rust sysroot, verified FFmpeg headers/libraries, a minimal framework workspace and its Cargo vendor graph. The bundle carries only the current framework tree; assembly does not export a legacy tree or require full Git history. Its compatibility manifest names the current crates (`fframes_version`, `runtime_version`) and the vendored kit (`kit_version`); projects pinned to earlier SDK releases require their matching SDK or migration. The bundled framework disables the FFmpeg dependency's native `build` feature so it honors the supplied `FFMPEG_DIR`, retaining static linkage on Unix. The repository's development dependency stays unchanged. Before archiving, assembly builds and renders the annotated worker and a kit component twice, with Cargo offline, an empty Cargo home and separate target directories. It derives archive sizes and hashes into the bundle's compatibility manifest. These local checks do not establish native release qualification. Cargo offline alone does not block native build-script downloads; the isolated qualification below disables external networking too.

Installation verifies compiler version/target, native libraries and complete SDK layout, then compiles and renders both the generated CPU template and annotated worker inside staging with the managed environment. Failed verification removes staging and preserves active. Each compiler/build/render process has a deadline, concurrently drained bounded output, and owned descendant cleanup. Only verified candidates are promoted; promotion failure restores the old active directory.

Packaging creates the native app, setup companion, worker source/font notices, SDK, inventory/checksums and a zip. Windows includes FFmpeg DLLs; macOS includes an unsigned `.app`; Unix records native dependency inventory. Omitting `--sdk-bundle` produces an app-only artifact, with SDK/setup gates still unmet. [Desktop self-hosted validation](../../.github/workflows/desktop-self-hosted.yml) builds the Linux and Windows test packages. Feasibility/CI artifacts remain unsigned; the separate [Desktop Release workflow](../../.github/workflows/desktop-release.yml) signs tag builds and opt-in manual release builds before upload.

### Windows installer

[package-windows-installer.py](../../desktop/scripts/package-windows-installer.py) turns the native Windows package directory into a per-user `fframes-studio-x86_64-pc-windows-msvc-setup.exe` with the pinned Inno Setup 7.1.0 compiler (`--iscc` or `INNO_SETUP_COMPILER`), using the [installer script](../../desktop/packaging/windows/fframes-studio.iss):

```powershell
py desktop/scripts/package-windows-installer.py --source <package-dir> --out fframes-studio-x86_64-pc-windows-msvc-setup.exe
```

Setup needs no administrator rights and shows only progress and a finish page. It installs into `%LOCALAPPDATA%\Programs\fframes Studio`, adds Start menu and desktop shortcuts and an Apps & features entry, and upgrades an existing install in place. It closes a running studio first. The installer embeds the SDK because guided online SDK acquisition is not connected yet. This differs from the Linux `.deb`. The shortcuts start `bin\fframes-studio.exe` directly, a GUI-subsystem executable that finds `sdk\` beside `bin\` and starts its build, worker and adapter processes without console windows. Uninstall removes the install folder and shortcuts only. Projects (`%USERPROFILE%\.fframes`), the managed SDK and app state (`%LOCALAPPDATA%\fframes-studio`) are kept. The Desktop Release workflow retains the installer as an attested candidate artifact, signed together with the package when signing is requested. Unsigned preview releases publish it (below); the consumer release waits until signing keys are configured and a target is made eligible in the release policy. Unsigned builds trigger the SmartScreen "unknown publisher" prompt when downloaded with a browser.

### Unsigned preview releases

Pushing a `v<version>` tag, or a manual Desktop Release run with `preview_release`, publishes an unsigned preview GitHub release. The tag must match `desktop/app/Cargo.toml`, and an existing release is never replaced. The `publish-preview` job collects every target ZIP, the Windows Setup.exe and the Linux `.deb`. [prepare-github-release.py](../../desktop/scripts/prepare-github-release.py) then adds:

- winget manifests (`khanhthanhdev.fframesStudio`, schema 1.12.0, Inno per-user installer, ProductCode from the installer's AppId),
- a Scoop manifest (`fframes-studio.json`, portable ZIP with a shortcut and a `fframes-studio` shim),
- [install.ps1](../../desktop/packaging/windows/install.ps1), which downloads Setup.exe, verifies it against `SHA256SUMS.txt` and installs per user,
- `SHA256SUMS.txt` and release notes with install instructions.

Preview releases are not marked as prereleases unless requested, because `releases/latest/download`, used by `install.ps1` and the Scoop URL, only follows full releases. The consumer release (`consumer_release`) publishes only the targets listed in `eligible_targets` of `desktop/packaging/release-policy.json`, and only when the policy enables it and the tag commit has a successful full `Desktop self-hosted validation` run (dispatched with `full`) and a successful `Desktop CI` run dispatched on `main`. `desktop/scripts/release-gate.py` makes that decision from the runs the workflow fetches, and `desktop/scripts/verify-package.py` checks each package against its inventory. With `dry_run` the job does both and publishes nothing. `desktop/scripts/test-release-gate.py` tests the refusals offline.

Users install with any of:

```powershell
irm https://github.com/khanhthanhdev/fframes-desktop/releases/latest/download/install.ps1 | iex
scoop install https://github.com/khanhthanhdev/fframes-desktop/releases/latest/download/fframes-studio.json
winget install khanhthanhdev.fframesStudio
```

winget only works after the package is accepted into [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs). To submit, place the three released `khanhthanhdev.fframesStudio*.yaml` files in `manifests/k/khanhthanhdev/fframesStudio/<version>/` of a fork and open a pull request, or run `wingetcreate submit <that folder>`. Later versions can be submitted the same way, or automated with a token-holding action once the package exists.

### Native release signing

Desktop Release builds Windows only. A manual run can opt in to signing with `sign_artifacts`; setting `consumer_release` also requires signing when it builds fresh artifacts. Windows Authenticode-signs the app executables and runtime DLLs in the package's `bin/` directory, timestamps signatures, verifies each file, then refreshes inventory/archive. GitHub artifact attestations provide provenance for the ZIP and the installer; these attestations do not replace code signing.

Configure these repository Actions secrets before requesting signed builds:

| Secret | Value |
| --- | --- |
| `WINDOWS_CODESIGN_PFX_BASE64` | Base64-encoded Authenticode code-signing `.pfx` certificate |
| `WINDOWS_CODESIGN_PFX_PASSWORD` | Password protecting that `.pfx` |

The signing script requires the secrets, validates/imports the expected certificate type, and fails before artifact retention if credentials are missing or invalid. Certificates and temporary key material are removed in cleanup handlers; secret values are never printed. Do not put credentials in the repository, qualification ledger, package inventory, or workflow inputs. The current checkout has no signing secrets configured, so signed Windows release runs are not yet executable here. Consumer publication remains separately blocked until trusted release keys and all target qualification gates are configured.

## Qualification history

Until 2026-10-15 each milestone was qualified by a harness. The `qualify-*` scripts drove the packaged app (on an isolated, network-less Xvfb account on Linux; with `SendInput` on Windows), ran the 1,000-presentation stress, and recorded the results with hashed evidence in ledgers under `desktop/qualification/`, which `validate-qualification.py` checked and the consumer release required. The harness, the ledgers and the validator were removed. The consumer release now requires a successful full self-hosted run and a Desktop CI run on `main` ([release gate](../../desktop/scripts/release-gate.py)), and full runs drive the real app on Xvfb with the X11 shell tests. The last versions are kept at commit `037b391`: the [ledgers and evidence](https://github.com/khanhthanhdev/fframes-desktop/tree/037b391e80690e85ca8329d14129dc97b746eadc/desktop/qualification), the [scripts](https://github.com/khanhthanhdev/fframes-desktop/tree/037b391e80690e85ca8329d14129dc97b746eadc/desktop/scripts) and [this page with the harness instructions](https://github.com/khanhthanhdev/fframes-desktop/blob/037b391e80690e85ca8329d14129dc97b746eadc/docs/desktop/phase-zero-feasibility.md).

What the ledgers recorded still holds. The Linux evidence was software-rendered X11 against a scripted ACP peer; the Windows runs on a Windows Server 2022 VM were development evidence. No authenticated provider, physical display, IME, physical audio device, macOS or Windows release qualification was recorded.

Windows packaging links the executables against the static C runtime and writes their import tables to `native-dependencies.txt`; it fails if any packaged binary imports a Visual C++ Redistributable DLL such as `VCRUNTIME140.dll`, so a machine without the Redistributable can start the app. Keep Windows build output paths short; nested Cargo targets otherwise exceed MSVC path limits.

## Agent workflow notes

**Linux publication primitive.** On the recording machine (Ubuntu 24.04, kernel 6.8.0-142-generic, ext4 on `/dev/vda2`) the Apply gate's probe selects `renameat2(RENAME_NOREPLACE)`; forcing a link-only filesystem blocks Apply and Undo because `linkat`+`unlinkat` could unlink an editor's concurrent replacement of a live project file, so the link fallback is refused for live names. Other filesystems and operating systems were not probed.

**Packaging.** The agent is told `studio-tools --capability <file> <tool>` and MCP uses the `studio-mcp` binary found next to the running executable (`agent_tools::sibling_binary`), so `package-phase-zero.py` installs `studio-tools` and `studio-mcp` beside `fframes-studio` (also inside the macOS bundle's `Contents/MacOS`), lists them in `inventory.json` and checks their dynamic dependencies. `test-packaging.py` extracts an archive and resolves the helpers from the *installed* layout, mirroring the Rust lookup; Windows and macOS package layouts are compiled by inspection only.

Physical, Windows, macOS and authenticated-provider qualification remain open; the development tests are not release, native-platform or provider qualification.
