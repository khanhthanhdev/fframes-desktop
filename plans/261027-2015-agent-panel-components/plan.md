---
title: "Agent panel: chat components in Cap's style"
description: "Redesign the Studio agent panel (chat, composer, tool activity, prompts, trays, history) as a small set of components built only from Cap's kit and the editor palette, learning layout and behaviour from Zeron's chat UI."
status: pending
priority: P1
effort: 6d
branch: main
tags: [design, desktop, studio, agent, ui]
blockedBy: [261027-2045-studio-design-system phases 1-3]
blocks: []
created: 2026-10-27
---

# Agent panel: chat components in Cap's style

## Why

The agent panel (`desktop/crates/studio-ui/src/agent`, ~21k lines) is the least consistent part of
Studio. A survey of the code found:

- the composer paints with raw `theme.gray(n)`, `blue_9` and `text_size(px(13.))` instead of the
  editor palette and `TextStyle` (`composer/mod.rs:685`, `attachments.rs:176`);
- five card radii (6, 7, 8, 9, 12) for the same roles, gaps of 2/4/6/8/10 between siblings;
- four button families side by side, two Stop buttons that look different
  (`chat_tab.rs:635` vs `composer/mod.rs:593`);
- three styles of "needs you" card in one stack (`section`, hand-built card, `Notice`);
- three ways to show tool status (icon, badge, plain text) and two outcome vocabularies that
  disagree (`mod.rs:860` vs `history/mod.rs:89`);
- the empty state repeats "Configured · not checked yet" twice and the composer hint repeats its
  placeholder.

Cap has no chat UI, so the panel improvised. This plan defines the chat components once, in Cap's
editor style, with layout and behaviour borrowed from Zeron (`C:\Users\Administrator\code\zeron`,
`crates/ui/src/transcript.rs`, `composer.rs`, `todo_panel.rs`, `queue.rs`).

## Principles

1. **Cap's kit and editor palette only.** Every element uses `theme.editor` roles, `METRICS`,
   `TextStyle`, and kit components. No `gray(n)`, no raw colours, no raw font sizes in `agent/`.
2. **Content first, chrome quiet.** Only the user message has a bubble. Assistant text is flush.
   Tool activity is one muted line until opened. Colour appears only for status and the primary
   action.
3. **One style per role.** One button per role, one card for "needs you", one status vocabulary,
   one row height for lists.
4. **Hover never moves layout.** Anything shown on hover (copy, time, row actions) has its space
   reserved.
5. **Same ids.** Every element id, probed bounds name and `debug_selector` listed in the inventory
   (see "Ids to keep") survives, so `x11_shell.rs`, telemetry and the in-folder tests keep passing.

## Scale

This plan is phase 6 of [the Studio design system](../261027-2045-studio-design-system/plan.md). Tokens and kit names follow that plan: `theme.colors` (was `theme.editor`), `Button` with `Emphasis` (was `EditorButton` filled/accent/destructive), one constructor per component (`::new`), `TextStyle::Code` (was `Mono`). Where the table below differs from phase 1's tokens, phase 1 wins.

Everything in the panel uses these values and nothing else.

| What | Values |
| --- | --- |
| Spacing | 4, 8, 12, 16 (`METRICS.space_1..4`) |
| Radius | 6 inner (chips, rows, code inline, small buttons), 12 outer (cards, bubble, composer, code blocks, trays), full (send/stop, jump pill) |
| Row heights | 24 list and activity rows, 28 chips and compact controls, 32 tray headers |
| Text | `Body` 13 for messages and input, `Label` 12 for rows and titles in rows, `Small` 11 for meta and captions, `BodyStrong` for card titles; mono 12/18 for code and output |
| Icons | 14 in rows and buttons, 12 for status marks |
| Panel inset | 12 left and right |
| Motion | `FAST_MS` for hover and fold, `NORMAL_MS` for height changes; reduced motion snaps |

