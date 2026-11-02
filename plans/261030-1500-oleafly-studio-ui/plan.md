---
title: "Oleafly to fframes Studio: native visual migration"
description: "Finish the existing Oleafly foundation and migrate every Studio surface, preserving native editor and agent behavior."
status: pending
priority: P1
effort: "28-45 person-days including contingency"
branch: main
tags: [design, desktop, studio, ui, refactor]
blockedBy: []
blocks: []
created: 2026-10-30
---

# Oleafly to fframes Studio

## Overview

Adopt Oleafly's neutral surfaces, its typography (Geist only if a full-coverage build passes D1), compact Lucide controls and ACP conversation vocabulary across the **whole native GPUI Studio UI**. Finish and validate the locally implemented foundation rather than rebuild it. Then migrate the remaining screens in small sequential slices, each with real-window evidence and preserved behavior. Size: **XL**, 23-35.5 person-days of execution plus 20-25% contingency (28-45 total, rounded), approximately 6-9 working weeks for one owner, excluding provider/SDK availability waits.

This is a new, planning-only deliverable. No UI, existing source, existing docs, older plan status, active-plan pin, commit, push or publication is authorized by this planning session. Markdown checklists are authoritative: there is no live task-management tool. Automatic journal omitted because it would create files outside the requested plan. Implementation is a separately authorized next activity.

## Scope HOLD and precedence

- Keep Studio branding, six-tab video-editor architecture, project/data formats, adapter capabilities, caller ids, telemetry bounds, shortcuts, native focus, selection, IME and file dialogs.
- Use `studio-theme`, `METRICS`, `TextStyle` and the current `kit`. No React/web rewrite, Oleafly leaf/mascot/copy, new backend features, accent customization, acrylic dependency, universal abstraction or new component solely to imitate an unsupported action.
- This plan supersedes **Cap/Zeron visual guidance only** in `plans/261027-2045-studio-design-system` and `plans/261027-2015-agent-panel-components`. Their behavior and useful tests remain; pending metadata is stale, not evidence work is missing. Neither older plan is modified or made active.
- Visual authority: [Oleafly extraction](../../docs/desktop/oleafly-design-system.md), fully read; [native implementation guide](../../docs/desktop/design-system.md), fully read. ACP, not built-in research ChatCore, is the agent reference. Reference checkout `C:\Users\Administrator\code\Oleafly`, HEAD **65aa177220c33cba50769a14f1ff705d7cfe373e**, reconfirmed during planning. Source-derived defaults assume 16px CSS root, not customized runtime screenshots or a promise of pixel-identical native rendering.
- Native correctness wins over literal web imitation. Preserve Studio's permission ordering/emphasis, both Send and Stop when supported, queue/steer decisions and 70px follow threshold. Record each deliberate deviation in phase evidence.

## Starting state and safety

Palette, metrics, text styles, font files, buttons, inputs, selects, menus, tooltips and persistent pane surfaces are **already locally implemented and uncommitted, with the defects F1-F6 below**. The fonts are registered but not applied, and they are a Latin-1 subset. Agent-specific reproduction is not. `agent/ui/message.rs` still uses a neutral user bubble and unframed assistant; `agent/ui/mod.rs` still derives activity/footer height from global small-control 32px. `composer/metrics.rs` still caps input at 8×20=160px and attachment thumbnails at 56px.

`agent/cards.rs` and `agent/question_card.rs` each contain another person's one-line replacement. Root/fixture Cargo.lock changes and existing untracked files belong to others. At each execution start, inspect `git status --short` and file diffs; preserve them and obtain ownership before touching overlapping hunks. Never reset, restore, stash, clean, regenerate lockfiles or mass-format the checkout. Use `--locked`. Rollback is a reverse of the executor's own reviewed hunks only, never an entire dirty file.

## Second-pass research findings (2026-10-30, verified against source and captures)

