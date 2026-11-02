---
phase: 1
title: "Scaffold the site"
status: completed
priority: P2
effort: "2h"
dependencies: []
---

# Phase 1: Scaffold the site

## Goal
Set up an Astro 7 + Starlight 0.42 project in `website/` that builds an empty docs section next to a custom landing route.

## Files to Create / Modify
All paths are under `C:\Users\Administrator\fframes-kit\`.
- Create: `website/package.json`: astro 7.4.x, @astrojs/starlight 0.42.x, starlight-links-validator 0.27.x, sharp, wrangler 4.x and @aws-sdk/client-s3 (dev). Scripts: `dev`, `build` (`node scripts/sync-docs.mjs && astro build` from phase 2 on; `astro build` until then), `preview`, `sync`, `media`.
- Create: `website/astro.config.mjs`: `site: "https://fframes-kit.pages.dev"`, Starlight integration with `title: "fframes-kit"`, the sidebar loaded from `src/generated/sidebar.json`, the links-validator plugin, and `customCss`.
- Create: `website/src/content.config.ts`: Starlight `docsLoader()` / `docsSchema()`, extended with the optional frontmatter fields `feature`, `scenes` and `ogImage`.
- Create: `website/src/pages/index.astro`: a placeholder that phase 5 replaces.
- Create: `website/src/styles/tokens.css`: color, type and spacing tokens for both the landing page and Starlight (`--sl-color-*` overrides), with dark and light themes.
- Create: `website/tsconfig.json` (extends `astro/tsconfigs/strict`), `website/.gitignore` (`dist/`, `.astro/`, `node_modules/`, `src/content/docs/docs/**` except `src/content/docs/docs/index.mdx`, `src/generated/`, `.media/`, `.wrangler/`, `src/assets/scenes/`).
- Create: `website/src/content/docs/docs/index.mdx`: the docs home at `/docs/`. Do not use `src/content/docs/index.mdx`, which Starlight would serve at `/` and collide with the landing page. (hand-written: what the kit is and how the docs are organised).
- Modify: `pnpm-workspace.yaml` is not needed. `website/` is a standalone pnpm project because the repository's root `package.json` is npm and only for odiff.

## Tasks & Steps
1. `pnpm create astro@latest website -- --template starlight --no-install --no-git`, then pin the versions listed above and run `pnpm install` in `website/`.
2. Move the docs under the `/docs/` base path. Starlight serves `src/content/docs/` at the root by default, so put the content under `src/content/docs/docs/` and let `src/pages/index.astro` own `/`. Check that `/` and `/docs/` both resolve.
3. Point `customCss` at `tokens.css` and the Starlight logo at an SVG frame of the kit (phase 3 provides it; use a text logo until then).
4. Add the fonts the gallery uses (`DMSans-Medium.ttf`, `JetBrainsMono-Regular.ttf` from `gallery/media`) as self-hosted `woff2`, with `font-display: swap` and a preload only for the hero font.

## Verification
- `cd C:\Users\Administrator\fframes-kit\website; pnpm build` exits 0.
- `pnpm preview --port 4321` (background, stopped afterwards) serves `/` and `/docs/`.
