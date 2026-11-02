---
title: "Studio design system: one palette, one kit, every screen"
description: "Centralise all of fframes Studio's styling (colours, spacing, radii, sizes, type, shadows, motion, UI config) into one token set and one component kit in Cap's editor style, then move every screen onto it and lock it with a style guard and a full UI gallery."
status: pending
priority: P1
effort: 22d
branch: main
tags: [design, desktop, studio, ui, refactor]
blockedBy: []
blocks: []
created: 2026-10-27
---

# Studio design system: one palette, one kit, every screen

## Overview

Studio's kit and palette are ports of Cap's GPUI editor (`C:\Users\Administrator\code\Cap\apps\desktop-gpui\src\ui`,
`theme.rs`), and `docs/desktop/design-system.md:3` already says screens "do not declare colours,
sizes or fonts of their own". The code does not do that. Two audits (theme and kit; every screen)
found:

- **Seven colour vocabularies.** The editor palette (`theme.editor`), the raw Radix steps
  (`gray_1..12`, `blue_*`, `red_*`, ...), legacy greys, `body_*` helpers, `settings_*` helpers,
  `SETTINGS_ACCENT` (fixed `0x007aff`, does not follow dark mode), and literals (`2a2a2c` in
  `menu.rs:236`, `hsla(211°…)` twice, raw `ef4444`/`2563eb` in `button.rs`). In Cap the `body_*`
  and `settings_*` helpers switch on native window materials; Studio dropped the materials, so they
  are now grey aliases that only make the settings look differ from the editor.
- **Many unused tokens.** 20 unused tokens and helpers. 330 raw `px()` literals inside the kit and
  about 265 outside it. About 95 local size and duration constants. `FAST_MS` and `NORMAL_MS` are
  never used, and `reduced_motion()` always returns `false`.
- **Duplicate components for one role.** Neutral, primary and danger buttons come in four
  families (`Button`, `EditorButton`, `IconButton`, the agent's local `Tone`). There are four
  palettes per component (`plain`, `body`, `settings`, `editor`), and `::plain` means editor tokens
  for some components and Radix for others.
- **Screens that mix sets.**
  - The export panel is a settings page inside an editor card.
  - First-run setup stacks settings, editor and Radix styles in one view.
  - The settings window's title strip is Radix grey while the main header is the editor window
    colour.
  - The agent panel's composer paints with `gray(n)`.
- **No shared patterns for common roles.** Four hand-built list rows, six empty-state styles,
  section headers from 12 to 16 px, two progress bars, four kinds of divider, and three disabled
  opacities (0.45, 0.9, 0.7).

## Decision

**Cap's editor style is the only style.** Every window and screen (main window, settings window,
export, onboarding, agent panel, present mode) uses one palette, one set of metrics and one kit,
each component with one constructor. There are no `body`, `settings`, `plain` or `editor`
surfaces any more. The settings window becomes an editor-styled window: nav column on `window`,
pages on `card`, rows on `card_2`.

Cap's sizes stay as they are (control 28, select 30, input 32, field row 34; radii 7/6/8/12/9).
They are attached to components rather than picked per screen, so a screen never chooses a
radius or a height.

## Target architecture

```diagram
┌──────────────────────────────────────────────────────────────────┐
│ studio-theme                                                     │
│  Theme { appearance, colors: Colors, ... }   one Colors per mode │
│  METRICS (space, radius, size, layout)   TextStyle   Shadow      │
│  MOTION (fast, normal, spin, tooltip)    UiPrefs (theme, motion) │
└───────────────────────────────┬──────────────────────────────────┘
                                │ only roles, no raw steps
┌───────────────────────────────▼──────────────────────────────────┐
│ studio-ui/src/kit   one constructor per component                │
│  Button IconButton Toggle SegmentedControl Select Menu Slider    │
│  TextInput NumberField ColorPicker RadioCards Tooltip KbdChip    │
│  Badge StatusDot Notice Toast Card Popover Section Field ListRow │
│  EmptyState Divider ResizeGrip Progress Spinner Chip TabRail     │
│  SettingsFrame CollapsibleSection SelectableText                 │
└───────────────────────────────┬──────────────────────────────────┘
                                │ kit + roles only (style guard)
┌───────────────────────────────▼──────────────────────────────────┐
│ screens: shell · timeline · sidebar tabs · agent · export ·      │
│          settings window · onboarding · present                  │
└──────────────────────────────────────────────────────────────────┘
```