The local foundation is **partly** done, not done. Phase 02 starts from these verified defects instead of a general audit. Evidence: `.amp\in\artifacts\oleafly-research-current\{light,dark}` (88 PNGs from the dirty checkout, 150% scale) and gate T run today: `studio-theme` 28, `studio-ui` 517 and `style_guard` 3 tests passed.

| # | Finding | Evidence | Consequence for this plan |
|---|---|---|---|
| F1 | **Geist is registered but never used for UI text.** `init_fonts` loads it (`desktop\app\src\assets.rs:81`, called from `main.rs:165` and the gallery), but no view applies `UI_FONT`; only `MONO_FAMILY` is set (markdown, diffs). Interface text still renders in GPUI's default system font (Segoe UI on Windows). | Search for `UI_FONT` and `font_family(` across `studio-ui`, `studio-theme` and the app; captures show Segoe UI shapes. | Phase 02 must apply `UI_FONT` once per window root (main shell, settings window, gallery), not per view, or drop the bundle (see F2). |
| F2 | **The bundled fonts are a 225-codepoint Latin-1 subset**, all three files (they match `landing\studio\fonts\geist-latin-*.woff2`). Missing: Vietnamese precomposed letters (ầ ơ ư đ ỹ …), Latin Extended (ł ş ž ő), Cyrillic, Greek and → (used in `onboarding.rs:68` and `project_tab.rs:19`). Combining marks are present. | `plans\261030-1500-oleafly-studio-ui\cmap_probe.py` run over `desktop\app\assets\fonts\*.ttf`. | Words such as "Trần" would mix Geist and fallback glyphs. Oleafly itself **does not bundle Geist** (no font files or `@font-face` in its source); it names `"Geist", "Inter", system-ui` and falls back. Decision D1 below. |
| F3 | **`card_2` equals `ctl` in both themes** (light `#f1f1f1`, dark `#262626`). Neutral buttons, segmented troughs, badges, chips, key chips and number-field fills have no visible fill on nested cards. | `research-current\dark\settings-general.png`: "Run setup again", "Check now" and the Theme/Motion troughs look like plain text. The palette test was rewritten to require opaque `ctl`, which contradicts `design-system.md` ("translucent, so they work on any surface"). | Phase 02 restores a translucent `ctl`/`ctl_hover`/`ctl_active` (or an opaque value distinct from every surface it sits on) and adds a test that composites each control fill over `window`, `card`, `card_2`, `sidebar` and `popover` and requires a visible step. |
| F4 | **Controls are taller than Oleafly's.** The foundation mapped Studio `Md` to Oleafly's `default` (36 px) and `Sm` to 32. Oleafly's buttons mostly use `size="sm"` (32 px, 545 uses) and `xs` (24 px, 155 uses); its workspace chrome is mostly `h-7` (28 px). Its text fields really are mostly 36 px (`h-9`). | Counts over `Oleafly\src\components\**\*.tsx`, excluding tests. | Decision D2 below. |
| F5 | **Width breakpoints were measured for the old sizes.** `METRICS.layout.actions_w` 680 and `actions_closed_w` 300 (header), and the transport comment's 566/333/214 px, assume 28 px buttons with the old padding. With 36 px buttons, 16 px label padding and 14 px text, the open-project header actions measure about 725 px in `research-current\light\shell-export.png` (approximate, read from the capture). | `metrics.rs:313-322`, `transport.rs:288-297`, `header.rs:227-238`. | Phase 04 re-measures from rendered bounds after D2 and updates the constants and their tests; the current numbers can't be treated as "as implemented". |
| F6 | Smaller gaps: `Small` and `Label` are now both 12 px (they differ only in weight); the transport clock uses a literal `font_family("monospace")` (`transport.rs:61`) that the style guard does not catch; the Cap-era palette and type tests were renamed and rewritten rather than kept as regression checks. | `typography.rs`, `transport.rs`, the `lib.rs` diff. | Phase 02 decides whether `Small` stays 12, and switches the clock to `TextStyle::Code` or `MONO_FAMILY`. |

