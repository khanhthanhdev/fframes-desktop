# Phase 0 planning decisions and verification

Date: 2026-10-01. Scope: planning only; no desktop application was implemented, no dependency was installed, and no native build or provider session was run.

## Outcome and scope

The [execution plan](../261001-1707-desktop-phase-zero-setup/plan.md) expands M0 from [the desktop roadmap](../../docs/desktop/implementation-plan.md). Its three stages belong to Phase 0: a native GPUI spike, managed SDK and project setup, and qualification of the five feasibility boundaries. Scope is held to the requested work. Full editor implementation and production signing/update flows keep their existing M1–M7 ownership.

## Validation session 1

Trigger: resolve material choices before completing the Phase 0 plan. Three questions were asked, and all three received explicit answers.

1. **Scope:** Should Phase 0 cover all five M0 feasibility checks in the roadmap, or only desktop build and dependency setup?
   - Options: “Cover all five checks, with detailed build and setup work (Recommended)” | “Focus only on GPUI, project compilation, and dependency setup”.
   - Answer: “Cover all five checks, with detailed build and setup work (Recommended)”.
   - Effect: managed compilation, GPUI input/image, renderer worker, real ACP edit, and selection/source anchor all have tasks and acceptance gates.
2. **Installation policy:** How should the app handle prerequisites that require administrator access, such as Windows Build Tools or macOS developer tools?
   - Options: “Install app-owned dependencies automatically and guide OS prerequisite installation (Recommended)” | “Require developers to install every prerequisite manually”.
   - Answer: “Install app-owned dependencies automatically and guide OS prerequisite installation (Recommended)”.
   - Effect: the app manages its SDK; doctor explains host prerequisites and offers reviewed OS installer/package actions.
3. **First platform:** Which platform should be the first interactive GPUI development target? Builds and dependency setup will still be planned for all three OSes.
   - Options: “Linux x64, matching this workspace (Recommended)” | “macOS Apple Silicon” | “Windows x64”.
   - Answer: “Linux x64, matching this workspace (Recommended)”.
   - Effect: Linux x64 is the first interactive gate; Windows x64 and macOS arm64 retain native build/setup qualification. Ubuntu 24.04 is the proposed initial Linux distribution baseline, subject to measured qualification.

All decisions are propagated into the plan index and phase files. No unanswered product choice blocks delivery of this plan. Real provider credentials and native test machines are execution prerequisites; they are not results of this planning session.

## Research and verification

Primary-source research is recorded in the [GPUI report](researcher-261001-1707-gpui-phase-zero.md) and [SDK/FFmpeg report](researcher-261001-1707-sdk-ffmpeg-phase-zero.md). Candidate versions are labeled unqualified until native tests pass.

Verified repository contracts include:

- Core workspace membership and FFmpeg binding version: `Cargo.toml:1`, `Cargo.toml:104`, `Cargo.lock:1092`.
- Native FFmpeg target-dependent static/shared feature behavior: `fframes-media/Cargo.toml:25`, `fframes-media/Cargo.toml:45`.
- Windows FFmpeg headers/libraries/DLL and LLVM setup: `README.md:263`, `.github/workflows/main.yml:329`, `.github/workflows/main.yml:338`.
- Existing generator standalone workspace and release pin: `cargo-fframes/src/main.rs:452`; generated non-Windows codec guard: `cargo-fframes/src/main.rs:500`.
- Concrete `Video` type and persistent preview boundary: `fframes/src/video.rs:10`, `fframes/src/renderer/preview.rs:245`.
- Straight RGBA frame, compiled timeline, rendering and inspection: `fframes/src/renderer/preview.rs:16`, `fframes/src/renderer/preview.rs:315`, `fframes/src/renderer/preview.rs:435`, `fframes/src/renderer/preview.rs:480`.
- Native player owns a separate main-thread event loop: `fframes-native-player/src/lib.rs:26`, `fframes-native-player/src/lib.rs:62`.
- Original M0 gates and proposed shared build/runtime boundaries: `docs/desktop/implementation-plan.md:34`, `docs/desktop/architecture.md:43`, `docs/desktop/architecture.md:355`.

New packages, app modes, JSON schemas, bootstrap scripts, qualification commands, and runtime APIs in the phases are explicitly implementation tasks. They are not claimed to exist today. Shared worker protocol ownership is assigned to one GPUI-free core crate. Existing public fframes APIs remain unchanged; the worker uses additive entry points.

## Review and whole-plan sweep

Three independent reviews covered [assumptions/contracts](review-assumptions-261001-1707-phase-zero.md), [SDK/security](review-security-261001-1707-phase-zero.md), and [failure behavior](review-failures-261001-1707-phase-zero.md). The assumption review sampled ten claims per phase, thirty total, distinguishing current source/API facts from future qualification work. Fourteen distinct findings remained after deduplicating FFmpeg fallback and Windows process ownership: eleven High and three Medium. Thirteen were accepted or accepted with modification, and one was rejected on source evidence. No correction changes the user's three decisions.

