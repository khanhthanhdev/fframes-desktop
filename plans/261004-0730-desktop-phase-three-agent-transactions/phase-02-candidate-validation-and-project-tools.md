---
title: "Stage 2: Immutable candidate validation and shared project tools"
status: todo
---

# Stage 2: Immutable candidate validation and shared project tools

## Context and dependencies

Depends on Stage 1 identities, driver completion and writer quiescence. Estimate: 6–8 engineer-days. One executor owns build/tool integration; source promotion remains disabled until Stage 3.

Immutable objects are captured/verified by `desktop/crates/studio-project/src/checkpoint.rs:26` and loaded at `checkpoint.rs:74`; SDK-bound materialization starts at `desktop/crates/studio-engine/src/build_materialization.rs:41`. Compilation always creates a fresh materialization (`desktop/app/src/worker_project.rs:79`), while `acquire_build_lock` (`worker_project.rs:47`) serializes shared Cargo target writes. `PreviewCoordinator::build` cancels its predecessor (`desktop/app/src/preview_coordinator.rs:108`): it is not a dedup service.

M2 preparation yields matching timeline, inspection, first frame and PCM (`desktop/crates/studio-engine/src/preview_state.rs:136`). Existing preview identity derives from the operation source base (`preview_state.rs:8`) and installation compares it to current portable source (`preview_state.rs:324`). M3 must stage candidate rendering separately without pretending candidate bytes equal task base.

## Requirements and data flow

- [x] Quiesced draft enters immutable object capture; verify twice and retain manifest/object hashes, then build only that immutable revision. A changing draft fails capture and retains previous playback.
- [x] Create a shared compile service keyed by project ID, immutable source revision, exact SDK compatibility digest/toolchain/target, package/worker entry, features/build profile and backend/options. Session/task/generation are subscriber authority, not substitute cache keys.
- [x] All UI/tool requests acquire the same in-flight compilation or leased completed artifact. Preserve target lock and isolated binary copy; cancellation detaches one subscriber, and kills underlying compile only when no consumers remain or project closes.
- [x] Produce an immutable validation report naming task base, candidate revision, build key, diagnostics, coverage, representative artifact hashes and prepared audio identity. A cached compiler success alone cannot qualify a task.
- [x] Expose exactly six operations: `project_context`, `timeline`, `render_frame`, `render_strip`, `inspect`, `build_status`; use one typed dispatcher for app, CLI and negotiated MCP. No arbitrary shell/filesystem writes, generalized selection/style/source lookup or new export tools.

Validation requires compile success, negotiated worker/timeline availability, supported CPU backend, no critical inspection errors/truncation and representative real frames. Cover start/end and scene boundaries plus current playhead and a bounded evenly spaced sample (initial maximum 24 frames). Whole-project task is the initial context; if shared functions change or targeted coverage cannot be established, broaden inspection to the full video with bounded batches rather than silently omit affected scenes. Warn visibly for unsupported shaders. Audio changes require bounded local checks of the prepared PCM and compiled timeline: matching revision, sample rate/channel geometry, finite samples, expected duration and track placement. Record clipping/silence diagnostics and actual coverage; valid worker-only imports do not need a video CLI. Existing CLI analysis can supply additional comparison evidence where available, without expanding the six-tool facade. Structured report states actual coverage and limitations.

Failure emits bounded compiler/inspection context, changed paths, candidate/source/build identities and repair count. At most one automatic repair turn reopens the stable draft, reaps the writer again and captures a new immutable candidate. A second failure is terminal/retained for user review; Stop is available in every state. No pixel-difference or title-anchor requirement from the M0 spike: a legitimate edit can leave frame zero unchanged or change audio only.

Tool calls bind to a short-lived app-local session capability and task/project/revision; reject cross-project IDs, traversal, symlink escape, unbounded ranges and stale/closed tasks. Mutable draft requests first acquire the writer gate and capture a labeled immutable draft revision, or return busy; never compile a changing tree. Tool builds are explicitly unvalidated until the acceptance validator runs. `build_status` observes work and does not launch Cargo. Artifacts are app-owned IDs/paths with expiry and size limits, not full-resolution base64 in ACP control.

CLI is a proposed `studio-tools` binary in the existing app package. It connects to an app-owned local broker rather than invoking Cargo independently. MCP stdio adapter forwards to that same dispatcher/broker; stdout contains only newline JSON-RPC, stderr bounded logs. Use owner-only socket permissions or authenticated loopback with an app-local secret capability passed outside portable source; no secret command arguments/logs. Broker outlives neither project nor app. Record exact negotiated MCP pin/version supported by the qualified provider; otherwise CLI remains available and MCP is marked unsupported.

Separate compiler sharing from render lanes: tool workers consume the same compiled materialization but cannot steal the displayed worker or thumbnail lane. Keep `Arc<MaterializedBuild>` leases while workers/audio/artifacts exist. Initial limits: 16 queued tool calls, two tool worker consumers, 8 MiB image artifact, 256 KiB text reply and eight leased completed build entries with a 2 GiB eviction target; refuse rather than evict live leases. Close releases all owned consumers. Measure bounds during qualification.

## Files owned

