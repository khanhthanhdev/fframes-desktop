# The shape of a throw

A six-slide, presenter-led lesson on **projectile motion**, connecting physics,
mathematics and Rust. Built with `fframes-kit` at 1920 × 1080, 30 fps.
The deck is intentionally silent; speaker notes supply talking points.

1. An animated throw traces a parabola.
2. State the ideal-model assumptions.
3. Separate horizontal and vertical motion, with a moving ball and live readouts.
4. Compare 30°, 45° and 60° throws at the same launch speed.
5. Translate the equations into a short Rust function.
6. Recap and questions.

The diagrams use an analytic trajectory, not numerical integration: launch speed
20 m/s, uniform gravity 9.81 m/s², no drag, and equal launch and landing heights.
The flight is slowed down for explanation. The angle-comparison curves draw by
fraction of their respective flights, not by a shared physical clock.

## Build and preview on this checkout

Run from the repository root in PowerShell. Until kit 0.3.0 is published, build
against the sibling kit checkout using a command-line patch (not a project patch):

```powershell
cargo +1.98.1 build --release -j 2 --config 'patch.crates-io.fframes-kit.path="C:/Users/Administrator/fframes-kit/fframes-kit"' -p projectile-motion
.\target\release\projectile-motion.exe preview 0s
```

Preview controls: Space plays/pauses, h/l seek, j/k step frames, q quits.
The native player uses the available backend, falling back to CPU if needed.
Fonts and optional `props.json`/`timeline.json` are loaded from this example's
directory, so the binary can be launched from the repository root.

```powershell
.\target\release\projectile-motion.exe timeline
.\target\release\projectile-motion.exe inspect
.\target\release\projectile-motion.exe strip launch -n 8
.\target\release\projectile-motion.exe frame components@3.4s,range@4s
.\target\release\projectile-motion.exe render -o projectile-motion.mp4
```

## Source and presentation stops

- `src/lib.rs`: kit `Deck`, slide order, text, theme and physics equations.
- `src/diagrams.rs`: custom vector diagrams and scene-local stop suggestions.
- `src/main.rs`: preview/render CLI.
- `src/bin/studio_worker.rs`: Studio worker protocol entry point.
- `slides.json`: 16 presentation stops and shareable speaker notes. Stable scene
  keys are `launch`, `assumptions`, `components`, `range`, `code`, `recap`.
- `media/`: DM Sans and JetBrains Mono with their SIL Open Font Licenses.

Kit bullet and code layouts expose text through `Deck::with_props`; custom
diagram labels are edited in Rust. Suggestions do not write `slides.json`.
Accept them explicitly in Studio, or edit the document directly. If bullet
stagger/build times change, update their scene-local frame stops too.

## Open in Studio

Open the standalone copy at
`C:\Users\Administrator\fframes-studio-examples\projectile-motion`, not this
repository workspace. It contains its own `Cargo.toml`, `studio.json`, worker,
fonts, slides and speaker notes, with no external path dependencies.

Kit 0.3.0 is not published yet. This machine has a separate local development SDK
at `C:\Users\Administrator\.fframes\sdk-projectile-dev\active`; the normal
installed SDK is unchanged. Launch Studio from the repository root with:

```powershell
$env:FFRAMES_SDK_HOME = "$HOME\.fframes\sdk-projectile-dev"
.\desktop\target\debug\fframes-studio.exe
```

Choose **Open project**, select the standalone folder above, then **Build preview**.
For subsequent normal launches, with the project open choose **Settings → SDK →
Select installed SDK…** and select the development SDK's `active` folder. Studio
saves this selection per project; the environment override is then unnecessary.
Do not choose **Use installed SDK** for this example: the normal v4 SDK lacks kit
0.3.0, and that action changes the project's pin rather than locating its SDK.
The Slides tab reads its 16 stops and speaker notes from `slides.json`; the Props
panel edits kit slide text. Custom diagram labels remain editable in Rust.
This local development SDK is not a published or release-qualified SDK.

To create another standalone copy with a compatible kit 0.3.0 SDK, use an output
folder outside any Cargo workspace or inherited `.cargo/config.toml`:

```powershell
cargo +1.98.1 run --locked --manifest-path desktop\Cargo.toml -p studio-engine --example prepare_examples -- --example projectile-motion --out C:\Videos\StudioExamples --sdk "$HOME\.fframes\sdk-projectile-dev\active" --check
```

The command preserves existing copies rather than overwriting their edits.
