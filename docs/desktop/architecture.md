# Product and technical architecture

This is a proposed architecture based on the [research findings](research.md). Unless explicitly marked implemented, types, paths and APIs below are design examples, not existing fframes APIs.

## 1. The product contract

Studio is a visual workspace for making videos with coding agents. The default experience is a brief, an agent conversation, a preview and a timeline. A user can work without reading Rust; the resulting project remains understandable and editable as Rust.

The app owns the authoring loop:

**Brief or selection → grounded task → agent source edit → compile → inspect → preview → accepted revision → export.**

Templates seed source, assets and editor metadata. They do not restrict the user to a fixed set of template parameters. A code/diff drawer is available for advanced users but does not dominate the workspace.

### Main workspace

```text
┌ Project name · agent/model · revision/build status ───────── Export ┐
│ Project / assets │                  Preview               │ Agent │
│ Scenes           │          canvas and selection          │ chat  │
│ Style presets    │                                       │       │
│                  │ Play / pause / frame / time / zoom      │ Scope │
├──────────────────┴───────────────────────────────────────┤prompt │
│ Timeline: scenes · audio · selected range · diagnostics   │       │
└───────────────────────────────────────────────────────────┴───────┘
```

The scope above the composer reads, for example, **Title · Intro · 4.2s**, **Scene: Outro**, or **Whole video**. A selected scope can be removed or expanded before sending. During a build, the preview identifies the last successful revision; a new candidate appears only once it is ready.

### Core workflows

| Action        | User-facing behavior                                 | Engine responsibility                                                      |
| ------------- | ---------------------------------------------------- | -------------------------------------------------------------------------- |
| Create        | Choose format/preset, add assets, describe the video | Scaffold Rust project, resolve style, submit agent task                    |
| Prompt edit   | Select project/scene/range and describe a change     | Freeze context, stage edits, validate and create revision                  |
| Canvas edit   | Click a supported element, then prompt               | Resolve frame-specific ID/source ownership and attach evidence             |
| Preset change | Choose a preset or edit tokens                       | Update bound values and ask the agent to adapt unbound design where needed |
| Compare/undo  | View earlier video or undo the last task             | Restore an app-owned revision without discarding unrelated user changes    |
| Handoff       | Continue with a different coding agent               | Stop the previous writer and transfer grounded project/task context        |
| Export        | Choose file/quality and save                         | Render an immutable accepted revision with progress/cancel                 |

## 2. Execution boundaries

```mermaid
flowchart TB
  subgraph App["Studio app"]
    UI["GPUI UI: main thread"]
    Engine["Project engine: async services"]
    AgentHost["Agent host and process supervisor"]
    Build["Build service and SDK manager"]
    UI <-->|typed commands/events| Engine
    Engine <--> AgentHost
    Engine <--> Build
  end
  AgentHost <-->|ACP stdio or native driver| Agent["Coding agent subprocess"]
  Agent --> Draft["Writable draft source"]
  Agent <-->|MCP or CLI facade| Tools["Project tools"]
  Tools <--> Engine
  Build --> Artifact["Immutable project executable"]
  Engine <-->|versioned IPC control| Worker["Renderer worker"]
  Artifact --> Worker
  Worker -->|bounded frames / timeline / audio| Engine
  Engine --> Store["Project files, revisions, journals"]
```

GPUI owns the only desktop UI event loop. The app's engine runs async work outside that thread and exposes typed commands/events. Blocking compilation, decoding, rendering and file indexing run in appropriate subprocesses or worker threads. ACP SDK executor requirements must be checked during the integration spike; a driver can own a dedicated executor without making the UI wait.

The project renderer is a compiled executable containing the user's concrete Video implementation, fframes and a small Studio runtime bridge. Keeping it outside the GPUI process solves the generic Rust type boundary and allows process recovery after a project panic/crash. It also avoids linking the UI's GPU stack to every project's renderer implementation.

Process separation is crash isolation, not a security sandbox. Agent subprocesses, Cargo build scripts, procedural macros and user video constructors execute code. The app must disclose the actual provider/build access policy and use OS confinement only where it has implemented and tested it.

### Workspace layout

```text
desktop/                          separate Cargo workspace and lockfile
  Cargo.toml
  rust-toolchain.toml
  crates/
    studio-protocol/              commands, events, IPC schemas
    studio-project/               metadata, source index, revisions
    studio-agent/                 ACP client, drivers, lifecycle
    studio-presets/               tokens, preset resolution/import
    studio-engine/                task, build, preview orchestration
    studio-media/                 presentation conversion and audio
    studio-ui/                    GPUI components and interaction
  app/                            fframes-studio executable
  packaging/                      icons, installers, runtime manifests

fframes-studio-runtime/            GPUI-free library in core workspace
                                  generic project worker entry point
```

