//! The shell's action menus: the transport bar's overflow menu (the project and agent
//! actions that do not fit on the bar) and the header's Export menu (the export choices
//! behind the Export split button's chevron). One is open at a time; they share the open
//! flag, the menu state and the focus handle.

use gpui::{Bounds, Context, IntoElement, Pixels, Window};
use studio_engine::engine::backend::Command;

use super::settings::SettingsPage;
use super::{Picker, StudioShell};
use crate::export::settings::ExportTarget;
use crate::kit::{Icon, IconButton, IconButtonSize, Menu, MenuItem, MenuState};

/// Which menu is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OverflowMenu {
    /// The transport bar's overflow menu.
    Transport,
    /// The header's Export menu.
    Export,
}

impl StudioShell {
    /// The open menu's rows: the action ids identify them, unavailable ones are disabled.
    fn overflow_items(&self) -> Vec<MenuItem> {
        let actions = self.current_overflow_actions();
        actions
            .iter()
            .zip(disabled_rows(&actions))
            .map(|((action, _), disabled)| {
                MenuItem::new(action.label(), false)
                    .id(action.id())
                    .disabled(disabled)
            })
            .collect()
    }

    /// Actions in the open menu, in menu order.
    fn current_overflow_actions(&self) -> Vec<(OverflowAction, bool)> {
        let project = self.presentation.project.as_ref();
        match self.overflow_kind {
            OverflowMenu::Transport => overflow_actions(
                !self.busy,
                project.is_some(),
                project.is_some_and(|p| p.draft.is_some()),
            ),
            // The rules of the export panel's `can_export`, plus an idle shell like the
            // header's Export button.
            OverflowMenu::Export => export_actions(
                !self.busy
                    && project.is_some()
                    && self.presentation.session.is_some()
                    && self.presentation.agent_sdk.is_some(),
            ),
        }
    }

    pub(super) fn overflow_trigger(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let entity = cx.entity();
        let anchor = self.overflow_anchor.clone();
        IconButton::new(&theme, "overflow-actions", Icon::MoreHorizontal.path())
            .size(IconButtonSize::Md)
            .label("More actions")
            .tooltip("More actions")
            .on_bounds(move |bounds, _| anchor.set(Some(bounds)))
            .on_click(move |_, window, cx| {
                entity.update(cx, |shell, cx| {
                    if shell.overflow_open {
                        shell.close_overflow(window, cx);
                    } else {
                        let anchor = shell.overflow_anchor.get();
                        shell.open_menu(OverflowMenu::Transport, anchor, window, cx);
                    }
                });
            })
            .into_any_element()
    }

    /// Opens or closes the Export menu against `trigger`, the Export split button.
    pub(super) fn toggle_export_menu(
        &mut self,
        trigger: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.overflow_open {
            self.close_overflow(window, cx);
        } else {
            self.open_menu(OverflowMenu::Export, Some(trigger), window, cx);
        }
    }

    /// Whether the Export menu is open, for its trigger's pressed look.
    pub(super) fn export_menu_open(&self) -> bool {
        self.overflow_open && self.overflow_kind == OverflowMenu::Export
    }

    fn open_menu(
        &mut self,
        kind: OverflowMenu,
        trigger: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overflow_kind = kind;
        let items = self.overflow_items();
        self.overflow_menu = match trigger {
            Some(trigger) => MenuState::anchored(trigger, &items),
            None => MenuState::new(gpui::Point::default(), &items),
        };
        self.overflow_open = true;
        window.focus(&self.overflow_focus, cx);
        cx.notify();
    }

    fn close_overflow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overflow_open {
            self.overflow_open = false;
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    /// Marks the open menu's rows from current availability (nothing is cached), so it
    /// never shows or keyboard-navigates a stale enable flag.
    pub(super) fn sync_overflow(&mut self) {
        let disabled = disabled_rows(&self.current_overflow_actions());
        self.overflow_menu.set_disabled(disabled);
    }

    fn run_overflow(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = enabled_overflow_action(&self.current_overflow_actions(), index) else {
            return;
        };
        self.close_overflow(window, cx);
        let project = self.presentation.project.clone();
        match action {
            OverflowAction::Refresh => self.dispatch(Command::Refresh, cx),
            OverflowAction::Checkpoint => self.dispatch(Command::Checkpoint, cx),
            OverflowAction::Restore => self.pick(Picker::Restore, cx),
            OverflowAction::RevealDraft => {
                if let Some(draft) = project.and_then(|p| p.draft) {
                    cx.reveal_path(&draft);
                }
            }
            OverflowAction::RevealCheckpoint => {
                if let Some(project) = project {
                    cx.reveal_path(&project.checkpoint);
                }
            }
            OverflowAction::ExportVideo | OverflowAction::ExportDeck => {
                // The panel's own rule: the target stays while an export runs.
                if self.export_cancelled.is_none() {
                    self.export_panel.target = if action == OverflowAction::ExportDeck {
                        ExportTarget::Deck
                    } else {
                        ExportTarget::Video
                    };
                }
                self.open_export(cx);
            }
            OverflowAction::ExportSettings => self.open_settings_page(SettingsPage::Export, cx),
        }
    }

    /// The transport bar's overflow menu while it is open; the player section hosts it.
    pub(super) fn render_overflow_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.render_menu(OverflowMenu::Transport, "overflow-menu", cx)
    }

