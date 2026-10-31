# Oleafly visual language → Studio

This is the reproduction reference for Studio's Oleafly-inspired UI. The reference was
extracted from `C:\Users\Administrator\code\Oleafly`, especially `src/styles/globals.css`,
`src/components/ui`, `src/components/layout`, `src/lib/theme-customization.ts` and
`src/store/settings.ts`. Values describe the shipped defaults, without saved customization,
at a 16 px root size. They are source-derived, not measurements of a customized running app.

Studio's implementation lives in `desktop/crates/studio-theme` and
`desktop/crates/studio-ui/src/kit`. See [the implementation guide](design-system.md) for
component APIs, focus behavior, the style guard and gallery capture commands.

## Visual character

- Quiet, nearly achromatic surfaces; the document remains the brightest light-mode plane.
- Blue is reserved for primary actions, selected controls and keyboard focus, not decoration.
- Flat persistent panes with fine separators; elevation belongs to temporary overlays.
- Rounded controls, compact Lucide outline icons, consistent spacing and restrained type.
- Selection, hover, focus, disabled and error states must remain distinguishable.
- Reproduce this vocabulary, not the leaf logo, wordmark, mascot or research-specific copy.

## Colors

Oleafly defines neutral colors in OKLCH. `N(x)` below means `oklch(x 0 0)`.
The hex values are Studio's rounded sRGB translations, not original CSS literals.

| Semantic role | Oleafly light | Oleafly dark | Studio light / dark |
| --- | --- | --- | --- |
| Document / background | N(1) | N(.145) | `#ffffff` / `#0a0a0a` |
| Foreground | N(.145) | N(.985) | `#0a0a0a` / `#fafafa` |
| Card | N(.985) | N(.205) | `#fafafa` / `#171717` |
| Sidebar | N(.975) | N(.205) | `#f7f7f7` / `#171717` |
| Muted / secondary / accent surface | N(.95) | N(.269) | `#f1f1f1` / `#262626` |
| Muted foreground | N(.556) | N(.708) | `#6b6b6b` / `#a1a1a1` (secondary text) |
| Popover | N(.985) | `#121212` | `#fafafa` / `#121212` |
| Border | N(.9) | white at 10% | `#dedede` / `#ffffff1a` |
| Input border | N(.9) | white at 15% | Studio uses stronger `line_strong`: `#a1a1a1` / `#ffffff26` |
| Primary | runtime `#2563eb` | runtime `#2563eb` | same, white text |

The neutral `primary` declared in Oleafly's CSS is a fallback: runtime theme customization
sets blue with white text in both appearances unless a saved accent/custom primary overrides
it. Available presets are blue `#2563eb`, green `#0b8842`, purple `#7c3aed`, rose `#db2777`,
orange `#ea580c` and teal `#0d9488`. Studio adopts blue, not a new accent-picker feature.

Studio uses translucent control fills (black 6/10/14 % in light, white 8/12/16 % in dark
for rest/hover/pressed) so controls show on every surface, with `#1d4ed8` for primary
hover. These are native state adaptations, not exact copies of Oleafly's opacity
compositing. Light secondary text is
slightly darker than Oleafly's approximate `#737373` to meet contrast on nested surfaces.
Status, code-syntax, timeline lane and red playhead colors retain their Studio meanings.

Oleafly can use a translucent shell (background at 32%) and a rail mixed from 30% muted
plus background; unsupported environments force solid surfaces. Studio intentionally uses
solid native panes, with no acrylic/vibrancy dependency.

## Spacing, sizing and shape

Oleafly uses Tailwind's standard spacing utilities, not a separately named spacing-token
collection. Reproduce the 4 px rhythm, with half steps when needed:

| Utility | .5 | 1 | 1.5 | 2 | 2.5 | 3 | 4 | 5 | 6 | 8 | 9 | 10 | 12 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Pixels | 2 | 4 | 6 | 8 | 10 | 12 | 16 | 20 | 24 | 32 | 36 | 40 | 48 |

