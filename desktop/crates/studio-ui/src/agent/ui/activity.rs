//! A run of tool calls, reasoning and permission notes: one muted summary line
//! ("Read 3 files · edited 2 files · 1 failed") that opens into one row per step.

use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::metrics::activity as a;
use super::{ITEM_RADIUS, STATUS_MARK};
use crate::kit::{Handler, Icon, Spinner, focusable, icon, on_activate, with_tooltip};

/// What a run of activity did, counted for its summary line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActivityCounts {
    pub read: usize,
    pub edited: usize,
    pub ran: usize,
    pub searched: usize,
    pub fetched: usize,
    pub other: usize,
    pub thoughts: usize,
    pub permissions: usize,
    pub failed: usize,
}

impl ActivityCounts {
    /// Counts one tool call by the ACP tool kind its driver reported.
    pub fn add_tool(&mut self, kind: Option<&str>, failed: bool) {
        let slot = match kind {
            Some("read") => &mut self.read,
            Some("edit" | "delete" | "move") => &mut self.edited,
            Some("execute") => &mut self.ran,
            Some("search") => &mut self.searched,
            Some("fetch") => &mut self.fetched,
            Some("think") => &mut self.thoughts,
            _ => &mut self.other,
        };
        *slot += 1;
        if failed {
            self.failed += 1;
        }
    }
}

fn counted(n: usize, verb: &str, one: &str, many: &str) -> Option<String> {
    (n > 0).then(|| format!("{verb} {n} {}", if n == 1 { one } else { many }))
}

/// The summary's parts in order, each with whether it reports a failure (only that part is
/// coloured). The first part starts with a capital.
pub fn summary_parts(counts: &ActivityCounts) -> Vec<(String, bool)> {
    let mut parts: Vec<(String, bool)> = [
        counted(counts.read, "read", "file", "files"),
        counted(counts.edited, "edited", "file", "files"),
        counted(counts.ran, "ran", "command", "commands"),
        counted(counts.searched, "searched", "time", "times"),
        counted(counts.fetched, "fetched", "page", "pages"),
        counted(counts.other, "used", "tool", "tools"),
        (counts.thoughts > 0).then(|| "thought".to_owned()),
        counted(counts.permissions, "asked", "permission", "permissions"),
    ]
    .into_iter()
    .flatten()
    .map(|part| (part, false))
    .collect();
    if counts.failed > 0 {
        parts.push((format!("{} failed", counts.failed), true));
    }
    if let Some((first, _)) = parts.first_mut() {
        let mut chars = first.chars();
        if let Some(c) = chars.next() {
            *first = c.to_uppercase().chain(chars).collect();
        }
    }
    parts
}

/// The collapsed line of a run, and its header while open: a chevron, the summary in
/// `text_2` (`text_1` on hover; only the failed part in `danger`). Progress shows once, in
/// the turn's "Working" trailer.
#[derive(IntoElement)]
pub struct ActivityHeader {
    theme: Theme,
    id: ElementId,
    parts: Vec<(String, bool)>,
    open: bool,
    on_toggle: Option<Handler>,
}

impl ActivityHeader {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, parts: Vec<(String, bool)>) -> Self {
        Self {
            theme: *theme,
            id: id.into(),
            parts,
            open: false,
            on_toggle: None,
        }
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for ActivityHeader {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ActivityHeader {
            theme,
            id,
            parts,
            open,
            on_toggle,
        } = self;
        let colors = theme.colors;
        let group = SharedString::from(format!("activity-{id}"));
        let interactive = on_toggle.is_some();
        let row = focusable(div().id(id), &theme, !interactive);
        let last = parts.len().saturating_sub(1);
        let summary = text_style(div(), TextStyle::Label)
            .min_w_0()
            .truncate()
            .text_color(colors.text_2)
            .group_hover(group.clone(), move |style| style.text_color(colors.text_1))
            .children(parts.into_iter().enumerate().map(|(ix, (part, failed))| {
                let text = if ix == last {
                    part
                } else {
                    format!("{part} · ")
                };
                if failed {
                    div()
                        .text_color(colors.danger)
                        .child(text)
                        .into_any_element()
                } else {
                    div().child(text).into_any_element()
                }
            }));
        on_activate(row, false, on_toggle)
            .group(group)
            .role(Role::Button)
            .aria_expanded(open)
            .h(px(a::ROW_H))
            .flex()
            .items_center()
            .gap(px(a::SUMMARY_GAP))
            .rounded(ITEM_RADIUS)
            .when(interactive, |row| row.cursor_pointer())
            .child(icon(
                if open {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                a::DISCLOSURE,
                colors.text_3,
            ))
            .child(summary.flex().flex_row())
    }
}

/// Where one step stands; only a failed or waiting step is coloured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ActivityState {
    /// Not started: the verb in `text_3`.
    Pending,
    /// A spinner at the end.
    Running,
    /// Nothing at the end.
    #[default]
    Done,
    /// The icon, the verb and a cross at the end in `danger`.
    Failed,
    /// Waits for the user: the icon in `warning`.
    Waiting,
}

