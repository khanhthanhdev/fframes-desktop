---
phase: 6
title: "SEO, performance and accessibility"
status: completed
priority: P2
effort: "4h"
dependencies: [4, 5]
---

# Phase 6: SEO, performance and accessibility

## Goal
Every page has correct metadata and the landing page has structured data. The Lighthouse thresholds in plan.md pass on the landing page and one docs page, on desktop and mobile.

## Files to Create / Modify
All paths are under `C:\Users\Administrator\fframes-kit\website\`.
- Create: `src/components/Head.astro`: a Starlight `Head` override that sets `og:image` and `twitter:image` from the page's first scene poster (1200×630 made by `sharp` in `sync-docs.mjs`), or the site default.
- Create: `public/robots.txt` (with the sitemap URL) and `public/og-default.png`.
- Modify: `src/layouts/Landing.astro`: JSON-LD `SoftwareSourceCode` (name, description, programmingLanguage Rust, license MIT; `codeRepository` only once the repository is public).
- Modify: `scripts/sync-docs.mjs`: write the per-page OG image path into frontmatter.
- Create: `scripts/lighthouse.mjs`: runs `lighthouse` (pinned, `pnpm dlx`) in headless Chrome against `pnpm preview`, on `/` and `/docs/kits/math/function-plot/` (or whatever slug is generated), desktop and mobile, and fails below the thresholds.

## Tasks & Steps
1. Check the Starlight defaults: sitemap (`@astrojs/sitemap` comes with Starlight when `site` is set), canonical URLs, `<title>` as "Page | fframes-kit", and meta description from frontmatter.
2. Add the OG images and JSON-LD. Validate the JSON-LD with a local parse of the built HTML: it must be valid JSON and have `@type`.
3. Start `pnpm preview --port 4321` in the background, run `node scripts/lighthouse.mjs`, fix what fails, and stop the preview server.
4. Accessibility: videos are decorative (aria-hidden) or labelled, every poster has alt text naming the component, focus is visible on cards and marquee items, and color contrast passes in both themes.

## Verification
- `node scripts/lighthouse.mjs` exits 0: Performance ≥ 95, Accessibility ≥ 95, Best Practices ≥ 95, SEO = 100, on each URL and form factor.
- `dist/sitemap-index.xml` exists and lists every docs page.
