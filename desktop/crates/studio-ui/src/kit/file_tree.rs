//! `FileTree` and `ExplorerSection`: Oleafly's Explorer, a folder tree built from
//! project-relative paths under a collapsible section header.
//!
//! [`build_tree`] turns paths such as `src/scenes/intro.rs` into folders and files, folders
//! first and each group in alphabetical order. [`TreeExpansion`] says which folders are open
//! (top-level ones by default) and [`visible_rows`] flattens the tree into the rows on show.
//! The caller keeps the expansion and the selection and passes the rows to [`FileTree`].
//!
//! The look follows Oleafly's `FileTree` and `SidebarSection`: 32 px rows at the item radius,
//! indented 12 px per depth with a hairline guide under each open folder, a chevron and a
//! `Folder` or `FolderOpen` glyph on folders, a coloured file-type glyph on files; selected
//! rows sit on `ctl_hover` and others turn `ctl` under the pointer. The section header is a
//! 32 px row with a chevron, a 14 px icon, the title in capitals, a count badge, actions shown
//! on hover and a trailing element that always shows.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    Role, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
    px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::badge::{Badge, Tone};
use super::icon::{Icon, icon};
use super::interact::{focusable, reveal_actions, with_tooltip};
use super::list_row::row_look;

/// The indent of one tree level (Oleafly's `depth * 12`).
const INDENT: f32 = 12.;
/// A row's padding before its first level and after its name (Oleafly's `+ 8`, `pr-2`).
const ROW_PAD: f32 = METRICS.space_2;
/// The gap between a row's chevron, glyph and name (Oleafly's `gap-1.5`).
const ROW_GAP: f32 = 6.;
/// The chevron's slot, kept empty on files so names line up (Oleafly's `w-3.5`).
const CHEVRON: f32 = 14.;
/// An indent guide's width: one pixel, as a hairline draws (a zero-width bordered element
/// draws nothing).
const GUIDE_W: f32 = 1.;
/// The padding around the rows (Oleafly's `p-1.5`).
const TREE_PAD: f32 = 6.;
/// The section header's padding and its chevron (Oleafly's `px-2.5`, `size-3`).
const HEADER_PAD: f32 = 10.;
const HEADER_CHEVRON: f32 = 12.;
/// The gap between the header's actions (Oleafly's `gap-0.5`).
const HEADER_ACTION_GAP: f32 = 2.;

/// A folder or a file in a [`build_tree`] tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    /// The last part of the path.
    pub name: String,
    /// The path relative to the tree's root, with `/` between parts.
    pub path: String,
    pub folder: bool,
    /// A folder's contents, sorted; always empty for a file.
    pub children: Vec<TreeNode>,
}

/// The tree of `paths` (relative, `/` or `\` between parts): folders first, then files, each
/// in alphabetical order regardless of case. Empty parts are skipped and repeated paths
/// appear once.
pub fn build_tree<S: AsRef<str>>(paths: &[S]) -> Vec<TreeNode> {
    let mut roots = Vec::new();
    for path in paths {
        let parts: Vec<&str> = path
            .as_ref()
            .split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .collect();
        let mut level = &mut roots;
        let mut prefix = String::new();
        for (index, part) in parts.iter().enumerate() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let folder = index + 1 < parts.len();
            let at = match level
                .iter()
                .position(|node: &TreeNode| node.folder == folder && node.name == *part)
            {
                Some(at) => at,
                None => {
                    level.push(TreeNode {
                        name: (*part).to_owned(),
                        path: prefix.clone(),
                        folder,
                        children: Vec::new(),
                    });
                    level.len() - 1
                }
            };
            level = &mut level[at].children;
        }
    }
    sort(&mut roots);
    roots
}

