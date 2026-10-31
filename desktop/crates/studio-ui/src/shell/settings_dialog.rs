//! The settings dialog, Oleafly's: a card over the main window, which dims under it. The
//! view holds what lives only while it is open (the search field, the Agent page's tab and
//! focus); the pages are the shell's, drawn through the shell like
//! [`AgentSetupPage`](crate::agent::AgentSetupPage) draws the panel's.
//!
//! The X in the page header, Escape and a press on the dimmed window close it (Escape in
//! a search with text empties it first); Ctrl+F (Cmd+F) focuses the search, as in Oleafly.
//! While it is open Tab keeps to its controls ([`modal`]); closing it gives the focus back
//! to whatever had it before.

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, Styled, Window, div,
};
use studio_theme::Theme;

use super::StudioShell;
use super::settings::{AgentTab, SEARCH_PLACEHOLDER, SettingsChrome};
use crate::agent::ConversationPanel;
use crate::kit::{
    Emphasis, Icon, IconButton, IconButtonSize, TextInputEvent, TextInputState, modal,
};

pub(super) struct SettingsDialog {
    shell: Entity<StudioShell>,
    focus: FocusHandle,
    /// The nav column's search field; Ctrl+F (Cmd+F) focuses it, Escape empties it.
    search: Entity<TextInputState>,
    agent_tab: AgentTab,
    /// What had the focus when the dialog opened; it gets it back on close.
    pub(super) return_focus: Option<FocusHandle>,
    _panel_changed: gpui::Subscription,
    _search_changed: gpui::Subscription,
}

impl SettingsDialog {
    /// A new dialog for `shell`, focused, whose Agent page opens at `agent_tab`. `panel` is
    /// the shell's agent panel: the dialog is built while the shell renders or updates, so it
    /// cannot read the shell then.
    pub(super) fn open(
        shell: Entity<StudioShell>,
        panel: &Entity<ConversationPanel>,
        agent_tab: AgentTab,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let return_focus = window.focused(cx);
        let dialog = cx.new(|cx| Self::new(shell, panel, agent_tab, window, cx));
        dialog.update(cx, |dialog, _| dialog.return_focus = return_focus);
        let focus = dialog.read(cx).focus.clone();
        window.focus(&focus, cx);
        dialog
    }

    fn new(
        shell: Entity<StudioShell>,
        panel: &Entity<ConversationPanel>,
        agent_tab: AgentTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The Agent page's Review reads the panel's snapshot, which changes without the shell.
        let search = cx.new(|cx| {
            let mut input = TextInputState::single_line(window, cx);
            input.set_placeholder(SEARCH_PLACEHOLDER);
            input
        });
        let search_changed = cx.subscribe(&search, |this, search, event, cx| match event {
            TextInputEvent::Changed => {
                let query = search.read(cx).text().to_owned();
                this.shell
                    .update(cx, |shell, cx| shell.follow_settings_search(&query, cx));
                cx.notify();
            }
            // Escape empties the field, as in Oleafly; in an empty field it closes the dialog.
            TextInputEvent::Cancelled => {
                if search.read(cx).text().is_empty() {
                    this.close(cx);
                } else {
                    search.update(cx, |input, cx| input.set_text("", cx));
                }
            }
            _ => {}
        });
        Self {
            _panel_changed: cx.observe(panel, |_, _, cx| cx.notify()),
            _search_changed: search_changed,
            shell,
            focus: cx.focus_handle(),
            search,
            agent_tab,
            return_focus: None,
        }
    }

    /// Turns the Agent page to `tab` (a deep link while the dialog is open).
    pub(super) fn show_agent_tab(&mut self, tab: AgentTab, cx: &mut Context<Self>) {
        if self.agent_tab != tab {
            self.agent_tab = tab;
            cx.notify();
        }
    }

    /// The Agent page's tab.
    #[cfg(test)]
    pub(super) fn agent_tab(&self) -> AgentTab {
        self.agent_tab
    }

