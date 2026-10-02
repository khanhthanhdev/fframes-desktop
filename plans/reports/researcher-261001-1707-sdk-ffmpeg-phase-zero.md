# Phase 0 research: managed SDK and FFmpeg

Date: 2026-10-01
Scope: Rust/fframes project compilation and FFmpeg setup for Windows x64, Linux x64, and macOS arm64.

## Decision

Use a versioned, app-owned SDK for the files the app can actually control, with a guided prerequisite check for the native toolchain. The initial supported local-build contract should be:

1. Pinned Rust toolchain, Cargo dependency cache/vendor tree, fframes release, generated-project template, fonts, and prebuilt Skia/FFmpeg artifacts are installed under app data and selected by a compatibility manifest.
2. The app downloads artifacts itself, verifies a pinned manifest and SHA-256 before unpacking, installs transactionally, supports cancellation/resume/retry, and then invokes Cargo with a fully constructed environment. Signed manifests belong to the later release/update gate.
3. Windows x64 requires the MSVC linker/Windows SDK and LLVM/libclang from the host (or a separately qualified installer path), plus a shared FFmpeg 9 installation. Linux x64 and macOS arm64 use the wrapper’s static prebuilt FFmpeg where the feature key exists, but codec builds still require external codec libraries unless those are separately supplied and qualified.
4. Advertise CPU preview and a tested codec profile first. Treat Skia GPU, H.264/H.265 licensing, and Windows MP4 export as qualification gates rather than consequences of a successful Rust compile.

This is the best fit for the current contracts. A fully terminal-free local workflow is not proven for any target until a clean-user spike exercises the exact generated project, feature set, linker, renderer, media, and export path.

## What the repository actually requires

