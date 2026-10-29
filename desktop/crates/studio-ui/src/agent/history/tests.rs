//! The history list in a test window: rows, filtering and the events it emits.

use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

use super::*;
use crate::kit::bind_text_input_keys;

type Events = std::rc::Rc<std::cell::RefCell<Vec<HistoryEvent>>>;

const ROWS: [&str; 5] = [
    "history-row-0",
    "history-row-1",
    "history-row-2",
    "history-row-3",
    "history-row-4",
];
const NOW: u64 = 1_000_000;

fn entry(id: &str, title: &str, prompt: &str, age_secs: u64) -> SessionEntry {
    SessionEntry {
        prompt_text: prompt.into(),
        updated_unix: NOW - age_secs,
        ..SessionEntry::new(id, title)
    }
}

/// Newest first, as the engine lists them.
fn sample() -> Vec<SessionEntry> {
    vec![
        entry(
            "c",
            "Fix the outro audio",
            "the outro is quiet, add the logo",
            60,
        ),
        SessionEntry {
            agent: Some("codex".into()),
            outcome: Some(OutcomeKind::Accepted),
            ..entry("b", "Logo animation", "animate the logo in", 7_200)
        },
        SessionEntry {
            current: true,
            ..entry("a", "Colour pass", "warmer colours", 200_000)
        },
    ]
}

fn open(
    cx: &mut TestAppContext,
    entries: Vec<SessionEntry>,
) -> (Entity<HistoryList>, &mut VisualTestContext, Events) {
    cx.update(bind_text_input_keys);
    let (list, cx) = cx.add_window_view(HistoryList::new);
    let events = Events::default();
    let sink = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&list, move |_, event: &HistoryEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach()
    });
    list.update(cx, |list, cx| list.set_entries(entries, NOW, cx));
    cx.run_until_parked();
    (list, cx, events)
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} was not laid out"));
    cx.simulate_click(bounds.center(), Modifiers::none());
    cx.run_until_parked();
}

fn type_query(list: &Entity<HistoryList>, text: &str, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let focus = list.read(cx).query_input.read(cx).focus_handle();
        window.focus(&focus, cx);
    });
    cx.simulate_input(text);
    cx.run_until_parked();
}

fn shown_ids(list: &Entity<HistoryList>, cx: &mut VisualTestContext) -> Vec<String> {
    list.read_with(cx, |list, _| {
        list.shown().iter().map(|e| e.id.clone()).collect()
    })
}

fn rows_on_screen(cx: &mut VisualTestContext) -> usize {
    ROWS.iter()
        .filter(|name| cx.debug_bounds(name).is_some())
        .count()
}

#[test]
fn times_and_outcomes_have_labels() {
    assert_eq!(relative_time(NOW, 0), "");
    assert_eq!(relative_time(NOW, NOW - 5), "just now");
    assert_eq!(relative_time(NOW, NOW - 300), "5m ago");
    assert_eq!(relative_time(NOW, NOW - 7_200), "2h ago");
    assert_eq!(relative_time(NOW, NOW - 3 * 86_400), "3d ago");
    assert_eq!(relative_time(40 * 86_400, 1), "1mo ago");
    assert_eq!(outcome_chip(OutcomeKind::Failed), ("Failed", Tone::Danger));
    assert_eq!(
        outcome_chip(OutcomeKind::AwaitingReview),
        ("In review", Tone::Warning)
    );
}

#[gpui::test]
fn every_session_is_a_row_in_the_given_order(cx: &mut TestAppContext) {
    let (list, cx, _) = open(cx, sample());
    assert_eq!(shown_ids(&list, cx), ["c", "b", "a"]);
    assert_eq!(rows_on_screen(cx), 3);
    assert!(cx.debug_bounds("history-search").is_some());
    assert!(cx.debug_bounds("history-empty").is_none());
}

#[gpui::test]
fn an_empty_list_says_so(cx: &mut TestAppContext) {
    let (_, cx, _) = open(cx, Vec::new());
    assert_eq!(rows_on_screen(cx), 0);
    assert!(cx.debug_bounds("history-empty").is_some());
    // New session stays available.
    assert!(cx.debug_bounds("history-new").is_some());
}

#[gpui::test]
fn clicking_a_row_opens_that_session(cx: &mut TestAppContext) {
    let (_, cx, events) = open(cx, sample());
    click(cx, "history-row-1");
    click(cx, "history-row-0");
    assert_eq!(
        *events.borrow(),
        [
            HistoryEvent::Open("b".into()),
            HistoryEvent::Open("c".into())
        ]
    );
}