The renderer bridge belongs with fframes because generated projects depend on it. The GPUI workspace uses the core libraries through their normal workspace/path dependencies where required, and pins one coherent GPUI revision. The root workspace must explicitly support the nested workspace arrangement; no GPUI dependencies should leak into existing core/CLI builds.

## 3. Project format and authority

```text
my-video/
  Cargo.toml
  Cargo.lock
  src/
    lib.rs                        Video and scene registration
    main.rs                       normal fframes CLI
    scenes/intro.rs                agent-editable scene implementation
    bin/studio_worker.rs           generated runtime bridge entry
  media/                          copied imports and embedded fonts
  style/
    tokens.json                   resolved preset values
    overrides.json                user-owned overrides
    guide.md                      design and motion instructions
    references/                   optional visual examples
  studio.json                     portable app metadata and SDK pin
  AGENTS.md                       generated project guidance
  .fframes/
    context/                      task packets, ignored
    cache/                        derived indices/thumbnails, ignored
```

App data outside the project stores managed SDKs, adapter installs, active draft workspaces, session journals and revision objects. Authentication credentials belong to provider-managed storage or OS credential facilities, not portable project files.

| Data                                                | Authority                                                           |
| --------------------------------------------------- | ------------------------------------------------------------------- |
| Scene layout, animation, timing and audio placement | Rust source plus any explicit runtime configuration it reads        |
| Active preset and local overrides                   | Style files, versioned with the project                             |
| Scene/object editor identities and source anchors   | Explicit source registration plus a revision-specific derived index |
| Timeline durations, overlaps and track positions    | Compiled renderer output                                            |
| Session transcript and build state                  | Local app journal/database                                          |
| Thumbnails, bounds and search index                 | Disposable revision-keyed caches                                    |

studio.json records project/schema ID, SDK release, entry target, preset identity/hash and display metadata. It must not invent an independent editable copy of scene durations that disagrees with Rust. An old project without editor annotations can still open with project/scene/range prompts; it should report that element selection is unavailable.

### Implemented portable foundation contracts

The GPUI-free [studio-project models](../../desktop/crates/studio-project/src/lib.rs) own portable metadata, validated relative paths and source identity. [Manifest](../../desktop/crates/studio-project/src/manifest.rs) defines `studio.json` version 1, including stable project identity, display metadata, an exact SDK compatibility digest, explicit Cargo manifest/package/worker target, asset references and instruction version. Optional canvas hints are informational; Rust remains authoritative. Parsing is read-only and checks the version before constructing typed state. Unknown versions never become writable manifests; no migrations are registered yet. Future migrations must preserve the original first.

[Source inventory](../../desktop/crates/studio-project/src/revision.rs) hashes sorted UTF-8 relative names, explicit file kinds and streamed content digests. It includes unknown durable files as well as Rust, Cargo, media, guidance and style files. Only root `.git`, root `target`, `.fframes/context` and `.fframes/cache` are excluded. Nested folders named `frames` or `target` remain source. Declared assets and Cargo entries cannot live in excluded trees. Links, special files and nonportable names return corrective errors. Unix file opens use held directory descriptors and no-follow opens for every descendant component. Windows rejects reparse points and checks components before and after opening; interactive platform qualification remains open.

Inventory version 1 also protects executable status, measured from the same opened file as its bytes. Readers verify both historical unversioned hashes without rewriting immutable manifests. Byte-only legacy hashes cannot authorize executable restoration or prove an unchanged relocated source with executable files; their recorded bytes remain exportable.

[ProjectState](../../desktop/crates/studio-engine/src/state.rs) belongs to one open-project controller. A fresh `OpenSession` identifies each open, including reopen. Current source, accepted checkpoint, immutable proposed candidate and successful build are separate identities. A checkpoint records saved bytes, not compilation success. Jobs move from queued to running and then succeeded, failed or interrupted; cancellation requests reject completion installation until interruption is recorded.

Every completion carries project, session, base source, operation and generation. The filesystem owner must supply a freshly reconciled inventory immediately before installation and serialize source mutation with installation. A scan alone is not an atomic filesystem snapshot. Source edits advance generation, clear candidate/build identities, retain the accepted checkpoint and interrupt obsolete work. Even an edit followed by restoring identical bytes invalidates earlier operations once reconciled.

