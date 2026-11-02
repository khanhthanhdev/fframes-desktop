---
phase: 1
title: "Feasibility spike"
status: pending
priority: P1
effort: "1w"
dependencies: []
---

# Phase 1: Feasibility spike

## Goal

Prove or disprove, with measured numbers, the six assumptions the web and mobile design rests on, before
any production code. The output is `reports/spike-results.md` with a pass or fail per spike and the
design changes that follow.

## Context

- [Solution proposal](./reports/solution-proposal.md), sections 4.3, 4.4, 5.3, 5.4 and 7.
- `plans/reports/researcher-261022-2044-dioxus-web-mobile.md` (item 14 lists what only device tests answer).
- `fframes/src/svgr.rs` (`Svgr`, `with_editor_object`), `fframes/src/editor.rs` (`EditorObjectKey`,
  geometry collection), `fframes/src/renderer/preview.rs` (`Previewer`, `FrameRenderer`).
- `fframes-studio-runtime/src/anchors.rs` (`validate_source_anchor`).

## Environment found on this VM (2026-10-23)

| Item | State | Needed for |
| --- | --- | --- |
| Rust 1.98.1 (pinned by `desktop/rust-toolchain.toml`) | Installed; targets windows-msvc, linux-gnu, aarch64-apple-darwin | All |
| `wasm32-unknown-unknown` for 1.98.1 | **Missing** (installed only for `stable`) | S1–S3 |
| Android Rust targets | **Missing** | S4 |
| `dx` (Dioxus CLI) | **Missing** | S1–S4 |
| Node v24.21.0, pnpm | Installed | AudioWorklet script, tooling |
| Chrome, Edge | Installed | S1–S3 |
| Firefox | Not installed | Optional |
| Managed SDK FFmpeg (`~/.fframes/sdk/active/ffmpeg`) | Has `libx264`, `libopenh264`, `h264_mf`, `mjpeg` | S2 |
| clang (`C:\Program Files\LLVM`) | Installed | Skia CPU build (S5) |
| JDK, Android SDK, NDK, emulator, adb | **Missing** | S4 |
| Windows Hypervisor Platform | **Disabled**. This is a Windows Server guest (32 cores, 32 GB); whether nested virtualization runs the Android emulator is unproven | Android emulator (best-effort) |
| Network | IPv4 `172.22.27.123/24`, gateway `172.22.27.1`; profile **Public**; Private firewall profile is off, Public is on | Phone on LAN: the user must confirm the phone's Wi-Fi reaches this subnet |
| Audio output | No sound device listed (`Win32_SoundDevice` empty), Windows Audio service running; over RDP, output exists only with audio redirection | S3 in the VM's browser |
| Tailscale | Not installed | Not needed in phase 1 |

## Prerequisites (setup day)

Steps marked **ask** change the machine beyond installing developer tools; confirm with the user first.

1. `rustup target add --toolchain 1.98.1 wasm32-unknown-unknown aarch64-linux-android x86_64-linux-android`.
2. `cargo +1.98.1 install dioxus-cli --version 0.7.10 --locked`; check `dx --version` prints 0.7.10.
3. Android: install Temurin JDK 17, Android command-line tools, then with `sdkmanager`: `platform-tools`,
   the platform and build-tools versions the Dioxus 0.7 mobile guide names, the NDK version it names, and
   an `x86_64` Google APIs system image. Set `ANDROID_HOME`, `JAVA_HOME`, `NDK_HOME` for the user.
4. **Ask:** enable the Windows Hypervisor Platform (`Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform`,
   administrator). It needs a **reboot, which ends the RDP session and this Claude session**, so it is
   done between sessions. Even then the emulator may not run under nested virtualization, so the
   physical phone is S4's primary device and the emulator is best-effort.
5. **Ask (allowed for testing, details to confirm):** this machine's network is in the **Public** profile,
   so a Private-profile rule would not apply. Add one inbound TCP rule named `fframes-spike-8471` for port
   8471 on the Public profile, limited to `RemoteAddress LocalSubnet`, and remove it at the end of
   phase 1. It only helps if the phone's Wi-Fi reaches `172.22.27.0/24`; if this VM sits behind NAT
   (for example a Hyper-V or cloud network), the phone cannot reach it, and S4 runs `spike-server` on a
   computer on the phone's network instead.
6. Physical phone: enable developer options and USB or wireless debugging; record model, Android version
   and Android System WebView version.
7. Mac (user runs it): Xcode, `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`, `dx` 0.7.10,
   and the `desktop/spikes` folder (how it gets there is a user decision, see below). A checklist is in
   S4-iOS below.
8. `cargo +1.98.1 build --release -p fframes_skia_renderer` in the root workspace, to find out on day 1
   whether the Skia binaries download and build here.