The workspace pins `ffmpeg-sys-fframes = "9.0.0"` ([`Cargo.toml:103-105`](../../Cargo.toml#L103-L105)); `fframes-media` uses the dependency on native non-WASM targets ([`fframes-media/Cargo.toml:26`](../../fframes-media/Cargo.toml#L26)). On non-Windows/non-WASM/doc builds it adds `build` and `static` ([`fframes-media/Cargo.toml:41-47`](../../fframes-media/Cargo.toml#L41-L47)). The public fframes contract says that Linux/macOS use statically linked FFmpeg, download a feature-matching prebuilt, and fall back to source; codec features still link system x264/x265/etc. ([`fframes/src/lib.rs:93-101`](../../fframes/src/lib.rs#L93-L101)).

The `build-portable` feature omits `-march=native`/`-mtune=native` for source builds ([`README.md:210-229`](../../README.md#L210-L229)). It should be enabled for every managed SDK build that may be reused on another machine. Without it, restoring a native-CPU FFmpeg cache can cause `SIGILL` ([`README.md:213-216`](../../README.md#L213-L216)).

The scaffold adds `h264` and `libav-agree-gpl` only under `cfg(not(windows))` ([`cargo-fframes/src/main.rs:498-507`](../../cargo-fframes/src/main.rs#L498-L507)); its Windows path therefore does not provide the same codec contract. The current Windows CI intentionally builds only core packages because examples request source codec features ([`.github/workflows/main.yml:329-333`](../../.github/workflows/main.yml#L329-L333)), then runs a limited core/e2e smoke path ([`.github/workflows/main.yml:357-365`](../../.github/workflows/main.yml#L357-L365)). This is evidence for the setup mechanics, not for a Windows consumer export path.

## Upstream wrapper behavior (9.0.0)

The checked-in Cargo registry source is the published `ffmpeg-sys-fframes 9.0.0` package; its manifest identifies the upstream repository as [`dmtrKovalenko/rust-ffmpeg-sys`](https://github.com/dmtrKovalenko/rust-ffmpeg-sys) and Cargo.lock records checksum `cfc8123240ea935f4d766323979843650afd6d00b475c561d3bb8a59a2e6e31f` ([`Cargo.lock:1091-1097`](../../Cargo.lock#L1091-L1097)). The same source is published as the [crate source on docs.rs](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/).

Relevant source behavior in the inspected published package (`/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ffmpeg-sys-fframes-9.0.0/`), cross-checked against its [official docs.rs source](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/):

- `build_prebuilt.rs` defines the default archive URL as `https://github.com/dmtrKovalenko/rust-ffmpeg-sys/releases/download/{tag}/ffmpeg-{key}.tar.gz`, with tag `binaries-9.0.0`; `key` is the target plus sorted enabled features ([`build_prebuilt.rs:30-31,49-80`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build_prebuilt.rs)).
- A prebuilt archive must contain `lib/`, `include/`, and `extralibs.txt` ([`build_prebuilt.rs:82-146`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build_prebuilt.rs)). `extralibs.txt` carries FFmpeg’s `EXTRALIBS` lines, so a static archive can still depend on external codec/system libraries ([`build_prebuilt.rs:8-10,189-198`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build_prebuilt.rs)).
- Downloads use `curl --fail --location --retry 3`, write to a per-process `.partial-*` file, and rename only after completion; a failed extraction deletes the cached archive ([`build_prebuilt.rs:112-170`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build_prebuilt.rs)). This is useful recovery behavior, but the wrapper itself does not verify a checksum or signature.
- The default cache is `~/Library/Caches/ffmpeg-sys-fframes` on macOS, `%LOCALAPPDATA%/ffmpeg-sys-fframes` on Windows, and `$XDG_CACHE_HOME` or `~/.cache/ffmpeg-sys-fframes` on Linux ([`build_prebuilt.rs:172-187`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build_prebuilt.rs)). An app SDK should set `FFMPEG_BINARIES_CACHE` to its own versioned SDK cache rather than silently sharing a mutable global cache.
- Prebuilt use is bypassed by `FFMPEG_FORCE_BUILD`, either CPU tuning variable, or `FFMPEG_BINARIES_EXPORT_DIR` ([`build_prebuilt.rs:82-110`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build_prebuilt.rs)). The source fallback clones FFmpeg’s `release/9.0` branch and runs configure/make/install ([`build.rs:163-187,310-356,782-801`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)).
- Source builds force `--enable-static --disable-shared`, disable autodetection/programs/docs, and enable selected libraries from Cargo features ([`build.rs:530-555`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)). `build-portable` suppresses native CPU flags; otherwise the default is `native` for both flags ([`build.rs:399-437`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)).

### Windows ABI and layout

When `build` is absent, the build script checks `FFMPEG_DIR` first, then vcpkg on MSVC, then pkg-config ([`build.rs:1213-1320`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)). `FFMPEG_DIR` contributes `<dir>/lib` (or architecture subdirectories) to the linker and `<dir>/include` to bindgen ([`build.rs:1214-1244`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)). With the non-static Windows contract, libraries are emitted as `dylib` links ([`build.rs:1066-1086`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)); the runtime DLL directory is therefore a separate deployment requirement.

The build script temporarily adds its source directory to `PATH`/`INCLUDE` for a source build and runs `sh` for the Windows configure path ([`build.rs:310-350`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)). For the supported prebuilt Windows route, the repository explicitly puts FFmpeg `bin` on `PATH` because DLLs are loaded both during compilation/tests and when running ([`README.md:263-277`](../../README.md#L263-L277); [workflow:338-348](../../.github/workflows/main.yml#L338-L348)). A correct SDK record must therefore contain the target triple, MSVC import-library ABI, header version, DLL directory, and a smoke test that loads every linked library.

The vcpkg branch calls `vcpkg::find_package("ffmpeg")`; for dynamic linking it sets `VCPKGRS_DYNAMIC=1` ([`build.rs:806-824`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)). Microsoft’s current vcpkg guidance uses `VCPKG_ROOT` and `PATH`, bootstraps with `bootstrap-vcpkg.bat`, and recommends a manifest/baseline for repeatable versions ([Microsoft vcpkg setup](https://learn.microsoft.com/en-us/vcpkg/get_started/get-started-vs), [install reference](https://learn.microsoft.com/en-us/vcpkg/commands/install)). The app should prefer its own pinned `<sdk>/vcpkg` or a validated `FFMPEG_DIR`; it should not mutate a user’s global vcpkg tree.

### Bindgen and host toolchains

The crate generates bindings at build time with bindgen and includes FFmpeg headers ([`build.rs:1665-1690,1920-1933`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)). The bindgen guide requires Clang/libclang 9+ and says Windows needs `LIBCLANG_PATH` pointing at LLVM’s `bin`; it documents `libclang-dev` for Debian and `brew install llvm` for macOS ([bindgen requirements](https://rust-lang.github.io/rust-bindgen/requirements.html)). The README’s Windows recipe is therefore concrete: `winget install LLVM.LLVM`, set `FFMPEG_DIR`, `LIBCLANG_PATH`, and prepend FFmpeg `bin` to `PATH` ([`README.md:263-277`](../../README.md#L263-L277)).

Rust’s MSVC target also needs a linker, libraries, and Windows API import libraries from Visual Studio ([rustup MSVC prerequisites](https://rust-lang.github.io/rustup/installation/windows-msvc.html)). The minimal official component set is MSVC v143 x64/x86 build tools plus a Windows SDK; the current WinGet example is:

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools --source winget --force `
  --override "--add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.22621 --addProductLang En-us"
winget install LLVM.LLVM
```

Rust itself is installed with rustup; use the pinned toolchain from the SDK manifest. Do not claim the app can redistribute the Windows SDK or MSVC toolchain without a separate Microsoft licensing and installer qualification.

On macOS, the Rust installation guide requires a linker and recommends `xcode-select --install` for the C compiler ([Rust installation](https://rust-lang.github.io/book/ch01-01-installation.html)). The repository’s source fallback additionally documents Homebrew `pkg-config ffmpeg x264 x265 opus nasm ninja` ([`README.md:231-236`](../../README.md#L231-L236)). On Debian-like Linux, the documented fallback is `yasm nasm ffmpeg libx264-dev libx265-dev libopus-dev libclang-dev clang ninja-build libvpx-dev libasound2-dev` ([`README.md:240-251`](../../README.md#L240-L251)); CI installs the analogous packages plus graphics/Wayland libraries ([`.github/workflows/main.yml:133-150`](../../.github/workflows/main.yml#L133-L150)). These are developer/source-build dependencies, not ordinary consumer runtime dependencies.

## App-controlled setup contract

The setup service should resolve a manifest such as `(app version, SDK id, target triple, Rust toolchain, fframes version, FFmpeg tag/key, Skia revision, codec profile, artifact URLs, SHA-256, license notices, minimum disk space)`. It should:

1. Probe architecture, OS version, Rust target, linker, Windows SDK/MSVC, LLVM/libclang, `pkg-config`, and GPU backend before downloading. Report each item as installed, app-managed, host-managed, or blocked.
2. Download to a temporary SDK staging directory with progress, cancellation, HTTP resume where available, and a per-artifact lock. Verify the pinned checksum before extraction. Keep an offline bundle option containing the manifest, all archives, Rust dependencies, and notices. Signature verification is a release/update concern for M7.
3. Extract into a new versioned directory, validate headers/import libraries/static archives/DLLs, run a tiny bindgen/link/load probe, then atomically switch the active SDK pointer. Never replace an active SDK in place.
4. On cancel or failure, remove only the staging transaction and preserve the last usable SDK. On rerun, reuse verified artifacts and resume incomplete downloads; a corrupted cache entry is quarantined and redownloaded.
5. Invoke Cargo with `FFMPEG_BINARIES_CACHE=<sdk cache>` and, for offline builds, `CARGO_NET_OFFLINE=true` plus the SDK’s vendor/registry configuration. Avoid the wrapper’s unverified network download in the normal app flow; its `FFMPEG_BINARIES_URL=file://...` override is suitable for a validated local artifact.
6. Persist the active SDK id in project metadata and retain one previous SDK for rollback. Do not put SDK state in the portable project directory.

Cancellation must be implemented by the app’s worker supervisor around the Cargo process and download process. The current FFmpeg build script has no app-level cancellation protocol; killing a build may leave Cargo `OUT_DIR` debris, which is harmless only when the app keeps builds isolated and rerunnable.

## Developer, consumer, and OS-admin boundaries

| Layer | App can manage | Host/admin dependency in the current contract | Qualification risk |
| --- | --- | --- | --- |
| Developer SDK | Rust toolchain, Cargo cache/vendor tree, generated templates, fframes/FFmpeg/Skia artifacts, fonts, codec profile, manifests/notices, worker build cache | None in principle for files; compiler still invokes host linker and bindgen | Exact Cargo features and target must match the archive key; precompiled Rust artifacts are cache entries, not a plugin ABI. |
| Windows x64 build host | FFmpeg shared archive in app data; `FFMPEG_DIR`; DLL search path; app-owned vcpkg if selected | MSVC linker/import libs/Windows SDK; LLVM/libclang; potentially user/admin rights to install them | Shared `.dll` + `.lib` + headers must be ABI-compatible with `x86_64-pc-windows-msvc`; current CI does not prove codec export. |
| Linux x64 build host | Static FFmpeg archive where the exact key exists; Rust and app caches | GCC/Clang linker; libclang; external x264/x265/opus/vpx dev libraries for enabled codec features; distro graphics/audio runtime for GPU | “Static FFmpeg” is not self-contained when `extralibs.txt` names external libraries. |
| macOS arm64 build host | Static FFmpeg archive where the key exists; Rust and app caches; possibly prebuilt Skia | Xcode Command Line Tools/macOS SDK/linker; external codec libraries for enabled codecs | Xcode/SDK redistribution and Homebrew dependency policy need separate qualification; Metal preview is unproven by this task. |
| Consumer runtime | Prebuilt worker, FFmpeg/Skia runtime libraries, fonts, notices, signed updates | OS graphics/audio/runtime facilities | Runtime can be terminal-free for preview/render of an existing accepted revision and the exact prebuilt worker/codec matrix; authoring arbitrary new Rust source still requires the build SDK. |

## Platform blocker matrix

| Target | Evidence-backed baseline | Phase 0 status | Blocker to clear |
| --- | --- | --- | --- |
| Windows x64 MSVC | Shared FFmpeg 9 through `FFMPEG_DIR` or vcpkg; LLVM/libclang; MSVC/Windows SDK; CPU backend is the lowest native surface | **Core build candidate; export blocked** | Clean machine with pinned MSVC + LLVM + shared FFmpeg, DLL load probe, generated CPU project, and explicit MP4 codec test. Do not enable `h264`, `h265`, etc.; repository says those request source builds unsupported on Windows. |
| Linux x64 | Static feature-matching prebuilt is supported by wrapper release policy; source fallback documented; portable mode available | **Conditional candidate** | Prove exact SDK archive and codec profile. Either bundle/qualify external codec libraries and pkg-config metadata or restrict the profile to codecs that are demonstrably self-contained. Validate linker, fonts, audio, and CPU fallback on a clean distro. |
| macOS arm64 | Wrapper README names macOS arm64 prebuilt archives; fframes links statically; source fallback uses Xcode/Homebrew-style tools | **Conditional candidate** | Clean Apple Silicon test with Xcode CLT, exact FFmpeg key, external codec profile, signing/notices, and CPU/Metal preview. Do not infer support from Linux CI or a successful Cargo check. |
| All three, arbitrary user project | Generated Rust source must compile against exact crate/features/native ABI | **Not terminal-free yet** | M0 spike must produce a real frame, seek repeatedly, and export a qualified file from a clean user account. Otherwise offer guided prerequisites or a remote build path, as the desktop architecture requires. |

## Codec and packaging qualification

The initial profile should separate “decode/render smoke” from “export codecs”. The generated scaffold’s measured non-Windows profile is `h264` plus `libav-agree-gpl`; that requests GPL-enabled FFmpeg and `libx264` rather than an LGPL-only build. The workspace also exposes `h265`, `vpx`, and related codec features, but the scaffold does not enable those by default ([`fframes/Cargo.toml:79-91`](../../fframes/Cargo.toml#L79-L91); [`cargo-fframes/src/main.rs:498-507`](../../cargo-fframes/src/main.rs#L498-L507)). The wrapper’s source build turns codec features into flags such as `--enable-libx264` ([`build.rs:602-639`](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/build.rs)). Any app SDK profile must record that licensing choice and measure its actual output; it must not silently replace the generated profile.

FFmpeg’s legal page states that optional GPL parts change the license obligations and recommends dynamic linking, source correspondence, and notices for an LGPL distribution; it also calls out GPL codecs such as libx264 ([FFmpeg legal guidance](https://ffmpeg.org/legal.html)). The repository currently statically links Linux/macOS FFmpeg, so the SDK record must preserve the corresponding source/configuration/notice data for the measured profile; release packaging is the later qualification gate. FFmpeg’s own site says it provides source and points to third-party Windows/Linux/macOS builds, so BtbN is a third-party supply chain, not an FFmpeg-maintained binary ([FFmpeg downloads](https://ffmpeg.org/download.html)). BtbN’s release page currently lists FFmpeg 9.0 x64 Windows shared/static GPL and LGPL artifacts and SHA-256 checksums ([BtbN latest release](https://github.com/BtbN/FFmpeg-Builds/releases/tag/latest)); pin an immutable release asset and checksum, never the moving `latest` name.

Package boundaries should be:

- app installer: signed UI/worker, platform runtime libraries, fonts/icons, compatibility manifest, notices;
- managed SDK bundle: Rust toolchain, source/vendor dependencies, headers/libs, build configuration, validated FFmpeg/Skia archives, and license/source offer data;
- project: Rust source, media, `studio.json`, and SDK id only;
- host prerequisites: OS-admin-installed MSVC/Windows SDK or Xcode/Linux build tools unless a separately licensed and tested installer path exists.

## Ranked approaches

| Rank | Approach | Fit | Trade-off/adoption risk |
| --- | --- | --- | --- |
| 1 | Guided host prerequisites + app-managed pinned SDK/artifacts | Best fit now | Requires honest setup UI and admin steps; low implementation risk because it follows current fframes contracts. |
| 2 | Fully app-managed local native toolchain | Better UX if legally/technically possible | Windows SDK/MSVC and Apple SDK redistribution, linker ABI, and Linux distro integration are unresolved; high qualification risk. |
| 3 | Remote build for platforms blocked by local prerequisites | Strong terminal-free fallback | Requires service, source/media transfer, trust/privacy, queueing, and cost; outside current local architecture. |
| 4 | Ship only a prebuilt worker/runtime | Simplest consumer install | Cannot satisfy arbitrary agent-generated Rust projects; useful only as a restricted export/preview mode. |

## Limitations and unresolved questions

- No clean Windows, Linux, or Apple machine was available for this report; no artifact was downloaded, no DLL/static ABI was executed, and no codec output was measured.
- The exact `binaries-9.0.0` feature archives and their `extralibs.txt` contents need to be enumerated for the chosen SDK profiles; repository source establishes the lookup algorithm, not availability of every key.
- Skia’s own prebuilt/runtime and GPUI/Metal/Vulkan prerequisites were outside this FFmpeg-focused pass; CPU rendering is the only low-risk first spike.
- Product/release owners still need to choose the LGPL-only versus GPL codec profile and carry the corresponding source, notice, and patent information into packaging.

## Sources

Primary project/upstream sources: [fframes README](../../README.md), [fframes-media manifest](../../fframes-media/Cargo.toml), [fframes manifest](../../fframes/Cargo.toml), [cargo-fframes generator](../../cargo-fframes/src/main.rs), [Windows CI](../../.github/workflows/main.yml), [desktop architecture](../../docs/desktop/architecture.md), [desktop implementation plan](../../docs/desktop/implementation-plan.md), [ffmpeg-sys-fframes upstream](https://github.com/dmtrKovalenko/rust-ffmpeg-sys), [published crate source](https://docs.rs/crate/ffmpeg-sys-fframes/9.0.0/source/), [bindgen requirements](https://rust-lang.github.io/rust-bindgen/requirements.html), [Rust MSVC prerequisites](https://rust-lang.github.io/rustup/installation/windows-msvc.html), [Microsoft vcpkg setup](https://learn.microsoft.com/en-us/vcpkg/get_started/get-started-vs), [FFmpeg legal guidance](https://ffmpeg.org/legal.html), [FFmpeg download/verification guidance](https://ffmpeg.org/download.html), and [BtbN FFmpeg 9.0 artifacts/checksums](https://github.com/BtbN/FFmpeg-Builds/releases/tag/latest).

Status: DONE
Summary: Repository and upstream 9.0.0 build behavior were verified; the report defines an app-managed SDK contract, host/admin boundary, concrete setup commands, recovery/checksum policy, codec qualification boundary, and platform blocker matrix.
Concerns/Blockers: Clean-machine native builds, exact prebuilt archive keys, Windows DLL/import-library ABI, GPU renderer packaging, and codec licensing/export remain empirical Phase 0 gates.
