//! The settings frame, shown as Oleafly's dialog over the main window
//! ([`SettingsDialog`](super::settings_dialog::SettingsDialog)). The header's Settings button
//! and the home dock's open it.
//!
//! The pages, after Oleafly's settings: General (the audio output and setup again),
//! Appearance (theme and motion), Agent (CLI agents, MCP servers, Tools, Permissions and
//! Review, on a pill tab bar), SDK (the managed SDK), Presets (the project's style presets),
//! Export (the defaults every export starts from), Keyboard Shortcuts (the keymap's
//! bindings, read from [`SHORTCUTS`](super::keymap::SHORTCUTS)) and Help & About (the
//! version, updates and links). The nav column's search field keeps the pages whose name or
//! settings match; its footer links to the documentation, the website, the issue tracker
//! and the release notes.

use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use fframes_studio_protocol::export::ExportSettings;
use gpui::{App, AppContext, Context, Entity, IntoElement, ParentElement, Styled, Window, div, px};
use studio_engine::export_options::{FORMATS, REDUCED_FPS, SCALES, format_label};
use studio_engine::services::audio_service::OutputDevice;
use studio_engine::updates::RELEASES_REPOSITORY;
use studio_engine::{ReviewPolicy, app_settings::AppSettings, updates::AvailableUpdate};
use studio_proto::{AppTheme, MotionPreference};
use studio_theme::{Appearance, METRICS, TextStyle, Theme, text_style};

use super::StudioShell;
use super::keymap::{SHORTCUTS, ShortcutScope};
use super::sections::box_probe;
use crate::kit::{
    Button, ButtonSize, Emphasis, Icon, KbdChip, List, ListRow, MODAL_RADIUS, PanelSection,
    PictureCard, PictureCards, RadioCard, RadioCards, SegmentOption, SegmentedControl, SettingRow,
    SettingsFooterLink, SettingsFrame, SettingsNavItem, TextInput, TextInputState, Toggle,
    appearance_picture, keystroke_caps, selectable, setting_card, setting_note,
};

/// The check for a newer Studio release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UpdateCheck {
    NotChecked,
    Checking,
    UpToDate,
    Available(AvailableUpdate),
    Failed(String),
}

/// The choices in the order the control shows them; option `i` is `theme-i`.
const THEMES: [(AppTheme, &str); 3] = [
    (AppTheme::System, "System"),
    (AppTheme::Light, "Light"),
    (AppTheme::Dark, "Dark"),
];

/// The motion choices in the order the control shows them; option `i` is `motion-i`.
const MOTIONS: [(MotionPreference, &str); 3] = [
    (MotionPreference::System, "System"),
    (MotionPreference::Reduced, "Reduced"),
    (MotionPreference::Full, "Full"),
];

/// A page of the settings frame, in the order of its nav column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum SettingsPage {
    #[default]
    General,
    Appearance,
    Agent,
    Sdk,
    Presets,
    Export,
    Shortcuts,
    About,
}

impl SettingsPage {
    const ALL: [SettingsPage; 8] = [
        SettingsPage::General,
        SettingsPage::Appearance,
        SettingsPage::Agent,
        SettingsPage::Sdk,
        SettingsPage::Presets,
        SettingsPage::Export,
        SettingsPage::Shortcuts,
        SettingsPage::About,
    ];

    /// The page's name, in its nav row and over the page.
    pub(super) fn label(self) -> &'static str {
        match self {
            SettingsPage::General => "General",
            SettingsPage::Appearance => "Appearance",
            SettingsPage::Agent => "Agent",
            SettingsPage::Sdk => "SDK",
            SettingsPage::Presets => "Presets",
            SettingsPage::Export => "Export",
            SettingsPage::Shortcuts => "Keyboard Shortcuts",
            SettingsPage::About => "Help & About",
        }
    }

    fn nav_item(self, selected: bool) -> SettingsNavItem {
        let (id, icon) = match self {
            SettingsPage::General => ("settings-general", Icon::Settings),
            SettingsPage::Appearance => ("settings-appearance", Icon::Palette),
            SettingsPage::Agent => ("settings-agent", Icon::Bot),
            SettingsPage::Sdk => ("settings-sdk", Icon::Package),
            SettingsPage::Presets => ("settings-presets", Icon::SlidersHorizontal),
            SettingsPage::Export => ("settings-export", Icon::Download),
            SettingsPage::Shortcuts => ("settings-shortcuts", Icon::Keyboard),
            SettingsPage::About => ("settings-about", Icon::LifeBuoy),
        };
        SettingsNavItem::new(id, self.label(), icon, selected)
    }

    /// The titles of the page's settings and sections, which the search looks through
    /// besides the page's name. Keyboard Shortcuts adds every shortcut's name.
    fn titles(self) -> &'static [&'static str] {
        match self {
            SettingsPage::General => &[
                "Audio output",
                "Output device",
                "Next output",
                "Retry audio",
                "No audio",
                "Setup",
                "First-run setup",
                "Run setup again",
            ],
            SettingsPage::Appearance => &[
                "Theme",
                "System",
                "Light",
                "Dark",
                "Motion",
                "Reduced",
                "Animations",
            ],
            SettingsPage::Agent => &[
                "CLI agents",
                "Claude Code",
                "Codex",
                "Sign in",
                "Install adapter",
                "MCP servers",
                "Add server",
                "Import",
                "Environment variables",
                "Tools",
                "Project tools",
                "Render frame",
                "Review",
                "Apply automatically",
                "Review before Apply",
                "Permissions",
                "Adapter",
                "Protocol",
                "Executable",
                "Arguments",
                "Sign-in variable names",
                "Project tools",
                "MCP",
                "Status",
            ],
            SettingsPage::Sdk => &["Managed SDK", "Install SDK", "Rust", "FFmpeg"],
            SettingsPage::Presets => &["Style presets", "Colors", "Fonts"],
            SettingsPage::Export => &["Export defaults", "Format", "Size", "Frame rate"],
            SettingsPage::Shortcuts => &["Keys", "Hotkeys", "Player", "Timeline", "Present"],
            SettingsPage::About => &[
                "Version",
                "Updates",
                "Check for updates at launch",
                "Latest version",
                "Check now",
                "Resources",
                "Documentation",
                "Website",
                "Report a problem",
                "What's new",
                "Release notes",
                "Source code",
            ],
        }
    }

    /// Whether the search `query` finds the page: its name or one of its titles holds every
    /// word of the query, ignoring case. An empty query finds every page.
    fn matches(self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let shortcuts = SHORTCUTS
            .iter()
            .filter(|_| self == SettingsPage::Shortcuts)
            .map(|shortcut| shortcut.label);
        std::iter::once(self.label())
            .chain(self.titles().iter().copied())
            .chain(shortcuts)
            .any(|title| {
                let title = title.to_lowercase();
                query.split_whitespace().all(|word| title.contains(word))
            })
    }
}