## Phases

| # | Phase | Effort | Status |
| --- | --- | --- | --- |
| 0 | [Baseline captures and the style guard](phase-00-baseline-and-guard.md) | 1.5d | pending |
| 1 | [Tokens: one palette, metrics, type, motion, UI prefs](phase-01-tokens.md) | 3d | pending |
| 2 | [Kit core: one constructor per component, merged buttons](phase-02-kit-core.md) | 3d | pending |
| 3 | [Kit additions: ListRow, EmptyState, Field, Section, Progress, ...](phase-03-kit-additions.md) | 2d | pending |
| 4 | [Shell: header, layout, player, transport, status, present](phase-04-shell.md) | 2d | pending |
| 5 | [Sidebar tabs: Project, Assets, Style, Props, Slides, Presets](phase-05-sidebar-tabs.md) | 2d | pending |
| 6 | [Agent panel](../261027-2015-agent-panel-components/plan.md) | 6d | pending |
| 7 | [Export, settings window, onboarding](phase-07-export-settings-onboarding.md) | 1.5d | pending |
| 8 | [Timeline](phase-08-timeline.md) | 0.5d | pending |
| 9 | [Docs, gallery completeness, guard at zero](phase-09-docs-and-lock.md) | 0.5d | pending |

Order matters for phases 0–3: tokens before kit, and kit before screens. Phases 4, 5, 6, 7 and 8
touch different files and can run in parallel once phase 3 lands. Each phase is one or more
reviewable commits that keep `cargo test`, `just clippy` and the app working.

## Rules every phase follows

1. **Ids are kept.** Every element id, probed bounds name, section probe, segment id
   (`{id}-{i}`) and `debug_selector` stays (lists in each phase file and in
   `shell/observation.rs:349-375`, `agent/mod.rs:808`).
2. **Before and after captures.** Each phase captures the screens it touches in light and dark,
   with the same fixtures, before and after (phase 0 builds the capture tool).
3. **The guard only goes down.** A phase may lower an allowlist count in the style guard, never
   raise it.
4. **No new values.** A value missing from the tokens is added to the theme with its role, never
   written at the call site.
5. **Behaviour unchanged.** Visual-only, except the listed fixes (e.g. the Slides review items).

## Success criteria

- `studio-theme` exposes only roles; the raw Radix steps are private (except the `syntax` set).
- No file under `studio-ui/src` outside `kit/` uses `gray(`, a Radix step, `rgb(`, `hsla(`,
  `gpui::black/white`, `text_size(`, `settings_*`, `body_*` or `SETTINGS_ACCENT`. Raw `px()` is
  limited to named constants in the timeline painters and the agent diff painter.
- Every kit component has exactly one constructor and appears in the gallery in every state, in
  light and dark.
- Captures of every screen in both appearances, reviewed next to Cap's editor.
- `docs/desktop/design-system.md` describes the system that exists, with no "deliberate
  differences" left except the documented additions (focus ring, notices, badges).

## Risks

- **Id drift breaks telemetry and the qualification tests.** Mitigation: rule 1, plus
  `x11_shell.rs` and the probe whitelist test run in every phase.
- **Settings-window look changes.** The nav, cards and accent shift from Cap's settings greys to
  editor roles. Intended, and called out in phase 7 captures.
- **Large mechanical diffs** (the palette rename touches about 300 lines). These go in their own
  commit with no behaviour change, so review is a skim.
- **Concurrent work in the tree.** Slides files are untracked; the parity suites are deleted.
  Each phase rebases on the current tree and does not restore or delete others' work.

## Related

- [Agent panel components](../261027-2015-agent-panel-components/plan.md) is phase 6. Its
  "Scale" table is superseded by phase 1's tokens where they differ.
- Audit sources: the theme and kit audit and the screen audit in thread
  T-01a17d2f-e5c9-759a-b059-2f3c8d1747d8.
