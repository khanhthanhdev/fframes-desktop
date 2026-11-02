---
phase: 8
title: "Agent composer, chips and queue"
status: pending
priority: P1
effort: "3-5d"
dependencies: [7]
---

# Phase 08: Agent composer, chips and queue

## Goal and owned paths

Adopt the22px ACP composer shape and compact dock without replacing Studio's native input or queue/steer semantics. This is the highest-risk native interaction phase. Root: `C:\Users\Administrator\fframes-desktop\`.

- Modify `desktop\crates\studio-ui\src\agent\composer\{mod,metrics,attachments}.rs`, `composer\input\{render,layout,element,geometry}.rs` only for demonstrated geometry effects; regression tests in `composer\tests.rs` and `composer\input\tests.rs`.
- Dock presentation: `agent\{chat_tab,composer_host,todo_panel}.rs`, `agent\ui\tray.rs`; `agent\mod.rs` only if measured width/plumbing is required. Shared `studio-theme\src\metrics.rs` and required typography roles by handoff; kit compact select/icon APIs only if necessary, using existing controls not a second family.
- Gallery `desktop\app\examples\ui_gallery\agent.rs` and `capture.rs` for new mode/width fixtures. Read-only semantics: `composer\steer.rs`, `keymap.rs`, `input\{ime,pointer,motion}.rs`, attachments host workflow. No queue delivery/backend changes.

## Just-in-time source scout

Read `Composer::{render,controls,can_send,trigger,running_hint}`, `steer_actions`, SendAction/ComposerEvent, keymap dispatch and send menu focus. Trace native input bounds/candidate rect/selection/caret/wrap/caches, current8×20=160px cap and attachments56px thumbnail calculations. Read current queue/todo tray behavior and cancellation/retry/edit events through composer_host. Inspect actual resize support; Oleafly's manual resizer is a reference, not a new required feature. Cross-check `ChatComposer`, `composer-input`, `AcpComposer`, `AcpControls`, attachment/chip ranges in the extraction.

## Behavior lock (must survive visual changes)

| Studio state/mode | Primary + Enter | Ctrl/CmdEnter | Visible running controls |
|---|---|---|---|
| Idle, all modes | Submit | Submit | existing nonrunning actions |
| Running AtBoundary | SendAtBoundary | SendNow (cancel then send) | **Send + Stop + More ways to send** |
| Running Native | Steer | SendNow (cancel then send) | **Send + Stop + More ways to send** |
| Running None | no primary; current explanatory hint | SendNow (cancel then send) | Stop + More, no fake send |

Keep ShiftEnter newline, attachment readiness/processing/capability restrictions, model/config locking, reasons for disabled send, drafts and existing queue/todo ordering. Never mandate Oleafly's Stop-replaces-submit behavior; Studio intentionally exposes both where supported. Existing ids include `composer`, `composer-send`, `composer-stop`, `composer-send-menu-toggle`, `composer-send-now`, `composer-model`, `composer-attach-file`, `composer-attach-image`; preserve labels/probes/events.

## Visual decisions

