# fframes-kit introduction video

Status: done

## Outcome

A designed introduction video for `fframes-kit`, 86 s at 1920x1080, 30 fps, that tours the
kit's features with its own components. Crate `components/intro` (package
`fframes-kit-intro`) in the kit workspace. Most beats are walls of tiles, each tile a gallery
scene playing live (`components_gallery::scenes::all()`), so the video shows the components
themselves and follows the gallery when it changes.

## Beat sheet

| # | Scene | s | Shows | In |
| --- | --- | --- | --- | --- |
| 1 | Open | 6 | starts on one tile full screen, pulls back to a 6x4 wall, `Letters` wordmark | - |
| 2 | Code | 10.5 | `Typewriter` code next to what it draws: `LowerThird`, `Counter`, `Star` | Wipe |
| 3 | Shapes | 6.5 | 3x3: Create, Write, Transform, SpinInFromNothing, Indicate, Flash, MoveAlongPath, LaggedStart, Succession | Slide |
| 4 | Transitions | 6.5 | 3x3 of `TransitionSeries`, one presentation each | Iris |
| 5 | Effects | 6 | ColorEffects, EdgeEffects, Trail, LightLeak | ClockWipe |
| 6 | Formulas | 6 | TransformMatchingTex, ChemEq, WriteFormula, TikzDiagram | SlantWipe |
| 7 | Math | 6.5 | FunctionPlot, UnitCircle, RiemannSum, LinearTransform | Wipe |
| 8 | Physics | 6.5 | hero: Pendulum, Orbit, FieldLines | Slide |
| 9 | Chemistry | 6.5 | Molecule, TitrationCurve, PeriodicTable, Molecule3D | CircleWipe |
| 10 | CS | 6.5 | hero: SortVisualizer, Dijkstra, TreeView | Wipe |
| 11 | Biology | 6.5 | MitosisStages, DnaHelix3D, Neuron, Translation | Slide |
| 12 | Overlays | 6 | LowerThird, CaptionStack, ChapterList, BarChart, Counter, KenBurns | PushCut |
| 13 | Outro | 6.5 | the wall dimmed, counts (18 transitions, 19 colour effects, 5 science kits), the import line | Fade |

A still HUD (`HudLabel` chapter, `ProgressBar`) runs from the second beat to the last but one.
Counts come from the source: 18 `impl Presentation`, the 19 rows of the colour effect table
in `docs/components/components.md`, and the five kit features.

## Acceptance criteria

- [x] `timeline` shows the scenes above; `inspect` reports only text leaving the canvas
  during slides.
- [x] `inspect --pops` findings (67) are expected changes, checked with crops and per-beat
  runs. They are: wipe fronts crossing tiles in the last frames of each beat, the largest
  group (99 occurrences, its box the union of all of them); the panels' own transitions in
  the Transitions grid; typing and counting digits; the gallery's step-sampled figures, such
  as SortVisualizer's swap and compare steps; and tiles moving into view during the opening
  pull-back. Physics, Math and Biology have none inside the beat.
- [x] Every beat reviewed with `strip`, key frames at full size.
- [x] `cargo clippy -p fframes-kit-intro --all-targets -- -D warnings` and `cargo fmt` clean.
- [x] Final render `components/intro/out/fframes-kit-intro.mp4` (ignored, not committed).

## Non-goals

No music (no licensed track in the repo), no editor bridge, no e2e snapshots, no change to
the gallery or upstream code.
