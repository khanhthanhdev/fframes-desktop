---
title: fframes-kit landing, modern dev-tool layout
status: completed (commits 01f237e, ee66eb8; not pushed or deployed)
created: 2026-10-27
repo: C:\Users\Administrator\fframes-kit (website/, branch khanhthanhdev/feat/website)
builds on: 261026-2306-kit-landing-studio-style (6b34ffc, f4b8120)
---

# fframes-kit landing, modern dev-tool layout

## Outcome

The landing reads like Remotion's or another modern library's page: a centered bold hero with
a large clip player (window chrome, timeline and timecode bound to the video), three pillars
(Rust, agents, Studio), a dark code-to-clip demo with tabs, a design principles grid quoting
the component model, kits as a bento grid, formula, effects row, a stats row, install, FAQ,
footer. Brand stays: Geist, cobalt, light page with dark panels.

## Constraints

- Only real content: no stars, install counts, logos, testimonials or pricing. Every principle
  quotes "The model" in fframes-kit/docs/components.md; snippets come from doctested blocks.
- Existing clips only (media.json); no new uploads. No fake interactivity.
- HERO_SIZES/HERO_WIDTHS match the bigger player so the preload still matches.
- Effects cheap (gradients, masks; no large blur/backdrop-filter); motion gated.
- Gates: build, website tests, link check, Lighthouse perf/a11y/bp >= 95 and SEO 100, no
  horizontal scroll at 390px, screenshots at 1440/1024/820/390.
- No push, no deploy.

## Steps

1. Player component (chrome, timeline, timecode) and the centered hero.
2. Pillars, tabbed demo (replaces the sticky story), principles grid.
3. Kits bento, stats cards, restyled formula/effects/install/FAQ/footer.
4. landing.ts: drop story code, add tabs and player timeline.
5. Verify, commit, advisor audit.
