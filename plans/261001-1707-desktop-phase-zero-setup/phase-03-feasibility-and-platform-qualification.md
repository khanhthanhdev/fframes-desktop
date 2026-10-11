---
phase: 3
title: "Qualify feasibility boundaries and platforms"
status: pending
priority: P1
effort: "3-5 engineer days"
dependencies: [1, 2]
---

# Phase 3: Qualify feasibility boundaries and platforms

## Goal

Complete all five confirmed M0 checks with measured native evidence: replace CLI-per-frame bootstrap with a versioned crash-isolated worker, run one current ACP adapter through a real authenticated edit, prove an explicit source anchor against the displayed revision, and decide which of Linux x64, Windows x64 and macOS arm64 actually pass the app/setup/project path.

## Context and fixed boundaries

- The user's confirmed M0 scope includes all five experiments. An unavailable real provider account is recorded as `NOT RUN` and leaves the ACP gate unmet; deterministic fixtures alone cannot qualify a connector ([implementation-plan.md:38-48](../../docs/desktop/implementation-plan.md#L38), [implementation-plan.md:182](../../docs/desktop/implementation-plan.md#L182)).
- Linux x64 is the confirmed first GUI target. Windows x64 and macOS arm64 still require native app/setup/project qualification; a blocked target keeps its relevant gate unmet rather than inheriting Linux results.
- `Previewer` keeps timeline, fonts, images, converter/text caches and decoder workers alive and can report the compiled timeline ([preview.rs:237-299](../../fframes/src/renderer/preview.rs#L237), [preview.rs:314-355](../../fframes/src/renderer/preview.rs#L314)). The worker owns the concrete `Video`, media and `Previewer`; GPUI never owns or loads that type.
- The worker is process crash isolation, not a security sandbox: project build scripts, procedural macros, constructors and agent tools execute code ([architecture.md:66-70](../../docs/desktop/architecture.md#L66)). The Phase 0 UI must describe this honestly.
- Start with bounded control + binary pipes and measure before shared memory ([architecture.md:303-319](../../docs/desktop/architecture.md#L303)). The native winit player cannot run inside GPUI; this path directly presents worker pixels.
- Selection starts with explicit semantic IDs and source registration. Do not modify `svgr!` or claim automatic macro spans until this annotated fixture proves revision/hash/span behavior ([architecture.md:220-229](../../docs/desktop/architecture.md#L220)).

## Files to create or modify

| Action | Path | Ownership and purpose |
|---|---|---|
| Modify | `Cargo.toml` and `Cargo.lock` | Keep the Phase 1 GPUI-free protocol member, add only `fframes-studio-runtime`, and retain `exclude = ["desktop"]`. |
| Create | `fframes-studio-runtime/Cargo.toml` and `src/{lib,worker,anchors}.rs` | Borrowed serving loop around `Previewer`, renderer, registrations and transport; the generated binary owns video/media/options as stack locals. |
| Modify | `fframes-studio-protocol/src/lib.rs` | Finalize hello/timeline/frame/error/shutdown and element metadata contracts without GPUI types. This remains the single schema owner. |
| Modify | `desktop/Cargo.toml` and `desktop/Cargo.lock` | Add only `studio-agent-spike`; pin the selected official ACP Rust SDK candidate exactly. |
| Create | `desktop/crates/studio-agent-spike/Cargo.toml` and `src/{lib,driver,supervisor}.rs` | Minimal app-owned ACP v1 client/adapter qualification boundary, separate from future full provider UX. |
| Modify | `desktop/crates/studio-bootstrap/src/process.rs` | Cross-platform worker/agent process groups, stderr drain, bounded logs, cancellation and crash events. |
| Modify | `desktop/crates/studio-sdk/src/project.rs` | Add the worker entry/annotation overlay to a generated project and build the immutable worker artifact. |
| Modify | `desktop/app/src/{app,frame_image,setup_view}.rs` | Connect worker requests, latest-seek state, crash status, ACP task controls and selected-source evidence to the minimal window. |
| Create | `desktop/app/src/{worker_client,agent_spike,selection_spike}.rs` | UI-independent clients/state for the three integration gates. |
| Create | `desktop/fixtures/annotated-video-overlay/` | Worker entry plus one scene/element registration layered onto the real generated CPU template; source markers and explicit SVG ID only, no macro changes. |
| Modify | `desktop/packaging/sdk/phase-zero-sdk.json` | Record final qualified compiler/GPUI/fframes/FFmpeg/ACP/worker pins and target status. |
| Modify | `desktop/packaging/sdk/compatibility.schema.json` and `desktop/scripts/build-phase-zero-sdk.{sh,ps1}` | Include checksum-pinned runtime/protocol sources with standalone manifests and relocatable offline Cargo patch/vendor configuration; assemble a new SDK ID for the expanded worker graph. |
| Create | `docs/desktop/phase-zero-feasibility.md` | Durable decision record for selected pins, transport, first agent and supported platform; link measurements rather than copying volatile logs. |
| Create | `desktop/qualification/m0-results.schema.json` | Machine-readable evidence contract used by native qualification. |
| Create | `plans/reports/<date>-phase-zero-results.md` | Stateful native run summary with links to redacted CI/manual artifacts; no credentials or machine-user paths. |
| Create | `.github/workflows/desktop-phase-zero.yml` | Native Ubuntu 24.04 x64, Windows x64 MSVC and macOS arm64 build/package/fixture jobs; manual authenticated ACP job remains local/manual unless a safe CI account is explicitly provisioned. |

Sequential modifications to Phase 1/2 owners are integration work in this phase. Do not scaffold the future project database, timeline/audio playback, full agent panel, provider picker, macro instrumentation, production installer or updater.

## Worker protocol and data flow

Use length-delimited JSON control messages on dedicated control pipes, stderr for logs, and a separate bounded binary frame pipe/OS handle. Stdout must never mix logs with protocol. Hello negotiation includes:

```text
protocol_version/range
project_id + source_revision
worker_generation
fframes/sdk/runtime versions
frame_contract_version
max_frame_bytes
capabilities: timeline, frame, element_metadata, shutdown
```

Every request/response carries `protocol_version`, `source_revision`, `worker_generation` and `request_id`. A frame header additionally carries frame index, dimensions, stride, channel order, alpha mode, color space and payload length as defined in Phase 1. Reject unsupported versions, stale revisions/generations, duplicate/unknown request IDs, oversized payloads, integer overflow, short/long reads and frames outside the compiled timeline before allocation/presentation.

The generated `studio_worker` entry constructs media, concrete video, options and renderer as ordinary stack locals, constructs `Previewer` from their references, and enters a proposed `serve(&mut previewer, &mut renderer, registrations, transport)` loop. The loop cannot outlive those locals. No self-referential owning runtime, leaked values or unsafe lifetime extension is required.

`RenderFrame(frame)` runs through that long-lived `Previewer` and `CpuFrameRenderer`, returning an actual straight-RGBA `RgbaFrame`. The app conversion remains the single GPUI boundary. Queues allow one active seek and one replaceable pending seek/response. One dedicated binary reader validates each complete header and drains exactly its admitted payload even if superseded; discarding a stale control message must never stop reading its bytes. Coalesce before sending, then finish an admitted frame. Correlate control completion and binary payload by revision/generation/request ID; publish only a complete matching current result. Stale payloads may be streamed into bounded discard storage. Recreate both transports for every new worker generation. Test supersession during a partial write larger than OS pipe capacity and prove the newer seek completes. EOF, malformed message, timeout or nonzero exit closes that worker's transports, preserves the last valid image, reaps the child and allows explicit restart.

For clean-user distribution, M0 uses SDK-vendored runtime/protocol sources rather than requiring publication. Assembly writes standalone manifests without inherited checkout/workspace paths, records their exact versions/source hashes, and includes their dependencies in the offline graph. The generated project keeps versioned dependency declarations; the build service injects a generated app-local Cargo patch configuration pointing to the currently resolved SDK sources. Absolute installation paths stay out of portable source. Verify all four consumers: core workspace, desktop app, generated worker and relocated offline SDK. Rebuild fresh project B with the worker overlay and an empty target directory under network denial; CLI-only offline success is insufficient for Phase 3.

Capture per-request render, pipe transfer, conversion and upload time, payload bytes, queue high-water mark and actual image/cache release evidence. Run 1,000 alternating/latest seeks and a forced worker abort. M0 passes only if the last requested frame appears, memory/queues stay bounded, and the app remains interactive/restartable. Independently test known byte-level RGBA/alpha/padded-row fixtures across the real binary transport and native presentation/readback. CLI/worker comparison shares renderer code and serves only as a secondary semantic check; it cannot replace the independent boundary fixture or the real fframes frame gate.

## Explicit selection/source-anchor experiment

The overlay gives one visible title an SVG `id` such as `intro.title` and registers:

```text
scene_instance_id + element_id + instance_key
frame-specific transformed bounds + paint order
workspace-relative Rust path + containing symbol
source file SHA-256 + marker-derived byte span
source_revision + worker_generation + frame/request identity
```

Use paired source markers around the exact title implementation in the fixture. At worker build/index time, locate one unambiguous pair, calculate the UTF-8 byte span and hash the whole source content. The worker returns element metadata for the same rendered frame. The app maps the click through displayed image letterboxing into video pixels, chooses the topmost registered bound, then rereads the current file and validates project root containment, revision, generation, whole-file hash, marker uniqueness, byte boundaries and containing symbol before showing the snippet.

Tests cover a valid title, click outside, overlap/paint order, repeated ID rejection, path traversal, invalid UTF-8 boundary, stale frame/generation, modified file/hash, moved markers and deleted element. Any mismatch invalidates the selection and asks for rebuild/reselection; it never returns best-effort code as an exact source result. Automatic macro spans remain out of scope.

## Real ACP experiment

1. Inspect the current official ACP Rust library and adapter distributions at execution time, then pin exact versions/hashes in Cargo.lock and the compatibility manifest. Treat research registry versions as observations, not final pins ([research.md:67-82](../../docs/desktop/research.md#L67)).
2. Choose the first provider by real evidence: executable discovery/auth succeeds, ACP v1 initialize/capability negotiation completes, a session opens at the isolated draft project, streaming and authoritative prompt completion behave correctly, and the provider can edit/build within the declared access mode. Record every evaluated adapter; do not claim Claude, Codex, Pi or Antigravity support from registry presence.
3. Credentials remain in provider-managed or OS credential storage ([architecture.md:119](../../docs/desktop/architecture.md#L119)). Start adapters with an allowlisted environment and the qualified provider auth lookup route. An explicitly required secret is injected only into that child and excluded from snapshots. Evidence records secret-variable names/presence, never their values. Redact known injected values and sensitive protocol fields before persistence; raw authentication payloads and raw environment dumps are never retained. Test a sentinel token in stderr and an unexpected protocol field. The supervisor uses owned Unix process groups or Windows kill-on-close Job Objects, drains bounded stderr, retains sanitized structured errors and shows the actual access policy.
4. Run one authenticated task against a copy of the generated project: ask the provider to change the annotated title text. Exercise one genuine input/question or permission request and resolve it from the minimal UI; if the selected adapter cannot emit one in its supported flow, that capability row fails rather than being simulated.
5. Wait for ACP's authoritative prompt response/stop reason, never stream quiet or a tool completion. Freeze/hash the changed draft, rebuild the worker with the app-controlled SDK, start a new generation, render the same frame and verify changed pixels/text plus a refreshed valid source anchor.
6. Start a second harmless task and cancel from the UI. Resolve pending permission/input as cancelled, close protocol pipes, reap the exact adapter process tree, retain draft/log evidence and verify no writer/build continues. A killed GUI with an orphaned adapter fails the gate.

If no real authenticated account is available, record provider, adapter/version, platform and `NOT RUN — credentials unavailable`. Continue worker/selection/platform work, but leave the ACP M0 acceptance checkbox unchecked and do not select a first agent.

## Tasks and steps

1. **Implement the GPUI-free runtime and one schema owner.**
   - [ ] Extend the Phase 1 root protocol and implement the borrowed runtime serving loop; generated workers own their video/media/options locals. Compile and run shutdown/abort/restart fixtures without unsafe lifetime extensions.
   - [ ] Implement hello negotiation, timeline, latest-wins frame, element metadata, structured error and graceful shutdown using `Previewer` and the CPU renderer. The binary reader must drain admitted stale payloads and correlate both transports before publishing a frame.
   - [ ] Package the exact runtime/protocol sources as standalone SDK crates; inject relocatable app-local Cargo patches, vendor their complete dependency graph and qualify all four consumers plus offline worker project B after SDK relocation.
   - [ ] Add malformed-message/frame-limit/stale-generation tests and a deterministic protocol fixture for crash, partial read, old response and ignored cancellation.
   - [ ] Prove root dependency trees contain no GPUI, GPUI platform or desktop app crate.

2. **Connect and stress the real worker.**
   - [ ] Overlay/build `studio_worker` into a real Phase 2 generated project and launch the immutable executable from the app with the SDK child environment.
   - [ ] Present timeline and frame zero, issue repeated and rapid seeks, validate pixel reference colors/alpha, and expose measured stage timings/queue counters.
   - [ ] Force panic/abort and malformed output; preserve the last frame, keep input responsive, show crash status, reap pipes/process, and restart a fresh generation.
   - [ ] Pass known byte-level RGBA/alpha/padded-row fixtures through the binary transport and native presentation/readback, then compare the actual project's frame with CLI CPU `frame` output. Both checks are required; neither replaces real rendering.

3. **Prove the annotated source anchor.**
   - [ ] Add exactly one scene and title registration to the overlay without changing `svgr!` internals.
   - [ ] Click the title in the displayed real frame and return the exact containing Rust implementation with matching revision/hash/span.
   - [ ] Change the file after rendering and verify selection rejects stale metadata; rebuild, reselect and verify the new hash/span.
   - [ ] Run the negative test matrix above and retain machine-readable results.

4. **Qualify one real ACP provider.**
   - [ ] Enumerate current candidates and record adapter distribution/version/hash, OS, auth path and negotiated capabilities.
   - [ ] Select the first only after a real authenticated title edit passes streaming, real question/permission resolution, authoritative completion and controlled rebuild.
   - [ ] Cancel a live second task and prove adapter plus owned descendants exit while GPUI remains open and the draft is retained.
   - [ ] Record all unexecuted providers as `NOT RUN` and failed providers with the exact failing lifecycle stage; do not generalize the selected result.

5. **Run native platform qualification and decide.**
   - [ ] Ubuntu 24.04 x64: app artifact launch under X11 and Wayland; IME/font/image; full app setup; online project A and offline project B; worker seek/crash; selection; authenticated ACP where available.
   - [ ] Windows 11 x64 MSVC: app artifact with VC runtime/DLL inventory; native input/image; guided VS SDK/LLVM setup; FFmpeg build/runtime DLL load; project A/B; worker seek/crash; selection. Do not enable unsupported source-codec features.
   - [ ] macOS arm64: native `.app` launch; Xcode/Metal/font-kit prerequisites; input/glyph/image; project A/B; worker seek/crash; selection. Record unsigned local test status; notarization is not an M0 pass condition.
   - [ ] Measure cold/warm build, SDK/app/offline-bundle size, agent startup/first token/completion/cancel, frame render/transfer/convert/upload, seek supersession, memory/queue high-water and cleanup.
   - [ ] Write `docs/desktop/phase-zero-feasibility.md` with final exact pins, worker transport, selected first agent or `UNSELECTED`, Linux-first supported target decision, per-platform pass/blockers and go/no-go recommendations for M1/M2/M3. Link the dated report and artifacts; evidence, not optimism, determines advertised support.

## Test matrix

| Layer | Automated check | Native/manual evidence |
|---|---|---|
| Unit | Protocol version/length/overflow/frame enums; RGBA→BGRA/stride/alpha; latest-wins reducer; anchor path/hash/span validation; ACP event-state transitions | None |
| Integration | Spawn fixture worker; hello/timeline/real frame; partial/oversize/stale message; crash/restart; owned cancellation; SDK environment isolation | Actual fframes project/FFmpeg linkage on each target |
| End to end | Package → doctor → SDK → project A → worker frame → selection; offline fresh project B; authenticated edit/cancel when credentials exist | GPUI window, real IME/font/GPU backend, OS elevation review, DLL/dylib/so inventory, real account |
| Soak/failure | 1,000 latest seeks; supersession mid-payload beyond pipe capacity; worker restart with new transports; nested Windows child cleanup; generation interruption/promotion conflict; cancelled download/build/agent; corrupt SDK artifact | 10-minute interaction with memory/process sampling on the Linux reference machine |

## Verification commands

Implementation is tracked in [remaining gates](../261001-2034-desktop-remaining-gates/plan.md). The qualification checkboxes remain open until reproducible native evidence exists. The implemented spike UI and ledger validator are invoked below; authenticated credentials are never supplied as command arguments or CI artifacts.

```bash
cargo fmt --all -- --check
cargo test -p fframes-studio-protocol -p fframes-studio-runtime
cargo clippy -p fframes-studio-protocol -p fframes-studio-runtime --all-targets -- -D warnings
cargo +1.98.1 test --locked --manifest-path desktop/Cargo.toml
cargo +1.98.1 clippy --locked --manifest-path desktop/Cargo.toml --all-targets -- -D warnings
cargo +1.98.1 run --locked --manifest-path desktop/Cargo.toml -p fframes-studio -- spike-ui
python3 desktop/scripts/validate-qualification.py desktop/qualification/m0-results.json
```

Native CI builds/packages all three targets on their native runners and runs noninteractive unit/integration checks. GUI/IME/GPU and administrative setup evidence comes from clean native VMs/accounts or a GUI-capable runner; a headless cross-compile does not count. The Linux job pins Ubuntu 24.04, and macOS must be an Apple Silicon runner. Root regression gates remain `cargo test -p fframes -p fframes-media -p cargo-fframes` plus the repository's `just clippy` before merge.

## Measurable M0 pass gates

| Gate | Pass condition | Failure status |
|---|---|---|
| Managed native compilation | On each target, the minimal app completes guided setup, builds/renders generated project A, then builds/renders fresh project B offline without repo/home caches. | Target `BLOCKED`; local-build support remains unadvertised. |
| GPUI startup | Linux x64 passes X11 + Wayland window, IME, font and real worker image; Windows/macOS each have native build/setup plans and recorded native runtime results. | GPUI M0 fails on the first target; a secondary target failure blocks that target. |
| Renderer worker | Real `Previewer` frame matches CLI CPU output, latest of 1,000 seeks appears with bounded queues/resources, and forced crash leaves app alive/restartable. | Worker M0 fails; no M2 work begins. |
| ACP task | One pinned adapter with real account streams, resolves actual question/permission, completes an edit, rebuilds changed output, and cancels/reaps a second task. | `NOT RUN` or `FAILED`; no first agent may be claimed. |
| Selection anchor | Clicking annotated title returns its exact implementation at displayed revision/hash/span; stale/path/duplicate cases reject. | Selection M0 fails; keep element selection out of later advertised scope. |

All five confirmed gates must pass for an unconditional M0 completion. The decision record may recommend continuing isolated M1 foundation work with a named blocker only if it makes that exception explicit; it may not relabel an unmet gate as passed.

## Risks, stop conditions, and rollback

| Risk | Likelihood × impact | Stop condition / mitigation | Rollback |
|---|---|---|---|
| Worker pipe copies are too slow or memory grows | Medium × High | Measure each stage and queue high-water; stop if bounded latest-wins transport cannot recover during 1,000 seeks. | Keep CLI frame bootstrap from Phase 2; plan shared-memory transport separately from measured evidence. |
| Crash cleanup harms unrelated processes | Low × Critical | Track exact child/process-group identity and test with an unrelated sentinel process. | Disable automatic restart/cancel path; retain diagnostic manual cleanup. |
| ACP adapter acknowledges but does not complete reliably | High × High | Require protocol stop reason, edit/build result and repeated cancellation. | Mark candidate failed and test the next current adapter; do not add a native driver inside M0 unless a specific accepted replan calls for it. |
| Credentials or prompt data leak to evidence | Low × Critical | Provider-managed auth, structured redaction, bounded stderr and review before storing run artifacts. | Delete/quarantine affected local evidence, rotate exposed credential through provider, leave ACP gate unmet. |
| Anchor points at stale/wrong code | Medium × High | Revision/generation/hash/span/path/symbol checks; reject ambiguity. | Fall back to scene/range selection; do not change macro/runtime. |
| Native target passes build but fails GUI graphics | Medium × High | Native IME/image/driver execution is mandatory; CPU fframes success is separate. | Mark GUI target blocked and keep Linux-first result; do not claim GPU-less support. |
| GPL/H.264 or binary notices are unresolved | Medium × High | Keep Phase 0 bundle internal and record exact profile/source/notices; product/legal decision gates distribution. | Retain internal feasibility artifact only; make no release-package claim. |

Rollback is phase-local: the app can return to Phase 2 CLI-render bootstrap; generated projects remain ordinary Rust crates; the worker/runtime/protocol additions are additive and removable; active SDK pointer returns to its previous immutable version; no global PATH/rustup/SDK state was mutated by the app. Stop every process started by the qualification harness before deleting run directories or native VM snapshots.

## Deliverable and completion

- [ ] `docs/desktop/phase-zero-feasibility.md` names exact tested pins, transport, first agent or unmet auth gate, initial supported target and every unresolved native dependency.
- [ ] Each result links a manifest digest, native machine/OS/tool versions, command/environment record, metrics and redacted logs.
- [ ] Minimal app and SDK artifacts can reproduce the passing clean-account flows without the development checkout.
- [ ] M1 remains the future full shell/persistence milestone and M7 remains production signing/installer/update work; neither is smuggled into this spike.
- [ ] The plan review report at [plan-review-261001-1707-desktop-phase-zero.md](../reports/plan-review-261001-1707-desktop-phase-zero.md) has no unresolved blocking finding before implementation handoff.

## Windows development run (2026-10-10)

Windows status (2026-10-10, rerun 2026-10-11): development evidence only, see the [M0 ledger](../../desktop/qualification/m0-results.json) `additional_platforms`. Passed: workspace tests, SDK assembly with offline double build, fresh-home managed compilation, FFmpeg DLL load at build and run, native SendInput typing and preview selection on a virtual display, process cleanup. Still open: VC runtime on a clean machine (the executables now link the C runtime statically, but no clean-machine launch ran), guided VS/LLVM first-run setup in the product shell, worker crash/restart, separate account, network-disabled build, physical display and IME. The Windows checkbox above stays unchecked.
