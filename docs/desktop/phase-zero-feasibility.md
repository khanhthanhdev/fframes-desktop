# Desktop feasibility and native qualification

Date: 2026-10-01. Status: implementation available; Phase Zero qualification **PENDING**.

The isolated desktop workspace connects managed setup, an external annotated renderer, image presentation, source inspection and a configurable ACP adapter session. Linux X11 software rendering has reproducible unprivileged, network-isolated install/build, presentation, keyboard and source-click evidence. Physical-display/IME, Windows/macOS and authenticated provider qualification remain open. A software-rendered X11 run does not qualify every native platform.

## M6 provider and handoff qualification

The native provider registry, experimental picker, stopped-writer handoff and guarded session-manifest restore are implemented with fixture-backed development coverage. Their user-visible availability is not a provider qualification: [M6 results](../../desktop/qualification/m6-results.json) currently rank all four connectors as **insufficient evidence**. On the recorded host, the Claude, Codex, Pi and Antigravity ACP executables were not discoverable; their authenticated workflow gates remain `not_run`, while Windows, macOS and physical-device gates remain `blocked` on unavailable hosts/hardware.

From the repository root, use the M6 runner as follows:

```sh
python3 desktop/scripts/qualify-m6-providers.py --help
python3 desktop/scripts/qualify-m6-providers.py --mode development
python3 desktop/scripts/qualify-m6-providers.py --mode authentic
python3 desktop/scripts/validate-qualification.py desktop/qualification/m6-results.json
python3 desktop/scripts/test-qualification-m6.py
```

Development mode executes the local fixture, actor/engine, panel, qualification-contract and format checks; it removes common API-key/token variables from child environments, redacts common credential forms and private paths in saved logs, and writes a run report and hashed logs under `desktop/qualification/evidence/`. Development evidence never edits the provider ledger and cannot satisfy authentic gates. Authentic mode performs only a read-only `PATH` lookup for the four adapter executable names. It launches no process, reads no credentials, makes no ACP handshake and leaves the ledger unchanged; therefore a found executable remains `not_run`, not ready or qualified. Real workflow qualification requires a supervised authenticated run in a disposable project, separate platform/device evidence and the full gate-specific structured proofs accepted by the validator. Do not infer passes from the fixture UI run or Xvfb. M6 export evidence uses the existing CLI backend; native export UI, installers and clean-machine release acceptance remain M7.

Compiler, GPUI and framework pins are owned by [the desktop workspace](../../desktop/Cargo.toml), [toolchain](../../desktop/rust-toolchain.toml) and [SDK manifest](../../desktop/packaging/sdk/phase-zero-sdk.json). Worker protocol and ACP protocol versions are both `1`; their transports differ. Worker control uses length-prefixed JSON over stdin/stdout, frames use a dedicated loopback TCP stream, and bounded stderr carries diagnostics. ACP uses newline-delimited JSON-RPC over adapter stdio.

## Run the native spike

```sh
cargo run --locked --manifest-path desktop/Cargo.toml -p fframes-studio -- spike-ui
```

Complete setup with an assembled SDK. Connect Worker builds the bundled annotated source with that SDK and launches the compiled executable. Step Worker Frame requests real pixels and metadata; Force Crash and Restart exercise process isolation. The last image survives a worker failure.

Connect a real worker before using the stress control. Each of 1,000 iterations completes an actual seek and its latest coalesced pending seek, requests metadata, and converts the returned pixels off the UI thread. Exactly one pending image advances only after successful painting and the next GPUI frame callback. Success requires 1,000 confirmations and 2,000 verified renders. It observes GPUI frame submission, not a hardware fence or physical display completion. The evidence records process RSS samples, queue depth, managed current-image count, image-release calls and UI heartbeat gaps; these are not GPU allocation measurements. A stalled frame or failed request aborts the run, preserves the displayed image and permits reconnection.

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

Run [Desktop CI](../../.github/workflows/desktop.yml) or [Desktop feasibility artifacts](../../.github/workflows/desktop-phase-zero.yml) from GitHub Actions with **Run workflow** to build the native Linux, Windows and macOS matrix. Both also run for relevant pushes to `main` and pull requests, including changes to the local framework crates. Download the `studio-<target>` artifact from Desktop CI for the packaged app and SDK; these feasibility packages remain subject to the qualification gates below.