#[gpui::test]
fn the_new_session_button_emits_new_session(cx: &mut TestAppContext) {
    let (_, cx, events) = open(cx, sample());
    click(cx, "history-new");
    assert_eq!(*events.borrow(), [HistoryEvent::NewSession]);
}

#[gpui::test]
fn typing_filters_and_ranks_a_title_match_first(cx: &mut TestAppContext) {
    let (list, cx, events) = open(cx, sample());
    // "c" mentions the logo only in its prompt, "b" has it in the title.
    type_query(&list, "logo", cx);
    assert_eq!(shown_ids(&list, cx), ["b", "c"]);
    assert_eq!(rows_on_screen(cx), 2);

    // The first shown row is the best match: clicking it opens "b".
    click(cx, "history-row-0");
    assert_eq!(*events.borrow(), [HistoryEvent::Open("b".into())]);

    // A word of a prompt finds its session; every word must match.
    list.update(cx, |list, cx| list.set_query("warmer", cx));
    cx.run_until_parked();
    assert_eq!(shown_ids(&list, cx), ["a"]);
    list.update(cx, |list, cx| list.set_query("warmer logo", cx));
    cx.run_until_parked();
    assert!(shown_ids(&list, cx).is_empty());
    assert_eq!(rows_on_screen(cx), 0);
    assert!(cx.debug_bounds("history-empty").is_some());

    // Clearing the query brings every session back.
    list.update(cx, |list, cx| list.set_query("", cx));
    cx.run_until_parked();
    assert_eq!(shown_ids(&list, cx), ["c", "b", "a"]);
}

#[gpui::test]
fn return_opens_the_best_match_and_escape_clears_the_query(cx: &mut TestAppContext) {
    let (list, cx, events) = open(cx, sample());
    type_query(&list, "logo", cx);
    cx.simulate_keystrokes("enter");
    assert_eq!(*events.borrow(), [HistoryEvent::Open("b".into())]);

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert_eq!(list.read_with(cx, |list, cx| list.query(cx)), "");
    assert_eq!(shown_ids(&list, cx), ["c", "b", "a"]);
}

#[gpui::test]
fn new_entries_keep_the_query(cx: &mut TestAppContext) {
    let (list, cx, _) = open(cx, sample());
    type_query(&list, "colour", cx);
    assert_eq!(shown_ids(&list, cx), ["a"]);

    let mut more = sample();
    more.insert(0, entry("d", "Another colour fix", "", 10));
    list.update(cx, |list, cx| list.set_entries(more, NOW, cx));
    cx.run_until_parked();
    assert_eq!(shown_ids(&list, cx), ["d", "a"]);
}

#[test]
fn a_summary_converts_to_an_entry() {
    let summary = SessionSummary {
        id: "s-1".into(),
        title: "Title".into(),
        agent: Some("claude".into()),
        created_unix: 1,
        updated_unix: 5,
        outcome: Some(OutcomeKind::Failed),
        prompt_text: "prompt".into(),
        rows: 3,
        is_current: true,
        resumable: false,
        unreadable: true,
    };
    let entry = SessionEntry::from(&summary);
    assert_eq!(entry.id, "s-1");
    assert_eq!(entry.agent.as_deref(), Some("claude"));
    assert_eq!(entry.updated_unix, 5);
    assert_eq!(entry.outcome, Some(OutcomeKind::Failed));
    assert!(entry.current);
    assert!(entry.unreadable);
}

#[gpui::test]
fn return_with_an_empty_query_opens_nothing(cx: &mut TestAppContext) {
    let (list, cx, events) = open(cx, sample());
    type_query(&list, "  ", cx);
    cx.simulate_keystrokes("enter");
    assert!(events.borrow().is_empty());
}

#[gpui::test]
fn the_outcome_label_is_searchable(cx: &mut TestAppContext) {
    let mut entries = sample();
    entries[0].outcome = Some(OutcomeKind::Failed);
    let (list, cx, _) = open(cx, entries);
    type_query(&list, "failed", cx);
    assert_eq!(shown_ids(&list, cx), ["c"]);
    list.update(cx, |list, cx| list.set_query("accepted", cx));
    cx.run_until_parked();
    assert_eq!(shown_ids(&list, cx), ["b"]);
}

#[gpui::test]
fn open_id_targets_the_session_not_the_position(cx: &mut TestAppContext) {
    let (list, cx, events) = open(cx, sample());
    list.update(cx, |list, cx| {
        list.open_id("a", cx);
        list.open_id("gone", cx);
    });
    assert_eq!(*events.borrow(), [HistoryEvent::Open("a".into())]);
}
