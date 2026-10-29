//! The files a task changed, as one card in the transcript with its Review button.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::{ROW_H, STATUS_MARK};
use crate::kit::{Card, selectable};

/// The most files the card lists before "…and N more".
pub const CHANGES_SHOWN: usize = 5;

/// How a file changed: one letter in its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeMark {
    Added,
    Modified,
    Deleted,
    /// Only the file mode changed.
    Mode,
}

impl ChangeMark {
    fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Mode => "X",
        }
    }
}

/// One changed file: how, and its project path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub mark: ChangeMark,
    pub path: SharedString,
}

/// `card` fill, `line` border, a `BodyStrong` count with the action on the right, then one
/// 24 px row per file (at most [`CHANGES_SHOWN`]): the mark, the name and its folder.
#[derive(IntoElement)]
pub struct ChangesCard {
    theme: Theme,
    total: usize,
    files: Vec<ChangedFile>,
    action: Option<AnyElement>,
}

impl ChangesCard {
    /// `total` counts every changed file, including the ones `files` leaves out.
    pub fn new(theme: &Theme, total: usize, files: Vec<ChangedFile>) -> Self {
        Self {
            theme: *theme,
            total,
            files,
            action: None,
        }
    }

    /// The Review button in the header.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for ChangesCard {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ChangesCard {
            theme,
            total,
            files,
            action,
        } = self;
        let colors = theme.colors;
        let shown = files.len().min(CHANGES_SHOWN);
        let more = total.saturating_sub(shown);
        let rows = files.into_iter().take(CHANGES_SHOWN).map(|file| {
            let color = match file.mark {
                ChangeMark::Added => colors.success,
                ChangeMark::Modified => colors.text_2,
                ChangeMark::Deleted => colors.danger,
                ChangeMark::Mode => colors.text_3,
            };
            let (folder, name) = match file.path.rsplit_once('/') {
                Some((folder, name)) => (Some(format!("{folder}/")), name.to_owned()),
                None => (None, file.path.to_string()),
            };
            text_style(div(), TextStyle::Label)
                .h(px(ROW_H))
                .flex()
                .items_center()
                .gap(px(METRICS.space_2))
                .child(
                    div()
                        .w(px(STATUS_MARK))
                        .flex_none()
                        .text_color(color)
                        .child(file.mark.letter()),
                )
                .child(
                    div()
                        .flex()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .children(
                            folder.map(|folder| div().text_color(colors.text_3).child(folder)),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(colors.text_1)
                                .child(name),
                        ),
                )
        });
        Card::raised(&theme)
            .w_full()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .p(px(METRICS.space_3))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(METRICS.space_2))
                    .child(
                        text_style(div().flex_1().min_w_0(), TextStyle::BodyStrong)
                            .text_color(colors.text_1)
                            .child(selectable(format!(
                                "{total} file{} changed",
                                if total == 1 { "" } else { "s" }
                            ))),
                    )
                    .children(action),
            )
            .child(div().flex().flex_col().children(rows))
            .children((more > 0).then(|| {
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_3)
                    .child(format!("…and {more} more"))
            }))
    }
}
