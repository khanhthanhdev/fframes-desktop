# Dioxus for one Rust UI on web, iOS, Android and desktop (state as of 22 Oct 2026)

Method: GitHub releases API (`gh api repos/DioxusLabs/dioxus/releases`, `published_at` dates), dioxuslabs.com docs, docs.rs source, Blitz README. Items marked UNVERIFIED come from general knowledge or weak sources.

## 1. Versions and renderers

- Latest stable: v0.7.10, published 2026-07-30, marked "Latest" on https://github.com/DioxusLabs/dioxus/releases (a hotpatch fix). The 0.7.x line has had a patch roughly every 2 to 6 weeks in 2026 (0.7.5 on 6 Apr, 0.7.6 on 22 Apr, 0.7.9 on 8 May).
- 0.7.0 was published 2025-10-31 (https://github.com/DioxusLabs/dioxus/releases/tag/v0.7.0), after rc.0 to rc.4 between 2025-08-11 and 2025-10-31. The blog post dated 8 Sep 2025 predates the stable release. Headlines: Subsecond hot-patching of Rust code (web WASM, desktop, iOS, Android), the Dioxus Native/Blitz renderer, a fullstack overhaul (Axum, `Websocket`, `use_websocket`, SSE, streaming), 28 Radix-style primitives, Stores, WASM bundle splitting, and a `dx` CLI that also works on non-Dioxus Rust projects. Blog: https://dioxuslabs.com/blog/release-070/.
- 0.8 is in alpha: alpha.0 on 2026-05-19, alpha.1 on 2026-07-31, alpha.2 on 2026-10-14, alpha.3 on 2026-10-16 (https://github.com/DioxusLabs/dioxus/releases/tag/v0.8.0-alpha.3). It moves to edition 2024 and an `objc2` port for macOS/iOS, and the template and rendering internals changed (trait-based HTML elements, renderer IDs). It uses tao 0.37 and wry 0.57 and Blitz 0.3.0-beta.3.
- 0.8 breaking changes already announced: `Key::Enter` becomes `Key::Named(NamedKey::Enter)`, the custom renderer `WriteMutations` trait changed, HTML elements moved out of `dioxus_elements`, gloo-net 0.7. Known issue in alpha.3: `dx components` is broken.
- Renderers today:
  - Web: DOM via WASM, with optional SSR and hydration.
  - Desktop and mobile: the platform webview through wry and tao (the default).
  - Native: Blitz (Taffy, Stylo, Vello on wgpu), crate `dioxus-native`. The README calls the desktop WGPU/Skia renderers experimental.
- Blitz status: the repo says "beta" with "many bugs and missing features" (https://github.com/DioxusLabs/blitz). It deliberately excludes WebSockets, localStorage and similar, and its Android support is documented only for its own browser demo. The 0.7 blog says performance "has not been a focus". Do not plan on Blitz for this app.
- Hot reload: `dx serve` gives RSX hot reload plus Subsecond hot-patching of Rust code on all platforms, with the caveat that struct-layout changes are not migrated.

## 2. Mobile

- How it works: the Rust code compiles natively (`aarch64-apple-ios`, Android NDK targets) and the UI renders in the system WebView (WKWebView, Android System WebView) through wry (https://dioxuslabs.com/learn/0.7/guides/platforms/mobile). The official page says native Android widgets and animations are unsupported, so CSS is the only styling path. The README lists the WGPU and Skia mobile renderers as experimental.
- Tooling: `dx serve --ios|--android` opens the simulator or emulator, and hot reload on a real Android device works via `adb reverse`. `Info.plist` and `AndroidManifest.xml` can be customised from `Dioxus.toml`. Android setup (SDK, NDK, `JAVA_HOME`, `NDK_HOME`) is described as "somewhat tricky".
- Bundling: the 0.7 bundle tutorial lists iOS as `.app` and Android as `.apk`. Signing is left to you: "Dioxus doesn't provide built-in utilities for this" (https://dioxuslabs.com/learn/0.7/tutorial/bundle). The README claims `.ipa` and `.apk` output.
- Store publishing: there is no official store-submission guide. Google Play needs an `.aab`, and I found no evidence that `dx` emits one, so expect Gradle or Xcode archive steps outside `dx`. iOS needs a Mac with Xcode and an Apple Developer account. A third-party wiki (https://mintlify.wiki/DioxusLabs/dioxus/platforms/mobile) describes an Xcode archive path, but that site looks AI-generated and is not authoritative.
- Known gaps: there is no first-party plugin system for camera, haptics or keychain, so native APIs need your own JNI or objc2 code. The `manganis::android::with_activity` API exists for getting at the Activity.
- Known bug on stable 0.7.x: on Android, changing dark mode, font scale or density recreated the activity and left a blank webview. The alpha.3 notes say the wry 0.57 upgrade fixes it, and that fix exists only in 0.8-alpha.3. A dark-mode switch is a normal phone event, so either wait for 0.8 stable, ship on the alpha, or patch wry/tao yourself (UNVERIFIED whether 0.7.10 is fully affected, so test on a device).
- Fit for our scope: mobile only needs prompt, preview and approve/undo, which is a normal webview app, so the gaps matter little.

## 3. Web, bundle size and networking

- Bundle size: the README claims web hello world is about 50 KB and desktop/mobile apps are under 5 MB. These are project claims, not measurements. The 0.6 cookbook says hello world gets under 100 KB only with nightly flags (https://dioxuslabs.com/learn/0.6/cookbook/optimizing). A real app with markdown, diff and timeline code will be well above that (UNVERIFIED, my estimate is 0.5 to 2 MB gzipped), so measure early. `dx` does `wasm-opt` and router-based splitting via `#[wasm_split]` (0.7 blog).
- Dioxus fullstack WebSocket (https://dioxuslabs.com/learn/0.7/essentials/fullstack/websockets): `Websocket<In, Out, Enc>` with `use_websocket`, JSON by default, `CborEncoding` or MsgPack as binary options, typed `send` always emits binary frames.
- Constraints found in the client source (https://docs.rs/dioxus-fullstack/0.7.9/src/dioxus_fullstack/payloads/websocket.rs.html):
  - On wasm32 it uses gloo-net and sends only the path, so it is same-origin only ("very important we use the path here and not the full url on web").
  - On native it uses tungstenite over a reqwest upgrade, with a `ws` to `http` scheme rewrite.
  - It assumes the server is a Dioxus/Axum server function. Our engine is not one.
- Recommendation: do not use dioxus-fullstack for the engine link. Define one shared protocol crate and use two thin transports:
  - wasm32: `gloo-net` `WebSocket` (or `web-sys`), binary frames, so any host and port works.
  - Native desktop and mobile (Rust runs natively, not as WASM): `tokio-tungstenite` (use `rustls`, not native-tls, for iOS and Android builds).
  - Hide both behind one `Transport` trait and expose the stream to components as a Dioxus signal or coroutine channel.
  - Connection count: on web one socket can carry control, frames and audio. On desktop and mobile Q4 recommends a second socket opened by JS inside the webview for media, so those targets use two connections (Rust for control and signals, webview JS for frames and audio). The engine must accept both, so this is not "one transport everywhere".
- Pitfalls:
  - A page served over HTTPS cannot open `ws://` (mixed content), so a hosted web client talking to a LAN engine needs `wss://` or must be served by the engine itself.
  - iOS App Transport Security blocks cleartext `ws://` unless `NSAllowsLocalNetworking` is set in `Info.plist`, and Android needs `usesCleartextTraffic` or a network security config. Both are configurable through `Dioxus.toml`.
  - The WebView origin for a mobile app is a custom or localhost scheme, so engine CORS and origin checks must allow it.
- Same client code on every platform: yes for components, state and protocol types. Only the transport and the frame sink differ per target (`cfg(target_arch = "wasm32")`).

## 4. Performance for heavy views

- Dioxus is a vdom that mutates the DOM (the web renderer batches mutations, and desktop and mobile send them over an IPC bridge to the webview). Keep large data out of the vdom:
  - Virtualized lists: there is no official component. The community crate `dioxus-lazy` is stale (last update Oct 2024, https://rust-digger.code-maven.com/crates/dioxus-lazy). Plan to write a small windowed list (fixed or measured row heights, `onscroll` or `onmounted` to read geometry, render the visible slice only). That is about 100 to 200 lines for the chat transcript. The timeline needs a canvas anyway.
  - Timeline and waveforms: draw into one `<canvas>` (2D context first, WebGL or WebGPU only if profiling demands it). Precompute min/max peaks per zoom level in the engine and send those, not raw samples.
  - Markdown and diffs: parse in Rust (`pulldown-cmark`, `similar`) and render only visible blocks. Streaming chat appends should touch only the last block's signal.
- Video frames (same technique on web and mobile webviews):
  - Preferred: the engine sends encoded frames (H.264 with a short GOP, or per-frame JPEG/WebP for scrubbing). The client decodes with WebCodecs `VideoDecoder` and draws `VideoFrame`s to a canvas with `requestAnimationFrame`.
  - Fallback: JPEG/PNG through `createImageBitmap` then `drawImage`. RGBA at 1080p is about 8 MB per frame, so about 250 to 500 MB/s at 30 to 60 fps. That is only viable over loopback and not over a phone LAN, so send encoded frames.
  - WebCodecs support: caniuse shows iOS 16.4+ as partial and 26.0+ as full for the WebCodecs page, and Android WebView 94+ (https://caniuse.com/webcodecs). Whether WKWebView gets the same APIs as Mobile Safari was not confirmed, so test on a device. Keep a JPEG fallback.
  - Drive the canvas from `web-sys` on web. On mobile the Rust side is native, so there are two options: (a) open the WebSocket in a small JS module inside the webview and decode there, with Rust handling control messages only (recommended, no per-frame bridge cost); (b) receive in Rust and pass bytes to the webview through a custom asset protocol handler. The docs warn to batch `eval_js` calls because the bridge has overhead, so never push frames through `eval`.
- Audio: Web Audio with an `AudioWorklet` ring buffer fed by the same socket, with the video clock slaved to the audio clock (`AudioContext.currentTime`) for A/V sync. iOS requires a user gesture to start the context. Same JS-module approach in the webview.
- Calling browser APIs from Dioxus: `web-sys` and `wasm-bindgen` on wasm32, and for code shared with desktop and mobile a small JS file loaded via `asset!()` plus `document::eval`. I could not open the 0.7 interop docs page (404), so check `document::eval` ergonomics in the 0.7 docs before committing.

## 5. Shared types crate

- Yes. A `fframes-protocol` crate with serde types (plus `postcard`, `rmp-serde` or `ciborium` for binary) compiles for both the native engine and wasm32. Dioxus's own typed `Websocket<In, Out>` relies on the same idea.
- Constraints for wasm32:
  - No `tokio` runtime or `std::fs`, and `std::thread` and `std::time::Instant` panic or are unavailable (use `web-time` or `instant`).
  - No `ring` or `openssl` features (the ring-based rustls provider needs special setup for wasm).
  - Use `getrandom` with the `js` (0.2) or `wasm_js` (0.3) feature.
  - Keep the crate `default-features = false` and put native-only items behind `cfg(not(target_arch = "wasm32"))`. The fframes editor bridge already follows this rule.
- Dioxus requires the `web` feature for the wasm build and `desktop` or `mobile` for native, and these cargo features are cleanly separable per platform.

## 6. Risks

- Breaking cadence: 0.6 (2024-12-09), 0.7 (2025-10-31) and the 0.8 alphas (since 2026-05-19) each broke APIs, about one minor per year with alphas lasting months. Alpha.2 and alpha.3 shipped breaking changes within two days of each other. Pin an exact version and budget a migration per minor release.
- Maturity: 39.5k GitHub stars (https://github.com/DioxusLabs/dioxus), but the README lists no "used by" section. Y Combinator's profile mentions Huawei production apps and Airbus work (https://www.ycombinator.com/companies/dioxus-labs, undated, partly unconfirmed). I found no named, large production app with heavy media or timeline editing on Dioxus. The awesome-dioxus repo page (https://github.com/DioxusLabs/awesome-dioxus) did not list app entries in the scraped content.
- Ecosystem gaps: no virtualized list, no rich-text, markdown or diff components, a small component library, and no mobile plugin system.
- Mobile toolchain friction is real (NDK versions, signing, no `.aab`), but it is one-time setup for a thin prompt/preview/approve client.
- Webview performance on mobile is bounded by the system webview and the IPC bridge. The design above avoids the bridge for frames and audio.
- Because the heavy parts (canvas, video decode, audio) are JS or web-sys code anyway, Dioxus would act as a Rust shell for DOM chrome. A TypeScript client sharing the protocol via generated types is a credible alternative if the Rust-everywhere goal is soft. This is an opinion, not a research finding.

Unresolved questions:
- Does `dx bundle --platform android` produce an `.aab`, and what is the current App Store flow (0.8 may change it)?
- Real WASM size for an app of our shape.
- Does WKWebView inside a wry app expose WebCodecs and AudioWorklet on current iOS (needs a device test)?
- Is there a current 0.7 interop page for `document::eval` with binary payloads, and what is the practical throughput of the custom asset protocol on mobile?
