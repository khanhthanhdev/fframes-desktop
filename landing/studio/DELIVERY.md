# Website asset delivery

Edit `home.css`, `features.css`, `how-it-works.css`, and `readability.css` as the stylesheet sources. Then run:

```sh
node scripts/build-studio-delivery.mjs
```

This rebuilds the marked, inline style block in each HTML page in cascade order. The small page-specific style payload ships with the document to avoid render-blocking stylesheet requests, including with JavaScript disabled. `npm run build:landing` runs this step automatically during deployment. Do not hand-edit the generated style blocks.

The same build embeds the small page controllers at the end of the document, preserving their execution order without separate script requests. Edit `studio.js`, `scroll-motion.js`, `home.js`, or `how-it-works.js`, then rebuild rather than editing the generated script blocks. Both local fonts are preloaded from the document head to avoid waiting for CSS discovery; `font-display: optional` keeps text visible and prevents a late font swap on slow connections.

The website uses `brand/studio-logo-320.webp` and `brand/studio-logo-640.webp` with responsive image sizes. The high-resolution PNG remains the editable master and is not requested by the page. To regenerate the WebP files, install Pillow and run:

```sh
python scripts/prepare-studio-web-logo.py
```

Generated logo assets are committed, so deployment does not need Python or Pillow. The optimizer crops transparent padding, preserves transparency, and produces the normal and high-density variants. Keep the image dimensions and responsive `sizes` attributes aligned with the logo's CSS layout boxes.

Social previews use `brand/studio-og.jpg` (home) and `brand/studio-og-how-it-works.jpg`: opaque 1200 × 630 cards in the page's sky style, with the headline beside frames rendered from explainer videos made with fframes. They were captured from a 1200 × 630 HTML layout in headless Chrome and encoded as JPEG. Open Graph and Twitter tags are present in the HTML for crawlers without JavaScript. Their absolute image and canonical URLs use the current GitHub Pages deployment; update both pages if the public domain changes. Use a new image filename when changing the card so social caches can discover it.

The earlier logo-only card, `brand/studio-social-blue-v1.png`, is kept. To regenerate it with Chrome running on debugging port 9226, run `node scripts/prepare-studio-social.mjs` (or set `CHROME_DEBUG_URL`). The script composes SVG geometry, type, and the existing logo, then exports a PNG. The generated PNG is committed, so deployment does not require Chrome. Existing Facebook shares may need a new scrape through the [Sharing Debugger](https://developers.facebook.com/tools/debug/).
