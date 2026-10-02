# Studio project instructions — version 1

Rust and media are the portable source of truth. Keep assets inside media/ and
ship their licenses. Use the bundled DM Sans font for reproducible text.
Do not store SDK paths, credentials, build output or app sessions in this folder.
Studio binds exact Cargo versions to its compatible SDK in an app-local build
copy; standalone Cargo builds require those releases to be published/available.
Keep the normal CLI in src/main.rs and worker entry in src/bin/studio_worker.rs.
Use frame, inspect and strip to verify changes. Never overwrite external edits.
