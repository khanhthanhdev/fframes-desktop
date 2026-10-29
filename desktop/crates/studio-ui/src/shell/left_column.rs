//! The tabbed sidebar right of the player (Agent, Project, Assets, Style, Props and Slides),
//! and the empty state with recent projects.

use std::path::Path;

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, prelude::FluentBuilder, px,
};
use studio_engine::engine::backend::Command;
use studio_theme::{METRICS, TextStyle, text_style};

use super::sections::{Section, box_probe, section_probe};
use super::settings::SettingsPage;
use super::{Picker, StudioShell};
use crate::kit::{
    Badge, Button, ButtonSize, Card, CollapsibleSection, Divider, Emphasis, EmptyState, FormField,
    Icon, IconButton, IconButtonSize, List, ListRow, PanelSection, StatusDot, TabRail, TabRailItem,
    Tone, selectable,
};

impl StudioShell {
    /// The sidebar card right of the player, after Cap's config sidebar: the tab rail
    /// above the selected tab's body. It is not cached itself; the agent panel and the
    /// drawer contents are separate cached sections, and only the selected one is drawn.
    pub(super) fn render_sidebar(
        &self,
        sections: &super::sections::Sections,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let entity = cx.entity();
        let selected = self.sidebar_tab;
        let attention = self.agent_attention.waiting && selected != SidebarTab::Agent;
        let rail = tab_rail(&theme, selected, attention).on_select(move |index, _, cx| {
            entity.update(cx, |shell, cx| {
                shell.select_sidebar_tab(SidebarTab::from_index(*index), cx)
            });
        });
        let body = match selected {
            SidebarTab::Agent => sections.agent.clone(),
            _ => sections.drawer.clone(),
        };
        Card::raised(&theme)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(box_probe("agent-column", cx))
            .child(rail)
            .child(body.cached(gpui::StyleRefinement::default().flex_1().min_h_0().w_full()))
            .into_any_element()
    }

    pub(super) fn select_sidebar_tab(&mut self, tab: SidebarTab, cx: &mut Context<Self>) {
        if self.sidebar_tab != tab {
            self.sidebar_tab = tab;
            cx.notify();
        }
    }

    /// The selected Project, Assets, Style, Props or Slides tab's contents (a cached section).
    pub(super) fn render_drawer(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let content = match self.sidebar_tab {
            SidebarTab::Assets => self.render_assets_tab(cx),
            SidebarTab::Style => self.render_style_tab(cx),
            SidebarTab::Props => self.props_panel.clone().into_any_element(),
            SidebarTab::Slides => self.slides_panel.clone().into_any_element(),
            // The Agent tab draws the agent section instead; this one is not in the tree.
            SidebarTab::Project | SidebarTab::Agent => self.render_project_tab(cx),
        };
        div()
            .id("drawer")
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .p(px(METRICS.space_4))
            .child(content)
            .child(section_probe(Section::Drawer, cx))
            .into_any_element()
    }

    /// A project file's row: icon and name; a click reveals the file. The full name is the
    /// tooltip.
    fn file_row(&self, root: &Path, file: &str, glyph: Icon) -> ListRow {
        let path = root.join(file);
        ListRow::new(&self.theme, format!("file-{file}"))
            .icon(glyph)
            .title(file.to_owned())
            .tooltip(file.to_owned())
            .on_click(move |_, cx| cx.reveal_path(&path))
    }

