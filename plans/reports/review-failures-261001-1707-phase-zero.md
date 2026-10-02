# Phase Zero plan failure-mode review

Scope: failure analysis and contract verification of `plan.md` and Phases 1–3 against the requested Cargo, generator, preview, architecture, and native-CI sources. Future experiments are treated as qualification work, not as existing support claims.

## Findings

### 1. High — Project creation is not transactional, so interruption leaves an unretryable partial project

**Plan location:** Phase 2, **Setup state machine and data flow**, lines 83–85; **Tasks and steps 4–5**, lines 113–122; plan-wide acceptance, `plan.md:48`.

**Source evidence:** `cargo-fframes/src/main.rs:398-405` rejects any non-empty destination, while `cargo-fframes/src/main.rs:545-561` creates directories and writes the project files one at a time. There is no staging directory or final rename. The plan's cancellation step only says to discard the build target directory (`phase-02-managed-sdk-and-project-setup.md:115`), so it does not repair generator output.

**Failing scenario:** the app invokes `cargo-fframes` directly in the user's chosen final directory and is killed after `Cargo.toml` or `src/lib.rs` is written. Relaunching against that path fails because it is non-empty; treating it as created would promote an incomplete project. A compile cancellation similarly leaves source at an ambiguous “created” state even though the displayed frame gate never passed.

**Correction:** generate into a unique sibling staging directory on the same filesystem; validate the generated manifest, add the overlay, build, and render there; promote with one atomic rename only when the chosen final path is still absent. Persist a small transaction marker outside the staging tree so restart can offer retry/cleanup, and test termination after every generator write and immediately before promotion. Never overwrite or merge into a now-nonempty user destination.

### 2. High — The worker/runtime crates have no relocatable route into a generated clean-user project

**Plan location:** Phase 3, **Files to create or modify**, lines 29–35; **Tasks and steps 1–2**, lines 96–103; Phase 2, **Compatibility manifest contract**, lines 48–54.

**Source evidence:** the standalone generator pins published `fframes` to the exact `cargo-fframes` package version and creates an independent workspace (`cargo-fframes/src/main.rs:452-477`). The proposed `fframes-studio-runtime` instead exists as a new root-workspace member (`phase-03-feasibility-and-platform-qualification.md:29-30`), while the architecture explicitly says generated projects depend on the renderer bridge (`docs/desktop/architecture.md:89-93`). The SDK manifest inventory lists fframes/cargo-fframes and vendored inputs but does not name distributable `fframes-studio-runtime` or `fframes-studio-protocol` artifacts (`phase-02-managed-sdk-and-project-setup.md:48-54`).

**Failing scenario:** Phase 3 overlays `studio_worker` into Project A/B on a clean machine with no repository checkout. A path dependency to the root runtime cannot resolve; a crates.io dependency also cannot resolve because the new crate has not been published or included in the offline vendor graph. The CLI-only Project B can pass Phase 2 while the actual worker graph fails offline.

**Correction:** define one relocatable distribution contract before Phase 3: publish exact compatible runtime/protocol versions or include their source as checksum-pinned SDK artifacts and vendor them into Cargo's offline source graph. Have the generated overlay use only that portable route. Extend the compatibility schema and assembly scripts to cover these two crates, then repeat the fresh-source/fresh-target/network-disabled Project B build **with the worker overlay**. Verify all consumers together: root workspace, nested desktop path consumer, generated worker, and offline vendor source.

### 3. High — The proposed long-lived worker ownership is self-referential under the current `Previewer` API

**Plan location:** Phase 3, **Context and fixed boundaries**, line 20; **Files to create or modify**, line 30; **Worker protocol and data flow**, line 63.

**Source evidence:** `Previewer` stores `video: &'a TVideo` and a cloned `RenderOptions<'a, 'media>` (`fframes/src/renderer/preview.rs:245-255`); `Previewer::new` explicitly borrows the video and media-bearing options (`fframes/src/renderer/preview.rs:257-271`). Rendering continues to dereference that borrowed video (`fframes/src/renderer/preview.rs:386-411`).

**Failing scenario:** a generic runtime/worker struct attempts to own the concrete `Video`, its media provider, and the `Previewer` described by the plan. That requires a safe Rust struct containing references to its own fields, which cannot be constructed or moved normally. Solving this ad hoc with leaked `Box` values would make restart/reaping and resource lifetime evidence unreliable.

**Correction:** make the generated `studio_worker` entry point own media, video, options, and renderer as ordinary stack locals, construct `Previewer` from their references, and then enter a runtime `serve(&mut previewer, &mut renderer, ...)` loop that cannot outlive them. Keep the reusable runtime generic over borrowed `Previewer`/renderer rather than owning the concrete video. Add a compile-and-run fixture that exercises construction, graceful shutdown, forced abort, and restart on every native target.

