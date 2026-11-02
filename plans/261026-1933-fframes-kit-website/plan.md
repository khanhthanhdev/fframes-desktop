---
title: "fframes-kit website: landing page and component docs"
description: "Build a static Astro + Starlight site for fframes-kit with a scroll-animated landing page and per-component docs pages, each with prose, compiled code and a rendered demo video."
status: completed
priority: P2
effort: 37h
branch: khanhthanhdev/feat/website (in the fframes-kit repository)
tags: [feature, frontend, docs]
blockedBy: []
blocks: []
created: 2026-10-26
---

# fframes-kit website

Mode: **hard**. The research was local checks rather than researcher agents (see Evidence).
Scope challenge: **HOLD SCOPE**. The plan delivers everything that was asked for and nothing more.

## Outcome

A fast, static website for fframes-kit with two parts:

1. **Landing page.** It uses scroll effects, animation and figures, and the visuals are the kit's own output: real rendered clips, SVG frames, and counts generated from the catalog.
2. **Docs.** Pages are grouped by category. Every component gets its documentation, its code (the same code the docs doctests compile) and a demo video rendered from its gallery scene.

## Constraints (verified, see Evidence)

- The site lives in the fframes-kit repository, `C:\Users\Administrator\fframes-kit\website\`. This plan lives in fframes-desktop only because that is where plans go.
- `fframes-kit/docs/*.md` are doctests (`#[doc = include_str!(..)]` in `fframes-kit/src/lib.rs`). They stay the only source of the docs prose and code. The site generates its pages from them and never copies them.
- Rustdoc hidden lines (`# `, a bare `#`, the `##` escape) must be stripped from the code shown on the site.
- The repository is private and the crate is not on crates.io yet, so there are no "edit this page" or source links and no docs.rs links until it is published.
- There is no CI, and the VM is CPU-only. Media is rendered and the site is built locally on Windows, then deployed from local.
- Gallery renders support only H.264 MP4 (`main.rs` pins `libx264`; `.webm` fails with libav -22).

## Non-goals

- A hand-written API reference. That job belongs to rustdoc/docs.rs, and the site links there once the crate is published.
- Hosting the web editor or a live WASM playground.
- Search beyond the Pagefind index that ships with Starlight.
- Translations.

## Acceptance criteria

- [ ] `pnpm build` in `website/` succeeds with no broken links (`starlight-links-validator`).
- [ ] `node scripts/sync-docs.mjs --check` passes. Every non-bench gallery scene (323 of 333) appears on exactly one docs page, and every `##` docs section has a sidebar category.
- [ ] Every docs page shows its prose, its code with hidden lines stripped, and a poster plus video for each scene it names.
- [ ] The landing page has a hero loop, catalog-derived figures, a sticky code-to-video scroll section, a formula reveal, a kits grid and a CTA. Animations are off under `prefers-reduced-motion`.
- [ ] Lighthouse in headless Chrome on the landing page and one docs page, desktop and mobile: Performance ≥ 95, Accessibility ≥ 95, Best Practices ≥ 95, SEO = 100.
- [ ] Landing page JavaScript is at most 10 KB gzip, with no framework runtime.
- [ ] `just test` in the kit still passes after the docs edits.
- [ ] `just site-deploy` publishes `website/dist` and `https://fframes-kit.pages.dev` serves the site. Videos stream from R2 through `/media/<key>`, and seeking works (206 Range responses).
- [ ] No R2 credential is committed. They come only from `R2_*` environment variables.

## Phases

| # | Phase | Effort | Depends on | Status |
|---|---|---|---|---|
| 1 | [Scaffold the site](phase-01-scaffold-site.md) | 2h | none | completed |
| 2 | [Docs generator and scene coverage](phase-02-docs-generator.md) | 10h | 1 | completed |
| 3 | [Media pipeline](phase-03-media-pipeline.md) | 4h | 1 | completed |
| 4 | [Docs page template and category pages](phase-04-docs-pages.md) | 5h | 2, 3 | completed |
| 5 | [Landing page](phase-05-landing-page.md) | 10h | 3 | completed |
| 6 | [SEO, performance and accessibility](phase-06-seo-performance.md) | 4h | 4, 5 | completed |
| 7 | [Deploy and repository docs](phase-07-deploy-docs.md) | 2h | 6 | completed |

Phases 2 and 3 can run in parallel, and so can phases 4 and 5. Their files do not overlap.

## Architecture

```
fframes-kit/docs/*.md ──sync-docs.mjs──► website/src/content/docs/<category>/<page>.mdx  (gitignored)
        (doctests)          │                    + website/src/generated/catalog.json
                            │ reads
gallery CLI: timeline --json ──render-media.mjs──► website/.media/**/*.mp4 ──--upload (S3 API)──► R2 bucket fframes-kit-media
             render <Scene> --scale 0.5           website/src/assets/scenes/<Scene>.png  (posters, gitignored)        │
             frame <Scene@t> / svg <Scene@t>      website/src/generated/figures/*.svg + media.json (R2 keys)   │ binding MEDIA
                            │                                                                                     ▼
            Astro 7 + Starlight 0.42 ──pnpm build──► website/dist + functions/media ──wrangler pages deploy──► fframes-kit.pages.dev
                                                                                     (/media/<key> streams from R2 with Range)
```

## Evidence (checked 2026-10-26)

- `cargo run --release -p components-gallery -- timeline --json` lists 333 scenes with frame and second ranges. Ten are benchmarks (`Bench*`).
- `render FunctionPlot --scale 0.5 -o fp.mp4`: under 1 s, 192 KB, h264 + aac at 960×540. `moov` comes before `mdat`, so the file is already faststart and needs no remux.
- `render Sequence -o seq.webm` fails with `libav error -22`.
- Scene names are unique (333 of 333) and equal `full_name`. The longest scenes are `TreeView` and `Dijkstra` at 13 s.
- `svg FunctionPlot@end` converts text to paths (0 `<text>`, 8 `<path>`), so inline figures need no webfonts.
- The docs name 75 scenes in "Gallery scene(s):" lines. `scripts/e2e.mjs` has a curated `Scene@time` key frame for about 170 scenes, which phase 3 reuses for posters.
- Tooling: Node 24.21, pnpm 10.15, ffmpeg/ffprobe 9.0 on PATH, Chrome and Edge installed. Versions: astro 7.4.2, @astrojs/starlight 0.42.7 (peer astro ^7.2.10), starlight-links-validator 0.27.0, wrangler 4.152.0.
- `gh repo view`: PRIVATE. `cargo search fframes-kit`: not published.

## Validation Log

### Session 1 (2026-10-26)
- Hosting: **Cloudflare Pages**, built and deployed from the local machine with wrangler. Changes phase 7.
- Media: **Cloudflare R2 bucket** `fframes-kit-media`. There is no custom domain and `r2.dev` URLs are rate-limited, so clips are served same-origin by a Pages Function (`functions/media/[[path]].ts`) bound to R2 with Range support. Posters stay in the site build so Astro optimises them. Changes phases 1, 3, 4 and 7.
- Page granularity: **one page per `##` section**. Each component gets its own heading, prose, video and code inside the page. No restructuring of the doctested docs. Phase 2 is unchanged.
- Domain: **`https://fframes-kit.pages.dev`** for now. A custom domain is one config value later. Changes phases 1 and 7.

### Verification Results
- Claims checked: 14 (CLI commands, timeline JSON, renders, faststart, SVG text, doc wiring, doc headings, e2e key frames, tool versions, repository visibility, crates.io)
- Verified: 14 | Failed: 0 | Unverified: 0
- Tier: Full (7 phases), checked by running the commands, see Evidence.

### Whole-Plan Consistency Sweep
- `public/media` was replaced by `.media/` plus R2 keys in plan.md and phases 1, 3 and 4. The `site` URL is the same in phases 1, 6 and 7.
- The bucket and token are created in phase 3 (its upload check needs them), not in phase 7.
- After the red-team fixes below, no contradictions remain.

## Red Team Review

An advisor review of the drafted plan served as the hard-mode red-team pass.

- **Assumptions:** that scene names are unique. Verified: 333 unique names, and `name` equals `full_name` for every scene. Docs parsing is now fence-aware (phase 2), because docs code contains `# fn main() {`-style hidden lines that a line-based split would treat as headings.
- **Failure:** a route collision between Starlight's `src/content/docs/index.mdx` and the landing page `/`. The docs home moved to `src/content/docs/docs/index.mdx` (phase 1). `wrangler pages deploy website/dist` from the repository root would skip `functions/`, so the deploy runs inside `website/` with no directory argument (phases 3 and 7). Dev media serving uses a Vite `configureServer` plugin instead of Astro middleware. `pnpm build` runs the sync first.
- **Scope:** HOLD. One change reaches beyond the site: phase 2 adds about 250 "Gallery scene" references to `fframes-kit/docs/*.md`. It follows the docs' existing convention, and the references also show in rustdoc.
- **Security:** R2 credentials come only from environment variables. Only `site-media --upload` writes to the bucket, and `site-deploy` never does. The first deploy needs the user's approval.