    fn close(&self, cx: &mut App) {
        self.shell.update(cx, |shell, cx| shell.close_settings(cx));
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::for_window(window, cx);
        let this = cx.weak_entity();
        let close = {
            let shell = self.shell.clone();
            IconButton::new(&theme, "settings-close", Icon::X.path())
                .emphasis(Emphasis::Ghost)
                .size(IconButtonSize::Md)
                .label("Close settings")
                .tooltip("Close settings")
                .on_click(move |_, _, cx| shell.update(cx, |shell, cx| shell.close_settings(cx)))
        };
        let chrome = SettingsChrome {
            header_end: Some(close.into_any_element()),
            search: Some(self.search.clone()),
            agent_tab: self.agent_tab,
            on_agent_tab: Some(std::rc::Rc::new(move |tab, cx: &mut App| {
                let _ = this.update(cx, |this, cx| {
                    this.agent_tab = tab;
                    cx.notify();
                });
            })),
            ..SettingsChrome::default()
        };
        let frame = self
            .shell
            .update(cx, |shell, cx| shell.render_settings(chrome, cx));
        let shell = self.shell.clone();
        modal(
            &theme,
            "settings-dialog",
            window.viewport_size(),
            &self.focus,
            move |_, cx| shell.update(cx, |shell, cx| shell.close_settings(cx)),
            div()
                .id("settings-window")
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    let keystroke = &event.keystroke;
                    // The search field answers its own Escape (see `Cancelled`).
                    let in_search = this.search.read(cx).focus_handle().is_focused(window);
                    if keystroke.key == "escape" && !keystroke.modifiers.modified() {
                        if !in_search {
                            this.close(cx);
                            cx.stop_propagation();
                        }
                        return;
                    }
                    if keystroke.modifiers.secondary() && keystroke.key == "f" {
                        this.search
                            .update(cx, |input, cx| input.focus_and_select_all(window, cx));
                        cx.stop_propagation();
                    }
                }))
                .size_full()
                .flex()
                .text_color(theme.colors.text_1)
                .child(frame),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use studio_engine::agent_settings::AgentToolSettings;
    use studio_engine::containment::OwnershipPolicy;
    use studio_engine::engine::backend::StudioEnvironment;
    use studio_proto::PermissionPolicy;

    use super::*;
    use crate::agent::{PanelEvent, SettingsLink};
    use crate::shell::settings::SettingsPage;

    /// A shell without a project whose engine has started, the first-run setup closed.
    fn idle_shell<'a>(
        temp: &tempfile::TempDir,
        cx: &'a mut gpui::TestAppContext,
    ) -> (Entity<StudioShell>, &'a mut gpui::VisualTestContext) {
        cx.update(crate::shell::bind_keys);
        let environment = StudioEnvironment {
            app_data: Some(temp.path().join("app")),
            sdk_home: Some(temp.path().join("sdk")),
            builds: studio_engine::services::worker_project::shared_build_service(),
            ownership: OwnershipPolicy::Validated,
        };
        cx.executor().allow_parking();
        let (shell, cx) = cx.add_window_view(|window, cx| {
            StudioShell::with_environment(environment, "0.0.0", window, cx)
        });
        for _ in 0..1000 {
            cx.run_until_parked();
            if !shell.read_with(cx, |shell, _| shell.busy) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        shell.update(cx, |shell, cx| {
            shell.onboarding = None;
            cx.notify();
        });
        cx.run_until_parked();
        (shell, cx)
    }

    fn emit(shell: &Entity<StudioShell>, event: PanelEvent, cx: &mut gpui::VisualTestContext) {
        let panel = shell.read_with(cx, |shell, _| shell.panel.clone());
        panel.update(cx, |_, cx| cx.emit(event));
        cx.run_until_parked();
    }

    /// The open dialog and its Agent tab; the page must be Agent.
    fn open_tab(
        shell: &Entity<StudioShell>,
        cx: &mut gpui::VisualTestContext,
    ) -> (Entity<SettingsDialog>, AgentTab) {
        let dialog = shell.read_with(cx, |shell, _| {
            assert!(shell.settings_open, "settings are open");
            assert_eq!(shell.settings_page, SettingsPage::Agent);
            shell.settings_dialog.clone().expect("the dialog is built")
        });
        let tab = dialog.read_with(cx, |dialog, _| dialog.agent_tab());
        (dialog, tab)
    }

    #[gpui::test]
    fn a_panel_link_opens_settings_on_its_agent_tab(cx: &mut gpui::TestAppContext) {
        let temp = tempfile::tempdir().unwrap();
        let (shell, cx) = idle_shell(&temp, cx);
        let tools = shell.read_with(cx, |shell, _| shell.agent_tools.clone());
        let mut settings = AgentToolSettings::default();
        settings
            .mcp_servers
            .push(studio_engine::agent_settings::UserMcpServer::stdio(
                "github",
                "npx",
                Vec::new(),
                Vec::new(),
            ));
        tools.update(cx, |page, cx| page.load(settings, cx));
        emit(
            &shell,
            PanelEvent::OpenSettings(SettingsLink::new(AgentTab::Mcp).target("github")),
            cx,
        );
        let (dialog, tab) = open_tab(&shell, cx);
        assert_eq!(tab, AgentTab::Mcp);
        // A link to a server highlights its card on MCP servers.
        assert_eq!(
            tools.read_with(cx, |page, _| page.highlighted().map(str::to_owned)),
            Some("github".to_owned())
        );

        // A link while the dialog is open turns the same dialog to its tab.
        emit(
            &shell,
            PanelEvent::OpenSettings(SettingsLink::new(AgentTab::Permissions)),
            cx,
        );
        let (again, tab) = open_tab(&shell, cx);
        assert_eq!(again.entity_id(), dialog.entity_id());
        assert_eq!(tab, AgentTab::Permissions);

        // A link to an agent opens CLI agents with that agent's card open.
        emit(
            &shell,
            PanelEvent::OpenSettings(SettingsLink::new(AgentTab::Agents).target("claude")),
            cx,
        );
        assert_eq!(open_tab(&shell, cx).1, AgentTab::Agents);
        let panel = shell.read_with(cx, |shell, _| shell.panel.clone());
        assert!(panel.read_with(cx, |panel, _| panel.settings_state().2.contains("claude")));

        // The keymap's Open agent setup opens CLI agents too.
        shell.update(cx, |shell, cx| shell.close_settings(cx));
        shell.update_in(cx, |shell, window, cx| window.focus(&shell.focus, cx));
        cx.dispatch_action(crate::actions::OpenAgentSetup);
        cx.run_until_parked();
        assert_eq!(open_tab(&shell, cx).1, AgentTab::Agents);
    }

    #[gpui::test]
    fn tab_stays_in_the_open_settings_and_closing_gives_the_focus_back(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::kit::test_support::press;
        let temp = tempfile::tempdir().unwrap();
        let (shell, cx) = idle_shell(&temp, cx);
        let before = shell.read_with(cx, |shell, _| shell.focus.clone());
        shell.update_in(cx, |_, window, cx| window.focus(&before, cx));
        cx.dispatch_action(crate::actions::OpenAgentSetup);
        cx.run_until_parked();
        let (dialog, _) = open_tab(&shell, cx);
        let inside = dialog.read_with(cx, |dialog, _| dialog.focus.clone());
        let mut seen: Vec<gpui::FocusHandle> = Vec::new();
        // More presses than the page has stops, both ways: the window under the scrim is
        // never reached.
        for key in std::iter::repeat_n("tab", 60).chain(std::iter::repeat_n("shift-tab", 60)) {
            press(cx, key);
            let focused = cx.update(|window, cx| {
                assert!(
                    inside.contains_focused(window, cx),
                    "{key} left the settings dialog"
                );
                window.focused(cx).expect("something has the focus")
            });
            if !seen.contains(&focused) {
                seen.push(focused);
            }
        }
        assert!(seen.len() > 3, "Tab moves through the dialog's controls");

        shell.update(cx, |shell, cx| shell.close_settings(cx));
        cx.run_until_parked();
        assert!(
            cx.update(|window, _| before.is_focused(window)),
            "the focus is back where it was"
        );
    }

    #[gpui::test]
    fn the_composer_saves_tools_and_policy_through_the_settings_paths(
        cx: &mut gpui::TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let (shell, cx) = idle_shell(&temp, cx);
        let mut tools = AgentToolSettings::default();
        tools.set_tool("timeline", false);
        emit(&shell, PanelEvent::SetAgentTools(tools.clone()), cx);
        emit(
            &shell,
            PanelEvent::SetPermissionPolicy(PermissionPolicy::AllowEverything),
            cx,
        );
        let page = shell.read_with(cx, |shell, cx| shell.agent_tools.read(cx).effective());
        assert_eq!(page, tools, "Settings → Agent → Tools shows the change");
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.permission_policy),
            PermissionPolicy::AllowEverything
        );
        let panel = shell.read_with(cx, |shell, _| shell.panel.clone());
        panel.read_with(cx, |panel, _| {
            let (held, policy, _) = panel.settings_state();
            assert_eq!(held, &tools, "the panel holds what the page holds");
            assert_eq!(policy, PermissionPolicy::AllowEverything);
        });

        // With a project open, the popover shows and changes that project's tools: turning
        // timeline back on there leaves it off for every other project.
        let id = studio_project::ProjectId::try_from("demo".to_owned()).unwrap();
        shell.update(cx, |shell, cx| {
            shell.agent_tools.update(cx, |page, _| {
                page.follow_project(Some((id, "Demo".into())), None)
            })
        });
        emit(
            &shell,
            PanelEvent::SetAgentTools(AgentToolSettings::default()),
            cx,
        );
        let (effective, overrides) = shell.read_with(cx, |shell, cx| {
            let page = shell.agent_tools.read(cx);
            (page.effective(), page.project_override().unwrap().1)
        });
        assert!(effective.enabled_tools().contains(&"timeline"));
        assert!(overrides.changes_tools(), "the change is the project's");
        let saved = studio_engine::app_settings::AppSettings::load(
            &shell.read_with(cx, |shell, _| shell.presentation.paths.clone().unwrap()),
        );
        assert!(
            !saved.agent.enabled_tools().contains(&"timeline"),
            "every other project keeps timeline off"
        );
        panel.read_with(cx, |panel, _| {
            assert_eq!(
                panel.settings_state().0,
                &effective,
                "the panel shows the project's tools"
            );
        });
    }
}
