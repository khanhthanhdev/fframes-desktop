---
title: "Stage 4: Native conversation, review and authentic qualification"
status: todo
---

# Stage 4: Native conversation, review and authentic qualification

## Context and dependencies

Depends on all three prior gates, including safe Linux publication. Estimate: 5–7 engineer-days. One executor owns native integration and evidence. An authenticated provider account is necessary for the final gate; absent access is NOT_RUN, not a fixture substitute.

The native backend is serialized (`desktop/app/src/studio_shell.rs:78`), shell state per entity (`studio_shell.rs:402`), and existing prepared installation is staged at `studio_shell.rs:394`. Actual source/install/audio guards run at `studio_shell.rs:1347`; preserve that control flow while integrating published-task authorization. Existing native text input supports composition/cursor handling (`desktop/app/src/text_input.rs:57`). The M0 agent spike exposes polling UI but not a virtualized conversation (`desktop/app/src/agent_spike.rs:31`); keep its qualification route independently available.

## User flow and data boundaries

- [ ] Configure/discover one adapter and show executable/runtime/auth readiness with guided provider-owned setup. SDK readiness and agent readiness remain distinct; project open never installs or launches an agent automatically.
- [ ] Submit brief against freshly reconciled current source; show frozen task/base/draft identities, explicit states, queued follow-ups and visible repair count. A second task waits for the first writer and refreshes its stable cwd/context safely.
- [ ] Render virtualized, paged messages/tool cards with batched deltas, advertised model/config options, permission choices, clarification/questions, structured errors and Stop. Unsupported controls stay hidden or explain their limitation.
- [ ] Show changed-file summary and validation coverage/diagnostics; automatic Apply after required gates is default. Manual review retains candidate and exposes Apply/discard/export-draft; changing policy while a task runs affects only subsequent tasks.
- [ ] Expose Undo and interrupted-task recovery with retained draft/candidate. Conflict states explain which paths changed and preserve variants. Keep M1 Restore as copy and M2 playback/timeline behavior intact.
- [ ] Produce machine-readable authentic two-edit/Undo/restart evidence, including repair/cancel/failure cases and cleanup; do not claim release/native-platform/physical qualification from Linux software rendering.

GPUI receives immutable bounded event/presentation batches; filesystem, adapter RPC, hashing, compile and inspection stay off UI/audio callback threads. View rows derive from ordered persisted events and task IDs; tool deltas update existing cards, permission/question reply IDs reject duplicates and stale clicks. Keep transcript scroll position while streaming, keyboard focus/Tab order and IME composition; batching cannot steal Space/timeline shortcuts from focused prompt input. Load only visible rows plus a bounded overscan and retain at most Stage 1 resident transcript limits.

Review setting is app-local per project (Stage 3 defaulted metadata), so it resets on a different install/project identity; display this scope. Optional manual review does not add a permission prompt for normal automatic tasks. Stop remains actionable during editing, waiting, validating and repair; promotion has an explicit short uncancellable commit boundary and Stop then requests safe interruption/recovery rather than hiding partial work. Closing cancels/reaps owned task/broker/compiler/worker consumers, while source journal completion determines accepted history.

M2 keeps the previous preview playing while work is prepared. After durable publication, hand off matching candidate timeline/frame/audio using latest playhead, seek serial and fresh output epoch; preserve/clamp position/selection and restore playing intent. Failures and conflicts keep previous video/audio and label the displayed revision. Accepted task/source may exist without a currently ready preview; show that state honestly. Streaming/tool renders cannot take the main frame lane or change accepted state.

## Files owned

| Action | Absolute path | Responsibility |
|---|---|---|
| Create | `/root/fframes-desktop/desktop/app/src/conversation-panel.rs` | Virtualized messages/cards, options, permissions/questions and prompt/Stop UI. |
| Modify | `/root/fframes-desktop/desktop/app/src/studio_shell.rs`, `/root/fframes-desktop/desktop/app/src/setup_view.rs`, `/root/fframes-desktop/desktop/app/src/lib.rs`, `/root/fframes-desktop/desktop/app/src/main.rs` | Serialized task commands, provider feedback, policy/review/Undo/recovery and qualification entry. |
| Modify | `/root/fframes-desktop/desktop/app/src/text_input.rs` | Reuse existing input; adjust only where prompt focus/IME tests show necessity. |
| Modify | `/root/fframes-desktop/desktop/app/src/agent_spike.rs` | Keep legacy spike routing compatible; do not replace its evidence gates with M3 claims. |
| Create | `/root/fframes-desktop/desktop/app/tests/agent-workflow.rs`, `/root/fframes-desktop/desktop/scripts/qualify-m3-agent.py` | Native/engine task workflow and authentic provider evidence harness. |
| Create | `/root/fframes-desktop/desktop/qualification/m3-results.json`, `/root/fframes-desktop/desktop/qualification/m3-results.schema.json` | Explicit pass/fail/blocked/not-run ledger with hashed evidence. |
| Modify | `/root/fframes-desktop/desktop/scripts/validate-qualification.py`, `/root/fframes-desktop/desktop/scripts/package-phase-zero.py`, `/root/fframes-desktop/desktop/scripts/test-packaging.py` | Validate M3 hashes and include tested CLI/MCP binaries in installed routes while preserving M0/M2 checks. |
| Modify | `/root/fframes-desktop/docs/desktop/README.md`, `/root/fframes-desktop/docs/desktop/architecture.md`, `/root/fframes-desktop/docs/desktop/phase-zero-feasibility.md` | After implementation, smallest owning surfaces document actual setup/task/recovery/tool route and qualification limits. |