fn sort(nodes: &mut [TreeNode]) {
    nodes.sort_by(|a, b| {
        b.folder
            .cmp(&a.folder)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    for node in nodes {
        sort(&mut node.children);
    }
}

/// Which folders of a tree are open: top-level folders unless closed, deeper ones once
/// opened. [`TreeExpansion::collapse_all`] closes every folder, top-level ones included.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeExpansion {
    chosen: HashMap<String, bool>,
    collapsed_all: bool,
}

impl TreeExpansion {
    pub fn is_open(&self, path: &str, depth: usize) -> bool {
        self.chosen
            .get(path)
            .copied()
            .unwrap_or(depth == 0 && !self.collapsed_all)
    }

    pub fn set_open(&mut self, path: &str, open: bool) {
        self.chosen.insert(path.to_owned(), open);
    }

    pub fn collapse_all(&mut self) {
        self.chosen.clear();
        self.collapsed_all = true;
    }
}

/// One row of a [`FileTree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub name: String,
    pub path: String,
    pub depth: usize,
    pub folder: bool,
    /// An open folder; always false for a file.
    pub open: bool,
}

/// The rows of `nodes` on show: every node whose folders are all open, depth first.
pub fn visible_rows(nodes: &[TreeNode], expansion: &TreeExpansion) -> Vec<TreeRow> {
    fn walk(nodes: &[TreeNode], depth: usize, expansion: &TreeExpansion, rows: &mut Vec<TreeRow>) {
        for node in nodes {
            let open = node.folder && expansion.is_open(&node.path, depth);
            rows.push(TreeRow {
                name: node.name.clone(),
                path: node.path.clone(),
                depth,
                folder: node.folder,
                open,
            });
            if open {
                walk(&node.children, depth + 1, expansion, rows);
            }
        }
    }
    let mut rows = Vec::new();
    walk(nodes, 0, expansion, &mut rows);
    rows
}

/// What a file holds, by its extension, for its glyph and colour (after Oleafly's
/// `FileIcon`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Code,
    Json,
    Text,
    Image,
    Video,
    Audio,
    Font,
    Other,
}

impl FileKind {
    pub fn of(file: &str) -> Self {
        let extension = Path::new(file)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase);
        match extension.as_deref() {
            Some(
                "rs" | "toml" | "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "wgsl" | "glsl"
                | "css" | "html" | "yml" | "yaml" | "sh" | "py",
            ) => Self::Code,
            Some("json" | "jsonc" | "json5") => Self::Json,
            Some("md" | "markdown" | "txt" | "vtt" | "srt" | "csv" | "log" | "lock") => Self::Text,
            Some(
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "avif" | "ico" | "tif"
                | "tiff",
            ) => Self::Image,
            Some("mp4" | "mov" | "webm" | "mkv" | "avi" | "m4v") => Self::Video,
            Some("mp3" | "wav" | "ogg" | "flac" | "m4a" | "aac" | "opus") => Self::Audio,
            Some("ttf" | "otf" | "woff" | "woff2") => Self::Font,
            _ => Self::Other,
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::Code => Icon::FileCode,
            Self::Json => Icon::FileJson,
            Self::Text => Icon::FileText,
            Self::Image => Icon::FileImage,
            Self::Video => Icon::FileVideo,
            Self::Audio => Icon::FileMusic,
            Self::Font => Icon::FileType,
            Self::Other => Icon::File,
        }
    }

    /// The glyph's colour: code in the accent, JSON amber, media in its timeline track's
    /// colour, the rest `text_2`.
    pub fn color(self, theme: &Theme) -> Hsla {
        let colors = theme.colors;
        match self {
            Self::Code => colors.accent,
            Self::Json => colors.warning,
            Self::Image => colors.track_overlay,
            Self::Video => colors.track_clip,
            Self::Audio => colors.track_audio,
            Self::Text | Self::Font | Self::Other => colors.text_2,
        }
        .into()
    }
}

type RowHandler = Rc<dyn Fn(&TreeRow, &mut Window, &mut App)>;
type RowText = Rc<dyn Fn(&TreeRow) -> Option<SharedString>>;

/// The rows of a folder tree. A folder row opens or closes on click, Enter and Space
/// ([`FileTree::on_toggle`]); a file row runs [`FileTree::on_open`]. Row ids are
/// `{prefix}-{path}` for files and `folder-{path}` for folders.
#[derive(IntoElement)]
pub struct FileTree {
    theme: Theme,
    file_prefix: SharedString,
    rows: Vec<TreeRow>,
    selected: Option<String>,
    tooltip: Option<RowText>,
    on_toggle: Option<RowHandler>,
    on_open: Option<RowHandler>,
}

