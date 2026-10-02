# Phase 0 Native Execution Results

Date: 2026-10-01
Host: Linux 6.8.0-60-generic x86_64 (Ubuntu Noble baseline)
Rustc: 1.98.1 (48a229cea 2026-09-01)
Cargo: 1.98.1 (797e8a9bc 2026-08-05)

## 1. Test Execution Ledger

| Crate / Suite | Target | Tests Passed | Clippy Status |
|---|---|---|---|
| `fframes-studio-protocol` | Root | 7 / 7 passed | Clean (`-D warnings`) |
| `fframes-studio-runtime` | Root | 3 / 3 passed | Clean (`-D warnings`) |
| `studio-bootstrap` | Desktop | 5 / 5 passed | Clean (`-D warnings`) |
| `studio-sdk` | Desktop | 10 / 10 passed | Clean (`-D warnings`) |
| `studio-agent-spike` | Desktop | 4 / 4 passed | Clean (`-D warnings`) |
| `fframes-studio` (unit + integration) | Desktop | 13 / 13 passed | Clean (`-D warnings`) |
| `annotated-video-overlay` (fixture) | Desktop Fixture | Compiles cleanly | N/A |
| **Total Test Suite** | **All** | **42 passed, 0 failed** | **0 warnings** |

## 2. Feasibility Gate Evidence

### Gate 1: Managed Native Compilation
- Generated test project outside workspace with `cli` and `compile-time-svgtree` features enabled.
- Standalone Cargo build and frame render executed end-to-end to verified PNG output.
- SdkEnvironment constructs isolated `CARGO_HOME`, `RUSTUP_HOME`, and `CARGO_TARGET_DIR`.
- Result: **PENDING** (architecture and build verified; sterile clean-account air-gapped run required).

### Gate 2: GPUI Startup & Frame Presentation
- Candidate Zed revision `1a84d5d92bd7d6c1cabb116062650af545783fe9` fetched and locked.
- `TextInput` implements `EntityInputHandler`, registers via `ElementInputHandler` with bounds, shaped line layout, and keyboard action dispatch.
- `convert_rgba_to_gpui_bgra` swaps R/B channels, strips row stride padding, and produces valid `RenderImage`.
- Replacement manager calls `window.drop_image()` on previous image, capping resident image count at 1.
- Result: **PENDING** (components verified in test harness; physical compositor display pending).

### Gate 3: Renderer Worker & Crash Isolation
- `fframes-studio-runtime` implements borrowed `serve_worker` serving loop around `Previewer` and `CpuFrameRenderer`.
- Control channel uses length-delimited JSON with `WorkerRequest` and `WorkerResponse`.
- Binary channel streams raw frame pixels.
- `WorkerClient` implements latest-wins seek coalescing, payload validation, and process lifecycle management.
- Result: **PENDING** (IPC transport and seek logic verified; long-running interactive worker validation pending).

### Gate 4: ACP Task
- ACP v1 protocol driver and `AgentSupervisor` implemented.
- Secret redaction sanitizes tokens and passwords in diagnostic buffers.
- Live qualification recorded as `NOT RUN — credentials unavailable` (real provider credentials not supplied in repository environment).
- Result: **NOT RUN** (gate unmet by design; provider unselected).

### Gate 5: Selection Anchor
- `SelectionSpike` maps viewport letterboxed coordinates to video pixels.
- Validates project root containment, whole-file SHA-256, byte boundary alignment, and source revisions.
- Fixture derives actual whole-file SHA-256 and exact span from `main.rs`.
- Rejects stale worker generations, stale revisions, and modified files.
- Result: **PENDING** (anchor math and file validation verified; live UI click-through pending).

## 3. Residual Platform Status

- Linux x64: Pending interactive compositor validation and sterile clean-account installation run.
- Windows x64 MSVC & macOS Apple Silicon: Contracts defined; native physical runners required for final platform qualification.
