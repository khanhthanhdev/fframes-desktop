---
phase: 2
title: "Shared protocol crate"
status: pending
priority: P1
effort: "1.5w"
dependencies: [1]
---

# Phase 2: Shared protocol crate

## Goal

A `studio-api` crate that compiles on native and `wasm32` and defines every message between the server
and its clients, plus a `studio-canvas` crate with the hit-testing and gizmo math both sides use.

## Context

- `desktop/crates/studio-engine/src/engine/backend.rs:34-83` (`Presentation`, `Command`)
- `desktop/crates/studio-engine/src/engine/handle.rs:77-95` (`EngineEvent`)
- `desktop/crates/studio-engine/src/agent/model.rs:50-703` (serde rows, `WorkflowSnapshot`)
- `desktop/crates/studio-engine/src/canvas_selection.rs`
- `fframes-studio-protocol/src/preview.rs` (`EditorFrameMetadata`, `PreviewIdentity`)

## Requirements

- Serde only. No tokio, no `std::fs`, no `PathBuf` on the wire for project files (use project-relative
  strings). `web-time` for timestamps.
- Every message carries `protocol_version`. The server refuses an older or newer client with a reason.
- DTOs are built from engine types by `From` impls that live in the server, not in the engine, so the
  engine gains no wire concerns.

## Architecture

- `ClientMessage { id, body: ClientCommand }` with variants grouped by area: `Project` (create, open,
  import, locate, close, refresh, checkpoint, restore), `Build`, `Sdk`, `Preset`, `Timeline`, `Edits`,
  `Agent` (submit, scoped submit, steer, stop, reply permission, reply question, apply, discard, undo,
  revert path, reject files, set mode/option, sessions), `Preview` (seek, play, pause, set view size),
  `Export`, `Files` (browse root, upload), `Settings`, `Devices`.
- `ServerMessage`: `Reply { id, result }`, `Presentation(PresentationView)`, `Workflow(WorkflowView)`,
  `Timeline(TimelineView)`, `Edits(EditsView)`, `Preview(PreviewStatusView)`, `Export(ExportProgressView)`,
  `Notice`.
- `MediaRecord` binary header: kind (`Jpeg`, `H264Chunk`, `Pcm`, `FrameMetadata`), preview identity,
  frame index, seek serial, timestamp, length.
- `Transport` trait with `send`, `recv` and `close`; implementations live in the client (gloo-net) and
  in tests (tokio-tungstenite).
- `studio-canvas`: move `canvas_selection.rs` here unchanged; `studio-engine` re-exports it so the GPUI
  app keeps compiling. Add `Gizmo` (handle layout, hit regions, drag to delta with Shift/Alt modifiers,
  snapping to guides).

## Related code files

- Create: `desktop/crates/studio-api/{Cargo.toml,src/lib.rs,src/client.rs,src/server.rs,src/media.rs,src/views/*.rs}`
- Create: `desktop/crates/studio-canvas/{Cargo.toml,src/lib.rs,src/selection.rs,src/gizmo.rs}`
- Modify: `desktop/Cargo.toml` (members, workspace dependencies)
- Modify: `desktop/crates/studio-engine/src/canvas_selection.rs` (becomes a re-export)

## Implementation steps

1. Create `studio-api` with the message enums and views; derive serde with `#[serde(tag = "type")]`.
2. Add golden JSON tests for each message so wire changes are deliberate.
3. Move `canvas_selection.rs` to `studio-canvas`; re-export from the engine; run the engine's canvas
   tests.
4. Write `gizmo.rs` with unit tests for handle hit regions, proportional scale and rotation about a
   chosen origin.
5. `cargo check --manifest-path desktop/Cargo.toml --target wasm32-unknown-unknown -p studio-api -p studio-canvas`
   locally on Windows.

## Todo

- [ ] `studio-api` messages and views
- [ ] Golden JSON tests
- [ ] `studio-canvas` with moved selection code
- [ ] Gizmo math with tests
- [ ] wasm32 check

## Success criteria

- Both crates build for `wasm32-unknown-unknown` and native.
- `cargo test --manifest-path desktop/Cargo.toml -p studio-engine` passes unchanged.

## Risks

Large `WorkflowView` snapshots on long chats. Mitigation: rows are sent once and referenced by
`RowId`; deltas come later if measured necessary.