The agent facts this plan relies on were rechecked and hold: input cap 8 × 20 = 160 px (`composer\metrics.rs:14-16`), 56 px thumbnails (`:31`), follow threshold 70 and jump 320 (`transcript\scroll.rs:10-14`). The Oleafly references were rechecked at HEAD `65aa177…`:
- `AssistantShellHeader.tsx:43` is `h-9 gap-1.5 border-b px-2`.
- `ChatComposer.tsx:458` is `rounded-[1.375rem] border bg-card px-3 pb-2 pt-2.5 shadow-sm focus-within:border-ring`. The composer is **not** a plain card: it has a small shadow, and focus changes its border.
- The submit and stop buttons are `size-8 rounded-full`.
- `composer-input.ts:12` grows to 224 px, capped at 60 % of the panel.

### Decisions taken in this revision (veto before phase 02)

- **D1, font.** Bundle Geist only if a full upstream Geist / Geist Mono build (from `vercel/geist-font`, OFL) passes `cmap_probe.py` for Vietnamese, Latin Extended and arrows. Then apply `UI_FONT` at the window roots and check the fallback rendering of CJK and emoji. If no such build is available, remove the unused Geist UI faces and keep the system UI font, as Oleafly effectively does on machines without Geist; keep Geist Mono only if it passes the same probe. A Latin-1 subset is not acceptable.
- **D2, density: map Studio's sizes onto Oleafly's real usage, not its default.**

  | Control | Height | Padding | Text |
  |---|---|---|---|
  | `ButtonSize::Md` (Oleafly `sm`) | 32 px | 12 px | `Label` 12 medium |
  | `ButtonSize::Sm` (Oleafly `xs`) | 24 px | 8 px | 11 medium |
  | Icon buttons | 24/28/32 px, unchanged; 28 is the toolbar size | | |
  | `TextInput`, `Select`, `NumberField` | 36 px (Oleafly `h-9`) | | |
  | List rows | 32 px | | |

  No third button size is added. Headers and toolbars keep icon-first 28 px controls. This lowers the risk in F5, but the breakpoints still have to be re-measured.
- **D3, baseline.** A clean pre-foundation baseline already exists for light: `.amp\in\artifacts\before\light` (44 PNGs, captured 20:38, before `palette.rs` changed at 21:23). For dark, phase 01 builds a `git worktree` at `HEAD` with its own `CARGO_TARGET_DIR`, outside this dirty checkout, and captures it with `--appearance dark`. The dirty checkout is never stashed or reset to get a baseline.

### Execution log

- **Approval and execution (2026-10-30):** the owner approved D1-D3 ("use a font like Geist") and asked for parallel subagents. Phases now run in waves of subagents on disjoint files instead of one serial owner. Shared files (`studio-theme`, the gallery root `ui_gallery.rs`, docs) stay with the coordinating thread.
- **Phase 01, baseline:** pre-foundation captures exist for both themes in `.amp\in\artifacts\before\{light,dark}`, 44 PNGs each. The dark set was captured from a temporary `HEAD` worktree, which has been removed.
- **Phase 02, foundation (done):**
  - **D1:** Geist 1.9.0 and Geist Mono 1.9.0 full builds (release zip SHA-256 `1F2DD9EA…A58C`, OFL), with Regular, Medium, SemiBold and Mono Regular bundled. `UI_FONT` is applied at the shell, settings window, gallery and tooltip roots, and `bundled_fonts_cover_vietnamese_and_extended_scripts` guards coverage.
  - **F3:** `ctl`/`ctl_hover`/`ctl_active` are translucent again (black 6/10/14 %, white 8/12/16 %), guarded by `control_fills_show_on_every_surface`.
  - **D2:** buttons are Md 32 / Sm 24 with `Label` text. Sm uses 12 medium, not 11, so no new text style was needed.
  - **F6:** the transport clock uses `MONO_FAMILY`.
  - Gates T and A pass. Captures are in `oleafly-migration-02-run01`.
  - Known follow-up: the clock clips with Geist Mono; this goes to phase 04.
