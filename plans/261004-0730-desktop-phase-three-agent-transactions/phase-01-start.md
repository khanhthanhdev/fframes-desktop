---
title: "Stage 1: ACP driver, task and draft contracts"
status: todo
---

# Stage 1: ACP driver, task and draft contracts

## Context and dependency

Depends on existing M0–M2 implementation and passing baseline regressions. Estimate: 5–7 engineer-days. This stage establishes reusable contracts without publishing changes. One executor owns its files; ownership transfers after the gate.

Existing handwritten ACP parsing is at `desktop/crates/studio-agent-spike/src/session.rs:70`; initialization checks v1 and the correlated prompt response is authoritative at `session.rs:407`. Cancellation resolves permissions/reaps at `session.rs:177`; drop reaps at `session.rs:547`. Configuration includes executable/arguments/auth environment names at `session.rs:22`. Preserve proven spike behavior while adding production modules within this package.

Task state is per open-project session (`desktop/crates/studio-engine/src/state.rs:81`). Existing jobs are Build/Checkpoint (`state.rs:47`), and completion accepts only their source base (`state.rs:270`). The controller allocates a fresh job draft (`controller.rs:343`); production agent cwd therefore needs a separate stable draft. Platform app-data paths already exist (`app_paths.rs:8`).

## Requirements and data flow

- [x] Add a driver for discovery, auth status, session creation/restoration, prompt, cancellation, options and normalized events. Keep provider/session/task/sequence identities and structured redacted error data.
- [x] Select/pin an exact official Rust ACP SDK release with tested v1 support, compatible MSRV/license and explicit negotiation/rejection; do not silently use v2 examples.
- [x] Capture current source, prior checkpoint/history, assets/instructions and whole-project brief. Dirty imported source is the task source base; never reconcile the draft from an older checkpoint. No M4 scoped packet or M5 retrieval.
- [x] Define separate task states: context-ready, editing, waiting, quiescing, validating, repair-needed, candidate-ready, promoting, accepted, conflict, failed, cancelled, interrupted. Candidate artifact revision differs from task source base; leave M1 checkpoint semantics intact.
- [x] Reserve stable app-data draft cwd and one writer lease per project. Archive a retained failed draft before refreshing it for another task; never change cwd under a live provider session.

Discovery checks configured absolute path, managed adapter paths and an explicitly resolved GUI PATH; launch argument arrays without shell interpolation. Differentiate missing executable/runtime, protocol mismatch, auth unknown/required/rejected and ready. Initialization alone does not prove auth. Credentials remain provider-managed; configuration stores names only, and secret values are redacted before logs/transcripts/evidence persistence. No global PATH/rustup or implicit package installation.

Events cover message deltas, tool lifecycle, permission/input requests, advertised options, prompt finish and process/protocol failures. Ordinary clarification uses a correlated subsequent prompt in the same session if needed; do not invent a universal v1 question RPC. Reject duplicate/late replies. Only advertise tested client capabilities; initially decline optional filesystem/terminal services. Images/resources/restoration/MCP require negotiated support; text-only mode returns bounded diagnostics/artifact paths with visible limitations.

A successful authoritative `end_turn` makes a task eligible for quiescing, not accepted. Refusal/cancel/exhausted limits cannot auto-apply. Close pending requests and stop/reap the adapter plus owned helpers before immutable capture. A stable draft need not mean a persistent writer process: restart/restore at the same cwd only if safe context refresh is proven; otherwise create a new session. Protocol completion, source scans and quiet periods cannot establish writer quiescence alone. Stop/timeout closes permissions as cancelled, reaps the owned tree, retains draft and rejects late events.

The initial Linux ownership route is a specifically qualified adapter whose writer descendants remain in its task process group. Strengthen bootstrap cleanup to verify terminal group membership after graceful/forced termination, not just direct-child exit. Record and test that adapter's writer model before enabling candidate capture. Existing groups cannot observe helpers that escape through `setsid`; do not claim general detached-writer detection. An adapter with detached/background writers or unknown ownership stays unsupported for Apply and retains its draft. An escaped-helper fixture must demonstrate this limitation and fail the qualification gate. Do not refresh/reuse that draft while an escaped writer may survive. Supporting such an adapter requires separately planned containment with verified membership and available host permissions; it cannot silently fall back to repeated scans or a fresh session.

Initial engineering bounds: 1 MiB wire message, 256 queued normalized events with batched deltas, 4 MiB resident transcript with bounded paged history, 1 MiB stderr tail; 30s initialize, 15min prompt and 2s graceful shutdown before forced owned-tree cleanup. Preserve critical events or fail visibly on overflow. Adjust only with measured evidence.

## Files owned

All new paths are proposed, not existing symbols. Shared engine files pass sequentially to later stages.