On Unix, build the pinned official FFmpeg source and supply its real static install. Windows uses an FFmpeg 9 shared install and a native MSVC build environment.

```sh
python3 desktop/scripts/build-native-ffmpeg.py --source /tmp/studio-ffmpeg-source --out /tmp/studio-ffmpeg-install
python3 desktop/scripts/assemble-phase-zero-sdk.py --out /tmp/studio-sdk --ffmpeg-root /tmp/studio-ffmpeg-install
python3 desktop/scripts/package-phase-zero.py --out /tmp/studio-package --sdk-bundle /tmp/studio-sdk
python3 desktop/scripts/validate-qualification.py desktop/qualification/m0-results.json
```

Use fresh output directories. The [shell wrapper](../../desktop/scripts/build-phase-zero-sdk.sh) forwards assembler arguments; the [PowerShell wrapper](../../desktop/scripts/build-phase-zero-sdk.ps1) accepts `-Out`, `-FfmpegRoot` and optional `-Vendor`. Assembly copies the actual native Rust sysroot, verified FFmpeg headers/libraries, a minimal framework workspace and its Cargo vendor graph. The bundled framework disables the FFmpeg dependency's native `build` feature so it honors the supplied `FFMPEG_DIR`, retaining static linkage on Unix. The repository's development dependency stays unchanged. Before archiving, assembly builds and renders the annotated worker twice with Cargo offline, an empty Cargo home and separate target directories. It derives archive sizes and hashes into the bundle's compatibility manifest. Cargo offline alone does not block native build-script downloads; the isolated qualification below disables external networking too.

Installation verifies compiler version/target, native libraries and complete SDK layout, then compiles and renders both the generated CPU template and annotated worker inside staging with the managed environment. Failed verification removes staging and preserves active. Each compiler/build/render process has a deadline, concurrently drained bounded output, and owned descendant cleanup. Only verified candidates are promoted; promotion failure restores the old active directory.

Packaging creates the native app, setup companion, worker source/font notices, SDK, inventory/checksums and a zip. Windows includes FFmpeg DLLs; macOS includes an unsigned `.app`; Unix records native dependency inventory. Omitting `--sdk-bundle` produces an app-only artifact, with SDK/setup gates still unmet. The [CI matrix](../../.github/workflows/desktop-phase-zero.yml) builds on Linux x64, Windows x64 MSVC and macOS arm64, assembles the native SDK and retains pending qualification records. Its Linux job additionally runs the isolated X11 procedure and retains evidence. The remote workflow has not been executed by this local implementation session.

## Reproduce isolated Linux evidence

Install Xvfb, xdotool, Mesa's software renderer, Python Pillow, iproute2 and util-linux alongside the documented host compiler/linker prerequisites. Run the packaged executable and packaged SDK:

```sh
sudo /usr/bin/python3 desktop/scripts/qualify-linux-native.py \
  --application /tmp/studio-package/bin/fframes-studio \
  --bundle /tmp/studio-package/sdk \
  --out /tmp/studio-native-evidence --display :94 --isolated-user
```

The harness rejects occupied display/output paths. It uses a fresh home for UID 65534, no host Cargo cache, an owned PID namespace, and a network namespace with loopback only for worker IPC. SDK staging builds/renders project A before promotion; the GUI then builds a fresh annotated project B with a separate target directory. An Xvfb window presents all 1,000 real worker frames. Readiness comes from painted viewport/input geometry; native keyboard and pointer events type into the registered component and select the title through letterboxing. JSON records, logs and a window screenshot are saved before owned processes are reaped. Noncompletion, typing or selection failure returns nonzero. `qualify-presentation` is also available directly with `--bundle`, `--sdk-home`, `--project` and `--output`; without the harness its process is not network/account isolated.

## Qualification evidence

The [results ledger](../../desktop/qualification/m0-results.json) and [schema](../../desktop/qualification/m0-results.schema.json) distinguish recorded Linux evidence from remaining platform gates; ACP stays `NOT_RUN`. Passed rows reference evidence files with SHA-256 hashes checked by the validator. Artifact build evidence remains separate from interactive gate evidence. The ledger's measured RSS is process memory under software rendering, not GPU memory; no physical GPU resource gate is inferred.

