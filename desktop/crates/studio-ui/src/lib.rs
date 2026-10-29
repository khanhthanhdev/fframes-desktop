//! Component kit, editor layout and panels for fframes studio.
pub mod actions;
pub mod agent;
pub mod app_state;
pub mod canvas_view;
pub mod create_panel;
pub mod export;
pub mod frame_image;
pub mod kit;
pub mod preset_panel;
pub mod probe;
pub mod props_panel;
pub mod shell;
pub mod slides_panel;
pub mod timeline;

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    /// The views run inside the app, so this crate's package version is not the app's:
    /// SDKs are checked against the version the app passes in.
    #[test]
    fn the_app_version_comes_from_the_caller() {
        let needle = concat!("CARGO_PKG", "_VERSION");
        let mut dirs = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        let mut offenders = Vec::new();
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if fs::read_to_string(&path).unwrap().contains(needle) {
                    offenders.push(path);
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:?}");
    }
}
