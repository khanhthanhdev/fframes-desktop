---
phase: 1
title: "Baseline and reference contract"
status: pending
priority: P1
effort: "1-1.5d"
dependencies: []
---

# Phase 01: Baseline and reference contract

## Goal and owned paths

Freeze an honest starting point and a component-by-component native contract before further UI changes. Own only evidence under `.amp\in\artifacts\oleafly-migration-01-<run>` and execution notes/checklists within this new plan. No source edits in this phase. All repository paths resolve from `C:\Users\Administrator\fframes-desktop\`.

## Just-in-time source scout

- Read both `docs\desktop\oleafly-design-system.md` and `design-system.md` in full again; confirm the source checkout revision and record if it differs from pinned `65aa177220c33cba50769a14f1ff705d7cfe373e`.
- Trace `Theme::for_window`, `METRICS`, `TextStyle`, `Card::panel`, `Icon::ALL`, gallery `Page::ALL`/`states`, `ConversationPanel`, `Composer::controls`, `steer_actions`, `permission_request` and scroll constants. Inspect current dirty diffs first.
- Read `desktop\app\examples\ui_gallery\capture.rs`, `agent.rs`, `desktop\crates\studio-ui\tests\style_guard.rs`, app tests `agent_workflow_ui.rs` and `x11_shell.rs` before assigning evidence claims.

## Reference contract to carry into all phases

The following are source-backed defaults from the extraction, corroborated where noted in `plan.md`; they are **not new local literals to scatter in views**. Colors belong in `studio-theme`, sizes in grouped metrics, typography in `TextStyle`.

| Role | Studio Light / Dark contract | Adaptation |
|---|---|---|
| document / foreground | `#ffffff/#0a0a0a` / `#0a0a0a/#fafafa` | Oleafly N(1)/N(.145), N(.145)/N(.985), rounded sRGB |
| card / sidebar | `#fafafa/#171717`; `#f7f7f7/#171717` | solid native panes, no translucent shell |
| muted / popover | `#f1f1f1/#262626`; `#fafafa/#121212` | opaque nested surfaces |
| secondary / tertiary text | `#6b6b6b/#a1a1a1`; `#737373/#8b8b8b` | secondary stronger than Oleafly light ~#737373 |
| line / strong input line | `#dedede/#ffffff1a`; `#a1a1a1/#ffffff26` | 0.5px hairline, stronger native input outline |
| control rest / hover / pressed | currently opaque `#f1f1f1/#262626`, `#ebebeb/#303030`, `#e5e5e5/#383838` | **F3:** resting `ctl` equals `card_2`; phase 02 makes the control fills translucent, or opaque values that differ from every surface they sit on |
| primary / hover / on-primary | `#2563eb`; `#1d4ed8`; white, both themes | runtime blue, not Oleafly neutral CSS fallback; no accent picker |
| status / syntax / lanes / playhead | existing Studio semantic roles | preserve, never monochromatize video/status meaning |

| Geometry/type | Contract |
|---|---|
| rhythm/radii | 4px rhythm with 2/6/10 half-steps; small6/medium8/base10/XL14; persistent panes flat and square |
| spacing | ordinary gap8, toolbar gap4, section24; overlays padding24/gap16 |
| global buttons | **D2:** Md 32 px, padding 12, `Label` 12 medium (Oleafly `sm`); Sm 24 px, padding 8, 11 medium (Oleafly `xs`); radius 8, gap 8, disabled 50 %. The foundation's 36/32 is a deviation to correct |
| icon controls | current 24/28/32px boxes; usual toolbar28 with glyph16; compact agent separate |
| fields | height 36 (Oleafly `h-9`, its most common field height), padding X 12, Body 14/20, radius 8; textarea reference min 80, native multiline behavior preserved |
| text | Caption10/14 medium; Small12/16 regular; Label12/16 medium; Body14/20 regular; BodyStrong14/20 semibold; Title16/24 medium; Heading18/24 semibold; Display24/32 semibold; Code12/18; CodeSmall11/16 |
| typefaces | **D1:** full-coverage Geist applied at every window root, or the system UI font. Today the Latin-1 subset is registered but `UI_FONT` is never applied (F1, F2); Oleafly itself does not bundle Geist |
| shell | keep52px native header, default416px sidebar, limits320-520, player minimum480; retain split behavior |
| focus/motion | visible2px focus, Enter/Space, disabled skip;150/200/320ms, tooltip300ms; reduced motion ends motion/stops spinners |

