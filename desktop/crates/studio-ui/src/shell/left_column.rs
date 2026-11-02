//! The tabbed sidebar right of the player (Agent, Project, Assets, Style, Props and Slides).
//! The start screen and the recent-projects list are in `home.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, prelude::FluentBuilder, px,
};
use studio_engine::engine::backend::{Command, Recent};
use studio_theme::{METRICS, TextStyle, text_style};

use super::sections::{Section, box_probe, section_probe};
use super::settings::SettingsPage;
use super::{Picker, StudioShell};
use crate::kit::{
    ButtonSize, Card, CollapsibleSection, Emphasis, EmptyState, ExplorerSection, FileKind,
    FileTree, FormField, Icon, IconButton, IconButtonSize, List, ListRow, PanelSection, StatusDot,
    TabRail, TabRailItem, Tone, TreeExpansion, build_tree, selectable, visible_rows,
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
        // Only the left edge is a seam: the header's hairline is above the sidebar and the
        // timeline's below it.
        div()
            .bg(theme.colors.sidebar)
            .border_l(px(METRICS.size.divider))
            .border_color(theme.colors.line)
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
        // The Project and Assets explorers run edge to edge, as Oleafly's do, and pad
        // themselves.
        let padded = !matches!(
            self.sidebar_tab,
            SidebarTab::Project | SidebarTab::Assets | SidebarTab::Agent
        );
        div()
            .id("drawer")
            .relative()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .when(padded, |drawer| drawer.p(px(METRICS.space_4)))
            .child(content)
            .child(section_probe(Section::Drawer, cx))
            .into_any_element()
    }

    /// A style file's row: a 16 px icon for its kind and its project-relative path,
    /// truncated; a click reveals the file. The full path on disk is the tooltip.
    fn file_row(&self, root: &Path, file: &str) -> ListRow {
        let path = root.join(file);
        let shown = studio_bootstrap::external_path(&path).display().to_string();
        let kind = FileKind::of(file);
        ListRow::new(&self.theme, format!("file-{file}"))
            .leading(crate::kit::icon(
                kind.icon(),
                METRICS.size.icon_md,
                kind.color(&self.theme),
            ))
            .title(file.to_owned())
            .tooltip(shown)
            .label(file.to_owned())
            .on_click(move |_, cx| cx.reveal_path(&path))
    }

    /// A project tab's Explorer, after Oleafly's: a folder tree of the project's files (or
    /// assets) under a collapsible header with the count and Collapse all, or the no-project
    /// state while none is open. A file row reveals the file and becomes the selected row;
    /// a folder row opens or closes. The full path on disk is each row's tooltip.
    fn explorer(
        &self,
        tree: ExplorerTree,
        empty: EmptyState,
        without_project: &str,
        cx: &mut Context<Self>,
    ) -> ExplorerSection {
        let theme = self.theme;
        let project = self.presentation.project.as_ref();
        let root = project.map(|p| p.root.clone());
        let state = root
            .as_deref()
            .map(|root| self.explorers.get(root, tree))
            .unwrap_or_default();
        let (id, glyph, title) = match tree {
            ExplorerTree::Files => ("explorer", Icon::Folder, "Explorer"),
            ExplorerTree::Assets => ("assets-explorer", Icon::Image, "Assets"),
        };
        let mut section = ExplorerSection::new(&theme, id, glyph, title, !state.closed);
        let Some((project, root)) = project.zip(root) else {
            return section.child(
                div()
                    .px(px(METRICS.space_4))
                    .py(px(METRICS.space_2))
                    .child(no_project(&theme, without_project)),
            );
        };
        let files = match tree {
            ExplorerTree::Files => &project.files,
            ExplorerTree::Assets => &project.assets,
        };
        section = section.count(match tree {
            ExplorerTree::Files => project.file_count,
            ExplorerTree::Assets => project.assets.len(),
        });

        let entity = cx.entity();
        let at = root.clone();
        section = section.on_toggle(move |open, _, cx| {
            entity.update(cx, |shell, cx| {
                shell
                    .explorers
                    .update(&at, tree, |state| state.closed = !open);
                cx.notify();
            });
        });
        let entity = cx.entity();
        let at = root.clone();
        section = section.action(
            IconButton::new(
                &theme,
                format!("{id}-collapse-all"),
                Icon::ChevronsDownUp.path(),
            )
            .size(IconButtonSize::Sm)
            .emphasis(Emphasis::Ghost)
            .label("Collapse all")
            .tooltip("Collapse all")
            .disabled(files.is_empty())
            .on_click(move |_, _, cx| {
                entity.update(cx, |shell, cx| {
                    shell
                        .explorers
                        .update(&at, tree, |state| state.expansion.collapse_all());
                    cx.notify();
                });
            }),
        );

        let rows = visible_rows(&build_tree(files), &state.expansion);
        let body = if rows.is_empty() {
            div()
                .px(px(METRICS.space_4))
                .py(px(METRICS.space_2))
                .child(empty)
                .into_any_element()
        } else {
            let toggle = {
                let entity = cx.entity();
                let at = root.clone();
                move |row: &crate::kit::TreeRow, _: &mut gpui::Window, cx: &mut gpui::App| {
                    entity.update(cx, |shell, cx| {
                        shell.explorers.update(&at, tree, |state| {
                            state.expansion.set_open(&row.path, !row.open)
                        });
                        cx.notify();
                    });
                }
            };
            let open = {
                let entity = cx.entity();
                let at = root.clone();
                move |row: &crate::kit::TreeRow, _: &mut gpui::Window, cx: &mut gpui::App| {
                    entity.update(cx, |shell, cx| {
                        shell
                            .explorers
                            .update(&at, tree, |state| state.selected = Some(row.path.clone()));
                        cx.notify();
                    });
                    cx.reveal_path(&at.join(&row.path));
                }
            };
            let at = root.clone();
            FileTree::new(&theme, "file", rows)
                .selected(state.selected.clone())
                .tooltip(move |row| {
                    Some(
                        studio_bootstrap::external_path(&at.join(&row.path))
                            .display()
                            .to_string()
                            .into(),
                    )
                })
                .on_toggle(toggle)
                .on_open(open)
                .into_any_element()
        };
        section = section.child(
            div()
                .id("inventory")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        );
        if tree == ExplorerTree::Files && project.file_count > project.files.len() {
            section = section.child(
                text_style(div(), TextStyle::Small)
                    .flex_none()
                    .px(px(METRICS.space_3))
                    .py(px(METRICS.space_2))
                    .text_color(theme.colors.text_3)
                    .child(selectable(format!(
                        "Showing {} of {} files",
                        project.files.len(),
                        project.file_count
                    ))),
            );
        }
        section
    }

    fn render_project_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let project = self.presentation.project.clone();
        let explorer = self.explorer(
            ExplorerTree::Files,
            EmptyState::new(&theme, "No files to show"),
            "browse its files",
            cx,
        );
        // The files and the library start at their content's height and share what is
        // missing, so neither collapses in a short window.
        let mut tab = div().flex_1().min_h_0().flex().flex_col().child(
            div()
                .flex_auto()
                .min_h_0()
                .flex()
                .flex_col()
                .child(explorer),
        );

        if project.is_some() {
            let entity = cx.entity();
            let library = div()
                .id("left-library")
                .min_h_0()
                .max_h(px(METRICS.layout.library_max_h))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .p(px(METRICS.space_4))
                .border_t(px(METRICS.size.divider))
                .border_color(theme.colors.line)
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
        let theme = self.theme;
        let project = self.presentation.project.clone();
        // The section's one action sits at the end of its header, as a small button that
        // always shows.
        let copy = self
            .action_button(
                "asset",
                "Copy asset…",
                !self.busy && project.is_some(),
                false,
                Some(Icon::Plus),
                cx,
                |s, cx| s.pick(Picker::Asset, cx),
            )
            .size(ButtonSize::Sm);
        let explorer = self
            .explorer(
                ExplorerTree::Assets,
                EmptyState::new(&theme, "No assets imported")
                    .body("Copy an image, video or audio file into the project to use it."),
                "copy assets into it",
                cx,
            )
            .trailing(copy);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(explorer)
            .into_any_element()
    }

    fn render_style_tab(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let styles = match &self.presentation.project {
            Some(p) => List::new()
                .children(p.styles.iter().map(|file| self.file_row(&p.root, file)))
                .empty(
                    EmptyState::new(&theme, "No style preset configured")
                        .body("Choose one in Style presets; it styles the video, not Studio."),
                ),
            None => List::new().empty(no_project(&theme, "see its style files")),
        };
        // The presets themselves are chosen in Settings → Presets.
        let presets = self
            .action_button(
                "open-presets",
                "Style presets…",
                true,
                false,
                Some(Icon::Palette),
                cx,
                |s, cx| s.open_settings_page(SettingsPage::Presets, cx),
            )
            .size(ButtonSize::Sm);
        div()
            .id("style-tab")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .child(
                PanelSection::new(&theme, "Style files")
                    .trailing(presets)
                    .child(styles),
            )
            .into_any_element()
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
            .when_some(
                self.presentation.sdk_upgrade.as_ref(),
                |controls, upgrade| {
                    // After a passing check the upgrade is the suggested action.
                    let checked = upgrade.check == Some(Ok(()));
                    controls.child(FormField::row(
                        &theme,
                        "Project SDK",
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(METRICS.space_2))
                            .child(self.action_button(
                                "check-sdk-upgrade",
                                "Check project",
                                enabled,
                                false,
                                None,
                                cx,
                                |s, cx| s.dispatch(Command::CheckSdkUpgrade, cx),
                            ))
                            .child(self.action_button(
                                "upgrade-project-sdk",
                                "Upgrade project",
                                enabled,
                                checked,
                                None,
                                cx,
                                |s, cx| s.dispatch(Command::UpgradeProjectSdk, cx),
                            )),
                    ))
                },
            )
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
    /// Create… is the view's primary action when `create_primary`; the onboarding's Project
    /// step has its own (Continue), so Create… is neutral there.
    pub(super) fn project_actions(
        &self,
        create_primary: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let enabled = !self.busy;
        div()
            .flex()
            .flex_wrap()
            .gap(px(METRICS.space_2))
            .child(self.action_button(
                "create",
                "Create…",
                enabled,
                create_primary,
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

    /// The Project, Assets and Style tabs at the sidebar's narrowest and widest, for the UI
    /// gallery's `shell` page: with fixture files, assets and recent projects (long names
    /// and a missing project among them) when `render_shell_fixture` opened a project, or
    /// their no-project state otherwise. The recents and their fold are put back afterwards.
    pub fn render_drawer_fixture(
        &mut self,
        theme: studio_theme::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if let Some(project) = &mut self.presentation.project {
            project.files = [
                "Cargo.toml",
                "README.md",
                "src/lib.rs",
                "src/main.rs",
                "src/scenes/intro.rs",
                "src/scenes/a-very-long-scene-name-for-the-closing-credits-sequence.rs",
                "src/shaders/glow.wgsl",
                "media/logo.svg",
                "media/voice-over.mp3",
                "style/preset.json",
            ]
            .map(String::from)
            .to_vec();
            project.assets = [
                "media/logo.svg",
                "media/background-photo-from-the-product-launch-event-2026.jpg",
                "media/voice-over.mp3",
            ]
            .map(String::from)
            .to_vec();
            project.styles = vec!["style/preset.json".into()];
            project.file_count = 214;
        }
        let recent = |id: &str, name: &str, path: &str, missing: bool| Recent {
            id: studio_project::ProjectId::try_from(id.to_owned()).expect("a valid id"),
            name: name.into(),
            path: path.into(),
            missing,
            preview: None,
        };
        let recents = std::mem::replace(
            &mut self.presentation.recents,
            vec![
                recent(
                    "launch-video",
                    "Launch video",
                    "C:/Videos/launch-video",
                    false,
                ),
                recent(
                    "quarterly-review",
                    "Quarterly review for the product and engineering all-hands",
                    "C:/Users/Trần Minh/Documents/fframes/quarterly-review-for-the-all-hands",
                    false,
                ),
                recent("old-demo", "Old demo", "D:/Archive/old-demo", true),
            ],
        );
        let open = std::mem::replace(&mut self.recents_open, true);
        // The Project tab with a second-level folder open and a file selected; the Assets
        // tab as it opens.
        let mut explorers = Explorers::default();
        if let Some(project) = &self.presentation.project {
            explorers.update(&project.root, ExplorerTree::Files, |state| {
                state.expansion.set_open("src/scenes", true);
                state.selected = Some("src/scenes/intro.rs".into());
            });
        }
        let explorers = std::mem::replace(&mut self.explorers, explorers);
        let main = std::mem::replace(&mut self.theme, theme);
        let tabs = [
            ("Project", self.render_project_tab(cx)),
            ("Assets", self.render_assets_tab(cx)),
            ("Style", self.render_style_tab(cx)),
        ];
        // Each tab is drawn twice; the second copy is the same tree rendered again.
        let tabs_again = [
            self.render_project_tab(cx),
            self.render_assets_tab(cx),
            self.render_style_tab(cx),
        ];
        self.theme = main;
        self.explorers = explorers;
        self.recents_open = open;
        self.presentation.recents = recents;

        // As in `render_drawer`, the explorers pad themselves and the Style tab is padded.
        let drawer = |width: f32, padded: bool, body: gpui::AnyElement| {
            Card::panel(&theme)
                .bg(theme.colors.sidebar)
                .w(px(width))
                .h(px(DRAWER_FIXTURE_H))
                .flex_none()
                .flex()
                .flex_col()
                .when(padded, |drawer| drawer.p(px(METRICS.space_4)))
                .child(body)
        };
        let caption = |text: String| {
            text_style(div(), TextStyle::Small)
                .text_color(theme.colors.text_3)
                .child(text)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_4))
            .p(px(METRICS.space_4))
            .bg(theme.colors.window)
            .children(
                tabs.into_iter()
                    .zip(tabs_again)
                    .map(|((name, narrow), wide)| {
                        let padded = name == "Style";
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(METRICS.space_2))
                            .child(caption(format!(
                                "{name} tab at {} and {} px",
                                METRICS.layout.sidebar_min_w, METRICS.layout.sidebar_max_w
                            )))
                            .child(
                                div()
                                    .flex()
                                    .items_start()
                                    .gap(px(METRICS.space_4))
                                    .child(drawer(METRICS.layout.sidebar_min_w, padded, narrow))
                                    .child(drawer(METRICS.layout.sidebar_max_w, padded, wide)),
                            )
                    }),
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

/// The height of a drawer in [`StudioShell::render_drawer_fixture`]: short enough that the
/// Project tab's files and recents share it, as in a small window.
const DRAWER_FIXTURE_H: f32 = 560.;

/// What a project tab shows while no project is open: the same title in every tab.
fn no_project(theme: &studio_theme::Theme, to: &str) -> EmptyState {
    EmptyState::new(theme, "No project open").body(format!("Create or open a project to {to}."))
}

/// Which list of a project an explorer shows: the Project tab's files or the Assets tab's
/// assets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExplorerTree {
    Files,
    Assets,
}

/// One explorer's state: whether its section is folded, its open folders and its
/// selected file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ExplorerState {
    closed: bool,
    expansion: TreeExpansion,
    selected: Option<String>,
}

/// The explorers' state per project root, kept while Studio runs: reopening a project
/// shows its folders as they were left.
#[derive(Debug, Default)]
pub(super) struct Explorers {
    projects: HashMap<PathBuf, [ExplorerState; 2]>,
}

impl Explorers {
    fn get(&self, root: &Path, tree: ExplorerTree) -> ExplorerState {
        self.projects
            .get(root)
            .map(|states| states[tree as usize].clone())
            .unwrap_or_default()
    }

    fn update(&mut self, root: &Path, tree: ExplorerTree, change: impl FnOnce(&mut ExplorerState)) {
        let states = self.projects.entry(root.to_path_buf()).or_default();
        change(&mut states[tree as usize]);
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
    fn explorer_state_is_kept_per_project_and_per_tree() {
        let mut explorers = Explorers::default();
        let (a, b) = (Path::new("C:/videos/a"), Path::new("C:/videos/b"));
        explorers.update(a, ExplorerTree::Files, |state| {
            state.expansion.set_open("src", false);
            state.selected = Some("src/lib.rs".into());
        });
        explorers.update(b, ExplorerTree::Assets, |state| state.closed = true);

        let files = explorers.get(a, ExplorerTree::Files);
        assert!(!files.expansion.is_open("src", 0));
        assert_eq!(files.selected.as_deref(), Some("src/lib.rs"));
        assert_eq!(
            explorers.get(a, ExplorerTree::Assets),
            ExplorerState::default()
        );
        assert!(explorers.get(b, ExplorerTree::Assets).closed);
        assert!(
            explorers
                .get(b, ExplorerTree::Files)
                .expansion
                .is_open("src", 0)
        );
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
