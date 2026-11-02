---
phase: 3
title: "Media pipeline"
status: completed
priority: P1
effort: "4h"
dependencies: [1]
---

# Phase 3: Media pipeline

## Goal
Render a demo clip, a poster and (for the landing page) SVG figures for gallery scenes with the gallery's own CLI. Rerunning it renders only what changed.

## Files to Create / Modify
All paths are under `C:\Users\Administrator\fframes-kit\`.
- Create: `website/scripts/render-media.mjs`.
- Create: `website/media.config.json`: the poster times that differ from the e2e list, the landing clips (hero range, kit showpieces, scroll-story steps) and the SVG figures.
- Generated (gitignored): `website/.media/scenes/<Scene>.mp4` and `website/.media/landing/*.mp4` (render output, uploaded to R2, never deployed as static files), `website/src/assets/scenes/<Scene>.png`, `website/src/generated/figures/<Scene>.svg`, `website/src/generated/media.json` (scene to R2 key).
- Create: `website/functions/media/[[path]].ts`: a Cloudflare Pages Function bound to R2 (`MEDIA`). It answers `GET`/`HEAD` with `Range` support (206 responses), `ETag`, `Content-Type: video/mp4` and `Cache-Control: public, max-age=31536000, immutable`, and returns 404 for unknown keys.
- Create: `website/wrangler.toml`: `pages_build_output_dir = "dist"` and the `[[r2_buckets]]` binding `MEDIA` to `fframes-kit-media`.

## Design
- **Scenes.** Take the scene list from `timeline --json` (the gallery's release build), without `Bench*`.
- **Clips.** `components_gallery_bin render <Scene> --scale 0.5 -o <Scene>.mp4` produces 960×540 h264 + aac with faststart already in place (verified). Call the built binary in `target/release` directly so cargo isn't run 323 times.
- **Posters.** `frame <Scene@t> --scale 0.5 -o <dir>`. `t` is the scene's time in `scripts/e2e.mjs` (parse the `frames:` string), else a `media.config.json` override, else `@50%`. PNGs go to `src/assets` so Astro's `<Image>` makes AVIF/WebP with width and height.
- **Hero loop.** Render the teaser crate's range (`cargo run --release -p teaser -- render <a>..<b> --scale 0.5 -o hero.mp4`, at most 6 s), then `ffmpeg -an -c:v copy` to drop the audio. Also render the full 46 s teaser at 1080p for click-to-play.
- **Figures.** `svg <Scene@t>` for the landing formula reveal and the kit cards. Strip the fixed width and height, keep the `viewBox`, and run SVGO with path precision set to 2.
- **Incremental.** `media.json` stores, for each scene, its seconds, frame range and a hash of the gallery binary's mtime plus the scene's source file (`gallery/src/scenes/<module>.rs`). A scene renders again when its hash changes, or with `--force` / `--only <Scene,..>`.
- **Timing.** `media.json` records each scene's render time. `media.config.json` can list slow scenes to skip in a quick run (`--skip-slow`). The longest scenes are `TreeView` and `Dijkstra` at 13 s, and the 3D scenes render on the CPU. The first full run measures the real total.
- **Parallelism.** Run `os.availableParallelism() / 4` renders at once, because each render already uses several threads.
- **Budget.** A clip is about 100–200 KB, so all of them come to about 50 MB. Warn when a clip is over 1.5 MB.
- **Upload to R2** (`--upload`). Put every mp4 whose hash changed into the R2 bucket `fframes-kit-media` under `scenes/<Scene>-<hash8>.mp4` and `landing/<name>-<hash8>.mp4`. The hash in the key lets the files be cached forever. Upload through R2's S3-compatible API (`@aws-sdk/client-s3`) with `R2_ACCOUNT_ID`, `R2_ACCESS_KEY_ID` and `R2_SECRET_ACCESS_KEY` from the environment. Never commit them; the script fails with a clear message if they are missing. `media.json` records the key, so the pages reference `/media/<key>`. Mp4s are not copied into `public/`. Local `pnpm dev` serves `/media/<key>` from `.media/` through a small Vite plugin (`configureServer` in `astro.config.mjs`, dev only, with Range support from `send`/`sirv`), so development needs no R2 access.
<!-- Updated: Validation Session 1 - media lives in Cloudflare R2, served same-origin by a Pages Function -->
- **Posters stay in the site build** (`src/assets/scenes/*.png`). Astro optimises them, and the LCP poster comes from the page's own origin.

## Tasks & Steps
1. Write `render-media.mjs` and run it with `--only Sequence,FunctionPlot,DnaHelix` first. Check the outputs with `ffprobe` (codec, size, duration matching the timeline) and by looking at the posters.
2. Run all scenes in a background run and record the total time (expected: a few minutes on the CPU VM).
3. Run it again and confirm that nothing renders a second time.
4. Create the bucket. The user logs in once (`! pnpm --dir website exec wrangler login`), then runs `wrangler r2 bucket create fframes-kit-media` and creates an R2 API token scoped to that bucket, kept in the `R2_*` environment variables. Then run `--upload`, then `pnpm build`, then `wrangler pages dev` from inside `website/` with no directory argument, so `wrangler.toml`'s output dir and `functions/` are both picked up. Check that `curl -r 0-99 -I http://localhost:8788/media/<key>` returns 206 with `Content-Range`. The function test runs against the local R2 simulation that `wrangler pages dev` provides.

## Verification
- `node scripts/render-media.mjs --check` checks that every scene in the timeline has an mp4 and a png and that `media.json` is current. It exits 0.
- `ffprobe -v error -show_entries stream=codec_name,width,height -of compact .media/scenes/FunctionPlot.mp4` shows h264 at 960×540.
- `node scripts/render-media.mjs --upload --dry-run` lists only the changed keys.

## Cost note
Each video request runs the Function. The Workers free plan allows 100k requests a day, and R2 egress is free. If traffic grows past that, attach a custom domain to the bucket and point `MEDIA_BASE_URL` at it. The pages then skip the Function and nothing else changes.

## Risks
- A scene panics or fails to render. The script keeps going, lists the failures, and exits non-zero.
- Formula scenes need the TeX cache (`gallery/tex-cache`). Renders use the cache that is checked in, as `just e2e` does.
