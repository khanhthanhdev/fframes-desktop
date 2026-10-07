# M6 assumption and contract verification

Date: 2026-10-07 (Asia/Saigon). Planning-only adversarial review of plan index and stages 1–2; stage 3 checked only for qualification consistency. No implementation changes, builds, tests, adapter installs or authentication runs. Source read and `rg` verification only. Full M6 scope; no `--yagni`.

## Result

No uncovered Critical/High/Medium finding in the reviewed current plan. The plan distinguishes implemented foundations from proposed work and authentic qualification. The principal failure scenarios—unknown writers, retained draft loss, changed source, stale grants, non-portable memory and unsupported restoration—have explicit gates and tests. No product decision needs reopening.

The following tables verify more than ten important claims per stage against owners and callers. Passing here means the **planning claim** matches inspected source, never that M6 is implemented or provider-qualified.

## Stage 1 — Provider profiles and evidence contracts

| # | Important claim | Verification |
|---|---|---|
| 1 | Native launch avoids ambient PATH discovery | `qualification.rs:148` constructs `ExecutableSearch` with managed adapters and `gui_path: None` at `:149–152`; discovery's generic search optionally supports explicit GUI PATH (`desktop/crates/studio-agent-spike/src/discovery.rs:24,95`). Preserve the native caller's restriction, not an assumed universal restriction of the helper. |
| 2 | Config stores environment names, not values | `desktop/app/src/conversation_panel/host.rs:44,55,101` stores/validates names; current `AdapterConfig` at `desktop/crates/studio-agent-spike/src/session.rs:25,31`; launch resolves values at `discovery.rs:207`. |
| 3 | Existing custom/auth/MCP fields must survive migration | `desktop/app/src/conversation_panel/host.rs:44–63` includes `provider`, executable, args, auth names, `auth_method`, `mcp`; resolver `:117–143` passes them into workflow settings. Plan explicitly preserves all existing fields. |
| 4 | Existing settings persist atomically off UI | `desktop/app/src/conversation_panel/host.rs:285,292`; native caller uses background executor at `desktop/app/src/conversation_panel.rs:787–793` and save ticket checks at `:769,797`. Registry migration must retain this ownership. |
| 5 | Probe uses scratch cwd and stops its process | `desktop/crates/studio-agent-spike/src/discovery.rs:308,340–359` starts driver with `options.scratch_cwd`, unknown writer ownership and no MCP servers; this is readiness, not edit qualification. |
| 6 | Load/resume are negotiated | `desktop/crates/studio-agent-spike/src/driver/events.rs:155–157`; initialize parses flags at `driver/runtime.rs:1537–1538`; session plan checks them at `:1626–1634`. |
| 7 | Stdio MCP is baseline, not a capability flag | Explicit host contract at `desktop/crates/studio-agent-spike/src/driver/runtime.rs:109`; config separates `mcp_servers` and host support policy at `:135–138`. Plan correctly requires actual successful tool calls separately. |
| 8 | User cannot assert writer ownership | `desktop/app/src/conversation_panel/host.rs:85–87` rejects legacy ownership setting; evidence-derived settings at `:125–136`; resolver at `qualification.rs:236`. |
| 9 | Legacy containment lookup is launch/platform/hash bound | `desktop/app/src/conversation_panel/qualification.rs:41–42,148–172,258,330,375,429–453`; tests cover changed executable/arguments, platform and evidence hashes at `:703,731,752`. |
| 10 | Legacy fingerprint is not already a dependency closure | `desktop/app/src/conversation_panel/qualification.rs:158–165` hashes only argument strings that name existing absolute files. Stage 1 step 1 expressly adds resolved artifact/dependency/runtime fingerprints and refreshes qualification when closure changes. This is proposed work, not an existing guarantee. |
| 11 | Provider candidates match current primary research | Source-cited observations in `research-261007-0712-desktop-phase-six-providers.md`: Claude ACP 0.86.0, Codex ACP 2.1.1/App Server, community Pi 0.0.34, Google Antigravity 1.3.0. Plan explicitly rechecks and avoids guessed Studio pins. |
| 12 | Pi MCP and Antigravity unknowns are kept honest | Research links the Pi adapter's explicit non-forwarding limitation and Google/registry sources. Stage 1 step 1 requires visible CLI qualification and leaves Antigravity image/tool/cancel unproven; stage 3 step 5 repeats both constraints. |
| 13 | Existing commands name real packages/test targets | Package names: `desktop/app/Cargo.toml:2`, `desktop/crates/studio-agent-spike/Cargo.toml:2`; test file `desktop/crates/studio-agent-spike/tests/acp_v1.rs` exists. `provider_profiles` and M6 Python tests are explicitly proposed, so cannot run before creation. |
| 14 | Validator extension is required, not implied present | `desktop/scripts/validate-qualification.py:82,446–448` currently owns authentic M3 MCP/CLI checks. These accept at least one call; proposed M6 comparable tool/image/workflow evidence must be implemented in the listed validator/schema changes. Existing M3 passes cannot be inflated into M6 recommendations. |

## Stage 2 — Controlled handoff and restoration