For each native target, launch the packaged artifact from a clean account, run host preflight and transactional SDK installation, build/render project A, then disconnect networking and build a fresh project B without ambient caches. Verify native typing/IME, fonts, pixel/alpha behavior, 1,000 presentation confirmations, click-through, worker crash/restart and process cleanup. Record measured timings/resources and failures. A platform stays pending or blocked until its own required gates pass. Authenticated ACP qualification remains deferred.

## M3 agent-transaction qualification

Status: **M3 authenticated acceptance is NOT claimed.** The Linux development evidence below proves the workflow, engine, tool and cleanup code of this repository against a *scripted* ACP peer; every gate that needs a real authenticated adapter, a physical device or another operating system is `not_run` with its missing prerequisite.

Run from `desktop/` (all arguments are optional; `--help` is the reference):

```sh
python3 scripts/qualify-m3-agent.py [--out DIR] [--date YYYY-MM-DD] [--ledger FILE] [--replace]
    [--checks NAME[,NAME...]] [--list-checks] [--cargo-timeout SECONDS]
    [--real-sdk-active DIR]
    [--adapter PATH [--adapter-arg ARG ...] [--auth-env NAME ...]]
python3 scripts/validate-qualification.py        # validates the M0, M2 and M3 ledgers
python3 scripts/test-packaging.py                # validator, harness safety and installed-helper regressions
```

The harness records OS, kernel, toolchain, git commit and a dirty-tree fingerprint, the locked ACP SDK crate and (with `--real-sdk-active`) the installed SDK. It runs, with `cargo --locked` and timeouts, the ACP driver, engine transaction/recovery, `agent_workflow`, tools/candidate/handoff and `m3_development_evidence` suites (plus `agent_workflow_ui`, and the ignored real-SDK workflow tests and the Xvfb shell test when an SDK is given), then writes hashed logs and measurement JSON to `desktop/qualification/evidence/linux-m3-<date>/` and rewrites [`m3-results.json`](../../desktop/qualification/m3-results.json) ([schema](../../desktop/qualification/m3-results.schema.json)). Safety: its only fixed endpoint is the single-instance socket `/tmp/fframes-m3-qualify-<uid>.sock` (it refuses to start if that path exists and binds no TCP port); every child runs in its own session with a harness marker, and only harness-spawned process groups or marker carriers are ever signalled (leftovers are recorded); one private work directory is removed at exit; credential values from `--auth-env` and common token shapes are redacted before anything is serialized, environment values are never written, and prompts are never recorded.

**Ledger layout.** Each gate has `kind` (`development` or `authentic`), `status` (`pass`, `fail`, `blocked`, `not_run`), `criteria`, `notes` and `evidence` (relative path + SHA-256). `not_run`/`blocked` require a stated `prerequisite`; `pass` requires evidence. The validator rechecks every hash, confines evidence to `qualification/evidence/`, rejects credential-like text in evidence and ledger, and derives `acceptance.m3_authenticated`: it can be `pass` only if every authentic gate passes. A development `pass` must be backed by the harness run record it cites (the producing check ran, exited 0 and passed tests). An authentic `pass` needs a non-fixture adapter identity (name, ACP v1, executable digest and launch identity) and may cite only structured records of schema `m3-authentic/1` that are bound to that one gate, state `fixture_only: false`, record the measured platform (Windows/macOS gates must name theirs), the adapter's launch identity, a Ready non-fixture probe, a clean teardown and the gate's own measurements (for example scenario-by-scenario containment, the two-edit/Undo/restart revision chain, 20 real cycles); a record cannot back two gates, a `not_run` authentic gate cites nothing, and a label such as `evidence_kind: authentic` alone is rejected. The same launch-identity digest gates the application's writer-containment qualification. Evidence and ledger are scanned NUL-safely for credential-like text (textual evidence must be UTF-8; unsupported binary types are rejected) and for absolute private paths (the harness normalizes its own paths to `<repo>`, `<home>`, `<sdk-active>`, `<work>`, `<evidence>` placeholders before hashing). No code path turns development evidence into an authentic pass.

