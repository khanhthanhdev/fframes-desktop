# Research findings

Date: 2026-10-01. Repository inspection and official protocol/framework documentation; no reference applications or real agent sessions were launched.

## 1. What the references establish

| Reference              | Inspected snapshot                       | Most useful contribution                                                                                       |
| ---------------------- | ---------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| Zeron, /root/zeron     | 42926c802a837097e6a05de89d48ca7ade326658 | Native GPUI agent UI, engine/harness boundary, session lifecycle, provider installation and recovery           |
| Cap, /root/Cap         | 97c0a450a08bdadabece6fc526fba43ba8ae924d | Real native preview, timeline interactions, background rendering, bounded frame delivery and desktop packaging |
| fframes, this checkout | 6454a6dc3ef39922ae804238904317a8d94f00d3 | Rust video authoring, rendering, CLI diagnostics, scenes, audio and existing agent guidance                    |

These sources support the architectural patterns below. Their performance measurements and platform claims are specific to their own versions and environments; they are not measurements of a proposed fframes app.

## 2. Zeron: an agent host with a native viewport

Zeron's [overview](/root/zeron/README.md) describes a local agent controller with optional device sync. Its [architecture](/root/zeron/ARCHITECTURE.md) separates a Rust engine from a GPUI viewport, with a typed RPC boundary used for embedded and daemon operation. The engine owns sessions, subprocesses, repositories, transcripts and recovery. The UI renders state and dispatches actions; it does not block on agent or filesystem work.

For Studio, this is a stronger reference than embedding agent subprocess handling directly inside UI components. A project engine can serve the GPUI app today and a headless render/test interface later. Studio does not need Zeron's CRDT sync, cloud relay, mobile clients or persistent daemon for its first release.

### ACP is one transport, not the entirety of Zeron's current integration

The current [Harness implementation](/root/zeron/crates/harness/src/lib.rs) is more specific than the high-level overview:

| Requested agent | Current Zeron implementation      | Lesson for Studio                                                             |
| --------------- | --------------------------------- | ----------------------------------------------------------------------------- |
| Claude Code     | Native stream-json driver         | Keep a provider fallback available if an ACP adapter loses important behavior |
| Codex           | Native app-server JSON-RPC driver | Provider protocol can expose richer lifecycle control, but adds maintenance   |
| Pi              | Native JSONL RPC driver           | A prompt acknowledgement may differ from completed work                       |
| Antigravity     | ACP server through AcpHarness     | Native ACP can fit the generic transport directly                             |

Zeron previously adopted Claude/Codex ACP adapters. Its code now documents a return to native drivers after completion/streaming problems. The historical [ACP decision record](/root/zeron/docs/research/acp.md), [ACP driver](/root/zeron/crates/harness/src/acp/mod.rs), and [Pi contract](/root/zeron/crates/harness/src/pi/PROTOCOL.md) distinguish acknowledgement, turn completion, background activity and session readiness.

This is evidence from Zeron's pinned implementation, not proof that the current upstream adapters still have the same defects. Studio should qualify current versions through a compatibility suite before selecting defaults.

### Patterns to adopt

- An app-owned agent interface normalizes streaming text, tool status, questions, permissions, cancellation and completion.
- A process supervisor owns the subprocess tree, drains stderr, preserves structured errors and cleans up on cancellation or app exit.
- GUI launches compose executable discovery with user installation locations and login-shell PATH. A CLI that works in a terminal may otherwise appear missing in a desktop app.
- Session history and event journals persist independently of a live process. Recovery closes interrupted tasks explicitly instead of leaving “Working” forever.
- Provider/model/option catalogs come from discovery where possible, with a bounded cached fallback.
- Agent installation happens in a visible setup step. Zeron's managed adapters install into versioned app-owned directories; they do not depend on a fresh npm download during every prompt.