### 4. High — Initial doctor criteria require app-owned FFmpeg before the SDK installer can install it

**Plan location:** Phase 2, **Cross-platform prerequisite matrix**, lines 60–66; **Setup state machine and data flow**, lines 73–82; **Tasks and steps 2–3**, lines 100–109.

**Source evidence:** the plan assigns FFmpeg archives and caches to the app-owned SDK (`phase-02-managed-sdk-and-project-setup.md:19`, `:60-64`) but runs doctor before download/extraction and asks it to probe FFmpeg layout/load (`phase-02-managed-sdk-and-project-setup.md:73-82`, `:101`). The current native dependency really does require static FFmpeg off Windows and shared FFmpeg on Windows (`fframes-media/Cargo.toml:25-46`); current Windows CI supplies `FFMPEG_DIR`, runtime DLL `PATH`, and `LIBCLANG_PATH` before compiling (`.github/workflows/main.yml:338-348`).

**Failing scenario:** a clean account correctly has no `FFMPEG_DIR` or app cache. If the first doctor enforces the stated “exact libraries resolved” condition, setup stops at prerequisites-needed and can never reach the download that would satisfy it. If it silently ignores the failure, its pass status does not mean what the matrix says.

**Correction:** split setup into explicit `HostPreflight` and `CandidateSdkVerify` states. Host preflight checks only host/admin-owned requirements needed to launch and install. After extraction, candidate verification checks the app-owned Rust/Cargo, FFmpeg headers/libs/DLLs/cache, bindgen, link, and runtime load before pointer promotion. Mark every manifest probe with its owner and phase, and test a machine with no prior FFmpeg installation to prove it advances from host-ready to SDK-ready without requesting a redundant system FFmpeg install.

### 5. High — Discarding a stale response can deadlock the separate bounded frame pipe

**Plan location:** Phase 3, **Worker protocol and data flow**, lines 49–65.

**Source evidence:** the architecture mandates separate control and bounded binary transport and latest-wins rejection (`docs/desktop/architecture.md:309-319`). A rendered frame is an owned pixel `Vec<u8>` (`fframes/src/renderer/preview.rs:16-22`) returned synchronously after the full tree render (`fframes/src/renderer/preview.rs:434-443`). The plan says late results are discarded but does not state that stale binary payloads are fully drained or define cross-pipe correlation/ordering.

**Failing scenario:** request A begins writing a multi-megabyte frame, then request B supersedes it. The app sees A's stale request ID and stops reading A's payload. Once the OS pipe buffer fills, the worker blocks finishing A and cannot process or send B, so the “latest” frame never arrives. A timeout/restart while a partial payload remains can also misalign the next header if transport ownership is reused.

**Correction:** specify one dedicated binary reader that always reads and validates a complete header plus exact payload length before either presenting or discarding it. Correlate control completion and binary frames by generation/request ID in one explicit client state machine; the writer may coalesce before starting a send, but an admitted frame must be drained. Close and recreate both transports on worker generation change. Add a deterministic partial-write test with a payload larger than pipe capacity, supersede it mid-transfer, and prove B completes without unbounded buffering.

### 6. High — “Process groups” do not establish descendant cleanup on Windows

**Plan location:** Phase 1, **Files to create or modify**, line 34 and **Tasks and steps 2**, line 69; Phase 3, **Files to create or modify**, line 34 and **Real ACP experiment**, line 90.

**Source evidence:** the architecture gives the app supervisor ownership of agent subprocesses and their writable draft (`docs/desktop/architecture.md:46-57`) and requires worker isolation (`docs/desktop/architecture.md:59-70`). Windows is a required native target (`.github/workflows/main.yml:318-365`), but the plan uses one generic “PID/process group” abstraction without a Windows Job Object contract.

**Failing scenario:** an adapter or Cargo process on Windows spawns a compiler, Node helper, or shell descendant. Terminating the parent/process group does not guarantee that descendant exits, so cancellation can leave a writer or build running after the UI reports completion and can mutate the retained draft.

**Correction:** make lifecycle semantics platform-specific while keeping one app API: use a Windows Job Object configured to terminate members on close and assign children before normal execution; use an owned Unix session/process group on Linux/macOS. Define and test the policy for breakaway children and assignment failure. Apply the same supervisor to setup, Cargo, worker, and ACP processes, with an unrelated sentinel proving cleanup is scoped.

## Unresolved questions

- Which portable dependency route will own `fframes-studio-runtime` and `fframes-studio-protocol`: published crates or SDK-vendored source?
- Is project promotion intended after generation, after compilation, or only after the first successful rendered frame? The plan-wide “no partial project promoted” gate needs one exact boundary.