    /// The file list of a project tab, or the hint shown while no project is open.
    fn file_list(
        &self,
        files: Option<&[String]>,
        root: Option<&Path>,
        glyph: Icon,
        empty: &'static str,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let list = match (files, root) {
            (Some(files), Some(root)) => List::new()
                .children(files.iter().map(|file| self.file_row(root, file, glyph)))
                .empty(EmptyState::new(&theme, empty)),
            _ => List::new().empty(EmptyState::new(
                &theme,
                "Create or open a project to browse its files.",
            )),
        };
        div()
            .id("inventory")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(list)
    }

    fn render_project_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let project = self.presentation.project.clone();
        let files = self.file_list(
            project.as_ref().map(|p| p.files.as_slice()),
            project.as_ref().map(|p| p.root.as_path()),
            Icon::File,
            "No files to show",
        );
        let mut section = PanelSection::new(&theme, "Files")
            .grow()
            .when_some(project.as_ref(), |section, p| {
                section.trailing(Badge::new(p.file_count.to_string()))
            })
            .child(files);
        if let Some(p) = &project
            && p.file_count > p.files.len()
        {
            section = section.child(
                text_style(div(), TextStyle::Small)
                    .flex_none()
                    .text_color(theme.colors.text_3)
                    .child(selectable(format!(
                        "Showing {} of {} files",
                        p.files.len(),
                        p.file_count
                    ))),
            );
        }
        // The files and the library start at their content's height and share what is
        // missing, so neither collapses in a short window.
        let mut tab = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_3))
            .child(div().flex_auto().min_h_0().flex().flex_col().child(section));

        if project.is_some() {
            let entity = cx.entity();
            let library = div()
                .id("left-library")
                .min_h_0()
                .max_h(px(METRICS.layout.library_max_h))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(METRICS.space_3))
                .child(Divider::horizontal(&theme))
                .child(
                    CollapsibleSection::new("recents-section", self.recents_open)
                        .header(
                            text_style(div(), TextStyle::Label)
                                .text_color(theme.colors.text_2)
                                .child("Recent projects"),
                        )
                        .content(div().pt(px(METRICS.space_2)).child(self.render_recents(cx)))
                        .on_toggle(move |open, _, cx| {
                            entity.update(cx, |shell, cx| {
                                shell.recents_open = *open;
                                cx.notify();
                            });
                        }),
                );
            tab = tab.child(library);
        }
        tab.into_any_element()
    }

    fn render_assets_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let project = self.presentation.project.clone();
        let files = self.file_list(
            project.as_ref().map(|p| p.assets.as_slice()),
            project.as_ref().map(|p| p.root.as_path()),
            Icon::Image,
            "No assets imported",
        );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(METRICS.space_3))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(PanelSection::new(&self.theme, "Assets").grow().child(files)),
            )
            .child(self.action_button(
                "asset",
                "Copy asset…",
                !self.busy && project.is_some(),
                false,
                Some(Icon::Plus),
                cx,
                |s, cx| s.pick(Picker::Asset, cx),
            ))
            .into_any_element()
    }

    fn render_style_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let styles = self.presentation.project.as_ref().map(|p| {
            PanelSection::new(&theme, "Style").child(
                List::new()
                    .children(
                        p.styles
                            .iter()
                            .map(|file| self.file_row(&p.root, file, Icon::File)),
                    )
                    .empty(EmptyState::new(&theme, "No style preset configured")),
            )
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(METRICS.space_3))
            .children(styles.map(|styles| div().w_full().child(styles)))
            // The presets themselves are chosen in Settings → Presets.
            .child(self.action_button(
                "open-presets",
                "Style presets…",
                true,
                false,
                Some(Icon::Palette),
                cx,
                |s, cx| s.open_settings_page(SettingsPage::Presets, cx),
            ))
            .into_any_element()
    }

    /// The recent-projects list. Rendered once: in the left column while a project is
    /// open, in the empty state otherwise, so its element ids stay unique.
    fn render_recents(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let enabled = !self.busy;
        let rows = self.presentation.recents.clone().into_iter().map(|recent| {
            let id = String::from(recent.id.clone());
            let shown_path = studio_bootstrap::external_path(&recent.path)
                .display()
                .to_string();

            // Shown on pointer hover and while keyboard focus is inside them.
            let mut actions = div()
                .id(format!("recent-actions-{id}"))
                .flex()
                .items_center()
                .gap(px(METRICS.space_1));
            for action in recent_row_actions(recent.missing) {
                let entity = cx.entity();
                let target = recent.id.clone();
                actions = actions.child(match action {
                    "locate" => Button::new(&theme, format!("locate-{id}"))
                        .label("Locate…")
                        .size(ButtonSize::Sm)
                        .emphasis(Emphasis::Neutral)
                        .disabled(!enabled)
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |shell, cx| {
                                shell.pick(Picker::Locate(target.clone()), cx)
                            });
                        })
                        .into_any_element(),
                    _ => IconButton::new(&theme, format!("remove-{id}"), Icon::X.path())
                        .size(IconButtonSize::Sm)
                        .label("Remove recent")
                        .tooltip("Remove recent")
                        .disabled(!enabled)
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |shell, cx| {
                                shell.dispatch(Command::RemoveRecent(target.clone()), cx)
                            });
                        })
                        .into_any_element(),
                });
            }

            let row = ListRow::new(&theme, format!("recent-{id}"))
                .title(recent.name.clone())
                .meta(shown_path.clone())
                .tooltip(shown_path)
                .action(actions);
            if recent.missing {
                // It cannot open; its actions still locate or remove it.
                row.muted(true)
                    .trailing(Badge::new("Missing").tone(Tone::Warning))
            } else {
                let entity = cx.entity();
                let path = recent.path.clone();
                row.disabled(!enabled).on_click(move |_, cx| {
                    entity.update(cx, |shell, cx| {
                        shell.dispatch(Command::Open(path.clone()), cx)
                    });
                })
            }
        });
        div().id("recents").child(
            List::new()
                .children(rows.collect::<Vec<_>>())
                .empty(EmptyState::new(&theme, "No recent projects")),
        )
    }

    /// The managed SDK's status and its Install, Import and Select actions (Settings → SDK
    /// and the onboarding's SDK step).
    pub(super) fn sdk_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let enabled = !self.busy;
        let sdk_ready = self.presentation.agent_sdk.is_some();
        let status = if self.busy {
            "Operation in progress…".to_owned()
        } else {
            self.presentation.sdk.clone()
        };
        let status_line = div()
            .flex()
            .items_center()
            .gap(px(METRICS.space_2))
            .child(StatusDot::new(sdk_status_tone(self.busy, sdk_ready)))
            .child(
                text_style(div().min_w_0().flex_1(), TextStyle::Small)
                    .text_color(theme.colors.text_2)
                    .child(selectable(status)),
            );
        div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .child(FormField::stacked(&theme, "Status", status_line))
            .child(FormField::row(
                &theme,
                "Managed SDK",
                self.action_button(
                    "install",
                    "Install managed SDK",
                    enabled,
                    false,
                    None,
                    cx,
                    |s, cx| s.dispatch(Command::InstallSdk, cx),
                ),
            ))
            .child(FormField::row(
                &theme,
                "Offline bundle",
                self.action_button(
                    "import-sdk-bundle",
                    "Import offline SDK bundle…",
                    enabled,
                    false,
                    None,
                    cx,
                    |s, cx| s.pick(Picker::SdkBundle, cx),
                ),
            ))
            .when(self.presentation.sdk_repin.is_some(), |controls| {
                controls.child(FormField::row(
                    &theme,
                    "Installed SDK",
                    self.action_button(
                        "use-installed-sdk",
                        "Use installed SDK",
                        enabled,
                        false,
                        None,
                        cx,
                        |s, cx| s.dispatch(Command::UseInstalledSdk, cx),
                    ),
                ))
            })
            .child(FormField::row(
                &theme,
                "This project",
                self.action_button(
                    "sdk",
                    "Select installed SDK…",
                    enabled && self.presentation.project.is_some(),
                    false,
                    None,
                    cx,
                    |s, cx| s.pick(Picker::Sdk, cx),
                ),
            ))
    }

    /// Create, Open and Import Rust (the empty state and the onboarding's Project step).
    pub(super) fn project_actions(&self, cx: &mut Context<Self>) -> gpui::Div {
        let enabled = !self.busy;
        div()
            .flex()
            .flex_wrap()
            .gap(px(METRICS.space_2))
            .child(self.action_button(
                "create",
                "Create…",
                enabled,
                true,
                Some(Icon::Plus),
                cx,
                |s, cx| s.open_create(cx),
            ))
            .child(self.action_button(
                "open",
                "Open…",
                enabled,
                false,
                Some(Icon::FolderOpen),
                cx,
                |s, cx| s.pick(Picker::Open, cx),
            ))
            .child(self.action_button(
                "import",
                "Import Rust…",
                enabled,
                false,
                Some(Icon::Upload),
                cx,
                |s, cx| s.pick(Picker::Import, cx),
            ))
    }

    /// The start screen: the app's name, the project actions and the recent projects.
    pub(super) fn render_empty_state(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let actions = self.project_actions(cx);
        div()
            .id("empty-state")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                Card::raised(&theme)
                    .w_full()
                    .max_w(px(METRICS.layout.start_max_w))
                    .max_h_full()
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_6))
                    .p(px(METRICS.space_6))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(METRICS.space_4))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(METRICS.space_1))
                                    .child(
                                        text_style(div(), TextStyle::Display)
                                            .text_color(theme.colors.text_1)
                                            .child(selectable("fframes Studio")),
                                    )
                                    .child(
                                        text_style(div(), TextStyle::Body)
                                            .text_color(theme.colors.text_2)
                                            .child(selectable(
                                                "A native workspace for your video projects",
                                            )),
                                    ),
                            )
                            .child(actions),
                    )
                    .child(
                        PanelSection::new(&theme, "Recent projects").child(
                            self.render_recents(cx)
                                .max_h(px(METRICS.layout.start_recents_max_h))
                                .overflow_y_scroll(),
                        ),
                    ),
            )
            .into_any_element()
    }

    /// The sidebar's Agent tab: the panel, which pads its own rows (a cached section).
    pub(super) fn render_agent_column(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(self.panel.clone())
            .child(box_probe("agent", cx))
            .into_any_element()
    }
}