Agent contract: ACP composition, header36px, transcript16×12px insets, row gap12/inner6; user blue+white max85%, assistant full-width lightly framed (light document + line60%, dark muted + transparent line), bubble radius10/padding12×8/type14, footer gap2/time10/copy14 glyph. Borderless activity with14px icon,12px disclosure,8px gap; reasoning collapsed, plain pre-wrapped11px with left rule/inset12/max224; output mono10/max320 and first4,000 characters with explicit show-all. Preserve current paging, statuses and meaningful results.

Composer reference (rechecked at `ChatComposer.tsx:458`): card with a `shadow-sm` lift and a focus-coloured border while focused, radius 22, padding12 sides/10 top/8 bottom, transparent Body input, 56px resting height, growth224 reference; controls min28px, 8px top gap, left/right groups4px gaps, 32px circular ArrowUp and14px filled Stop. At container<=384px use2px gaps, attachment/usage targets24 and send28. **Native decisions:** preserve current160px automatic input cap and native resize contract in this migration (56px resting viewport is a presentation adjustment, not an auto-growth rewrite); preserve56px image thumbnails as Studio's useful video/screenshot context, not Oleafly24px. Document these intentional differences in phase08. Do not add manual web resize behavior if absent.

Requests: permission-only spectrum border2px, outer14/inner10/padding14, ShieldQuestion20 in28 slot, heading14 semibold, description13 reference, gap12, actions32/wrapping/right aligned with separator/top10/gap8; 14s animation frozen under reduced motion. Native semantic-role colors replace Oleafly local color literals. Questions neutral radius10/padding12/gap8/type12, compact field28/type12. Changed-file labels mono11/FileDiff14; compact review24. None authorizes a new commit/undo backend action.

## Tasks and steps

1. [ ] Save a redacted starting status/diff identity and note every existing/untracked ownership boundary. Confirm `.git/info/exclude` still contains `/.amp/in/` before evidence writes. Do not mutate old plans or active-plan pins.
2. [ ] Build a ledger mapping each row above to current token/component, reference source range, implemented/partial/missing, native deviation, phase owner and verification. Distinguish screenshots from source facts; keep unknowns explicit.
3. [ ] Run T/A/L/G from `plan.md` **before new implementation**, preserving logs including baseline failures. If already-built binaries are used, record their provenance and don't call them rebuilt. Resolve compile/tool issues before interpreting screenshots.
4. [ ] Record **two** baselines; do not overwrite the existing ones:
   - **Pre-foundation (Cap):** light already exists in `.amp\in\artifacts\before\light` (44 PNGs, captured before the palette edit). For dark, run `git worktree add <sibling dir> HEAD`, build the gallery there with its own `CARGO_TARGET_DIR`, and run `--capture <dir> --appearance dark`. The worktree contains no dirty work. Remove it afterwards with `git worktree remove`.
   - **Current foundation:** `.amp\in\artifacts\oleafly-research-current` (both themes) exists from the research pass. Recapture into a fresh phase 01 directory only if the dirty diff has changed since then (compare `git diff --stat`).

   Inspect every PNG for blank or missing assets and annotate the F3 nested-surface fills, the F5 header width and the agent mismatches. Leave the historical `oleafly-before/after` and review PNGs untouched.
5. [ ] Open N with an installed SDK and disposable fixture project. Record real shell→open project→preview→agent focus→settings return path as the baseline thin vertical flow; exercise native focus/IME rather than only gallery mocks. If prerequisites are unavailable mark blocked, not passed.
6. [ ] Record baseline DPI/window sizes and same-workload performance counters described in plan.md. List live-only states absent from gallery; map each to later phase proof. Review contract/ledger before authorizing phase02.

## Verification and acceptance

- [ ] New baseline provenance is distinct from all historical artifacts; both themes and every existing Page are listed, including failures/blocked captures.
- [ ] Existing foundation marked verification work, not queued rewrite; agent-specific pass marked remaining.
- [ ] Behavioral exceptions locked: rejects before allows/allow emphasis; running Send+Stop+More preserved; None/AtBoundary/Native unchanged; Ctrl/CmdEnter cancels via SendNow; follow70/jump320 retained.
- [ ] Source/value/deviation ledger and execution checklist have no unowned surface. All gates have truthful result labels.

Rollback: this phase adds evidence only. Remove only new disposable evidence if incorrect; never relabel old captures or change source to make a baseline green. Dependencies: none; phase02 blocked on accepted contract, not a claim that every pre-existing diagnostic is fixed.
