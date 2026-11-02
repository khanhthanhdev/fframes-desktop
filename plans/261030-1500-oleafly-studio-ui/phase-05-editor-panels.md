---
phase: 5
title: "Project, assets, style, props, slides and presets"
status: pending
priority: P1
effort: "2-3d"
dependencies: [4]
---

# Phase 05: Project, assets, style, props, slides and presets

## Goal and owned paths

Apply the accepted kit to every non-agent editing panel and retain all engine-owned editing/data contracts. M3: everyday editor shell and panels visually coherent. Root: `C:\Users\Administrator\fframes-desktop\`.

- Modify in separate slices: `desktop\crates\studio-ui\src\shell\left_column.rs` (Project/Assets/Style/recent project bodies only), `props_panel.rs`, `preset_panel.rs`, `slides_panel.rs`.
- Gallery fixture paths: `desktop\app\examples\ui_gallery\panels.rs`, root `ui_gallery.rs`; `capture.rs` only if adding a necessary missing fixture state, not speculative new page names.
- Read-only behavioral callers: `shell\props_edits.rs`, `shell\timeline_edits.rs`, engine props/presets/slides commands. Presets project styling stays distinct from Studio chrome. Handoff `preset_panel.rs` to phase10 for settings integration only.

## Just-in-time scout

Read `render_project_tab`, `render_assets_tab`, `render_style_tab`, `file_row`/`file_list`; `PropsPanel::{set_snapshot,set_selection,set_info,render}`, color picker/menu focus, `PropsEvent`; `PresetPanel::{set_view,report,render}` and `PresetEvent`; `SlidesPanel::{set_slides,select_stop,render,commit}` and `SlidesEvent`. Trace disabled/read-only/no-project states through callers and tests, not only builder declarations. Inspect actual edited types and schemas before layout changes.

## Tasks and steps

1. [ ] Extend fixtures and assertions first for long filenames, empty/error/disabled/no-project, read-only props, unapplied/committed presets and unresolved slide stops. Keep field ids and telemetry assertions; test select/menu return focus and no write on mere navigation.
2. [ ] Project/Assets slice: use existing List/ListRow/PanelSection/EmptyState; 16px icons and shared row sizing with selected/hover states, full paths in tooltips, truncation not wrapping endless filenames. Keep file reveal/open/pick actions and native dialogs unchanged. Verify actual shell because no dedicated Project/Assets capture pages exist.
3. [ ] Style/presets slice: selected preset, imported/bundled origins, CSS report, override fields, reset/export/import keep existing command semantics. Neutralize panel chrome, not video preset tokens; never let a selected project preset recolor Studio UI. Style presets entry still opens the separate settings Presets page.
4. [ ] Props slice: arrange `PanelSection`/`FormField` with 24px section rhythm and existing controls. Check text/number/color/toggle/select/slider, reset-to-code, read-only/error explanations and unavailable schemas. Do not change parsing/ranges; preserve native IME, one slider gesture→one undo/write and Escape→shell focus.
5. [ ] Slides slice: list number/title/scene/time, selected fields, speaker notes, stop problems, scans/progress/cancel and Present from here use existing kit hierarchy. Preserve title/notes commit-on-blur as one undo, default/explicit stop distinctions and Add stop actions. Do not move timeline controls into this panel or rework deck logic.
6. [ ] Run T/L/G and C `props`, `presets`, `slides`, `shell`. Review all enumerated states in plan.md, both themes and320/520 sidebar widths; add320/384/385/416 native checks with long translated-like labels and Unicode paths.
7. [ ] Real N with a copied disposable video project: reveal an asset, select canvas object, edit each prop kind, reset then undo, choose/apply/reset preset, add/rename stop and speaker notes, navigate and undo once per edit. Verify on-disk data only changes for explicit existing commands; ordinary focus/theme changes do not write project files.
8. [ ] Record M3 visual+behavior acceptance and evidence paths. Hand off shared gallery and panel files; list any live-only missing coverage for phase12 rather than inventing unsupported states.

## Acceptance

- [ ] All six editing tabs are accounted for (Agent deferred next); Project/Assets/Style proven in real shell, not falsely claimed as gallery pages.
- [ ] Fields/actions remain usable at narrow width/high DPI; no hidden reset, lost validation, duplicate primary emphasis or changed ids.
- [ ] Props/presets/slides data and one-gesture undo semantics preserved; file pickers/reveal remain native.
- [ ] M3 gate logged with both-theme review and successful real edits on disposable data.

Rollback only each new panel's styling hunks; preserve edited fixture projects for diagnosis until verified disposable. No restoring project files as part of code rollback unless explicitly owned disposable test data. Dependency04; estimate2-3d, split by panel families to keep the app runnable throughout.
