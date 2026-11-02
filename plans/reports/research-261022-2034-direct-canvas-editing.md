# Research: direct canvas editing in fframes Studio

Question: when a user selects an element on the preview canvas, can Studio let them drag it,
change its font, its color and similar properties directly, instead of describing the change to
the agent? How would that be built?

**Answer:** yes, for objects that carry an `EditorObjectKey`. The workable design is a sidecar
override document keyed by that identity, applied by the framework at render time. The design
copies how `timeline.json` already works. Writing straight into the Rust source does not work as
the main path, because most `svgr!` values are Rust expressions, and every write needs a
recompile.

## 1. What exists today

| Piece | Where | What it gives direct editing |
|---|---|---|
| Stable object identity | `fframes/src/editor.rs` (`EditorObjectKey`, `render_id()`), `Svgr::with_editor_object` in `fframes/src/svgr.rs` | Every editable object is already a `<g id=…>` wrapper with a reversible ID. The ID is the natural key for an override. |
| Per-frame geometry | `EditorGeometryWalker` in `fframes/src/editor.rs`, `editor_frame_v1` worker capability | Bounds, paint order, parent, style tokens and support label (`ExactBounds` / `ApproximateBounds` / `Unsupported`) for each object, atomically with the displayed frame |
| Hit testing and viewport | `desktop/crates/studio-engine/src/canvas_selection.rs` (`CanvasViewport`, `hit_test`) | Pointer to video-pixel mapping with zoom, pan and letterbox. Drag deltas can be converted the same way. |
| Selection UI | `desktop/crates/studio-ui/src/canvas_view.rs` | Click, cycle, rectangle. Today a selection only scopes the next agent task: "Selections do not edit source" (`docs/desktop/README.md`). |
| Live, no-rebuild document | `fframes::TimingDoc`, `PreviewRequest::SetTiming` in `fframes-studio-runtime/src/preview_worker.rs`, `cli::new(..).timing_file(..)` in `fframes/src/renderer/cli.rs`, `--timing-file` in `fframes-studio-runtime/src/export.rs`, engine mirror in `desktop/crates/studio-engine/src/timing/` | The precedent: a JSON document outside the code, applied by the worker without recompiling and read by preview, CLI and export alike, so they all produce the same frames |
| Design tokens | `fframes::Styles` (`fframes/src/styles.rs`), `style/tokens.json` + `style/overrides.json`, `preset_panel.rs`, `studio-engine/src/preset_state.rs` | Colors, typography and spacing as named tokens. The template loads them with `include_str!`, so a token change commits files and then needs a normal **cargo rebuild** before the preview updates. |
| Inspector widgets | `desktop/crates/studio-ui/src/kit/`: `color_picker`, `number_field`, `slider`, `select`, `segmented` | Most of the property panel can be built from existing kit parts. |

The main gap is timing. Trims and reordering are live through `SetTiming`. Visual properties go
through a Rust compile: a token edit triggers a rebuild, and an agent edit triggers a rebuild
plus validation. Free dragging needs a result within about one frame. It cannot wait for a
compile on every pointer move.

## 2. How other tools do it

