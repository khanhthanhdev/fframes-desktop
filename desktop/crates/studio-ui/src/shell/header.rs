//! The window header, after Cap's editor header: the project's name, then undo and redo,
//! the project actions, Settings and Export in a row flush with the window's top edge.
//!
//! The header is a cached section. On Windows the row around it (in `layout.rs`) is the
//! window's drag area and holds the caption buttons, because a cached view does not replay
//! window control areas. On macOS the header leaves room for the traffic lights and moves
//! the window from its empty middle.

use gpui::{
    Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled, div,
    prelude::FluentBuilder, px,
};
use studio_engine::engine::backend::Command;
use studio_engine::timing::TimelineCommand;
use studio_theme::{METRICS, TextStyle, text_style};

use super::sections::{Section, section_probe};
use super::{Picker, StudioShell};
use crate::kit::{CAPTION_BUTTON_W, Divider, Icon, IconButtonSize, selectable};

impl StudioShell {
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let enabled = !self.busy;
        let project = self.presentation.project.clone();
        let compact = compact_header_controls(self.viewport[0], chrome_width(), project.is_some());
        let mut actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_2))
            // Keeps the Windows drag area behind from answering for the buttons.
            .occlude();
        let timeline = self.timeline_controls;
        for id in header_actions(project.is_some()) {
            let button = match id {
                "undo" | "redo" => {
                    let undo = id == "undo";
                    let can = if undo {
                        timeline.can_undo
                    } else {
                        timeline.can_redo
                    };
                    let command = if undo {
                        TimelineCommand::Undo
                    } else {
                        TimelineCommand::Redo
                    };
                    actions = actions.child(self.action_icon_button(
                        if undo {
                            "timeline-undo"
                        } else {
                            "timeline-redo"
                        },
                        if undo { "Undo" } else { "Redo" },
                        if undo { Icon::Undo } else { Icon::Redo },
                        IconButtonSize::Md,
                        enabled && timeline.editable && can,
                        cx,
                        move |s, cx| s.timeline_history(command.clone(), cx),
                    ));
                    if !undo {
                        actions = actions.child(Divider::vertical(&theme));
                    }
                    continue;
                }
                // App-level, so it shows with or without a project; it opens the settings
                // window or brings it to the front.
                "settings" => {
                    actions = actions.child(self.action_icon_button(
                        "settings",
                        "Settings",
                        Icon::Settings,
                        IconButtonSize::Md,
                        true,
                        cx,
                        |s, cx| s.open_settings(cx),
                    ));
                    continue;
                }
                "create" => self.action_button(
                    "create",
                    "Create…",
                    enabled,
                    false,
                    Some(Icon::Plus),
                    cx,
                    |s, cx| s.open_create(cx),
                ),
                "open" => self.action_button(
                    "open",
                    "Open…",
                    enabled,
                    false,
                    Some(Icon::FolderOpen),
                    cx,
                    |s, cx| s.pick(Picker::Open, cx),
                ),
                "import" => self.action_button(
                    "import",
                    "Import Rust…",
                    enabled,
                    false,
                    Some(Icon::Upload),
                    cx,
                    |s, cx| s.pick(Picker::Import, cx),
                ),
                "export" => self.action_button(
                    "export",
                    "Export…",
                    enabled && project.is_some() && self.presentation.agent_sdk.is_some(),
                    true,
                    Some(Icon::Download),
                    cx,
                    |s, cx| s.open_export(cx),
                ),
                // No icon, so it keeps its label in the compact header.
                _ => self.action_button(
                    "close",
                    "Close project",
                    enabled && project.is_some(),
                    false,
                    None,
                    cx,
                    |s, cx| s.dispatch(Command::Close, cx),
                ),
            };
            let button = if compact && id != "close" {
                button.icon_only()
            } else {
                button
            };
            actions = actions.child(button);
        }
        self.header_appearance.set(Some(theme.appearance));
        let title = project
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "fframes Studio".into());
        let mut text = div()
            .flex()
            .items_center()
            .gap(px(METRICS.space_3))
            .min_w_0()
            .child(
                text_style(div().truncate(), TextStyle::BodyStrong)
                    .child(selectable(title).over_drag_area()),
            );
        if let Some(p) = &project {
            let path = studio_bootstrap::external_path(&p.root)
                .display()
                .to_string();
            // Selectable rather than a tooltip: the text blocks the hover a tooltip needs,
            // and a triple click selects the whole path however it is truncated.
            text = text.child(
                text_style(div().id("project-path").truncate(), TextStyle::Small)
                    .text_color(theme.colors.text_2)
                    .child(selectable(path).over_drag_area()),
            );
        }
        div()
            .relative()
            .size_full()
            .flex()
            .items_center()
            .gap(px(METRICS.space_4))
            .px(px(if compact {
                METRICS.space_2
            } else {
                METRICS.space_4
            }))
            .when(cfg!(target_os = "macos"), |header| {
                header.child(
                    div()
                        .h_full()
                        .w(px(METRICS.layout.traffic_lights_w))
                        .flex_none(),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .min_w_0()
                    .gap(px(METRICS.space_3))
                    .child(
                        gpui::img("branding/studio-icon.png")
                            .size(px(METRICS.layout.header_logo))
                            .flex_none(),
                    )
                    .child(text),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .when(cfg!(target_os = "macos"), |area| {
                        area.on_mouse_down(MouseButton::Left, |event, window, _| {
                            if event.click_count == 2 {
                                window.titlebar_double_click();
                            } else {
                                window.start_window_move();
                            }
                        })
                    }),
            )
            .child(actions)
            .child(section_probe(Section::Header, cx))
            .into_any_element()
    }
}

