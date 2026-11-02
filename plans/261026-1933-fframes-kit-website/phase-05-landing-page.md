---
phase: 5
title: "Landing page"
status: completed
priority: P1
effort: "10h"
dependencies: [3]
---

# Phase 5: Landing page

## Goal
A landing page at `/` that shows what the kit makes, using the kit's own clips and figures, scroll-driven effects and counts generated from the catalog. It stays under the performance budget.

## Files to Create / Modify
All paths are under `C:\Users\Administrator\fframes-kit\website\`.
- Modify: `src/pages/index.astro`: the page, built from the section components below.
- Create: `src/layouts/Landing.astro`: head (title, description, canonical, OG, JSON-LD from phase 6), header nav (Docs, Gallery, GitHub when public), footer.
- Create: `src/components/landing/Hero.astro`, `Figures.astro`, `ScrollStory.astro`, `FormulaReveal.astro`, `KitsGrid.astro`, `EffectsMarquee.astro`, `Cta.astro`.
- Create: `src/styles/landing.css`: section layout and the scroll-driven animations.
- Create: `src/scripts/landing.ts`: the IntersectionObserver fallback, count-up, ScrollStory step switching, and video play/pause in view. Budget: 10 KB gzip or less.

## Sections
1. **Hero.** Headline, one-line pitch ("Remotion's model, Manim's names, LaTeX by built-in Typst"), a copy button for `cargo add fframes-kit --features math,latex`, and links to "Read the docs" and "Watch the teaser". The hero video is the at most 6 s muted teaser loop. Its poster is the LCP image, preloaded at `fetchpriority="high"`. "Watch the teaser" opens the full 46 s teaser with sound in a `<dialog>`.
2. **Figures.** A strip of counts read from `catalog.json` (components, scenes, kits, doctested examples, renderers). The numbers count up once when the strip comes into view. The final values are in the HTML for SEO and for no-JS visitors.
3. **ScrollStory.** On the left, 4 steps, each with a short code sample: `Sequence` + `FadeIn`, `Theme`, `ui!` tag form, a kit figure. On the right, a sticky panel plays that step's clip. The active step follows the scroll position. On narrow screens the steps stack with their clip inline.
4. **FormulaReveal.** An inline SVG from `svg MathTex@..` (text as paths). A clip-path or mask moves with `animation-timeline: view()`, so the formula draws as you scroll.
5. **KitsGrid.** Seven cards (math, physics, chemistry, CS, ML, biology, 3D), each with its showpiece clip (`FourierWord`, `ChaosFan`, `MoleculeParade`, `RadialSort`, `NeuralRelay`, ..., chosen in `media.config.json`). A clip plays in view and pauses out of view, and each card links to its kit's index page.
6. **EffectsMarquee.** A horizontal strip of transition and effect posters that drifts with scroll (`animation-timeline: scroll()`). Hovering or focusing a poster plays it, and each links to its docs anchor.
7. **Cta.** The getting-started command and links.

## Animation rules
- Use CSS scroll-driven animations inside `@supports (animation-timeline: view())`. Where they are not supported, `landing.ts` adds `.is-visible` from an IntersectionObserver and a CSS transition runs.
- Animate only `transform`, `opacity` and `clip-path`. No layout properties.
- Under `@media (prefers-reduced-motion: reduce)`, animations are off and videos do not autoplay (posters only, with a play button).
- No more than 3 videos play at once on the page, using the same policy as `scene-video.ts` (share the module).

## Tasks & Steps
1. Build the sections in order with real media from phase 3. Check each one at 375 px, 768 px and 1440 px wide, in both themes.
2. Measure the JavaScript size: `pnpm build`, then gzip the landing page's entry chunk in `dist/_astro/` and check it is 10 KB or less.
3. Check with reduced motion emulated in Chrome DevTools.

## Verification
- `pnpm build` exits 0.
- A Lighthouse pre-check on `/` (the full gate is in phase 6).
- With reduced motion emulated, nothing moves and nothing autoplays.

## Risks
- Several autoplaying videos hurt mobile performance. The in-view-only policy, `preload="none"` and the 3-at-once limit address this, and phase 6 measures it.
- The ScrollStory sticky panel on iOS Safari: test the stacked fallback on narrow screens.
