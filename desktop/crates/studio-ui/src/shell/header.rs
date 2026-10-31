//! The window header, after Oleafly's top bar (`TopToolbar.tsx`) on Cap's editor header
//! layout: the brand, then the open project as a breadcrumb (a chevron, the project's colour
//! dot, its name and its folder), and on the right compact icon actions in groups parted by
//! hairlines (undo and redo; create, open and import; close; settings) and Export as a split
//! button, its chevron opening the export choices. The row is flush with the window's top
//! edge.
//!
//! The header is a cached section. On Windows the row around it (in `layout.rs`) is the
//! window's drag area and holds the caption buttons, because a cached view does not replay
//! window control areas. On macOS the header leaves room for the traffic lights and moves
//! the window from its empty middle.

use gpui::{
    Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Rgba, Styled, div,
    prelude::FluentBuilder, px, svg,
};
use studio_engine::engine::backend::Command;
use studio_engine::timing::TimelineCommand;
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::sections::{Section, section_probe};
use super::{Picker, StudioShell};
use crate::kit::{CAPTION_BUTTON_W, Divider, Icon, IconButtonSize, SplitButton, selectable};

impl StudioShell {
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let enabled = !self.busy;
        let project = self.presentation.project.clone();
        let compact = compact_header_controls(self.viewport[0], chrome_width(), project.is_some());
        let timeline = self.timeline_controls;
        let mut actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(METRICS.space_1))
            // Keeps the Windows drag area behind from answering for the buttons.
            .occlude();
        for (index, group) in header_groups(project.is_some()).iter().enumerate() {
            if index > 0 {
                actions = actions.child(
                    div()
                        .flex_none()
                        .px(px(METRICS.space_1))
                        .child(Divider::vertical(&theme)),
                );
            }
            for id in group.iter().copied() {
                let (label, glyph, can) = match id {
                    "timeline-undo" => (
                        "Undo",
                        Icon::Undo,
                        enabled && timeline.editable && timeline.can_undo,
                    ),
                    "timeline-redo" => (
                        "Redo",
                        Icon::Redo,
                        enabled && timeline.editable && timeline.can_redo,
                    ),
                    "create" => ("Create…", Icon::Plus, enabled),
                    "open" => ("Open…", Icon::FolderOpen, enabled),
                    "import" => ("Import Rust…", Icon::FolderInput, enabled),
                    "close" => ("Close project", Icon::X, enabled && project.is_some()),
                    // App-level, so it shows with or without a project; it opens the settings
                    // window or brings it to the front.
                    _ => ("Settings", Icon::Settings, true),
                };
                actions = actions.child(self.action_icon_button(
                    id,
                    label,
                    glyph,
                    IconButtonSize::Lg,
                    can,
                    cx,
                    move |s, cx| match id {
                        "timeline-undo" => s.timeline_history(TimelineCommand::Undo, cx),
                        "timeline-redo" => s.timeline_history(TimelineCommand::Redo, cx),
                        "create" => s.open_create(cx),
                        "open" => s.pick(Picker::Open, cx),
                        "import" => s.pick(Picker::Import, cx),
                        "close" => s.dispatch(Command::Close, cx),
                        _ => s.open_settings(cx),
                    },
                ));
            }
        }
        actions = actions.child(
            div()
                .flex_none()
                .pl(px(METRICS.space_2))
                .child(self.export_split_button(compact, project.is_some(), cx)),
        );
        self.header_appearance.set(Some(theme.appearance));

        // Oleafly's brand, then the project as a breadcrumb. In the compact header the brand
        // keeps only its mark and the project only its name.
        let mut title = div()
            .flex()
            .items_center()
            .gap(px(METRICS.space_2))
            .min_w_0()
            .child(
                gpui::img("branding/studio-icon.png")
                    .size(px(METRICS.layout.header_logo))
                    .flex_none(),
            )
            .when(!compact || project.is_none(), |title| {
                title.child(
                    text_style(div().flex_none(), TextStyle::BodyStrong)
                        .child(selectable("fframes Studio").over_drag_area()),
                )
            });
        if let Some(p) = &project {
            let path = studio_bootstrap::external_path(&p.root)
                .display()
                .to_string();
            title = title
                .child(
                    svg()
                        .path(Icon::ChevronRight.path())
                        .size(px(METRICS.size.icon_md))
                        .flex_none()
                        .text_color(theme.colors.text_3),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(METRICS.space_2))
                        .min_w_0()
                        .child(
                            div()
                                .flex_none()
                                .size(px(METRICS.layout.project_dot))
                                .rounded_full()
                                .bg(project_dot_color(&theme, &p.name)),
                        )
                        .child(
                            text_style(div().min_w_0().truncate(), TextStyle::Body)
                                .text_color(theme.colors.text_2)
                                .child(selectable(p.name.clone()).over_drag_area()),
                        )
                        // Selectable rather than a tooltip: the text blocks the hover a
                        // tooltip needs, and a triple click selects the whole path however
                        // it is truncated.
                        .when(!compact, |crumb| {
                            crumb.child(
                                text_style(div().id("project-path").truncate(), TextStyle::Small)
                                    .min_w_0()
                                    .text_color(theme.colors.text_3)
                                    .child(selectable(path).over_drag_area()),
                            )
                        }),
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
            .child(title)
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
            .children(self.render_export_menu(cx))
            .into_any_element()
    }

    /// Export, the view's one Primary action, with its menu behind the chevron. The action
    /// needs an open project and an installed SDK; the menu always opens, as Export
    /// settings needs neither.
    fn export_split_button(
        &self,
        compact: bool,
        project_open: bool,
        cx: &mut Context<Self>,
    ) -> SplitButton {
        let entity = cx.entity();
        let menu = cx.entity();
        let probe = self.bounds_probe(&gpui::ElementId::Name("export".into()), cx);
        let button = SplitButton::new(
            &self.theme,
            "export",
            "export-options",
            "Export…",
            "Export options",
        )
        .icon(Icon::Download.path())
        .icon_only(compact)
        .disabled(!(!self.busy && project_open && self.presentation.agent_sdk.is_some()))
        .open(self.export_menu_open())
        .on_click(move |_, _, cx| entity.update(cx, |shell, cx| shell.open_export(cx)))
        .on_open(move |bounds, window, cx| {
            menu.update(cx, |shell, cx| {
                shell.toggle_export_menu(*bounds, window, cx)
            });
        });
        match probe {
            Some(probe) => button.on_bounds(probe.into_bounds_handler()),
            None => button,
        }
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

/// Whether the header goes compact (the brand drops its name, the project its folder and
/// Export its label), after Cap's `compact_header_controls`: when the title and the full
/// actions do not fit beside the window controls. The title keeps at least `title_min_w`
/// (the brand, the chevron, the dot and ten characters of the name); `actions_w` and
/// `actions_closed_w` are the action cluster's full width with a project open and without
/// one, measured from the rendered bounds on Windows with Geist plus a margin for other
/// systems' fonts (see `the_breakpoints_cover_the_measured_actions`).
fn compact_header_controls(viewport_width: f32, chrome_width: f32, project_open: bool) -> bool {
    let layout = METRICS.layout;
    let actions = if project_open {
        layout.actions_w
    } else {
        layout.actions_closed_w
    };
    viewport_width < layout.title_min_w + actions + chrome_width + 2. * METRICS.space_4
}

/// The header's icon actions by group, a hairline between groups, Export after them. Undo,
/// redo, create, open and import need an open project here: create, open and import live
/// in the empty state while no project is open, so each id is rendered once.
fn header_groups(project_open: bool) -> Vec<&'static [&'static str]> {
    if project_open {
        vec![
            &["timeline-undo", "timeline-redo"],
            &["create", "open", "import"],
            &["close"],
            &["settings"],
        ]
    } else {
        vec![&["close"], &["settings"]]
    }
}

