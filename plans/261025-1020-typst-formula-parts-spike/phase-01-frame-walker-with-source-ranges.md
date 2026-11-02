---
phase: 1
title: "Frame walker with source ranges"
status: completed
priority: P1
effort: "2h"
dependencies: []
---

# Phase 1: Frame walker with source ranges

## Goal

Replace `typst_svg::svg` in the spike with a walk over Typst's layout frames that keeps,
for every glyph and rule, the byte range of the source it came from.

## Context

`typst_svg` throws the spans away. The frame tree has them:
- `FrameItem::Group(GroupItem)` holds a transform and a child frame.
- `FrameItem::Text(TextItem)` holds the font, size, fill and `glyphs`, each with
  `span: (Span, u16)`.
- `FrameItem::Shape(Shape, Span)` holds fraction bars, radical overlines and the like.

`WorldExt::range(span)` turns a span into a byte range of its source.

## Files

- Modify: `C:\Users\Administrator\fframes-kit\spikes\typst-probe\src\lib.rs`. Add
  `typeset_glyphs(math, mode) -> Result<Vec<SourcedGlyph>, String>` next to `typeset_svg`.
  `SourcedGlyph` holds the path in pt (y down, as `TexGlyph`), the fill, and
  `source: Option<Range<usize>>`.
- Modify: `C:\Users\Administrator\fframes-kit\spikes\typst-probe\Cargo.toml`. Add
  `ttf-parser` at the version typst-library uses (check `cargo tree -i ttf-parser`), and `kurbo = "0.13"`.
- Modify: `C:\Users\Administrator\fframes-kit\spikes\typst-vs-tex\src\main.rs`. Add a
  `--compare-walker` check.

## Steps

1. Walk the page frame recursively and multiply the group transforms.
2. For each glyph, read its outline with `ttf_parser::Face::outline_glyph` from
   `TextItem.font`, scale by `size / units_per_em`, and place it with the advances and
   offsets. Copy the placement rules from `typst-svg/src/text.rs` instead of inventing them.
3. Turn shapes (`Geometry::Line`, `Rect`, `Curve`) into paths, as stroked or filled.
4. Resolve each glyph's span to a range in `main.typ`. A span from another file (the mitex
   module) or a detached span becomes `None`.
5. Add `to_svg(glyphs, parts, labels) -> String`. It writes the glyphs as paths inside
   `<g id="part-N">` and `<g id="label-NAME">` groups, so that `Formula::from_svg` reads them.

## Verification

```powershell
cd C:\Users\Administrator\fframes-kit\spikes\typst-vs-tex
cargo run --release -- out --compare-walker
```

Pass: for all 22 formulas that typeset in the first experiment, the walker gives the same glyph
count as `typst_svg` + `from_svg`. Each glyph's bounding box differs by at most 0.01 pt, and
the whole formula's ink box by at most 0.01 pt.

## Risk

A glyph that Typst draws as a bitmap or SVG glyph (color emoji) has no outline. That does
not happen with New Computer Modern. Record it as "no outline" instead of failing.
