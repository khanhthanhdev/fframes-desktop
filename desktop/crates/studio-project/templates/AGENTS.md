# Studio project instructions — version 2

Rust and media are the portable source of truth. Keep assets inside media/ and
ship their licenses. Use the bundled DM Sans font for reproducible text.
Do not store SDK paths, credentials, build output or app sessions in this folder.
Studio binds exact Cargo versions to its compatible SDK in an app-local build
copy; standalone Cargo builds require those releases to be published/available.
Keep the normal CLI in src/main.rs and worker entry in src/bin/studio_worker.rs.
Use frame, inspect and strip to verify changes. Never overwrite external edits.

Design tokens: style/tokens.json is the resolved, generated token file (colors,
typography, spacing, motion, shadows). Construct fframes::Styles once in
StudioVideo::new from include_str!("../style/tokens.json"), then resolve every
token used by render_frame there into a typed field on StudioVideo. Propagate
StylesError with `?`; if an accessor returns a borrowed value such as
Typography, clone it when storing an owned field. render_frame must only read
those resolved fields: do not call styles accessors, parse/read token files, or
use `.expect()` for style lookup in the per-frame path. Use semantic token
names (color.accent, spacing.md, typography.title) instead of literal colors,
sizes or timings, and add a token rather than hard-coding a repeated value. Do
not edit style/tokens.json by hand when a preset owns it; Studio regenerates it.
Assets and licensing: put only assets you may redistribute in media/, keep each
license beside its asset (OFL.txt for DM Sans), and never invent attribution.
Timing: scene order, trims and splits made in Studio's timeline are stored in
timeline.json, not in the code. Edit timing there, keep define_scenes listing
each scene once, and give every scene a stable editor_instance_key: a clip
finds its scene by that key (or the scene's name() without one), so changing
a key orphans its clips. Scenes timeline.json omits play after its clips,
untrimmed. An invalid timeline.json fails the CLI and export with the reason;
fix the file, run inspect, and never delete it to make a build pass.
Slides: slide stops and speaker notes live in slides.json, a document Studio
reads at run time like timeline.json, so editing it needs no rebuild:
{"version":1,"stops":[{"scene":"intro","at":"end","title":"Welcome",
"notes":"Say hello."},{"scene":"features","at":40}]}. "scene" is the clip key
timeline.json uses (editor_instance_key, else name()). "at" is a frame in the
scene's own frame numbers (trims do not move it) or "end", the clip's last
frame before the next clip cross-fades over it; "end" follows trims. Without
the file or with no stops, every clip end is a stop; the video's last frame
always is. Limits: 300 stops, 512 KiB in all, title 256 bytes on one line,
notes 8 KiB; unknown fields and other versions are refused. Stops must land on
frames the video plays, or validation fails. Notes are copied into exported
decks that are shared, so write nothing private in them.
Component kit: Studio's SDK includes fframes-kit and fframes-studio-kit. To use
them add `fframes-kit = { version = "=0.3.0", features = ["studio"] }` (plus
features such as "latex", "3d" or "physics" as needed) and
`fframes-studio-kit = "=0.2.0"` to Cargo.toml. Use exactly the kit version the
SDK names; never a git or path dependency and never a [patch] section (Studio
points the kit at its own fframes). For the Props panel keep an
Arc<fframes_studio_kit::PropsStore> in StudioVideo, draw with
Ui::new(frame, ctx, &theme).with_props(store.view()) (scene_view(self) in
scenes), and in src/bin/studio_worker.rs call store.listen() and pass
store.hooks() to the worker config's with_props and to run_export_with_props.
Kit components propose stops through these hooks. Scan in the Slides tab and
accept suggestions explicitly; rendering never writes slides.json. For a custom
component, use Ui::suggest_stops with scene-local seconds, labels and optional
speaker notes. Stops inside shifted Ui scopes are converted to scene time.
Studio validation tools: after changes run frame, inspect and strip, and use
Studio's preview and validation results to confirm tokens resolve and the
project still builds before reporting it done.
