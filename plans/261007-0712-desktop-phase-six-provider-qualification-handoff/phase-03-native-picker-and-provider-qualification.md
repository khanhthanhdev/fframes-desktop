---
title: "Phase 3: Native picker and provider qualification"
status: todo
---

# Native picker and provider qualification

## Overview and dependency

Priority P1. Tentative estimate 7–11 engineer-days, excluding waits for accounts/native hosts. Depends on stages 1 and 2 safety gates. Deliver the native picker and qualify the full Claude/Codex/Pi/Antigravity matrix. Read [provider research](../reports/research-261007-0712-desktop-phase-six-providers.md), [M5 evidence](../../desktop/qualification/m5-results.json), [desktop README](../../docs/desktop/README.md) and [roadmap M6/M7](../../docs/desktop/implementation-plan.md#m6--qualify-providers-and-implement-handoff).

## Verified context and data flow

Native configuration/open behavior lives in `desktop/app/src/conversation_panel/host.rs:264,270,423,449`; opening reads configuration and launches no adapter. `AgentWorkflow` commands enqueue (`desktop/app/src/agent_workflow.rs:525,571,582,713`) and snapshots feed the panel. Existing controls follow task lifecycle and reject closed permissions (`desktop/app/tests/agent_workflow_ui.rs:327,483`). Add picker/handoff controls to that surface; keep all blocking discovery, hashing, RPC and session persistence off GPUI.

Profile snapshots → provider picker/readiness/experimental labels → stage 2 actor switch intent → stopped-writer variant choice → context/continuity notice → new writer/task → existing validation/review/Apply/Undo → accepted source/preview. Session history offers read-only outgoing conversation access and eligible Continue; it never suggests model memory moved across providers.

The existing managed-SDK CLI test invokes the project's video command (`desktop/app/tests/real_sdk_selection.rs:20`) and validates rendered semantic title geometry (`:40`). M6 export tests reuse that CLI backend on an immutable revision. Native export queue/UI and consumer install → edit → export acceptance remain M7; passing backend exports must not be labelled native app export or release qualification.

## Requirements

Picker lists all four profiles with provider/distribution ownership, setup guidance, readiness, known negotiated limits and qualification status. Prefer the best two only after authentic comparable passes; otherwise display insufficient qualification evidence. Experimental connectors stay visible with limitations and cannot bypass writer containment. Custom/legacy profiles remain accessible.

Provide visible busy/Stop/wait states and explicit continue-draft/start-from-accepted choice with revision/outcome summary. Keep queued briefs and stale selections labelled. Show resumable session state, restoring, restored, failed/unsupported and fresh-session fallback. No additional approval gate beyond the product's required variant choice. Do not auto-start adapters on open or retarget queued prompts.

Each advertised connector passes authenticated create/edit/export through the existing backend, scoped title edit, build/inspect/preview, Apply/Undo/reopen, Stop/crash cleanup, restoration/fallback and actual tool routes. Include scene/range/element/rectangle context from M4/M5. Cross-provider handoff uses Claude→Codex or an equivalently passing pair, including unsaved drafts and failed/cancelled turns. No overlapping writers or post-stop writes are allowed.

Qualify Linux, Windows and macOS separately where advertised; archive existence is not support. Existing Linux-only transaction gates still apply. Physical display/input/audio, IME and access checks remain authentic gates. Missing accounts, hosts or prerequisites keep full acceptance blocked/not_run, never narrowed or inferred from Xvfb.

## File ownership

All paths are under `/root/fframes-desktop/`. Stage 3 owns these files after stages 1–2 transfer their shared boundaries.

| Action | Exact path | Purpose |
|---|---|---|
| Modify | `desktop/app/src/conversation_panel.rs` | Native picker, switch choice and continuity UI |
| Modify | `desktop/app/src/conversation_panel/controls.rs` | Pure state-derived controls and keyboard behavior |
| Modify | `desktop/app/src/conversation_panel/host.rs` | Profile selection/session integration |
| Modify | `desktop/app/src/conversation_panel/rows.rs` | Bounded continuity/history rows |
| Modify | `desktop/app/src/conversation_panel/qualification.rs` | Final strict evidence consumption |
| Modify | `desktop/app/src/agent_workflow/model.rs` | Final presentation integration only |
| Modify | `desktop/app/tests/agent_workflow_ui.rs` | Picker/switch/restore control contracts |
| Modify | `desktop/app/tests/x11_shell.rs` | Native development interaction smoke |
| Create | `desktop/app/tests/real_sdk_provider_handoff.rs` | Managed-SDK handoff/export integration |
| Create | `desktop/scripts/qualify-m6-providers.py` | Explicit development/authentic matrix runner |
| Modify | `desktop/scripts/test-qualification-m6.py` | Authentic evidence negative checks |
| Modify | `desktop/scripts/validate-qualification.py` | Complete contract/ranking validation |
| Modify | `desktop/qualification/m6-results.json` | Measured gate states and evidence references |
| Create | `desktop/qualification/evidence/m6-<platform>-<run-id>/` | Relative, hash-bound run summaries and sanitized artifacts |
| Modify | `docs/desktop/README.md` | Actual setup/picker/handoff/restore usage and evidence limits |
| Modify | `docs/desktop/architecture.md` | Implemented registry/manifest/context ownership |
| Modify | `docs/desktop/phase-zero-feasibility.md` | Owning qualification navigation and boundaries |

`<platform>-<run-id>` is an execution-generated directory, not a guessed current artifact. No code native driver or export UI is pre-authorized by this inventory. No deletion or broad documentation churn.

## Implementation steps

1. Extend pure control/snapshot tests first. Implement picker states using existing panel tokens/layout/input patterns. Keep provider switching and variant choice keyboard-operable, readable under bounded content and explicit about native memory continuity.
2. Wire stage 2 commands; show outgoing retained versions, missing prerequisites and unsupported restoration. Display the wait for any uncancellable Apply/Undo before offering a revision choice, and preserve queued briefs for explicit cancel/transfer. Read-only session history uses existing paged conversation rendering within 400 rows/4 MiB. Repeated selection/restore requests cannot start duplicate writers.
3. Implement a runner with explicit development versus authentic mode. Reuse M3/M5 test/world/SDK harnesses rather than duplicate tools/build paths. Record exact pinned adapter/CLI/runtime/dependency closure, platform, model/auth route class, SDK, timestamps and process ownership. Keep credentials, prompts and opaque IDs out of evidence.
4. Run development matrix first: fixtures exercise permissions, cancel/crash, long quiet turns, protocol errors, restore replay, stale grants/selection, both variant choices, queue ownership, bounded transcript/context and close/reopen cleanup. Use native X11 only as development UI evidence.
5. Run authentic matrix for all four providers with operator-provided authenticated setup. Probe first; contain/process-qualify writers in disposable scratch projects before enabling capture/Apply. Never use test writer injection in authentic runs. Record blocked setup as prerequisites. Unknown Antigravity ACP image/tool/cancel semantics are live probes; Pi requires proven CLI tool access rather than a false MCP pass.
6. For each eligible provider perform create → selected title/scene edit → inspect/build/preview → Apply → second edit → Undo → reopen → CLI export from the accepted immutable revision. Decode export and compare representative frames/metadata to the same revision. Cover compiler-error repair without exceeding one automatic retry. Actual broker results from all nine tools are required on the advertised MCP or CLI route; qualify advertised routes separately, visibly label unsupported MCP.
7. Verify image context with a bounded PNG whose randomly chosen visible property is absent from text; verify image-disabled agents receive honest text/artifact fallback. Test negotiated model/config options and approve/decline requests without accepting upstream declarations as behavior.
8. Measure Stop during streaming, permission wait and background command, adapter crash, app close and twenty edit/cancel/reopen cycles. Record descendants, escaped writers, post-stop writes, empty scope, compiler concurrency, RSS/row/image/context budgets and cleanup. No descendant or unproven termination is a pass.
9. Run restart load/resume at identical cwd with fresh grants, plus unsupported/expired/unusable session fallback. Demonstrate passing-pair handoff with accepted and unsaved drafts, external change during choice and stale selection. Require zero temporal overlap between outgoing and incoming writers and exact retained-project hashes.
10. Evaluate comparable results and select best two only from full mandatory passes. Keep others experimental with precise failures/prerequisites. Update smallest owning docs with actual commands, evidence routes and M7 export/release dependency. Leave prior qualification gaps explicit.

## Executable validation

Run from repository root. Existing targets are manifest-confirmed; new real_sdk_provider_handoff/test-qualification-m6 targets are created in this plan. Ignored SDK tests require a verified assembled `SDK_BUNDLE`; never treat a skipped test as a pass.

```bash
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test agent_workflow_ui
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test provider_handoff
cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test selection_tools
SDK_BUNDLE=/path/to/verified-sdk cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test real_sdk_provider_handoff -- --ignored
SDK_BUNDLE=/path/to/verified-sdk cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test x11_shell the_native_shell_selects_the_managed_starter_title_and_keeps_rectangle_scope_nonsemantic -- --ignored --exact
python3 desktop/scripts/test-qualification-m6.py
python3 desktop/scripts/validate-qualification.py
cargo fmt --all -- --check
cargo fmt --manifest-path desktop/Cargo.toml --all -- --check
cargo test --locked --manifest-path desktop/Cargo.toml --workspace
cargo clippy --locked --manifest-path desktop/Cargo.toml --workspace --all-targets -- -D warnings
just clippy
```

The SDK path above is an operator-selected prerequisite, not a shipped command value. The selected X11 test also requires Xvfb, xdotool, xwd and ffmpeg; other existing native shell tests require SDK_ACTIVE and should not be run without their own prerequisites. The runner's flags must be implemented/documented and checked through `--help` before use; no invented current command interface. Run root workspace tests if shared runtime/project contracts change. WASM checks apply only if shared/editor bridge changes arise, which are outside planned ownership. Stop all harness-owned processes on completion; never kill unrelated provider/user processes.

## Test and acceptance matrix

| Area | Development coverage | Authentic evidence required |
|---|---|---|
| Picker/setup | Missing runtime/auth, save race, unknown format, status distinction, keyboard controls | Each advertised provider/platform auth and runtime probe |
| Handoff | Stop barrier, variant preservation, dirty source, delayed events, queued prompts | Passing pair; zero writer overlap and no post-stop writes |
| Restoration | Exact opaque ID, unchanged cwd, replay dedupe, mismatch/unsupported fallback | Account-backed restart continuation and explicit fallback |
| Context/tools | Revision fences, bounded images/text, denied old capability, nine tool dispatches | Real calls through advertised MCP/CLI; visual property proof |
| Create/edit/export | Managed SDK, inspect/Apply/Undo/reopen and CLI frame/export parity | Each advertised connector completes same loop; native export remains M7 |
| Resources | Rows/images/queues and owned teardown | Twenty-cycle measurement; physical-device and platform gates independently recorded |

## Todo and success criteria

- [x] Native picker and handoff/restore states work without blocking UI/playback.
- [ ] Development safety/UI/SDK suite passes and its evidence is labelled development.
- [ ] All four provider entries retain exact observed versions, capabilities, prerequisites and authentic gate results.
- [ ] Every advertised connector passes mandatory authentic create/edit/backend-export, tools, cleanup and restart tests.
- [ ] A passing pair demonstrates safe retained-draft handoff; best two follow measured evidence only.
- [x] Experimental/unsupported limitations and missing platform/device evidence remain visible.
- [x] M6 backend export does not claim M7 native export UI, installers or release readiness.
- [x] Required tests/format/Clippy pass, qualification validator refuses tampered/fake claims and docs links resolve.

## Risk, security and rollback

High likelihood × high impact: credentials/platform hosts unavailable; report explicit blockers and keep authentic/full acceptance open. Medium × critical: provider subprocesses escape; fail ownership gate and disable editing/handoff until qualified. Medium × high: exported video from wrong revision; pin source/build identity and compare decoded frames. Medium × medium: performance/UI regression; measure existing budgets during repeated authenticated cycles and keep blocking work on actors/jobs. Medium × high: evidence overclaim; strict per-gate hash/launch/platform binding and negative tests precede any recommendation.

Rollback removes recommended status/picker integration while preserving registry/session records and retained variants. Disable failed connector launches without deleting source, archives or provider-managed account state. Invalidate outdated qualification fingerprints. Revert source only through guarded existing Undo; keep exported test outputs/evidence in run-owned directories. M7 consumes qualified results and remaining blockers; M6 must not silently lower its acceptance to fit the available environment.

Unresolved product questions: none. Execution uncertainties are provider authentication, actual capability/ownership behavior, resolved dependency pins and native hosts; record them as measured prerequisites rather than approval gates.