- **Wave 1 (phases 03, 04, 05, 10) done:** kit families and six glyphs, flat shell with re-measured breakpoints (actions 677/258, transport 1270/830/520) and the clock fix, editor panels, settings/onboarding/create/export. Captures `oleafly-migration-0{3,4,5}-run*`, `oleafly-migration-10-run01`; native Windows shell screenshots in `oleafly-migration-04-run01\native`.
- **Theme tokens added for wave 1 requests:** `info_bg` = accent at 10 %, `TextStyle::BodyMedium` (14 medium), `TextStyle::Overline` (12 semibold, caller uppercases), `METRICS.layout.workflow_max_w` = 672.
- **Wave 2 (phases 06-09 and kit follow-ups) done:** agent header/transcript/activity, composer, requests and review, `SettingRow`, Overline section headings, SettingsFrame 48 px header, sidebar left-only border, single primary on onboarding. Deviations: tool output not shown (`ToolCard` carries no output); static permission ring; no +/- line counts (`ChangedPath` has none); reasoning text 12 px, not 11.
- **Combined gates (2026-10-31):** T (29 + 527 + 4), A (5), L (strict clippy on studio-theme and studio-ui, clean) and W (`agent_workflow_ui` 14, `_review` 7, `_apply_gate` 3) pass.
- **Wave 3 done:** phase 11 (mono ruler and length labels, `UI_FONT` on the Present root), the gallery follow-ups (new agent states, Brain reasoning rows, one progress cue, 1320 px shell capture), `design-system.md` rewritten for the Oleafly result, and the unused `ActivityHeader::running` removed.
- **Final validation (2026-10-31):** T (29 + 527 + 4), A (5), strict clippy on studio-theme and studio-ui, and W (14, 7, 3) pass on the combined tree. Every gallery page is captured in both themes in `.amp\in\artifacts\oleafly-final-run01`. The agent and onboarding pages first failed with a 0 × 0 client area (window minimized in the detached remote session) and passed on retry.
- **Not yet done:** native IME, hover/focus walkthroughs, 125-200 % DPI, real-adapter agent flows, performance comparison, macOS/Linux.

Review artifacts belong in `.amp\in\artifacts\oleafly-migration-<phase>-<run>`. `git rev-parse --git-path info/exclude` resolved `.git/info/exclude`; `/.amp/in/` is already present. Preserve it. Existing `oleafly-before`, `oleafly-after`, `oleafly-light-review.png`, `oleafly-dark-review.png` are historical evidence; do not overwrite, relabel or call current output a newly captured baseline.

## Phases

All phases are pending. Dependencies are strict finish-to-start. Shared files (theme metrics/type, `agent/mod.rs`, `chat_tab.rs`, `cards.rs`, gallery and docs) have **one writer at a time**, handed off after each gate. There is no claimed parallel speed-up. A phase may inspect a later surface, but not edit it early.

