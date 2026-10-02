# GPUI Phase 0 feasibility report

Research date: 2026-10-01. This report covers the requested local files and
current primary sources from Zed/GPUI and Rust. No packages were installed and
no build was run during this research pass.

## Decision

GPUI is a credible Phase 0 choice for Studio's native shell. Its upstream
README documents a standalone `gpui_platform::application()` entry point,
native window creation, platform text backends, and image elements. The same
README also says GPUI is pre-1.0, actively changing, and may break between
versions. Treat this as a pinned, qualification-gated dependency rather than a
stable application platform.

Use Linux x64 as the first development and integration host, preserving the
plan's target order. Qualify one exact Zed revision for `gpui` and
`gpui_platform` together, commit the desktop workspace lockfile, and do not
mix a crates.io `gpui` with a git or different-revision platform crate. A source-listed candidate appears in the official Zed commit history on
2026-09-10 (project-panel fix):

```text
https://github.com/zed-industries/zed/commit/1a84d5d92bd7d6c1cabb116062650af545783fe9
```

Provenance: the [official commit history](https://github.com/zed-industries/zed/commits/main/) lists short SHA `1a84d5d` and its commit-link target contains the full SHA above. The individual commit page and immutable raw files could not be fetched by the planning web tool. This is a listed historical candidate, not the current branch tip or a qualification result. Fetch the exact object and inspect its manifests/toolchain before adopting the pin; choose another source-verified revision if that check fails. Use a fork only if the spike
finds a concrete blocker and record the fork commit and patch set.

The proposed target roles in the local plan remain appropriate: Linux x64 for
development/qualification, macOS Apple Silicon and Windows x64 as primary
consumer targets, with other architectures deferred (implementation plan,
`docs/desktop/implementation-plan.md:196`).

## Evidence from this repository

- M0 explicitly requires a coherent GPUI revision, a native window, text input,
  IME and image presentation, plus clean-machine build evidence
  (`docs/desktop/implementation-plan.md:32`).
- The architecture keeps GPUI in a separate desktop workspace and keeps the
  project renderer in a GPUI-free worker process. It also requires one coherent
  GPUI revision and no GPUI dependency leakage into the existing core workspace
  (`docs/desktop/architecture.md:72`, `docs/desktop/architecture.md:89`).
- The architecture already specifies the important presentation boundary:
  frame dimensions, stride, channel order, alpha and color space must be
  explicit, with conversion at the GPUI boundary (`docs/desktop/architecture.md:313`).
- The existing workspace is a Rust resolver-2 workspace with fframes, Skia,
  native-player, FFmpeg bindings and many examples, but no GPUI dependency or
  desktop workspace (`Cargo.toml:1`). Its native rendering
  dependencies will need their own clean-machine qualification; GPUI alone does
  not establish that fframes projects build.
- Existing research correctly identifies GPUI's main-thread window ownership,
  renderer-worker need, and the risk of assuming managed compilation removes OS
  SDK/linker/native-library requirements (`docs/desktop/research.md:116`,
  `docs/desktop/research.md:136`).

## Current upstream facts

### Dependency and toolchain shape

The official [GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md)
says GPUI is pre-1.0, requires the latest stable Rust, and starts standalone
apps through `gpui_platform::application().run(...)`. It documents these
platform selections:

| Target | Upstream feature guidance | Source-level evidence | Phase 0 interpretation |
| --- | --- | --- | --- |
| Linux x64 | `gpui_platform`: `wayland`, `x11` (one or both) | [README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md), [platform manifest](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_platform/Cargo.toml) | Enable both for the first qualification so the app can exercise runtime selection. |
| macOS Apple Silicon | `gpui_platform`: `font-kit`; Metal is the renderer | [README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md) | Require the font feature; qualify a native `aarch64-apple-darwin` build and glyph rendering. |
| Windows x64 | No GPUI platform feature required; Win32 and DirectWrite | [README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md), [Windows platform crate](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_windows/Cargo.toml) | Build with the native MSVC toolchain and Windows SDK; do not use MinGW/MSYS2 for the advertised target. |

The current manifests show why a single revision matters. `gpui` is version
`0.2.2` and Apache-2.0, while `gpui_platform` is version `0.1.0` and Apache-2.0;
the platform crate selects sibling crates (`gpui_macos`, `gpui_windows`,
`gpui_linux`, and Linux `gpui_wgpu`) from the same Zed workspace
([gpui manifest](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/Cargo.toml),
[platform manifest](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_platform/Cargo.toml)).
Those version numbers alone are not a compatibility contract for a pre-1.0
framework. Use the exact git revision and `Cargo.lock` for the spike.

Upstream Zed currently pins its own development compiler to Rust `1.98.1` with
rustfmt, clippy, rust-analyzer and rust-src in
[rust-toolchain.toml](https://raw.githubusercontent.com/zed-industries/zed/main/rust-toolchain.toml).
That is a useful SDK candidate, but GPUI's README says “latest stable Rust”; the
desktop SDK should record the tested compiler after the spike rather than
silently inheriting a moving channel.

### Linux x64: X11, Wayland, graphics, fonts and audio

The official [Linux development guide](https://github.com/zed-industries/zed/blob/main/docs/src/development/linux.md)
says Zed supports both X11 and Wayland and selects the available compositor at
runtime. The current
[gpui_linux manifest](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_linux/Cargo.toml)
shows the relevant source dependencies:

- Wayland: `wayland-client`, `wayland-backend`, cursor/protocol crates,
  `calloop-wayland-source`, and `xkbcommon`'s Wayland path.
- X11: `x11rb` with XKB/XInput/DRI3 features, `xkbcommon`'s X11 path, and the
  `zed-xim` input method crate.
- Graphics: the Linux platform enables `gpui_wgpu`; the Zed setup script lists
  Vulkan loader packages.
- Fonts: the Linux platform enables `gpui_wgpu` with its `font-kit` feature;
  the Zed setup script lists Fontconfig development headers.
- Build/runtime support: the official [Zed Linux setup script](https://raw.githubusercontent.com/zed-industries/zed/main/script/linux)
  currently lists, among other full-Zed dependencies, `gcc`, `g++`, `cmake`,
  `clang`, `lld`, `llvm`, `libfontconfig-dev`, `libwayland-dev`,
  `libx11-xcb-dev`, `libxkbcommon-x11-dev`, `libvulkan1`, `libasound2-dev`,
  `pipewire`, and `xdg-desktop-portal` on Debian-like systems. This list is a
  full Zed setup list, not a proved minimum for a small GPUI app. Derive the
  Studio minimum by building the hello-window spike on a clean image and
  inspecting its dynamic dependencies.

The source proves compile-time/backend intent; it does not prove every Linux
distribution, compositor, GPU driver, font installation, or audio route. The
spike must run once under X11 and once under Wayland, and must include text
input, a loaded font, an image and a GPU/CPU fallback decision.

### Windows x64/MSVC

The official [Zed Windows guide](https://github.com/zed-industries/zed/blob/main/docs/src/development/windows.md)
requires rustup, Visual Studio or Build Tools with the Desktop development with
C++ workload, x64/x86 MSVC build tools and Spectre libraries, a Windows 10/11
SDK (at least `10.0.20348.0` in the current guide), and CMake. Build Tools must
be initialized through the Visual Studio developer shell; rustup alone does not
discover that installation. The same guide says Zed does not support unofficial
MSYS2 MinGW packages.

The Rust project identifies `x86_64-pc-windows-msvc` as the 64-bit Windows MSVC
target and documents that the MSVC target requires the Visual Studio host tools
([Rust MSVC platform support](https://doc.rust-lang.org/stable/rustc/platform-support/windows-msvc.html)).
This supports the plan's Windows x64 target, but GPUI/fframes integration still
needs a native run. Verify `cl`, `link`, `rc`, the Windows SDK, CMake, GPUI text
input, and the frame presentation path in a Developer PowerShell.

### macOS Apple Silicon

The official [Zed macOS guide](https://github.com/zed-industries/zed/blob/main/docs/src/development/macos.md)
requires rustup, Xcode with macOS components, Xcode command line tools, selecting
the Xcode developer directory, accepting the Xcode license, and CMake. It also
documents that on macOS 26 the Metal toolchain may need an explicit
`xcodebuild -downloadComponent MetalToolchain` after first launch. The GPUI
README says Metal is the renderer and `font-kit` is required for actual glyph
rasterization; without it GPUI can lay out text but render placeholders.

The source therefore supports the planned Apple Silicon shell direction. It
does not establish notarization, codec/FFmpeg redistribution, renderer parity,
or clean-user setup. The spike must use an Apple Silicon host and capture the
Xcode/Metal SDK paths, target triple, font rendering, image presentation and
worker build separately.

### Window, text input/IME and image feasibility

The official [hello-world example](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/examples/hello_world.rs)
creates an application with `gpui_platform::application()`, opens a native
window, registers a root view, and renders text and styled elements. The official
[GPUI input implementation](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/input.rs)
exposes an `EntityInputHandler` with marked-text ranges, replacement and marked
text operations, UTF-16 selection, caret bounds, character indexing and
`TextInputConfiguration`. This is sufficient evidence that an IME-capable input
contract exists; it is not evidence that Studio's composer works on all three
platforms. Composition tests (for example, Vietnamese/Japanese/Chinese input,
candidate-window placement, selection and paste) remain a Phase 0 acceptance
test.

The official [image element](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/elements/img.rs)
accepts resources, cached `RenderImage`/`Image` values and custom loaders. The
[asset definition](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/assets.rs)
states that `RenderImage` is cached in BGRA format and exposes byte access and
dimensions. fframes' architecture must therefore perform and test the
RGBA/alpha/stride conversion once at the presentation boundary; it must not
assume that a straight-RGBA worker buffer can be handed to GPUI unchanged.
`RenderImage` is feasible for a bounded static-frame spike. Sustained playback,
image eviction and GPU upload cost are still qualification work.

## Dependency matrix and ranked options

| Strategy | Reproducibility | Integration risk | Maintenance | Recommendation |
| --- | --- | --- | --- | --- |
| Git `gpui` + `gpui_platform` at one exact Zed `rev`, committed lockfile | High | Lowest among current options; all sibling crates resolve from one source snapshot | Must intentionally refresh and requalify | **1 — use for Phase 0** |
| Published crates.io versions selected independently | Medium | High for a pre-1.0 framework with separate package versions and rapid upstream changes | Easy initially, difficult to diagnose when APIs diverge | 2 — use only after a passing compatibility check |
| Local fork at one exact commit | High | High until the fork has a narrowly demonstrated fix | Highest; carries patch/merge burden | 3 — only for a measured blocker |

For the first desktop manifest, keep the separate workspace described by the
architecture and pin both packages to the same git revision. Use the documented
platform features (`font-kit` on macOS, `wayland` + `x11` on Linux, no extra
Windows feature), commit the lockfile, and record the tested Rust toolchain and
OS SDK versions in the compatibility manifest. Do not add GPUI to the current
root workspace until the desktop boundary is accepted.

## Executable Phase 0 checks

These commands are verification recipes; they do not claim that this workspace
already contains the desktop spike.

Common, after the desktop workspace exists:

```bash
rustup show active-toolchain
rustc -vV
cargo metadata --locked --manifest-path desktop/Cargo.toml
cargo tree --locked --manifest-path desktop/Cargo.toml -e features -i gpui
cargo tree --locked --manifest-path desktop/Cargo.toml -e features -i gpui_platform
cargo run --locked --manifest-path desktop/Cargo.toml -p gpui-spike
```

The spike must open a window, accept composed text, load one known font, display
one worker-produced image, and close cleanly. For the frame path, compare a
known RGBA color/alpha test frame before and after conversion, then replace the
frame repeatedly while reporting live queue length and resident image count.

Linux x64:

```bash
uname -m
echo "$XDG_SESSION_TYPE  DISPLAY=$DISPLAY  WAYLAND_DISPLAY=$WAYLAND_DISPLAY"
ldconfig -p | rg 'libX11|libxcb|libwayland|libxkbcommon|libfontconfig|libvulkan'
vulkaninfo --summary                 # when Vulkan tooling is available
```

Run the binary from a native X11 desktop session with only its X11 display
available, then from a native Wayland desktop session with only its Wayland
display available. Leaving both display variables available while assigning one
of them does not prove backend selection. If the pinned source exposes a
documented backend-selection API, use it and record that API; otherwise use the
native session environment as the qualification control:

```bash
XDG_SESSION_TYPE=x11 DISPLAY="$DISPLAY" WAYLAND_DISPLAY= \
  cargo run --locked --manifest-path desktop/Cargo.toml -p gpui-spike
XDG_SESSION_TYPE=wayland WAYLAND_DISPLAY="$WAYLAND_DISPLAY" DISPLAY= \
  cargo run --locked --manifest-path desktop/Cargo.toml -p gpui-spike
ldd desktop/target/debug/gpui-spike | sort
```

Record compositor and GPU details, and repeat with the graphics fallback
selected by the spike. A container build check can validate compilation and
libraries, but it cannot replace a real compositor, font, IME or GPU run.

Windows x64, from a Visual Studio Developer PowerShell:

```powershell
rustc -vV
rustup target list --installed
where.exe cl.exe; where.exe link.exe; where.exe rc.exe; where.exe cmake.exe
$env:VCToolsInstallDir
$env:WindowsSdkDir
cargo build --locked --manifest-path desktop/Cargo.toml --target x86_64-pc-windows-msvc
cargo run --locked --manifest-path desktop/Cargo.toml --target x86_64-pc-windows-msvc -p gpui-spike
dumpbin /dependents desktop\target\x86_64-pc-windows-msvc\debug\gpui-spike.exe
```

Run the text composition, image, resize and close checks on a clean x64 Windows
account. Record the MSVC, SDK, CMake and GPU driver versions. Do not treat a
cross-build from Linux/macOS as Windows runtime evidence.

macOS Apple Silicon:

```bash
uname -m
rustc -vV
rustup target list --installed
xcode-select -p
xcodebuild -version
xcrun --show-sdk-path
xcrun --find metal
cargo build --locked --manifest-path desktop/Cargo.toml
cargo run --locked --manifest-path desktop/Cargo.toml -p gpui-spike
otool -L desktop/target/debug/gpui-spike | sort
```

Use a native arm64 host, run the Xcode first-launch/Metal-toolchain setup if
the official guide says it is missing, and check glyphs rather than accepting a
placeholder text render. macOS signing/notarization is outside this spike.

## Clean-machine gate and limitations

Use a fresh Linux x64 image for the initially selected distribution (record the
distribution explicitly; broader distro support remains pending), Windows x64
with only the documented MSVC/SDK/CMake components, and an Apple Silicon macOS
account. Capture compiler/toolchain, SDK, dependency package
versions, `cargo tree`, build times, binary dependencies, compositor/backend,
font, IME, image conversion and repeated-frame memory results. The gate is
passed only when the real window/input/image checks and a tiny fframes worker
build succeed on each advertised target. If native project compilation remains
blocked, keep guided OS prerequisite setup or a remote-build fallback explicit;
do not promise terminal-free local setup.

Known facts above come from current upstream source/manifests and official Zed
development guides. They do not prove the proposed revision is buildable in
this repository, that the published crates are mutually compatible, that every
Linux distribution has the same package names, that fframes' Skia/FFmpeg stack
fits the GPUI SDK, or that IME/image behavior is correct under each compositor.
Those are the unresolved qualification items for M0.

Status: DONE
Summary: GPUI is feasible for the native shell with a same-revision git pin and a separate desktop workspace; Linux x64 should be the first qualification host, with Windows MSVC and macOS Metal prerequisites explicitly gated.
Concerns/Blockers: No clean-machine builds or runtime checks were performed; the candidate Zed revision, minimal Linux package set, fframes native dependencies, IME behavior, and frame conversion still require the Phase 0 spikes.