/// The sub-pages of the Agent page, on its pill tab bar; tab `i` is `agent-tab-{i}`. In
/// Oleafly's order: the agents first, then what they may use, then how their work lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentTab {
    #[default]
    Agents,
    Mcp,
    Tools,
    Permissions,
    Review,
}

impl AgentTab {
    const ALL: [AgentTab; 5] = [
        AgentTab::Agents,
        AgentTab::Mcp,
        AgentTab::Tools,
        AgentTab::Permissions,
        AgentTab::Review,
    ];

    fn label(self) -> &'static str {
        match self {
            AgentTab::Agents => "CLI agents",
            AgentTab::Mcp => "MCP servers",
            AgentTab::Tools => "Tools",
            AgentTab::Permissions => "Permissions",
            AgentTab::Review => "Review",
        }
    }
}

/// Receives the Agent tab the user picks.
pub(super) type AgentTabHandler = Rc<dyn Fn(AgentTab, &mut App)>;

/// What the settings window holds around the shell's pages: the strip above the nav rows,
/// the search field and the Agent page's tab.
#[derive(Default)]
pub(super) struct SettingsChrome {
    pub(super) nav_header: Option<gpui::AnyElement>,
    /// The dialog's close button, at the end of the page header.
    pub(super) header_end: Option<gpui::AnyElement>,
    pub(super) search: Option<Entity<TextInputState>>,
    pub(super) agent_tab: AgentTab,
    pub(super) on_agent_tab: Option<AgentTabHandler>,
}

/// A page of the project's site the settings link to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    Docs,
    Website,
    Issues,
    Releases,
    Source,
}

impl Link {
    /// The links in the nav column's footer.
    const FOOTER: [Link; 4] = [Link::Docs, Link::Website, Link::Issues, Link::Releases];
    /// The links on Help & About.
    const RESOURCES: [Link; 5] = [
        Link::Docs,
        Link::Website,
        Link::Releases,
        Link::Issues,
        Link::Source,
    ];

    fn url(self) -> String {
        let repository = format!("https://github.com/{RELEASES_REPOSITORY}");
        match self {
            Link::Docs => format!("{repository}/blob/main/docs/desktop/README.md"),
            Link::Website => "https://fframes.studio".to_owned(),
            Link::Issues => format!("{repository}/issues/new"),
            Link::Releases => format!("{repository}/releases"),
            Link::Source => repository,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Link::Docs => "docs",
            Link::Website => "website",
            Link::Issues => "issues",
            Link::Releases => "releases",
            Link::Source => "source",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Link::Docs => "Documentation",
            Link::Website => "Website",
            Link::Issues => "Report a problem",
            Link::Releases => "What's new",
            Link::Source => "Source code",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Link::Docs => "How Studio works, from setup to export.",
            Link::Website => "fframes, the video framework Studio is built on.",
            Link::Issues => "Open an issue on GitHub with what happened.",
            Link::Releases => "The release notes of every version.",
            Link::Source => "The project on GitHub.",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Link::Docs => Icon::BookOpen,
            Link::Website => Icon::Globe,
            Link::Issues => Icon::LifeBuoy,
            Link::Releases => Icon::ScrollText,
            Link::Source => Icon::FileCode,
        }
    }
}

/// The frame rates Settings → Export offers as the default: the video's own, or 30 where
/// it divides the video's.
const DEFAULT_FPS: [(Option<u32>, &str); 2] =
    [(None, "The video's own"), (Some(REDUCED_FPS), "30 fps")];

/// Saves the theme choices one at a time, in the order they were made: a save that a
/// later choice overtook is skipped.
#[derive(Default)]
pub(super) struct SettingsWriter {
    latest: AtomicU64,
    latest_motion: AtomicU64,
    latest_export: AtomicU64,
    latest_setup: AtomicU64,
    latest_update_check: AtomicU64,
    latest_sdk_auto_update: AtomicU64,
    latest_agent_tools: AtomicU64,
    write: parking_lot::Mutex<()>,
}

impl SettingsWriter {
    pub(super) fn next_export_generation(&self) -> u64 {
        self.latest_export.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Runs `save` unless a later export choice was made meanwhile. Theme and export
    /// saves share the lock, so neither overwrites the other's field.
    pub(super) fn save_if_latest_export(
        &self,
        generation: u64,
        save: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let _write = self.write.lock();
        if self.latest_export.load(Ordering::Acquire) != generation {
            return Ok(());
        }
        save()
    }
}

impl StudioShell {
    /// Opens the settings window, or brings it to the front when it is open. Not while the
    /// setup shows: it draws the Agent page's panel too.
    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        if self.onboarding.is_some() {
            return;
        }
        self.overflow_open = false;
        // The Export page shows them; a render cannot read the file.
        self.export_defaults();
        // The main window's next render builds the dialog: its search field needs the window.
        self.settings_open = true;
        cx.notify();
    }