/// The project's colour dot, after Oleafly's project colour: one of the theme's track hues
/// and the warning amber, chosen by a hash of the name, so a project keeps its colour across
/// launches and machines.
fn project_dot_color(theme: &Theme, name: &str) -> Rgba {
    let colors = theme.colors;
    let hues = [
        colors.track_clip,
        colors.track_overlay,
        colors.track_audio,
        colors.warning,
    ];
    hues[(name_hash(name) % hues.len() as u64) as usize]
}

/// FNV-1a: stable across runs and Rust versions, unlike the standard library's hasher.
fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use studio_theme::Appearance;

    use super::*;

    #[test]
    fn header_hides_project_pickers_while_the_empty_state_shows() {
        let flat = |open| header_groups(open).concat();
        assert_eq!(flat(false), ["close", "settings"]);
        assert_eq!(
            flat(true),
            [
                "timeline-undo",
                "timeline-redo",
                "create",
                "open",
                "import",
                "close",
                "settings"
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
        assert!(compact_header_controls(800., caption, true));
        assert!(!compact_header_controls(800., 0., true));
        assert!(!compact_header_controls(800., caption, false));
        assert!(compact_header_controls(400., 0., false));
    }

    #[test]
    fn the_breakpoints_cover_the_measured_actions() {
        // The rendered clusters on Windows with Geist: 418 px with a project open (seven
        // 32 px icons, three dividers and Export with its label and chevron), 214 px without
        // one; the title's minimum 259 px (the 131 px brand, the chevron, the dot and ten
        // characters of the name).
        let layout = METRICS.layout;
        assert!(layout.actions_w >= 418. && layout.actions_w <= 418. + 40.);
        assert!(layout.actions_closed_w >= 214. && layout.actions_closed_w <= 214. + 40.);
        assert!(layout.title_min_w >= 259. && layout.title_min_w <= 259. + 40.);
        // The full header goes exactly where the title would drop below its minimum.
        let caption = 3. * CAPTION_BUTTON_W;
        let edge = layout.title_min_w + layout.actions_w + caption + 2. * METRICS.space_4;
        assert!(!compact_header_controls(edge, caption, true));
        assert!(compact_header_controls(edge - 1., caption, true));
    }

    #[test]
    fn a_project_keeps_its_dot_colour() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let theme = Theme::new(appearance);
            assert_eq!(
                project_dot_color(&theme, "Launch video"),
                project_dot_color(&theme, "Launch video")
            );
        }
        // Pinned, so a change of hash or palette order is a deliberate one.
        assert_eq!(name_hash(""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(name_hash("a"), 0xaf63_dc4c_8601_ec8c);
        // Different names spread over the hues.
        let theme = Theme::new(Appearance::Light);
        let distinct: std::collections::HashSet<_> = [
            "Launch video",
            "Quarterly review",
            "Old demo",
            "Intro",
            "Credits",
            "Promo",
        ]
        .iter()
        .map(|name| format!("{:?}", project_dot_color(&theme, name)))
        .collect();
        assert!(distinct.len() > 1);
    }
}
