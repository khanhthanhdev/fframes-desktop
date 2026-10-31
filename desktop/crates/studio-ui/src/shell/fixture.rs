//! The window's chrome as the UI gallery's `shell` capture page draws it: the header, the
//! notices, the sidebar's tab rail, the transport bar and the status bar, in three states.

use std::path::PathBuf;

use gpui::{Context, InteractiveElement, IntoElement, ParentElement, Styled, div, px};
use studio_engine::project_presentation::ProjectPresentation;
use studio_engine::services::export_service::ExportProgress;
use studio_project::{ProjectId, SourceRevision};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::StudioShell;
use super::left_column::{SidebarTab, tab_rail};

/// The window the pieces are laid out in: the gallery's content width.
const WIDTH: f32 = 1200.;

impl StudioShell {
    /// The chrome pieces in state `state` (0 no project, 1 a project with an export running,
    /// 2 the same with an error and the status details open), drawn with `theme`. For the UI
    /// gallery's captures; it changes the shell's state, so the shell is the gallery's own.
    pub fn render_shell_fixture(
        &mut self,
        state: usize,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let revision = |digit: &str| {
            SourceRevision::try_from(digit.repeat(64)).expect("64 hexadecimal digits")
        };
        self.presentation.project = (state > 0).then(|| ProjectPresentation {
            id: ProjectId::try_from("launch-video".to_owned()).expect("a valid id"),
            name: "Launch video".into(),
            root: PathBuf::from("launch-video"),
            source: revision("a"),
            accepted: revision(if state == 1 { "a" } else { "b" }),
            job: "Idle".into(),
            interrupted: state == 2,
            draft: None,
            recovery_notice: None,
            checkpoint: PathBuf::from("launch-video/.checkpoint"),
            files: Vec::new(),
            assets: Vec::new(),
            styles: Vec::new(),
            file_count: 0,
            history_available: true,
            worker_available: state == 1,
        });
        self.export_status = (state == 1).then_some(ExportProgress::Rendering {
            done: 1260,
            total: 3600,
        });
        self.error = (state == 2).then(|| {
            "The build failed\nerror[E0425]: cannot find value `title` in this scope".into()
        });
        self.status_open = state == 2;
        self.viewport = [WIDTH, 720.];
        let main = std::mem::replace(&mut self.theme, theme);
        let header = self.render_header(cx);
        let notices = self.render_notices(cx);
        // The transport at the narrowest width of each of its modes: Full, Compact and Narrow.
        let player_width = self.player_width;
        let layout = METRICS.layout;
        let transports: Vec<_> = [
            ("Full", layout.transport_compact_below),
            ("Compact", layout.transport_narrow_below),
            ("Narrow", layout.transport_total_hidden_below),
        ]
        .into_iter()
        .map(|(mode, width)| {
            self.player_width = width;
            (mode, width, self.render_transport(cx))
        })
        .collect();
        self.player_width = player_width;
        let status = self.render_status_strip(cx);
        let empty = (state == 0).then(|| self.render_empty_state(cx));
        self.theme = main;
        let rail = tab_rail(&theme, SidebarTab::Props, state == 1);
        let caption = |text: &str| {
            text_style(div(), TextStyle::Small)
                .text_color(theme.colors.text_3)
                .child(text.to_owned())
        };
        div()
            .w(px(WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_4))
            .p(px(METRICS.space_4))
            .bg(theme.colors.window)
            .child(caption("Header"))
            .child(header)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_4))
                    .children(notices),
            )
            .child(caption("Sidebar tab rail (Props selected)"))
            .child(
                div()
                    .w(px(METRICS.layout.sidebar_w))
                    .flex()
                    .flex_col()
                    .bg(theme.colors.sidebar)
                    .child(rail),
            )
            .children(
                empty.map(|empty| div().h(px(METRICS.layout.player_min_h)).flex().child(empty)),
            )
            .children(transports.into_iter().flat_map(|(mode, width, transport)| {
                [
                    caption(&format!("Transport, {mode} ({width} px player)")).into_any_element(),
                    // Its own id, so the three bars' controls do not share element ids.
                    div()
                        .id(mode)
                        .flex_none()
                        .w(px(width))
                        .child(transport)
                        .into_any_element(),
                ]
            }))
            .child(caption("Status bar"))
            .children(status)
            .into_any_element()
    }
}
