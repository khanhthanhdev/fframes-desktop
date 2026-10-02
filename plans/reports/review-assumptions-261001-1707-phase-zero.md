# Phase 0 plan assumption and contract review

Review date: 2026-10-01. This is a plan review only. I did not edit the plan,
install packages, or run builds. Local source references use `file:line` text so
the report remains portable.

## Verdict

The phase ordering and Linux-first decision are technically coherent. The plan
should proceed with five qualification findings resolved in the implementation
spikes. The GPUI choice remains a candidate: the official GPUI README describes
the project as pre-1.0 and subject to breaking changes, so the pinned revision
must remain qualification-gated.

## Findings requiring action

### High — GPUI BGRA does not establish the planned alpha and row contract

**Location:** `phase-01-start.md:23,55,75-76` and `phase-03-feasibility-and-platform-qualification.md:49-65`.

**Evidence:** The official [GPUI asset source](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/assets.rs)
documents `RenderImage` byte access, dimensions, and BGRA storage. It does not
provide the pinned revision's complete public contract for premultiplied versus
straight alpha, row stride, or upload lifetime. The local renderer explicitly
returns straight RGBA and unpremultiplies tiny-skia data in
`fframes/src/renderer/preview.rs:16-51,121-160`.

**Failure scenario:** A half-transparent reference pixel is uploaded with the
wrong alpha interpretation or row layout. The window still displays an image,
but edges have halos or dark fringes; a byte comparison of only opaque colors
passes.

**Required fix:** On the exact pinned revision, make a native readback or
screenshot test containing transparent, half-transparent, opaque, and padded
rows. Record channel order, alpha mode, stride, and ownership/release behavior
as an executable contract. Keep the 2x2 test in the acceptance evidence.

### High — The Windows FFmpeg prerequisite route is unresolved

**Location:** `phase-02-managed-sdk-and-project-setup.md:22-23,62-64` and
`phase-03-feasibility-and-platform-qualification.md:120-125`.

**Evidence:** The repository's `fframes-media/Cargo.toml:34-46` says that the
Windows path uses a prebuilt FFmpeg 9 distribution through `FFMPEG_DIR` or
vcpkg, while also stating that building it needs an “MSYS2 + MSVC toolchain.”
The plan's Windows matrix requires Visual Studio/MSVC/CMake/LLVM and explicitly
rejects MinGW/MSYS2 as an ABI. The generator confirms the Windows dependency
path omits the non-Windows codec feature block at `cargo-fframes/src/main.rs:498-507`.

**Failure scenario:** The doctor passes on a clean Windows account because
`cl.exe` and the SDK exist, but a missing prebuilt FFmpeg layout causes the
build script to seek a shell/tool or a different vcpkg layout. Project A then
fails before the GUI gate, despite the documented host probes passing.

**Required fix:** Choose and document one supported Windows route: verified
FFMPEG_DIR/vcpkg prebuilt inputs, or a source-build route with the exact MSYS2
tools. Clarify whether MSYS2 is an auxiliary host shell or prohibited entirely;
then run the generated CPU project from a clean Developer PowerShell and record
headers, import libraries, DLLs, `LIBCLANG_PATH`, and child DLL search paths.

### High — Offline setup can fall back to a network/source build

**Location:** `phase-02-managed-sdk-and-project-setup.md:90,108-121` and
`phase-03-feasibility-and-platform-qualification.md:129-150`.

**Evidence:** The exact registry source used by this checkout documents the
behavior in `ffmpeg-sys-fframes-9.0.0/build_prebuilt.rs:82-107,112-169`: a
missing or invalid prebuilt archive falls back to compiling from source, and
the download helper invokes `curl`. The build entry point at
`ffmpeg-sys-fframes-9.0.0/build.rs:1124-1139` calls that fallback before source
fetch/build. `file://` is supported (`build_prebuilt.rs:12-17,93-99`), but it
does not itself verify the archive checksum or make a missing key fatal.

