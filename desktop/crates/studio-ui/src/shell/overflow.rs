//! The transport bar's overflow menu: the project and agent actions that do not fit on the bar.

use gpui::{Context, IntoElement, Window};
use studio_engine::engine::backend::Command;

use super::{Picker, StudioShell};
use crate::kit::{Icon, IconButton, IconButtonSize, Menu, MenuItem, MenuState};

impl StudioShell {
    /// The overflow menu's rows: the action ids identify them, unavailable ones are
    /// disabled.
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

    /// Actions behind the overflow button, in menu order.
    fn current_overflow_actions(&self) -> Vec<(OverflowAction, bool)> {
        let project = self.presentation.project.as_ref();
        overflow_actions(
            !self.busy,
            project.is_some(),
            project.is_some_and(|p| p.draft.is_some()),
        )
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
                        shell.open_overflow(window, cx);
                    }
                });
            })
            .into_any_element()
    }

    fn open_overflow(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.overflow_items();
        self.overflow_menu = match self.overflow_anchor.get() {
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
        }
    }

    pub(super) fn render_overflow_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        if !self.overflow_open {
            return None;
        }
        let select = cx.entity();
        let highlight = cx.entity();
        let dismiss = cx.entity();
        Some(
            Menu::new(
                &self.theme,
                "overflow-menu",
                self.overflow_items(),
                &self.overflow_menu,
            )
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
}

impl OverflowAction {
    /// The id the action's button had before it moved into the menu.
    fn id(self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::Checkpoint => "checkpoint",
            Self::Restore => "restore",
            Self::RevealDraft => "draft",
            Self::RevealCheckpoint => "reveal-checkpoint",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Refresh => "Refresh",
            Self::Checkpoint => "Save checkpoint",
            Self::Restore => "Restore as copy…",
            Self::RevealDraft => "Reveal retained draft",
            Self::RevealCheckpoint => "Reveal checkpoint",
        }
    }
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