9. Render the S2 source frames with the root workspace's own CLI, so `spike-server` needs no fframes
   dependency: `cargo run --release -p hello-world-example --bin hello_world_example_bin -- render -o <scratchpad>/hello.mp4`,
   then `ffmpeg -i hello.mp4 -f rawvideo -pix_fmt rgba hello.rgba` with the SDK FFmpeg (300 frames of
   1080p RGBA, about 2.5 GB, kept in the scratchpad).

## Where the spike code lives

`desktop/spikes/` is its own Cargo workspace (its own `[workspace]` table and `Cargo.lock`, not a member
of `desktop/Cargo.toml`), so it builds with the pinned 1.98.1 toolchain and never touches the product
workspaces. The results are kept in this plan's `reports/`.

- The root `Cargo.toml` has `[patch.crates-io] fframes = { path = "fframes" }`; a separate workspace
  does not inherit it, so `desktop/spikes/Cargo.toml` repeats it (with the relative path
  `../../fframes`).
- `spike-patch` enables fframes' `compile-time-svgtree` feature; without it `Svgr::svg_tree` does not
  exist.
- `spike-web` and `spike-server` have no path dependencies outside `desktop/spikes`, so they can be
  copied to the Mac alone.
- How the code reaches the Mac is the user's choice: commit only `desktop/spikes/**` on a new branch
  `spike/studio-web-mobile` and push (staging those paths explicitly, because `main` has many unrelated
  uncommitted deletions that must not be included), or copy a zip.

```text
desktop/spikes/
  Cargo.toml                 [workspace] members below, dioxus = "=0.7.10"
  spike-server/              native: tokio, axum, tokio-tungstenite, rustls, rcgen, jpeg-encoder;
                             reads pre-rendered frames, no fframes dependency
  spike-web/                 Dioxus 0.7.10 app: web and android/ios bundles; assets/audio-worklet.js
  spike-patch/               S5 tests: fframes, fframes_skia_renderer (CPU), a small test video
  spike-anchor/              S6: anchored literal rewrite test against a template project
  results/                   CSV and PNG outputs written by the spikes
```

`spike-server` listens on `127.0.0.1:8471` (and on the LAN with TLS only for S4), serves the built
`spike-web` bundle, and exposes `/ws` (control and echo), `/media` (frames and audio, binary records)
and `/report` (the client posts its measurements; the server writes `results/*.csv`).

## Spikes

### S1: WebSocket transport and WASM size

- **Build:** `spike-web` page with a 10,000-row windowed list, a `<canvas>`, a text input, and a
  connection panel. Transport is `gloo-net` WebSocket; messages are serde JSON of a 1 KB struct shared
  with the server from a small `spike-proto` module.
- **Measure:** 1,000 sequential round trips, p50 and p95 latency; 10,000 messages in a burst, throughput;
  `dx bundle --platform web --release`, then `gzip -9` size of the `.wasm` plus JS glue.
- **Pass:** p95 round trip under 5 ms on loopback; release bundle under 2 MB gzipped; while the page
  auto-scrolls the windowed list, a `PerformanceObserver` for `longtask` records no task over 50 ms.

All browser spikes run unattended: `spike-server` prints a URL with `?auto=s1` (or `s2`, `s3`), which
runs the test and posts results to `/report`. Chrome and Edge are started with
`--autoplay-policy=no-user-gesture-required` and a separate `--user-data-dir` in the scratchpad, so
AudioContext starts without a click and the user's own browser profile is untouched.

### S2: Frames from server to canvas

- **Server:** load the 300 pre-rendered 1080p RGBA frames from prerequisite 9 into memory, so render
  cost does not mix with transport cost. Then:
  - JPEG: encode each frame with `jpeg-encoder` at quality 80; record encode ms per frame and bytes.
  - H.264: pipe RGBA into the managed SDK `ffmpeg.exe` with `libx264 -preset ultrafast -tune zerolatency -g 30 -f h264`;
    split the Annex B output into access units; record encode frames per second. Repeat once with
    `h264_mf` to see whether Media Foundation helps on this VM.
  - Raw RGBA over loopback as a third variant.
- **Client:** JPEG through `createImageBitmap` on a `Blob`, drawn with `drawImage`; H.264 through
  WebCodecs `VideoDecoder` (Annex B, no description), drawn from `VideoFrame`. A HUD shows received,
  decoded, drawn and dropped counts and decode time; the client posts them to `/report`.
- **Runs:** scrub (random seeks, newest wins) at 30 requests per second with JPEG; playback at 30 and 60
  fps with H.264 and with JPEG; raw RGBA at 30 fps. Chrome and Edge.
