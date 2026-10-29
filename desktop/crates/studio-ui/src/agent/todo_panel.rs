//! The agent's current plan as a todo panel docked above the composer.
//!
//! Every driver that plans sends the *whole* list on each update, so the panel
//! holds the latest list and replaces it wholesale. A plan is a display
//! surface only: nothing here feeds back into the workflow.
//!
//! The first half of the file is pure (what the header says, the default open
//! state) and is unit-tested without a window; the second half is the
//! [`TodoPanel`] view.
//!
//! The panel is collapsed to one line (`Todo 2/5` and the item being worked
//! on) or expanded to the full list. The choice is remembered per session key
//! for the life of the view, so switching sessions and back keeps it.

use std::collections::HashMap;

use gpui::{
    AnyElement, Context, Empty, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px,
};
use studio_theme::{METRICS, Theme};

use super::ui::{STATUS_MARK, TRAY_MAX_H, TrayHeader, TrayRow, TrayRowState};
use crate::kit::{Icon, Spinner, icon};

/// Where an item stands. Same variants as the driver's plan status, so a
/// `From` impl on the host side is a one-to-one match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// One line of the agent's plan. Same fields as the driver's plan entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoItem {
    pub text: String,
    pub status: TodoStatus,
}

impl TodoItem {
    pub fn new(text: impl Into<String>, status: TodoStatus) -> Self {
        Self {
            text: text.into(),
            status,
        }
    }
}

impl From<studio_engine::agent::PlanStatus> for TodoStatus {
    fn from(status: studio_engine::agent::PlanStatus) -> Self {
        use studio_engine::agent::PlanStatus;
        match status {
            PlanStatus::Pending => Self::Pending,
            PlanStatus::InProgress => Self::InProgress,
            PlanStatus::Completed => Self::Completed,
        }
    }
}

impl From<studio_engine::agent::PlanEntry> for TodoItem {
    fn from(entry: studio_engine::agent::PlanEntry) -> Self {
        Self::new(entry.text, entry.status.into())
    }
}

/// What the collapsed header reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TodoSummary {
    pub total: usize,
    pub done: usize,
    /// First in-progress item.
    pub active: Option<usize>,
    /// First item not yet completed.
    pub next: Option<usize>,
}

impl TodoSummary {
    pub fn of(items: &[TodoItem]) -> Self {
        let mut summary = Self {
            total: items.len(),
            done: 0,
            active: None,
            next: None,
        };
        for (ix, item) in items.iter().enumerate() {
            match item.status {
                TodoStatus::Completed => summary.done += 1,
                TodoStatus::InProgress => {
                    summary.active.get_or_insert(ix);
                    summary.next.get_or_insert(ix);
                }
                TodoStatus::Pending => {
                    summary.next.get_or_insert(ix);
                }
            }
        }
        summary
    }

    pub fn finished(&self) -> bool {
        self.total > 0 && self.done == self.total
    }

    /// The item the collapsed header names: what is being worked on, else what
    /// is next.
    pub fn headline(&self) -> Option<usize> {
        self.active.or(self.next)
    }

    /// `2/5`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.done, self.total)
    }
}

/// The empty ring of a pending item and the ring of an item paused mid-way.
const RING: f32 = 10.0;

/// The panel view. Hosts keep one entity and call [`TodoPanel::set_plan`]
/// whenever the session's plan changes.
pub struct TodoPanel {
    session: SharedString,
    items: Vec<TodoItem>,
    /// Whether the agent is working right now: only then does the in-progress
    /// glyph spin, so a stopped run does not look busy forever.
    live: bool,
    /// The user's explicit choice per session; absent follows the default
    /// (open while work remains, compact once everything is done).
    expanded: HashMap<SharedString, bool>,
}

impl Default for TodoPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoPanel {
    pub fn new() -> Self {
        Self {
            session: SharedString::default(),
            items: Vec::new(),
            live: false,
            expanded: HashMap::new(),
        }
    }