New qualification evidence goes under `/root/fframes-desktop/desktop/qualification/evidence/linux-m3-<execution-date>/`; no credentials, private prompts or provider tokens. Proposed harness and schema are implementation deliverables, not currently runnable commands. Read docs before editing and link to machine-owned tools/contracts instead of duplicating details. Root/core crates and GPUI dependency pins remain untouched.

## Execution and qualification matrix

1. Wire shell commands/events to prior-stage engine, driver and shared tools; add virtualized conversation/review panel. Preserve old qualification/UI routes and current preview controls.
2. Test focus/IME, options/capability omission, permission/question reply correlation, bounded deltas/cards, repair status, manual-review Apply and conflict recovery through the native shell.
3. Create M3 harness/ledger validator. Track every owned PID/port/artifact lease; refuse occupied deterministic endpoints and stop only harness-owned processes. Redact before evidence serialization.
4. Run authentic adapter qualification with configured executable/account, record OS, adapter/SDK versions, auth route without secrets, v1 negotiation, writer process-group model and terminal cleanup, options, visual/restoration/CLI/MCP support and completion behavior. Unknown or detached/background writer behavior blocks Apply qualification; no general escaped-descendant detection is claimed. Only supported capabilities receive pass claims.
5. In a real generated/imported project, enter brief and produce playable result A; request second edit producing B; confirm timeline/frame/audio/revision identities, Undo to A (or explicit validated merged revision), restart and recover committed acceptance. Verify source/Git/draft inventories at every step.
6. Deliberately introduce a compiler error via authentic task, prove structured repair context and exactly one attempted repair. Exercise repair failure, permission/question waiting, Stop, provider crash, source conflict, interrupted publication/Undo and restart. Inspect retained drafts and no stuck writer; no checkpoint=validation inference.
7. Stream a long transcript during playback/scrub/tool calls; record resident transcript/event/tool/cache bounds, compiler count and resource cleanup. Repeat at least 20 edit-or-failed-task cycles; assert no owned process, broker capability or materialization/PCM lease after close. Compare representative frames/tools with existing CLI parity.
8. Document actual behavior and evidence limits; leave unavailable physical/Windows/macOS gates pending. Do not mark M3 authenticated acceptance complete without step 5 evidence.

Focused tests: create then run `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test agent-workflow`; `cargo test --locked --manifest-path desktop/Cargo.toml -p studio-agent-spike -p studio-engine -p studio-project`; then `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio`. Real SDK ignored tests require their documented SDK bundle and explicit execution; skipped is not pass.

Broaden public workflow checks with `cargo clippy --locked --manifest-path desktop/Cargo.toml --workspace --all-targets -- -D warnings`, `cargo fmt --manifest-path desktop/Cargo.toml --all --check`, and `cargo test --locked -p fframes-studio-protocol -p fframes-studio-runtime`. SDK/native prerequisites may be necessary for runtime checks; retain honest failure/blocked evidence. Existing packaging regressions run `python3 desktop/scripts/test-packaging.py`. Add new facade binaries to packaging only if required for the qualified CLI/MCP route, and test installed paths rather than repository paths.

Native commands already available: `cargo run --locked --manifest-path desktop/Cargo.toml -p fframes-studio` and legacy `cargo run --locked --manifest-path desktop/Cargo.toml -p fframes-studio -- spike-ui`. Define/document exact arguments for the new M3 harness during implementation; do not invent currently supported flags. Manually launch the configured real adapter via the native setup flow for the authentic gate.

## Risks, response and rollback

| Likelihood × impact | Failure and pre-decided response |
|---|---|
| High × high | Credentials/provider unavailable: mark authentic gates NOT_RUN; keep development evidence separate and M3 acceptance pending. |
| Medium × high | UI state outruns publication/playback authority: only engine committed events change acceptance; retain old preview and reject stale task/reply/install IDs. |
| High × medium | Transcript/tool work stalls playback: bounded virtual rows, batched events and separate tool lanes; reduce rendering workload, never drop critical state. |
| Medium × high | Close/crash leaves writer or leases: owned scopes, terminal broker capabilities and journal recovery; cleanup failure blocks qualification. |

Rollback returns native routing to existing M2 shell/spike controls, retaining journals/drafts/task evidence. Resolve pending transactions with Stage 3 reader before downgrading. Preserve source and last playable revision; no automatic history erasure. Qualification artifacts distinguish local development from release/platform certification.

## Gate and unresolved execution questions

- [ ] Real authenticated brief, second edit, Undo and restart meet source/history/preview identity checks.
- [ ] Compiler-error repair, Stop, provider failure, external conflict and interrupted mutation leave recoverable data and zero stuck writers.
- [ ] Virtualized native conversation/tool/options/question/review controls pass focus/input and bounded-resource checks.
- [ ] SDK/adapter/tool capability pins and evidence are recorded, with unsupported/physical/platform gates explicitly pending.

No blocking product question. Exact first qualified adapter/account, SDK/MCP pins and remaining native hardware access are execution prerequisites, not assumptions of success.
