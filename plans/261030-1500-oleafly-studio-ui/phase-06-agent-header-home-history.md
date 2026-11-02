---
phase: 6
title: "Agent header, home and history"
status: pending
priority: P1
effort: "1.5-2.5d"
dependencies: [5]
---

# Phase 06: Agent header, home and history

## Goal and owned paths

Give the native external-agent panel the ACP frame without transplanting unsupported controls or built-in research behavior. Root: `C:\Users\Administrator\fframes-desktop\`.

- Modify `desktop\crates\studio-ui\src\agent\ui\chrome.rs` (`PanelHeader`, `StatusLine`, `panel_empty`), `agent\mod.rs`, `chat_tab.rs`, `history\mod.rs`; host presentation in `history_host.rs` only if required for existing actions.
- Shared token ownership for this phase: `desktop\crates\studio-theme\src\metrics.rs` and typography only for required compact roles; preserve global sizes. Gallery `desktop\app\examples\ui_gallery\agent.rs` with `capture.rs` for actual fixture additions only.
- Read `agent\setup_page.rs`, `provider_setup.rs`, `controls.rs`, `history\search.rs`/tests and `history_host.rs`; do not alter adapter readiness/session semantics or ranking. Hand `mod.rs`/`chat_tab.rs`/gallery to phase07 after gate.

## Just-in-time source scout

Trace `ConversationPanel` header and Chat/Changes/History tabs, `PanelHeader::render` current toolbar height, empty-home readiness from `adapter_status`, `HistoryList` events/search/current/unreadable entries and the host's session switching. Map each existing Studio action to Oleafly `AssistantShellHeader`, `AssistantShellAcpActions`, `AcpSessionHeader`, `AcpHistoryDialog` and `AssistantHome` source ranges in the extraction. Verify capability visibility before drawing an action. No new runtime switch, usage panel, floating panel, fork, delete or signout without existing support.

## Visual decisions

- Agent header **36px**, bottom hairline, paddingX8, gap6; utility groups gap2, target28/glyph16 (14 only existing float/hide equivalents). Keep Studio Chat/Changes/History navigation instead of adding Oleafly runtime choices.
- Reference order Plus/History/Settings2/BarChart3/PictureInPicture2/PanelRightClose is an inventory, not a required toolbar. Only New session, History and setup actions that Studio already supports are restyled/repositioned. Use existing glyph equivalents where clear.
- Session/title strip, if already present, padding12×6/type12/gap6 and24px actions, truncated title; don't create protocol/revision badges from nonexistent data.
- Home centered with20px gaps/24px vertical padding/Display24 heading and12px subtitle. Oleafly max672 adapts to available320-520 sidebar and existing Studio home max; retain Studio mark/Bot (64px tile/30px mark only if a tile exists), no mascot. Connecting state may use48px tile and existing cancel/status only.
- Oleafly quick-history288×max288 and full dialog70vh are reference geometry. **Keep Studio's existing searchable History tab/list**, compact title/agent/time hierarchy and shared36px search field as a native adaptation of the reference32px field. Do not introduce a second menu/dialog navigation path or special history input API solely for parity. Record this structural adaptation.

## Tasks and steps

1. [ ] Add missing ready/unchecked/checking/auth-required/connecting/failed/current/unreadable-history regression fixtures before changing layout. Preserve actual ids, event/shortcut names and search ordering tests.
2. [ ] Introduce only the used agent header/status metrics in the existing theme grouping and switch `PanelHeader` without altering shell52px header or global32/36 buttons. Keep all action targets and tabs keyboard reachable at320px.
3. [ ] Simplify duplicated readiness copy visually using existing source data: one clear status line plus optional details, not invented operational states. Restyle empty home/suggestions and not-set-up action with Studio copy; maintain focus-to-composer and settings-entry routing.
4. [ ] Restyle HistoryList search, current row, unreadable row, outcome icon and relative time. Preserve Return→best match and Escape→clear query, stable session ids and history host's load/resume fencing. Do not add RecentChats to ACP home or change saved history format.
5. [ ] Update gallery header/home/history fixtures to use the exact same component geometry as the real panel. Do not present the fixture's mocked controller as live-workflow parity; note gallery doesn't mount a working ConversationPanel.
6. [ ] Run T/L/G and C `agent`, `settings`. Review empty/not-set-up/failed/history at320+520 both themes; native test384/385, long agent/session names,125/150% DPI and keyboard-only use.
7. [ ] In N configure/check an existing adapter, open/new/resume a conversation, search/clear History, navigate Chat/Changes/History and setup settings then return. Keep credentials redacted; absent adapter marks live gate blocked. Record acceptance and hand off shared files.

## Acceptance and rollback

- [ ] Header measures36px independently of shell/global controls; no overflow or inaccessible utilities at320px.
- [ ] Ready/setup/connecting/failed/history identity is unambiguous without copying unsupported Oleafly controls or branding.
- [ ] Search/session persistence/events/ids unchanged and real native history flow recorded; compact metrics do not resize non-agent screens.

Reverse only new frame/home/history visual hunks if issues arise; preserve session records and other owners' work. Dependency05; estimate1.5-2.5d. This phase deliberately leaves transcript bubbles, dock and permission chrome for07-09 to avoid simultaneous ownership.