impl FileTree {
    /// File rows get the ids `{file_prefix}-{path}`.
    pub fn new(theme: &Theme, file_prefix: impl Into<SharedString>, rows: Vec<TreeRow>) -> Self {
        Self {
            theme: *theme,
            file_prefix: file_prefix.into(),
            rows,
            selected: None,
            tooltip: None,
            on_toggle: None,
            on_open: None,
        }
    }

    /// The path of the selected row, drawn on `ctl_hover`.
    pub fn selected(mut self, path: Option<String>) -> Self {
        self.selected = path;
        self
    }

    /// A row's tooltip, such as a file's full path on disk.
    pub fn tooltip(mut self, text: impl Fn(&TreeRow) -> Option<SharedString> + 'static) -> Self {
        self.tooltip = Some(Rc::new(text));
        self
    }

    pub fn on_toggle(
        mut self,
        handler: impl Fn(&TreeRow, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    pub fn on_open(mut self, handler: impl Fn(&TreeRow, &mut Window, &mut App) + 'static) -> Self {
        self.on_open = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for FileTree {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let FileTree {
            theme,
            file_prefix,
            rows,
            selected,
            tooltip,
            on_toggle,
            on_open,
        } = self;
        let colors = theme.colors;
        let tree_id = SharedString::from(format!("{file_prefix}-tree"));
        let rows = rows.into_iter().map(|row| {
            let id = if row.folder {
                format!("folder-{}", row.path)
            } else {
                format!("{file_prefix}-{}", row.path)
            };
            let look = row_look(&theme, selected.as_deref() == Some(&row.path), false, false);
            let handler = if row.folder { &on_toggle } else { &on_open }.clone();
            let tip = tooltip.as_ref().and_then(|text| text(&row));
            let (glyph, glyph_color) = if row.folder {
                let glyph = if row.open {
                    Icon::FolderOpen
                } else {
                    Icon::Folder
                };
                (glyph, Hsla::from(colors.text_2))
            } else {
                let kind = FileKind::of(&row.name);
                (kind.icon(), kind.color(&theme))
            };
            let chevron = div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(px(CHEVRON))
                .when(row.folder, |slot| {
                    let glyph = if row.open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    };
                    slot.child(icon(glyph, METRICS.size.icon_sm, colors.text_2))
                });
            // A hairline down the middle of each ancestor's chevron: the indent guides.
            let guides = (0..row.depth).map(|level| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(ROW_PAD + level as f32 * INDENT + CHEVRON / 2.))
                    .w(px(GUIDE_W))
                    .bg(colors.line)
            });
            let element = with_tooltip(
                focusable(div().id(SharedString::from(id)), &theme, false),
                &theme,
                tip,
            )
            .role(Role::TreeItem)
            .aria_label(row.name.clone())
            .aria_level(row.depth + 1)
            .aria_selected(selected.as_deref() == Some(&row.path))
            .when(row.folder, |this| this.aria_expanded(row.open))
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(ROW_GAP))
            .h(px(METRICS.size.list_row_h))
            .w_full()
            .pl(px(ROW_PAD + row.depth as f32 * INDENT))
            .pr(px(ROW_PAD))
            .rounded(px(METRICS.radius.item))
            .when_some(look.fill, |this, fill| this.bg(fill))
            .when_some(look.hover, |this, hover| {
                this.hover(move |style| style.bg(hover))
            })
            .cursor_pointer()
            .children(guides)
            .child(chevron)
            .child(icon(glyph, METRICS.size.icon_md, glyph_color))
            .child(
                text_style(div().flex_1().min_w_0(), TextStyle::Body)
                    .text_color(look.title)
                    .truncate()
                    .child(row.name.clone()),
            );
            match handler {
                Some(handler) => element
                    .on_click(move |_, window, cx| handler(&row, window, cx))
                    .into_any_element(),
                None => element.into_any_element(),
            }
        });
        div()
            .id(tree_id)
            .role(Role::Tree)
            .flex()
            .flex_col()
            .w_full()
            .p(px(TREE_PAD))
            .children(rows)
    }
}

type ToggleHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Oleafly's Explorer section: a 32 px header that folds the content, with a chevron, a
/// 14 px icon, the title in capitals (`Overline`, `text_2`) and an optional count badge; the
/// [`ExplorerSection::action`]s show while the pointer is over the section or keyboard
/// focus is in them, the [`ExplorerSection::trailing`] element always. A hairline closes
/// the header while it is open. Open, the section takes the height its column leaves.
#[derive(IntoElement)]
pub struct ExplorerSection {
    theme: Theme,
    id: ElementId,
    icon: Icon,
    title: SharedString,
    count: Option<usize>,
    open: bool,
    on_toggle: Option<ToggleHandler>,
    actions: Vec<AnyElement>,
    trailing: Option<AnyElement>,
    children: Vec<AnyElement>,
}

impl ExplorerSection {
    /// The header's toggle gets the id `{id}-toggle`.
    pub fn new(
        theme: &Theme,
        id: impl Into<ElementId>,
        glyph: Icon,
        title: impl Into<SharedString>,
        open: bool,
    ) -> Self {
        Self {
            theme: *theme,
            id: id.into(),
            icon: glyph,
            title: title.into(),
            count: None,
            open,
            on_toggle: None,
            actions: Vec::new(),
            trailing: None,
            children: Vec::new(),
        }
    }

    /// The number in the badge after the title.
    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    /// Runs with the new open state when the header is clicked or activated.
    pub fn on_toggle(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    /// A small control shown on hover, such as Collapse all.
    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }

    /// An element at the header's end that always shows.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }
}

