//! Lucide icons as gpui `svg()` elements. The app embeds the files under
//! `icons/`; its asset tests check every path here resolves.

use gpui::{Hsla, Styled, Svg, px, svg};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    SkipBack,
    SkipForward,
    StepBack,
    StepForward,
    ZoomIn,
    ZoomOut,
    Maximize,
    Volume2,
    VolumeX,
    Speaker,
    RefreshCw,
    /// Build.
    Hammer,
    /// Stop or cancel.
    Square,
    Folder,
    FolderOpen,
    File,
    Image,
    Palette,
    Bot,
    Download,
    Upload,
    Plus,
    X,
    Check,
    ChevronDown,
    ChevronRight,
    MoreHorizontal,
    Settings,
    AlertTriangle,
    Info,
    CircleCheck,
    History,
    Save,
    Eye,
    Layers,
    Package,
    ArrowUp,
    ArrowLeft,
    Search,
    Trash,
    Undo,
    Redo,
    Scissors,
    /// Slides (the Slides tab).
    Presentation,
    /// Windows caption glyphs, from Cap.
    CaptionMinimize,
    CaptionMaximize,
    CaptionRestore,
    CaptionClose,
}

impl Icon {
    pub const ALL: [Icon; 50] = [
        Icon::Play,
        Icon::Pause,
        Icon::SkipBack,
        Icon::SkipForward,
        Icon::StepBack,
        Icon::StepForward,
        Icon::ZoomIn,
        Icon::ZoomOut,
        Icon::Maximize,
        Icon::Volume2,
        Icon::VolumeX,
        Icon::Speaker,
        Icon::RefreshCw,
        Icon::Hammer,
        Icon::Square,
        Icon::Folder,
        Icon::FolderOpen,
        Icon::File,
        Icon::Image,
        Icon::Palette,
        Icon::Bot,
        Icon::Download,
        Icon::Upload,
        Icon::Plus,
        Icon::X,
        Icon::Check,
        Icon::ChevronDown,
        Icon::ChevronRight,
        Icon::MoreHorizontal,
        Icon::Settings,
        Icon::AlertTriangle,
        Icon::Info,
        Icon::CircleCheck,
        Icon::History,
        Icon::Save,
        Icon::Eye,
        Icon::Layers,
        Icon::Package,
        Icon::ArrowUp,
        Icon::ArrowLeft,
        Icon::Search,
        Icon::Trash,
        Icon::Undo,
        Icon::Redo,
        Icon::Scissors,
        Icon::Presentation,
        Icon::CaptionMinimize,
        Icon::CaptionMaximize,
        Icon::CaptionRestore,
        Icon::CaptionClose,
    ];

    /// Asset path, relative to the app's asset root.
    pub const fn path(self) -> &'static str {
        match self {
            Icon::Play => "icons/play.svg",
            Icon::Pause => "icons/pause.svg",
            Icon::SkipBack => "icons/skip-back.svg",
            Icon::SkipForward => "icons/skip-forward.svg",
            Icon::StepBack => "icons/step-back.svg",
            Icon::StepForward => "icons/step-forward.svg",
            Icon::ZoomIn => "icons/zoom-in.svg",
            Icon::ZoomOut => "icons/zoom-out.svg",
            Icon::Maximize => "icons/maximize.svg",
            Icon::Volume2 => "icons/volume-2.svg",
            Icon::VolumeX => "icons/volume-x.svg",
            Icon::Speaker => "icons/speaker.svg",
            Icon::RefreshCw => "icons/refresh-cw.svg",
            Icon::Hammer => "icons/hammer.svg",
            Icon::Square => "icons/square.svg",
            Icon::Folder => "icons/folder.svg",
            Icon::FolderOpen => "icons/folder-open.svg",
            Icon::File => "icons/file.svg",
            Icon::Image => "icons/image.svg",
            Icon::Palette => "icons/palette.svg",
            Icon::Bot => "icons/bot.svg",
            Icon::Download => "icons/download.svg",
            Icon::Upload => "icons/upload.svg",
            Icon::Plus => "icons/plus.svg",
            Icon::X => "icons/x.svg",
            Icon::Check => "icons/check.svg",
            Icon::ChevronDown => "icons/chevron-down.svg",
            Icon::ChevronRight => "icons/chevron-right.svg",
            Icon::MoreHorizontal => "icons/ellipsis.svg",
            Icon::Settings => "icons/settings.svg",
            Icon::AlertTriangle => "icons/triangle-alert.svg",
            Icon::Info => "icons/info.svg",
            Icon::CircleCheck => "icons/circle-check.svg",
            Icon::History => "icons/history.svg",
            Icon::Save => "icons/save.svg",
            Icon::Eye => "icons/eye.svg",
            Icon::Layers => "icons/layers.svg",
            Icon::Package => "icons/package.svg",
            Icon::ArrowUp => "icons/arrow-up.svg",
            Icon::ArrowLeft => "icons/arrow-left.svg",
            Icon::Search => "icons/search.svg",
            Icon::Trash => "icons/trash.svg",
            Icon::Undo => "icons/undo-2.svg",
            Icon::Redo => "icons/redo-2.svg",
            Icon::Scissors => "icons/scissors.svg",
            Icon::Presentation => "icons/presentation.svg",
            Icon::CaptionMinimize => "icons/caption-minimize-windows.svg",
            Icon::CaptionMaximize => "icons/caption-maximize-windows.svg",
            Icon::CaptionRestore => "icons/caption-restore-windows.svg",
            Icon::CaptionClose => "icons/caption-close-windows.svg",
        }
    }
}

/// A square icon of `size` px. gpui's `svg()` keeps only the glyph's alpha and
/// does not inherit `text_color`, so the colour is set on the element itself.
pub fn icon(icon: Icon, size: f32, color: impl Into<Hsla>) -> Svg {
    svg()
        .path(icon.path())
        .size(px(size))
        .flex_none()
        .text_color(color.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_paths_are_unique_svgs_under_icons() {
        let mut paths: Vec<_> = Icon::ALL.iter().map(|icon| icon.path()).collect();
        assert!(
            paths
                .iter()
                .all(|path| path.starts_with("icons/") && path.ends_with(".svg"))
        );
        paths.sort_unstable();
        paths.dedup();
        assert_eq!(paths.len(), Icon::ALL.len());
    }
}