**Failure scenario:** The app cache contains the wrong feature/target key or no
archive. Project B is marked offline, but the build script removes the failed
cache entry and attempts a source fetch/build; the run either unexpectedly
touches the network or fails with an opaque missing-source error.

**Required fix:** Preflight the exact archive key, tag, checksum, `lib/`,
`include/`, and `extralibs.txt` before Cargo starts. In the offline harness,
make a missing/mismatched archive fail closed and audit child processes for
network attempts. Do not treat `CARGO_NET_OFFLINE` alone as proof of this
property.

### Medium — The resident-image counter is not a GPU-resource proof

**Location:** `phase-01-start.md:55,75-82,104,113-115`.

**Evidence:** The planned counter is app-owned. Official Zed code in
[`image_viewer.rs`](https://github.com/zed-industries/zed/blob/main/crates/image_viewer/src/image_viewer.rs)
explicitly releases the window image and removes the source asset when an image
is replaced. The plan does not yet name the corresponding release operation on
the pinned GPUI API.

**Failure scenario:** The counter returns to one while old atlas/GPU resources
remain retained by a GPUI asset cache. The 1,000-replacement gate passes while
the process's graphics memory grows.

**Required fix:** Track actual pinned-GPUI image IDs and release calls, then
sample process/graphics memory before and after replacements. If the revision
offers no observable GPU metric, rename the gate to logical ownership and mark
GPU eviction as an unresolved qualification result.

### Medium — CLI-frame comparison is not independent worker evidence

**Location:** `phase-03-feasibility-and-platform-qualification.md:49-65,129-136`.

**Evidence:** The CLI `frame` path creates `Previewer` and a default
`CpuFrameRenderer` in `fframes/src/renderer/cli.rs:375-383`. `Previewer::render`
returns the same `RgbaFrame` path in `fframes/src/renderer/preview.rs:434-443`.

**Failure scenario:** The worker and CLI share the same conversion bug, or the
worker test compares metadata while bypassing the actual binary frame payload.
Both outputs agree even though GPUI receives incorrect bytes or the transport
truncates a row.

**Required fix:** Add an independent boundary fixture: known byte-level RGBA
frames enter the worker protocol, are validated after transport, and are
checked after GPUI presentation/readback. Keep CLI comparison as a secondary
semantic check, not the sole renderer-worker gate.

## Ten-claim verification ledger: Phase 1

| # | Claim and plan location | Status and evidence |
|---:|---|---|
| 1 | Root workspace is resolver 2 and has no GPUI (`phase-01-start.md:20`) | **VERIFIED.** `Cargo.toml:1-51` has resolver 2, explicit members, and no GPUI. |
| 2 | A nested desktop workspace can be isolated (`phase-01-start.md:20,29-36`) | **FUTURE.** Architecture requires this at `docs/desktop/architecture.md:72-93`; no desktop manifest exists to verify exclusion. |
| 3 | GPUI has a standalone application entry point (`phase-01-start.md:62-63`) | **VERIFIED.** Official [hello-world](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/examples/hello_world.rs) uses `gpui_platform::application()`. |
| 4 | Linux platform selection exposes X11 and Wayland (`phase-01-start.md:22,82`) | **VERIFIED.** Official [GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md) and [platform manifest](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_platform/Cargo.toml) document the features. Native selection remains future. |
| 5 | macOS uses Metal/font-kit and Windows uses native Win32/DirectWrite (`phase-01-start.md:22`) | **VERIFIED.** Official GPUI README documents those platform paths; native runs remain future. |
| 6 | One exact Zed revision is needed for the two GPUI packages (`phase-01-start.md:21,62`) | **VERIFIED.** Official [gpui manifest](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/Cargo.toml) and platform manifest resolve sibling crates from the Zed source tree. The proposed commit is still unqualified. |
| 7 | GPUI is pre-1.0 and latest-stable-Rust guidance is not a compatibility guarantee (`phase-01-start.md:21`) | **VERIFIED.** Official GPUI README says it is pre-1.0 and may break between versions. |
| 8 | GPUI exposes marked text, UTF-16 selection/replacement, and caret configuration (`phase-01-start.md:63-65`) | **VERIFIED API; FUTURE runtime.** Official [input source](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/input.rs) contains the contract; platform IME behavior needs native tests. |
| 9 | GPUI can present an image resource (`phase-01-start.md:66-76`) | **VERIFIED API; FUTURE conversion.** Official [image element](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/elements/img.rs) accepts image/resource forms; channel/alpha proof remains Finding 1. |
| 10 | fframes Previewer keeps caches and returns `RgbaFrame` (`phase-01-start.md:75-82`) | **VERIFIED.** `fframes/src/renderer/preview.rs:237-299,434-443` shows persistent preview state and the render return type. |

## Ten-claim verification ledger: Phase 2

| # | Claim and plan location | Status and evidence |
|---:|---|---|
| 1 | The generator can create a standalone project (`phase-02-managed-sdk-and-project-setup.md:20-21`) | **VERIFIED.** `cargo-fframes/src/main.rs:452-477` emits its own workspace and exact release dependency. |
| 2 | The CPU profile does not select Skia (`phase-02-managed-sdk-and-project-setup.md:20-21`) | **VERIFIED.** `cargo-fframes/src/main.rs:481-486` maps `Backend::Cpu` to the built-in CPU backend. |
| 3 | A CPU project still consumes native FFmpeg through fframes-media (`phase-02-managed-sdk-and-project-setup.md:20-23`) | **VERIFIED.** `fframes-media/Cargo.toml:25-46` declares `ffmpeg-sys-fframes`; CPU selection alone does not remove that native build. |
| 4 | Non-Windows generated projects request H.264/GPL features (`phase-02-managed-sdk-and-project-setup.md:21-22`) | **VERIFIED.** `cargo-fframes/src/main.rs:498-507` emits `h264` and `libav-agree-gpl` under `cfg(not(windows))`. |
| 5 | Windows uses shared/prebuilt FFmpeg inputs rather than that codec feature block (`phase-02-managed-sdk-and-project-setup.md:22-23`) | **VERIFIED with route concern.** `fframes-media/Cargo.toml:34-46` and the generator source support the distinction; exact clean-host route is Finding 2. |
| 6 | Prebuilt FFmpeg archives are keyed by target and enabled features (`phase-02-managed-sdk-and-project-setup.md:48-54,90`) | **VERIFIED.** `ffmpeg-sys-fframes-9.0.0/build_prebuilt.rs:49-79` constructs tag/key values. |
| 7 | A local `file://` archive source is supported (`phase-02-managed-sdk-and-project-setup.md:90`) | **VERIFIED API.** `build_prebuilt.rs:12-17,93-99` accepts the URL override, including `file://`. Checksum enforcement is app-owned and future. |
| 8 | A cache miss is not fail-closed today (`phase-02-managed-sdk-and-project-setup.md:90,108-121`) | **VERIFIED.** `build_prebuilt.rs:99-107` falls through to source compilation; `build.rs:1131-1139` fetches/builds. This is Finding 3. |
| 9 | App-managed Rust/Cargo/vendor/native artifacts are an intended architecture, not an existing repository capability (`phase-02-managed-sdk-and-project-setup.md:18-23,48-56`) | **FUTURE.** `docs/desktop/architecture.md:355-365` states the target design; no SDK manager or compatibility manifest exists yet. |
| 10 | Clean-account online project A, fresh offline project B, cancellation, and rollback are qualification experiments (`phase-02-managed-sdk-and-project-setup.md:118-154`) | **FUTURE.** The plan labels these as proposed checks; no implementation or native evidence exists. |

## Ten-claim verification ledger: Phase 3

| # | Claim and plan location | Status and evidence |
|---:|---|---|
| 1 | A persistent Previewer can retain render state (`phase-03-feasibility-and-platform-qualification.md:18-23,49-65`) | **VERIFIED API.** `fframes/src/renderer/preview.rs:237-299` owns caches/decoders, and `:434-443` renders through the persistent object. |
| 2 | The CPU renderer produces straight RGBA after premultiplied rasterization (`phase-03-feasibility-and-platform-qualification.md:49-65`) | **VERIFIED.** `fframes/src/renderer/preview.rs:16-51,121-160`. |
| 3 | The worker is intended for crash isolation, not security isolation (`phase-03-feasibility-and-platform-qualification.md:18-23`) | **VERIFIED design constraint.** `docs/desktop/architecture.md:66-70` says process separation is not a sandbox. |
| 4 | Length-delimited control and bounded binary frame transport are the proposed contract (`phase-03-feasibility-and-platform-qualification.md:49-65`) | **FUTURE.** `docs/desktop/architecture.md:303-319` states the design; no protocol implementation exists. |
| 5 | Latest-wins seeks and generation/request rejection are required (`phase-03-feasibility-and-platform-qualification.md:63-65`) | **FUTURE.** The architecture specifies the behavior at `docs/desktop/architecture.md:309-317`; queue bounds and supersession latency need measurement. |
| 6 | A CLI frame comparison is available as a secondary semantic check (`phase-03-feasibility-and-platform-qualification.md:113-122`) | **VERIFIED path; insufficient as sole proof.** `fframes/src/renderer/cli.rs:375-383` uses `Previewer`/CPU rendering; see Finding 5. |
| 7 | Explicit source registration/selection is an architectural requirement (`phase-03-feasibility-and-platform-qualification.md:69-81`) | **FUTURE.** `docs/desktop/architecture.md:213-229` specifies the authority model; current fframes sources do not expose the planned metadata protocol. |
| 8 | Marker/hash/path/span validation can reject stale or ambiguous anchors (`phase-03-feasibility-and-platform-qualification.md:69-81,131`) | **FUTURE experiment.** The plan defines negative cases; no implementation or fixture result exists. |
| 9 | ACP completion/cancellation requires a real authenticated provider (`phase-03-feasibility-and-platform-qualification.md:85-92`) | **FUTURE experiment.** The plan correctly requires real negotiation, edit, completion, permission/input, and cancellation evidence; registry presence is not support. |
| 10 | Linux, Windows, and macOS M0 status requires native setup/runtime evidence (`phase-03-feasibility-and-platform-qualification.md:120-125,154-162`) | **FUTURE.** The matrix is a qualification contract; no target has been built or run in this review. |

## Official-source basis and limitations

Primary GPUI sources reviewed were the [GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md),
[gpui manifest](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/Cargo.toml),
[gpui_platform manifest](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_platform/Cargo.toml),
[hello-world example](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/examples/hello_world.rs),
[input implementation](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/input.rs),
[image element](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/elements/img.rs),
[asset implementation](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/assets.rs), and
official [Linux](https://github.com/zed-industries/zed/blob/main/docs/src/development/linux.md),
[Windows](https://github.com/zed-industries/zed/blob/main/docs/src/development/windows.md), and
[macOS](https://github.com/zed-industries/zed/blob/main/docs/src/development/macos.md) development guides.
These establish APIs, dependency intent, and host prerequisites; they do not
prove the proposed commit, this repository's native dependency graph, GPU
eviction behavior, IME correctness, or clean-machine success. Those remain
explicit future experiments in the ledgers above.

Status: DONE_WITH_CONCERNS
Summary: The plan is factually grounded and its 30 phase claims are marked verified or future qualification. Five actionable findings must be addressed before treating M0 evidence as conclusive, especially the Windows FFmpeg route, offline fail-closed behavior, and independent frame-boundary validation.
Concerns: No native build/runtime or clean-account setup was run; GPUI revision compatibility, alpha/lifetime semantics, Windows FFmpeg setup, GPU eviction, ACP, and all platform gates remain unqualified.
