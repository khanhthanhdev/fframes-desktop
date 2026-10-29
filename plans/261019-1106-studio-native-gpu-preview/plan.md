---
title: "Native GPU render preview in fframes Studio"
description: "When a GPU works, Studio's preview and export render with Skia on the GPU (Vulkan or Metal) so shaders show; without one they use the built-in CPU renderer, and Settings can choose Auto, GPU, Skia CPU or CPU."
status: in-progress
priority: P1
effort: 7.5d
branch: main
tags: [feature, desktop, renderer, sdk]
blockedBy: [261019-1437-studio-layout-a]
blocks: []
created: 2026-10-19
---

# Native GPU render preview in fframes Studio

## Overview

When a GPU is available, Studio's preview and export workers always render with the Skia GPU
backend: Vulkan on Windows and Linux, Metal on macOS. Shader layers then appear, drawn by the same
renderer the projects' own CLIs use. Without a GPU, Studio uses the existing built-in CPU renderer
(tiny-skia) by default, so shaders are still skipped there. A Settings option chooses Auto, GPU,
Skia CPU or CPU.

This continues [the diagnosis thread](https://ampcode.com/threads/T-01a1537e-90e4-72a7-bbcf-dff9edffc300).
That thread found that the VM is not why the preview runs on the CPU. The worker hard-codes
tiny-skia (`fframes-studio-runtime/src/preview_worker.rs:344`, `src/export.rs:125-130`), and Studio
only builds and accepts a `"cpu"` worker, so a physical GPU would change nothing today.

## Scope challenge (HOLD SCOPE)

- **Existing code:**
  - `fframes::FrameRenderer` / `FFramesRenderBackend` already abstract the renderer, and
    `fframes_skia_renderer` implements both.
  - The Vulkan and Metal setup exists in `examples/fframes-studio-intro/src/render_cli.rs:90-126`.
  - Hello already carries a free-form `backend` string and `capability_gaps`.
  - The worker already measures `render_duration_micros`.
  - No new protocol version is needed.
- **Requested scope:** Skia GPU rendering for Studio's preview, with export on the same backend. The
  architecture promises that preview and export share one backend (`docs/desktop/architecture.md:371`).
  The SDK packaging is required to deliver it, not an addition. Validation
  session 1 added a Settings renderer picker and kept the red-team additions.
- **Complexity:** about 28 files in 5 crates plus the SDK script; two new modules
  (`fframes-studio-runtime/src/renderer.rs`, `studio-engine/src/services/renderer_preference.rs`).
  Five phases.

## Key decisions

1. **Selection lives in the runtime.** `fframes-studio-runtime` gets a default `skia` feature.
   `serve_preview_worker` and `run_export` keep their signatures and choose the renderer themselves,
   so `desktop/crates/studio-project/templates/src/bin/studio_worker.rs` and existing projects need
   no code change.
2. **Selection order (user decision):**
   - `Auto` (the default): use the GPU whenever one works (Vulkan, preferring real devices over
     CPU-type ones; Metal on macOS). Otherwise use tiny-skia, with the shader gap reported.
   - Skia CPU runs only when it is chosen.
   - A 64×64 synthetic probe frame decides, not the user's frame 0. It runs before Hello, so Hello
     names the backend that actually runs.
   - A GPU that fails mid-session falls back to tiny-skia in the worker.
   - A GPU that hangs is killed by the engine, which relaunches the worker on tiny-skia.
3. **Preference from Settings, passed by argument.** Settings offers Auto, GPU, Skia CPU and CPU.
   Workers get `--renderer`, and changing the choice relaunches the worker without a rebuild. Studio's
   own `FFRAMES_STUDIO_RENDERER` overrides the choice for scripted tests. The child environment is
   cleared and hashed into the build key, so a variable there would never arrive, or would force a
   rebuild.
4. **Three meanings of `"cpu"` are split.** The build key (`skia`), the runtime backend from Hello
   (`skia-gpu`, `skia-cpu`, `cpu`), and the thumbnail cache key (the runtime backend).
   `shader_preview` is a gap only for `cpu`.
5. **The Skia archive is a pinned, fail-closed SDK input.** Its hash is checked in and reviewed, the
   archive is nested in the framework artifact, `SKIA_BINARIES_URL` points at it, and the SDK copy of
   the manifest enables `skia-safe/no-compile`, so a miss is a fast error (the dev workspace is unchanged). It is also listed in the signed runtime
   inventory with its licenses.
6. **No migration from older SDKs (user decision, session 2).** Studio is in development, so a
   project pinned to an older SDK is refused with the existing "SDK pin mismatch" error and is
   recreated. Framework crate versions stay `0.2.0`.
7. **Transport stays the same.** Frames are read back to straight RGBA and sent over loopback.

## Non-goals

- Zero-copy texture sharing between the worker and GPUI.
- A release profile for the worker build.
- Raising the 1280×720 preview cap.
- Vulkan Video or VideoToolbox hardware encoding in Studio export.
- Projects that add their own Skia features (they fail fast with the key named).
- Changes to `studio-ui/src/shell/player.rs` and `studio-ui/src/frame_image.rs`, which hold
  uncommitted edits from `261019-1437-studio-layout-a`.

## Phases

| # | Phase | Status |
|---|-------|--------|
| 1 | [Phase 1: Runtime renderer selection](./phase-01-runtime-renderer-selection.md) | Completed |
| 2 | [Phase 2: SDK Skia packaging](./phase-02-sdk-skia-packaging.md) | In progress (Linux/macOS runner checks and the signed inventory remain) |
| 3 | [Phase 3: Engine backend wiring](./phase-03-engine-backend-wiring.md) | In progress (engine done; the relaunch and thumbnail key move to Phase 4) |
| 4 | [Phase 4: Renderer setting, metrics and backend label](./phase-04-preview-metrics-and-backend-label.md) | Blocked on layout-a |
| 5 | [Phase 5: Verification and docs](./phase-05-verification-and-docs.md) | Pending (needs the GPU hosts) |

Dependencies run 1 → 2 → 3 → 4 → 5. Phase 4 also waits until `261019-1437-studio-layout-a` is
committed, because it touches `settings.rs`, `status_strip.rs` and `observation.rs`. Phases 2 and 3
each start with a spike that can stop the plan: the Skia archive key and the SDK pin.

```diagram
┌───────────────────────┐  --renderer <pref>     ┌─────────────────────────────────┐
│ studio-engine         │ ─────────────────────▶ │ project worker (debug build)    │
│ renderer_preference   │  Hello{backend,detail} │ fframes-studio-runtime[skia]    │
│ coordinator           │ ◀───────────────────── │  64×64 probe before Hello:      │
│  deadline → relaunch  │  RGBA over loopback    │   Vulkan/Metal ─fail─▶ tiny-skia│
│  as cpu               │ ◀───────────────────── │   Skia CPU only when chosen     │
└──────────┬────────────┘                        │  renderers: main + thumbnail    │
           ▼                                     └─────────────────────────────────┘
┌───────────────────────┐   SDK v5: vendored skia crates, pinned skia-binaries
│ Settings, label, p95  │   archive in framework/skia, SKIA_BINARIES_URL=file://,
└───────────────────────┘   no-compile; projects of older SDKs are refused
```

## Success criteria

- [ ] On the Windows Vulkan host and the Apple Silicon Mac, Auto reports `skia-gpu`, and a project with
      an SkSL shader layer shows the shader in the preview, where today's worker draws nothing in its place.
- [ ] On this VM (no GPU), Auto reports `cpu` with the shader warning and the reason in the status bar
      tooltip. Choosing Skia CPU in Settings shows the shader.
- [ ] Export uses the same backend as the preview. With a Skia backend, the shader pixels are in the
      exported file.
- [ ] The Settings choice survives a restart, and changing it switches the backend without a rebuild.
- [ ] The SDK builds a Skia worker with the network blocked, fails fast on a bad archive, and
      lists Skia in the signed inventory with its notices. The SDK size growth is recorded.
- [ ] Per-frame worker render time (p50/p95) is visible in the observation JSON, and
      `reports/benchmark.md` holds preview, export and build numbers for each backend.
- [ ] `just clippy` and the test commands in each phase pass.

## Red Team Review

### Session — 2026-10-19

**Findings:** 28 raw, deduplicated to 15 (15 accepted, 0 rejected). Every finding cites code. Three were
checked directly: the SDK pin (`build_materialization.rs:26-54`), the lock deletion (`:230-233`), and
the Skia CPU speed note (`fframes-skia-renderer/src/backends/mod.rs:101-104`). The rest were accepted on their cited evidence. The user kept all of them as applied (validation session 1).
**Severity breakdown:** 3 Critical, 9 High, 3 Medium.

| # | Finding | Severity | Disposition | Applied To |
|---|---------|----------|-------------|------------|
| 1 | SDK v5 pin mismatch strands every existing project; no repin path | Critical | Withdrawn in session 2: Studio is in development, so older projects are not migrated | Plan |
| 2 | `FFRAMES_STUDIO_RENDERER` is stripped from the cleared child environment, and is hashed into the build key if allowlisted | Critical | Accept: `--renderer` argument | Phases 1, 3, 5 |
| 3 | Skia archive fails open: curl ignores offline mode, silent source build, archive unpacked by the installer | Critical | Accept: `no-compile`, nested archive, network-blocked check | Phase 2 |
| 4 | Lock-refresh premise is false; Studio already deletes and regenerates the lock in staging | High | Accept: task removed | Phase 3 |
| 5 | Thumbnails interleaved with main frames drop the Skia context and shader cache on every size change | High | Accept: one renderer per size class | Phase 1, Phase 5 |
| 6 | Probing with the user's frame 0 before Hello risks the 30 s deadline and blames user errors on the GPU | High | Accept (modified): 64×64 synthetic probe before Hello, so Hello stays truthful | Phase 1 |
| 7 | No recovery from GPU loss or a GPU hang mid-session | High | Accept: worker fallback plus engine relaunch | Phases 1, 3 |
| 8 | Skia archive hash is trust-on-first-use; qualification inherits host `SKIA_*` variables | High | Accept: checked-in pin, stripped variables | Phase 2 |
| 9 | Skia missing from the signed runtime inventory and notices | High | Accept | Phase 2 |
| 10 | Key spike would read a cached or dev-workspace key, and other targets' keys need their own hosts | High | Accept | Phase 2 |
| 11 | Export probe and speed: probe through the encode path, use MaxPerformance, add an export benchmark | High | Accept | Phases 1, 5 |
| 12 | Skia CPU is slower than tiny-skia by the renderer's own docs | High | Accept, superseded by validation session 1: tiny-skia is the no-GPU default, so no cost spike is needed | Phase 1 |
| 13 | Thumbnail cache key must use the runtime backend | Medium | Accept | Phase 3 |
| 14 | Shaders need `compile-time-svgtree`, which the `skia` feature must enable | Medium | Accept | Phase 1 |
| 15 | Wrong citations and missed sites (template worker, `SkiaCpuCtx` path, test fixtures, architecture.md:71) | Medium | Accept | Phases 1, 3, 5 |

### Whole-Plan Consistency Sweep

Searched all plan files for `from_env`, the `FFRAMES_STUDIO_RENDERER` worker-environment route,
`cargo update`, the lock refresh, `<sdk>/skia/`, `skia_backend.rs:108`, `annotated-video-overlay`
used as the project template, and `release-manifest.rs:504` used as a contract site. The remaining
mentions are intended: the fixture is named as the SDK qualification fixture, line 504 is named
as the test fixture that stays, and `FFRAMES_STUDIO_RENDERER` is named as Studio's own variable.
No unresolved contradictions.

## Open questions

None remain. Validation session 1 resolved all four.

## Validation Log

### Session 1 — 2026-10-19
**Trigger:** `/ak:plan validate` after the hard-mode plan and red-team review.
**Questions asked:** 7, including one follow-up to clarify an answer.

#### Verification Results
- Claims checked: 9 (added after the red-team review; the earlier claims were verified there).
- Verified: 8 | Failed: 1 | Unverified: 0
- Tier: Light (the red-team guard applied)
- Failures: `prepare_examples.rs:203` should have been `:196` (the "CPU-rendered copy" text). It is
  a line-number typo, fixed in Phase 5 without asking.

#### Questions & Answers

1. **[Scope]** The red-team review added scope you didn't ask for: (a) a confirmed v4→v5 SDK repin,
   (b) killing a hung GPU worker and restarting it on the CPU, (c) listing the Skia binaries and
   licenses in the signed SDK manifest. Which should the plan keep?
   - Options: Keep all three | Keep (a) and (c) | Drop the repin | Decide per item
   - **Answer:** Keep all three.
   - **Rationale:** Without (a) every existing project stops building on SDK v5.
2. **[Assumptions]** When no GPU works, which renderer should Studio use?
   - Options: Skia CPU gated by a cost spike | Skia CPU always | tiny-skia default, Skia CPU when
     forced | Skia CPU for preview, tiny-skia for export
   - **Answer:** Other.
   - **Custom input:** "If have GPU, always on GPU, if not CPU by default"
   - **Rationale:** This reverses the plan's assumption that the fallback would be Skia CPU.
3. **[Assumptions, follow-up]** To be precise about "CPU by default": which CPU renderer?
   - Options: The existing built-in CPU renderer (tiny-skia) | Skia on the CPU | tiny-skia by default,
     Skia CPU through the override
   - **Answer:** The existing built-in CPU renderer (tiny-skia). Shader layers stay missing on
     GPU-less machines.
4. **[Architecture]** How should Studio offer the v4 → v5 repin?
   - Options: Per project on open, with confirmation | Once for all projects | Automatic, journaled
   - **Answer:** Per project on open, with confirmation.
5. **[Scope]** Should users be able to choose the renderer themselves?
   - Options: No picker, env override only | Settings option now: Auto / GPU / Skia CPU / CPU |
     Settings option later
   - **Answer:** Settings option now: Auto / GPU / Skia CPU / CPU.
6. **[Risks]** Is a GPU machine available for the acceptance run?
   - Options: Windows with a Vulkan GPU | Apple Silicon Mac | Both | None
   - **Answer:** Both.
7. **[Scope]** How should the 15 applied red-team findings be handled?
   - Options: Keep as applied | Review the table | Revert all
   - **Answer:** Keep as applied.

#### Confirmed Decisions
- Selection: Auto uses the GPU whenever one works, and tiny-skia otherwise. Skia CPU runs only when
  chosen. This favours speed on GPU-less machines over shader fidelity there.
- Settings picker with Auto, GPU, Skia CPU and CPU, stored in `AppSettings`. A change relaunches the
  worker without a rebuild.
- The repin is offered per project on open and needs a confirmation.
- All three red-team additions stay, and all 15 findings stay as applied.
- GPU acceptance runs on both a Windows Vulkan host and an Apple Silicon Mac.

#### Action Items
- [x] Phase 1: fall back to tiny-skia instead of Skia CPU, remove the cost spike, and update tests.
- [x] Phase 2: the SDK probe runs once with `skia-cpu` and once with the default (`cpu` on a GPU-less host).
- [x] Phase 3: hang relaunch uses `cpu`, and the preference comes from Settings.
- [x] Phase 4: add the Settings picker and its persistence.
- [x] Phase 5: VM checks use the default `cpu` and Skia CPU from Settings, GPU checks run on both
  hosts, and parity uses the intro CLI's own backend.

#### Impact on Phases
- Phase 1: Goal, Context, selection order, probe failure, mid-session fallback, tests and Risks.
- Phase 2: Hello probe and Verification.
- Phase 3: `renderer_preference.rs`, coordinator relaunch, fault test and manual check.
- Phase 4: renamed to "Renderer setting, metrics and backend label", with new files and tasks. Effort
  goes from 0.5d to 1d.
- Phase 5: Context, Tasks and Risks.

### Whole-Plan Consistency Sweep
Searched all plan files for "then Skia CPU", "relaunch as skia-cpu", the Phase 1 "cost spike",
"open question", "not reviewed", the old Phase 4 title, and the Settings picker non-goal. Each was
updated: the frontmatter description, the overview, decisions 2 and 3, the diagram, the non-goals,
success criteria 1 to 4, red-team row 12, and the Phase 1 to 5 bodies. The remaining `skia-cpu`
mentions are the explicit-choice backend, which is intended. No unresolved contradictions.

<!-- slug: studio-native-gpu-preview -->

### Session 2 — 2026-10-19
- **Decision (user):** "I want it totally new since it's in development, no need to care about the
  compatibility for old version." The v4 → v5 repin (answer 1a above, red-team finding 1) is
  dropped. The engine repin shipped in `v0.1.2` was removed again; nothing else changes.