[Controller](../../desktop/crates/studio-engine/src/controller.rs) implements checkpoint/draft jobs, per-project ownership locks, source reconciliation and interrupted recovery. A synced append-only intent/commit journal is the lifecycle authority; SQLite stores its transactional projection, recents and SDK selection. Immutable manifests reference streamed, verified content-addressed objects outside source. Truncated journal tails are preserved separately; interior corruption stops recovery. Recovery preserves external edits and accepted/draft identities. Explicit restore exports an independent copy instead of replacing the current checkout. Agent validation/promotion/Undo remain later work.

Failed scans and identity mismatches invalidate derived state and interrupt old operations before history writes, so repairing identical source cannot resurrect a stale result. Recovery inventories update source only after project identity matches and never count as successful reconciliation. Startup/capture failures settle queued or running work without changing accepted. A committed record reaches memory before SQLite projection; write failures report recovery requirements. The shell exports the selected immutable checkpoint independently of current-source validation. Root and scoped process managers serialize spawn/publication, cleanup and terminal shutdown through a shared lifecycle lock; scoped cleanup preserves unrelated children and root ownership of cleanup survivors.

[Lifecycle](../../desktop/crates/studio-project/src/lifecycle.rs) creates a portable ordinary Rust crate or imports by publishing a sidecar without rewriting source/Git/instructions. [Build materialization](../../desktop/crates/studio-engine/src/build_materialization.rs) binds captured Cargo files to the exact managed SDK in a separate tree, with isolated config/lock/target and workspace-root build cwd. Unsupported external dependencies/configuration produce diagnostics. The [native shell](../../desktop/app/src/studio_shell.rs) owns a serialized background backend and bounded immutable presentation; filesystem notifications are hints, with revision checks before completion. Terminal process-owner shutdown rejects late background spawns. Project/asset/recovery/SDK controls are functional; preview, timeline, agent editing and preset regions remain honest empty states.

For desktop-created templates, prefer runtime media loading for large/changing files and runtime style loading in the video constructor. Existing include_media_dir! projects remain supported, with an explicit rebuild after embedded assets change.

Project import preserves pre-existing dirty files and Git history. Desktop-managed checkpoints should use app-owned snapshots or a private history store; Undo must not use a blanket git reset --hard on a user's checkout.

## 4. Agent integration and handoff

### App-owned interface

Each driver exposes discovery, authentication status, session creation/restoration, prompt submission, cancellation, supported options and an event stream. Normalize events such as:

```text
SessionReady
MessageDelta
ToolStarted / ToolUpdated / ToolFinished
PermissionRequested / InputRequested
ConfigUpdated
PromptFinished { reason }
AgentActivityChanged
ProcessExited / ProtocolError
```

Keep provider ID, session ID, task ID and sequence numbers on events. Retain raw diagnostic detail locally with credentials redacted. Transcript rendering should be virtualized and batched so token streaming cannot stall video playback.

ACP v1 is the initial generic driver. Prefer the official Rust SDK for wire types/framing and own the subprocess supervisor separately. Native Claude/Codex/Pi drivers are optional implementations behind the same interface, activated when provider qualification shows a concrete missing behavior.

### Capability-driven behavior

Display model/config options advertised by the agent. Use image content only when supported, embedded resources only when supported, and session restoration only when supported. A text-only agent receives source/context files and artifact paths, with a visible limitation for visual review.

The app may initially decline optional ACP filesystem/terminal client capabilities and let the provider use its own workspace tools, as Zeron does. Add host-mediated operations only when they provide a tested benefit; implement path checks, output limits, cancellation and process ownership before advertising them.

### Session and writer ownership

Allow one active agent writer per project. Use a stable, app-owned draft workspace path for a persistent provider session, so session restoration does not silently change its working directory. Before a new task, reconcile that workspace to the accepted revision and deliver a context refresh. If the provider cannot safely refresh/reset context, start a new session.

An agent switch is a controlled handoff:

1. Cancel the old task or wait for its explicit completion; reap owned processes and close pending requests.
2. Choose whether the new agent continues the captured draft or starts from the accepted revision. Preserve both versions until the user chooses.
3. Create a session for the new provider with the same project instructions, selection, style, current source, previous outcome and open problems.
4. Record that conversation state/model-specific memory has not transferred. Store the outgoing session for later access.

Queued follow-up prompts wait for the current writer. Mid-turn steering is an optional provider capability, not the baseline for editing transactions.