Colour roles: `text_1` primary, `text_2` secondary (assistant meta, subjects), `text_3` captions
and placeholders; `ctl` user bubble and chip fill, `ctl_hover` row hover, `ctl_active` selected;
`card` composer and cards, `card_2` code and output wells; `line` borders; `accent` send, primary
button, spinner, focus; `green_11`, `amber_11`, `red_11` status marks with `*_3` soft fills.

## Layout

```diagram
┌──────────────────────────────── sidebar (320–520) ┐
│ [Agent][Proj][Assets][Style][Props][Slides]       │  existing TabRail
├───────────────────────────────────────────────────┤
│ [Chat|History|Changes]                    (+) (⚙) │  PanelHeader 44
│ ● Claude Code · ready              Asks every time│  StatusLine 24
├───────────────────────────────────────────────────┤
│                    ┌───────────────────────────┐  │
│                    │ Make the intro bigger     │  │  UserMessage
│                    └───────────────────────────┘  │
│ Sure. I'll scale the title and slow the fade.     │  AssistantMessage
│ ▸ Read 3 files · edited 2 files · ran 1 command   │  ActivityGroup (collapsed)
│ ┌───────────────────────────────────────────────┐ │
│ │ 2 files changed  +18 −4              [Review] │ │  ChangesCard
│ │ M scenes/intro.rs                             │ │
│ └───────────────────────────────────────────────┘ │
│ ◌ Working · 12s                                   │  WorkingTrailer
│                    ( ↓ Jump to latest )           │  JumpPill (floating)
├───────────────────────────────────────────────────┤
│ ┌▌Run cargo build? ─────────────────────────────┐ │  ActionCard (dock)
│ │ The agent wants to build the preview.         │ │
│ │                          [Deny] [Allow once]  │ │
│ └───────────────────────────────────────────────┘ │
│ ┌ ☑ Todo 2/4 · Retime the outro fade        ⌄ ─┐ │  Tray
│ ┌───────────────────────────────────────────────┐ │
│ │ [chip] [chip]                                 │ │  Composer
│ │ Describe the change you want                  │ │
│ │ 📎 🖼                     Opus · High ⌄   (↑) │ │
│ └───────────────────────────────────────────────┘ │
│ Mode [Default|Plan]   Model [Opus ⌄]              │  OptionsBar (only if the agent advertises more)
└───────────────────────────────────────────────────┘
```

## Components

Chat components live in `agent/ui/` (new module) and are `RenderOnce` builders like the kit.
Generic pieces that other screens can use go into `kit/`.

### Kit additions

