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
them add `fframes-kit = { version = "=0.2.1", features = ["studio"] }` (plus
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
Captions: subtitles, lyrics and word-by-word captions use fframes::captions
(the "captions" feature of fframes, on in this template; add it to fframes'
features if a project lacks it). Put the track in media/ as WebVTT (phrases
only) or JSON: {"cues":[{"start":0.2,"end":2.6,"words":[{"text":"Hello",
"start":0.2,"end":0.7,"metadata":{"effect":"underline"}}]},{"start":3,
"end":5,"text":"A phrase without word timing."}]}. Times are absolute seconds
over [start, end); cues are ordered by start, words are ordered inside their
cue, a cue has "text" or "words", and "space_before":false joins punctuation.
Never invent word timings by splitting a phrase evenly: use word timestamps
from a transcription, or show whole phrases. Captions carry no sound: if the
transcript belongs to a voiceover, add the audio file to media/ and audio().
Build each treatment once in StudioVideo::new; the builder cannot fail:
use fframes::captions::{CaptionClock, CaptionMode, CaptionRect, Captions};
Captions::from_media("speech.json", CaptionRect::new(x, y, w, h)) takes a
file name inside media/ and four f64 frame pixels (convert token f32 with
.into()). From typography.caption, scaled like the title: .font(family.as_str(),
size.round() as usize), .font_weight(weight) (u16), .line_height(
line_height.into()). Fills take any CSS colour string, so pass color tokens
with Color::to_string(): .active_fill, .upcoming_fill, .spoken_fill, and
.outline(colour, width_f64), which needs a stroke width (about 5% of the
font size). Usual mapping: active color.accent,
upcoming color.muted, spoken color.text, outline color.background. Optional
.active_background(colour, padding) and .underline(colour, width) mark the
active word. .mode(CaptionMode::Highlight | Karaoke | Reveal | Word). Lines
are centred and sit on the rect's bottom edge: make it about two lines tall
(size * line_height * 2) inside spacing.margin. Draw it in render_frame,
which must take `mut frame`: {captions.render(&mut frame, ctx)}; in
Video::render_frame call ctx.render_scenes(&frame) into a variable first.
Clock: scene-local by default, so captions inside a scene follow its trims;
for one transcript over the whole video draw it in Video::render_frame with
.clock(CaptionClock::Global). Per-word drawing: render_with(&mut frame, ctx,
|scope, text| ..) runs for every visible word, highlight_with only while a
timed word is spoken; use render_with for an effect that stays on a word
(an underline for the whole cue). The callback returns the word's SVG in
word-local coordinates: (0, 0) is the top-left of its box, scope.width() by
scope.height(), so y = scope.height() is under the text; return `text` to keep
the preset look. scope.word.state is Upcoming, Active or Spoken and
scope.word.progress is 0..=1. scope.word.metadata.typed::<T>() returns
Option<&T>, decoded once and cached, with T deriving
fframes::serde::Deserialize and #[serde(crate = "fframes::serde")]; prefer an
enum with #[serde(tag = "effect", rename_all = "kebab-case")]. Do not add a
serde dependency. Never parse JSON, call CaptionTrack::parse or
metadata.parse in render_frame. A missing font, a word wider than the rect or
too many lines drops the captions from those frames and is reported by
inspect with the cue (cues[0].layout ...); fix the rect or size, not the data.
Studio's captions tool lists the tracks in media/, the field an invalid one
fails on, and each track's cues and word timings, without a build: read the
timings there instead of guessing, and call it again after editing a track.
Check frames at word boundaries, in a gap and where cues overlap, and a strip
over one cue (strip 0.2s..2.6s -n 8 -o strip.png; -o is a .png file).
Studio validation tools: after changes run frame, inspect and strip, and use
Studio's preview and validation results to confirm tokens resolve and the
project still builds before reporting it done.