| # | Phase / milestone | Depends on | Estimate | Primary owned surface |
|---|---|---|---|---|
| 01 | [Baseline and reference contract](phase-01-start.md) | none | 1-1.5d | inventory/evidence, no UI |
| 02 | [Verify/finalize foundation](phase-02-foundation.md) | 01 | 1.5-2.5d | theme, fonts, F1-F4 fixes; M1 token contract |
| 03 | [Primitives, icons and overlays](phase-03-primitives-icons-overlays.md) | 02 | 2-3d | kit/assets; M2 kit complete |
| 04 | [Shell and navigation](phase-04-shell-navigation.md) | 03 | 1.5-2.5d | native shell; thin end-to-end slice |
| 05 | [Project/assets/style/props/slides/presets](phase-05-editor-panels.md) | 04 | 2-3d | editing panels; M3 editor shell |
| 06 | [Agent header/home/history](phase-06-agent-header-home-history.md) | 05 | 1.5-2.5d | agent frame and history |
| 07 | [Agent transcript/activity](phase-07-agent-transcript-activity.md) | 06 | 2.5-4d | messages, activity, markdown |
| 08 | [Agent composer/chips/queue](phase-08-agent-composer-chips-queue.md) | 07 | 3-5d | native input and dock |
| 09 | [Agent requests/change review](phase-09-agent-requests-review.md) | 08 | 2-3d | permission/question/review; M4 agent complete |
| 10 | [Settings/create/onboarding/export](phase-10-settings-create-onboarding-export.md) | 09 | 2-3d | standalone and modal workflows |
| 11 | [Timeline and Present](phase-11-timeline-present.md) | 10 | 1.5-2.5d | painters and presentation; M5 all screens |
| 12 | [Integrated regression/docs/handoff](phase-12-regression-handoff.md) | 11 | 2.5-3d | test evidence and docs; M6 release candidate |

Base estimates sum to **23-35.5d**; ranges include phase-local tests and review, not deployment or unrelated bug fixes. The four agent phases account for 9-14.5d because state coverage/native input are higher risk than palette work. Re-estimate after M2 and M4 against observed defects. No phase is complete from compilation or a static screenshot alone.

## Execution protocol (applies to every phase)

1. Just-in-time scout the listed sources, callers, tests, ids and dirty diff. Read newly discovered AGENTS.md before edits; if a path moved, resolve the symbol with scoped `rg` and update this new plan before implementing. Do not execute stale older plans.
2. Add/extend the relevant regression fixture or behavioral assertion **before** the presentation change, then make one 2-3-file slice; rerun that assertion afterwards. Do not test only copies of constants. Keep the application runnable after each slice.
3. Use semantic theme roles; add only required agent-specific metrics/type roles under the existing grouped token system. Never shrink global buttons to make the agent fit. Keep both style-guard allowlists empty; no new exceptions or lint suppression.
4. Run the relevant command gates below, capture named pages in both themes, inspect the PNGs, and run the phase's real-stack smoke. Track requested/passed/failed/blocked/not-run separately in Markdown, with date, command, platform, revision + dirty diff identity, dimensions/DPI and artifact paths. Redact prompts/credentials/private paths.
5. On failure stop at the smallest reproducible slice. Classify baseline vs introduced failure; fix introduced regressions, do not weaken assertions. An unavailable SDK/provider/platform is **blocked**, not pass. Recheck shared callers if a token changed. Hand off shared-file ownership only after acceptance is recorded.

## Command gates (run from repository root during implementation, not run by this planner)

Paths, package names, test targets and capture parser were source-checked. Default target path assumes no `CARGO_TARGET_DIR` override; execution must verify the actual binary before capture.

```powershell
# T: theme + UI library + guard
cargo test --locked --manifest-path desktop\Cargo.toml -p studio-theme -p studio-ui --lib --test style_guard
# A: app library and asset/font tests
cargo test --locked --manifest-path desktop\Cargo.toml -p fframes-studio --lib
# L: targeted strict lint
cargo clippy --locked --manifest-path desktop\Cargo.toml -p studio-theme -p studio-ui --all-targets -- -D warnings
# G: native gallery build
cargo build --locked --manifest-path desktop\Cargo.toml -p fframes-studio --example ui_gallery
# C: example: omit --appearance to capture BOTH themes; change unique dir per run
desktop\target\debug\examples\ui_gallery.exe --capture .amp\in\artifacts\oleafly-migration-07-run01 --page agent
# W: controller + scripted ACP + fake worker, NOT a native-window/provider qualification
cargo test --locked --manifest-path desktop\Cargo.toml -p fframes-studio --test agent_workflow_ui
# N: build and open the real Windows application for manual/native smoke
cargo build --locked --manifest-path desktop\Cargo.toml -p fframes-studio --bin fframes-studio
desktop\target\debug\fframes-studio.exe studio
```

