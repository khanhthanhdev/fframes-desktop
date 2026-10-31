//! The files a task changed, as one card in the transcript with its Review button.

use gpui::{
    AnyElement, App, IntoElement, ParentElement, Pixels, RenderOnce, SharedString, Styled, Window,
    div, px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::{ROW_H, STATUS_MARK};
use crate::kit::{Button, ButtonSize, Icon, icon, selectable};

/// The most files the card lists before "…and N more".
pub const CHANGES_SHOWN: usize = 5;

/// Inside the card: 10 px left and right, 6 px top and bottom (Oleafly's `px-2.5 py-1.5`).
const PAD_X: f32 = 10.;
const PAD_Y: f32 = 6.;
/// Corner of the card: the control radius (Oleafly's `rounded-md`).
const RADIUS: Pixels = px(METRICS.radius.control);

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

/// A muted card after Oleafly's turn changes: `card` fill, `line` border at the control
/// radius, 10 × 6 px inside. The header has the 14 px file-diff glyph, the count in `Label`
/// and the compact Review button on the right; then one 24 px row per file (at most
/// [`CHANGES_SHOWN`]) with its mark and its path in 11 px mono.
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

    /// The Review button in the header, made compact (24 px) to sit on the header line.
    pub fn action(mut self, action: Button) -> Self {
        self.action = Some(action.size(ButtonSize::Sm).into_any_element());
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
            text_style(div(), TextStyle::CodeSmall)
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
        div()
            .w_full()
            .flex()
            .flex_col()
            .px(px(PAD_X))
            .py(px(PAD_Y))
            .rounded(RADIUS)
            .border(px(METRICS.size.divider))
            .border_color(colors.line)
            .bg(colors.card)
            .child(
                div()
                    .min_h(px(ROW_H))
                    .flex()
                    .items_center()
                    .gap(px(METRICS.space_2))
                    .child(icon(Icon::FileDiff, METRICS.size.icon_sm, colors.text_2))
                    .child(
                        text_style(div().flex_1().min_w_0(), TextStyle::Label)
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
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .text_color(colors.text_3)
                    .child(format!("…and {more} more"))
            }))
    }
}