| Action | Absolute path | Responsibility |
|---|---|---|
| Create | `/root/fframes-desktop/desktop/app/src/build-service.rs` | In-flight/result sharing, subscriber cancellation and leases. |
| Create | `/root/fframes-desktop/desktop/app/src/agent-tools.rs`, `/root/fframes-desktop/desktop/app/src/bin/studio-tools.rs`, `/root/fframes-desktop/desktop/app/src/bin/studio-mcp.rs` | Typed six-operation dispatcher, local broker and CLI/MCP adapters. |
| Modify | `/root/fframes-desktop/desktop/app/src/worker_project.rs`, `/root/fframes-desktop/desktop/app/src/preview_coordinator.rs`, `/root/fframes-desktop/desktop/app/src/preview_worker_client.rs`, `/root/fframes-desktop/desktop/app/src/studio_shell.rs`, `/root/fframes-desktop/desktop/app/src/lib.rs` | Route preparation/compilation through shared service; preserve existing locks/transport. |
| Modify | `/root/fframes-desktop/desktop/app/Cargo.toml`, `/root/fframes-desktop/desktop/Cargo.toml`, `/root/fframes-desktop/desktop/Cargo.lock` | Register facade binaries and compatible MCP dependency pin; set default-run to fframes-studio so existing cargo run commands stay compatible. |
| Create | `/root/fframes-desktop/desktop/crates/studio-engine/src/candidate-validation.rs` | Immutable validation report, coverage and repair policy. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-engine/src/agent-task.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/src/lib.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/src/build_materialization.rs` | Candidate/build identities and immutable lease integration. |
| Modify | `/root/fframes-desktop/desktop/crates/studio-agent-spike/src/session.rs` | Pass tested task-specific MCP configuration instead of empty list. |
| Create | `/root/fframes-desktop/desktop/app/tests/build-sharing.rs`, `/root/fframes-desktop/desktop/app/tests/agent-tools.rs`, `/root/fframes-desktop/desktop/crates/studio-engine/tests/candidate-validation.rs` | Sharing, facade parity, validation/repair contracts. |
| Modify | `/root/fframes-desktop/desktop/app/tests/preview_coordination.rs`, `/root/fframes-desktop/desktop/app/tests/audio_preview.rs`, `/root/fframes-desktop/desktop/app/tests/timeline_controls.rs` | Preserve existing callers and immutable playback checks. |

Current build callers are `desktop/app/src/worker_project.rs:20` and `desktop/app/src/preview_coordinator.rs:247`. Build-spec consumers are `desktop/app/src/studio_shell.rs:235`, `desktop/app/tests/preview_coordination.rs:46`, `desktop/app/tests/audio_preview.rs:162`, `desktop/app/tests/timeline_controls.rs:181`. Migrate them without changing legacy `--worker` or additive `--preview-worker` protocols. No root runtime/protocol API changes are planned.

## Steps and validation matrix

1. Implement keyed in-flight sharing above existing target lock; UI compile path first, then tools. Bind every completion to its subscriber identity; a shared artifact never grants install authority.
2. Capture immutable candidate and implement deterministic report/repair routing. Prepare matching playback without publishing it; keep existing preview playable on failure.
3. Implement typed dispatcher, protected local broker, CLI and optional MCP. Facades share exact revision/results/errors and cannot start a second compiler.
4. Unit test key separation (source/SDK/target/features/backend), stale identities, coverage/errors, one-repair limit, bounded replies and capability/path rejection.
5. Integrate concurrent GUI/CLI/MCP equal-key requests: count one Cargo process, cancel one subscriber, retain remaining result; different keys never reuse bytes. Test crash/close, eviction while leases live and broker reconnect refusal after task expiry.
6. Run real SDK worker checks and compare representative CPU frames/timeline/inspection with existing project CLI using identical assets/fonts/backend where that entry exists; worker-only imports use direct worker fixtures and prepared PCM checks. Test intentional compiler and inspection failures, audio-only edit, a custom-main import with no video CLI, and no change at frame zero.

Create proposed tests before running `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test build-sharing --test agent-tools`; run `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-engine` and existing `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test preview_coordination --test audio_preview --test timeline_controls`. Real SDK tests may require `-- --ignored` plus `SDK_BUNDLE` as documented by their source; record executed versus skipped paths. Do not report skipped real-worker tests as qualification.

## Risks, response and rollback

| Likelihood × impact | Failure and pre-decided response |
|---|---|
| High × high | False cache hit or cancellation sharing: include full compile environment in key, separate subscribers/process owners, invalidate uncertain entry. |
| Medium × high | Candidate/base confusion: immutable artifact revision independent of task base; old preview remains until guarded promotion. |
| Medium × high | Tool access escape: owner-only broker/capability, strict task/revision binding and path validation; reject unknown methods. |
| Medium × medium | Unsupported MCP/visual capability: qualify CLI route, show limitation and record unsupported MCP; do not fabricate success. |

Rollback disables facade/candidate routing and restores direct M2 compile preparation; retain immutable candidates/reports as local data, reap broker/tool workers and release leases. Existing M0/M1/M2 entry points remain available; no portable source/schema change occurs here.

## Gate

- [x] Immutable candidate validation and one bounded repair preserve old playback on every failure.
- [x] Equal-key UI/CLI/supported MCP requests compile once; cancellation and cache leasing remain isolated.
- [x] Six tools have revision/error/artifact parity, explicit limits and no unrestricted execution surface.

Source: [MCP transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports), [protocol research](../reports/research-261004-0730-desktop-phase-three-acp-contract.md). Select/test the compatible pin rather than assume this specification version is supported.
