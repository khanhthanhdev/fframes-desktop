---
phase: 9
title: "Agent permissions, questions and change review"
status: pending
priority: P1
effort: "2-3d"
dependencies: [8]
---

# Phase 09: Agent permissions, questions and change review

## Goal and owned paths

Differentiate requests that block the agent, neutral questions and proposed/completed changes without changing option semantics or review safety. M4: agent panel complete. Root: `C:\Users\Administrator\fframes-desktop\`.

- Modify `desktop\crates\studio-ui\src\agent\ui\{action_card,changes_card}.rs`, `agent\{cards,question_card,chat_tab,review_entry}.rs`; change-review styling in `agent\changes\{file_list,strips,unified,split,paint}.rs` only if proven needed.
- Shared theme `src\{metrics,typography,palette,lib,motion}.rs` only for required role mappings/permission border; gallery `desktop\app\examples\ui_gallery\agent.rs` and fixture state additions in `capture.rs`.
- Own `desktop\crates\studio-ui\src\kit\text_input\element.rs` only for the required compact question-field presentation builder and its existing input tests; keep the same native TextInput/state/IME implementation and default36px API.
- Preserve another person's one-line edits in **both cards.rs and question_card.rs**; inspect current diff and coordinate ownership before touching them. Read `changes\{host,model,source}.rs`/tests, `controls.rs`, reply tracker and workflow references, no review-engine/data change.

## Just-in-time source scout

Trace `permission_request` vs transcript `permission_card`, pending reply suppression, `ActionCard` callers (errors/validation also use it), `question_card`/`collect_answers`/`toggle_option`/`send_answers`, `ChangesCard`, candidate review actions and real change-view host states. Verify source option ids/kinds and stale/pending references. Extracted authority: `PermissionCard.tsx:42-110`, `AiChrome.tsx:25-51`, globals permission spectrum, ElicitationCard and TurnChangesCard ranges. Spectrum must not leak through generic ActionCard to ordinary errors/tools/questions.

## Native behavior lock and visual contract

- **Studio rejects sort before allows; allow options are Primary.** Preserve `options.sort_by_key(kind.starts_with("allow"))`, each option id/name/action and allow emphasis; do not mark first supplied option primary or label all requests Allow/Deny. If no reject option exists, current Decline fallback remains. Existing multiple-allow emphasis is an explicit exception to general one-primary guidance.
- Pending reply stays non-reentrant, stale/closed request cannot answer again, request refs correlate correctly. Question drafts start empty, nothing preselected/submitted, all items required as Studio already enforces, Cancel declines. No copying Oleafly URL-elicitation feature without an existing type.
- Permission request only: spectrum border2px, outer14/inner10, padding14, ShieldQuestion20 in28 slot; headingBodyStrong14, description reference13 adapted **Small12** for native existing type set; gap12; actions32px wrap/right aligned/gap8 after separator/top10. Surface/text/line use existing semantic palette, not reference's local #666/#404 literals. Ordinary permission history row remains quiet activity.
- Reference spectrum is radial `#1abc9c → #a07cfe → #fe8fb5 → #ffbe7b → #16a34a → #ffffff`, size300%×300%, position0%50%→100%50%→0%50% over14s linear (`globals.css:1469-1498`, directly source-checked). Define this exceptional permission-border palette centrally, not as card/text/status colors. Animate visible border only in Full/System-allowed motion; Reduced freezes. Build minimal native rendering in existing component; do not add a new renderer/shader dependency. If native gradient animation exceeds performance budget, use static spectrum and document the visual deviation; no pulsing whole-card fill.
- Questions: neutral card radius10/line/padding12/gap8/type12, compact28px field/type12; visible validation, labels and selection. Add one fluent compact presentation builder on existing TextInput if absent, applying named metrics/TextStyle inside kit rather than raw `.text_size` in the screen. Default fields stay36px; no second input/state family. No spectrum. Errors/validation retain semantic Notice/ActionCard treatment, not permission chrome.
- Proposed/finished changes: muted bordered radius8/padding10×6, filename mono11 + FileDiff14/gap8, compact24px existing review actions, green/red line counts **only when already supplied**. Preserve review surface, apply/reject/undo availability and stale-source guards. Never add commit or fake one-file-at-time behavior just from Oleafly.
- Requests stay outside transcript scroll above composer, separator/padding12/gap8, independent region capped at60% available agent-panel height as native analogue of60vh, further constrained so composer/controls remain reachable on short windows. No hiding unanswered requests beneath transcript or attachment strips.

## Tasks and steps

1. [ ] Add regression fixtures/assertions before changes: rejection-first option order, arbitrary labels/multiple allows/no-reject fallback, pending/stale/closed replies, unanswered/multi-choice/free-text questions, validation failures, proposed vs accepted/stale/unavailable change review. Preserve action ids/probes and real host tests.
2. [ ] Extend the existing ActionCard only with the minimal explicit permission presentation needed; don't add a generic AI chrome abstraction used once. Prototype border against both themes/Reduced, then wire only `permission_request`. Keep ordinary ActionCard callers unchanged.
3. [ ] Apply permission layout and wrapping without touching sorting/emphasis or reply functions. Verify reject appears before allow and original labels survive narrow wrapping. Pending/stale text is explicit and keyboard cannot send twice.
4. [ ] Style neutral question fields/choices and validation. Keep native text input/IME, no default selections, cancellation and collect-all requirement. Test many options/long labels at320px and retained draft while theme changes.
5. [ ] Style changed-file summary and existing full review in a separate2-3-file slice. Preserve one source of review truth, path labels, diff line status, copy/selection and safe disabled states; don't change host/apply/reject commands.
6. [ ] Constrain pinned request region using measured panel height; test several simultaneous permissions/questions plus queue/attachments, short window and expanded diff. Ensure scroll reaches every action, composer stays usable and focus return doesn't fall into clipped content.
7. [ ] Run T/L/G/W and existing change host tests via T; C `agent` permission/question/working/failed/code/history both themes/widths. Add genuine pending/stale fixture coverage where existing fixtures omit it. Verify color contrast and visible gradient costs under Reduced/Full.
8. [ ] N real-stack gate: receive request, reject one then allow a new one; answer/cancel a real question where adapter supports it; inspect diff then apply or reject disposable candidate and exercise existing Undo. Check promoted preview/audio match and focus survives return. Scripted W is controller evidence only; mark any unavailable provider path blocked.
9. [ ] Record M4 all-agent acceptance, native deviations, measured gradient cost and unqualified provider cases. Re-estimate final UI work against actual defect count before handing off.

## Acceptance and rollback

- [ ] Only active permission requests use spectrum; question/error/tool/composer remain correct neutral/semantic surfaces.
- [ ] Reject-first/allow-primary, fallback Decline, pending correlation, required question collection and safe review semantics unchanged; no new backend action.
- [ ] Multi-request short/narrow cases and real replies/review verified; frozen reduced motion has no persistent animation wakeups; all agent phases' shared callers still pass.

Rollback own request/review visual hunks only; preserve original one-line edits and all candidate/session data. Use static border fallback only with measured reason and recorded contract deviation. Dependency08; estimate2-3d; any need to change permission/backend semantics is a scope stop.