| Action | Absolute path | Responsibility |
|---|---|---|
| Modify | `/root/fframes-desktop/desktop/Cargo.toml`, `/root/fframes-desktop/desktop/Cargo.lock`, `/root/fframes-desktop/desktop/crates/studio-agent-spike/Cargo.toml` | Pin official SDK dependency; retain upstream GPUI pin and existing package name. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-agent-spike/src/driver.rs`, `/root/fframes-desktop/desktop/crates/studio-agent-spike/src/session.rs`, `/root/fframes-desktop/desktop/crates/studio-agent-spike/src/supervisor.rs`, `/root/fframes-desktop/desktop/crates/studio-agent-spike/src/lib.rs` | Driver/event abstraction, SDK transport, supervision and compatible exports. |
| Create | `/root/fframes-desktop/desktop/crates/studio-agent-spike/src/discovery.rs` | Executable/runtime/auth feedback. |
| Create | `/root/fframes-desktop/desktop/crates/studio-engine/src/agent-task.rs` | Task identities/lifecycle, stable draft, writer lease and repair budget. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/controller.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/src/app_paths.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/src/lib.rs` | Serialized ownership and app-data integration; preserve existing job routes. |
| Create | `/root/fframes-desktop/desktop/crates/studio-agent-spike/tests/acp-v1.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/tests/agent-task.rs` | Transport and task boundary scenarios. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-agent-spike/tests/acp-peer.py` | Extend real subprocess wire fixture; it cannot qualify authentication. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-bootstrap/src/process.rs` | Verify terminal task-group cleanup after forced termination; preserve unrelated/root process ownership. |

Preserve legacy exported surface (`desktop/crates/studio-agent-spike/src/lib.rs:4`) and consumers: `desktop/app/src/agent_spike.rs:18`, `desktop/app/src/worker_project.rs:7`, `desktop/app/src/app.rs:540`, `desktop/app/tests/worker_roundtrip.rs:3`. Keep `spike-ui` and existing worker behavior; introduce wrappers where the SDK changes internal types.

## Steps and test matrix

1. Run `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-agent-spike -p studio-engine -p studio-project` before changes; record baseline failures honestly.
2. Select SDK pin; prove typed v1 initialization/prompt/permission behavior and executor integration off GPUI. Reuse owned process scopes (`desktop/crates/studio-bootstrap/src/process.rs:562`).
3. Implement task/source/candidate identities, stable draft archive/refresh and one-writer lease; retain current-source Build/Checkpoint jobs.
4. Add unit state tests for stale session/generation, dirty import base versus checkpoint, repeat tasks, finite repair counters, counter exhaustion and lease release.
5. Add real subprocess tests for discovery/auth error, malformed/truncated/oversized messages, unsupported version/options, authoritative completion, late events, permission/question cancellation, blocked stdin, deadlines, helper cleanup, post-kill group verification and unsafe refresh fallback. A setsid-escaped helper must remain unqualified under the process-group route; retain its draft and clean up only its known test-owned PID.
6. Prove no snapshot proceeds while any owned writer remains; invalid source retains draft/checkpoint and never resurrects obsolete task after edit-back.

Focused commands: repeat the baseline package tests, run `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-bootstrap` for shared cleanup, then `cargo check --locked --manifest-path desktop/Cargo.toml -p fframes-studio` for compatibility imports. Proposed new tests are created before invoking their targets. Record SDK and actual adapter versions; fixtures only prove transport behavior.

## Risks, response and rollback

| Likelihood × impact | Failure and pre-decided response |
|---|---|
| Medium × high | SDK v1/MSRV/executor incompatibility: stop adoption, select a compatible tested pin; keep legacy spike, never silently upgrade protocol. |
| Medium × high | Detached writer or unsafe restoration: mark adapter ownership unsupported, retain/avoid reusing draft and block capture; a fresh session cannot resolve an unknown surviving writer. |
| Medium × high | Secret disclosure: redact before persistence, test sentinel credentials, refuse evidence export with secrets. |
| High × medium | Streaming pressure: batch/coalesce deltas, bound storage/queues and fail critical-event overflow visibly. |

Process ownership and draft cwd provide workflow isolation, not a sandbox against arbitrary adapter tools or Cargo scripts. Rollback disables production routing and restores dependency pins/compatibility wrappers; retains drafts and portable source. This stage adds no source mutation recovery or checkpoint migration.

## Gate

- [x] Pinned official SDK negotiates v1 explicitly and passes existing/new transport regressions.
- [x] Stable draft, current-source base, prior checkpoint and candidate identities stay distinct across tasks/reopens.
- [x] Stop/timeout/failure resolves requests, reaps owned writers and preserves drafts; unproven quiescence blocks capture.

Sources: [official Rust SDK](https://github.com/agentclientprotocol/rust-sdk), [v1 prompt turns](https://agentclientprotocol.com/protocol/v1/prompt-turn), [research](../reports/research-261004-0730-desktop-phase-three-acp-contract.md). Exact SDK pin is an execution gate.
