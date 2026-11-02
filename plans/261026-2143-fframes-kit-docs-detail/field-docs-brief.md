# Brief: doc comments for component fields

The website (https://fframes-kit.pages.dev) now ends every component page with a props
table built from rustdoc: field, type, theme default, and the field's `///` comment as the
description. 2832 public fields of components have no comment, so the Description column is
empty. You write those comments, for the files you own.

## Input

`reports/undocumented-fields.json` in this plan folder: an array of
`{ component, field, type, default, file, line }`. `file` is relative to the repo
`C:\Users\Administrator\fframes-kit`, `line` is where the field is declared (1-based, as of
the start of this phase, so lines shift as you insert comments: work from the bottom of each
file up, or search for the declaration). Take only the entries whose `file` you own.

Where the declaration is:

- A normal struct: `pub field: Type,` on that line. Put `/// ...` on the line above it, after
  existing attributes' position rules (doc comment above `#[...]` attributes is fine).
- A macro invocation (`shape! { Arrow(t) { x1: f32 = 0.0, .. } }`,
  `filter_effect!( Blur(..) { radius: Anim<'a, f32> = .. } ..)`): put `/// ...` above the
  `field: Type = default,` entry inside the invocation. The macros accept `$(#[$fmeta:meta])*`
  per field, so doc comments work there. If several fields share one line
  (`{ amount: Anim<'a, f32> = Anim::Fixed(1.3) }`), break the entry onto its own lines.
- A field every struct of a macro has (`fill`, `stroke`, `stroke_width`, `dash`, `opacity`,
  `enter`, `exit`, `emphasis` in `shapes/mod.rs`; `key`, `children` in `effects/mod.rs`):
  one comment in the macro definition's `pub struct $name { .. }` covers all of them.
- Several entries can point at the same line (the same struct reached twice): write it once.

## How to write them

1. Read the code that uses the field (the component's `render`, helpers it calls) before
   writing. Say what the field does to the picture or the timing, not what its type is.
2. Give the unit when there is one: pixels of the canvas, units of `ui.space` (the explain
   layer and the kits: Manim-like units, the frame is 8 units tall, y up), seconds (local
   to the parent container), degrees or radians (check!), 0..1 fractions, per second.
3. Say what `None`, `0`, empty or `false` means when it is special
   ("`None` for no label", "0 never leaves", "empty for none").
4. One line where possible, two or three when the behavior needs it. Present tense, plain
   words, no "This field", no "The X of the Y" filler when a shorter phrase says it.
5. Backtick every identifier, field name, type and code value: clippy's pedantic
   `doc_markdown` lint fails on bare CamelCase or snake_case words (`ValueTracker`,
   `run_time`). Words like "x-axis" are fine. Use [`Type`] links only for types in scope.
6. Write "colour" (the kit's spelling), "centre" is NOT used: the kit writes "center".
7. Never change code, only add comment lines (and split a one-line macro field entry onto
   its own lines if needed). Do not touch fields that already have docs. Do not reformat.

## Shared meanings (keep these consistent across the kit)

Use these wordings (adapt them, but keep the sense) unless the code does something else:

| Field | Comment |
| --- | --- |
| `enter: Option<EnterAnim>` | How it appears; `None` shows it from the start. |
| `exit: Option<ExitAnim>` | How it leaves at the end of its container; `None` stays. |
| `enter: Option<FadeIn>` / `exit: Option<FadeOut>` | Fade in at the start / fade out before the end; `None` for none. |
| `emphasis: Option<Emphasis>` | An emphasis animation (`Indicate`, `Wiggle`, ..) played while it is shown. |
| `delay: f32` | Seconds after the container's start before it begins. |
| `run_time: f32` | Seconds the animation takes. |
| `rate_func: RateFunc` | Easing of the animation over its run time. |
| `opacity: f32` | 0 transparent to 1 opaque. |
| `stroke_width: f32` | Outline width in pixels (or units, check). |
| `fill` / `stroke: Option<Color>` | `None` leaves the inside empty / draws no outline. |
| `style: TextStyle` | The font: family, size in pixels and weight. |
| `center: (f64, f64)` | The middle, in units of `ui.space`. |
| `x`, `y` (f32) | Position in pixels of the canvas (say which point: top left, centre of, baseline..). |
| `key: Option<&str>` | Names it when two of the same kind share a frame. |
| `children: Children` | What it draws, wraps or times (a closure taking the `Ui`). |
| `*_color` / `*_fill` | Colour of the named part. |
| `decimals` | Digits after the decimal point of written numbers. |
| `label` | Text written beside it; empty or `None` for none. |
| `*_enter` / `*_exit` | Like `enter`/`exit`, for the named part only. |

## Do not

- Do not run `cargo`, `just`, or `node scripts/sync-docs.mjs` (several agents share the
  build directory; the lead runs clippy, tests and the coverage check once at the end).
- Do not edit files outside your list, commit, or switch branches. PowerShell only.

## Finish

Check your files yourself: every entry of yours in the JSON has a `///` line right above its
declaration (search for it). Then report: files changed, number of comments written, any
field whose meaning you could not work out from the code (listed, not guessed), and
`Status: DONE | DONE_WITH_CONCERNS | BLOCKED`.
