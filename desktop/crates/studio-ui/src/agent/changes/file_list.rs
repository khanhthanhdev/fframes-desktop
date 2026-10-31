//! The file list beside the diff: each file with its +/- counts and, before apply, an accept
//! toggle.

use gpui::{
    ClickEvent, Context, InteractiveElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::model::FileDiff;
use super::paint::Colors;
use super::{Changes, Review};
use crate::kit::Toggle;

pub const LIST_W: f32 = 248.0;

/// `+a -d` for a file, or what stands in for it.
fn counts(file: &FileDiff, colors: &Colors) -> gpui::Div {
    let row = div().flex().flex_row().gap(px(METRICS.space_1)).flex_none();
    if file.not_shown.is_some() {
        return row.child(div().text_color(colors.faint).child("not shown"));
    }
    row.child(
        div()
            .text_color(colors.add)
            .child(SharedString::from(format!("+{}", file.additions))),
    )
    .child(
        div()
            .text_color(colors.del)
            .child(SharedString::from(format!("-{}", file.deletions))),
    )
}

impl Changes {
    pub(super) fn file_list(
        &self,
        theme: &Theme,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let mut list = div()
            .id("changes-file-list")
            .w(px(LIST_W))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(colors.line)
            .py(px(METRICS.space_1));
        for (ix, file) in self.files.iter().enumerate() {
            let (dir, name) = file.path.rsplit_once('/').unwrap_or(("", &file.path));
            let jump = file.path.clone();
            let toggle_path = file.path.clone();
            let accepted = !self.rejected.contains(&file.path);
            // Paths and counts in 11 px mono, like the change card in the transcript.
            let mut line = text_style(div(), TextStyle::CodeSmall)
                .id(SharedString::from(format!("changes-list-{ix}")))
                .w_full()
                .h(px(METRICS.size.control_h))
                .px(px(METRICS.space_3))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(METRICS.space_2))
                .text_color(colors.text)
                .hover(|el| el.bg(gpui::Hsla::from(theme.colors.ctl_hover)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.reveal(&jump, cx);
                }));
            if self.review == Review::Pending {
                line = line.child(
                    div()
                        .debug_selector(move || format!("changes-toggle-{ix}"))
                        .child(
                            Toggle::new(theme, format!("changes-accept-{ix}"), accepted)
                                .disabled(self.busy)
                                .label(format!("Keep {}", file.path))
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.stop_propagation();
                                    if this.busy {
                                        return;
                                    }
                                    let accepted = this.rejected.contains(&toggle_path);
                                    this.set_accepted(&toggle_path, accepted, cx);
                                })),
                        ),
                );
            }
            list = list.child(
                line.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .child(div().text_color(colors.faint).child(SharedString::from(
                                    if dir.is_empty() {
                                        String::new()
                                    } else {
                                        format!("{dir}/")
                                    },
                                )))
                                .child(SharedString::from(name.to_owned())),
                        ),
                )
                .child(counts(file, colors)),
            );
        }
        list
    }
}

/// Totals for the toolbar: files, additions, deletions.
pub fn totals(files: &[FileDiff]) -> (usize, u32, u32) {
    files.iter().fold((files.len(), 0, 0), |(n, a, d), file| {
        (n, a + file.additions, d + file.deletions)
    })
}
