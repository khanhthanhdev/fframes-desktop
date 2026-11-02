---
phase: 2
title: "Verify and finalize existing foundation"
status: pending
priority: P1
effort: "1-2d"
dependencies: [1]
---

# Phase 02: Verify and finalize existing foundation

## Goal and ownership

Accept the already-local token/font/surface migration with only proven gap fixes. M1 is a stable shared contract, not an unnecessary rewrite. Root for paths: `C:\Users\Administrator\fframes-desktop\`.

- Potential modify: `desktop\crates\studio-theme\src\{palette,metrics,typography,elevation,motion,appearance,lib}.rs`; theme tests inline there; `desktop\app\src\assets.rs` only if font-registration evidence fails.
- Verify, normally no edit: `desktop\app\assets\fonts\{Geist-Regular.ttf,Geist-Medium.ttf,GeistMono-Regular.ttf,LICENSE.txt}` and startup/gallery calls to `init_fonts`.
- Borrow gallery root `desktop\app\examples\ui_gallery.rs` for a missing regression fixture only. Hand it off before phase03. No Cargo.lock change or replacing another owner's font files.

## Just-in-time scout

Read phase01 ledger and current diff of each candidate file. Trace `Colors::{light,dark}`, derived semantic status/lane/syntax roles, grouped `METRICS` aliases and tests, `TextStyle::{size,weight,line_height,family}`, `shadow_for`, preference initialization and `bind_window`. Find `init_fonts` callers in app startup and gallery. Check contrast tests cover nested surfaces and alpha compositing; inspect synthesized semibold appearance rather than assuming a missing font file is a bug.

## Known defects to fix first (from `plan.md` F1-F6; each needs a failing test or capture before the fix)

1. [ ] **F3, invisible control fills.** Write a `studio-theme` test that composites `ctl`, `ctl_hover` and `ctl_active` over `window`, `card`, `card_2`, `sidebar` and `popover` in both themes, and requires a visible lightness step at rest (start with ΔL* ≥ 3 using `contrast::lightness`) and between rest, hover and pressed. It fails today: `ctl == card_2`. Fix it in `palette.rs`, preferably by restoring translucent fills (about black 6/9/12 % in light, white 8/11/15 % in dark), so the documented "works on any surface" contract holds again. Replace the rewritten `neutral_surfaces_are_opaque…` assertion for the control roles, keep it for surfaces, and recapture `settings-general` and `kit` in dark.
2. [ ] **F4/D2, density.** Set `ButtonSize::Md` to 32 px with 12 px padding and `Label` text, and `ButtonSize::Sm` to 24 px with 8 px padding and 11 px medium text, in `kit\button.rs` and the grouped `METRICS.size`. Fields stay 36 px; list rows stay 32. Update `button.rs` tests to the new heights. Do not touch icon-button sizes. After this, F5 (the header and transport widths) belongs to phase 04; note any visible overflow now.
3. [ ] **F1/F2/D1, font.**
   - Download the upstream Geist and Geist Mono release (OFL; record the URL, version and SHA-256) into a scratch folder outside the repo, and run `py -I plans\261030-1500-oleafly-studio-ui\cmap_probe.py <files>`.
   - If Vietnamese precomposed letters, Latin Extended and → all pass, replace the three files and `LICENSE.txt`, then apply `UI_FONT` once at each window root (main shell root in `shell\layout.rs`, the settings window root and the gallery root) through a theme helper, not per view.
   - Otherwise remove the Geist UI faces from `init_fonts` and the assets, restore `UI_FONT = ".SystemUIFont"`, and keep Geist Mono only if it passes.
   - Either way, add an app test that loads the shipped files and asserts the coverage set, so a subset can't come back. In the gallery, render "Trần Thành, Đức, Ơn, Ứng → 漢字 😀" in `Body`, `BodyStrong` and `Code` and inspect it at 100 % and 150 %.
4. [ ] **F6.** Switch `transport.rs:61` `font_family("monospace")` to `TextStyle::Code`, `CodeSmall` or `MONO_FAMILY`, and extend the style guard to reject literal `font_family("…")` strings outside `studio-theme`. Decide whether `Small` stays 12 px (equal to `Label`) or returns to 11; write the choice in the M1 record.

## Tasks and steps

1. [ ] Compare every phase01 color/type/geometry row against actual tokens and record exact match/deviation. Do not replay old Cap sizes over the D2 sizes.
2. [ ] Extend existing token/contrast tests before any correction: white-on-blue>=4.5:1, secondary>=4.5:1 on document/card/nested/sidebar/popover, tertiary>=3:1 on the surfaces where used. Composite alpha borders/focus over actual backgrounds; do not strengthen colors globally without checking all callers.
3. [ ] Fix only failed role mappings or missing aliases in 2-3-file slices. Keep semantic status/lane/playhead/syntax colors and theme preferences. Control fills follow the F3 fix above; surfaces stay opaque. `Card::panel` remains flat/square, `Shadow::Card` flat, overlay Pop offset4/blur12 (not CSS shadow replication).
4. [ ] After the D1 decision, confirm that the chosen faces load before the first window and are actually *applied*: a captured `Body` sample must not be Segoe UI when Geist is chosen. Check licensing and glyph fallback, and inspect Body/Label/Heading/Code at 100/150/200 % DPI with Vietnamese, CJK and emoji. If Geist is kept, compare synthesized semibold with the upstream SemiBold face, and add that file only if the synthesized weight is visibly wrong.
5. [ ] Verify System→Light→Dark while main and settings windows are open; colors repaint cached sections. Toggle Reduced/System/Full motion and verify spinners/animations stop appropriately; do not add custom transition infrastructure.
6. [ ] Run T/A/L/G; capture `kit`, `additions`, `shell`, `settings` in both themes to unique phase02 artifacts and inspect for cross-surface regressions. Reopen the real baseline flow N; confirm no lost focus or layout changes caused by metrics.
7. [ ] Record M1 token inventory, accepted deliberate differences and test results. Reserve later agent-specific additions for phase06-09 under one serial owner; don't preemptively create unused metrics.

## Acceptance

- [ ] Colors, typography, global controls and licensing match phase01; no local screen literals, palette family or theme picker added.
- [ ] Both style-guard allowlists remain empty; contrast tests and font asset tests recorded; known baseline blockers separated.
- [ ] Native real-window theme switch/focus smoke and both-theme visual captures reviewed. No ordinary control shrunk for agent density.
- [ ] M1 signed off in Markdown; downstream surfaces can rely on token meanings without re-deriving them.

Rollback: reverse only this phase's own token/registration hunks if new regressions appear; preserve pre-existing dirty palette/font work. Re-run impacted callers' captures after any correction. Estimate 1.5-2.5d (raised from 1-2d for F1-F4 and the font download and verification). It assumes the rest of the foundation is correct; a broad mismatch triggers re-estimation, not uncontrolled redesign.