/// Width the window's own controls take from the header: the caption buttons on Windows,
/// the traffic lights on macOS.
pub(super) fn chrome_width() -> f32 {
    if cfg!(target_os = "windows") {
        3. * CAPTION_BUTTON_W
    } else if cfg!(target_os = "macos") {
        METRICS.layout.traffic_lights_w
    } else {
        0.
    }
}

/// Whether the header actions drop their labels, after Cap's `compact_header_controls`:
/// when the title, the labelled actions and the window controls do not fit side by side.
/// The title keeps at least `title_min_w`; `actions_w` and `actions_closed_w` are the action
/// cluster's width with labels, Settings included, with a project open and without one:
/// 652 px (undo, redo and the divider included) and about 282 px in the test platform, plus
/// a margin for other systems' fonts.
fn compact_header_controls(viewport_width: f32, chrome_width: f32, project_open: bool) -> bool {
    let layout = METRICS.layout;
    let actions = if project_open {
        layout.actions_w
    } else {
        layout.actions_closed_w
    };
    viewport_width < layout.title_min_w + actions + chrome_width + 2. * METRICS.space_4
}

/// The header actions in Cap's order, Export last. Undo and redo edit the open project's
/// timeline; create, open and import live in the empty state while no project is open, so
/// each id is rendered once.
fn header_actions(project_open: bool) -> Vec<&'static str> {
    if project_open {
        vec![
            "undo", "redo", "create", "open", "import", "close", "settings", "export",
        ]
    } else {
        vec!["close", "settings", "export"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_hides_project_pickers_while_the_empty_state_shows() {
        assert_eq!(header_actions(false), ["close", "settings", "export"]);
        assert_eq!(
            header_actions(true),
            [
                "undo", "redo", "create", "open", "import", "close", "settings", "export"
            ]
        );
    }

    #[test]
    fn the_default_window_keeps_the_labels() {
        for chrome in [0., METRICS.layout.traffic_lights_w, 3. * CAPTION_BUTTON_W] {
            for project_open in [false, true] {
                assert!(!compact_header_controls(1275., chrome, project_open));
            }
        }
    }

    #[test]
    fn the_window_controls_and_an_open_project_need_room() {
        let caption = 3. * CAPTION_BUTTON_W;
        assert!(compact_header_controls(960., caption, true));
        assert!(!compact_header_controls(960., 0., true));
        assert!(!compact_header_controls(960., caption, false));
        assert!(compact_header_controls(400., 0., false));
    }
}