| Tool | Mechanism | What carries over |
|---|---|---|
| **Remotion Studio** ([visual editing](https://www.remotion.dev/docs/visual-editing), [saveDefaultProps](https://remotion.dev/docs/studio/save-default-props)) | The author declares editable props with a Zod schema, and Studio generates controls from it (`zColor()`, numbers, enums…). Saving rewrites the root file by static analysis, and **only works when default props are inlined** in `<Composition>`. A render can also use unsaved values as input props. | Code declares what can be edited. Writing back into code only works on literal values, and Remotion accepts that limit. |
| **Theatre.js** ([projects](https://www.theatrejs.com/docs/0.5/manual/projects), [core API](https://www.theatrejs.com/docs/0.5/api/core)) | `sheet.object("key", { x: 0, color… })` declares editable props with defaults. Studio edits a JSON state, and production loads the exported state with `getProject(name, { state })`. Studio is dev only. Same key = same object. | Code gives the defaults, a JSON document gives the edits, and stable keys connect the two. This is the closest match to `EditorObjectKey` + `TimingDoc`. |
| **Onlook** (React visual editor; [LogRocket overview](https://blog.logrocket.com/onlook-react-visual-editor), [repo docs index](https://docsearch.algolia.com/mcp/docs/repo/onlook-dev/onlook)) | Instruments JSX with a per-element ID attribute that maps each DOM node to its source location, then rewrites the matching AST node (JSX attributes, Tailwind classes) and relies on hot reload. Third-party sources disagree on the attribute name and on the patching details; not verified against the repo. | Direct source rewriting works when the language reloads quickly and styles are declarative strings. Neither holds for Rust and `svgr!`. |

## 3. Options for fframes

### A. Sidecar override document (recommended)

A new versioned document, for example `studio/overrides.json`, keyed by the object's render ID:

```json
{ "version": 1, "objects": {
  "<render_id of intro/starter-title/headline/primary>": {
    "translate": [120, -40], "scale": 1.0, "rotate": 0,
    "fill": "#ffcc00", "font_family": "DM Sans", "font_size": 96, "font_weight": 700
  } } }
```

- **Framework (`fframes`)**: `OverrideDoc` next to `TimingDoc`; `Previewer::set_overrides`,
  `RenderOptions::overrides`, `cli::new(..).overrides_file(..)`. After `render_frame`, a pass
  finds `<g id=render_id>` nodes in the `Svgr` tree and applies the overrides. Preview, CLI,
  export and the agent's candidate validation must all read the file. Otherwise exported frames
  and the agent's evidence frames will not match what the user sees. `timeline.json` already
  meets this requirement through `timing_file` and `--timing-file`.
- **Worker**: `PreviewRequest::SetOverrides { generation, json }`, like `SetTiming`. The
  previous document stays in force if the new one is invalid.
- **Engine**: a mirror type plus a durable write through the controller's file-set transaction,
  the same way `preset_state.rs` and the timing document are written. Undo and redo of
  override edits work like timeline edits.
- **UI**: drag handles on the selected object's bounds in `canvas_view.rs`, which only send
  `SetOverrides` while dragging and write the file on release. An inspector panel with
  position, font (`select`), size and weight (`number_field`), and color (`color_picker`).

What can be generic and what cannot:

- **Move, scale and rotate are generic.** The pass inserts **its own** `<g transform=…>` directly
  outside the keyed `<g id=…>` wrapper, instead of setting `transform` on the wrapper. Code may
  already put an animated `transform` on that group, as AGENTS.md recommends, and a second
  attribute would replace it. Placed outside, the offset is not rotated or scaled by the
  object's own animation, so a drag of 100 px moves the object 100 px on screen. No code changes
  are needed, and static-subtree caching survives because the cached children are untouched.
- **Units.** The document stores offsets in **video pixels**, the unit `CanvasViewport` gives
  the drag. The pass converts them into the parent's user units through the inverse of the
  viewBox scale (see `tree_rect` in `fframes/src/editor.rs`) and any ancestor transforms.
- **Selection follows.** The geometry walker runs on the converted tree, so the hit-test bounds
  of a moved object already include the override. No separate bookkeeping is needed.
- **Color and font are not generic on the wrapper.** SVG inheritance does not override a child's
  own attribute, and the template's `<text … fill={*text} font-family=…>` sets its own. Setting
  `fill` on the wrapper does nothing. The override pass has to rewrite the attributes of
  descendant `<text>` and shape nodes. It also has to clear `static_hash` on each changed node
  **and on every static ancestor up to the keyed group**. The Skia renderer caches a recorded
  picture of a whole static group, and paints, under that hash
  (`fframes-skia-renderer/src/render/mod.rs`). A stale ancestor hash would redraw the old
  picture. That is more code, and it needs rules for what "the color" of a multi-colored
  object means. Expose these fields only when an object has one clear text or fill target, or
  let the author declare them (option C).
- **Text layout.** Changing font or size after layout was measured (`text_fit`,
  `text_break_lines`) does not re-flow the text that was already laid out. The pass only changes
  how the text is drawn. If layout matters, the font has to come from a declared prop or token
  that the code reads before measuring.

### B. Write the Rust source directly (Remotion and Onlook style)

The source index (`studio-project/src/source_index.rs`) and anchors can locate the
`render_frame` that holds a marker. Writing a value back fails in most cases:

- Attributes are mostly expressions (`x={*margin}`, `fill={*text}`), not literals. There is
  nothing to rewrite without understanding the code. Remotion has the same limit and only saves
  inline literal defaults.
- `svgr!` input is a macro token stream, not markup `syn` can edit attribute by attribute.
- Every write means a cargo rebuild before the preview changes.

It is reasonable as an optional **"Bake into code"** step for literal attributes only, never as
the live path.

### C. Declared editable props (Theatre.js style, complements A)

The author marks what is editable in code, with defaults that the document overrides:

```rust
let p = ctx.props(&self.title_key);          // reads the override doc
<text x={p.f32("x", *margin)} fill={p.color("fill", *text)} font-size={p.f32("size", title.size)} …>
```

This handles everything A cannot do generically: exact color and font targets, text measured
with the edited font, values inside animations. It needs a `fframes` API and a template update,
and only works in projects that adopt it. A generic transform (A) plus declared props (C) for
the rest covers both new and existing projects.

### D. Through the agent (exists today)

A drag or a picker change becomes a structured, scoped task ("move headline by +120, −40 px,
set color to #ffcc00"). It needs no runtime work and handles any code, but takes seconds to
minutes and the result is not deterministic. Its best use is to **bake** sidecar overrides into
real code when the user asks, then clear them from the document.

## 4. Recommended direction

1. Add an override document for transforms only, using the timing-document pattern end to end
   (framework type, worker request, engine durable write, export flag), with drag handles on
   the canvas. This works on every keyed object without code changes.
2. Add the inspector panel. Font and color go through a declared-props API (C) for new
   templates. For token-bound objects, offer "edit token" (changes every object using it) next to
   "this object only" (an override).
3. Add "Bake into code" through the agent, with literal rewriting as an optional fast path.

## 5. Constraints and risks

- **Source of truth.** `docs/desktop/README.md` says "Rust source remains the authority for the
  video". A sidecar document weakens that. `timeline.json` is the accepted precedent, and the
  agent must see the overrides in its context so it does not fight them.
- **Animated objects.** A constant offset on the wrapper is added on top of the code's
  animation in every frame. Per-scene offsets or keyframed overrides are a larger design.
- **Unannotated objects** have no key. Objects without one fall back to the existing
  scene, rectangle or agent task.
- **Fonts** must come from the project's `media/` folder for reproducible renders (AGENTS.md).
  The picker lists only bundled families, with an "add font" import.
- **Web editor (WASM)** uses the string form of `Svgr`. The override pass needs that form too,
  or the web preview will not match the native render.
- **Stale keys.** If code renames a key, its override no longer matches anything. Report it like a
  timing clip with an unknown key, and do not remap it silently.

## Unresolved questions

1. Should overrides live in a sidecar document at all, given "Rust source is the authority"? Or
   should every visual edit be baked into code?
2. When an animated object is dragged, should the offset apply to all frames, to the current
   scene only, or become a keyframe?
3. When the user changes a token-bound color, should the default edit the token (every object
   using it) or only the selected object?
4. Is a code change to templates (declared props, option C) acceptable, or must direct editing
   work only on existing projects without code changes?

## Sources

- [Remotion: visual editing](https://www.remotion.dev/docs/visual-editing)
- [Remotion: saveDefaultProps](https://remotion.dev/docs/studio/save-default-props)
- [Theatre.js: projects](https://www.theatrejs.com/docs/0.5/manual/projects)
- [Theatre.js: core API](https://www.theatrejs.com/docs/0.5/api/core)
- [LogRocket: Onlook React visual editor](https://blog.logrocket.com/onlook-react-visual-editor)
- [Onlook repository docs index](https://docsearch.algolia.com/mcp/docs/repo/onlook-dev/onlook)
