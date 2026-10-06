# Studio project instructions — version 1

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
Studio validation tools: after changes run frame, inspect and strip, and use
Studio's preview and validation results to confirm tokens resolve and the
project still builds before reporting it done.