- **Pass:** JPEG scrub at 30 per second with no backlog; H.264 playback at 60 fps with under 1% dropped
  frames; server JPEG encode under 15 ms per 1080p frame or H.264 encode at 60 fps or more on this VM.
  The report names the encoder to use.

### S3: Audio clock

- **Server:** decode `examples/conference-splash-screen/media/track.mp3` (hello-world has no audio) to
  48 kHz stereo `f32` with the SDK FFmpeg; send 20 ms chunks, staying at most 500 ms ahead
  of the position the client reports.
- **Client:** `assets/audio-worklet.js` holds a 200 ms ring buffer and counts zero-filled render quanta
  (underruns). Every 100 ms the client sends its played sample position. The displayed video frame is
  chosen from the audio position (`AudioContext.getOutputTimestamp` minus output latency).
- **Measure:** 60 s of playback; underruns; A/V offset as the difference between each drawn frame's
  timestamp and the audio clock at draw time (p50, p95). Repeat while scrolling the 10,000-row list.
- **Pass:** zero underruns in 60 s; p95 A/V offset under 40 ms.
- **Where:** this VM has no sound device. Run S3 here first to see whether Chrome's AudioContext runs at
  all (with RDP audio redirection, or on a null sink). If it does not, S3's numbers come from the phone
  (S4) and the Mac's Safari and iOS runs, and the report says so.

### S4: Android (and iOS) webview

Build `spike-web` with `dx bundle --platform android` (and `dx serve --platform android` for iteration);
`x86_64` for the emulator, `aarch64` for the phone.

The app shows a checklist screen that runs these checks and posts the results to `/report`:

| Check | How | Pass |
| --- | --- | --- |
| Secure context | `window.isSecureContext` | `true` |
| WebCodecs | `typeof VideoDecoder`, then decode 60 H.264 frames from S2 | Decodes and draws |
| AudioWorklet | Run the S3 worklet for 30 s | No underruns |
| Pinned native connection | Rust side: `tokio-tungstenite` with a `rustls` `ServerCertVerifier` that accepts only the SHA-256 fingerprint of the server's self-signed certificate (made with `rcgen`), to `wss://<lan-ip>:8471/ws` | Connects; a different certificate is refused |
| Media bridge A: loopback relay | Rust receives media from the server and re-serves it on `ws://127.0.0.1:<random>` inside the app; JS connects to it | 30 fps of 1080p JPEG delivered; note any cleartext-policy setting needed |
| Media bridge B: custom protocol | Rust serves media through Dioxus's asset or custom-protocol handler; JS polls `fetch` for the next frame | 30 fps of 1080p JPEG delivered |
| Camera | `navigator.mediaDevices.getUserMedia({video:true})` | Result recorded; manual code entry is the fallback |
| Dark mode and font scale | `adb shell cmd uimode night yes/no`, `adb shell settings put system font_scale 1.3` while the app runs; `adb exec-out screencap -p` before and after | Screen is not blank after each change |

- **Pass:** secure context, WebCodecs (or a working JPEG fallback), AudioWorklet, pinned connection and
  at least one media bridge pass on the phone (and on the emulator if it runs). The report picks the
  bridge.