    /// The Export menu while it is open; the header hosts it, so it shows over the export
    /// and create panels too.
    pub(super) fn render_export_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.render_menu(OverflowMenu::Export, "export-menu", cx)
    }

    fn render_menu(
        &self,
        kind: OverflowMenu,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.overflow_open || self.overflow_kind != kind {
            return None;
        }
        let select = cx.entity();
        let highlight = cx.entity();
        let dismiss = cx.entity();
        Some(
            Menu::new(&self.theme, id, self.overflow_items(), &self.overflow_menu)
                .focus(&self.overflow_focus)
                .on_highlight(move |index, _, cx| {
                    highlight.update(cx, |shell, cx| {
                        shell.overflow_menu.set_highlighted(*index);
                        cx.notify();
                    });
                })
                .on_select(move |index, window, cx| {
                    select.update(cx, |shell, cx| shell.run_overflow(*index, window, cx));
                })
                .on_dismiss(move |window, cx| {
                    dismiss.update(cx, |shell, cx| shell.close_overflow(window, cx));
                })
                .into_any_element(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverflowAction {
    Refresh,
    Checkpoint,
    Restore,
    RevealDraft,
    RevealCheckpoint,
    /// The export panel with a video file chosen.
    ExportVideo,
    /// The export panel with a slide deck chosen.
    ExportDeck,
    /// Settings → Export, the defaults every export starts from.
    ExportSettings,
}

impl OverflowAction {
    /// The row's id: for the transport's rows, the id the action's button had before it
    /// moved into the menu.
    fn id(self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::Checkpoint => "checkpoint",
            Self::Restore => "restore",
            Self::RevealDraft => "draft",
            Self::RevealCheckpoint => "reveal-checkpoint",
            Self::ExportVideo => "export-menu-video",
            Self::ExportDeck => "export-menu-deck",
            Self::ExportSettings => "export-menu-settings",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Refresh => "Refresh",
            Self::Checkpoint => "Save checkpoint",
            Self::Restore => "Restore as copy…",
            Self::RevealDraft => "Reveal retained draft",
            Self::RevealCheckpoint => "Reveal checkpoint",
            Self::ExportVideo => "Export video…",
            Self::ExportDeck => "Export slide deck (HTML)…",
            Self::ExportSettings => "Export settings…",
        }
    }
}

/// The Export menu's rows: a video or a slide deck when an export is possible (a project, a
/// session and an installed SDK, and an idle shell); the export defaults always, as in
/// Settings.
fn export_actions(can_export: bool) -> Vec<(OverflowAction, bool)> {
    vec![
        (OverflowAction::ExportVideo, can_export),
        (OverflowAction::ExportDeck, can_export),
        (OverflowAction::ExportSettings, true),
    ]
}

/// Every overflow entry with whether it is available now, under the enable rules the
/// buttons had: refresh needs an idle shell; checkpoint and restore need an idle shell and
/// a project; the draft needs a retained draft; the checkpoint reveal needs a project.
/// Unavailable entries stay in the menu as disabled rows.
fn overflow_actions(idle: bool, has_project: bool, has_draft: bool) -> Vec<(OverflowAction, bool)> {
    vec![
        (OverflowAction::Refresh, idle),
        (OverflowAction::Checkpoint, idle && has_project),
        (OverflowAction::Restore, idle && has_project),
        (OverflowAction::RevealDraft, has_project && has_draft),
        (OverflowAction::RevealCheckpoint, has_project),
    ]
}

/// The menu's disabled flags for `actions`, in menu order.
fn disabled_rows(actions: &[(OverflowAction, bool)]) -> Vec<bool> {
    actions.iter().map(|(_, enabled)| !enabled).collect()
}

/// The action at `index` when it is currently available.
fn enabled_overflow_action(
    actions: &[(OverflowAction, bool)],
    index: usize,
) -> Option<OverflowAction> {
    match actions.get(index) {
        Some((action, true)) => Some(*action),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_on_an_action_that_became_unavailable_is_inert() {
        use OverflowAction::*;
        let idle = overflow_actions(true, true, false);
        assert_eq!(enabled_overflow_action(&idle, 1), Some(Checkpoint));
        // The shell went busy after the menu opened.
        let busy = overflow_actions(false, true, false);
        assert_eq!(enabled_overflow_action(&busy, 1), None);
        assert_eq!(enabled_overflow_action(&busy, 4), Some(RevealCheckpoint));
        assert_eq!(enabled_overflow_action(&busy, 9), None);
    }

    #[test]
    fn overflow_actions_follow_the_old_enable_rules() {
        use OverflowAction::*;
        assert_eq!(
            overflow_actions(true, true, true),
            vec![
                (Refresh, true),
                (Checkpoint, true),
                (Restore, true),
                (RevealDraft, true),
                (RevealCheckpoint, true)
            ]
        );
        assert_eq!(
            overflow_actions(true, true, false),
            vec![
                (Refresh, true),
                (Checkpoint, true),
                (Restore, true),
                (RevealDraft, false),
                (RevealCheckpoint, true)
            ]
        );
        assert_eq!(
            overflow_actions(true, false, false),
            vec![
                (Refresh, true),
                (Checkpoint, false),
                (Restore, false),
                (RevealDraft, false),
                (RevealCheckpoint, false)
            ]
        );
        assert_eq!(
            overflow_actions(false, true, false),
            vec![
                (Refresh, false),
                (Checkpoint, false),
                (Restore, false),
                (RevealDraft, false),
                (RevealCheckpoint, true)
            ]
        );
        assert!(
            overflow_actions(false, false, false)
                .iter()
                .all(|(_, on)| !on)
        );
        let ids: Vec<_> = overflow_actions(true, true, true)
            .iter()
            .map(|(a, _)| a.id())
            .collect();
        assert_eq!(
            ids,
            [
                "refresh",
                "checkpoint",
                "restore",
                "draft",
                "reveal-checkpoint"
            ]
        );
    }
}
