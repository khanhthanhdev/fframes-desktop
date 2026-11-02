---
phase: 10
title: "Settings, create, onboarding and export"
status: pending
priority: P1
effort: "2-3d"
dependencies: [9]
---

# Phase 10: Settings, create, onboarding and export

## Goal and owned paths

Finish every secondary Studio workflow with the same kit, preserving separate native windows, setup recovery and export capabilities. Root: `C:\Users\Administrator\fframes-desktop\`.

- Modify `desktop\crates\studio-ui\src\shell\{settings,settings_window,onboarding,create,export_panel}.rs`, `create_panel.rs`, `export\{panel,settings,progress}.rs`, `agent\{setup_page,provider_setup}.rs` presentation as needed. `preset_panel.rs` ownership returns from05 only for settings embedding, not another redesign.
- Gallery `desktop\app\examples\ui_gallery.rs`, `ui_gallery\capture.rs` fixture gaps. Shared kit `settings_frame.rs`/metrics only via explicit serial handoff when required. No actual setup/install/export service changes.
- Read behavioral tests and app settings/setup/export types before changes; preserve native folder/save picker, lifecycle and persisted preference formats. No source/doc edits during this planning session: these paths are **future execution ownership**.

## Just-in-time source scout

Trace `render_settings`, `SettingsPage::ALL`, `open_settings`/window focus lifecycle, theme/motion preferences, `render_appearance`, `render_audio_output`, `render_review_policy`, export defaults; `render_onboarding` and recorded failed setup step; `CreatePanel::render`/template table; `ExportPanelModel`, support/probe/deck blocker/editability/outcome/cancel. Check provider setup state/errors and bounded logs. Read Windows caption handling for settings independently from main header; do not convert it to an Oleafly dialog.

## Tasks and steps

1. [ ] Extend fixtures/assertions first for all settings pages, no-project preset state, theme persistence, create Blank/Slide deck, setup busy/failed/retry and export probing/unavailable/blocked/cancelled outcomes. Current captures cover common states only; don't claim they include every failure path.
2. [ ] Settings slice: nav rows40px/type14/icon16/gap10/radius8/padding10×8, active document fill+border/foreground medium, inactive muted; use existing SettingsFrame and PanelSection/FormField hierarchy. Preserve General/Agent/SDK/Presets/Export order, nav ids `settings-general` through `settings-export`, and per-project unavailable notices.
3. [ ] Keep settings native separate window782×775/min780×560 and Windows36px title strip. Theme System/Light/Dark and motion preferences apply immediately and persist, main editor can keep playing; settings close/CtrlW must not quit main app. Run setup again closes settings, which stays unavailable during first-run setup. Adapter setup and Presets keep their own ids and event ownership.
4. [ ] Create slice: restyle title, description, RadioCards and actions without changing `ProjectTemplate::ALL`, Blank first/default, `create-template`, `create-back`, `create-continue` or Choose folder flow. Native cancellation preserves current project and does not write new files.
5. [ ] Onboarding slice: use Display24/Body14,24px section spacing and shared progress/status; Welcome/Appearance/SDK/Project/Agent/Permissions, Back/Next/Finish/Skip remain. Failed step attribution persists (Install SDK owns install/build/render pipeline failures); retry doesn't falsely mark later steps done. No new setup backend or account flow.
6. [ ] Export slice: unify Output/Video-or-Deck fields, editable/disabled/blocked notices and busy progress using shared kit. Keep support probing, renderer-specific choices, per-export vs saved-default separation, deck notices/skipped stops and cancel semantics. A failed export stays persistent Notice with details, successful transient confirmation may toast; no misleading success when backend says failed.
7. [ ] Provider/SDK setup presentation slice: align fields, agent test/auth/readiness feedback and installation/progress/log surfaces; preserve credential handling and native input focus/IME. Do not copy Oleafly model/persona/theme-customization choices.
8. [ ] Run T/A/L/G; C `settings`, `create`, `onboarding`, `export`, `presets` in both themes/all existing states. Inspect long copy, keyboard nav and minimum window at125/150/200% DPI. Tooltips/popovers stay in-window and return focus; only active main action accent.
9. [ ] In N disposable app/project state: open/close settings while preview plays, change theme/motion then restart to verify saved choice; create/cancel/open each template; run setup through successful and controlled failure/retry (do not corrupt installed SDK); export short video and deck when supported, cancel an export and inspect failure presentation via owned fixture. Validate output actually opens and UI returns to editor. Mark unsupported backend targets N/A with reason, not newly implemented.
10. [ ] Record secondary-surface acceptance and hand gallery/metrics to11; protect real user app-settings/session data by using existing isolated fixture mechanisms discovered in scout.

## Acceptance

- [ ] Every settings/setup/create/export page has visual and native flow evidence; fixtures not present are covered explicitly or blocked.
- [ ] Separate settings lifecycle, native pickers, persistence, template defaults, setup recovery and export semantics remain unchanged.
- [ ] Readable long-content and error/busy/disabled hierarchy across minimum dimensions, DPI and both themes; no duplicate theme system/backend feature.

Rollback only new presentation hunks per slice; do not revert existing preferences or installed SDK to recover styling. Restore only owned disposable fixture data. Dependency09; estimate2-3d assumes shared kit is stable and backend flows unchanged.