`C` without `--page` captures all 11 pages. Capture is Windows Win32 `PrintWindow`, stitched scrolling content, not a UI-interaction driver. Hover/focus must be exercised interactively; the gallery itself documents that they cannot be forced by static capture. Never treat it as evidence of the live workflow.

Conditional Unix-only extra proof, on an owned Xvfb with xdotool and installed SDK (`SDK_ACTIVE` set to installed SDK active directory): `cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio --test x11_shell -- --ignored --nocapture`. `desktop/app/tests/x11_shell.rs` is `#![cfg(unix)]`. It cannot prove native Windows or a real provider and is not a Windows execution gate.

Full-app clippy has a known pre-existing `fetch_update` deprecation in `desktop\crates\studio-bootstrap\src\process\mod.rs:1220`; record baseline failure, do not fix or suppress it as visual migration scope. The transparent ghost focus-ring defect has already been reported separately. Keep remediation outside this plan unless it blocks keyboard use of a touched surface; if blocking, isolate a minimal separately tracked fix with a regression before proceeding. Neither is an excuse to hide new failures.

## Coverage and acceptance matrix

| Surface | Existing fixture coverage | Additional required native evidence |
|---|---|---|
| Kit/overlays | `kit`, `additions` | pointer hover/press, focus, Enter/Space, disabled tab skip, Escape/outside click and return focus, edge placement |
| Shell | `shell`: no-project/export/error | create/open/import/close, recent project, caption drag/Snap/maximize/restore, sidebar/timeline resizing, status/error disclosure |
| Editing panels | `props`: fields/disabled/error/not-editable/no-selection/no-project; `presets`: list/unapplied/committed/disabled/empty/no-project; `slides` combined fixtures | actual Project/Assets/Style lists (no dedicated capture pages), native file reveal/pick, props gesture undo, preset apply/reset, slide title/notes edits |
| Agent | `agent`: empty/not-set-up/working/failed/permission/question/todo-queue/code/history; **320 and 520px** in each PNG | real ConversationPanel, readiness/auth/connecting/cancelled, all steer modes, menu open/keyboard, long streams, paging, live request/reply, apply/reject/undo, lost/reconnect where supported |
| Settings/setup/export | settings general/agent/sdk/presets/export; onboarding welcome/appearance/sdk/project/agent/permissions; create blank/slide-deck; export settings/running/done/failed | separate live settings window and persistence; setup failure/retry; native save picker; video/deck unsupported/probing/cancelled/blocked states |
| Timeline/Present | slides markers plus live shell | trim/reorder/split/scrub/zoom/undo, explicit/default stops, full screen with audio, navigation, exit focus and pending build |

Cross-cut every touched surface with Light/Dark and System live switch, reduced/full motion, focus/hover/pressed/selected/disabled/error/busy/empty/long-content. Use normal window, minimum supported dimensions, sidebar 320/384/385/416/520px and short-height windows; measure composer **container**, not window, at the 384 boundary. Test Windows 100/125/150/200% DPI (including moving monitors), crisp hairlines/icon masks and no clipped focus rings. Use long English, Vietnamese combining marks, CJK, emoji, mixed-direction names and 30-50% longer translated-like strings; this is a layout/fallback audit, **not a new localization framework**. Verify no splitting graphemes, keyboard reading order, truncation with tooltip, horizontal code scrolling and reachable actions. Confirm native accessible names/roles and focus return; screen-reader limitations must be explicitly recorded rather than claimed solved.

Performance compares the same disposable project and deterministic stream before/after: idle CPU/wakeups, stream scroll p95/frame stalls, remeasure counts, memory after 1,000 transcript rows, resize latency, playback drops and overlay open latency. Hold caches/windowed paging and bounded output. No new persistent timers in reduced motion; permission animation only while visible. Investigate >10% median/p95 regression across three equivalent runs or new >50ms UI stalls; these are proposed migration budgets, not existing measured baselines. Record machine/load and absolute results before deciding significance.