    /// Opens the settings window on `page`.
    pub(super) fn open_settings_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.settings_page = page;
        self.open_settings(cx);
    }

    /// Opens the settings window on the Agent page at `tab`, or turns an open one there: the
    /// deep link of the agent panel's settings buttons and links. `target` names what to
    /// show on that tab: an agent's profile id opens its card on CLI agents, a server's name
    /// highlights its card on MCP servers, and either card is scrolled into view. Other
    /// targets open the tab only.
    pub(super) fn open_agent_settings(
        &mut self,
        tab: AgentTab,
        target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_page(SettingsPage::Agent, cx);
        if !self.settings_open {
            return;
        }
        let dialog = match &self.settings_dialog {
            Some(dialog) => dialog.clone(),
            None => {
                let dialog = super::settings_dialog::SettingsDialog::open(
                    cx.entity(),
                    &self.panel,
                    tab,
                    window,
                    cx,
                );
                self.settings_dialog = Some(dialog.clone());
                dialog
            }
        };
        dialog.update(cx, |dialog, cx| dialog.show_agent_tab(tab, cx));
        let scroll = self.settings_scroll.clone();
        match (tab, target) {
            (AgentTab::Agents, Some(id)) => self
                .panel
                .update(cx, |panel, cx| panel.expand_agent(id, scroll, cx)),
            (AgentTab::Mcp, Some(name)) => self
                .agent_tools
                .update(cx, |page, cx| page.show_server(&name, cx)),
            _ => {}
        }
    }

    pub(super) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings_open = false;
        // The next render hands the focus back (it needs the window).
        if let Some(dialog) = self.settings_dialog.take() {
            self.settings_return_focus = dialog.read(cx).return_focus.clone();
        }
        cx.notify();
    }

    /// Applies `choice` to every window and saves it in the background.
    pub(super) fn choose_theme(&mut self, choice: AppTheme, cx: &mut Context<Self>) {
        if studio_theme::current_preference(cx) == choice {
            return;
        }
        // Refreshes every window, which renders the cached sections again too.
        studio_theme::set_preference(choice, cx);
        let Some(paths) = self.presentation.paths.clone() else {
            self.error = Some("Cannot save the theme: the app's storage is unavailable".into());
            return;
        };
        let writer = self.settings_writer.clone();
        let generation = writer.latest.fetch_add(1, Ordering::AcqRel) + 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { save_theme(&writer, generation, choice, &paths) })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |shell, cx| {
                    shell.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Applies the motion `choice` to every window and saves it in the background, like
    /// the theme.
    fn choose_motion(&mut self, choice: MotionPreference, cx: &mut Context<Self>) {
        if studio_theme::current_motion(cx) == choice {
            return;
        }
        studio_theme::set_motion(choice, cx);
        let Some(paths) = self.presentation.paths.clone() else {
            self.error =
                Some("Cannot save the motion setting: the app's storage is unavailable".into());
            return;
        };
        let writer = self.settings_writer.clone();
        let generation = writer.latest_motion.fetch_add(1, Ordering::AcqRel) + 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { save_motion(&writer, generation, choice, &paths) })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |shell, cx| {
                    shell.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Saves the setup record and the permission policy in the background. Both fields are
    /// written from the shell's current values, so the latest save carries every choice and
    /// an overtaken one is skipped.
    pub(super) fn save_setup_choices(&mut self, cx: &mut Context<Self>) {
        let Some(paths) = self.presentation.paths.clone() else {
            self.error = Some("Cannot save the setup: the app's storage is unavailable".into());
            return;
        };
        let (completed, policy) = (self.onboarding_completed, self.permission_policy);
        let writer = self.settings_writer.clone();
        let generation = writer.latest_setup.fetch_add(1, Ordering::AcqRel) + 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _write = writer.write.lock();
                    if writer.latest_setup.load(Ordering::Acquire) != generation {
                        return Ok(());
                    }
                    let mut settings = AppSettings::load(&paths);
                    settings.onboarding_completed = completed;
                    settings.permission_policy = policy;
                    settings.save(&paths)
                })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |shell, cx| {
                    shell.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Settings → Agent → MCP servers or Tools changed: hands the effective settings (with
    /// the open project's override) to the project's agent (for its next task) and saves the
    /// settings for all projects, and the open project's override, in the background.
    pub(super) fn apply_agent_tools(
        &mut self,
        settings: studio_engine::agent_settings::AgentToolSettings,
        cx: &mut Context<Self>,
    ) {
        self.hand_agent_tools(cx);
        let Some(paths) = self.presentation.paths.clone() else {
            self.error =
                Some("Cannot save the agent settings: the app's storage is unavailable".into());
            return;
        };
        let project = self.agent_tools.read(cx).project_override();
        let writer = self.settings_writer.clone();
        let generation = writer.latest_agent_tools.fetch_add(1, Ordering::AcqRel) + 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _write = writer.write.lock();
                    if writer.latest_agent_tools.load(Ordering::Acquire) != generation {
                        return Ok(());
                    }
                    if let Some((id, overrides)) = &project {
                        overrides.save(&paths, id)?;
                    }
                    let mut saved = AppSettings::load(&paths);
                    saved.agent = settings;
                    saved.save(&paths)
                })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |shell, cx| {
                    shell.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Hands the current MCP servers and tool choices, with the open project's override, to
    /// the project's agent.
    pub(super) fn hand_agent_tools(&self, cx: &Context<Self>) {
        if let Some(workflow) = &self.agent.workflow {
            let _ = workflow.set_agent_tools(self.agent_tools.read(cx).effective());
        }
    }

    /// Points the MCP and Tools pages at the open project, reading its override when the
    /// project changed.
    pub(super) fn follow_agent_tools_project(&self, cx: &mut Context<Self>) {
        let project = self
            .presentation
            .project
            .as_ref()
            .map(|project| (project.id.clone(), project.name.clone()));
        let paths = self.presentation.paths.clone();
        self.agent_tools
            .update(cx, |page, _| page.follow_project(project, paths.as_ref()));
    }

    /// Asks GitHub for a newer Studio in the background; the answer shows on the
    /// General page and, when there is one, as a notice under the header.
    pub fn check_for_update(&mut self, cx: &mut Context<Self>) {
        if self.update == UpdateCheck::Checking {
            return;
        }
        self.update = UpdateCheck::Checking;
        self.update_dismissed = false;
        cx.notify();
        let version = self.app_version;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { studio_engine::updates::check_for_update(version) })
                .await;
            let _ = this.update(cx, |shell, cx| {
                shell.update = match result {
                    Ok(Some(update)) => UpdateCheck::Available(update),
                    Ok(None) => UpdateCheck::UpToDate,
                    Err(error) => UpdateCheck::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Turns the launch check on or off and saves it in the background.
    fn set_update_check(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.update_check_disabled = !enabled;
        self.save_update_setting(
            |writer| &writer.latest_update_check,
            move |settings| settings.update_check_disabled = !enabled,
            cx,
        );
    }

    /// Turns installing the SDK an updated app ships by itself on or off, and saves it.
    fn set_sdk_auto_update(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.sdk_auto_update_disabled = !enabled;
        self.save_update_setting(
            |writer| &writer.latest_sdk_auto_update,
            move |settings| settings.sdk_auto_update_disabled = !enabled,
            cx,
        );
    }

    /// Saves one Updates setting in the background with `apply`; a save that a later
    /// choice of the same setting (counted by `latest`) overtook is skipped.
    fn save_update_setting(
        &mut self,
        latest: fn(&SettingsWriter) -> &AtomicU64,
        apply: impl FnOnce(&mut AppSettings) + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
        let Some(paths) = self.presentation.paths.clone() else {
            self.error = Some("Cannot save the setting: the app's storage is unavailable".into());
            return;
        };
        let writer = self.settings_writer.clone();
        let generation = latest(&writer).fetch_add(1, Ordering::AcqRel) + 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let _write = writer.write.lock();
                    if latest(&writer).load(Ordering::Acquire) != generation {
                        return Ok(());
                    }
                    let mut settings = AppSettings::load(&paths);
                    apply(&mut settings);
                    settings.save(&paths)
                })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |shell, cx| {
                    shell.error = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Help & About: the app and its version with the update check, the launch check, and
    /// the project's links.
    fn render_about(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let colors = theme.colors;
        let entity = cx.entity();
        let toggle_entity = entity.clone();
        let toggle = Toggle::new(&theme, "update-check", !self.update_check_disabled)
            .label("Check for updates at launch")
            .on_change(move |enabled, _, cx| {
                toggle_entity.update(cx, |shell, cx| shell.set_update_check(enabled, cx))
            });
        let sdk_toggle = Toggle::new(&theme, "sdk-auto-update", !self.sdk_auto_update_disabled)
            .label("Install SDK updates automatically")
            .on_change(move |enabled, _, cx| {
                entity.update(cx, |shell, cx| shell.set_sdk_auto_update(enabled, cx))
            });
        let status = match &self.update {
            UpdateCheck::NotChecked => "Not checked yet".to_owned(),
            UpdateCheck::Checking => "Checking…".to_owned(),
            UpdateCheck::UpToDate => format!("fframes Studio {} is the latest", self.app_version),
            UpdateCheck::Available(update) => format!(
                "fframes Studio {} is available (this is {})",
                update.version, self.app_version
            ),
            UpdateCheck::Failed(error) => error.clone(),
        };
        let mut actions =
            div()
                .flex()
                .flex_wrap()
                .gap(px(METRICS.space_2))
                .child(self.action_button(
                    "check-update",
                    "Check now",
                    self.update != UpdateCheck::Checking,
                    false,
                    Some(Icon::RefreshCw),
                    cx,
                    |shell, cx| shell.check_for_update(cx),
                ));
        if let UpdateCheck::Available(update) = &self.update {
            actions = actions.child(self.download_update_button(update.url.clone(), cx));
        }
        // Oleafly's About card: the name and version, a line on what the app is, and the
        // update actions under them.
        let app = setting_card(&theme)
            .flex()
            .flex_col()
            .gap(px(METRICS.space_3))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_1))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(METRICS.space_2))
                            .child(
                                text_style(div(), TextStyle::Title)
                                    .text_color(colors.text_1)
                                    .child("fframes Studio"),
                            )
                            .child(
                                text_style(div(), TextStyle::Small)
                                    .text_color(colors.text_2)
                                    .child(selectable(format!("v{}", self.app_version))),
                            ),
                    )
                    .child(
                        text_style(div(), TextStyle::Small)
                            .text_color(colors.text_2)
                            .child(selectable(
                                "Describe a video, watch it, select a scene or an object and \
                                 ask a coding agent to change it. The video stays an ordinary \
                                 fframes Rust project.",
                            )),
                    ),
            )
            .child(actions);
        let links = Link::RESOURCES.map(|link| {
            let url = link.url();
            SettingRow::new(&theme, link.label())
                .description(link.description())
                .icon(link.icon())
                .link(format!("about-{}", link.key()), move |_, cx| {
                    cx.open_url(&url)
                })
                .into_any_element()
        });
        settings_page()
            .child(app)
            .child(
                PanelSection::new(&theme, "Updates").children([
                    SettingRow::new(&theme, "Check for updates at launch")
                        .description(
                            "Studio asks GitHub for a newer release. Nothing is downloaded or \
                             installed without you.",
                        )
                        .control(toggle)
                        .into_any_element(),
                    SettingRow::new(&theme, "Install SDK updates automatically")
                        .description(
                            "When an updated Studio brings a newer managed SDK, Studio installs \
                             it at launch. The SDK it replaces stays installed, so projects \
                             pinned to it keep building until you upgrade them.",
                        )
                        .control(sdk_toggle)
                        .into_any_element(),
                    SettingRow::new(&theme, "Latest version")
                        .description(status)
                        .into_any_element(),
                ]),
            )
            .child(PanelSection::new(&theme, "Resources").children(links))
            .into_any_element()
    }

    /// Opens the release page of `url`, where the installer and the notes are.
    pub(super) fn download_update_button(
        &self,
        url: String,
        cx: &mut Context<Self>,
    ) -> crate::kit::Button {
        self.action_button(
            "download-update",
            "Download…",
            true,
            true,
            None,
            cx,
            move |_, cx| cx.open_url(&url),
        )
    }

    /// The settings window's frame: the nav column with the search field, the pages the
    /// search keeps and the footer links, beside the selected page.
    pub(super) fn render_settings(
        &self,
        chrome: SettingsChrome,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let entity = cx.entity();
        let query = chrome
            .search
            .as_ref()
            .map(|search| search.read(cx).text().to_owned())
            .unwrap_or_default();
        let visible: Rc<Vec<SettingsPage>> = Rc::new(
            SettingsPage::ALL
                .into_iter()
                .filter(|page| page.matches(&query))
                .collect(),
        );
        let page = match self.settings_page {
            SettingsPage::General => settings_page()
                .child(self.render_audio_output(cx))
                .child(
                    PanelSection::new(&theme, "Setup").children([SettingRow::new(
                        &theme,
                        "First-run setup",
                    )
                    .description(
                        "Walk through the theme, SDK, first video, agent and permissions \
                         again.",
                    )
                    .control(self.action_button(
                        "run-setup-again",
                        "Run setup again",
                        true,
                        false,
                        None,
                        cx,
                        |shell, cx| shell.run_setup_again(cx),
                    ))
                    .into_any_element()]),
                )
                .into_any_element(),
            SettingsPage::Appearance => self.render_appearance(cx),
            SettingsPage::Agent => self.render_agent(chrome.agent_tab, chrome.on_agent_tab, cx),
            SettingsPage::Sdk => PanelSection::new(&theme, "Managed SDK")
                .description(
                    "Studio builds previews and exports with this SDK. Selecting an installed \
                     one applies to the open project.",
                )
                .children([setting_card(&theme)
                    .child(self.sdk_controls(cx))
                    .into_any_element()])
                .into_any_element(),
            SettingsPage::Presets => self.preset_panel.clone().into_any_element(),
            SettingsPage::Export => self.render_export_defaults(cx),
            SettingsPage::Shortcuts => render_shortcuts(&theme),
            SettingsPage::About => self.render_about(cx),
        };
        let footer = Link::FOOTER
            .into_iter()
            .map(|link| {
                let url = link.url();
                SettingsFooterLink::new(
                    format!("settings-footer-{}", link.key()),
                    link.label(),
                    link.icon(),
                    move |_, cx| cx.open_url(&url),
                )
            })
            .collect();
        let mut frame = SettingsFrame::new(
            &theme,
            "settings",
            visible
                .iter()
                .map(|page| page.nav_item(*page == self.settings_page))
                .collect(),
        )
        .on_select({
            let entity = entity.clone();
            let visible = visible.clone();
            move |index, _, cx| {
                if let Some(page) = visible.get(*index) {
                    entity.update(cx, |shell, cx| {
                        shell.settings_page = *page;
                        cx.notify();
                    });
                }
            }
        })
        .heading("Settings")
        .title(self.settings_page.label())
        .header_end(chrome.header_end)
        .radius(MODAL_RADIUS)
        .empty("No settings match")
        .footer(footer)
        .nav_header(chrome.nav_header)
        .scroll(&self.settings_scroll)
        .children([page]);
        if let Some(search) = &chrome.search {
            frame = frame.search(
                TextInput::new(&theme, "settings-search", search)
                    .search()
                    .height(px(METRICS.size.control_h)),
            );
        }
        let frame = if self.qualifying {
            frame.on_item_bounds(move |index, bounds, cx| {
                if let Some(page) = visible.get(index) {
                    entity.update(cx, |shell, _| {
                        shell.button_bounds.insert(
                            page.nav_item(false).id.to_string(),
                            crate::probe::bounds_to_record(bounds),
                        );
                    });
                }
            })
        } else {
            frame
        };
        div()
            .relative()
            .size_full()
            .child(frame)
            .child(box_probe("settings", cx))
            .into_any_element()
    }

    /// Follows a new search `query`: when it no longer finds the open page, opens the first
    /// page it finds, as Oleafly opens its best match.
    pub(super) fn follow_settings_search(&mut self, query: &str, cx: &mut Context<Self>) {
        if self.settings_page.matches(query) {
            return;
        }
        if let Some(page) = SettingsPage::ALL
            .into_iter()
            .find(|page| page.matches(query))
        {
            self.settings_page = page;
            cx.notify();
        }
    }

    /// The settings frame in the gallery's fixture `state` (0 General … 7 Help & About, in
    /// nav order; 8 the Agent page's CLI agents tab; 9 a search for "frame"; 10 and 11 its
    /// MCP servers and Tools tabs with sample servers; 12, 13 and 14 its Permissions tab
    /// with saved rules, with none, and with no project open; 17 the MCP import panel, 18
    /// the MCP registry browser, 19 and 20 the MCP servers and Tools tabs in This project;
    /// 21 and 22 Find more agents loading and failed, 23 and 24 the MCP registry browser
    /// searching and failed, 25 the MCP servers after a link to one, 26 Permissions while
    /// an open project's agent attaches, 27 CLI agents after a link to Antigravity, low on the
    /// page), drawn with
    /// `theme` the way the settings window draws it. For the UI gallery's captures.
    pub fn render_settings_fixture(
        &mut self,
        state: usize,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let search = cx.new(|cx| {
            let mut input = TextInputState::single_line(window, cx);
            input.set_placeholder(SEARCH_PLACEHOLDER);
            input
        });
        let mut agent_tab = AgentTab::Review;
        self.approvals_fixture = None;
        // Only the attaching Permissions state and the SDK upgrade have a project open.
        self.presentation.project =
            matches!(state, 26 | 28).then(|| super::fixture::sample_project(1));
        self.presentation.sdk = String::new();
        self.presentation.sdk_upgrade = None;
        match state {
            // 28 the SDK page of a project pinned to an older SDK, after a passing check.
            28 => {
                self.settings_page = SettingsPage::Sdk;
                self.presentation.sdk = "Available · C:\\Users\\you\\.fframes\\sdk\\previous · \
                    Upgrade project to SDK studio-sdk-x86_64-pc-windows-msvc-v5 (fframes 1.4.0) · \
                    the project builds and renders with it"
                    .into();
                self.presentation.sdk_upgrade = Some(studio_engine::engine::backend::SdkUpgrade {
                    sdk_id: "studio-sdk-x86_64-pc-windows-msvc-v5".into(),
                    fframes_version: "1.4.0".into(),
                    check: Some(Ok(())),
                });
            }
            26 => {
                self.settings_page = SettingsPage::Agent;
                agent_tab = AgentTab::Permissions;
            }
            // Without the link's reveal ending: the gallery draws the fixture on every frame.
            27 => {
                self.settings_page = SettingsPage::Agent;
                agent_tab = AgentTab::Agents;
                let scroll = self.settings_scroll.clone();
                self.panel.update(cx, |panel, cx| {
                    panel.setup_fixture(cx);
                    panel.expand_agent("antigravity".into(), scroll, cx);
                });
            }
            // 21 and 22 Find more agents loading and failed.
            21 | 22 => {
                self.settings_page = SettingsPage::Agent;
                agent_tab = AgentTab::Agents;
                self.panel.update(cx, |panel, cx| {
                    panel.registry_state_fixture(state == 22, window, cx)
                });
            }
            12..=14 => {
                self.settings_page = SettingsPage::Agent;
                agent_tab = AgentTab::Permissions;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let rule = |kind: &str, title: Option<&str>, ago: u64| {
                    studio_engine::agent::ApprovalRule {
                        kind: kind.to_owned(),
                        title: title.map(str::to_owned),
                        saved_unix: now - ago,
                    }
                };
                self.approvals_fixture = match state {
                    12 => Some(vec![
                        rule("edit", None, 3 * 86_400),
                        rule("other", Some("mcp__github__list_issues"), 2 * 3600),
                        rule("read", None, 5 * 60),
                    ]),
                    13 => Some(Vec::new()),
                    _ => None,
                };
            }
            8 => {
                self.settings_page = SettingsPage::Agent;
                agent_tab = AgentTab::Agents;
                self.panel.update(cx, |panel, cx| panel.setup_fixture(cx));
            }
            // 15 the CLI agents tab with Find more agents loaded, 16 with Register a custom
            // agent open.
            15 | 16 => {
                self.settings_page = SettingsPage::Agent;
                agent_tab = AgentTab::Agents;
                self.panel.update(cx, |panel, cx| {
                    if state == 15 {
                        panel.registry_fixture(window, cx);
                    } else {
                        panel.custom_agent_fixture(cx);
                    }
                });
            }
            // 10 and 11 the MCP servers and Tools tabs with sample servers; 17 the import
            // panel, 18 the registry browser, 19 and 20 the two tabs in This project.
            10 | 11 | 17..=20 | 23..=25 => {
                use super::agent_tools::Fixture;
                self.settings_page = SettingsPage::Agent;
                agent_tab = if matches!(state, 11 | 20) {
                    AgentTab::Tools
                } else {
                    AgentTab::Mcp
                };
                let fixture = match state {
                    17 => Fixture::Import,
                    18 => Fixture::Registry,
                    19 | 20 => Fixture::Project,
                    23 => Fixture::RegistryLoading,
                    24 => Fixture::RegistryFailed,
                    25 => Fixture::Linked,
                    _ => Fixture::Servers,
                };
                self.agent_tools
                    .update(cx, |page, cx| page.fixture(fixture, window, cx));
            }
            9 => {
                search.update(cx, |input, cx| input.set_text("frame", cx));
                self.follow_settings_search("frame", cx);
            }
            page => {
                if let Some(page) = SettingsPage::ALL.get(page) {
                    self.settings_page = *page;
                }
            }
        }
        self.export_defaults();
        let main = std::mem::replace(&mut self.theme, theme);
        let frame = self.render_settings(
            SettingsChrome {
                header_end: Some(
                    crate::kit::IconButton::new(&theme, "settings-close", Icon::X.path())
                        .emphasis(crate::kit::Emphasis::Ghost)
                        .size(crate::kit::IconButtonSize::Md)
                        .label("Close settings")
                        .into_any_element(),
                ),
                search: Some(search),
                agent_tab,
                ..SettingsChrome::default()
            },
            cx,
        );
        self.theme = main;
        frame
    }

    /// The Agent page: Review, Permissions and the agent panel's Adapter setup, one at a
    /// time under a pill tab bar.
    fn render_agent(
        &self,
        tab: AgentTab,
        on_tab: Option<AgentTabHandler>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let mut tabs = SegmentedControl::new(
            &theme,
            "agent-tab",
            AgentTab::ALL
                .iter()
                .map(|each| SegmentOption::new(each.label(), *each == tab))
                .collect(),
        )
        .tabs();
        if let Some(on_tab) = on_tab {
            tabs = tabs.on_select(move |index, _, cx| {
                if let Some(tab) = AgentTab::ALL.get(*index) {
                    on_tab(*tab, cx);
                }
            });
        }
        let content = match tab {
            AgentTab::Review => self.render_review_policy(cx),
            AgentTab::Permissions => PanelSection::column()
                .child(
                    PanelSection::new(&theme, "Permissions")
                        .description(
                            "How the agent's permission requests are answered. A running task \
                             keeps the choice it started with.",
                        )
                        .children([self.permission_choice(
                            self.permission_policy,
                            |shell, policy, cx| shell.choose_permission_policy(policy, cx),
                            cx,
                        )]),
                )
                .child(self.render_saved_approvals(cx))
                .into_any_element(),
            AgentTab::Agents => self.setup_page.clone().into_any_element(),
            AgentTab::Mcp | AgentTab::Tools => {
                let view = if tab == AgentTab::Mcp {
                    super::agent_tools::ToolsView::Mcp
                } else {
                    super::agent_tools::ToolsView::Tools
                };
                self.follow_agent_tools_project(cx);
                self.agent_tools.update(cx, |page, _| page.view = view);
                self.agent_tools.clone().into_any_element()
            }
        };
        settings_page()
            .child(tabs)
            .child(content)
            .into_any_element()
    }

    /// The Permissions tab's "Saved for this project": the open project's "Always in this
    /// project" rules, each with when it was saved and a Remove button.
    fn render_saved_approvals(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let section = PanelSection::new(&theme, "Saved for this project");
        let rules = match &self.approvals_fixture {
            Some(rules) => Some(rules.clone()),
            None => self.panel.read(cx).approval_rules(),
        };
        let Some(rules) = rules else {
            // A project is open but its agent has not attached yet.
            let note = if self.presentation.project.is_some() {
                "Loading this project's saved rules…"
            } else {
                "Open a project to see its saved rules."
            };
            return section.child(setting_note(&theme, note)).into_any_element();
        };
        if rules.is_empty() {
            return section
                .child(setting_note(
                    &theme,
                    "No saved rules yet. Use \"Always in this project\" on a request.",
                ))
                .into_any_element();
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let shell = cx.entity();
        let rows = rules.into_iter().enumerate().map(|(ix, rule)| {
            let saved = crate::agent::history::relative_time(now, rule.saved_unix);
            let panel = self.panel.clone();
            let shell = shell.clone();
            let description = rule.description();
            ListRow::new(&theme, ("approval-rule", ix))
                .icon(Icon::ShieldQuestion)
                .title(description.clone())
                .meta(if saved.is_empty() {
                    "Saved".to_owned()
                } else {
                    format!("Saved {saved}")
                })
                .label(description)
                .trailing(
                    Button::new(&theme, ("approval-rule-remove", ix))
                        .label("Remove")
                        .size(ButtonSize::Sm)
                        .emphasis(Emphasis::Neutral)
                        .on_click(move |_, _, cx| {
                            panel.update(cx, |panel, cx| panel.forget_approval(&rule, cx));
                            shell.update(cx, |_, cx| cx.notify());
                        }),
                )
        });
        section
            .child(setting_card(&theme).child(List::new().children(rows)))
            .into_any_element()
    }

    /// A settings segmented control whose option `i` reports its bounds as `{id}-{i}`, and
    /// whose clicks call `on_select` with the option's index.
    fn measured_segments(
        &self,
        id: &'static str,
        options: Vec<SegmentOption>,
        on_select: impl Fn(&mut Self, usize, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let entity = cx.entity();
        let mut control = SegmentedControl::new(&self.theme, id, options).on_select({
            let entity = entity.clone();
            move |index, _, cx| entity.update(cx, |shell, cx| on_select(shell, *index, cx))
        });
        if self.qualifying {
            control = control.on_option_bounds(move |index, bounds, cx| {
                entity.update(cx, |shell, _| {
                    shell.button_bounds.insert(
                        format!("{id}-{index}"),
                        crate::probe::bounds_to_record(bounds),
                    );
                });
            });
        }
        control.into_any_element()
    }

    /// The General page's Audio output: the device the preview plays on and the choice of
    /// another, of the default again, or of none.
    fn render_audio_output(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        // The device opens with the preview's audio.
        let ready = self.displayed.is_some();
        let actions = div()
            .flex()
            .flex_wrap()
            .gap(px(METRICS.space_2))
            .child(self.action_button(
                "next-audio-output",
                "Next output",
                ready,
                false,
                None,
                cx,
                |s, cx| s.cycle_output(cx),
            ))
            .child(self.action_button(
                "retry-audio-output",
                "Retry audio",
                ready,
                false,
                None,
                cx,
                |s, cx| s.change_output(OutputDevice::Default, cx),
            ))
            .child(self.action_button(
                "no-audio-output",
                "No audio",
                ready,
                false,
                None,
                cx,
                |s, cx| s.change_output(OutputDevice::Unavailable, cx),
            ));
        let status = if ready {
            self.audio_status()
        } else {
            "Open a preview to choose its output".to_owned()
        };
        PanelSection::new(&theme, "Audio output")
            .description(
                "Where the preview plays its sound. Retry audio goes back to the system's \
                 default device.",
            )
            .children([SettingRow::new(&theme, "Output device")
                .description(status)
                .below(actions)
                .into_any_element()])
            .into_any_element()
    }

    /// The Agent page's Review: whether later tasks apply validated changes themselves or
    /// wait for Apply.
    fn render_review_policy(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let mut children = Vec::new();
        match self.panel.read(cx).review_policy() {
            None => children.push(
                SettingRow::new(&theme, "No project open")
                    .description("Open a project to choose how its agent's changes are applied.")
                    .into_any_element(),
            ),
            Some((current, running)) => {
                let entity = cx.entity();
                let mut control = RadioCards::new(
                    &theme,
                    "review-policy",
                    REVIEW_POLICIES
                        .iter()
                        .map(|(_, label)| RadioCard::new(*label, None))
                        .collect(),
                    REVIEW_POLICIES
                        .iter()
                        .position(|(policy, _)| *policy == current),
                )
                .on_select({
                    let entity = entity.clone();
                    move |index, _, cx| {
                        if let Some((policy, _)) = REVIEW_POLICIES.get(*index) {
                            entity.update(cx, |shell, cx| {
                                shell
                                    .panel
                                    .update(cx, |panel, cx| panel.set_review_policy(*policy, cx));
                            });
                        }
                    }
                });
                if self.qualifying {
                    control = control.on_option_bounds(move |index, bounds, cx| {
                        entity.update(cx, |shell, _| {
                            shell.button_bounds.insert(
                                format!("review-policy-{index}"),
                                crate::probe::bounds_to_record(bounds),
                            );
                        });
                    });
                }
                children.push(control.into_any_element());
                children.push(setting_note(&theme, ReviewPolicy::SCOPE_NOTICE).into_any_element());
                if let Some(running) = running {
                    children.push(
                        setting_note(
                            &theme,
                            format!("The running task keeps {}.", review_label(running)),
                        )
                        .into_any_element(),
                    );
                }
            }
        }
        PanelSection::new(&theme, "Review")
            .description("A change applies to tasks started afterwards.")
            .children(children)
            .into_any_element()
    }

    /// The Export page: the format, size and frame rate every export starts from. The
    /// export panel can change them for one export; what a project's SDK or video cannot
    /// take falls back to the default there.
    fn render_export_defaults(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let defaults = self.export_panel.saved.unwrap_or_default();
        let format = self.measured_segments(
            "default-format",
            FORMATS
                .iter()
                .map(|format| {
                    SegmentOption::new(
                        format_label(*format),
                        format.container == defaults.container && format.codec == defaults.codec,
                    )
                })
                .collect(),
            move |shell, index, cx| {
                if let Some(format) = FORMATS.get(index) {
                    shell.choose_export_defaults(
                        ExportSettings {
                            container: format.container,
                            codec: format.codec,
                            ..defaults
                        },
                        cx,
                    );
                }
            },
            cx,
        );
        let size = self.measured_segments(
            "default-scale",
            SCALES
                .iter()
                .map(|scale| {
                    SegmentOption::new(
                        format!("{}%", (scale * 100.).round() as u32),
                        defaults.scale == *scale,
                    )
                })
                .collect(),
            move |shell, index, cx| {
                if let Some(scale) = SCALES.get(index) {
                    shell.choose_export_defaults(
                        ExportSettings {
                            scale: *scale,
                            ..defaults
                        },
                        cx,
                    );
                }
            },
            cx,
        );
        let fps = self.measured_segments(
            "default-fps",
            DEFAULT_FPS
                .iter()
                .map(|(fps, label)| SegmentOption::new(*label, defaults.fps == *fps))
                .collect(),
            move |shell, index, cx| {
                if let Some((fps, _)) = DEFAULT_FPS.get(index) {
                    shell.choose_export_defaults(
                        ExportSettings {
                            fps: *fps,
                            ..defaults
                        },
                        cx,
                    );
                }
            },
            cx,
        );
        PanelSection::new(&theme, "Export defaults")
            .description(
                "Every export starts with these. The export panel changes them for that export \
                 only.",
            )
            .children([
                // Four formats are too wide to sit beside the label.
                SettingRow::new(&theme, "Format")
                    .description("When the project's SDK lacks it, MP4 · MPEG-4 is used.")
                    .below(format)
                    .into_any_element(),
                SettingRow::new(&theme, "Size")
                    .description("Half size renders faster and makes a smaller file.")
                    .control(size)
                    .into_any_element(),
                SettingRow::new(&theme, "Frame rate")
                    .description("30 fps applies where it divides the video's rate.")
                    .control(fps)
                    .into_any_element(),
            ])
            .into_any_element()
    }

    /// The Appearance page (and the setup's step): the theme as three picture cards, as in
    /// Oleafly's appearance settings, and the motion.
    pub(super) fn render_appearance(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let current = studio_theme::current_preference(cx);
        let entity = cx.entity();
        let mut control = PictureCards::new(
            &theme,
            "theme",
            THEMES
                .iter()
                .map(|(value, label)| {
                    let (icon, appearance) = match value {
                        AppTheme::System => (Icon::Monitor, None),
                        AppTheme::Light => (Icon::Sun, Some(Appearance::Light)),
                        AppTheme::Dark => (Icon::Moon, Some(Appearance::Dark)),
                    };
                    PictureCard::new(*label, appearance_picture(appearance)).icon(icon)
                })
                .collect(),
            THEMES.iter().position(|(value, _)| *value == current),
        )
        .on_select({
            let entity = entity.clone();
            move |index, _, cx| {
                if let Some((choice, _)) = THEMES.get(*index) {
                    entity.update(cx, |shell, cx| shell.choose_theme(*choice, cx));
                }
            }
        });
        if self.qualifying {
            control = control.on_option_bounds(move |index, bounds, cx| {
                entity.update(cx, |shell, _| {
                    shell.button_bounds.insert(
                        format!("theme-{index}"),
                        crate::probe::bounds_to_record(bounds),
                    );
                });
            });
        }
        let current_motion = studio_theme::current_motion(cx);
        let motion = self.measured_segments(
            "motion",
            MOTIONS
                .iter()
                .map(|(value, label)| SegmentOption::new(*label, *value == current_motion))
                .collect(),
            |shell, index, cx| {
                if let Some((choice, _)) = MOTIONS.get(index) {
                    shell.choose_motion(*choice, cx);
                }
            },
            cx,
        );
        PanelSection::new(&theme, "Appearance")
            .children([
                SettingRow::new(&theme, "Theme")
                    .description(
                        "System follows the operating system's light or dark appearance and \
                         changes when it does.",
                    )
                    .below(control)
                    .into_any_element(),
                SettingRow::new(&theme, "Motion")
                    .description("Reduced makes animations snap into place.")
                    .control(motion)
                    .into_any_element(),
            ])
            .into_any_element()
    }
}

/// The review policies in the order the control shows them.
const REVIEW_POLICIES: [(ReviewPolicy, &str); 2] = [
    (ReviewPolicy::AutoApply, "Apply automatically"),
    (ReviewPolicy::ManualReview, "Review before Apply"),
];

fn review_label(policy: ReviewPolicy) -> &'static str {
    match policy {
        ReviewPolicy::AutoApply => "Automatic Apply",
        ReviewPolicy::ManualReview => "Review before Apply",
    }
}

/// The search field's placeholder, Oleafly's.
pub(super) const SEARCH_PLACEHOLDER: &str = "Search settings";

/// One row of Keyboard Shortcuts: the bindings of a scope that share a name, with the
/// keystrokes of each, in [`SHORTCUTS`] order.
fn shortcut_rows(scope: ShortcutScope) -> Vec<(&'static str, Vec<&'static str>)> {
    let mut rows: Vec<(&'static str, Vec<&'static str>)> = Vec::new();
    for shortcut in SHORTCUTS.iter().filter(|shortcut| shortcut.scope == scope) {
        match rows.iter_mut().find(|(label, _)| *label == shortcut.label) {
            Some((_, keys)) => keys.push(shortcut.keys),
            None => rows.push((shortcut.label, vec![shortcut.keys])),
        }
    }
    rows
}

/// Keyboard Shortcuts: every binding of the keymap, grouped by where it works, its keys as
/// key chips. Read-only.
fn render_shortcuts(theme: &Theme) -> gpui::AnyElement {
    let colors = theme.colors;
    let keys = |strokes: Vec<&'static str>| {
        let mut row = div()
            .flex()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap(px(METRICS.space_1));
        for (index, stroke) in strokes.into_iter().enumerate() {
            if index > 0 {
                row = row.child(
                    text_style(div(), TextStyle::Small)
                        .px(px(METRICS.space_1))
                        .text_color(colors.text_3)
                        .child("or"),
                );
            }
            let caps = keystroke_caps(stroke).unwrap_or_else(|| vec![stroke.into()]);
            row = row.children(caps.into_iter().map(|cap| KbdChip::literal(theme, cap)));
        }
        row
    };
    let sections = ShortcutScope::ALL.into_iter().map(|scope| {
        PanelSection::new(theme, scope.label())
            .description(scope.description())
            .children(shortcut_rows(scope).into_iter().map(|(label, strokes)| {
                SettingRow::new(theme, label)
                    .control(keys(strokes))
                    .into_any_element()
            }))
    });
    settings_page()
        .child(
            text_style(div(), TextStyle::Body)
                .text_color(colors.text_2)
                .child(selectable(
                    "The keys Studio answers. Each group works while its part of the window \
                     has focus, never while you type in a field.",
                )),
        )
        .children(sections)
        .into_any_element()
}