- Shell radius 22, semantic card fill and hairline, padding 12 sides / 10 top / 8 bottom, and the border turns to the focus colour while the input has focus (Oleafly `focus-within:border-ring`). Oleafly also gives the composer a `shadow-sm` lift, but `Shadow::Card` is now empty. Use an existing shadow role that fits (`RaisedPill` in light is close to `shadow-sm`), or add one small `Shadow` role in `elevation.rs` through the phase 02 owner; don't put a literal shadow in the composer. Wrapper12 sides/bottom/4 top. No gradient. Body14 transparent input/no inner border,2px horizontal/8px vertical input inset.
- Resting viewport56px (including native vertical padding accounting); **retain160px auto-growth cap**, document difference from Oleafly224. Avoid reinterpreting max as number of lines after padding; tests must assert actual viewport/caret visibility. Retain existing native resize behavior; do not add60%-panel manual resize/grip if absent. Reference grip8px hit/32×4 visual/inset24/up4 and60% cap are explicitly not required new behavior.
- Controls min28px,8px top gap, left/right groups4px gaps; model compact28px/type12 medium/paddingX8/max176, unknown/unselectable model a label (no fake dropdown). Preserve capability-supported existing effort/mode controls only; no Plan/Web/Persona/Prompts imports.
- Send32px circular ArrowUp; Stop same-size accessible target with14px filled Square, still separately present when running. At **composer container<=384px**, send/stop28, shared gaps2, attachment/remove targets24; all semantic actions stay reachable. Nonshrinking right group, left controls may horizontal-scroll; lengthy model truncates with tooltip rather than hiding Stop/More/send. Never apply built-in label-collapse rules indiscriminately.
- Queue/context/attachments above shell; queue radius10/padding8×4/type12/Clock and truncated first line, existing Retry/Edit when supported. Attachments wrap gap8/radius8/name12/meta10/remove24; **retain56px image thumbnail**, not reference24, for Studio video/screenshot context. Processing/error clearly labelled. Keep native attach-file and attach-image actions, not a single faux paperclip dropdown.

## Tasks and steps

1. [ ] Before styling, extend existing composer tests across every table row, blank/multiline/attachment-only/processing/failed states, SendNow cancellation, menu Escape/focus return and all ids. Add layout fixtures at320,384,385,416,520 container widths and short height; no hard-coded answer to make geometry tests pass.
2. [ ] Add only used agent geometry metrics; implement shell/inset/resting56 and separate strip layout first. Update native geometry/IME candidate rect/scroll viewport together in a2-3-file slice; rerun input tests immediately. No text-engine rewrite or event rename.
3. [ ] Apply compact control sizing and width-aware arrangement, preserving both Send and Stop/More. Use available GPUI measured bounds, not window-wide breakpoint. Do not mutate state based on measurement or rebuild input entities while typing. Model/config locks remain capability-derived.
4. [ ] Restyle queue/todo/context/attachment strips in a separate slice, aligning remove targets and explicit errors. Recompute attachment strip height for retained56px thumbs and wrapping; preserve ids, data and queue ordering/retry/edit behavior. Verify pinned dock never covers the newest transcript row.
5. [ ] Run T/L/G/W and C `agent` all states/themes. Add real component fixtures for all three steer modes because existing working fixture alone cannot prove them. Inspect narrow right-group reachability with long model/agent/filename strings at125/150/200% DPI.
6. [ ] In interactive gallery and N use native IME (Vietnamese/CJK), combining/emoji selection, multiline paste, ShiftEnter, Home/End, word/line selections, drag/autoscroll, undo/redo, caret at wrap boundaries and candidate window placement. Verify no send on composition Enter and no lost draft when resizing/theme switching or opening menus.
7. [ ] With real adapter/SDK exercise supported boundary/native/none modes as available: send while running, steer/queue, Stop, More→Send now, Ctrl/CmdEnter cancellation, failed-queue retry/edit, attach image/file/remove. Unsupported provider modes get deterministic W coverage plus a marked provider-native gap; never claim one provider qualifies all modes.
8. [ ] Measure input/resize/attachment overhead against baseline, record retained160/56 deviations and acceptance, then hand `chat_tab.rs` and request dock ownership to09. Stop on any IME/send/queue regression before visual polish continues.

## Acceptance and rollback

- [ ] 22px shell/56px rest/32→28 submit adaptation visually correct at actual384 boundary, with all running controls accessible; global controls unchanged.
- [ ] Steer matrix, hints/ids/keyboard, processing restrictions and queue/todo behavior unchanged; no new backend/config feature.
- [ ] Native composition/caret/selection/undo and both-theme narrow/high-DPI evidence complete;160px growth and56px thumbs deliberately documented.

Rollback only new presentation/geometry hunks as a tested slice; never restore whole dirty input/host files or erase drafts/session data. Dependency07; estimate3-5d includes manual native input testing and measured-width corrections.