## 5. The edit transaction

Agent edits are draft changes until validated. The default requested edit can apply automatically after validation, with Undo and comparison available; an optional review setting can require Apply. Permission to edit a project should not create a separate confirmation for every ordinary task.

```mermaid
stateDiagram-v2
  [*] --> ContextReady
  ContextReady --> Editing
  Editing --> WaitingForUser
  WaitingForUser --> Editing
  Editing --> Building: authoritative agent completion
  Building --> Inspecting: executable ready
  Inspecting --> CandidateReady: required checks pass
  CandidateReady --> Accepted: apply policy permits
  Accepted --> [*]
  Building --> RepairNeeded: compile failure
  Inspecting --> RepairNeeded: validation failure
  RepairNeeded --> Editing: bounded repair attempt
  RepairNeeded --> Failed: budget or retry limit
  Editing --> Cancelled
  Building --> Cancelled
  Inspecting --> Cancelled
  CandidateReady --> Conflict: accepted base changed
```

The task freezes a base revision, assets/style revision and user selection. After the agent stops writing, take an immutable candidate snapshot before building it. Do not compile/render changing source while a task is still in progress unless explicitly labeling it as an unvalidated live draft.

Validate compilation, scene/timeline availability, critical diagnostics and representative frames. Inspect the selected range and neighboring boundaries; when shared functions change, broaden coverage to affected scenes. Audio changes require appropriate loudness/placement checks. The app can send bounded repair tasks with structured compiler/inspection output; expose the retry count and allow Stop.

Promotion checks that the accepted base still matches the task's base. Apply a journaled file set with content-hash conflict checks; if unrelated edits occurred, perform a safe merge or show a conflict. Multi-file promotion is a recoverable transaction, not a claim that ordinary filesystem writes are globally atomic.

Associate the revision with its prompt, changed files, preset hash, SDK version, build artifact and validation report. Export always names a specific accepted revision; background prompts do not change an active export.

## 6. Selection and source retrieval

### Start with scopes that already have evidence

The first version supports whole-project, scene and absolute time-range selection. Convert UI seconds into the current video's integer frame domain and use half-open ranges [start, end). Show overlapping active scenes and let the user choose one or both rather than assuming a single scene at a crossfade.

A timeline selection is an editing scope, not a guarantee that only those frames can change. Editing a shared animation/helper or retiming a scene can affect later frames. Context retrieval and validation must account for those dependencies.

### Stable identities for element selection

Introduce an opt-in editor annotation/registration API for generated projects. Its exact syntax needs a macro/runtime spike, but its data contract should carry:

- Stable scene-instance ID separate from a Rust type name or timeline index.
- Stable element/component ID plus an instance key for repeated/generated elements.
- Source anchor: workspace-relative file, symbol/component and revision-validated span.
- Frame-specific transformed bounds, visibility and paint order.
- Optional text, media reference and style-token bindings.

A lexical static_hash is a rendering-cache identity. It changes with markup, can be shared by identical subtrees and does not distinguish dynamic instances; it must not be the editor object ID.

For initial templates, explicit semantic IDs and source registration can precede automatic macro span capture. Later extend svgr! output with optional sidecar source metadata. Determine which Rust span/location APIs and renderer conversion stages preserve useful anchors; do not assume source spans are already available from the final SVG.

### Hit testing

Map the pointer through preview letterboxing/zoom into video coordinates. Query metadata from the exact displayed frame and revision. Start with transformed bounds and topmost selection, plus a list to cycle through overlapping groups. Treat text outlines, clips, masks, opacity and shader content conservatively.

Bounding-box selection is approximate for non-rectangular objects. A shader/video layer is selectable as a layer; an arbitrary pixel inside it is not necessarily a separately editable object. Fall back to a rectangle/time selection with a screenshot when no semantic object is available.

### Frozen selection packet

```json
{
  "schema_version": 1,
  "project_id": "project-123",
  "base_revision": "rev-018",
  "render_generation": 24,
  "scope": "element",
  "frame": 126,
  "fps": 30,
  "scene_instance_id": "intro-01",
  "element_id": "intro.title",
  "instance_key": "main",
  "range_frames": [90, 180],
  "bbox_video_px": [160, 220, 1200, 180],
  "source_refs": [
    {
      "path": "src/scenes/intro.rs",
      "symbol": "Intro::render_frame",
      "source_hash": "sha256:example"
    }
  ],
  "style_hash": "sha256:example",
  "frame_artifact": "context/task-031/frame-126.png",
  "request": "Make this title smaller and slide it in from the left."
}
```

