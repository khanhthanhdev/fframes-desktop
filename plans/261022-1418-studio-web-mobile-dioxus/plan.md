---
title: "fframes Studio on the web and phones with Dioxus"
description: "A local studio-server exposes the engine over WebSocket to a Dioxus web client (full Studio, direct canvas editing) and a Dioxus phone app (prompt, watch, approve)."
status: pending
priority: P1
effort: "18-22w"
branch: main
tags: [feature, frontend, backend, api, experimental]
blockedBy: []
blocks: []
created: 2026-10-22
---

# fframes Studio on the web and phones with Dioxus

## Overview

The engine stays on the user's computer behind a new local `studio-server`. A Dioxus client runs in the
browser with the full Studio and adds direct canvas editing (select, move, scale, rotate, inspector,
text in place). The same client, built for iOS and Android, does prompt, watch and approve over the
local network. The GPUI app is frozen. The design, findings and answered questions are in
[the solution proposal](./reports/solution-proposal.md).

## Phases

| # | Phase | Effort | Depends on | Status |
|---|-------|--------|------------|--------|
| 1 | [Feasibility spike](./phase-01-feasibility-spike.md) | 1w | none | Pending |
| 2 | [Shared protocol crate](./phase-02-shared-protocol-crate.md) | 1.5w | 1 | Pending |
| 3 | [Studio server and session host](./phase-03-studio-server-host.md) | 3w | 2 | Pending |
| 4 | [Media streaming](./phase-04-media-streaming.md) | 2w | 3 | Pending |
| 5 | [Canvas edit layer](./phase-05-canvas-edit-layer.md) | 2.5w | 1 | Pending |
| 6 | [Web client workspace](./phase-06-web-client-workspace.md) | 4w | 3 | Pending |
| 7 | [Web canvas editing and timeline](./phase-07-web-client-canvas-and-timeline.md) | 3w | 4, 5, 6 | Pending |
| 8 | [Mobile app](./phase-08-mobile-app.md) | 2w | 4, 6 | Pending |
| 9 | [Docs, tests and desktop freeze](./phase-09-docs-tests-and-desktop-freeze.md) | 1w | 7, 8 | Pending |
| 10 | [Remote access tunnel](./phase-10-remote-access-tunnel.md) | 1.5w | 8 | Pending |

Phase 5 is independent of phases 2 to 4 and can run alongside them.

## Decisions (2026-10-23)

- Dioxus for every new client; the phone renders in the system webview, accepted.
- GPUI is frozen and used as a reference only. The server gets its own implementation of the
  orchestration that lives in `studio-ui`; GPUI code is not refactored.
- Local network first, then a tunnel like T3 Code (tailnet first, managed relay later).
- Test devices: Android phone, Android emulator on this VM, a Mac for iOS; an inbound firewall rule for
  the server port may be added for testing.

## Acceptance criteria

- [ ] `fframes-studio-server` serves the web client on `127.0.0.1`; a connection without the token or
      from a foreign `Origin` is refused.
- [ ] In the browser a user can create or open a project, build, play with audio, prompt the agent,
      answer permissions, review diffs, apply, undo, edit the timeline and export.
- [ ] Clicking an annotated object selects it; dragging, scaling or rotating it updates the preview
      without a rebuild and writes `edits.json` once per gesture; undo restores it.
- [ ] `cargo run -- render` of the project draws the same edits as the preview.
- [ ] Text edited in place changes the Rust source and rebuilds.
- [ ] A paired Android phone on the LAN can prompt, watch with audio, approve and undo; the same on
      iOS, built and tested on the Mac.
- [ ] The same phone on mobile data works through the user's Tailscale tailnet.
- [ ] The GPUI app still passes its Windows test sets.

## Non-goals

Hosted or cloud Studio, a managed relay service (design note only), store release of the phone apps,
editing on the phone, a Dioxus desktop build, migrating to Dioxus 0.8.

## Dependencies

Dioxus 0.7.10 (pinned), tokio, axum, tokio-tungstenite, rustls, gloo-net, web-sys. Android SDK and
NDK for phase 8; the user's Mac with Xcode for iOS; the user's Tailscale install for phase 10.
