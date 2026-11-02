---
title: kit landing on a real design system, without the generated look
status: completed (52c3a83, 9edff81; not pushed)
created: 2026-10-27
repo: C:\Users\Administrator\fframes-kit (branch khanhthanhdev/feat/website)
---

# Kit landing on a real design system

User: "The landing page is not good, do not have design system, very bad gradient and AI Slop design".

## Outcome

1. One design system file, `website/src/styles/system.css`: colour primitives and semantic
   roles, a fixed type scale, a 4 px spacing scale, two radii, one border, no shadows, motion
   durations. Landing and docs both read it (docs keep their dark theme through tokens.css).
2. The landing page uses only those tokens: no literal colours, sizes or radii in landing.css.
3. Removed: gradient headline, glow blobs, dot and line grids, radial-lit dark section, two-tone
   headlines, mono uppercase eyebrow on every section, traffic-light window chrome, 01/02/03
   numbering, pill buttons and badges, card-per-item grids, the six stat boxes and the count-up.
4. Content stays: hero clip and teaser dialog, code-to-clip tabs, the principles, the seven
   kits, MathTex, effects, install (crates.io), agent links, FAQ, footer.
5. The og image follows the same system (no gradient, no grid).

## Direction

Neutral paper and ink, so the clips carry the colour. One accent, used for links, focus and
the primary button only. Left-aligned editorial layout on a 12-column grid, hairline rules
between sections instead of boxes. Media sits in plain dark wells with a mono caption line
(scene name, kit, length), like a contact sheet. Code panels are the only other dark surface.

## Sections

Hero (headline, lede, two actions, install line, facts line, clip) / Ways to use it (three
columns, rules between) / Code to clip (underline tabs) / Principles (2x3 text grid, rules,
code under each) / Kits (uniform contact sheet) / MathTex / Effects strip / Install / FAQ / footer.

## Acceptance

- The system is the kit's default `Theme` (fframes-kit/src/theme.rs) on paper: its colours for
  dark surfaces and the accent, its type scale at half size (48/28/18/14), its spacing
  (4/8/16/24/32/64/96), radius 4/8 and 1 px dividers. Paper neutrals are added for text on white.
- Plain descriptive headings. No two-beat slogans; the "Remotion's model, Manim's names" line
  appears once (hero); the footer callout goes.
- Grep check on landing.css and the og card: no `gradient(`, `blur(`, `999px`, uppercase
  transforms, hex or rgba literals, and no `box-shadow` except focus rings. No font size outside
  the scale. Literals live only in system.css.
- No scroll reveals, no count-up (their JS goes too).
- tokens.css maps Starlight to system.css; docs.css loses its pills.
- landing.css is written from an empty file.

## Constraints

- No push, no deploy. Landing light only; docs keep both themes.
- Gates: pnpm build, pnpm test, Lighthouse >= 95 and SEO 100 on `/` and a docs page in both
  themes, no horizontal scroll at 390 / 820 / 1440, tabs with and without reduced motion.
