---
phase: 4
title: "Docs page template and category pages"
status: completed
priority: P1
effort: "5h"
dependencies: [2, 3]
---

# Phase 4: Docs page template and category pages

## Goal
On each docs page, each component reads as a unit: its explanation, then its demo video, then its code. Category index pages show every component as a thumbnail grid.

## Files to Create / Modify
All paths are under `C:\Users\Administrator\fframes-kit\website\`.
- Create: `src/components/SceneVideo.astro`: looks up the scene in `src/generated/media.json` for its `/media/<key>` source (served from R2 by the phase 3 Function) and its poster. It renders a poster `<Image>` (AVIF/WebP, width and height set, `loading="lazy"`) inside a `<figure>` with a `<video preload="none" muted playsinline loop>` that starts when it is half in view and pauses out of view. It has controls, an unmute button (scenes with audio), the scene name and length as the caption, and a "Copy render command" button for `cargo run --release -p components-gallery -- strip <Scene> -n 8`.
- Create: `src/components/SceneGrid.astro`: a grid of category thumbnails that link to page anchors. Hovering or focusing a thumbnail plays its clip.
- Create: `src/components/FeatureBadge.astro`: a ``features = ["math"]`` chip with a copy button, shown under the page title.
- Create: `src/scripts/scene-video.ts`: one shared IntersectionObserver for every video, so no more than 3 play at once. It does nothing under `prefers-reduced-motion`, where only the poster and controls show.
- Modify: `astro.config.mjs`: Starlight `components` override for `PageTitle` (adds the FeatureBadge) and `expressiveCode` settings (theme matching tokens, rust, the copy button).
- Modify: `scripts/sync-docs.mjs`: import `SceneVideo` and `SceneGrid` in the generated MDX, and put `<SceneGrid category=".." />` on the kit index pages.

## Tasks & Steps
1. Build `SceneVideo` and check it on the `FunctionPlot` page: the poster shows first, nothing downloads until the video is near the viewport, playback is muted and loops, and the controls are reachable by keyboard.
2. Build `SceneGrid` and the kit index pages.
3. Check a page with several scenes (`components` "Effects", about 15 scenes). Video bytes should load only when you scroll to them (DevTools Network, or Lighthouse's network requests list in phase 6).
4. Previous/next links come from Starlight's ordering. Check that they follow the category order.

## Verification
- `pnpm sync && pnpm build` exits 0 and the links validator passes.
- On a `pnpm preview` of the Effects page, only the posters load at first. No mp4 loads until you scroll.