Frame and pixel evidence are invalidated when source/style/assets change. If metadata belongs to another generation, regenerate it or ask the user to reselect; never pass stale coordinates as an exact object identity.

### Retrieval strategy

Begin with deterministic retrieval: IDs → source anchors → containing Rust implementation → imported helpers/tokens/assets. Use a Rust syntax index and symbol/text search to gather a small relevant context set. Add vector/semantic retrieval only if actual task data shows it improves results.

For arbitrary legacy projects, type/file/text matching is a best-effort fallback. Surface uncertainty rather than pretending it is an exact source map. The agent receives retrieval results as pointers and snippets and can request full files or additional dependency context.

## 7. Style presets and design systems

A preset is a portable, versioned directory or archive:

```text
preset.json                      name, version, supported token schema
tokens.json                      typed values and semantic aliases
guide.md                         typography/layout/motion rules
fonts/                           licensed files and family metadata
assets/                          reusable images/SVGs
examples/                        reference frames and optional Rust snippets
```

Support colors, typography, spacing, radii, stroke widths, shadows where the renderer supports them, and motion tokens such as durations, stagger and easing/spring parameters. Resolve aliases and validate cycles/types/units. Store a snapshot in each project so later preset updates do not silently alter existing videos.

CSS custom properties are an **import format**. Parse a declared subset of values from a supplied CSS token block and convert it to canonical typed JSON. This does not add a browser CSS layout/cascade engine to GPUI or fframes. Report unsupported selectors, functions, units and component rules instead of inventing an approximate interpretation. Define px/rem conversion relative to declared design dimensions/base typography; keep UI display pixels separate from video coordinates.

Use precedence: preset defaults → project overrides → explicit scene/element overrides. Reapplying a preset preserves user overrides unless the user requests a reset. Store instruction text, font availability, contrast rules and examples alongside tokens; numbers alone do not describe a design system.

Prefer a typed runtime Styles accessor loaded once in the video constructor. Agents use semantic names such as color.accent and typography.title rather than copying literal values throughout scenes. A token change can refresh bound values without source rewriting; hardcoded or structural design changes still require an agent task.

Separate application chrome appearance from the video's preset. A pink brand video should not automatically make every Studio panel pink.

### Guiding different agents

Keep one canonical, versioned fframes instruction bundle and reference it in every task packet. Materialize provider-specific skill entry points only as needed and preserve user-written instruction files. ACP does not provide a universal skill-installation/discovery mechanism.

The existing skill is a starting point, but desktop instructions should route preview/build/inspection through Studio tools and explain IDs, style bindings and draft transactions. Avoid asking the agent to start a second native preview window, install packages globally, or accept snapshot baselines without review.

## 8. Preview, timeline and audio

### Bootstrap and persistent preview

For the first integration slice, Studio can build a project once and invoke its compiled CLI for timeline JSON, PNG frames, inspection and a cached draft MP4. This proves the authoring loop. It does not establish smooth interactive frame generation.

The shipping interactive path uses a persistent worker with the concrete Video, media provider, Previewer, decoder state and renderer caches kept alive. The worker exposes versioned operations such as hello, timeline, render_frame, element_metadata, inspect, prepare_audio and export. Standard output carries protocol messages; logs go to stderr. Bulk image/audio data travels through a separate bounded binary transport.

Each message includes protocol version, project/source revision, worker generation and request ID. A paused seek is latest-wins. Reject late results from superseded requests or old renderer generations. On a successful rebuild, start a new worker, prepare its first frame, then switch atomically at the UI model level while preserving/clamping playhead and selection.

### Frame contract and memory

Define dimensions, byte stride, channel order, alpha mode, color space and frame number in the frame header. fframes' RgbaFrame is straight RGBA; other paths may produce premultiplied data. Convert into the representation expected by the pinned GPUI API once at the presentation boundary and verify with known colors/transparency.

A bounded binary pipe is acceptable for the first static-frame spike. For sustained playback, evaluate a small shared-memory ring with explicit ownership/release or another measured binary transport. Do not send full-resolution base64 images inside ACP or control JSON.

At 1280×720, one RGBA frame is about 3.5 MiB; 30 frames per second moves about 105 MiB/s before extra copies. Use preview resolution, bounded queues, latest-wins coalescing and explicit GPUI image eviction. Zero-copy texture interop between Skia and GPUI is a later platform-specific optimization.

### Timeline authority and edits