/// The sidebar's tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum SidebarTab {
    #[default]
    Agent,
    Project,
    Assets,
    Style,
    Props,
    Slides,
}

/// The actions of a recent-project row, as the id prefix of each button: a missing
/// project can be located, every project can be removed.
fn recent_row_actions(missing: bool) -> Vec<&'static str> {
    if missing {
        vec!["locate", "remove"]
    } else {
        vec!["remove"]
    }
}

/// Dot tone of the managed SDK line: busy, ready or missing.
fn sdk_status_tone(busy: bool, ready: bool) -> Tone {
    if busy {
        Tone::Info
    } else if ready {
        Tone::Success
    } else {
        Tone::Warning
    }
}

/// The sidebar's tab rail with selected chosen; the Agent tab shows its dot when
/// ttention is set. The gallery's shell fixture draws it too.
pub(super) fn tab_rail(
    theme: &studio_theme::Theme,
    selected: SidebarTab,
    attention: bool,
) -> TabRail {
    TabRail::new(
        theme,
        "sidebar-tabs",
        SidebarTab::ALL
            .iter()
            .map(|tab| {
                TabRailItem::new(tab.icon().path(), tab.name(), selected == *tab, false)
                    .id(format!("sidebar-tab-{}", tab.name()))
                    .dot(*tab == SidebarTab::Agent && attention)
            })
            .collect(),
    )
}
impl SidebarTab {
    pub(super) const ALL: [SidebarTab; 6] = [
        SidebarTab::Agent,
        SidebarTab::Project,
        SidebarTab::Assets,
        SidebarTab::Style,
        SidebarTab::Props,
        SidebarTab::Slides,
    ];

