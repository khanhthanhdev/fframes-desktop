//! The review controls that are not part of the diff itself: the Reject button (which turns
//! into a discard when every file is toggled off), the confirmation that follows, and the
//! notice for files the listing left out.

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, div, px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::paint::Colors;
use super::{Changes, ChangesEvent};
use crate::kit::{Button, Emphasis};

impl Changes {
    /// "Reject N" for some files; "Discard all" when every file is toggled off, because the
    /// engine treats rejecting everything as a discard of the candidate. That one asks first.
    pub(super) fn reject_button(
        &self,
        rejected: Vec<String>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.busy;
        if self.rejects_everything() {
            return div()
                .debug_selector(|| "changes-reject".into())
                .child(
                    Button::new(theme, "changes-reject-button")
                        .emphasis(Emphasis::Danger)
                        .label("Discard all…")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            if !this.busy {
                                this.confirm_discard = true;
                                cx.notify();
                            }
                        })),
                )
                .into_any_element();
        }
        // No empty path ever leaves the viewer: it would name no file.
        let paths: Vec<String> = rejected.into_iter().filter(|p| !p.is_empty()).collect();
        div()
            .debug_selector(|| "changes-reject".into())
            .child(
                Button::new(theme, "changes-reject-button")
                    .emphasis(Emphasis::Danger)
                    .label(if paths.is_empty() {
                        SharedString::from("Reject")
                    } else {
                        SharedString::from(format!("Reject {}", paths.len()))
                    })
                    .disabled(paths.is_empty() || busy)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if !this.busy && !paths.is_empty() {
                            cx.emit(ChangesEvent::Reject(paths.clone()));
                        }
                    })),
            )
            .into_any_element()
    }

    /// The question under the toolbar after "Discard all…".
    pub(super) fn confirm_strip(
        &self,
        theme: &Theme,
        colors: &Colors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.confirm_discard {
            return None;
        }
        Some(
            text_style(div(), TextStyle::Label)
                .id("changes-confirm")
                .w_full()
                .flex_none()
                .px(px(METRICS.space_3))
                .py(px(METRICS.space_2))
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(METRICS.space_2))
                .bg(colors.del_wash)
                .border_b_1()
                .border_color(colors.line)
                .text_color(colors.text)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child("Every file is turned off. This discards the whole candidate; nothing is applied."),
                )
                .child(
                    div().debug_selector(|| "changes-discard-confirm".into()).child(
                        Button::new(theme, "changes-discard-confirm-button").emphasis(Emphasis::Danger)
                        .label("Discard candidate")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.confirm_discard = false;
                            cx.emit(ChangesEvent::Discard);
                            cx.notify();
                        })),
                    ),
                )
                .child(
                    div().debug_selector(|| "changes-discard-cancel".into()).child(
                        Button::new(theme, "changes-discard-cancel-button").emphasis(Emphasis::Neutral)
                        .label("Keep reviewing")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.confirm_discard = false;
                            cx.notify();
                        })),
                    ),
                )
                .into_any_element(),
        )
    }

    /// "N more files not shown": the listing was cut short, so these files have no row, no
    /// toggle and cannot be rejected from here.
    pub(super) fn more_footer(&self, colors: &Colors) -> Option<AnyElement> {
        if self.more == 0 {
            return None;
        }
        Some(
            text_style(div(), TextStyle::Label)
                .debug_selector(|| "changes-more".into())
                .w_full()
                .flex_none()
                .h(px(METRICS.size.control_h))
                .px(px(METRICS.space_3))
                .flex()
                .items_center()
                .border_t_1()
                .border_color(colors.line)
                .text_color(colors.muted)
                .child(SharedString::from(more_files_text(self.more)))
                .into_any_element(),
        )
    }
}

pub(super) fn more_files_text(more: usize) -> String {
    format!(
        "{more} more {} not shown",
        if more == 1 { "file" } else { "files" }
    )
}