| Finding | Disposition | Applied contract |
|---|---|---|
| FFmpeg local URL permits source fallback | Accepted | Phase 2 requires a verified fail-closed SDK route; record a minimal SDK-local wrapper patch if upstream lacks one, test corrupt/missing keys, and deny networking during both build qualification runs. |
| Bootstrap manifest/elevation trust origin | Accepted | Embed checked bytes/digest; installer actions use built-in executable/argument allowlists. |
| Archive links/special entries | Accepted with modification | Reject escapes/devices; necessary SDK links require explicit manifest declarations, contained targets and post-extraction verification. Blanket link rejection could break legitimate native SDK layouts. |
| Windows descendant lifetime | Accepted | Job Object assignment before execution, kill-on-close, no unqualified breakaway; owned Unix groups on Linux/macOS. |
| Provider secrets in evidence | Accepted with modification | Explicit auth lookup/injection, exclude secret values from snapshots, redact before persistence and test a sentinel in unexpected fields. No claim of detecting every unknown secret is made. |
| Partial generated projects | Accepted | Sibling staging, transaction marker, build/frame before atomic promotion, destination conflict protection and relocation/media retest. |
| Runtime/protocol offline distribution | Accepted | SDK-vendored standalone sources with manifest hashes and app-local Cargo patches; verify all four consumers and relocated offline worker project B. |
| Previewer borrowing | Accepted as implementation clarification | Generated binary owns stack locals; runtime serves borrowed previewer/renderer without self-referential ownership. |
| Doctor requires missing SDK before install | Accepted | Owner/stage-aware HostPreflight followed by CandidateSdkVerify. |
| Stale binary frame blocks newer seek | Accepted | Always drain admitted frames, correlate both pipes, recreate transports per generation and test partial writes beyond pipe capacity. |
| GPUI alpha/stride/lifetime | Accepted as stronger evidence | Inspect exact pinned upload path and run native transparent/semitransparent/padded-row readback. |
| Windows requires MSYS2 source route | Rejected | `fframes-media/Cargo.toml:45-46` enables source build only off Windows; generator `cargo-fframes/src/main.rs:500` omits codec features on Windows. The selected prebuilt FFMPEG_DIR route needs headers/import libraries/DLL qualification, not an implicit source route. Reviewer agreed after clarification. |
| Logical image count proves GPU release | Accepted | Require actual pinned image IDs/release/cache evidence and available process/graphics memory measurements. |
| CLI comparison independently proves transport | Accepted | Add known-byte protocol/presentation fixtures alongside real fframes output; CLI comparison remains a secondary semantic check. |

The controller separately corrected fourteen desktop verification commands to use explicit candidate `cargo +1.98.1`. A root-issued `--manifest-path` does not activate a nested toolchain file; [rustup's override rules](https://rust-lang.github.io/rustup/overrides.html) use the current directory. SDK children select `RUSTUP_TOOLCHAIN` or the direct managed compiler path.

GPUI SHA provenance was rechecked through the [official Zed history](https://github.com/zed-industries/zed/commits/main/): the 2026-09-10 `1a84d5d` link targets full SHA `1a84d5d92bd7d6c1cabb116062650af545783fe9`. It is a listed historical candidate. Direct immutable-file fetches failed in the planning web tool, so Phase 1 explicitly fetches the object and inspects its manifests before adoption. Rust `1.98.1` is a separate candidate observed in [the current upstream toolchain file](https://raw.githubusercontent.com/zed-industries/zed/main/rust-toolchain.toml); neither has been built here.

Whole-plan sweep: the index and all three phase files were reread after corrections. Single protocol ownership, host/setup ordering, SDK relocation, compiler selection, supported ABI, frame draining, project promotion, process ownership and all five acceptance gates agree. A targeted reviewer reread found no remaining contradiction in the six failure findings. Future maintainer Markdown lives under `docs/` or `plans/`. No source-backed blocker remains in the planning contract; native and authenticated qualification gates remain unchecked execution work.

Final verification passed: `ak plan validate --json` reported `valid: true`; a files-first check covered eleven Markdown files and 102 local links with zero broken targets; all four plan files have frontmatter, no stub placeholders and no completed execution items. Desktop commands select the explicit compiler candidate. Whitespace checks passed, and the index has 61 lines. Status is pending implementation; format validation does not imply application build success.

## Tooling state

`ak plan create` and `ak plan add-phase` scaffolded the canonical files. Creation warned that the user-level plan-store index at `/root/.agentkit/plans` was read-only in the sandbox; repository files were created successfully. The CLI's live help confirms titles and phase bodies are file-owned. The plan remains usable without that optional index.

No live task-management or installed journal capability is available in this session. Execution progress remains in the phase checkboxes. No absent skill was invoked, and no background process was started.
