//! Embedded application assets served to gpui (`svg().path("icons/play.svg")`).

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icon_files {
    ($($name:literal),* $(,)?) => {
        /// `(asset path, file bytes)` for every embedded icon.
        const ICON_FILES: &[(&str, &[u8])] = &[
            $((
                concat!("icons/", $name, ".svg"),
                include_bytes!(concat!("../assets/icons/", $name, ".svg")),
            )),*
        ];
    };
}

icon_files!(
    "play",
    "pause",
    "skip-back",
    "skip-forward",
    "step-back",
    "step-forward",
    "zoom-in",
    "zoom-out",
    "maximize",
    "volume-2",
    "volume-x",
    "speaker",
    "refresh-cw",
    "hammer",
    "square",
    "folder",
    "folder-open",
    "file",
    "image",
    "palette",
    "bot",
    "download",
    "upload",
    "plus",
    "x",
    "check",
    "chevron-down",
    "chevron-right",
    "ellipsis",
    "settings",
    "triangle-alert",
    "info",
    "circle-check",
    "history",
    "save",
    "eye",
    "layers",
    "package",
    "arrow-up",
    "arrow-left",
    "search",
    "trash",
    "undo-2",
    "redo-2",
    "scissors",
    "presentation",
    "caption-minimize-windows",
    "caption-maximize-windows",
    "caption-restore-windows",
    "caption-close-windows",
);

const BRAND_FILES: &[(&str, &[u8])] = &[(
    "branding/studio-icon.png",
    include_bytes!("../assets/branding/studio-icon.png"),
)];

/// Serves the embedded Lucide icons (ISC, see `packaging/sdk/notices`) and Cap's Windows
/// caption glyphs, along with the full-color Studio application mark.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICON_FILES
            .iter()
            .chain(BRAND_FILES.iter())
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICON_FILES
            .iter()
            .chain(BRAND_FILES.iter())
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use studio_ui::kit::Icon;

    use super::*;

    #[test]
    fn asset_source_lists_icons() {
        let listed = Assets.list("icons/").expect("list");
        assert_eq!(listed.len(), ICON_FILES.len());
        assert!(listed.iter().all(|name| name.ends_with(".svg")));
        assert!(Assets.list("fonts/").expect("list").is_empty());
    }

    #[test]
    fn unknown_asset_is_none() {
        assert!(Assets.load("icons/missing.svg").expect("load").is_none());
    }

    #[test]
    fn every_icon_has_an_asset() {
        for icon in Icon::ALL {
            let bytes = Assets.load(icon.path()).expect("load");
            let bytes = bytes.unwrap_or_else(|| panic!("missing asset {}", icon.path()));
            assert!(bytes.starts_with(b"<!--") || bytes.starts_with(b"<svg"));
        }
    }

    #[test]
    fn icon_paths_are_unique_and_listed() {
        let listed = Assets.list("icons/").expect("list");
        let mut paths: Vec<_> = Icon::ALL.iter().map(|icon| icon.path()).collect();
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), Icon::ALL.len());
        assert_eq!(paths.len(), listed.len());
    }
}