    /// Shows `items` for `session`, replacing whatever was shown. An empty list
    /// hides the panel. Collapse choices made for other sessions are kept.
    pub fn set_plan(
        &mut self,
        session: impl Into<SharedString>,
        items: Vec<TodoItem>,
        cx: &mut Context<Self>,
    ) {
        let session = session.into();
        if self.session == session && self.items == items {
            return;
        }
        self.session = session;
        self.items = items;
        cx.notify();
    }

    pub fn set_live(&mut self, live: bool, cx: &mut Context<Self>) {
        if self.live != live {
            self.live = live;
            cx.notify();
        }
    }

    pub fn session(&self) -> &str {
        &self.session
    }

    pub fn items(&self) -> &[TodoItem] {
        &self.items
    }

    /// Whether the panel draws anything: an empty plan hides it.
    pub fn is_visible(&self) -> bool {
        !self.items.is_empty()
    }

    pub fn summary(&self) -> TodoSummary {
        TodoSummary::of(&self.items)
    }

    /// Whether the current session's list is open.
    pub fn is_expanded(&self) -> bool {
        self.is_expanded_for(&self.session)
    }

    /// Whether `session`'s list is open, judged against the list now shown.
    pub fn is_expanded_for(&self, session: &str) -> bool {
        let finished = self.summary().finished();
        self.expanded.get(session).copied().unwrap_or(!finished)
    }

    /// Flips the current session's list between open and collapsed.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        let open = !self.is_expanded();
        self.expanded.insert(self.session.clone(), open);
        cx.notify();
    }

    fn header(&self, expanded: bool, theme: &Theme, cx: &mut Context<Self>) -> TrayHeader {
        let summary = self.summary();
        let colors = theme.colors;
        let entity = cx.entity();
        let mut header = TrayHeader::new(theme, "todo-panel-toggle", Icon::Layers, "Todo")
            .count(summary.label())
            .open(expanded)
            .selectors("todo-panel-toggle", "todo-panel-count")
            .on_toggle(move |_, cx| entity.update(cx, |this, cx| this.toggle(cx)));
        if summary.finished() {
            header = header
                .icon_color(colors.success)
                .current("All done", colors.text_3);
        } else if let Some(ix) = summary.headline() {
            header = header.current(self.items[ix].text.clone(), colors.text_2);
        }
        header
    }

    fn item_row(&self, ix: usize, item: &TodoItem, theme: &Theme) -> TrayRow {
        TrayRow::new(theme, ("todo-item", ix), item.text.clone())
            .selector(format!("todo-item-{ix}"))
            .state(match item.status {
                TodoStatus::Completed => TrayRowState::Done,
                TodoStatus::InProgress => TrayRowState::Current,
                TodoStatus::Pending => TrayRowState::Pending,
            })
            .mark(self.status_glyph(ix, item.status, theme))
    }

    /// Check, spinner (or a still ring when the agent is idle), empty ring.
    fn status_glyph(&self, ix: usize, status: TodoStatus, theme: &Theme) -> AnyElement {
        let colors = theme.colors;
        match status {
            TodoStatus::Completed => {
                icon(Icon::Check, STATUS_MARK, colors.success).into_any_element()
            }
            TodoStatus::InProgress if self.live => {
                Spinner::new(theme, ("todo-spin", ix)).into_any_element()
            }
            TodoStatus::InProgress => div()
                .size(px(RING))
                .rounded_full()
                .border_1()
                .border_color(colors.accent)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .size(px(METRICS.space_1))
                        .rounded_full()
                        .bg(colors.accent),
                )
                .into_any_element(),
            TodoStatus::Pending => div()
                .size(px(RING))
                .rounded_full()
                .border_1()
                .border_color(colors.text_3)
                .into_any_element(),
        }
    }
}

