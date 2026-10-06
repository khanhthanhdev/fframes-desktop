# Bundled presets

Three original, read-only presets embedded into the app by `src/builtin.rs`. Each directory is a complete
schema-v1 preset package (`preset.json`, `tokens.json`, `guide.md`, `LICENSE.txt`, `licenses/`, `fonts/`,
`examples/`) and is verified offline exactly like an imported package.

All three share one semantic vocabulary (40 tokens: `color.*`, `typography.title|heading|body|caption`,
`spacing.*`, `radius.*`, `stroke.*`, `shadow.card|glow`, `motion.duration.*`, `motion.easing.*`,
`motion.stagger.*`, plus alias tokens such as `spacing.margin`, `radius.card`, `color.title`), so a project
bound to semantic tokens renders under any of them.

| Preset | Typography | Layout | Motion |
|---|---|---|---|
| `editorial` | DM Sans Medium; 112 px title, 1.05 leading, -2 px tracking | Warm paper, 120 px margins, 2-8 px radii, hairline rules, soft shadow | Slow, confident: 0.25/0.5/0.9 s, expo-style cubic-bezier, 0.08/0.2 s stagger |
| `pulse` | DM Sans Medium; 144 px title, 0.92 leading, -5 px tracking | Dark, dense, 56 px margins, 12-48 px radii, hard offset shadow, 12 px strokes | Springy and fast: 0.12/0.2/0.35 s, spring easings, 0.03/0.07 s stagger |
| `quiet-motion` | DM Sans Regular; 88 px title, 1.2 leading, 400 weight | Cool grey-blue, 96 px margins, 16-48 px radii, diffuse shadow | Calm: 0.6/1.0/1.8 s, ease-in-out cubic-beziers, 0.2/0.4 s stagger |

## Provenance and licenses

Tokens, guides and example recipes are original work, licensed MIT (`LICENSE.txt` in each bundle is a copy of
the repository root `LICENSE.txt`). No images are bundled.

| Resource | Used by | Source in this repository | License notice |
|---|---|---|---|
| `fonts/DMSans-Medium.ttf` | `editorial`, `pulse` | `desktop/fixtures/annotated-video-overlay/media/DMSans-Medium.ttf` (byte-identical; also `cargo-fframes/templates/`) | SIL OFL 1.1, `licenses/DMSans-OFL.txt` (normalized line endings and trailing whitespace from `desktop/fixtures/annotated-video-overlay/media/OFL.txt`), Copyright 2014 The DM Sans Project Authors |
| `fonts/DMSans-Regular.ttf` | `quiet-motion` | `examples/beta/media/DMSans-Regular.ttf` | Same DM Sans family and the same OFL notice. The repository carries no separate notice next to this file; the DM Sans project publishes all weights under that one license. Verify before redistribution beyond this repository. |

No font, image or other resource was downloaded; the only fonts bundled are ones already in the repository whose
family license is recorded above. Add a resource only together with a checked license notice.

## Regenerating digests

`preset.json` records the size and SHA-256 of every file. After editing any bundled file run:

```sh
cargo run --locked --manifest-path desktop/Cargo.toml -p studio-presets --example seal -- \
  desktop/crates/studio-presets/builtins/editorial \
  desktop/crates/studio-presets/builtins/pulse \
  desktop/crates/studio-presets/builtins/quiet-motion
```

which rewrites the digests and verifies each bundle. `cargo test -p studio-presets --test builtins` then checks
the registry (exactly three validated bundles), vocabulary parity, differing typography/layout/motion and the
font/license provenance above.