    /// Also the tab's element id, `sidebar-tab-{name}`.
    pub(super) fn name(self) -> &'static str {
        match self {
            SidebarTab::Agent => "Agent",
            SidebarTab::Project => "Project",
            SidebarTab::Assets => "Assets",
            SidebarTab::Style => "Style",
            SidebarTab::Props => "Props",
            SidebarTab::Slides => "Slides",
        }
    }

    fn icon(self) -> Icon {
        match self {
            SidebarTab::Agent => Icon::Bot,
            SidebarTab::Project => Icon::Folder,
            SidebarTab::Assets => Icon::Image,
            SidebarTab::Style => Icon::Palette,
            SidebarTab::Props => Icon::Layers,
            SidebarTab::Slides => Icon::Presentation,
        }
    }

    fn from_index(index: usize) -> Self {
        Self::ALL.get(index).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_rows_offer_locate_only_when_missing() {
        assert_eq!(recent_row_actions(true), ["locate", "remove"]);
        assert_eq!(recent_row_actions(false), ["remove"]);
    }

    #[test]
    fn sdk_status_tone_prefers_busy_then_ready() {
        assert_eq!(sdk_status_tone(true, true), Tone::Info);
        assert_eq!(sdk_status_tone(false, true), Tone::Success);
        assert_eq!(sdk_status_tone(false, false), Tone::Warning);
    }

    #[test]
    fn sidebar_tabs_keep_their_element_ids() {
        let names: Vec<_> = SidebarTab::ALL.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            ["Agent", "Project", "Assets", "Style", "Props", "Slides"]
        );
        for (index, tab) in SidebarTab::ALL.into_iter().enumerate() {
            assert_eq!(SidebarTab::from_index(index), tab);
        }
        assert_eq!(SidebarTab::from_index(9), SidebarTab::Agent);
    }
}