- **S4-iOS (on the user's Mac):** `dx bundle --platform ios`, run in the simulator and on an iPhone,
  same checklist (without the `adb` steps; toggle appearance and text size in Settings instead). The
  user sends the `/report` results or screenshots back.

### S5: Patching objects before conversion

`usvgr` 0.50.2 keeps `static_hash` crate-private and has no mutation API for a converted `usvgr::Tree`,
so edits cannot be applied after conversion from outside that crate. `fframes::Svgr` exposes its nested
tree (`svg_tree`, with public `kind`, `attrs`, `children` and `static_hash` on each node), and
annotated objects are `<g id="fframes.editor.v1....">` wrappers. The spike therefore patches the `Svgr`
that `render_frame` returns, before conversion, through a wrapper `Video`, with no change to fframes.

- **Test video:** a `svgr!` frame with an annotated group whose children are static (cached by
  `static_hash`), an animated parent transform, and a second annotated object with `repeat_key`s.
- **Patch:** `PatchedVideo<V>` calls the inner `render_frame` and then (1) wraps each matching annotated
  group in a new `<g>` with the edit's transform (about the bounding box centre) and opacity, and (2) for
  a fill override, rewrites `fill` on every descendant shape that sets one, adds it on the group for
  shapes that inherit, and sets `static_hash = None` on every node it changed and on their ancestors
  inside the object.
- **Ground truth:** the same video with the edit written directly in the markup.
- **Runs:** 10 consecutive frames where the fill alternates red and blue each frame, and the translate
  changes each frame; CPU backend (`fframes::cpu`) and Skia CPU backend (`fframes_skia_renderer`, CPU
  context). Compare each patched frame with ground truth (maximum channel difference at most 1).
  Render the unpatched video with and without the wrapper and compare byte for byte. Collect geometry
  with `fframes::editor::editor_geometry` for the patched frames and check the bounds moved by the
  translate.
- **Pass:** all comparisons hold on both backends; geometry includes the edit; patch time under 1 ms for
  a frame with 200 annotated objects.

### S6: Text edit through an anchored literal

Today's anchors do not point at text. In the template the title's anchor marker
(`// studio-title-source-anchor`, `templates/src/lib.rs:89`) sits in `render_frame` above the
`svgr!` call, while the headline strings are passed in elsewhere
(`TitleScene::new("intro", "Your video starts here", ..)`, `templates/src/lib.rs:120-122`). The wire type
`EditorSourceAnchor` carries only path, symbol and marker; byte spans and hashes exist only in the
runtime's `ElementRegistration`. So S6 tests a new convention instead of the existing anchor:

- **Convention under test:** a text marker comment on the line before the literal that supplies the text,
  `// studio-text: <scene>/<component>/<object>` (for the template: one per `TitleScene::new` call), and
  the object key gains the same text marker. The server, not the client, resolves it at edit time: it
  reads the file from the current source revision, finds exactly one marker, parses the next statement
  with `syn` to find exactly one string literal, and records the file hash and the literal's byte span.
- **Setup:** create a project from the Studio template in the scratchpad and add the markers by hand.
- **Test:** rewrite the literal (escaped for Rust), build through the engine's managed-build path (the
  same path `prepare_examples --check` uses, not a plain `cargo build`, which does not resolve the SDK
  dependencies), render frame 0 through the preview worker and check the new text is drawn. Then change
  the file by one byte between resolve and write and check the rewrite is refused with a hash mismatch;
  add a second marker with the same key and check it is refused as ambiguous.
- **Pass:** rewrite, build and render succeed; stale and ambiguous edits are refused; strings with
  quotes, `\`, newlines and non-ASCII text round-trip. If the convention fails, text edits go through the
  agent only.

## Schedule

| Day | Work |
| --- | --- |
| 1 | Prerequisites 1–3, 5, 8, 9; spike workspace; S1; S6 (prerequisite 4 and its reboot happen after the session, if approved) |
| 2 | S2 on Chrome and Edge |
| 3 | S3; S5 |
| 4 | S4 on the phone, then the emulator if it runs |
| 5 | S4-iOS with the user on the Mac; write `reports/spike-results.md`; remove the firewall rule; stop all spike processes |

## Decision rules

| Result | Consequence |
| --- | --- |
| S1 fails on size only | Continue; set a size budget task in phase 6 |
| S2 H.264 fails, JPEG passes | Playback uses JPEG at the measured rate; WebCodecs becomes optional |
| S2 and S3 fail | Stop; revisit the web client design with the user |
| S4 both media bridges fail | Stop the phone part; discuss alternatives with the user (a native view for the player, or another toolkit for the phone) |
| S4 pinning fails | Phone routes use the Tailscale HTTPS route only (phase 10 moves earlier) |
| S5 fails on Skia CPU | Stop canvas editing; investigate the Skia caches before phase 5 |
| S6 fails | Text edits go only through the agent |

## Verification

- `cargo +1.98.1 test --manifest-path desktop/spikes/Cargo.toml -p spike-patch -p spike-anchor`
- `dx bundle --platform web --release` inside `desktop/spikes/spike-web`
- `results/*.csv` exist for S1–S4; `reports/spike-results.md` lists every number in the pass criteria.

## Todo

- [ ] Prerequisites (ask before 4 and 5)
- [ ] Spike workspace
- [ ] S1 WebSocket and WASM size
- [ ] S2 frames: JPEG, H.264, raw
- [ ] S3 audio clock
- [ ] S4 Android emulator and phone
- [ ] S4-iOS on the Mac
- [ ] S5 patching on CPU and Skia CPU
- [ ] S6 anchored literal rewrite
- [ ] Spike results report and design updates
- [ ] Firewall rule removed, spike processes stopped

## Risks

- No GPU here: S5 covers CPU and Skia CPU only; GPU Skia is unverified.
- Rendering on CPU is excluded from S2 timings on purpose; real preview speed is measured in phase 4.
- `spike-server` and `dx serve` are long-running: track their PIDs, reuse port 8471, never start a second
  copy, and stop them at the end of each session.

## Next steps

Phase 2 and phase 5 start when the spikes pass. Phase 5 uses the patch point that S5 proves.
