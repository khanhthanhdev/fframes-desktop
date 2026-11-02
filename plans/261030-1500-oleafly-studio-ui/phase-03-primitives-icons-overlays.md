---
phase: 3
title: "Complete primitive kit, icons and overlays"
status: pending
priority: P1
effort: "2-3d"
dependencies: [2]
---

# Phase 03: Complete primitive kit, icons and overlays

## Goal and owned paths

Finish only missing shared treatments and inventory icons before screen-specific work. M2 is a complete usable native kit. Root: `C:\Users\Administrator\fframes-desktop\`.

- Modify as evidenced, one small family per slice: `desktop\crates\studio-ui\src\kit\{button,select,menu,tooltip,surface,tab_rail,settings_frame,list_row,chip,badge,notice,form_field,panel_section,radio_cards,toggle,segmented,slider,number_field,color_picker,progress,spinner,toast,interact,icon}.rs`, `kit\text_input\{mod,element}.rs`.
- Icon ownership: `desktop\app\assets\icons\*.svg`, `desktop\app\src\assets.rs`, `kit\icon.rs`; new files only from the justified inventory below, preserve ISC notice.
- Gallery ownership: `desktop\app\examples\ui_gallery.rs`, `ui_gallery\kit_extras.rs`; token corrections by explicit handoff of phase02 files only. Do not edit style-guard allowlists.

## Just-in-time scout

Read builders and callers before changing APIs: `ButtonSize`, `IconButtonSize`, `focusable`/`on_activate`, menu disabled navigation/focus return, text input IME, tooltip placement/delay, `Card`/`Popover`, settings nav, color-picker bounds. Read app asset tests checking `Icon::ALL` against embedded files. Audit exact existing SVG viewboxes/strokes; don't infer a Lucide symbol from its filename alone. Match no-regression fixtures before alterations.

## Icon inventory and sizing

Current `Icon::ALL` is50 including4 native captions. Inventory by purpose (retain all; glyph use is not permission to add controls):

- Playback/timeline: Play, Pause, SkipBack, SkipForward, StepBack, StepForward, ZoomIn, ZoomOut, Maximize, Volume2, VolumeX, Speaker, Scissors, Presentation.
- File/project/style: Folder, FolderOpen, File, Image, Palette, Download, Upload, Save, Layers, Package.
- Agent/actions/status: Bot, Hammer, Square, RefreshCw, Plus, X, Check, ChevronDown, ChevronRight, MoreHorizontal, Settings, AlertTriangle, Info, CircleCheck, History, Eye, ArrowUp, ArrowLeft, Search, Trash, Undo, Redo.
- Preserve CaptionMinimize/Maximize/Restore/Close paths and Studio mark; no leaf branding.

Required new glyphs if still absent after scout: **Copy** for message action, **Brain** for reasoning, **Paperclip** for attachment chips, **Clock** for queue, **ShieldQuestion** for permission request, **FileDiff** for changes. Prefer reusing Settings/MoreHorizontal/Maximize for existing semantics; do not add Settings2, BarChart3, PictureInPicture2, PanelRightClose, SlidersHorizontal merely because Oleafly lists unavailable utilities. New utility glyph requires an existing Studio action mapping and actual caller in the same slice.

All ordinary SVGs:24-unit viewbox, stroke2, rounded caps/joins, currentColor/mask-compatible, no decorative fill. Default16px, small actions14, disclosure/status12; permission20 in28 slot; home agent mark30 only if existing branding supports it. Boxes24/28/32; toolbar28. Selected badge is distinct16px blue circle with white10px check/stroke3.5 (do not globally thicken Check). File interior1.75 reference is optional only where a distinct file treatment already exists. Stop14px filled Square is a local presentation of existing stop, not a new generic icon style.

## Tasks and steps

1. [ ] Add or extend gallery/regression cases for default/hover/pressed/focused/selected/disabled/error, long labels and no-handler controls before changing each kit family. Audit existing foundation first; unchanged compliant primitives need evidence, not edits.
2. [ ] Verify buttons at the D2 sizes (Md 32, Sm 24) and fields at 36 px, radius 8, role-based fills,50% disabled; keep fluent APIs and caller ids. Ensure row labels are not accidentally selectable but text bodies still are. Transparent ghost focus defect remains a separate tracked fix unless this touched surface is unusable.
3. [ ] Complete navigation/list/form/card/status families with phase01 rhythm. Preserve `Card::panel` flatness, nested content hierarchy, selected/disabled/error distinctions, one main primary action per view (permission's existing multi-allow exception remains native semantics).
4. [ ] Verify Menu32px rows/type14, popover fill/radius8/inset4 and ordinary min160; select/context reference min128 and select max384; generic popover min168 only where appropriate. Use current native anchors/viewport clamping and Pop shadow4/12, not web CSS libraries. Dialog adaptation: padding24/gap16/radius14, viewport minus32 and max-width reference32rem=512px where a dialog actually exists; no modal conversions of settings windows.
5. [ ] Verify tooltip type12/padding8×4/radius8/delay300ms, keyboard immediate, gap6/clearance8/max260. Walk all four viewport corners and disabled controls; preserve focus-return/Escape/outside click behavior for every overlay. Backdrop55% black where already modal, no blur dependency.
6. [ ] Add the six justified glyphs in small asset+enum+gallery-inventory slices; update `Icon::ALL`, `Icon::path`, embedded `icon_files!` together and render each in the gallery icon inventory now. Actual agent action wiring remains owned by07-09, not early edits to those screens. Run app asset tests after each addition. Record unsupported utility candidates as not added, not placeholder enum variants.
7. [ ] Run T/A/L/G and C for `kit`/`additions`/`settings`/`shell`. Inspect both themes and100/125/150/200% scaling; in the interactive native gallery Tab/ShiftTab/Enter/Space through controls, arrow past disabled menu items, Escape back to trigger, and type a CJK IME composition in real TextInput.
8. [ ] Open the real app N: navigate editor tabs, open an anchored menu and field/picker, dismiss/return focus. Record M2 acceptance, including accessibility limitations, then hand shared files to phase04. Re-estimate downstream if any kit API correction affects all callers.

## Acceptance and rollback

- [ ] Every current primitive has state evidence and no caller semantics/id change; newly added icons resolve in app+gallery and licensing remains intact.
- [ ] No speculative variants, global compact-agent sizing, style-guard exceptions or custom overlay framework.
- [ ] Menu/tooltip geometry and native focus/IME verified in real windows, not just static paints; both themes inspected.
- [ ] M2 ledger identifies exact accepted native differences and no missing required glyph caller.

Rollback only newly owned family/asset hunks; keep enum/embedding/files synchronized when reversing an icon. Do not remove another person's files or replace all kit files from HEAD. Dependency02; estimate2-3d assumes existing migrated primitives are largely reusable.
