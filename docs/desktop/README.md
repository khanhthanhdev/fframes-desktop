# fframes Studio: research and implementation plan

Research date: 2026-10-01. Working product name: **fframes Studio**.

Build a native GPUI app in Rust where a person describes a video, watches it, selects a scene or visible object, and asks a coding agent to change it. The agent edits an ordinary fframes Rust project. The app owns project setup, style presets, agent connections, compilation, preview, validation, history and export.

This direction follows the requested agent-driven authoring workflow. It supersedes the earlier proposal to make template parameters the primary project model. Templates are useful starting points; Rust source remains the authority for the video.

Read the documents in this order:

1. [Research findings](research.md): what Zeron, Cap, ACP and the current fframes implementation establish, with source references and limitations.
2. [Product and technical architecture](architecture.md): user workflow, process boundaries, project format, selection-to-code retrieval, style presets, preview and recovery.
3. [Implementation plan](implementation-plan.md): ordered milestones, concrete work items, acceptance criteria, release matrix and unresolved decisions.

## Run the implemented foundation

The [M1 foundation](../../plans/261002-0434-desktop-phase-one-foundation/plan.md) provides a native project workspace, create/open/import, copied assets, SDK setup/status, recent projects, saved checkpoints and interrupted-job recovery. Preview/playback, compiled timeline, agent editing and presets are not connected in the product shell yet; their regions show explicit empty states.

```sh
cargo run --locked --manifest-path desktop/Cargo.toml -p fframes-studio
# Existing Phase 0 development/qualification view:
cargo run --locked --manifest-path desktop/Cargo.toml -p fframes-studio -- spike-ui
```

Create chooses a new folder. Import Rust selects the package's `Cargo.toml` (not a virtual workspace root), preserves existing files/instructions/Git and adds only `studio.json`. Missing worker bridges remain navigable. Managed builds support contained workspace dependencies; external paths, symlinks, inherited Cargo configuration and Cargo overrides need explicit compatibility repair rather than hidden rewrites. Exact portable dependency versions must be available for standalone Cargo; Studio's isolated SDK-bound build does not require publishing development versions.

Source and copied assets travel with the folder; local history does not. App data is `${XDG_DATA_HOME:-~/.local/share}/fframes-studio` on Linux, `~/Library/Application Support/fframes-studio` on macOS and `%LOCALAPPDATA%/fframes-studio` on Windows. It contains SQLite metadata, durable per-project journals, immutable checkpoint objects, retained drafts and build copies. A checkpoint saves bytes, not proof of a successful build. Recovery never replaces current source. **Restore as copy** exports a separately identified folder; Remove recent removes only list metadata. Locate reconnects a moved, unchanged project; duplicate IDs or changed relocated content require an explicit independent-copy choice.

If source becomes invalid while open, Studio interrupts obsolete work and preserves the saved checkpoint and draft. **Restore as copy** exports the checkpoint selected when its picker opened; it does not require a successful current-source scan. Repair source before checkpointing or running other source-sensitive operations. Reopening still requires readable, compatible `studio.json` metadata to identify the project safely.

Checkpoint inventories now record an explicit format version. Both earlier unversioned formats remain readable without rewriting history. The oldest format never recorded executable permissions: its exports restore file bytes without inventing executable bits. Relinking to that format requires non-executable current files; otherwise use an independent copy and explicitly repair script permissions.

SDK setup reuses the [Phase 0 local bundle procedure](phase-zero-feasibility.md); `FFRAMES_SDK_BUNDLE` selects an assembled bundle. Opening a project never installs an SDK or runs Cargo. Filesystem and setup work run in the background. Tab/Shift-Tab navigate commands; Enter/Space activate them.

Linux X11 software-rendered native flows and real SDK worker regressions were exercised. Physical GPU/display/IME, Windows/macOS interactive behavior and authenticated configurable ACP adapters remain unqualified. This is local development evidence, not release certification.

## Proposed experience

The user installs Studio, connects a coding agent, chooses a style preset, and writes “Make a 30-second announcement for this product.” Studio creates a normal Rust video project and passes the brief, assets, selected style and fframes guidance to the agent. Once the result compiles and passes basic inspection, the preview and timeline update.

The user selects the title at 4.2 seconds and writes “Make this smaller and animate it from the left.” Studio captures the selected scene/object, frame, screenshot, source references and relevant style tokens. The agent retrieves the relevant Rust implementation, edits it in a draft workspace, and Studio validates the candidate. The result becomes a new revision with a one-click Undo. Export renders the accepted revision through the same backend used for preview.

The UI should show **what is changing** and **which version is on screen**. “Agent finished” and “video ready” are separate states: the app must still build and render the result.

## Architecture at a glance

```mermaid
flowchart LR
  User[User: prompt or selection] --> UI[GPUI workspace]
  UI <--> Engine[Rust project engine]
  Engine <--> Agents[Agent host: ACP and provider drivers]
  Agents <--> Coding[Claude / Codex / Pi / Antigravity]
  Coding --> Draft[Draft Rust project]
  Engine --> Context[Selection, source index, style, skills]
  Context --> Agents
  Coding <--> Tools[fframes tools: MCP or local CLI]
  Tools <--> Engine
  Draft --> Build[Managed build service]
  Build --> Runner[Project renderer process]
  Runner --> Frames[Frames, timeline, diagnostics, audio]
  Frames --> UI
  Engine --> History[Accepted revisions and recovery]
```

## Decisions recommended now

| Area                  | Recommendation                                                                     | Reason                                                                                    |
| --------------------- | ---------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| Authoring model       | Ordinary Rust fframes projects                                                     | Preserves existing capabilities and lets agents edit real source                          |
| Native UI             | GPUI plus its platform entry point, in a separate desktop workspace                | Fits the requested Rust UI while keeping the core dependency graph manageable             |
| Agent integration     | ACP v1 baseline behind an app-owned agent interface                                | Supports interoperability and leaves room for native provider drivers                     |
| Rendering             | Compile a project-specific renderer executable and run it outside the GPUI process | The app cannot load arbitrary new Rust video implementations through today's generic APIs |
| First selection scope | Project, scene and time range                                                      | Existing timeline reports can support this before element source mapping exists           |
| Element selection     | Stable IDs, frame-specific metadata and source anchors                             | Makes “change this” reproducible; screenshots alone are ambiguous                         |
| Presets               | Versioned tokens, design guidance, fonts, assets and examples                      | Combines consistent values with instructions agents can follow                            |
| Preview               | Reuse a persistent renderer; preserve the last successful revision during builds   | Keeps incomplete agent edits from interrupting playback                                   |
| Installation          | Signed app plus a managed build SDK and explicit agent connection                  | Hides setup commands while acknowledging compilation and authentication requirements      |

## Work that determines feasibility

Three short experiments should precede a broad editor implementation:

1. Compile agent-written fframes source on a clean machine using an app-managed SDK. Establish what compiler, linker, native libraries and OS SDK components are actually required on each target.
2. Display frames from a persistent project renderer in GPUI, with scrubbing, bounded memory and a clear pixel/alpha format contract.
3. Complete one real agent edit over ACP: stream updates, handle permission/input, detect completion, cancel, compile the changed project, and show the result.

The source-selection experiment follows immediately: assign stable IDs to a small scene and prove that selecting an object retrieves the exact source region that produced it.

The broader authoring experience remains a roadmap. See [Phase 0 evidence](phase-zero-feasibility.md) and the M1 execution plan for implemented paths and the limits of their verification.
