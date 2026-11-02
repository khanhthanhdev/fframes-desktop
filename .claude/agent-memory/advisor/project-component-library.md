---
name: project-component-library
description: fframes component-library initiative (Remotion/Manim-like), its research reports under plans/reports, and the audit findings from 2026-10-23
metadata:
  type: project
---

The user is designing an `fframes-components` crate (Remotion-like component model, Manim vocabulary) through a chain of research reports in `plans/reports/`: `components-261023-0753-overview-map.md`, `components-261023-0804-library-strategy.md`, `research-261023-1404-manim-api-for-fframes.md`, `research-261023-1700-component-model-remotion-manim.md` (the last one claims to supersede the "how a video uses a component" sections of the others).

**Why:** the user wants a web-framework-like model: import a component, set its props, many components, editable in Studio later. Nothing of it exists in code yet as of 2026-10-23.

**How to apply:** when advising on this work, check these verified core constraints before accepting an API sketch: `Frame::text_width/text_fit` need `value: &'a str` tied to the context lifetime (examples leak strings to work around it); `Frame`'s `TextCache` is per-worker `Rc<RefCell<LruCache>>`, not a shared lock; `EditorObjectKey::new` requires all four keys non-empty (`scene_instance_key`, `component_key`, `object_key`, `repeat_key`); the `svgr!` proc macro runs `nodes_to_svgtree` whenever `compile-time-svgtree` is on regardless of target, so any tag pre-pass must sit in `svgr()` before `create_svgr_ident`.