impl RenderOnce for ExplorerSection {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ExplorerSection {
            theme,
            id,
            icon: glyph,
            title,
            count,
            open,
            on_toggle,
            actions,
            trailing,
            children,
        } = self;
        let colors = theme.colors;
        let group = SharedString::from(format!("explorer-section-{id}"));
        let chevron = if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        let toggle = focusable(
            div().id(SharedString::from(format!("{id}-toggle"))),
            &theme,
            false,
        )
        .role(Role::Button)
        .aria_label(title.clone())
        .aria_expanded(open)
        .flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .items_center()
        .gap(px(ROW_GAP))
        .px(px(HEADER_PAD))
        .cursor_pointer()
        .child(icon(chevron, HEADER_CHEVRON, colors.text_2))
        .child(icon(glyph, METRICS.size.icon_sm, colors.text_2))
        .child(
            text_style(div().min_w_0(), TextStyle::Overline)
                .text_color(colors.text_2)
                .truncate()
                .child(title.to_uppercase()),
        )
        .children(count.map(|count| Badge::new(count.to_string()).tone(Tone::Info)))
        .when_some(on_toggle, |this, handler| {
            this.on_click(move |_, window, cx| handler(!open, window, cx))
        });
        let actions = (!actions.is_empty()).then(|| {
            reveal_actions(
                div().id(SharedString::from(format!("{id}-actions"))),
                group.clone(),
            )
            .flex()
            .flex_none()
            .items_center()
            .gap(px(HEADER_ACTION_GAP))
            .children(actions)
        });
        let ctl = colors.ctl;
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_1))
            .h(px(METRICS.size.list_row_h))
            .pr(px(ROW_PAD))
            .hover(move |style| style.bg(ctl))
            .when(open, |this| {
                this.border_b(px(METRICS.size.divider))
                    .border_color(colors.line)
            })
            .child(toggle)
            .children(actions)
            .children(
                trailing.map(|trailing| div().flex().flex_none().items_center().child(trailing)),
            );
        div()
            .id(id)
            .group(group)
            .flex()
            .flex_col()
            .min_h_0()
            .when(open, |this| this.flex_1())
            .child(header)
            .when(open, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .children(children),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|row| format!("{}{}", "  ".repeat(row.depth), row.name))
            .collect()
    }

    #[test]
    fn paths_nest_into_folders_listed_before_files_in_alphabetical_order() {
        let tree = build_tree(&[
            "src/scenes/outro.rs",
            "Cargo.toml",
            "src/main.rs",
            "build.rs",
            "src/scenes/Intro.rs",
            "assets/b.png",
            "src/lib.rs",
            "src/a/deep/x.json",
        ]);
        let all = TreeExpansion::default();
        let mut open = all.clone();
        for folder in ["src/scenes", "src/a", "src/a/deep"] {
            open.set_open(folder, true);
        }
        assert_eq!(
            names(&visible_rows(&tree, &open)),
            [
                "assets",
                "  b.png",
                "src",
                "  a",
                "    deep",
                "      x.json",
                "  scenes",
                "    Intro.rs",
                "    outro.rs",
                "  lib.rs",
                "  main.rs",
                "build.rs",
                "Cargo.toml",
            ]
        );
        let scenes = &tree[1].children[1];
        assert_eq!((scenes.path.as_str(), scenes.folder), ("src/scenes", true));
        assert_eq!(scenes.children[0].path, "src/scenes/Intro.rs");
        assert!(scenes.children.iter().all(|file| !file.folder));
    }

    #[test]
    fn only_top_level_folders_start_open_and_collapse_all_closes_them() {
        let tree = build_tree(&["a/b/c/d.rs", "a/e.rs", "z/y.rs", "top.rs"]);
        let mut expansion = TreeExpansion::default();
        let rows = visible_rows(&tree, &expansion);
        assert_eq!(
            names(&rows),
            ["a", "  b", "  e.rs", "z", "  y.rs", "top.rs"]
        );
        assert!(rows[0].open && !rows[1].open && !rows[5].open);

        expansion.set_open("a/b", true);
        expansion.set_open("z", false);
        assert_eq!(
            names(&visible_rows(&tree, &expansion)),
            ["a", "  b", "    c", "  e.rs", "z", "top.rs"]
        );

        expansion.collapse_all();
        assert_eq!(
            names(&visible_rows(&tree, &expansion)),
            ["a", "z", "top.rs"]
        );
        expansion.set_open("a", true);
        assert_eq!(
            names(&visible_rows(&tree, &expansion)),
            ["a", "  b", "  e.rs", "z", "top.rs"],
            "a folder opened after Collapse all shows its children closed"
        );
    }

    #[test]
    fn repeated_paths_empty_parts_and_backslashes_build_one_node_each() {
        let tree = build_tree(&[
            "media/logo.svg",
            "media\\logo.svg",
            "/media//intro.mp4",
            "media",
        ]);
        let rows = visible_rows(&tree, &TreeExpansion::default());
        assert_eq!(
            names(&rows),
            ["media", "  intro.mp4", "  logo.svg", "media"]
        );
        assert_eq!(
            rows.iter().map(|row| row.folder).collect::<Vec<_>>(),
            [true, false, false, false],
            "a file named like a folder stays a file"
        );
        assert_eq!(rows[1].path, "media/intro.mp4");
    }

    #[test]
    fn files_get_the_glyph_and_colour_of_their_kind() {
        let theme = Theme::light();
        for (file, kind, glyph) in [
            ("src/lib.rs", FileKind::Code, Icon::FileCode),
            ("Cargo.TOML", FileKind::Code, Icon::FileCode),
            ("shaders/glow.wgsl", FileKind::Code, Icon::FileCode),
            ("style/preset.json", FileKind::Json, Icon::FileJson),
            ("README.md", FileKind::Text, Icon::FileText),
            ("media/Logo.PNG", FileKind::Image, Icon::FileImage),
            ("media/clip.mov", FileKind::Video, Icon::FileVideo),
            ("media/voice-over.mp3", FileKind::Audio, Icon::FileMusic),
            ("fonts/Geist.woff2", FileKind::Font, Icon::FileType),
            ("Makefile", FileKind::Other, Icon::File),
        ] {
            assert_eq!(FileKind::of(file), kind, "{file}");
            assert_eq!(kind.icon(), glyph, "{file}");
        }
        assert_eq!(
            FileKind::Code.color(&theme),
            Hsla::from(theme.colors.accent)
        );
        assert_eq!(
            FileKind::Json.color(&theme),
            Hsla::from(theme.colors.warning)
        );
        assert_ne!(FileKind::Image.color(&theme), FileKind::Code.color(&theme));
    }
}
