# Formulas in the web editor: embedded cache

**Outcome: done.** The web editor now draws every formula a native render has cached, from
both backends. Two commits on `feat/typst-formulas` in `C:\Users\Administrator\fframes-kit`,
fast-forwarded into kit `main` and pushed on 2026-10-26. This resolves question 2 of
[cook-261025-2112-typst-formula-engine.md](cook-261025-2112-typst-formula-engine.md).

| Commit | What |
| --- | --- |
| `d84b6ff` | feat(latex): embed the formula cache for the web editor |
| `0255633` | feat(gallery): draw the gallery's formulas in the web editor |

## What changed

- `fframes_kit::latex::tex_cache!("tex-cache")` embeds the folder's `.svg` files (sorted, by
  name and content), relative to the calling crate. A missing folder gives an empty table.
- `TexEngine::with_embedded(table)` reads the table first, then the directory, then
  typesets. Cache files are named by a hash of all their inputs, so embedded files are
  never stale.
- The editor's `NotInstalled` help now says to render natively once and embed the cache.
- Gallery: one `engine()` helper with the embedded cache, `gallery/build.rs` reruns when
  `tex-cache/` changes, and `gallery/editor/` is the page that opens the gallery in the
  web editor (`@fframes/editor` 1.3.0 from npm). `just editor` builds and serves it at
  http://localhost:5173.
- For a video author the change is one call plus a two-line `build.rs`.

## Verification

- Kit: 592 unit tests, doctests and macro tests pass. Gallery release tests pass, including
  a new test that the embedded table equals `gallery/tex-cache` file by file.
- An engine unit test shows embedded files win over the directory and nothing is typeset.
- Clippy (three packages, native, and the wasm32 libraries), fmt and `just check-wasm` pass.
- All 20 cache files were found inside the built `.wasm`.
- In headless Chrome on the real editor page, the page's wasm rendered MathTex,
  FormulaParts, EulerJourney (Typst) and ChemEq and TikZ (TeX) as glyphs, with no formula
  left as source text.
- Adding or removing a cache file rebuilds the gallery; an unchanged folder does not.

## Notes

- The kit's `.crate` size does not change: the files come from the caller's crate. The
  gallery's wasm grows by the 177 KB of SVG text. The dev wasm is 40 MB because `--dev`
  keeps debug info, as in fframes' own `just run`.
- A formula changed since the last native render is not in the table and still shows as
  source text in the editor, by design.
- `wasm-pack` 0.15.0 was installed into `~/.cargo/bin`.
- `cargo clippy` on the gallery bridge fails on `clippy::non_std_lazy_statics` in
  `gallery/editor/editor-bridge/lib.rs`, which this work did not change.
- The other writer's `teaser/`, `Cargo.toml`, `Cargo.lock` and README row remain
  uncommitted and untouched.
- Cleanup: both Vite servers are stopped, and the temporary fframes-desktop worktree at
  `C:\hc\editor-demo` is removed. The main fframes-desktop checkout is as it was.