- Radius base: 10 px; small 6, medium 8, large 10, extra-large 14.
- Typical control gap: 8 px; dense toolbar gap: 4 px.
- Border width: 0.5 px; explicit `h-px` separators remain 1 px in Oleafly.
- Section spacing: 24 px; overlay content: 24 px padding and 16 px gap.
- Oleafly toolbar min-height: 48 px; native Studio retains its 52 px header and editor splits.
- Oleafly scrollbars: 6 px; transparent until hover; rounded muted-foreground thumb at
  42%, then 62% on thumb hover. Studio's native scrolling behavior is unchanged.

## Typography

Oleafly declares `Geist, Inter, ui-sans-serif, system-ui, sans-serif` and
`Geist Mono, JetBrains Mono, ui-monospace, monospace`, with locale-specific CJK fallbacks.
It does **not** ship a font-loading implementation in the inspected application: actual
typeface depends on installed fonts. Studio bundles Geist regular/medium and Geist Mono
regular and registers them before opening windows, so the chosen families are reproducible.
Their OFL license is alongside the fonts in `desktop/app/assets/fonts/LICENSE.txt`.

| Usage | Oleafly rule | Studio style |
| --- | --- | --- |
| Body / ordinary controls | 14 px | `Body` 14/20, regular |
| Compact control | 12 px | `Small` 12/16; `Label` 12/16 medium |
| Sidebar section label | 11 px semibold, uppercase, .08em tracking | Reference only; existing Studio section hierarchy retained |
| Dialog title | 18 px semibold | `Heading` 18/24 |
| Title | Context-dependent | `Title` 16/24 medium |
| Start-screen heading | Context-dependent | `Display` 24/32 semibold |
| Code | Mono | `Code` 12/18; `CodeSmall` 11/16 |
| Tiny annotation | Context-dependent | `Caption` 10/14 medium |

## Components and states

### Buttons

