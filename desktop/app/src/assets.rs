//! Embedded application assets served to gpui (`svg().path("icons/play.svg")`).

use std::borrow::Cow;

use gpui::{App, AssetSource, Result, SharedString};

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
    "copy",
    "brain",
    "paperclip",
    "clock",
    "shield-question-mark",
    "file-diff",
    "caption-minimize-windows",
    "caption-maximize-windows",
    "caption-restore-windows",
    "caption-close-windows",
    "file-code",
    "file-json",
    "file-text",
    "file-video",
    "file-music",
    "file-image",
    "file-type",
    "keyboard",
    "life-buoy",
    "database",
    "sun",
    "moon",
    "monitor",
    "book-open",
    "layout-grid",
    "list",
    "star",
    "sliders-horizontal",
    "sparkles",
    "folder-plus",
    "folder-input",
    "external-link",
    "panel-left",
    "command",
    "chevrons-down-up",
    "list-tree",
    "scroll-text",
    "globe",
    "pencil",
);

const BRAND_FILES: &[(&str, &[u8])] = &[(
    "branding/studio-icon.png",
    include_bytes!("../assets/branding/studio-icon.png"),
)];

/// Registers Studio's bundled, OFL-licensed interface and code fonts.
///
/// # Errors
/// Returns the platform text system's error if a bundled font cannot be loaded.
pub fn init_fonts(cx: &App) -> Result<()> {
    cx.text_system().add_fonts(
        FONT_FILES
            .iter()
            .map(|bytes| Cow::Borrowed(*bytes))
            .collect(),
    )
}

/// Geist 1.9.0 and Geist Mono 1.9.0 (vercel/geist-font, OFL): the full builds, not the
/// web's Latin subset, so Vietnamese, Latin Extended, Cyrillic, Greek and arrows render in
/// the interface face.
const FONT_FILES: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
];

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

    /// The code points a font's `cmap` maps (formats 4 and 12, which these fonts use).
    fn cmap_code_points(font: &[u8]) -> std::collections::HashSet<u32> {
        let u16_at = |at: usize| u16::from_be_bytes([font[at], font[at + 1]]) as usize;
        let u32_at = |at: usize| u32::from_be_bytes(font[at..at + 4].try_into().unwrap());
        let cmap = (0..u16_at(4))
            .map(|i| 12 + 16 * i)
            .find(|&record| &font[record..record + 4] == b"cmap")
            .map(|record| u32_at(record + 8) as usize)
            .expect("cmap table");
        let mut points = std::collections::HashSet::new();
        for i in 0..u16_at(cmap + 2) {
            let table = cmap + u32_at(cmap + 4 + 8 * i + 4) as usize;
            match u16_at(table) {
                4 => {
                    let segments = u16_at(table + 6) / 2;
                    for s in 0..segments {
                        let end = u16_at(table + 14 + 2 * s);
                        let start = u16_at(table + 16 + 2 * segments + 2 * s);
                        if start != 0xffff {
                            points.extend(start as u32..=end as u32);
                        }
                    }
                }
                12 => {
                    for g in 0..u32_at(table + 12) as usize {
                        let group = table + 16 + 12 * g;
                        points.extend(u32_at(group)..=u32_at(group + 4));
                    }
                }
                _ => {}
            }
        }
        points
    }

    #[test]
    fn bundled_fonts_cover_vietnamese_and_extended_scripts() {
        // A Latin-1 web subset once shipped here; "Trần Đức" then mixed two typefaces.
        let sample = "ạăâđêôơưỹẫệ ĐƠƯ łşžő Жд λΩ →← …—•×";
        for (index, font) in FONT_FILES.iter().enumerate() {
            let points = cmap_code_points(font);
            let missing: Vec<char> = sample
                .chars()
                .filter(|c| !c.is_whitespace() && !points.contains(&(*c as u32)))
                .collect();
            assert!(missing.is_empty(), "font {index} lacks {missing:?}");
        }
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