These patterns are demonstrated in [JSON-RPC transport](/root/zeron/crates/harness/src/jsonrpc.rs), [managed adapter installation](/root/zeron/crates/harness/src/adapter_install.rs), [run journal](/root/zeron/crates/engine/src/run_journal.rs), and [installation overview](/root/zeron/crates/harness/README.md).

### Policies to choose independently

Zeron's drivers document unattended auto-approval. Studio should expose the agent's actual access mode and honor user decisions; worktree isolation alone does not confine filesystem or network access. It should also avoid presenting native provider session state as portable across different agents. Switching providers transfers a project and a task summary; it normally starts a new provider session.

## 3. ACP: capabilities and current compatibility

ACP standardizes editor-to-agent communication. Local connections use JSON-RPC over subprocess stdio. For this design, Studio is the ACP **client** and the coding tool or adapter is the ACP **agent**. [Official introduction](https://agentclientprotocol.com/get-started/introduction).

Implement a stable ACP v1 baseline first. The official site currently publishes ACP v2 as a draft; it changes prompt lifecycle semantics and should be version-gated, rather than assumed to behave like v1. [ACP v2 draft announcement](https://agentclientprotocol.com/announcements/acp-v2-draft).

### Minimal connection lifecycle

Start the configured agent process, call initialize, inspect its returned capabilities/authentication methods, authenticate when needed, and create a session for a fixed project working directory. The app can also configure supported model/options and restore a session when the agent advertises the relevant capability. [Initialization](https://agentclientprotocol.com/protocol/v1/initialization), [session setup](https://agentclientprotocol.com/protocol/v1/session-setup), [configuration options](https://agentclientprotocol.com/protocol/v1/session-config-options).

A v1 prompt produces session/update notifications for messages and tools, then a response carrying its stop reason. Completion of one tool, a quiet stream, or partial prose must not be interpreted as completion of the request. The UI needs an explicit state for an interrupted, refused or incomplete result. [Prompt turn](https://agentclientprotocol.com/protocol/v1/prompt-turn).

On cancellation, respond to pending permission requests as cancelled and finish process cleanup. Readiness for another prompt follows the protocol outcome, not a timer that declares the agent done. [Cancellation](https://agentclientprotocol.com/protocol/v1/cancellation), [tool calls and permission requests](https://agentclientprotocol.com/protocol/v1/tool-calls).

ACP's optional client filesystem and terminal capabilities let an agent ask the app to read/write files and run commands. Advertise them only when implemented. Agents can also have their own tools, so these capabilities must not be advertised as an OS sandbox. [Filesystem methods](https://agentclientprotocol.com/protocol/v1/file-system), [terminal methods](https://agentclientprotocol.com/protocol/v1/terminals).

### Requested providers in the official registry

The registry entries inspected on the research date provide these distributions:

| Provider    | Registry entry / distribution snapshot                                | Studio integration candidate                                    |
| ----------- | --------------------------------------------------------------------- | --------------------------------------------------------------- |
| Claude      | claude-acp, 0.84.0, npm package @agentclientprotocol/claude-agent-acp | ACP adapter first; native fallback if qualification requires it |
| Codex       | codex-acp, 2.1.0, npm package @agentclientprotocol/codex-acp          | ACP adapter first; native app-server fallback                   |
| Pi          | pi-acp, 0.0.34, npm package pi-acp                                    | Community ACP adapter; native Pi RPC fallback                   |
| Antigravity | antigravity-acp, 1.2.1, platform-specific Google binary archives      | Native ACP server; qualify each supported platform              |

Source manifests: [Claude](https://raw.githubusercontent.com/agentclientprotocol/registry/main/claude-acp/agent.json), [Codex](https://raw.githubusercontent.com/agentclientprotocol/registry/main/codex-acp/agent.json), [Pi](https://raw.githubusercontent.com/agentclientprotocol/registry/main/pi-acp/agent.json), [Antigravity](https://raw.githubusercontent.com/agentclientprotocol/registry/main/antigravity-acp/agent.json).

These versions are a research snapshot, not the implementation's final dependency pins. A registry listing establishes a distribution path; it does not establish equivalent permissions, model controls, image support, MCP behavior or reliable completion on every platform. The app should display only capabilities negotiated and qualified for the installed version.

Use the official Rust ACP library as the initial implementation candidate behind Studio's own interface. Zeron's hand-written transport is useful lifecycle evidence, but reproducing its raw wire layer is not automatically necessary. The official library supports both client and agent implementation and is used by Zed's integration. [Rust library](https://agentclientprotocol.com/libraries/rust).

### ACP and fframes tools have different roles

ACP connects Studio's conversation UI to a coding agent. A small fframes MCP server can expose project selection, source lookup, frame rendering, inspection and audio analysis as tools that the agent calls. ACP session setup accepts MCP server configuration; image/resource content support is negotiated. A local CLI facade should offer the same operations for providers that cannot use the MCP path. [Session setup](https://agentclientprotocol.com/protocol/v1/session-setup), [content blocks](https://agentclientprotocol.com/protocol/v1/content).

Code retrieval remains an app/agent tool concern. ACP does not automatically map a pixel, timeline clip or SVG object to its Rust implementation.

## 4. Cap: a reference for the video workspace

The inspected Cap checkout contains a Tauri desktop app and a separate GPUI implementation. The [GPUI README](/root/Cap/apps/desktop-gpui/README.md) contains historical development sections, some of which describe earlier stages; the current source is the better authority for which behaviors are implemented.

The native app uses the same Cap project, editor, rendering and export crates as the existing app. Its [EditorInstance interface](/root/Cap/crates/editor/src/lib.rs) is UI independent. The [editor window](/root/Cap/apps/desktop-gpui/src/editor_window.rs) composes preview, playback, timeline and configuration UI around that engine.

### Frame delivery and playback

Cap's editor demonstrates that seeking state and rendering a frame are separate operations. Its preview callback delivers real renderer output through a bounded channel; background work normalizes stride and channel order before GPUI presents a RenderImage. The current code accounts for padded rows and RGBA/BGRA conversion. This is the right kind of explicit frame contract for Studio.

Playback and playhead updates arrive from background threads. The UI consumes coalesced signals on the main thread. A desired-state transport driver tracks play/pause/seek generation so a rapid scrub does not queue every obsolete pointer position. Old images are released as frames are replaced to keep GPU memory bounded.

Cap documents local playback measurements, but Studio must measure its own pipeline: fframes generates scenes, Cap primarily decodes/composites recordings. Their costs and caches differ.

### Timeline and history

Cap keeps selection/drag/trim/history mathematics outside GPUI in [editor_edits.rs](/root/Cap/apps/desktop-gpui/src/editor_edits.rs). Studio should use the same separation for time geometry, snapping, range selection and hit testing.

The data model differs. Cap edits a structured recording project. fframes scenes, timing and animations are ordinary Rust functions. Studio cannot directly borrow Cap's timeline mutation model: an edit must change an explicit runtime parameter or become a coding task, and the resulting timeline must be re-derived from the compiled project.

### Packaging

Cap's [release workflow](/root/Cap/.github/workflows/desktop-release.yml) and [GPUI build script](/root/Cap/scripts/build-gpui-binary.sh) show the breadth of platform work: native builds, bundled sidecars/libraries, signing, macOS notarization and Linux packages. Treat this as a release engineering workstream, not a final step after UI work.

Use Cap as an architectural reference. Its GPUI/editor code is covered by the repository's AGPL terms; the [license](/root/Cap/LICENSE) lists narrower exceptions for capture/camera crates. Do not copy those editor files into fframes' MIT code as part of this proposal. Zeron is MIT licensed; any source reuse should retain notices and go through a dependency review.

## 5. GPUI fit

The framework matches the requested native Rust UI. Current upstream separates the framework from platform startup: the canonical example imports gpui_platform::application. The platform crate selects macOS, Windows and Linux implementations and exposes Linux backend features. [GPUI example](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/examples/hello_world.rs), [platform manifest](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_platform/Cargo.toml).

GPUI's manifest identifies it as Apache-2.0. Zed has other crates with their own licensing; selecting GPUI does not mean adopting Zed's complete editor, agent UI or component stack. Start from the framework and app-owned components. [GPUI manifest](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/Cargo.toml).

Both reference apps use pinned fork revisions. Studio should first qualify one coherent upstream revision for gpui, gpui_platform and any runtime bridge, then use a fork only for a demonstrated blocking issue. Pinning and platform checks matter more than reproducing custom glass/window decoration.

## 6. What fframes can already supply

| Existing capability                         | Source                                                                                                        | How Studio uses it                                                          |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| Rust Video and Scene implementations        | [video.rs](../../fframes/src/video.rs), [scenes.rs](../../fframes/src/scenes.rs)                              | Source authority and compiled scene structure                               |
| Persistent frame generation and reports     | [preview.rs](../../fframes/src/renderer/preview.rs)                                                           | Render at time, timeline, SVG and diagnostics                               |
| CLI JSON results and progress               | [cli.rs](../../fframes/src/renderer/cli.rs), [logger](../../fframes/src/renderer/fframes_logger.rs)           | Bootstrap orchestration and agent tools                                     |
| Skia rendering and native playback          | [Skia crate](../../fframes-skia-renderer/src/lib.rs), [native player](../../fframes-native-player/src/lib.rs) | Renderer/scheduler/audio reference, without starting a second UI event loop |
| Native and runtime SVG tree representations | [svgr.rs](../../fframes/src/svgr.rs), [macro output](../../svgr-macro/src/nodes_to_svgtree.rs)                | Basis for selectable element metadata                                       |
| Agent video guidance                        | [fframes-video skill](../../skills/fframes-video/SKILL.md)                                                    | Versioned task context, design and audio guidance                           |
| Scaffolding                                 | [cargo-fframes](../../cargo-fframes/src/main.rs)                                                              | App-created starter projects                                                |

The important missing pieces are a project supervisor, managed compilation, renderer IPC, stable editor identities/source anchors, preset management, agent sessions and app installers.

The Video trait is Sync + Sized with associated constants, and Previewer is generic over a concrete video type. A prebuilt Studio executable cannot simply load an arbitrary newly written Rust crate into that API. Compile the user's project into its own renderer process; keep that Rust type and its borrowed media inside the process.

The native player currently owns a winit event loop and requires the main thread. It is a useful renderer/audio reference, but calling play() inside a GPUI window would not produce an embedded preview.

Timeline reports identify scenes by names/indices and resolved frame ranges. They do not provide stable scene-instance IDs or a complete object-to-source mapping. SVG id attributes and diagnostic bounding boxes are useful starting points; they are not yet a complete editable scene graph.

## 7. Principal feasibility risks

| Risk                                                                 | Consequence                                              | Experiment or mitigation                                                                   |
| -------------------------------------------------------------------- | -------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| Managed compilation still requires an OS SDK/linker/native libraries | “Install and create” fails on a clean user machine       | Prove one supported SDK per platform before promising a frictionless local build           |
| ACP adapter behavior differs                                         | Agent appears stuck, context is lost, or edits run twice | Qualified version matrix, lifecycle fixtures and a provider-neutral interface              |
| Visual selection has ambiguous source ownership                      | Prompt changes the wrong object or every instance        | Stable IDs and frame/source revision matching; show scope explicitly                       |
| Agent changes shared Rust helpers                                    | Selected edit affects unrelated scenes                   | Dependency-aware retrieval plus before/after checks outside the target range               |
| Preview uses a different backend from export                         | Effects or color differ in the final file                | Match backend and record rendering capability; explicitly flag unsupported shader fallback |
| Native frame readback/upload grows memory or latency                 | Scrubbing stutters or memory grows continuously          | Bounded frame buffers, explicit image disposal and measured pipeline stages                |

The accompanying architecture and roadmap describe proposed mitigations. None of these experiments has been completed in this research pass.