/// A settings page's column of sections, `space_6` apart as in Oleafly's settings.
fn settings_page() -> gpui::Div {
    div().flex().flex_col().gap(px(METRICS.space_6))
}

/// Writes the motion `choice` into the app's settings unless a later choice was made
/// meanwhile.
fn save_motion(
    writer: &SettingsWriter,
    generation: u64,
    choice: MotionPreference,
    paths: &studio_engine::app_paths::AppPaths,
) -> Result<(), String> {
    let _write = writer.write.lock();
    if writer.latest_motion.load(Ordering::Acquire) != generation {
        return Ok(());
    }
    let mut settings = AppSettings::load(paths);
    settings.motion = choice;
    settings.save(paths)
}

/// Writes `choice` into the app's settings unless a later choice was made meanwhile.
fn save_theme(
    writer: &SettingsWriter,
    generation: u64,
    choice: AppTheme,
    paths: &studio_engine::app_paths::AppPaths,
) -> Result<(), String> {
    let _write = writer.write.lock();
    if writer.latest.load(Ordering::Acquire) != generation {
        return Ok(());
    }
    let mut settings = AppSettings::load(paths);
    settings.theme = choice;
    settings.save(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_overtaken_by_a_later_choice_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let paths = studio_engine::app_paths::AppPaths::new(dir.path()).unwrap();
        let writer = SettingsWriter::default();
        writer.latest.store(2, Ordering::Release);
        save_theme(&writer, 2, AppTheme::Dark, &paths).unwrap();
        save_theme(&writer, 1, AppTheme::Light, &paths).unwrap();
        assert_eq!(AppSettings::load(&paths).theme, AppTheme::Dark);
    }

    #[test]
    fn the_pages_follow_oleaflys_order_from_general_to_help_and_about() {
        let ids: Vec<_> = SettingsPage::ALL
            .iter()
            .map(|page| page.nav_item(false).id)
            .collect();
        assert_eq!(
            ids,
            [
                "settings-general",
                "settings-appearance",
                "settings-agent",
                "settings-sdk",
                "settings-presets",
                "settings-export",
                "settings-shortcuts",
                "settings-about",
            ]
        );
        assert_eq!(SettingsPage::default(), SettingsPage::General);
    }

    #[test]
    fn the_search_finds_pages_by_name_and_by_their_settings() {
        let found = |query| -> Vec<_> {
            SettingsPage::ALL
                .into_iter()
                .filter(|page| page.matches(query))
                .collect()
        };
        assert_eq!(found(""), SettingsPage::ALL);
        assert_eq!(found("  appear "), [SettingsPage::Appearance]);
        assert_eq!(found("FRAME RATE"), [SettingsPage::Export]);
        assert_eq!(found("rate frame"), [SettingsPage::Export]);
        assert_eq!(found("split"), [SettingsPage::Shortcuts]);
        assert!(found("updates").contains(&SettingsPage::About));
        assert!(found("zzz").is_empty());
    }

    #[test]
    fn keyboard_shortcuts_list_every_binding_of_the_keymap() {
        let listed: Vec<_> = ShortcutScope::ALL
            .into_iter()
            .flat_map(|scope| {
                shortcut_rows(scope)
                    .into_iter()
                    .flat_map(move |(label, keys)| {
                        keys.into_iter().map(move |keys| (scope, label, keys))
                    })
            })
            .collect();
        // Every binding the keymap makes comes from SHORTCUTS, so a binding missing here is
        // one the keymap does not have either.
        assert_eq!(listed.len(), SHORTCUTS.len());
        assert_eq!(super::super::keymap::key_bindings().len(), SHORTCUTS.len());
        for shortcut in SHORTCUTS {
            assert!(
                listed.contains(&(shortcut.scope, shortcut.label, shortcut.keys)),
                "{} {} is not listed",
                shortcut.scope.label(),
                shortcut.keys
            );
            assert!(
                keystroke_caps(shortcut.keys).is_some(),
                "{} does not parse",
                shortcut.keys
            );
        }
    }

    #[test]
    fn the_links_lead_to_the_project() {
        for link in Link::RESOURCES.into_iter().chain(Link::FOOTER) {
            assert!(link.url().starts_with("https://"), "{link:?}");
        }
        assert!(Link::Issues.url().contains(RELEASES_REPOSITORY));
    }

    #[test]
    fn the_options_are_system_light_and_dark() {
        let order: Vec<_> = THEMES.iter().map(|(theme, _)| *theme).collect();
        assert_eq!(order, [AppTheme::System, AppTheme::Light, AppTheme::Dark]);
    }

    #[test]
    fn motion_is_system_reduced_or_full_and_saved_beside_the_theme() {
        let order: Vec<_> = MOTIONS.iter().map(|(motion, _)| *motion).collect();
        assert_eq!(
            order,
            [
                MotionPreference::System,
                MotionPreference::Reduced,
                MotionPreference::Full
            ]
        );
        let dir = tempfile::tempdir().unwrap();
        let paths = studio_engine::app_paths::AppPaths::new(dir.path()).unwrap();
        let writer = SettingsWriter::default();
        writer.latest.store(1, Ordering::Release);
        save_theme(&writer, 1, AppTheme::Dark, &paths).unwrap();
        writer.latest_motion.store(2, Ordering::Release);
        save_motion(&writer, 2, MotionPreference::Reduced, &paths).unwrap();
        save_motion(&writer, 1, MotionPreference::Full, &paths).unwrap();
        let saved = AppSettings::load(&paths);
        assert_eq!(saved.motion, MotionPreference::Reduced);
        assert_eq!(saved.theme, AppTheme::Dark);
    }
}
