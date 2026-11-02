# Research: ramithuh/protein-design-interactive-slides

Source: https://github.com/ramithuh/protein-design-interactive-slides (cloned at `f09f9a1`, 2026-10-22).
Live demo: https://slides.ramith.fyi. 51 stars, 18 commits, ~4,000 lines of own code (JS + Python).

Read in full: the README, `AGENTS.md`, `web/deck.js`, `web/manim.js`, `tools/check_deck.py`,
`tools/export_pdf.py` and `talks/example/slides/manim_graph.js`. The 3D add-on and design-language
sections come from the file headers and the docs; `deck.css`, `plots.js`, `alphabet.js` and
`design_check.js` were not opened.

## What it is

A template for slide decks drawn live in the browser. There is no build step and no rendered video.
Each slide is a small JS module that draws itself into a 16:9 frame using HTML/SVG for text,
[manim-web](https://github.com/maloyan/manim-web) 0.39.4 (a TypeScript port of manim, bundling
three.js and KaTeX) for click-stepped animation, and an optional 3D add-on for molecules and protein
cartoons. Reveal.js 6.0.1 only supplies navigation, fragments (click steps), the overview and speaker
notes. Everything is vendored, so a deck opens offline from `file://`.

## Architecture

| Layer | File | Role |
| --- | --- | --- |
| Runtime | `web/deck.js` (433 lines) | Slide registry, mount/unmount lifecycle, step sync with Reveal, section footer with topic progress, themes, laser pointer, click-to-advance, WebGL budget |
| Design language | `web/deck.css` | Type scale in `--u` (1/100 of frame width), layout grid, components, light/dark tokens |
| Animation stage | `web/manim.js` (108 lines) | `Deck.manim`: a manim-web scene that plays one step per click |
| Charts | `web/plots.js` | `Deck.groupedBars`, the only chart layout allowed |
| 3D add-on | `web/addons/*.js` (~750 lines) | `turntable` (drag/zoom/rock camera, eased `focus()` moves, `project()` for HTML labels), `molstar` (custom SSAO + outline + fog pass imitating Mol*), `cartoon` (reimplementation of Mol*'s ribbon spline), `atoms` (instanced ball-and-stick, cages) |
| Data prep | `tools/prep_protein.py`, `prep_amino_acids.py` | gemmi/numpy/mdtraj/RDKit turn PDB/mmCIF into `data/*.js` files that set `window.DATA` (because `fetch` fails on `file://`) |
| Checking | `tools/check_deck.py` | Playwright + SwiftShader walks every slide and step in both themes, fails on any console warning, saves a PNG per slide |
| Export | `tools/export_pdf.py` | Same walk, screenshots per slide or per click, stitched into a PDF with Pillow |
| Publish | `tools/build_site.sh`, `deploy_pages.sh` | Static site on Cloudflare Pages; every push to `main` publishes |

## The ideas worth knowing

1. **Slide contract.** `Deck.slide(name, { steps, notes, interactive, smoothBack, mount(frame, ctx) })`.
   `mount` returns `{ enter(step, animate), leave, freeze, destroy }`. `enter` must be idempotent and
   jump-safe: `enter(4, false)` right after mount must equal clicking 1→4. State is written per step,
   not as deltas.
2. **Forward plays, everything else rebuilds.** `Deck.manim` keeps a `build()` that returns fresh
   mobjects and a list of step functions. Going forward plays step k live. Going back or jumping
   clears the scene, calls `build()` again and replays steps 1..k through an "instant" path that calls
   `begin(); interpolate(1); finish(); cleanUpFromScene()` on every leaf animation. The same step code
   serves both paths because `play`/`wait` are swapped for no-op versions. Requests queue, so clicking
   ahead during an animation plays the following steps in order.
3. **Smooth back is opt-in.** `Deck.start({ smoothBack: 'none' | '3d' | 'all' })`. One click back passes
   `animate === 'back'` (play in reverse, about twice as fast); a second click within 700 ms or arriving
   from a later slide jumps. Every reverse must end in the state a jump gives.
4. **WebGL budget.** Browsers keep about 16 contexts. Slides more than `KEEP = 3` away are frozen: canvases
   are captured to ≤800 px WebP stills after every step, then `destroy()` runs and the context is
   force-released with `WEBGL_lose_context`. The next slide is pre-mounted so crossfades never meet a
   half-built frame; the previous one is pre-mounted after 400 ms. The overview builds unvisited slides
   one at a time at their last step and freezes them for thumbnails.
5. **A busy signal for automation.** While an `enter()` promise is pending, the root gets `deck-busy`
   (after 150 ms). `check_deck.py` and `export_pdf.py` wait on that class plus running finite Web
   Animations before screenshotting, which is how a headless checker knows a step has settled.
6. **Agent-oriented docs.** `AGENTS.md` (with `CLAUDE.md` pointing to it) is the real spec: design
   tokens, "never hard-code a colour", "never px for text", manim-web pitfalls (Transform between
   unlike shapes throws "mismatched point counts"), and a mandatory "look at the pictures in both
   themes" check. The repo is built to be extended by a coding agent.
7. **Scientific 3D without a viewer library.** No Mol* code runs; the cartoon and the Mol*-like look are
   reimplemented on manim-web's bundled three.js and tuned side by side against Mol*. Cartoon geometry is
   built in Å and scaled as a mesh, because scaling Cα coordinates would also fatten the ribbons.

## Relevance to fframes

The two projects aim at different outputs (live, clicked deck versus an encoded video), but several
pieces line up with work already in this repo:

- **Manim vocabulary.** The fframes-kit layer recently mapped its explain layer, plots and math figures
  to Manim names (`067687f6`). manim-web's API (`Axes.plot`, `getRiemannRectangles`, `getArea`,
  `Create`, `Transform`, `Write`) is the same vocabulary, so `talks/example/slides/manim_graph.js` is a
  compact reference scene for kit parity.
- **Verification loop.** `check_deck.py` (every step, both themes, fail on warnings, PNG per slide) is
  the browser counterpart of `inspect` + `strip`/`frame` in `fframes::cli`. The "wait for a busy flag,
  then screenshot" pattern is what makes it deterministic.
- **Step model versus timeline.** fframes is time-driven (`frame.animate(&timeline!)`), so it already has
  the jump-safety property the deck has to enforce by hand: any frame is a pure function of time. A
  click-stepped presentation mode for fframes would map steps to scene ranges or timeline marks and hold
  at each mark, which the native player (`fframes_native_player`) could drive.
- **3D.** `fframes::three_d` already covers PBR, shadows, DoF and glTF. The deck's Mol*-style look
  (screen-space AO, depth outlines, matte light) and the ribbon builder are not among the effects its docs list
  (checked against `AGENTS.md`, not the crate source); the eased `focus(point, zoom, ms)` camera move and `project()` for pinned labels
  match `camera.project` in fframes.

## Licensing caveat

The repository has **no licence file** (GitHub reports `licenseInfo: null`). Only the vendored
components carry licences (manim-web, three.js, Reveal.js: MIT; Lato: OFL). The author's own code in
`web/` and `tools/` is therefore all-rights-reserved by default. Studying the design is fine; copying
code into fframes would need the author's permission or a licence added upstream.

## Unresolved questions

- What the goal is on the fframes side: a slide or presenter mode in Studio, porting the Mol*-style look
  or cartoons into `fframes::three_d`, or using this deck format as a kit reference. Each leads to a
  different follow-up plan.
