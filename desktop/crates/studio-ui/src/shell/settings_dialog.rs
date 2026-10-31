//! The settings dialog, Oleafly's: a card over the main window, which dims under it. The
//! view holds what lives only while it is open (the search field, the Agent page's tab and
//! focus); the pages are the shell's, drawn through the shell like
//! [`AgentSetupPage`](crate::agent::AgentSetupPage) draws the panel's.
//!
//! The X in the page header, Escape and a press on the dimmed window close it (Escape in
//! a search with text empties it first); Ctrl+F (Cmd+F) focuses the search, as in Oleafly.

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
    _panel_changed: gpui::Subscription,
    _search_changed: gpui::Subscription,
}

impl SettingsDialog {
    /// A new dialog for `shell`, focused. `panel` is the shell's agent panel: the dialog is
    /// built while the shell renders, so it cannot read the shell then.
    pub(super) fn open(
        shell: Entity<StudioShell>,
        panel: &Entity<ConversationPanel>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let dialog = cx.new(|cx| Self::new(shell, panel, window, cx));
        let focus = dialog.read(cx).focus.clone();
        window.focus(&focus, cx);
        dialog
    }

    fn new(
        shell: Entity<StudioShell>,
        panel: &Entity<ConversationPanel>,
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
            agent_tab: AgentTab::default(),
        }
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