| Oleafly size | Height | Horizontal padding | Type | Studio equivalent |
| --- | --- | --- | --- | --- |
| Default | 36 | 16 | 14 medium | Reference only (Oleafly's text fields keep 36) |
| Small | 32 | 12 | 12 medium | `ButtonSize::Md`, Oleafly's most used size |
| Extra-small | 24 | 8 | 11 medium | `ButtonSize::Sm` (12 medium in Studio) |
| Large | 40 | 32 | 14 medium | Reference only |
| Icon | 36 square | — | 16 px glyph | Studio toolbar uses 28 square / 16 glyph |

Medium radius (8 px), centered content, 8 px gap, no text wrapping, disabled opacity 50%.
Oleafly's toolbar specifically overrides the icon box to 28 px; Studio retains existing
24/28/32 px icon-button options. Default/primary is blue, secondary is muted, ghost is
transparent until hover, outline uses an input border. Oleafly also has link and ghost-primary
variants; Studio keeps its existing `Primary`, `Neutral`, `Ghost` and soft `Danger` APIs.
No new variants are necessary just to restyle existing actions.

### Fields

Inputs/selects are 36 px high, medium radius, document background, outlined border,
12 px horizontal padding, 14 px type and muted placeholder. Oleafly uses 8 px vertical
padding and a 16 px chevron at 50% opacity. Textarea minimum height is 80 px.
Studio applies the height, padding, type and outline to its shared input/select, keeping
IME, selection, validation, number editing and keyboard contracts intact.

### Navigation and tabs

Oleafly nav items: 14 px text, 16 px icon, 10 px gap, 10×8 px padding, 8 px radius.
Active rows use foreground text, medium weight, document fill and a border; inactive rows
use muted text and a transparent border. Studio's settings nav uses this treatment with
40 px rows. Its six editor tabs and their shortcuts remain intact.

Oleafly tabs: muted rounded container, 36 px high with 4 px inset (small: 32 px / 2 px);
active tab uses document fill and a small shadow. A toolbar segmented selector is different:
muted/40 container, 2 px inset, 28 px options, active primary/10 fill with foreground text.
Studio retains its segmented-control geometry and behavior rather than treating every
editor tab as one of Oleafly's toolbar selectors.

### Menus, overlays and tooltip

| Component | Oleafly extraction |
| --- | --- |
| Dropdown | Min-width 160 px, 4 px inset, radius 8, popover fill, hairline, shadow-xl, trigger gap 4 |
| Menu row | Padding 8×6 px, gap 8, type 14, radius 6, accent fill on focus |
| Context menu | Min-width 128 px, shadow-md; otherwise similar row vocabulary |
| Select popup | Min-width 128 px, max-height 384 px, 4 px viewport inset, shadow-md |
| Generic popover | Card fill, min-width 168 px, inset 4, radius 8, shadow-xl |
| Dialog | Centered, viewport minus 32 px, max-width-lg, padding 24, gap 16, radius 14, shadow-2xl |
| Backdrop | Black 55% + blur-sm (custom ModalShell: black 50%) |
| Dark dialog | Global override to `#121212` |
| Tooltip | Type 12, padding 8×4, radius 8, popover fill, hairline, shadow-md |

Tooltip behavior: 300 ms hover delay; immediate on keyboard focus; 6 px trigger gap;
8 px viewport clearance; ordinary max-width 260 px. Studio adopts the neutral tooltip
colors, dimensions and delay, preserving its existing native placement logic. Its menus
use 32 px rows, 14 px type and the popover role. Native popover shadows are deliberately
subtle (4 px offset, 12 px blur) rather than an attempted CSS box-shadow emulation.

### Icons

Lucide React dominates Oleafly; Studio already embeds the equivalent Lucide SVG vocabulary.
Use a 24-unit viewbox, stroke 2, rounded caps and joins, currentColor, no decorative fill.
Default glyph 16 px; small section actions 14 px; disclosure 12 px. Oleafly's file-icon
interiors use stroke 1.75. Its selected badge is a 16 px primary circle containing a white
10 px check at stroke 3.5; this is a distinct component, not the ordinary check icon.
Keep Studio's application mark and native caption glyphs; do not replace them with leaf branding.

### Motion and accessibility

Oleafly controls transition colors; local generic controls do not specify a duration.
Dialogs fade/scale from 95% to 100% over 200 ms; menus/popovers use fade/95%-zoom utilities.
Some exits differ (dropdown closes immediately). Reduced-motion support exists for the
custom modal and special shimmers. Studio keeps its motion preference and uses 150 ms
fast / 200 ms normal / 320 ms slow, with 300 ms tooltip delay.

**Do not copy Oleafly's global suppression of outlines and ring shadows.** Studio keeps
its visible 2 px keyboard focus ring, Enter/Space activation and disabled tab-stop skipping.
This is a deliberate accessibility difference, not a fidelity defect.

## Applying the system consistently

1. Resolve `Theme::for_window` once per render; use semantic color roles, never local hexes.
2. Use `METRICS` and `TextStyle`, and shared `kit` controls rather than bespoke styled divs.
3. Persistent editor panes use `Card::panel`; content groups use raised/nested cards;
   temporary overlays use `Popover` or `Menu`.
4. Reserve the primary emphasis for the main action. Status colors communicate status,
   and lane colors communicate video content types.
5. Verify both appearances, disabled/selected/error states and the narrow sidebar. Use the
   native gallery, then inspect actual captures; a successful build alone is not visual review.

This migration changes shared tokens and controls across Studio; it does not transplant
Oleafly's file-management/research workflows, create new theme customization settings,
or redesign Studio's video-editor information architecture.

## Agent panel: component-specific extraction

This section is a source-based study, not a claim of runtime screenshot parity. Agent-panel
changes below are **reproduction guidance, not implemented Studio behavior**. The global
theme migration above does not by itself reproduce these component-specific treatments.
Source paths below are relative to Oleafly's `src/components/ai`, except where
`styles/globals.css` explicitly refers to `src/styles/globals.css`.

### Which Oleafly panel is the reference?

`ChatPanel.tsx` mounts `ResearchAssistant.tsx`, which switches between the built-in agent
(`ChatCore`) and CLI/ACP agents (`acp/AcpWorkspaceAssistant`). Both use the shared
`MessageList.tsx` and `chat-parts.tsx` transcript components. ACP is the closest reference
for Studio's external-agent workflow. Do not mistake built-in-only research controls for
controls that every agent has.

### Panel composition and spacing

The ACP panel is a full-height flex column on `sidebar`, not a stack of independent cards:

```text
Shared assistant header: runtime switch + utility actions
Session header (when a non-delegated session exists)
Scrollable transcript: flexible remaining height
Status / reconnect / pending questions and permissions
Agent picker (only before the conversation has messages)
Context and attachment chips
Bottom composer: input + compact controls
```

- Transcript inset: **16 px horizontally, 12 px vertically**.
- Message-row separation: **12 px**; within a message's activity/content stack: **6 px**.
- Composer wrapper: **12 px** on sides/bottom, **4 px** at top.
- Pending questions/permissions sit **outside the transcript scroll area**, above the
  composer, with a top separator, **12 px padding**, **8 px card spacing**, and their own
  scroll region capped at **60vh**. This keeps a blocking request reachable.
- Error/remote banners use **12 px horizontal / 8 px vertical outer margins**; remote
  status is a muted/40 bordered surface with 8 px inset and 12 px text.
- Following the stream is conditional: ACP regards the reader as near the bottom within
  **80 px**. New output must not forcibly drag a reader away from earlier messages.

Sources: `acp/AcpWorkspaceAssistant.tsx:398–565, 586–632`;
`MessageList.tsx:30–36, 143–155`.

### Conversation: the two voices

| Element | Extracted visual rule |
| --- | --- |
| User message | Right aligned; max **85%** of row; **primary blue with white text** |
| Assistant answer | Left aligned; **full available row width**; light: background fill with border/60; dark: muted fill, transparent border |
| Both bubbles | `rounded-lg` (**10 px**), padding **12×8 px**, **14 px** text; selectable, wrapping content |
| Bubble/footer gap | **2 px** |
| Timestamp | **10 px**, muted, tabular numerals |
| Copy | **14 px** glyph, **6 px padding**, appears on message hover; green check after copying |
| Edit/regenerate | **24 px** icon button / **14 px** glyph; hover or keyboard-focus reveal; retry remains visible |
| Attachment label | Bordered muted/60 chip, radius 8, **6×2 px** padding, **11 px** type, **12 px** paperclip; name truncates at 140 px |

There is no universal avatar column. Identity is conveyed by alignment, surface and
activity-specific icons. Message actions are conditional on actual capabilities; ACP's
current `MessageList` call does not supply the generic edit/regenerate handler.

Sources: `chat-parts.tsx:551–563, 1278–1323, 1356–1461, 1500–1580`.

### Thinking and tool activity: quiet first, detail on demand

- Reasoning is **collapsed by default**, max-width **85%**, with a **14 px Brain** glyph,
  **8 px gap**, **4 px vertical header padding** and muted text. When closed, hovering
  substitutes a right chevron; open uses a down chevron. A running label shimmers;
  completion can show the elapsed duration. Empty completed reasoning is hidden.
- Expanded reasoning: plain pre-wrapped text, **11 px** relaxed type, a left hairline,
  **7 px left margin + 12 px inner inset**, **224 px maximum height**, internal scroll.
  Although an older nearby comment mentions Markdown, the actual rendered body is plain text.
- The ACP fallback Thinking row disappears while a live reasoning block already supplies
  the indicator: **one active thinking cue**, not duplicated spinners.
- A normal tool is a **borderless activity row**, not a heavy card. Max-width **85%**;
  14 px tool glyph and 8 px gap; title 14 px, foreground/90; command titles **12 px mono**.
  Trailing status is a spinner, **12 px green check**, muted stop, or destructive cross.
- Tool output is collapsed by default. Expansion follows the same left rule and inset;
  output uses **10 px mono**, muted/60 fill, radius 8, **10×8 px** padding, max-height
  **320 px**. Long output initially shows up to 4,000 characters with an explicit show-all action.
- Consecutive exploration tools group into a muted **12 px summary**, with **12 px chevron**,
  **6 px gap**, **4×2 px** padding. Expanded rows have **6 px spacing**, **10 px inner left
  inset**, and a 150 ms fade (disabled for reduced motion).
- Finished activity can fold into a “worked” summary **only when it has no separately
  visible outcome or subagents**. Preserve meaningful results instead of hiding every
  completed tool indiscriminately.

Sources: `chat-parts.tsx:617–655, 849–924, 1122–1163, 1534–1556`;
`activity/ResearchToolCard.tsx:36–67, 134–174, 242–312`;
`acp/AcpWorkspaceAssistant.tsx:635–665`.

### Approvals, questions and changes are different surfaces

**Permission request:** `PermissionCard` uses `AiChrome` with a **2 px animated spectrum
border**, outer radius **14 px**, inner radius **10 px**, shadow-lg, and **14 px content
padding**. Its icon is a **20 px ShieldQuestion** in a 28 px slot; heading **14 px semibold**,
request description **13 px** muted, content gap **12 px**. Buttons are 32 px high, wrap,
align right, and sit after a separator with 10 px top padding / 8 px gaps. Dismiss is ghost;
**the first agent-supplied option is primary**, the rest outline—not a guarantee that
“Allow” is always the first option. Busy disables choices; expired replaces them with text.

This AI prompt has local semantic colors beyond the global palette: light **#fafafa**
surface, **#666666** muted text, **#d4d4d4** borders; dark **#171717** surface,
**#f5f5f5** text, **#a3a3a3** muted text, **#404040** borders. The permission border runs a
14 s background-position animation over a spectrum including teal, purple, pink, orange,
green and white; reduced motion freezes it. Ordinary tool rows do **not** get this chrome.

**Question/elicitation:** separate neutral `card` surface, radius **10 px**, hairline,
**12 px padding**, **8 px vertical spacing**, **12 px** type; compact fields override
defaults to **28 px high / 12 px text**. Supports required fields, inline validation and
distinct URL/waiting states. Do not restyle every question as a permission prompt.

**Proposed changes:** mono **11 px** filenames, **14 px FileDiff** glyph, 8 px gap;
“Show change” expands one proposed diff at a time.

**Finished-turn changes:** a quiet bordered **muted/40** surface, radius **8 px**, **10×6 px**
padding, **11 px** type; header has file count, green/red line counts and compact
**24 px** review/undo/commit controls. Full file review expands below it. These affordances
describe Oleafly's workflow, not permission to add commit or undo semantics to Studio.

Sources: `acp/PermissionCard.tsx:42–110`; `AiChrome.tsx:25–51`;
`styles/globals.css:331–365, 1469–1498, 1653–1664`;
`acp/ElicitationCard.tsx:146–176, 268–335`; `acp/ChangeList.tsx:35–76, 82–89`;
`turns/TurnChangesCard.tsx:581–638`.

### Composer: the strongest agent-specific shape

Both built-in and ACP paths use `composer/ChatComposer`; `acp/AcpComposer` configures
that shared component rather than defining a second visual shell. The composer is a
**plain bordered card, not a gradient AI frame**.

| Part | Extracted rule |
| --- | --- |
| Shell | **22 px radius**, card fill, hairline, shadow-sm; padding **12 px sides / 10 px top / 8 px bottom**; focus-within changes border to ring |
| Text area | **14 px**, transparent, no border, 2 px horizontal padding; retains base 8 px vertical padding; no native resize grip |
| Resting input height | **56 px** by default; minimum 3.5rem |
| Automatic growth | Up to **max(resting height, 224 px)**, then scrolls |
| Manual resizing | Resting height capped at **60% of nearest composer panel**, subject to minimum; this does not cap automatic growth below 224 px |
| Resize affordance | 8 px high hit area, inset 24 px from sides, positioned 4 px above shell; centered **32×4 px** grip, visible on hover/focus; upward drag grows, double-click resets |
| Controls row | 8 px top margin, **28 px minimum height**, nowrap, 2 px gap between left/right groups |
| Left controls | **4 px gaps**, horizontally scrollable with scrollbar hidden |
| Right controls | Fixed/nonshrinking, right aligned, **4 px gaps** |
| Submit | **32 px circular** primary, ArrowUp glyph, white foreground; disabled opacity 40% |
| Stop | Replaces submit in the same circle, **14 px filled Square** |

ACP controls, left to right:

```text
[Start + agent label OR session status] [Paperclip]  …  [Model] [Effort?] [Mode?] [More?] [Usage] [↑ / ■]
```

Model/effort/mode triggers are **28 px high**, transparent, borderless, **12 px medium**
type, 8 px horizontal padding, up to 176 px wide. Unknown/unselectable model information
is a label, not a fake dropdown. More uses a **28 px circular SlidersHorizontal** control.
Agent configuration locks while running/pending/not-ready; this does not disable drafting
or attachment selection.

The session pill is **24 px high**, max-width 176 px, **8 px horizontal padding**, **6 px
gap**, **10 px** type and a **6 px dot**. Ready shows agent name plus green dot; other
states include status text. Connecting can use a 10 px spinner; motion is preference-aware.

| ACP state | Visible action and input behavior |
| --- | --- |
| Usable agent, no session | ArrowUp starts with the draft |
| Ready | ArrowUp sends; a send already in flight makes the next draft queue |
| Connecting | ArrowUp can queue |
| Running | **Stop is visible**, while **Enter queues**; there is no separate visible Queue button |
| Cancelling | Stop remains disabled; Enter can still queue |
| Unusable/auth-required/disconnected/failed | Sending blocked, but editing/attaching remains possible |

Shift+Enter inserts a newline. Sending/queueing requires nonblank text or a ready
attachment and no attachment still processing. These are extracted Oleafly semantics;
Studio must preserve its own adapter-advertised queue/steer capabilities.

**Chips stay above the shell:** queue rows first, then skill/selection chips, then the
shared attachment strip. Queue rows use card fill, radius 10, **8×4 px padding**, **12 px**
type, Clock glyph and a truncated first-line preview; failures expose Retry/Edit.
Skill and quote chips are outline extra-small controls. Attachments use wrapping **8 px
gaps**, muted/50 fill, radius 8, **12 px** name / **10 px** metadata, **24 px image thumbnail**
or 16 px file icon, with a **24 px remove target**. Processing/error states are explicit.

At a composer-container width of **384 px or less**, shared gaps reduce to **2 px**,
attachment/usage targets to **24 px**, and submit to **28 px**. Do not assume built-in
model/persona/plan label-collapse rules apply to ACP: its selectors lack those wrappers.
Built-in Plan, Web, Persona, Prompts and approval controls are not part of ACP's control set.

Sources: `composer/ChatComposer.tsx:63–75, 175–202, 443–473, 515–601`;
`composer/composer-input.ts:8–27, 48–66`; `composer/ComposerResizeHandle.tsx:108–134`;
`use-auto-size-textarea.ts:16–63`; `acp/AcpComposer.tsx:193–258`;
`acp/acp-composer.ts:7–28`; `acp/AcpControls.tsx:27–32, 318–355`;
`acp/AcpComposerChips.tsx:12–145`; `AttachmentChips.tsx:18–114`;
`acp/AcpWorkspaceAssistant.tsx:667–739`; `styles/globals.css:1588–1649`.

### Header, history and empty state

- Shared assistant header: **36 px high**, bottom hairline, **8 px horizontal padding**,
  6 px gap. Runtime switch sits left; utility icons sit right with 2 px gaps. ACP order:
  **Plus, History, Settings2, BarChart3, PictureInPicture2, PanelRightClose**. Targets are
  generally **28 px**, glyphs **16 px** (float/hide 14 px).
- Runtime switch: muted segmented surface, 2 px inset; text-only **12 px** options with
  **8×4 px** padding. Active uses background fill, foreground and a small shadow.
- ACP session strip: separate bottom-bordered row, **12×6 px padding**, **12 px** type,
  6 px gap; flexible truncated title, optional revision/protocol/remote badges, **24 px**
  More target. Rename is inline; title action/input height **24 px**. Fork/sign-out/delete
  availability depends on session capabilities and busy state.
- ACP History first opens a **288 px wide / max 288 px high** scrolling menu with up to
  20 recent conversations and an all-history action. Full history is a centered dialog,
  max-width-lg, inner **70vh** height cap, **16 px padding / 12 px gap**, **32 px** search
  field and compact title/agent/time rows. Built-in history opens its dialog directly.
- Empty home uses a centered column, max-width **672 px**, **20 px gaps**, **24 px vertical
  padding**, **24 px semibold heading**, **12 px** subtitle. ACP shows a **64 px** rounded
  agent tile with **30 px** mark, not the built-in mascot. Starting replaces home with a
  compact connecting state: **48 px** tile, spinner, label, hint and optional Cancel.
- Agent picker appears only before messages: full-pill muted/60 rail, 4 px inset, 2 px gaps;
  selected agent is **36 px high**, logo plus label and raised background; alternatives
  are **36 px icon-only squares**, glyphs **18 px**. Rail scrolls horizontally.
- ACP does not mount the built-in home’s RecentChats section. Do not invent model,
  history, skill or fork controls when Studio has no corresponding capability.

Sources: `AssistantShellHeader.tsx:29–80`; `RuntimeSwitch.tsx:25–48`;
`AssistantShellAcpActions.tsx:134–240`; `acp/AcpSessionHeader.tsx:49–138`;
`acp/AcpHistoryDialog.tsx:303–420`; `home/AssistantHome.tsx:158–298`;
`home/AgentPickerRow.tsx:25–64`; `acp/AcpWorkspaceAssistant.tsx:297–390, 541–550`.

### What this means for Studio

Studio already has the right architectural pieces: `agent/ui/message.rs`,
`agent/ui/activity.rs`, `agent/ui/action_card.rs`, `agent/ui/changes_card.rs`, and
`agent/composer`. Its 85%-width user bubble, expandable activity and pinned composer are
useful foundations. However, the current user bubble is **neutral**, the assistant is
**unframed**, and activity/footer row heights follow the shared **32 px** small-control
metric. Matching Oleafly requires agent-specific visual treatment, not another global
increase to button size or padding.

For a subsequent implementation, prioritize:

1. Blue user bubbles and full-width, lightly framed assistant answers.
2. Dense, borderless activity summaries with indented expandable output; keep status colors.
3. Dedicated compact agent controls and metadata rather than 32/36 px buttons everywhere.
4. Separate permission, question and completed-change visual hierarchies.
5. Preserve Studio's actual send/queue/steer/stop, permission and apply/reject semantics;
   visual correspondence must not introduce unsupported backend capabilities.
6. Use the 22 px composer shell and compact controls as the visual reference, while
   retaining Studio's native text input, IME, focus and resizing contracts.

The earlier global theme changes remain local and uncommitted. This agent study adds
documentation only; it does not change the agent panel implementation.
