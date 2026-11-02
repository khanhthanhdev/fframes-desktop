---
phase: 8
title: "Mobile app"
status: pending
priority: P2
effort: "2w"
dependencies: [4, 6]
---

# Phase 8: Mobile app

## Goal

The Dioxus client built for Android and iOS with the phone feature set: pair with the computer, open a
recent project, prompt the agent, watch with audio, answer permissions, review changes, apply and undo.

## Context

- Proposal sections 4.3 (pairing, TLS), 6 (feature table) and 7 (mobile findings).
- Spike S4 results.

## Requirements

- Same crate as the web client with a `mobile` feature that compiles only the phone screens.
- Layout for phones: tabs for Chat, Preview and Changes; preview on top in landscape.
- Pairing by typing the address and one-time code shown in **Settings → Remote devices** (baseline);
  QR scanning only if spike S4 shows camera access works in the webview.
- Control messages go over a native Rust tokio-tungstenite connection with a rustls verifier pinned to
  the fingerprint from pairing; `wss://` only.
- Frames and audio reach the webview's JavaScript through the bridge spike S4 selected (in-app loopback
  relay or custom-protocol handler), never through `eval`.
- Reconnects when the phone wakes or changes network; shows "Computer not reachable" otherwise.
- Cannot change permission policy, SDK, agents or devices (the server enforces this too).
- Survives dark mode and font scale changes without a blank screen (spike S4).

## Architecture

- `studio-web/src/mobile/`: `pair.rs` (QR scan through the webview camera API, or manual entry),
  `layout.rs`, reuse of `agent/`, `player/` and the changes view with unified diffs only.
- Stored pairing (address, device token, fingerprint) in the app's private storage.

## Related code files

- Create: `desktop/clients/studio-web/src/mobile/*.rs`
- Modify: `desktop/clients/studio-web/Cargo.toml`, `Dioxus.toml` (mobile bundle settings)

## Implementation steps

1. `mobile` feature and phone layout.
2. Pairing flow and certificate pinning.
3. Chat, preview with audio, changes with apply and undo.
4. Background and network change handling.
5. Android build with `dx bundle --platform android`; test on the emulator on this VM first, then on the
   Android phone over the LAN. For the phone, add an inbound Windows Firewall rule for the server port on
   the Private profile only, named so it can be found, and remove it when testing ends.
6. iOS build and test on the user's Mac with Xcode, against the server on this VM over the LAN.

## Todo

- [ ] Mobile feature and layout
- [ ] Pairing and pinning
- [ ] Chat, preview, changes
- [ ] Reconnect handling
- [ ] Android build and test
- [ ] iOS build and test on the Mac

## Success criteria

The phone acceptance criterion in `plan.md` passes on the Android emulator, the Android phone and iOS
on the Mac.

## Risks

- Dioxus mobile bundling has no documented `.aab` or signing path; store release is a non-goal.
- Android 0.7.x blank-webview bug; if it reproduces, document the workaround or evaluate 0.8 with the user.
