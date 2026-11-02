---
phase: 12
title: "Integrated regression, documentation and handoff"
status: pending
priority: P1
effort: "2.5-3d"
dependencies: [11]
---

# Phase 12: Integrated regression, documentation and handoff

## Goal and owned paths

Qualify the whole native Studio experience and hand off an auditable visual migration without hiding test gaps or unrelated work. M6 is a release candidate, not authorization to publish. Root: `C:\Users\Administrator\fframes-desktop\`.

- Future doc edits: `docs\desktop\design-system.md`, `oleafly-design-system.md` only after implementation is accepted; preserve source extraction provenance and separate implemented guidance from historical study.
- Verification fixtures/tests: `desktop\app\examples\ui_gallery.rs`, `ui_gallery\{capture,agent,panels,kit_extras}.rs`, `desktop\crates\studio-ui\tests\style_guard.rs`, existing app tests only for proven missing assertions. Both guard allowlists stay empty/unmodified. No broad code cleanup.
- Execution status/evidence index in this new plan and `.amp\in\artifacts\oleafly-migration-12-<run>`. If a regression requires source changes, explicitly return ownership to the responsible phase, fix narrowly and rerun its dependent gates; final phase does not own every source file by default.

## Just-in-time source scout

Re-read accepted phase logs, current diffs/ownership, all supported `Page` states and actual app test scope. Verify commands against current manifests/binaries; check `CARGO_TARGET_DIR`. Reconcile docs' stale Cap layout origin references with the new Oleafly **visual** authority without rewriting historical implementation/licensing facts or older plans. Enumerate any new surfaces added during execution and assign them coverage; don't assume initial inventory still complete.

## Tasks and steps

1. [ ] Rebuild gallery and real app, then run **T/A/L/G/W** exact commands from plan.md with `--locked`. Record toolchain/date/platform/dirty revision identity and logs. Run full-app strict clippy as an additional diagnostic only; separate the already-known bootstrap deprecation at process/mod.rs:1220 from any new failure. Do not suppress it or claim it passed.
2. [ ] Capture **all11 pages/all declared states/both themes** in a fresh unique directory and inspect actual PNGs at full resolution, not merely output file existence. Verify agent320/520 panels and no accidental clipping from stitched capture. Exercise hover/pressed/focus interactively; record screenshots of keyboard-focused surfaces separately.
3. [ ] Run the full Windows native smoke on a disposable installed-SDK project: create/open/import/cancel pickers→real preview/playback→Project/Assets/Style/Props/Slides edits and undo→agent setup/history/send/stream/backscroll→queue/steer/Stop/SendNow→permission/question→candidate review/apply-or-reject/Undo→settings persistence→video/deck export/cancel→Present/fullscreen exit. Attach measured bounds/logs/screenshots or short recordings for each gate, not a general statement that app opened.
4. [ ] For each native agent capability distinguish provider-exercised, controller-scripted-only, unsupported, blocked. Run `agent_workflow_ui` for controller/scripted ACP/fake worker evidence; it opens no window and qualifies no provider. On a Unix host only, run ignored x11_shell with SDK_ACTIVE/Xvfb/xdotool as optional additional production-shell wiring proof; on Windows mark not applicable, never a successful empty test count as proof.
5. [ ] Complete cross-cut matrix: Light/Dark/System repaint, motion Full/Reduced/System, sidebar320/384/385/416/520 and short/minimum windows,100/125/150/200% DPI/monitor moves, long English/Vietnamese combining/CJK/emoji/mixed-direction paths. Native field typing/composition, selection, keyboard order,2px focus, accessible names/roles, disabled skip, menu arrows/Escape and focus return must work. Use available Windows screen-reader tooling; report unsupported platform accessibility gaps instead of claiming blanket compliance.
6. [ ] Compare exact workload baseline/final performance over three equivalent runs: idle wakeups/CPU, overlay open, resize, stream scroll p95/long stalls/remeasures/memory1,000 rows, playback drops/audio. Investigate>10% regressions or new>50ms stalls with absolute numbers and load context. Retain virtualization/cache/bounded-output evidence; measure permission border on/off and Reduced independently.
7. [ ] Triage final findings as blocking introduced regression, accepted deliberate adaptation, pre-existing unrelated defect or platform/provider qualification gap. Blocking: lost action/IME/focus/data/semantic id, unreadable contrast, hidden request, broken playback/export/present, introduced performance failure. Cosmetic nits can be recorded with owner and evidence; don't quietly widen scope to fix unrelated bootstrap/ghost issues unless blocking a touched flow.
8. [ ] After acceptance, update the two design-system docs: implemented token/kit APIs, exact compact agent metrics, icon inventory/licensing, native adaptations (history tab,160px growth,56px thumbnail, both Send+Stop, reject-first/allow-primary,70/320 scroll), gradient/static decision, capture/test command scope. Remove claims that agent treatment is merely future only when code/evidence supports completion. Do not edit old plan metadata or replace native behavioral documentation with web semantics.
9. [ ] Publish **nothing**. Review `git diff --check` and changed-file list; no foreign one-line edits, lockfiles, existing untracked assets or historical artifacts removed. Record executor-owned files and the small rollback unit for each phase. Ask for separate approval before any commit/push/deploy/publication; leave those as explicitly unperformed.
10. [ ] Mark only genuinely completed checkboxes/phases through scoped CLI phase status tooling if used, never old-plan pins. Write M6 handoff with result table, approved screenshots, reference revision, exact commands/results, known baseline failures, remaining platform/provider gates and next owner. If required native gates remain blocked, hand off as **not fully qualified**, not complete release-ready migration.

## Acceptance and handoff table

Use this table per execution run; leave unchecked until actual proof exists.

| Gate | Required evidence | Result |
|---|---|---|
| Token/kit T/A/L | logs, empty guard allowlists, contrast/font/icon checks | not run |
| Gallery G/C | both themes,11 pages/states, inspected PNG index | not run |
| Controller W | script/fake-worker scope labelled correctly | not run |
| Windows N | all critical native flows, real SDK/adapter, IME/focus | not run |
| Unix x11 | conditional platform/dependencies, never substitute for N | not run / conditional |
| DPI/a11y/localized-like content | screenshots + native interaction findings | not run |
| Performance | matched baseline/final numbers and budget verdict | not run |
| Docs/ownership/rollback | reviewed diffs, accepted adaptations, no foreign changes | not run |

- [ ] Every critical native gate passes or handoff clearly states blocked/not fully qualified; no static gallery-only completion claim.
- [ ] Visual contract is coherent across all screens and accepted native differences explicit; no unsupported Oleafly workflow/control/branding imported.
- [ ] No behavior/data/ids changed, no expanded guard exemptions, no hidden introduced failures, no source/font/icon licensing omission.
- [ ] M6 handoff includes realistic remaining uncertainty, no commit/push/publication, and precise own-hunk rollback sequence.

Rollback: if integration reveals a regression, reverse only executor-owned dependent visual slices in reverse phase order, preserving accepted foundation and everyone else's work. Re-run native baseline smoke after each reversal. No destructive project/session/SDK recovery operation is authorized. Dependency11; estimate2.5-3d assumes earlier phase gates were genuinely completed.