**Development gates** (fixture evidence, Linux software only): `dev_acp_transport`, `dev_engine_transactions`, `dev_workflow_fixture`, `dev_tools_and_validation`, `dev_native_ui_workflow` (`agent_workflow_ui`: conversation controls, permission/clarification cards, paging inside the resident limits, the UI preview-handoff sink), `dev_native_x11_shell` (the production GPUI shell as a real process on an owned Xvfb driven with xdotool: a baseline preview stepped to a non-zero playhead, a prompt typed through native input, a media-changing edit adopted through `adopt_promotion` and displayed with the same video and audio revision, a changed audio digest, a fresh transport epoch, the playhead position/frame/PCM window retained and the awaiting label cleared, and Undo restoring the original audio through another matching hand-off under yet another epoch; writer containment is the explicit test-only injection; scripted peer, real SDK, software rendering; runs only with `--real-sdk-active` and `Xvfb` + `xdotool` on PATH, otherwise `not_run` naming the missing program; it is not a provider, physical-display or IME qualification), `dev_resource_bounds` (a 1,499-row / 4.8 MB streamed transcript against the 400-row, 4 MiB resident window and the 256-event driver queue), `dev_compiler_count` (one compile per equal build key through the shared `BuildService`), `dev_cleanup_cycles` (20 accepted edits plus 20 failed/cancelled tasks, zero owned processes, broker capabilities, build leases and materializations after every cycle and after close), `dev_cli_mcp_parity` (`studio-tools` and `studio-mcp` answer identically against one broker), `dev_publication_primitive`, and `dev_real_sdk_workflow` (real Cargo compile and preview worker with a scripted agent, only with `--real-sdk-active`).

**Authentic gates** (all `not_run` on this machine): `auth_adapter_probe_v1`, `auth_two_edit_undo_restart`, `auth_writer_process_group`, `auth_compiler_error_repair`, `auth_interaction_and_failure_modes`, `auth_mcp_cli_support`, `auth_twenty_cycle_cleanup`, `auth_visual_ime`, `auth_physical_audio`, `auth_windows`, `auth_macos`. The only authentic observation the harness can make truthfully is the handshake: `--adapter PATH` (with `--adapter-arg`/`--auth-env NAME`) initializes the real adapter, requires ACP v1, creates a scratch session and records capabilities; it sends no prompt, makes no edit and claims no writer containment, and a pass records only that handshake. Repository fixtures and adapters that identify as fixtures are refused or recorded as development evidence. The harness cannot prove that an operator-supplied executable is a genuine provider; the executable digest and identity are recorded for review. Two-edit/Undo/restart, repair, Stop/crash/conflict/interrupted-publication, MCP/CLI support, 20-cycle cleanup and writer process-group qualification need an operator-driven session with a real authenticated adapter and have no automated driver; visual/IME, physical audio, Windows and macOS need that hardware or OS. They stay pending.

**Evidence limits.** Measurements use the scripted peer and a fake preview worker: they bound *our* code (the driver exposes no queue-depth gauge, so the event-queue bound is the absence of an overflow while the scripted event count exceeds 256; the peer's flood is paced because an unpaced burst of non-coalescing events deliberately seals the producer), not provider behavior. Playback/scrub concurrency during streaming, GPU layout cost, pixel parity against the project CLI on a real SDK, physical audio and IME are not measured here. The scripted peer's writer ownership is test configuration, not an adapter qualification; unknown or detached writers keep automatic Apply unqualified.

**Linux publication primitive.** On the recording machine (Ubuntu 24.04, kernel 6.8.0-142-generic, ext4 on `/dev/vda2`) the Apply gate's probe selects `renameat2(RENAME_NOREPLACE)`; forcing a link-only filesystem blocks Apply and Undo because `linkat`+`unlinkat` could unlink an editor's concurrent replacement of a live project file, so the link fallback is refused for live names (`dev_publication_primitive`, `publication-primitive.json`). Other filesystems and operating systems were not probed.

**Packaging.** The agent is told `studio-tools --capability <file> <tool>` and MCP uses the `studio-mcp` binary found next to the running executable (`agent_tools::sibling_binary`), so `package-phase-zero.py` installs `studio-tools` and `studio-mcp` beside `fframes-studio` (also inside the macOS bundle's `Contents/MacOS`), lists them in `inventory.json` and checks their dynamic dependencies. `test-packaging.py` extracts an archive and resolves the helpers from the *installed* layout, mirroring the Rust lookup; Windows and macOS package layouts are compiled by inspection only.

Physical, Windows, macOS and authenticated-provider gates remain pending; do not read the development gates as release, native-platform or provider qualification.
