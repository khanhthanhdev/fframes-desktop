---
phase: 7
title: "Deploy and repository docs"
status: completed
priority: P2
effort: "2h"
dependencies: [6]
---

# Phase 7: Deploy and repository docs

## Goal
One command renders, builds and deploys the site from the local Windows machine, and the kit's README documents it.

## Files to Create / Modify
All paths are under `C:\Users\Administrator\fframes-kit\`.
- Modify: `justfile`: add recipes `site-media` (`node website/scripts/render-media.mjs`), `site-sync`, `site-dev`, `site-build` (sync, `--check` and build), `site-deploy` (site-build, then `cd website && pnpm exec wrangler pages deploy --project-name fframes-kit` with no directory argument. Wrangler finds `functions/` relative to its working directory, and `wrangler.toml` supplies `dist`.) and `site-lighthouse`. The justfile already runs bash on Windows, so keep the recipes POSIX.
- Modify: `README.md`: a "Website" row in the Layout table and the `site-*` commands in Commands.
- Modify: `.gitignore` (root) only if `website/.gitignore` does not cover the generated paths.

## Tasks & Steps
1. The bucket and token already exist from phase 3. Run `wrangler pages project create fframes-kit --production-branch main` and bind R2 `MEDIA` to the Pages project (production and preview). `wrangler.toml` declares the binding, so check it in the dashboard.
2. Run `just site-media --upload`, then `just site-deploy`. Open `https://fframes-kit.pages.dev` and check `/`, one docs page, a video playing and seeking (a Range request through the Function), `/sitemap-index.xml` and `/robots.txt`.
3. `site` in `astro.config.mjs` is `https://fframes-kit.pages.dev` for now. A custom domain later means changing that one value and rebuilding.
<!-- Updated: Validation Session 1 - Cloudflare Pages on pages.dev, media in R2 -->
- Note: `site-deploy` should not upload media. `site-media --upload` stays a separate step so a deploy never touches the bucket.

## Verification
- `just site-deploy` exits 0 and prints the deployment URL.
- The deployed `/` returns 200 with the expected `<title>` and the canonical URL `https://fframes-kit.pages.dev/`.
- `curl -r 0-99 -sI https://fframes-kit.pages.dev/media/<a key from media.json>` returns 206.

## Risks
- Deploying publishes content. Ask the user before the first deploy. The repository is private, so check that nothing private (local paths, the unpublished roadmap in docs "Not yet available") is meant to stay hidden.