| # | Important claim | Verification |
|---|---|---|
| 1 | Reuse a per-project actor rather than a new orchestrator | `desktop/app/src/agent_workflow.rs:327,339,401–405` constructs bounded state/RowStore and spawns the workflow actor; source distinguishes actor/job/UI ownership at `:18–23`. |
| 2 | Actual task caller reaches controller setup | `desktop/app/src/agent_workflow/actor.rs:1221,1301–1307` chooses controller scoped/unscoped begin after resolving configured launch at `:1280`. Controller scoped implementation is `desktop/crates/studio-engine/src/controller.rs:751`. |
| 3 | Source base includes current external edits | `desktop/crates/studio-engine/src/controller.rs:758–764` reconciles, captures current source and verifies inventory identity; prior saved checkpoint is separate at `:815`. |
| 4 | Scope freezes revision instead of remapping | Controller checks project/source identity at `desktop/crates/studio-engine/src/controller.rs:769–777`; actor rejects obsolete displayed compiled scope at `desktop/app/src/agent_workflow/actor.rs:1234–1250`. |
| 5 | One reserved lease precedes draft refresh | `desktop/crates/studio-engine/src/controller.rs:788–804` reserves task and seals previous process scope before draft prepare; context is installed at `:826` and new scoped manager at `:840`. |
| 6 | Current actor always starts a fresh provider session | `desktop/app/src/agent_workflow/actor.rs:1541` sets `resume_session: None`; stage 2 must intentionally change this, not claim restart already exists. |
| 7 | Driver supports negotiated resume/load/failure | `desktop/crates/studio-agent-spike/src/driver/runtime.rs:1626–1683`; prefers resume, otherwise load; missing support returns RestoreUnsupported. |
| 8 | Display IDs cannot restore native sessions | `desktop/crates/studio-agent-spike/src/driver/runtime.rs:1706–1718` sanitizes emitted SessionReady; exact `wire_session_id()` at `:2126–2128` returns None if secret redaction would alter it. Current stage 2 step 2 explicitly respects this safeguard. |
| 9 | Stable per-project cwd exists | `desktop/crates/studio-engine/src/app_paths.rs:55–68` defines project agent/draft/marker/archive paths; `agent_task.rs:1083–1095` builds DraftStore from them. |
| 10 | Blind existing refresh would lose continued draft working state | `desktop/crates/studio-engine/src/agent_task.rs:1182–1245` archives retained draft then materializes `base`; accepted draft is removed. Plan explicitly requires a separate chosen-snapshot route before refresh and preserves archived bytes. |
| 11 | Restart refuses abandoned ownership | `desktop/crates/studio-engine/src/controller.rs:272–274` calls recovery; `agent_task.rs:1128–1138` converts Active to UnsafeWriter. Existing writer-gone evidence is checked at `:1150–1177`; plan does not infer safety from an absent process handle. |
| 12 | Capture must name current writer epoch and a single-use proof | `desktop/crates/studio-engine/src/agent_task.rs:824–869` verifies current writer generation, scope observation and quiescence; ticket consumption at `:875–901` prevents reuse. |
| 13 | Cancel completion is not termination evidence | Actor finalization calls cancel, waits, drains then calls driver shutdown and forwards outcome to controller at `desktop/app/src/agent_workflow/actor.rs:3295–3315`; scope shutdown seals before verified termination at `desktop/crates/studio-bootstrap/src/process.rs:1178–1196`. Plan separately blocks unknown/escaped ownership. |
| 14 | Tool grants must be replaced per incoming task | `desktop/app/src/agent_workflow/tools.rs:119–135` grants with task identity and draft revision; `:148–155` revokes and unregisters on task end. Stage 2 explicitly requires fresh grants and retired-token tests. |
| 15 | Replay is not already a stable message-ID event stream | `desktop/crates/studio-agent-spike/src/driver/runtime.rs:963–972` stores restoring text only in driver transcript pages; `driver/events.rs:283–290` live deltas contain role/text, no provider message ID. Plan lists minimal replay exposure if necessary and log ownership changes, so implementation must make replay/import boundaries explicit rather than assuming existing live-event sequence IDs dedupe historical content. |
| 16 | Existing app logs can preserve history across resume without provider replay | `desktop/app/src/agent_workflow/log.rs:152–177` loads persisted rows with local row identity; driver resume branch at `driver/runtime.rs:1655–1665` does not set `restoring`, unlike load at `:1667–1672`. Resume and load are different paths; plan's tests must exercise both. |
| 17 | Context limit fits actual guards | `desktop/crates/studio-engine/src/agent_task.rs:26,1261–1264` limits brief to 64 KiB; driver prompt max 256 KiB at `desktop/crates/studio-agent-spike/src/driver/runtime.rs:57`; tool text reply max at `desktop/app/src/agent_tools.rs:42`. Reserve identity fields inside the final aggregate envelope. |
| 18 | Claimed existing regression test targets exist | `desktop/crates/studio-engine/tests/task_recovery.rs`, `edit_transaction.rs`; `desktop/app/tests/agent_workflow.rs`, `agent_tools.rs`, `scoped_editing.rs`; `desktop/crates/studio-agent-spike/tests/acp_v1.rs`. Cargo auto-discovers integration files; new `provider_handoff` targets are clearly proposed. No command was executed. |

## Execution watchpoints already covered by the plan

- Legacy M3 containment may remain usable for its original narrow launch contract. It must not silently authorize an M6 launch with an unmeasured or changed runtime/dependency closure. Stage 1 requires extending resolver and validator together and invalidating closure changes; include a legacy-compatible launch whose transitive script changes as a negative fixture.
- Define resume history behavior separately from load replay. Preserve app-local rows on resume; either intentionally expose/import load pages with a provider-history boundary or preserve local history and present the replay as separate read-only history. Never dedupe by matching repeated text. Stage 2 already owns the needed log/driver seam and replay tests.
- First-party Antigravity binary availability and ACP registry initialize results cannot establish create/edit/export, tools or descendant containment. Stage 3 retains authentic scenario and platform gates; no inferred support found.

Unresolved questions: none for plan approval. Runtime accounts, exact dependency closures, provider restoration/image/tool behavior and platform evidence remain execution prerequisites recorded by the plan.
