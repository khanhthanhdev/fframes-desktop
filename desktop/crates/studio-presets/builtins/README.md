# Bundled presets

Ten original, read-only presets embedded into the app by `src/builtin.rs`. Each directory is a complete
schema-v1 preset package (`preset.json`, `tokens.json`, `guide.md`, `LICENSE.txt`, `licenses/`, `fonts/`,
`examples/`) and is verified offline exactly like an imported package.

All ten share one semantic vocabulary (40 tokens: `color.*`, `typography.title|heading|body|caption`,
`spacing.*`, `radius.*`, `stroke.*`, `shadow.card|glow`, `motion.duration.*`, `motion.easing.*`,
`motion.stagger.*`, plus alias tokens such as `spacing.margin`, `radius.card`, `color.title`), so a project
bound to semantic tokens renders under any of them.

| Preset | Typography | Layout | Motion |
|---|---|---|---|
| `editorial` | DM Sans Medium; 112 px title, 1.05 leading, -2 px tracking | Warm paper, 120 px margins, 2-8 px radii, hairline rules, soft shadow | Slow, confident: 0.25/0.5/0.9 s, expo-style cubic-bezier, 0.08/0.2 s stagger |
| `pulse` | DM Sans Medium; 144 px title, 0.92 leading, -5 px tracking | Dark, dense, 56 px margins, 12-48 px radii, hard offset shadow, 12 px strokes | Springy and fast: 0.12/0.2/0.35 s, spring easings, 0.03/0.07 s stagger |
| `quiet-motion` | DM Sans Regular; 88 px title, 1.2 leading, 400 weight | Cool grey-blue, 96 px margins, 16-48 px radii, diffuse shadow | Calm: 0.6/1.0/1.8 s, ease-in-out cubic-beziers, 0.2/0.4 s stagger |
| `sunset` | DM Sans Medium; 128 px title, 1.0 leading, -3 px tracking | Deep plum with coral and gold, 88 px margins, 12-64 px radii, warm soft shadow | Bouncy: 0.3/0.6/1.2 s, overshooting enter, 0.12/0.25 s stagger |
| `swiss` | DM Sans Medium; 160 px title, 0.95 leading, -6 px tracking | White with signal red, 72 px margins, square corners, 1-16 px rules | Precise: 0.18/0.4/0.75 s, in-out cubic-beziers, 0.06/0.15 s stagger |
| `terminal` | JetBrains Mono Regular; 96 px title, 1.1 leading, 400 weight | Phosphor green on near-black, 80 px margins, 1-6 px radii, green glow | Quick and linear: 0.08/0.15/0.3 s, 0.05/0.12 s stagger |
| `launch` | Geist SemiBold; 120 px title, 1.02 leading, -4 px tracking | Product launch: white with electric indigo, 100 px margins, 8-28 px radii, soft indigo shadow | Smooth: 0.22/0.45/0.85 s, expo-out cubic-beziers, 0.07/0.16 s stagger |
| `promo` | Geist SemiBold; 176 px title, 0.9 leading, -6 px tracking | Sale: yellow, black and red, 48 px margins, 6-24 px radii, hard black offset shadow, 18 px strokes | Punchy: 0.1/0.25/0.5 s, spring enter, 0.04/0.09 s stagger |
| `luxe` | DM Sans Regular; 100 px title, 1.15 leading, +4 px tracking | Premium: black with champagne gold, 128 px margins, 3-12 px radii, hairlines | Slow fades: 0.7/1.4/2.4 s, ease-in-out, 0.3/0.6 s stagger |
| `fresh` | Geist Medium; 108 px title, 1.05 leading, -2 px tracking | Wellness: mint, deep green and coral, 64 px margins, 20-80 px pill radii | Lively: 0.35/0.7/1.4 s, gentle overshoot, 0.1/0.22 s stagger |

## Provenance and licenses

Tokens, guides and example recipes are original work, licensed MIT (`LICENSE.txt` in each bundle is a copy of
the repository root `LICENSE.txt`). No images are bundled.

| Resource | Used by | Source in this repository | License notice |
|---|---|---|---|
| `fonts/DMSans-Medium.ttf` | `editorial`, `pulse`, `sunset`, `swiss` | `desktop/fixtures/annotated-video-overlay/media/DMSans-Medium.ttf` (byte-identical; also `cargo-fframes/templates/`) | SIL OFL 1.1, `licenses/DMSans-OFL.txt` (normalized line endings and trailing whitespace from `desktop/fixtures/annotated-video-overlay/media/OFL.txt`), Copyright 2014 The DM Sans Project Authors |
| `fonts/DMSans-Regular.ttf` | `quiet-motion`, `luxe` | `examples/beta/media/DMSans-Regular.ttf` | Same DM Sans family and the same OFL notice. The repository carries no separate notice next to this file; the DM Sans project publishes all weights under that one license. Verify before redistribution beyond this repository. |
| `fonts/Geist-SemiBold.ttf`, `fonts/Geist-Medium.ttf` | `launch`, `promo` (SemiBold), `fresh` (Medium) | `desktop/app/assets/fonts/` (byte-identical) | SIL OFL 1.1, `licenses/Geist-OFL.txt` (copied from `desktop/app/assets/fonts/LICENSE.txt`), Copyright 2024 The Geist Project Authors |
| `fonts/JetBrainsMono-Regular.ttf` | `terminal` | `examples/projectile-motion/media/JetBrainsMono-Regular.ttf` (byte-identical to the other copies in `examples/` and `e2e/media/`) | SIL OFL 1.1, `licenses/JetBrainsMono-OFL.txt` (copied from `examples/projectile-motion/media/JetBrainsMono-OFL.txt`), Copyright 2020 The JetBrains Mono Project Authors |

No font, image or other resource was downloaded; the only fonts bundled are ones already in the repository whose
family license is recorded above. Add a resource only together with a checked license notice.

## Regenerating digests

`preset.json` records the size and SHA-256 of every file. After editing any bundled file run:

```sh
cargo run --locked --manifest-path desktop/Cargo.toml -p studio-presets --example seal -- \
  desktop/crates/studio-presets/builtins/editorial \
  desktop/crates/studio-presets/builtins/pulse \
  desktop/crates/studio-presets/builtins/quiet-motion \
  desktop/crates/studio-presets/builtins/sunset \
  desktop/crates/studio-presets/builtins/swiss \
  desktop/crates/studio-presets/builtins/terminal \
  desktop/crates/studio-presets/builtins/launch \
  desktop/crates/studio-presets/builtins/promo \
  desktop/crates/studio-presets/builtins/luxe \
  desktop/crates/studio-presets/builtins/fresh
```

which rewrites the digests and verifies each bundle. `cargo test -p studio-presets --test builtins` then checks
the registry (exactly ten validated bundles), vocabulary parity, differing typography/layout/motion and the
font/license provenance above.