| Component | Spec |
| --- | --- |
| `Spinner` | 12 px, `accent`, one rotation per 1100 ms (the todo panel's current spinner, moved to the kit). Reduced motion shows a static ring. |
| `Chip` | 28 high (24 compact), radius 6, `ctl` fill, no border, px 8, gap 6, 14 icon, `Label` text in `text_1`, truncates at a max width; optional remove button (16, radius 6) shown on hover in reserved space; `.tone(Tone)` swaps icon colour only. Replaces the composer context chip, file chip and history badges that are not status. |
| `IconButton::round` | full radius, sizes 28 and 24; `.solid()` fills `text_1` with a `card` glyph, `.accent()` fills `accent` with `on_accent`. Used for send and stop only. |
| `TextStyle::Code` | 12/18 in the platform mono family. Removes the two copies of `MONO_FAMILY` and `CODE_SIZE` (`markdown/style.rs`, `changes/paint.rs`). |
| `outcome_style(Outcome) -> (&str, Tone)` | one status vocabulary for the transcript, status line and History. |

### PanelHeader

- 44 high (`METRICS.toolbar_h`), px 12, `line` bottom border.
- Left: kit `SegmentedControl::editor` with Chat, History, Changes. Ids stay `agent-tab-Chat`,
  `agent-tab-History`, `agent-tab-Project`. Replaces the hand-built `segment_rail`.
- Right: `IconButton::editor` Small for New chat (`agent-new-chat`) and Settings
  (`agent-open-setup`), gap 4.
- A panel notice renders below as a `Notice` with `on_dismiss`, not a separate × button.

### StatusLine

One 24 px line replaces the current headline, the policy badge row and the phase strip.

- `StatusDot` (tone from phase) + `Small` `text_2` summary ("Claude Code · ready",
  "Working on: retime outro", "Waiting for your review"), truncating.
- Right: policy as a `Badge` (Neutral, Info or Warning as today), then the details chevron
  (`agent-task-details`) when a task exists.
- Details expand below in a `card_2` well, radius 12, p 12: lines, evidence thumbnails (128×72,
  radius 6), conflict lists. Review buttons (`agent-apply`, `agent-discard`,
  `agent-export-candidate`, `agent-review-banner`) sit in a right-aligned row at the bottom.
- Adapter not checked or missing: no paragraph of internals. One `Notice` (Warning) "Agent not
  set up" with **Set up agent** (opens settings) and **Check now**. The setup explanation moves to
  the settings page.

### UserMessage

- Right-aligned, max width 85%, `ctl` fill, radius 12, px 12, py 8, `Body` `text_1`, no border.
- Attachments above the bubble as `Chip`s or 56×56 thumbnails (radius 6), gap 4.
- Over 6 lines: collapse to 6 with a text-only **Show more** (`Small` `text_2`, hover `text_1`).
- Source captions ("Your answer", "Follow-up", "Automatic repair context") as `Small` `text_3`
  above the bubble.
- A 20 px hover lane below holds time (`Small` `text_3`) and a copy `IconButton` Small; the lane is
  always reserved.

### AssistantMessage

- No frame, flush with the panel inset. `MarkdownRow` with `Body` 13/20, gap 8 between blocks.
- Markdown on the scale: headings 16/14/13 semibold (no 18 or 14.5), lists gap 4 with a 16 px
  marker column, quote bar 2 px `line_strong`, inline code `card_2` fill with radius 4 and px 2.
- Code block and table: `card_2`, `line` border, radius 12; header 28 with the language in `Small`
  `text_3` and an `IconButton` Small copy (`md-code-copy-*`); body `TextStyle::Code`, px 12,
  pb 8, scrolls horizontally.
- Selection colour from `theme.selection()`, not the hard-coded `hsla` in `markdown/style.rs:55`.
- Same hover lane as the user message, left-aligned.

### ActivityGroup and ActivityRow

Consecutive tool calls, reasoning and permission notes form one group.

- **Collapsed:** one 24 px row: 14 px chevron, `Label` `text_2` summary built from counts:
  "Read 3 files · edited 2 files · ran 1 command · 1 failed". Hover `text_1`. It stays muted
  when a child failed; only the "1 failed" segment uses `red_11`.
- **Running:** the summary ends with a `Spinner`; the newest group opens while it runs and folds
  `NORMAL_MS` after it settles (Zeron's behaviour).
- **Expanded:** `ActivityRow`s indented 16 under the chevron, gap 0:
  - 24 high, px 6, radius 6, hover `ctl_hover`;
  - 14 icon in `text_3` (from `tool_verb`), verb `Label` `text_1`, subject `Label` `text_2`
    truncating;
  - trailing status mark, 12 px: `Spinner` while running, nothing when done, `red_11` X when
    failed (and the icon and verb turn `red_11`); pending shows the verb in `text_3`;
  - a 14 px chevron shows on hover when the row has detail.
- **Row detail** (output, diff, arguments): `card_2` well, radius 6, px 8, py 6,
  `TextStyle::Code` in `text_2`, at most 24 lines, then "… N more lines" and **Show full output
  (12 KB)** as `Small` `text_3` links. Diffs reuse the review window's line painter.
- **Reasoning** is a row in the group: verb "Thought", subject the first line; open only while it
  streams; body `Small` `text_2`.
- Permission notes in the transcript (`perm-auto-*`, `perm-open-*`, `perm-notice-*`) are rows:
  "Allowed: cargo build", "Waiting for you: cargo build" (amber icon), "You chose Deny".
- Ids stay `("agent-tool", id)` and `("agent-thought", id)`.

### ChangesCard

- `card` fill, `line` border, radius 12, p 12, gap 8.
- Header: `BodyStrong` "2 files changed", `Small` "+18" `green_11` and "−4" `red_11`, right-aligned
  **Review** `EditorButton` filled (`agent-review-candidate`).
- Rows 24: 12 px mark letter (A `green_11`, M `text_2`, D `red_11`), path in `Label`, directory in
  `text_3`. At most 5 rows, then "…and N more".

### ActionCard (everything that waits for the user)

One card for permission requests, questions, the outside-file prompt, held and refused messages,
and the read-only notice. It is the kit `Notice` with a fixed recipe:

- tone bar 3 px: Warning when the agent is blocked on the user (permission, question, stage),
  Info when it only informs (held, read-only), Danger for errors;
- title `BodyStrong` `text_1`, body `Small` `text_2`, p 12, gap 8, radius 12;
- actions right-aligned, gap 8: secondary `EditorButton` filled, primary `EditorButton` accent;
  destructive choices use `.destructive`;
- questions: options are `RadioCards` (one per option, description inside), free text is a
  `TextInput::plain`; **Send answers** primary, **Cancel** secondary;
- ids unchanged (`perm-request-*`, `perm-{key}-{option}`, `question-*`, `agent-held`,
  `agent-stage-*`, `agent-refused-*`).

Errors and validation results in the transcript use the same card with Danger or Success; their
body wraps and never truncates, and a copy `IconButton` sits in the title row.

### Tray (todo and queue)

- Sits directly above the composer, inset 8 more than the composer on each side, `card_2` fill,
  `line` border, radius 12 at the top and 0 at the bottom, overlapping the composer by 8.
- Header 32: 14 icon, `Label` title ("Todo", "Queued"), `Label` `text_3` count, and when
  collapsed the current item truncated; 14 chevron at the right (`todo-panel-toggle`).
- Rows 24, px 8, gap 8, `Label`: done `text_3` with strike-through, current `text_1` medium,
  pending `text_2`; status marks are `Spinner`, 12 px check `green_11`, or a 10 px ring in `text_3`.
- Queue rows add a remove `IconButton` Small (`queue-remove-{id}`) shown on hover in reserved
  space; stale rows add "will be refused" in `amber_11`.
- At most 220 high, then it scrolls.

### Composer

- `card` fill, `line` border, radius 12; focus changes the border to `accent` at 60%.
- Attachments row (when present): `Chip`s and 56 px thumbnails, px 12, pt 12, gap 8.
- Input: `Body` 13/20 `text_1`, placeholder `text_3`, caret `accent`, px 12, pt 10, grows from 1
  to 8 lines (max 200) then scrolls.
- Controls row 40, px 8, pb 8:
  - left: attach file and image as `IconButton::editor` Small (`composer-attach-file`,
    `composer-attach-image`); a disabled reason goes in the tooltip;
  - right: the model `Select` drawn as a ghost (no fill, `Small` `text_2`, chevron), only when the
    agent advertises a model option; then the send button.
- Send: `IconButton::round` 28 `.accent()` with an up arrow (`composer-send`); disabled at
  `DISABLED_OPACITY`. Its label (Send, Send at boundary, Steer) becomes the tooltip.
- Stop: the same 28 circle `.solid()` with a 10 px square (`composer-stop`). It replaces the
  separate `agent-stop` in the status strip; `agent-stop` stays as the id of the details-area
  button only if telemetry still needs both.
- While running with text typed: a `Small` `text_3` line under the composer explains what Send
  will do ("Sends after this step" or "Steers the current step"), and **Send now** moves into the
  send button's menu instead of a third button.
- No helper line that repeats the placeholder.
- Slash menu: the kit `Menu::editor` anchored above the composer, rows 28, `Label` name and
  `Small` `text_3` description.
- OptionsBar: only when the agent advertises modes or options beyond the model. One row, gap 8:
  `SegmentedControl::editor` for modes, `Select` and `Toggle` with `Small` captions.

### WorkingTrailer and JumpPill

- Trailer under the last row while the agent works: `Spinner` + `Small` `text_2` "Working" +
  elapsed `text_3` ("12s"). Before the run starts: "Sending".
- Jump pill: 28 high, full radius, `card` fill, `line` border, `Pop` shadow, px 12, `Small`
  `text_1` "↓ Jump to latest" (plus " · N new"), floating 8 above the dock
  (`agent-jump-latest`). Shown when more than 320 px from the bottom.

### Empty states

- No project: centred `Heading` "Agent" and one `Small` `text_2` line.
- New chat, agent ready: centred `Title` "What should we change?", one `Small` line, and up to 3
  suggestion `Chip`s from the project (e.g. "Make the intro faster", "Add a title card").
- Agent not ready: the StatusLine notice above, nothing else.

### History tab

- Search `TextInput::plain` (`history-search-input`) and **New session** as `EditorButton` filled
  (`history-new-button`, same family as the header's New chat), gap 8, pb 8.
- Rows 40, px 8, radius 6, hover `ctl_hover`, current `ctl_active`: title `Label` `text_1`,
  meta `Small` `text_3`, right-aligned `StatusDot` + outcome from `outcome_style`.
- No surrounding hand-built card; the list sits on the panel.

### Changes tab

Keep the content (Review, Undo, Recovery, Accepted edits) as `Section`s with `field_row`s;
spacing on the scale. Accepted edits are History-style rows with a **Review** button.

## Ids to keep

All ids from the inventory survive, in particular the probed bounds whitelist
(`agent/mod.rs:808`): `agent-apply`, `agent-undo`, `agent-review-candidate`,
`agent-review-banner`, `agent-tab-Chat`, `agent-tab-History`, `agent-tab-Project`, `agent-stop`,
`agent-held-cancel`, `agent-refused-restore`, `agent-refused-dismiss`, `agent-stage-confirm`,
`agent-stage-cancel`, `agent-open-setup`, `agent-save-adapter`, `setup-Label`, `question-*`; the
probe names `composer-input`, `composer-attach-file`, `composer-attach-image`, `composer-stop`,
`composer-send-now`, `composer-send`, `composer-slash-row-{n}`, `history-row-{n}`,
`history-search`, `history-new`; and every `debug_selector` used by the tests in `agent/`.
`composer-send-now` moves into the send menu; its probe name stays on that menu item.

## Phases

1. **Foundations (done in design-system phases 1-3).** Kit `Spinner`, `Chip`, `IconButton::round`, `TextStyle::Code`,
   `outcome_style`; gallery entries for each in light and dark.
2. **Composer (1.5d).** Palette and `TextStyle` only, round send and stop, chips, ghost model
   select, slash menu on `Menu`. Removes every `gray(n)` from `agent/composer`.
3. **Transcript (2d).** UserMessage, AssistantMessage and markdown on the scale, ActivityGroup
   and rows, ChangesCard, WorkingTrailer, JumpPill, hover lane.
4. **Dock (1d).** ActionCard recipe for permission, question, stage, held, refused; Tray for
   todo and queue.
5. **Header, status and tabs (1d).** PanelHeader on `SegmentedControl`, StatusLine, empty states,
   History and Changes tabs.
6. **Guard (0.5d).** A test in `studio-ui` fails when a file under `agent/` calls `gray(`,
   `rgb(`, `hsla(`, `text_size(` or `rounded(px(` with a value off the scale; allowlist starts
   empty for `agent/`.
7. **Gallery and review (1d).** An "Agent panel" page in `ui_gallery` with fixture states: empty,
   not set up, working, failed tool, permission, question, todo plus queue, long code block,
   history list; captured in light and dark at 320 and 520 widths.

## Verification

- `cargo test -p studio-ui` and `desktop/app/tests/x11_shell.rs` pass unchanged (ids kept).
- The guard test passes for `agent/`.
- Gallery captures for every state above, reviewed side by side with Cap's editor sidebar.
- `just clippy` clean.

## Open questions

- Does Studio's GPUI fork have Zeron's per-pixel `EdgeFade`? If not, the transcript fade at the
  composer edge is a gradient overlay on the opaque card.
- Should History become a header popover instead of a tab? It would remove the second tab row, but
  `agent-tab-History` is in the telemetry whitelist, so this plan keeps the tab.