Render the scene track and audio placements from the compiled TimelineReport. Cache thumbnails/waveforms by revision and media hash. Timeline geometry and selection logic belong in a UI-independent model, following Cap's separation.

Initially, a scene resize/reorder/trim gesture creates an agent request and ghosted pending change. The timeline becomes authoritative only after compilation produces the new report. Where future projects expose typed runtime parameters for duration/order, direct controls can update those parameters through the same revision transaction. Avoid two independent timelines with different timing rules.

### Audio and presentation clock

First support an audio mix prepared for an immutable preview revision; an app-owned audio service handles play/pause/seek and uses the audio clock to schedule/drop video frames. The exact mix/audio API must be proven in the worker spike. Later add streaming audio preparation for long projects.

Keep the audio revision coupled to the visible renderer revision. Resuming playback after a rebuild must use a matching mix and seek position. Test mute, output-device changes, end-of-video and rebuild while playing.

Use the same selected rendering backend and fonts/media for preview and export. A tiny-skia fallback cannot reproduce Skia-only shaders; report that capability gap or use a validated Skia CPU path. GPUI acceleration and fframes renderer acceleration are separate capabilities.

## 9. Agent-facing fframes tools

Expose a narrow task-scoped MCP server and an equivalent local CLI facade:

| Tool                        | Purpose                                                   |
| --------------------------- | --------------------------------------------------------- |
| project_context             | Project revision, target format, files and active preset  |
| selection_context           | Frozen selection packet and source anchors                |
| source_lookup               | Resolve IDs/symbols to relevant source and dependencies   |
| timeline                    | Compiled scene/audio structure                            |
| render_frame / render_strip | Return image artifacts for a requested revision/range     |
| inspect                     | Structured diagnostics tied to frames/scenes              |
| audio_analyze / audio_at    | Mix quality and placement evidence                        |
| style_context               | Resolved tokens, overrides, guidance and available assets |
| build_status                | Current candidate artifact and compiler diagnostics       |

The agent retains its own coding tools. fframes tools should neither accept arbitrary shell commands nor provide a second unrestricted filesystem interface. Bind them to the session's project/draft/revision and route expensive operations through the build/preview coordinator so duplicate requests share work.

Return image artifacts in a supported form and with size limits; include text diagnostics for providers without visual input. Project build dependencies and toolchains are managed by the app's build service, not repeatedly installed by each agent.

## 10. Installation, builds and updates

An ordinary Rust project requires a compiler, compatible crate dependencies, a linker and relevant native libraries/OS SDK pieces. Packaging the GPUI app does not remove this requirement.

Use a versioned, app-managed SDK consisting of a pinned Rust toolchain, tested fframes/runtime release, offline/vendorable Rust dependencies where practical, prebuilt Skia/FFmpeg libraries, a known codec set and a platform build configuration. Treat reuse of precompiled Rust artifacts as a cache optimization: it depends on exact compiler/features/target compatibility and is not a stable plugin format.

Phase zero must establish how macOS SDK/linker and Windows SDK/runtime needs can be satisfied and redistributed. If a fully managed native build is unavailable for a platform, show the prerequisite or offer an explicit remote-build alternative; do not advertise a terminal-free local path that has not been demonstrated. WASM compilation is an alternative research direction, but today's browser bridge and native shader/media differences mean it is not a drop-in equivalent.

Keep the installer relatively small; download the tested SDK once during guided setup, with progress, disk-size information, resumable downloads and an offline bundle option. Users should never have to run just init-repo or install Node solely for the UI. Agent adapters may need Node or another runtime; manage that separately and reuse an already installed provider when supported.

Ship verified packages for each qualified platform, with runtime libraries, fonts, icons/file associations, checksums and signed updates. Pin app/SDK/worker protocol compatibility and retain one usable previous SDK for rollback. Imported custom dependencies may require extra builds; record this as an advanced compatibility path.

## 11. Persistence and recovery

Use an app-local database for project/session metadata and an append-only task journal for lifecycle/recovery. Large media, screenshots and build artifacts stay in files with retention limits. Keep portable project settings in the project rather than only in the database.

After a crash, recover to the last accepted revision, mark unfinished agent/build/export tasks interrupted, clean up owned workers and offer the retained draft. Never treat process exit as proof that its edits compiled or were applied. Closing a window cancels/reaps jobs by default; a later background-job mode should be explicit.

Record latency/memory/build timings locally to tune the pipeline. Remote telemetry and cloud sync are separate opt-in product decisions.
