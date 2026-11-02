---
phase: 7
title: "Agent transcript and activity"
status: pending
priority: P1
effort: "2.5-4d"
dependencies: [6]
---

# Phase 07: Agent transcript and activity

## Goal and owned paths

Reproduce the two voices and quiet expandable ACP activity while preserving native paging, selection and scroll behavior. Root: `C:\Users\Administrator\fframes-desktop\`.

- Modify `desktop\crates\studio-ui\src\agent\ui\{message,activity,chrome,mod}.rs`, `agent\cards.rs` (message/activity presentation only), `chat_tab.rs`, and markdown `style.rs`/`render\{blocks,code}.rs` only for proven contrast/wrapping gaps.
- Token additions: `desktop\crates\studio-theme\src\{metrics,typography,palette,lib}.rs` only for actual agent roles. Do not create a separate theme. Gallery `desktop\app\examples\ui_gallery\agent.rs`.
- Read-only behavior locks: `agent\transcript\{mod,rows,scroll}.rs`, markdown selection/cache/syntax and app `tests\agent_workflow_ui.rs`; add assertions in existing tests only for affected behavior, not backend changes. `cards.rs` has another person's1-line change: preserve it explicitly before any own hunk. Hand cards/dock callers to08/09 sequentially.

## Just-in-time source scout

Read `UserMessage`/`AssistantMessage`/`hover_lane`, `ROW_H`/`HOVER_LANE_H`, `activity_spans`/`sync_activity`/`row_element`/`activity_row`, reasoning/tool-output render paths, `WorkingTrailer`, list invalidation and markdown selection/render caches. Verify expansion defaults/current user overrides before modifying appearance. `transcript/scroll.rs` currently `STICK_THRESHOLD_PX=70` and `JUMP_DISTANCE_PX=320`: these **stay**, rather than Oleafly80px. Cross-reference extraction `chat-parts.tsx:551-563,617-655,849-924,1122-1163,1278-1580`, ResearchToolCard and ACP fallback Thinking.

## Visual contract and native adaptations

- Transcript insets16 horizontal/12 vertical; rows12 apart, inner activity/content6. User max85% right aligned, accent fill/on_accent white, radius10/padding12×8/Body14. Preserve6-line long-message clamp and explicit Show more/less.
- Assistant full available width, left aligned, lightly framed: light document + line60%, dark muted + transparent border, same10 radius/padding12×8. Markdown syntax/link/selection colors remain semantic and readable on new surface. No universal avatar column.
- Footer gap2,10px timestamps/tabular figures,14px Copy glyph, compact28px target with reserved28px lane (native adaptation of source6px copy padding); reveal on keyboard focus **as well as hover**. Copy confirmation is non-destructive and reduced-motion aware. No new edit/regenerate action unsupported by adapter.
- Activity summaries borderless and compact; 12px summary/disclosure,14px tool glyph,8px glyph gap. Reasoning body plain pre-wrap11/16 sans via one used theme text role, left rule with7px margin+12px inset/max224; output muted fill/radius8/padding10×8/max320,4,000-character initial cap with show-all preserving full content. Use existing CodeSmall11/16 for native tool output instead of source10px mono; this is an explicit readability adaptation, not an unresolved font choice.
- Set **agent-specific24px activity and28px footer baselines** plus4px vertical reasoning header padding, rather than deriving everything from32px global small buttons. Let wrapped detail grow; don't clip content to a fixed row. Status mark12, expansion gap6, summary inset10 where nested. Preserve real grouping and live-open/user override rules; no blanket hiding of completed meaningful results.
- One visible active thinking cue for the same stream; suppress duplicate fallback presentation only, not workflow phases or meaningful validation progress. No gradients on ordinary activity. Motion150ms at most; Reduced jumps to end/stops shimmer.

## Tasks and steps

1. [ ] Add regression fixtures first: user short/long/multiline, assistant Markdown/table/link/code, selected/copied text, reasoning streaming/empty-done, activity pending/running/done/failed/waiting, long output, history replay and tool-group expansion. Add assertion that row identity/selection survives remeasure and no lost outcomes.
2. [ ] Implement `UserMessage` and `AssistantMessage` styling in the first small slice; verify blue-on-white/white-on-blue contrast and selected text visibility. Reserve action space so hover/focus never changes transcript height. Keep `agent-row-*` ids and copy content unchanged.
3. [ ] Add used agent density/type roles and decouple row/footer constants from global control_sm_h. Update `ActivityHeader`/`ActivityRow` and footer callers, not all ordinary buttons. Recheck header/status/tray shared use to avoid accidental phase06 regressions.
4. [ ] Restyle expanded reasoning/tool output and associated summaries; cap rendered preview without truncating stored/copyable data. Preserve current tool status/outcome mapping, `activity_spans` grouping, manual override and selective `list.remeasure_items`; no backend grouping rewrite to imitate web summaries.
5. [ ] Resolve duplicate Thinking cue only where same live reasoning is actually rendered. Keep phase-specific Working/Validating feedback when no reasoning supplies it. Verify error/notice surfaces remain persistent and distinguishable; permission request styling stays phase09.
6. [ ] Run T/L/G/W; C `agent` all9 states at320+520 and both themes. Inspect wrapping, code overflow/scroll, full-width framing and keyboard-focus actions at125/150/200% DPI. Gallery runs real components but not the working native controller; record that limitation.
7. [ ] In N with real adapter/SDK: stream a long answer/tool run, scroll back beyond70px, continue stream, expand/copy code and reasoning, load older rows, jump to latest beyond320px. Ensure reader anchor doesn't move unexpectedly, selection/caret isn't stolen, ids stay measurable and outputs/outcomes remain available. Repeat on saved history/reconnect if supported.
8. [ ] Compare streaming scroll p95/stalls/remeasures/memory against phase01 on the same deterministic row workload (1,000 rows). Investigate budget regressions rather than removing coverage. Log phase acceptance and hand off shared files.

## Acceptance and rollback

- [ ] Blue max85% user and framed full-width assistant match source contract; dense activity is not a stack of heavy cards; ordinary controls still32/36.
- [ ] Focus/hover actions don't move rows; text/Markdown/code copying/selection and semantic colors are legible in both appearances.
- [ ] Follow70/jump320, paging/resident limits, ids,6-line clamp and real activity behavior unchanged. No forced scroll or hidden meaningful result.
- [ ] Source/visual/test/performance/native evidence all recorded truthfully; W is not described as native/provider qualification.

Rollback one visual slice at a time; preserve native scroll/selection code and another owner's `cards.rs` hunk. If rendering performance fails, first remove unnecessary animation/repaint and use bounded detail; do not expand resident history or weaken tests. Dependency06; estimate2.5-4d.
