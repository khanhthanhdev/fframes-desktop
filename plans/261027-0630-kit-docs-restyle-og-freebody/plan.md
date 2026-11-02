---
title: kit docs restyle, crates install line, og image, FreeBody text colour
status: completed (41dfcbf, f7e8fd1, 4cffa4a, 80a2da6; not pushed)
created: 2026-10-27
repo: C:\Users\Administrator\fframes-kit (branch khanhthanhdev/feat/website)
---

# Kit docs restyle, crates install line, og image, FreeBody text colour

## Outcome

1. FreeBody's `text_color` colours the force and net labels (it had no effect).
2. Install commands use crates.io (`cargo add fframes-kit`), the user publishes soon: revert
   db6cf60 (skill, llms intro) and the landing's `--git` command and status note. README
   stays as it is.
3. The Starlight docs share the landing's look: Geist and Geist Mono, cobalt on white in
   light, the night panels' palette in dark, rounded corners, the brand logo. Both themes stay.
   DM Sans and JetBrains Mono leave the site.
4. A new light og-default.png (1200x630) in the landing style.

## Constraints

- No push, no deploy. Gallery snapshot for FreeBody updated with `just e2e cpu --update`
  if it changes; site clips are not re-uploaded (the FreeBody docs clip stays old until the
  next `site-media --upload`).
- Gates: just test / clippy for the kit, website build, tests, Lighthouse >= 95 / SEO 100 on
  `/` and a docs page in both themes.
