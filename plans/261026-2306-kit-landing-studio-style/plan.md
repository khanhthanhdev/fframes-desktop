---
title: fframes-kit landing in the Studio style
status: completed (commits 6b34ffc, f4b8120; not pushed or deployed)
created: 2026-10-26
repo: C:\Users\Administrator\fframes-kit (website/, branch khanhthanhdev/feat/website)
---

# fframes-kit landing in the Studio style

## Outcome

https://fframes-kit.pages.dev/ looks like a sibling of landing/studio: ice-white paper, navy
ink, cobalt `#2864F5`, Geist and Geist Mono, hairline rules, 4px corners, small mono uppercase
eyebrows, light 450-weight headings with tight tracking, a sticky figure beside numbered
story steps, a powder-blue install band with a navy terminal card, a FAQ, and the Studio
footer (callout, link columns, bottom line). The kit's own content stays: rendered clips,
the four story steps with code, numbers, formula, seven kits, effects row, teaser.

## Constraints

- Only the landing (src/pages/index.astro, src/layouts/Landing.astro,
  src/components/landing/*, src/styles/landing.css, src/scripts/landing.ts, fonts). The
  Starlight docs keep their own theme (tokens.css untouched).
- Light only, like Studio; the landing drops its theme toggle. The docs theme key is untouched.
- Geist fonts copied from landing/studio/fonts with their OFL licence files.
- Install commands use the git form (`cargo add fframes-kit --git ...`): the crate is not on
  crates.io (404), and the skill already says so.
- FAQ claims only what the repo docs state.
- Gates: pnpm build (links valid), website tests, Lighthouse perf/a11y/best practices >= 95,
  SEO 100 on `/`, at 390px no horizontal scroll.

## Non-goals

- Studio's generated SVG diagrams and scroll-scrubbed scenes (the kit has real clips).
- Restyling the docs pages.

## Steps

1. Fonts + tokens + header/footer in Landing.astro.
2. Hero + story merged into one `.demonstration` (rail left, sticky clip figure right with
   heading, readout, step nav); mobile stacks with inline clips.
3. Numbers band, formula split, kits as numbered principles, effects row.
4. Install band (terminal card + agent card) and FAQ.
5. landing.ts: drop theme toggle, keep clip/story/marquee/copy/teaser; readout text per step.
6. Build, Lighthouse, screenshots at 1440 and 390, commit.
