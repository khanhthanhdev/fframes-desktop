//! What a click on a link in agent text does, and the confirmation shown before an external URL
//! opens.
//!
//! The decision is [`activate`]: it goes through the link policy in [`super::links`] and returns
//! an effect to perform, a URL to confirm, or nothing. Revealing a project file never launches it
//! and an external URL opens only after the reader accepted the complete address.

use std::path::Path;
use std::rc::Rc;

use gpui::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, Pixels, Point, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use studio_markdown::mend::PENDING_LINK_URL;
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::links::{self, LinkAction, LinkEffect, LinkTarget, action_for, classify, effect};
use crate::kit::{Button, Emphasis, Popover};

/// The link confirmation's width, the least it shrinks to in a narrow window, and the tallest
/// its address grows before it scrolls.
const CONFIRM_W: f32 = 360.;
const CONFIRM_MIN_W: f32 = 336.;
const URL_MAX_H: f32 = 96.;

/// Performs the effect of a link click. The default hands it to the OS; tests record it.
pub type LinkSink = Rc<dyn Fn(&LinkEffect, &mut App)>;

pub fn os_sink() -> LinkSink {
    Rc::new(links::perform)
}

/// An external URL waiting for the reader's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    pub url: String,
    pub at: Point<Pixels>,
}

/// The outcome of a click on a link.
#[derive(Debug, PartialEq, Eq)]
pub enum Activation {
    /// Do this now.
    Perform(LinkEffect),
    /// Show this complete URL and ask first.
    Confirm(String),
    /// Nothing: not a link the policy acts on.
    Ignored,
}

/// Decides what clicking a link to `dest` does. `project_root` is `None` when the row does not
/// know its project, which leaves project-file links inert. Reads the filesystem for project
/// files only.
pub fn activate(dest: &str, project_root: Option<&Path>) -> Activation {
    if dest == PENDING_LINK_URL {
        return Activation::Ignored;
    }
    let action = match project_root {
        Some(root) => links::resolve(dest, root).1,
        None => match classify(dest) {
            target @ LinkTarget::External { .. } => action_for(&target, Path::new("")),
            _ => LinkAction::None,
        },
    };
    match action {
        LinkAction::ConfirmExternal(url) => Activation::Confirm(url),
        reveal @ LinkAction::Reveal(_) => {
            effect(&reveal, false).map_or(Activation::Ignored, Activation::Perform)
        }
        LinkAction::None => Activation::Ignored,
    }
}

/// The effect of accepting `url` in the confirmation.
pub fn accepted(url: &str) -> Option<LinkEffect> {
    // Only a URL the policy still calls external opens, whatever the caller held on to.
    match classify(url) {
        LinkTarget::External { url } => effect(&LinkAction::ConfirmExternal(url), true),
        _ => None,
    }
}

/// The confirmation: a dismissing backdrop over the window and a popover with the full URL.
pub fn confirm_overlay(
    theme: &Theme,
    id: u64,
    confirm: &Confirm,
    viewport: gpui::Size<Pixels>,
    on_open: impl Fn(&mut Window, &mut App) + 'static,
    on_cancel: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let on_cancel = Rc::new(on_cancel);
    let dismiss = on_cancel.clone();
    let body = Popover::new(theme)
        .relative()
        .id(SharedString::from(format!("md-confirm-{id}")))
        .role(Role::Dialog)
        .occlude()
        .gap(px(METRICS.space_2))
        .p(px(METRICS.space_3))
        .w(px(CONFIRM_W))
        .max_w(px(CONFIRM_MIN_W).max(viewport.width - px(METRICS.space_6 * 2.)))
        .child(
            text_style(div(), TextStyle::Body)
                .text_color(theme.colors.text_1)
                .child("Open this link in your browser?"),
        )
        .child(
            text_style(div(), TextStyle::Small)
                .id(SharedString::from(format!("md-confirm-url-{id}")))
                .text_color(theme.colors.text_2)
                .max_h(px(URL_MAX_H))
                .overflow_y_scroll()
                // The whole address, wrapped anywhere: nothing of it is truncated.
                .child(SharedString::from(confirm.url.clone())),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(px(METRICS.space_2))
                .child(
                    div().debug_selector(|| "md-confirm-cancel".into()).child(
                        Button::new(theme, SharedString::from(format!("md-confirm-cancel-{id}")))
                            .emphasis(Emphasis::Neutral)
                            .label("Cancel")
                            .on_click(move |_, window, cx| on_cancel(window, cx)),
                    ),
                )
                .child(
                    div().debug_selector(|| "md-confirm-open".into()).child(
                        Button::new(theme, SharedString::from(format!("md-confirm-open-{id}")))
                            .emphasis(Emphasis::Primary)
                            .label("Open")
                            .on_click(move |_, window, cx| on_open(window, cx)),
                    ),
                ),
        );
    let layer = div()
        .relative()
        .w(viewport.width)
        .h(viewport.height)
        .child(
            div()
                .id(SharedString::from(format!("md-confirm-backdrop-{id}")))
                .occlude()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_click(move |_, window, cx| dismiss(window, cx)),
        )
        .child(
            gpui::anchored()
                .position(confirm.at)
                .snap_to_window_with_margin(px(METRICS.space_3))
                .child(body),
        );
    gpui::deferred(
        gpui::anchored()
            .position_mode(gpui::AnchoredPositionMode::Window)
            .position(gpui::Point::default())
            .child(layer),
    )
    .with_priority(2)
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("tools")).unwrap();
        fs::write(dir.path().join("tools").join("x.cmd"), "@echo off").unwrap();
        dir
    }

    #[test]
    fn a_project_file_link_reveals_and_never_opens() {
        let dir = project();
        match activate("tools/x.cmd", Some(dir.path())) {
            Activation::Perform(LinkEffect::Reveal(path)) => {
                assert!(path.ends_with("x.cmd"));
            }
            other => panic!("expected a reveal, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_project_file_does_nothing() {
        let dir = project();
        assert_eq!(
            activate("tools/none.cmd", Some(dir.path())),
            Activation::Ignored
        );
    }

    #[test]
    fn an_external_link_asks_with_the_complete_url_and_opens_only_when_accepted() {
        let url = "https://example.com/a/very/long/path?query=1&other=two#frag";
        assert_eq!(activate(url, None), Activation::Confirm(url.to_owned()));
        assert_eq!(accepted(url), Some(LinkEffect::OpenUrl(url.to_owned())));
    }

    #[test]
    fn the_pending_placeholder_and_refused_links_are_ignored() {
        let dir = project();
        for dest in [
            PENDING_LINK_URL,
            "javascript:alert(1)",
            "file:///c:/windows/system32/calc.exe",
            "C:\\Windows\\notepad.exe",
            "../escape.txt",
            "",
        ] {
            assert_eq!(
                activate(dest, Some(dir.path())),
                Activation::Ignored,
                "{dest}"
            );
            assert_eq!(activate(dest, None), Activation::Ignored, "{dest}");
        }
    }

    #[test]
    fn project_files_are_inert_without_a_project() {
        assert_eq!(activate("tools/x.cmd", None), Activation::Ignored);
    }
}