## Read first and inspected implementation map

Use paths below relative to `C:\Users\Administrator\fframes-desktop\` (all owned paths in phase files resolve from this root). First reads: the two reference docs, `desktop\crates\studio-theme\src\metrics.rs`, `desktop\crates\studio-ui\src\kit\button.rs`, `desktop\crates\studio-ui\src\agent\composer\mod.rs`.

Planning inspected the actual theme `palette`, `typography`, `metrics`, `contrast`; kit `icon`, `button`, `surface`; agent `ui/{mod,chrome,message,activity}`, `cards`, `question_card`, `chat_tab`, `history/mod`, `composer/{mod,metrics,steer}`, `transcript/scroll`; shell `layout`, `left_column`, `header`, `settings`, `onboarding`, `present`; `props_panel`, `preset_panel`, `slides_panel`, `create_panel`, `export/panel`, `timeline/view`; app `src/assets.rs`, `Cargo.toml`, gallery root/agent/capture; `tests/style_guard.rs` and both allowlists; app `agent_workflow_ui` and `x11_shell` test headers/contracts. Reads of large modules were scoped to relevant render/contract sections, not claims of full-file audit. Each phase mandates a deeper execution-time scout of its adjacent files.

External corroboration: Oleafly `AssistantShellHeader.tsx:43` is `h-9`/`px-2`/`gap-1.5`; `ChatComposer.tsx:458,562,583-595` confirms 1.375rem radius, compact control row, 32px submit; `composer-input.ts:12` confirms 224px growth. Full extraction contains per-component source ranges used in phases 01 and 06-09. No browser runtime fidelity review was performed here.

## Decisions, risks and rollback

| Decision/risk | Resolution and early check | Recovery |
|---|---|---|
| Global foundation already present | Phase 02 audits and fixes only proven gaps; don't replay stale pending phases | Preserve/review existing dirty work before any own diff |
| Compact agent vs global sizing | Local grouped agent metrics; global controls follow D2 (buttons 32/24, fields 36), not the foundation's 36/32 | Reverse only new local metrics/call-site hunks |
| Web behavior differs | Native behavior locks in phases 07-09; never import Plan/Web/Persona, first-option permission emphasis or Stop-only layout | Restore visual hunk while behavioral tests remain |
| Agent requests compete for height | Bounded independent request region, keep composer reachable; short-window manual proof | Fall back to neutral bounded request styling, no hidden blocking request |
| Font coverage, application and semibold | D1: full-coverage Geist applied at window roots, or the system font; check synthesized semibold and fallback at several DPIs | If no full build passes the probe, remove the unused UI faces and keep the system font; never ship the Latin-1 subset |
| Nested-surface controls invisible (F3) | Composited-contrast test over every surface before any colour edit | Restore Cap-style translucent `ctl` alphas over the new surfaces |
| Permission gradient GPUI support/cost | Prototype smallest native border in phase 09, static under reduced motion; no new renderer/backend | Static spectrum border if animation exceeds budget; document deviation |
| Shared-file conflicts | Strict serial ownership and fresh diff/scout at every phase | Stop and reconcile; never revert another person's hunk |
| Real SDK/provider unavailable | Disposable installed-SDK project + qualified adapter required for final manual smoke | Mark gate blocked; do not substitute gallery/scripted tests as qualification |
| Screenshot/OS variation | Stable fixture/DPI/theme + semantic visual review, not universal bitwise pixels | Compare matched runs; retain rejected and approved evidence |

Alternative rejected: restarting a wholesale design-system rewrite would erase existing work and enlarge risk. Rejected: copying Oleafly React/workflows or turning every agent action into a global small button. Adopt its visual vocabulary with explicitly documented native adaptations instead. Only revisit if a concrete native limitation prevents a required surface; use the smallest local rendering adaptation, not a new architecture.

### Consequential assumptions and remaining uncertainty

Assume one sequential owner and an unchanged native architecture. Preserve160px automatic input cap/56px thumbnails, History tab navigation and all current permission/send semantics deliberately; adopt56px resting composer and compact local controls as presentation changes. Use CodeSmall11/16 for native tool output rather than source10px mono, and Small12 for permission description rather than source13px. These choices avoid extra tiny-font variants while keeping the overall hierarchy. Source dimensions are logical pixels, with actual raster/DPI/contrast reviewed at execution.

Not qualified by this planner: availability of a real SDK/adapter supporting every queue/steer/question path, GPUI animated radial-border fidelity/cost, platform screen-reader exposure, synthesized semibold/fallback rendering, cross-platform window behavior and measured performance. Phase01/02 discover environment/font constraints; phase08/09 qualify native input/provider paths/gradient; phase12 reports unresolved platform gates. Do not change architecture or capability behavior to make those unknowns disappear.

## Success Criteria

- [ ] M1-M6 accepted with source/value/deviation records and no unresolved visual contract conflict.
- [ ] Every existing gallery page/state reviewed in both themes; missing live-only states covered in native Windows evidence.
- [ ] Behavior ids, data, queue/steer/send-now, permission order/emphasis, IME, focus, undo and presentation preserved.
- [ ] T/A/L/G/W gates recorded truthfully; known baseline blockers and platform exclusions separated; real Windows smoke complete.
- [ ] DPI/localization/a11y/reduced-motion/performance matrix complete with measured evidence, not assertions of readiness.
- [ ] Documentation describes the implemented result and accepted deviations; no new theme customization, backend feature or branding leak.
- [ ] Only executor-owned changes included; other people's work and historical artifacts preserved; no commit/push/publication assumed.

## Validation log — planning only, 2026-10-30

- Loaded official planning-work and read installed `ak-plan` skill plus CLI integration, output standards, planning rules and validation workflow; applied source-first planning directly, no subagents.
- CLI `create --help` and `add-phase --help` checked; `ak plan create oleafly-studio-ui --basedir plans --json` succeeded; 11 `add-phase` calls succeeded. Read `plan.md` and all 12 generated stubs before replacing content with the editing tool. No `use`, reindex, status mutation of older plans, archive or publish command requested.
- Source verified: reference revision, existing palette/type/fonts, 50 Icon variants, gallery's 11 pages/9 agent states/320+520 widths, native menu/ids contracts, permission reject-first/allow-primary, SteerMode matrix, 70/320 scroll constants, 160px input cap, 56px thumbnails, test platform boundaries, package/test paths and empty guard allowlists.
- **Second pass (same day):**
  - Ran gate T on the dirty checkout: it passed.
  - Built gate G and captured all pages in both themes to `.amp\in\artifacts\oleafly-research-current`, then inspected the shell, agent, props and settings captures.
  - Probed the font cmaps and rechecked the Oleafly sources and the Studio agent constants.
  - Findings F1-F6 and decisions D1-D3 come from this pass. Phase 01-04 and 08 were edited to match.
- The first pass's record is **source inspection**, not test execution. Prior-run tests described in the brief were not rerun today. No Cargo build/test/clippy, gallery capture, native smoke or performance measurement was performed during planning.
- `ak plan validate plans\261030-1500-oleafly-studio-ui --json` returned `valid: true` with no errors. `ak plan parse ... --json` recognized12 phases,0 complete; it normalizes pending phases to todo and counts bullet acceptance checkboxes, not the ordered execution checklist. Markdown remains the execution authority.
- Whole-plan source/consistency sweep checked phase links, sequential dependencies, owned-file handoffs, exact native exceptions, estimate arithmetic, command/test platform scope and absence of scaffold placeholders. New plan files were reread after editing. Structural CLI validation is not semantic or UI proof. No user interview was possible; assumptions above are decisions, not user-confirmed answers.

<!-- slug: oleafly-studio-ui -->
