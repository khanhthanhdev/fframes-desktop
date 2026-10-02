# Desktop feasibility and native qualification

Date: 2026-10-01. Status: implementation available; Phase Zero qualification **PENDING**.

The isolated desktop workspace connects managed setup, an external annotated renderer, image presentation, source inspection and a configurable ACP adapter session. Linux X11 software rendering has reproducible unprivileged, network-isolated install/build, presentation, keyboard and source-click evidence. Physical-display/IME, Windows/macOS and authenticated provider qualification remain open. A software-rendered X11 run does not qualify every native platform.

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
