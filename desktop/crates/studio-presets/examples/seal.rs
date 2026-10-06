//! Authoring helper: recompute sizes and SHA-256 digests in a preset's `preset.json`, then verify it.
//! `cargo run --locked --manifest-path desktop/Cargo.toml -p studio-presets --example seal -- <preset-dir>...`

use std::path::Path;

fn main() {
    let mut failed = false;
    for dir in std::env::args().skip(1) {
        let dir = Path::new(&dir);
        let result = studio_presets::package::seal_dir(dir).and_then(|()| {
            studio_presets::package::read_dir_files(dir)
                .and_then(studio_presets::Package::from_files)
        });
        match result {
            Ok(p) => println!("{}: sealed, hash {}", dir.display(), p.hash()),
            Err(e) => {
                eprintln!("{}: {e}", dir.display());
                failed = true;
            }
        }
    }
    if failed {
        std::process::exit(1);
    }
}