impl Render for TodoPanel {
    /// The header and the rows, without a frame: the dock sets them on the tray above the
    /// composer.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_visible() {
            return Empty.into_any_element();
        }
        let theme = Theme::for_window(window, cx);
        let expanded = self.is_expanded();
        div()
            .id("todo-panel")
            .debug_selector(|| "todo-panel".into())
            .w_full()
            .flex()
            .flex_col()
            .child(self.header(expanded, &theme, cx))
            .when(expanded, |panel| {
                panel.child(
                    div()
                        .id("todo-panel-rows")
                        .max_h(px(TRAY_MAX_H))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(
                            self.items
                                .iter()
                                .enumerate()
                                .map(|(ix, item)| self.item_row(ix, item, &theme)),
                        ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

    use super::*;

    #[test]
    fn an_engine_plan_entry_becomes_the_same_item() {
        use studio_engine::agent::{PlanEntry, PlanStatus};
        let entry = |text: &str, status| PlanEntry {
            text: text.into(),
            status,
        };
        let items: Vec<TodoItem> = [
            entry("a", PlanStatus::Completed),
            entry("b", PlanStatus::InProgress),
            entry("c", PlanStatus::Pending),
        ]
        .into_iter()
        .map(TodoItem::from)
        .collect();
        assert_eq!(
            items,
            vec![
                TodoItem::new("a", TodoStatus::Completed),
                TodoItem::new("b", TodoStatus::InProgress),
                TodoItem::new("c", TodoStatus::Pending),
            ]
        );
    }

    /// x = completed, > = in progress, . = pending
    fn items(spec: &str) -> Vec<TodoItem> {
        spec.chars()
            .enumerate()
            .map(|(i, c)| {
                TodoItem::new(
                    format!("item {i}"),
                    match c {
                        'x' => TodoStatus::Completed,
                        '>' => TodoStatus::InProgress,
                        _ => TodoStatus::Pending,
                    },
                )
            })
            .collect()
    }

    fn panel(cx: &mut TestAppContext) -> (Entity<TodoPanel>, &mut VisualTestContext) {
        cx.add_window_view(|_, _| TodoPanel::new())
    }

    fn set(
        panel: &Entity<TodoPanel>,
        session: &str,
        list: Vec<TodoItem>,
        cx: &mut VisualTestContext,
    ) {
        panel.update(cx, |panel, cx| panel.set_plan(session.to_owned(), list, cx));
        cx.run_until_parked();
    }

    fn click(selector: &'static str, cx: &mut VisualTestContext) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"));
        cx.simulate_click(bounds.center(), Modifiers::none());
        cx.run_until_parked();
    }

    fn shown_items(cx: &mut VisualTestContext, count: usize) -> usize {
        const NAMES: [&str; 8] = [
            "todo-item-0",
            "todo-item-1",
            "todo-item-2",
            "todo-item-3",
            "todo-item-4",
            "todo-item-5",
            "todo-item-6",
            "todo-item-7",
        ];
        NAMES[..count]
            .iter()
            .filter(|name| cx.debug_bounds(name).is_some())
            .count()
    }

    #[test]
    fn summary_counts_and_names_the_current_item() {
        let s = TodoSummary::of(&items("xx>.."));
        assert_eq!(
            (s.total, s.done, s.active, s.next),
            (5, 2, Some(2), Some(2))
        );
        assert_eq!(s.label(), "2/5");
        assert_eq!(s.headline(), Some(2));
        assert!(!s.finished());
        // Nothing started: the first unfinished item is the headline.
        let s = TodoSummary::of(&items("x..."));
        assert_eq!((s.active, s.headline()), (None, Some(1)));
        // An in-progress item later in the list wins over an earlier pending
        // one: the agent is working on it.
        assert_eq!(TodoSummary::of(&items(".>.")).headline(), Some(1));
        let s = TodoSummary::of(&items("xxx"));
        assert!(s.finished());
        assert_eq!(s.headline(), None);
        assert!(!TodoSummary::of(&[]).finished());
    }

    #[gpui::test]
    fn an_empty_plan_hides_the_panel(cx: &mut TestAppContext) {
        let (panel, cx) = panel(cx);
        assert!(!panel.read_with(cx, |p, _| p.is_visible()));
        assert!(cx.debug_bounds("todo-panel").is_none());

        set(&panel, "a", items("x.."), cx);
        assert!(cx.debug_bounds("todo-panel").is_some());

        // The agent clearing its list takes the panel away again.
        set(&panel, "a", Vec::new(), cx);
        assert!(!panel.read_with(cx, |p, _| p.is_visible()));
        assert!(cx.debug_bounds("todo-panel").is_none());
        assert!(cx.debug_bounds("todo-panel-toggle").is_none());
    }

    #[gpui::test]
    fn an_update_replaces_the_list(cx: &mut TestAppContext) {
        let (panel, cx) = panel(cx);
        set(&panel, "a", items("x>."), cx);
        assert_eq!(shown_items(cx, 6), 3);

        set(&panel, "a", items("xx>..."), cx);
        assert_eq!(
            panel.read_with(cx, |p, _| p.items().to_vec()),
            items("xx>...")
        );
        assert_eq!(shown_items(cx, 6), 6);

        // A shorter list leaves nothing of the longer one behind.
        set(&panel, "a", items(">"), cx);
        assert_eq!(shown_items(cx, 6), 1);
        assert_eq!(panel.read_with(cx, |p, _| p.summary().label()), "0/1");
    }

    #[gpui::test]
    fn the_collapsed_summary_reads_completed_over_total(cx: &mut TestAppContext) {
        let (panel, cx) = panel(cx);
        set(&panel, "a", items("xx>.."), cx);
        // Open while work remains.
        assert!(panel.read_with(cx, |p, _| p.is_expanded()));
        assert_eq!(shown_items(cx, 5), 5);

        click("todo-panel-toggle", cx);
        assert!(!panel.read_with(cx, |p, _| p.is_expanded()));
        assert_eq!(shown_items(cx, 5), 0);
        assert!(cx.debug_bounds("todo-panel-count").is_some());
        assert_eq!(panel.read_with(cx, |p, _| p.summary().label()), "2/5");

        // The count follows the list while collapsed.
        set(&panel, "a", items("xxx>."), cx);
        assert_eq!(panel.read_with(cx, |p, _| p.summary().label()), "3/5");
        assert!(!panel.read_with(cx, |p, _| p.is_expanded()));
    }

    #[gpui::test]
    fn the_collapse_choice_is_kept_per_session_across_updates(cx: &mut TestAppContext) {
        let (panel, cx) = panel(cx);
        set(&panel, "a", items("x.."), cx);
        click("todo-panel-toggle", cx);
        assert!(!panel.read_with(cx, |p, _| p.is_expanded_for("a")));

        // An update of the same session keeps the choice.
        set(&panel, "a", items("xx."), cx);
        assert!(!panel.read_with(cx, |p, _| p.is_expanded()));

        // Another session starts from its own default and keeps its own choice.
        set(&panel, "b", items("x.."), cx);
        assert!(panel.read_with(cx, |p, _| p.is_expanded()));
        assert_eq!(shown_items(cx, 3), 3);

        // Back on the first session its choice is still there.
        set(&panel, "a", items("xx."), cx);
        assert!(!panel.read_with(cx, |p, _| p.is_expanded()));
        assert_eq!(shown_items(cx, 3), 0);
        assert!(panel.read_with(cx, |p, _| p.is_expanded_for("b")));

        // Reopening it by hand sticks too.
        click("todo-panel-toggle", cx);
        assert!(panel.read_with(cx, |p, _| p.is_expanded()));
        set(&panel, "a", items("xxx"), cx);
        assert!(panel.read_with(cx, |p, _| p.is_expanded()));
    }

    #[gpui::test]
    fn a_finished_list_defaults_to_compact(cx: &mut TestAppContext) {
        let (panel, cx) = panel(cx);
        set(&panel, "a", items("xxx"), cx);
        assert!(!panel.read_with(cx, |p, _| p.is_expanded()));
        assert!(panel.read_with(cx, |p, _| p.summary().finished()));
        assert_eq!(shown_items(cx, 3), 0);
        // New work reopens it.
        set(&panel, "a", items("xxx."), cx);
        assert!(panel.read_with(cx, |p, _| p.is_expanded()));
    }

    #[gpui::test]
    fn the_list_renders_while_live_and_idle(cx: &mut TestAppContext) {
        let (panel, cx) = panel(cx);
        set(&panel, "a", items("x>."), cx);
        panel.update(cx, |p, cx| p.set_live(true, cx));
        cx.run_until_parked();
        assert_eq!(shown_items(cx, 3), 3);
        panel.update(cx, |p, cx| p.set_live(false, cx));
        cx.run_until_parked();
        assert_eq!(shown_items(cx, 3), 3);
    }
}
