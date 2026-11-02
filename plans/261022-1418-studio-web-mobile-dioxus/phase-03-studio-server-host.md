---
phase: 3
title: "Studio server and session host"
status: pending
priority: P1
effort: "3w"
dependencies: [2]
---

# Phase 3: Studio server and session host

## Goal

A native `fframes-studio-server` binary that owns one engine, exposes it over an authenticated
WebSocket, serves the built web client, and holds the orchestration that today lives in `studio-ui`.

## Context

- `desktop/crates/studio-engine/src/engine/handle.rs` (`EngineHandle::new`, `send`, `open_workflow`)
- `desktop/crates/studio-engine/src/providers/host.rs` (`HostInbox`, `WorkflowWake`)
- `desktop/crates/studio-engine/src/services/preview_coordinator.rs`
- Logic to move: `studio-ui/src/shell/engine_events.rs`, `shell/player.rs`, `setup_view.rs:200-203`,
  `agent/provider_setup.rs:420`, `agent/commands.rs:260`, `agent/attachment_flow.rs:97`,
  `agent/changes/host.rs`
- Decision: GPUI is frozen and only a reference. The listed logic is ported into the server together
  with its tests; `studio-ui` is not changed.

## Requirements

- One server process per user; a second start finds the running one through a lock file in app data and
  prints its URL instead of starting another.
- Default bind `127.0.0.1:<fixed port>`; on "address in use" by a stale Studio server, report it rather
  than picking another port.
- Token required on every connection; `Origin` must match the server origin or a configured dev origin.
- LAN mode is opt-in, uses TLS with a self-signed certificate created on first enable, and accepts only
  paired devices.
- Several clients may connect at once and all see the same session. Commands are serialized by the
  engine as today.
- The phone cannot change the permission policy, SDK, agent setup or remote-device settings.

## Architecture

- `session_host` module: owns `EngineHandle`, the open `AgentWorkflow`, the `PreviewCoordinator`, export
  jobs and app settings. It turns engine events into `ServerMessage`s and client commands into engine
  calls. It applies the preview readiness verdict (`presentation_verdict`) and seek orchestration that
  `engine_events.rs` and `player.rs` do today.
- `transport` module: axum router with `/` (static web client, `index.html` with CSP), `/ws` (upgrade),
  `/pair` (one-time code exchange), `/upload` (attachments and assets, size-limited, into app data, then
  handed to the engine's existing attachment checks).
- `files` module: folder browser limited to roots the user allowed (home by default), no symlink
  following, returns names and kinds only.
- `devices` module: paired device list with revoke, stored in app data.
- Bridge: engine `async_channel` receivers are awaited directly (async-channel works with tokio);
  blocking calls go through `spawn_blocking`.

## Related code files

- Create: `desktop/crates/studio-server/{Cargo.toml,src/main.rs,src/session_host/*.rs,src/transport.rs,src/auth.rs,src/files.rs,src/devices.rs,src/views.rs}`
- Modify: `desktop/Cargo.toml`
- Read only (reference): the `studio-ui` files listed above

## Implementation steps

1. Server skeleton: lock file, token generation, loopback bind, `/ws` with token and Origin checks, a
   ping message. Tests: missing token, wrong Origin and wrong protocol version are refused.
2. Session host for projects: initialize, recents, create, open, import, close, refresh, checkpoint,
   restore; presentation snapshots to all clients.
3. Build and preview orchestration moved from `engine_events.rs` and `player.rs`; preview status views.
4. Agent workflow: open, snapshots, every `AgentWorkflow` method in the protocol; permission and
   question replies; review and undo.
5. SDK setup, agent detection and adapter install, settings, presets, export start and cancel.
6. Folder browser and uploads.
7. LAN mode: TLS certificate, pairing with QR payload (address, one-time code, certificate
   fingerprint), device list and revoke, role restrictions for phones.
8. Integration tests with a scripted agent peer driving the server over a real WebSocket. Reuse
   `desktop/crates/studio-test-support` and `desktop/crates/studio-engine/tests/support/workflow_world.rs`,
   which exist today. The working tree currently has uncommitted deletions of
   `desktop/app/tests/agent_workflow/*` and `studio-ui/tests/parity/*`; do not depend on them, and
   record the regression baseline (which GPUI test sets exist and pass) before phase 3 starts.

## Todo

- [ ] Skeleton with auth and Origin checks
- [ ] Projects and presentation
- [ ] Build and preview orchestration
- [ ] Agent workflow
- [ ] SDK, agent setup, settings, presets, export
- [ ] Files and uploads
- [ ] LAN pairing and TLS
- [ ] Integration tests

## Success criteria

- `cargo test --manifest-path desktop/Cargo.toml -p studio-server` passes on Windows.
- A test client completes create, build, prompt, permission reply, apply and undo over WebSocket with a
  scripted agent.

## Security considerations

Covered in proposal 4.3. The token file is readable by the user only. Uploads are size-limited and
never written into the project directly; they pass the engine's existing attachment and asset checks.

## Risks

The moved orchestration is the most delicate part of the current app (install fence, presentation
verdicts). Move it with its existing tests, not rewritten.