/// One step of an open run: an icon, a verb, the subject truncated (the full text in the
/// tooltip) and a status mark. Indented under the run's chevron.
#[derive(IntoElement)]
pub struct ActivityRow {
    theme: Theme,
    id: ElementId,
    icon: Icon,
    verb: SharedString,
    subject: SharedString,
    state: ActivityState,
    trailing: Option<AnyElement>,
    on_click: Option<Handler>,
    open: Option<bool>,
}

impl ActivityRow {
    pub fn new(
        theme: &Theme,
        id: impl Into<ElementId>,
        icon: Icon,
        verb: impl Into<SharedString>,
        subject: impl Into<SharedString>,
    ) -> Self {
        Self {
            theme: *theme,
            id: id.into(),
            icon,
            verb: verb.into(),
            subject: subject.into(),
            state: ActivityState::Done,
            trailing: None,
            on_click: None,
            open: None,
        }
    }

    pub fn state(mut self, state: ActivityState) -> Self {
        self.state = state;
        self
    }

    /// An element before the status mark: a `Small` note.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    /// A row with detail: clicking toggles it, and a chevron shows its state on hover.
    pub fn on_toggle(
        mut self,
        open: bool,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.open = Some(open);
        self.on_click = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for ActivityRow {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ActivityRow {
            theme,
            id,
            icon: glyph,
            verb,
            subject,
            state,
            trailing,
            on_click,
            open,
        } = self;
        let colors = theme.colors;
        let (icon_color, verb_color) = match state {
            ActivityState::Failed => (colors.danger, colors.danger),
            ActivityState::Waiting => (colors.warning, colors.text_1),
            ActivityState::Pending => (colors.text_3, colors.text_3),
            ActivityState::Running | ActivityState::Done => (colors.text_3, colors.text_1),
        };
        let group = SharedString::from(format!("activity-row-{id}"));
        let spinner = SharedString::from(format!("activity-row-spin-{id}"));
        let tip = SharedString::from(format!("{verb} {subject}"));
        let interactive = on_click.is_some();
        let row = div().id(id);
        let row = if interactive {
            focusable(row, &theme, false)
        } else {
            row
        };
        let row = on_activate(row, false, on_click);
        let row = with_tooltip(row, &theme, Some(tip))
            .group(group.clone())
            .h(px(a::ROW_H))
            .flex()
            .items_center()
            .gap(px(a::GAP))
            .px(px(METRICS.space_2))
            .rounded(ITEM_RADIUS)
            .hover(move |style| style.bg(colors.ctl_hover))
            .when(interactive, |row| row.cursor_pointer())
            .child(icon(glyph, a::ICON, icon_color))
            .child(
                text_style(div().flex_none(), TextStyle::Label)
                    .text_color(verb_color)
                    .child(verb),
            )
            .child(
                text_style(div().flex_1().min_w_0().truncate(), TextStyle::Label)
                    .text_color(colors.text_2)
                    .child(subject),
            )
            .children(trailing)
            .children(match state {
                ActivityState::Running => Some(Spinner::new(&theme, spinner).into_any_element()),
                ActivityState::Failed => {
                    Some(icon(Icon::X, STATUS_MARK, colors.danger).into_any_element())
                }
                _ => None,
            })
            .children(open.map(|open| {
                icon(
                    if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    a::DISCLOSURE,
                    colors.text_3,
                )
                .invisible()
                .group_hover(group, |style| style.visible())
            }));
        div().w_full().pl(px(a::NESTED_INSET)).child(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_summary_counts_what_the_run_did() {
        let mut counts = ActivityCounts::default();
        for (kind, failed) in [
            (Some("read"), false),
            (Some("read"), false),
            (Some("read"), false),
            (Some("edit"), false),
            (Some("delete"), false),
            (Some("execute"), true),
        ] {
            counts.add_tool(kind, failed);
        }
        let parts = summary_parts(&counts);
        let text: Vec<&str> = parts.iter().map(|(part, _)| part.as_str()).collect();
        assert_eq!(
            text,
            [
                "Read 3 files",
                "edited 2 files",
                "ran 1 command",
                "1 failed"
            ]
        );
        // Only the failure is coloured.
        let failed: Vec<bool> = parts.iter().map(|(_, failed)| *failed).collect();
        assert_eq!(failed, [false, false, false, true]);
    }

    #[test]
    fn a_run_of_one_thought_reads_as_a_word() {
        let counts = ActivityCounts {
            thoughts: 1,
            ..Default::default()
        };
        assert_eq!(summary_parts(&counts), [("Thought".to_owned(), false)]);
        assert!(summary_parts(&ActivityCounts::default()).is_empty());
    }
}
