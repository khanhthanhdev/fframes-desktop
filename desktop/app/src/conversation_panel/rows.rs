//! Pure window, paging and diff logic of the virtualized conversation list.
//!
//! The list element (`gpui::list`) measures and renders only the rows near the viewport
//! (visible rows plus [`OVERDRAW_PX`] of overscan); this module decides *which rows the
//! list owns* and how a new snapshot turns into list mutations:
//!
//! * [`compose_view`] merges rows paged in from the conversation log with the workflow's
//!   live window under the Stage 1 resident limits (400 rows AND 4 MiB). The panel's own
//!   retention (paged history plus everything it lays out, counted by unique allocations
//!   with [`unique_retained`]) never exceeds one such budget however far back the reader
//!   pages. The workflow's live window is a separate allocation set owned and bounded by
//!   the workflow (same limits); the panel shares it by `Arc` and never copies it, so the
//!   whole process holds at most those two bounded sets, reported separately.
//! * [`diff_rows`] turns two ordered row sets into `ListState` operations. A tool card or
//!   a streaming message that was updated under its row id becomes an in-place
//!   `Remeasure` (the scroll anchor is kept); appended and evicted rows become splices
//!   (the list shifts its anchor item by the splice, so a prepended page or an evicted
//!   head does not move what the user is reading).
//! * [`page_request`] decides when to ask the log for an older page.
use crate::agent_workflow::{MAX_HISTORY_PAGE, MAX_RESIDENT_ROW_BYTES, MAX_RESIDENT_ROWS, Row};
use std::sync::Arc;

/// Extra pixels rendered above and below the viewport (the bounded overscan).
pub const OVERDRAW_PX: f32 = 240.;
/// Rows requested per history page (never above the workflow's page limit).
pub const PAGE_ROWS: usize = if MAX_HISTORY_PAGE < 50 {
    MAX_HISTORY_PAGE
} else {
    50
};
/// A page is requested once the viewport's first row is this close to the window's head.
pub const PAGE_TRIGGER_ROWS: usize = 6;

/// Identity of one list item: the persisted row id plus the identity of the shared row
/// value. Equal ids with different versions mean the row was updated in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowKey {
    pub id: u64,
    pub version: usize,
}

impl RowKey {
    pub fn of(row: &Arc<Row>) -> Self {
        Self {
            id: row.id.0,
            version: Arc::as_ptr(row) as usize,
        }
    }
}

/// One mutation of a `gpui::ListState`, applied in order (each index refers to the list
/// after the earlier operations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListOp {
    /// `ListState::splice(at..at + old, new)`.
    Splice { at: usize, old: usize, new: usize },
    /// `ListState::remeasure_items(at..at + len)`: same items, new content.
    Remeasure { at: usize, len: usize },
    /// `ListState::reset(len)`: the ordering invariant broke; start over.
    Reset { len: usize },
}

fn ascending(keys: &[RowKey]) -> bool {
    keys.windows(2).all(|pair| pair[0].id < pair[1].id)
}

/// The operations that turn a list holding `old` into one holding `new`. Both must be
/// strictly ascending by row id; otherwise the result is a single [`ListOp::Reset`].
pub fn diff_rows(old: &[RowKey], new: &[RowKey]) -> Vec<ListOp> {
    if !ascending(old) || !ascending(new) {
        return if old == new {
            Vec::new()
        } else {
            vec![ListOp::Reset { len: new.len() }]
        };
    }
    let mut ops: Vec<ListOp> = Vec::new();
    let (mut i, mut j, mut pos) = (0, 0, 0);
    let remeasure = |ops: &mut Vec<ListOp>, at: usize| {
        if let Some(ListOp::Remeasure { at: start, len }) = ops.last_mut()
            && *start + *len == at
        {
            *len += 1;
            return;
        }
        ops.push(ListOp::Remeasure { at, len: 1 });
    };
    while i < old.len() || j < new.len() {
        match (old.get(i), new.get(j)) {
            (Some(o), Some(n)) if o.id == n.id => {
                if o.version != n.version {
                    remeasure(&mut ops, pos);
                }
                i += 1;
                j += 1;
                pos += 1;
            }
            (Some(o), Some(n)) if o.id < n.id => {
                let mut run = 0;
                while old.get(i + run).is_some_and(|k| k.id < n.id) {
                    run += 1;
                }
                ops.push(ListOp::Splice {
                    at: pos,
                    old: run,
                    new: 0,
                });
                i += run;
            }
            (Some(o), Some(_)) => {
                let mut run = 0;
                while new.get(j + run).is_some_and(|k| k.id < o.id) {
                    run += 1;
                }
                ops.push(ListOp::Splice {
                    at: pos,
                    old: 0,
                    new: run,
                });
                j += run;
                pos += run;
            }
            (Some(_), None) => {
                ops.push(ListOp::Splice {
                    at: pos,
                    old: old.len() - i,
                    new: 0,
                });
                i = old.len();
            }
            (None, Some(_)) => {
                ops.push(ListOp::Splice {
                    at: pos,
                    old: 0,
                    new: new.len() - j,
                });
                j = new.len();
            }
            (None, None) => break,
        }
    }
    ops
}

/// Resident limits of the list's rows (the Stage 1 transcript limits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidentLimits {
    pub rows: usize,
    pub bytes: usize,
}

impl Default for ResidentLimits {
    fn default() -> Self {
        Self {
            rows: MAX_RESIDENT_ROWS,
            bytes: MAX_RESIDENT_ROW_BYTES,
        }
    }
}

/// The rows the list owns right now.
#[derive(Debug, Clone)]
pub struct ComposedView {
    pub rows: Vec<Arc<Row>>,
    /// Live rows that are newer than the last row shown and not resident in the list
    /// (the window is full or paged history is no longer adjacent). "Jump to latest"
    /// clears the paged history and shows the live window again.
    pub hidden_newer: usize,
}

fn resident_bytes(rows: &[Arc<Row>]) -> usize {
    rows.iter().map(|r| r.estimated_bytes()).sum()
}

/// Rows and bytes of the UNIQUE `Arc<Row>` allocations across `sets` (by pointer, so a row
/// held by both the paged history and the composed view counts once). This is what the
/// panel measures against its own budget: the paged history and everything laid out.
pub fn unique_retained(sets: &[&[Arc<Row>]]) -> (usize, usize) {
    let mut seen = std::collections::HashSet::new();
    let mut bytes = 0;
    for row in sets.iter().flat_map(|set| set.iter()) {
        if seen.insert(Arc::as_ptr(row)) {
            bytes += row.estimated_bytes();
        }
    }
    (seen.len(), bytes)
}

/// Paged-in `older` rows followed by the workflow's `live` window, kept inside `limits`.
///
/// Rows that do not fit are dropped from the NEWEST end: the user is reading history, so
/// the top of the window is stable. Live rows that would leave a hole (the live window
/// moved past the last paged row while the user was away) are hidden too, never mixed.
pub fn compose_view(older: &[Arc<Row>], live: &[Arc<Row>], limits: ResidentLimits) -> ComposedView {
    if older.is_empty() {
        return ComposedView {
            rows: live.to_vec(),
            hidden_newer: 0,
        };
    }
    let mut rows: Vec<Arc<Row>> = older.to_vec();
    // Drop paged rows the live window already contains (their live version wins).
    let first_live = live.first().map(|r| r.id.0);
    if let Some(first_live) = first_live {
        while rows.last().is_some_and(|r| r.id.0 >= first_live) {
            rows.pop();
        }
    }
    let adjacent = match (first_live, rows.last()) {
        (Some(first_live), Some(row)) => first_live <= row.id.0 + 1,
        _ => true,
    };
    let mut hidden_newer = 0;
    if adjacent {
        rows.extend(live.iter().cloned());
    } else {
        hidden_newer = live.len();
    }
    let mut bytes = resident_bytes(&rows);
    while rows.len() > limits.rows || bytes > limits.bytes {
        let Some(dropped) = rows.pop() else { break };
        bytes -= dropped.estimated_bytes();
        if live.iter().any(|l| l.id == dropped.id) {
            hidden_newer += 1;
        }
    }
    ComposedView { rows, hidden_newer }
}

/// Adds a freshly loaded `page` (ascending, older than everything in `older`) in front of
/// `older`, then enforces `limits` by dropping the newest paged rows.
pub fn prepend_page(older: &mut Vec<Arc<Row>>, page: Vec<Arc<Row>>, limits: ResidentLimits) {
    let boundary = older.first().map_or(u64::MAX, |r| r.id.0);
    let mut merged: Vec<Arc<Row>> = page.into_iter().filter(|r| r.id.0 < boundary).collect();
    merged.append(older);
    let mut bytes = resident_bytes(&merged);
    while merged.len() > limits.rows || bytes > limits.bytes {
        let Some(dropped) = merged.pop() else { break };
        bytes -= dropped.estimated_bytes();
    }
    *older = merged;
}

/// What the viewport looks at, as the list reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    /// Index of the first row intersecting the viewport.
    pub first_visible: usize,
}

/// The older page to request, if any: `(rows before this id, limit)`.
///
/// `log_has_older` is true while rows older than the head of the window exist in the
/// conversation log (the snapshot's `older_rows` while nothing is paged in; afterwards
/// the previous page's answer).
pub fn page_request(
    view: &[Arc<Row>],
    viewport: Viewport,
    log_has_older: bool,
    loading: bool,
) -> Option<(u64, usize)> {
    if loading || !log_has_older || viewport.first_visible > PAGE_TRIGGER_ROWS {
        return None;
    }
    view.first().map(|row| (row.id.0, PAGE_ROWS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_workflow::{NoticeLevel, RowId, RowKind};

    fn row(id: u64, text: &str) -> Arc<Row> {
        Arc::new(Row {
            id: RowId(id),
            task: None,
            at_unix: 0,
            kind: RowKind::Notice {
                level: NoticeLevel::Info,
                text: text.to_owned(),
            },
        })
    }

    fn rows(ids: std::ops::RangeInclusive<u64>) -> Vec<Arc<Row>> {
        ids.map(|id| row(id, "x")).collect()
    }

    fn keys(rows: &[Arc<Row>]) -> Vec<RowKey> {
        rows.iter().map(RowKey::of).collect()
    }

    /// A model of `gpui::ListState` item bookkeeping: ops applied to ids, with the same
    /// scroll-anchor rule `ListState::splice` documents in its implementation (an anchor
    /// inside the replaced range snaps to the range start; one after it shifts).
    struct SimList {
        ids: Vec<u64>,
        anchor: usize,
    }

    impl SimList {
        fn apply(&mut self, op: &ListOp, new_ids: &[u64]) {
            match *op {
                ListOp::Splice { at, old, new } => {
                    let range = at..at + old;
                    let inserted: Vec<u64> = new_ids[at..at + new].to_vec();
                    self.ids.splice(range.clone(), inserted);
                    if range.contains(&self.anchor) {
                        self.anchor = range.start;
                    } else if range.end <= self.anchor {
                        self.anchor = self.anchor - old + new;
                    }
                }
                ListOp::Remeasure { .. } => {}
                ListOp::Reset { len } => {
                    self.ids = new_ids[..len].to_vec();
                    self.anchor = 0;
                }
            }
        }
    }

    fn replay(old: &[Arc<Row>], new: &[Arc<Row>], anchor: usize) -> (Vec<ListOp>, SimList) {
        let ops = diff_rows(&keys(old), &keys(new));
        let new_ids: Vec<u64> = new.iter().map(|r| r.id.0).collect();
        let mut sim = SimList {
            ids: old.iter().map(|r| r.id.0).collect(),
            anchor,
        };
        // Ops carry positions in the evolving list; inserted ids come from `new` at the
        // same position (the diff walks both lists in step).
        for op in &ops {
            sim.apply(op, &new_ids);
        }
        (ops, sim)
    }

    #[test]
    fn identical_windows_produce_no_operations() {
        let live = rows(1..=5);
        assert!(diff_rows(&keys(&live), &keys(&live)).is_empty());
    }

    #[test]
    fn a_tool_update_remeasures_in_place_and_keeps_the_scroll_anchor() {
        let old = rows(1..=6);
        let mut new = old.clone();
        new[2] = row(3, "tool card, updated");
        new[3] = row(4, "next card, updated");
        let (ops, sim) = replay(&old, &new, 2);
        assert_eq!(ops, vec![ListOp::Remeasure { at: 2, len: 2 }]);
        assert_eq!(sim.ids, vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(sim.anchor, 2, "no splice, the anchor row does not move");
    }

    #[test]
    fn streaming_appends_never_move_the_row_being_read() {
        let old = rows(1..=6);
        let mut new = old.clone();
        new[5] = row(6, "streamed text grew");
        new.extend(rows(7..=9));
        let (ops, sim) = replay(&old, &new, 1);
        assert_eq!(
            ops,
            vec![
                ListOp::Remeasure { at: 5, len: 1 },
                ListOp::Splice {
                    at: 6,
                    old: 0,
                    new: 3
                }
            ]
        );
        assert_eq!(sim.ids.len(), 9);
        assert_eq!(sim.ids[sim.anchor], 2, "still reading row 2");
    }

    #[test]
    fn a_prepended_page_shifts_the_anchor_so_the_same_row_stays_in_view() {
        let live = rows(11..=20);
        let mut all = rows(1..=10);
        all.extend(live.clone());
        let (ops, sim) = replay(&live, &all, 3);
        assert_eq!(
            ops,
            vec![ListOp::Splice {
                at: 0,
                old: 0,
                new: 10
            }]
        );
        assert_eq!(sim.ids[sim.anchor], 14, "row 14 was the first visible row");
    }

    #[test]
    fn head_eviction_while_streaming_keeps_the_reader_on_the_same_row() {
        let old = rows(1..=10);
        let new: Vec<Arc<Row>> = old[3..].iter().cloned().chain(rows(11..=12)).collect();
        let (ops, sim) = replay(&old, &new, 6);
        assert_eq!(
            ops,
            vec![
                ListOp::Splice {
                    at: 0,
                    old: 3,
                    new: 0
                },
                ListOp::Splice {
                    at: 7,
                    old: 0,
                    new: 2
                }
            ]
        );
        assert_eq!(sim.ids[sim.anchor], 7);
        assert_eq!(
            sim.ids,
            new.iter().map(|r| r.id.0).collect::<Vec<_>>(),
            "the list ends up with exactly the new rows"
        );
    }

    #[test]
    fn an_anchor_inside_an_evicted_range_snaps_to_the_range_start() {
        let old = rows(1..=10);
        let new: Vec<Arc<Row>> = old[5..].to_vec();
        let (_, sim) = replay(&old, &new, 2);
        assert_eq!(sim.anchor, 0);
        assert_eq!(sim.ids[sim.anchor], 6, "the oldest surviving row");
    }

    #[test]
    fn a_broken_ordering_resets_the_list_instead_of_guessing() {
        let old = rows(1..=3);
        let mut new = rows(1..=3);
        new.swap(0, 2);
        assert_eq!(
            diff_rows(&keys(&old), &keys(&new)),
            vec![ListOp::Reset { len: 3 }]
        );
    }

    #[test]
    fn every_diff_replays_to_exactly_the_new_rows() {
        // Exhaustive over small windows: any old/new subset pair of ids 1..=7 with some
        // rows updated in place replays to the new id list.
        for old_mask in 0u32..128 {
            for new_mask in 0u32..128 {
                let pick = |mask: u32, version: &str| -> Vec<Arc<Row>> {
                    (1..=7u64)
                        .filter(|id| mask & (1 << (id - 1)) != 0)
                        .map(|id| row(id, version))
                        .collect()
                };
                let old = pick(old_mask, "a");
                let new = pick(new_mask, "b");
                let (_, sim) = replay(&old, &new, 0);
                assert_eq!(
                    sim.ids,
                    new.iter().map(|r| r.id.0).collect::<Vec<_>>(),
                    "old {old_mask:07b} new {new_mask:07b}"
                );
            }
        }
    }

    #[test]
    fn composing_without_paged_history_is_the_live_window() {
        let live = rows(5..=9);
        let view = compose_view(&[], &live, ResidentLimits::default());
        assert_eq!(view.rows.len(), 5);
        assert_eq!(view.hidden_newer, 0);
    }

    #[test]
    fn paged_history_is_adjacent_to_the_live_window() {
        let older = rows(1..=4);
        let live = rows(5..=8);
        let view = compose_view(&older, &live, ResidentLimits::default());
        assert_eq!(
            view.rows.iter().map(|r| r.id.0).collect::<Vec<_>>(),
            (1..=8).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_resident_row_limit_hides_the_newest_rows_and_counts_them() {
        let older = rows(1..=6);
        let live = rows(7..=12);
        let limits = ResidentLimits {
            rows: 9,
            bytes: usize::MAX,
        };
        let view = compose_view(&older, &live, limits);
        assert_eq!(view.rows.len(), 9);
        assert_eq!(view.rows.last().unwrap().id.0, 9);
        assert_eq!(view.hidden_newer, 3);
    }

    #[test]
    fn the_resident_byte_limit_applies_with_the_row_overhead() {
        let big = "y".repeat(1000);
        let older: Vec<Arc<Row>> = (1..=3).map(|id| row(id, &big)).collect();
        let live: Vec<Arc<Row>> = (4..=6).map(|id| row(id, &big)).collect();
        let one = older[0].estimated_bytes();
        let limits = ResidentLimits {
            rows: 100,
            bytes: one * 4 + 1,
        };
        let view = compose_view(&older, &live, limits);
        assert_eq!(view.rows.len(), 4);
        assert_eq!(view.hidden_newer, 2);
        assert!(resident_bytes(&view.rows) <= limits.bytes);
    }

    #[test]
    fn a_live_window_that_moved_past_the_paged_rows_is_hidden_not_mixed() {
        let older = rows(1..=4);
        let live = rows(50..=60);
        let view = compose_view(&older, &live, ResidentLimits::default());
        assert_eq!(view.rows.len(), 4, "no hole in the middle of the list");
        assert_eq!(view.hidden_newer, 11);
    }

    #[test]
    fn a_paged_row_the_live_window_now_holds_uses_the_live_version() {
        let older = rows(1..=6);
        let mut live = rows(5..=8);
        live[0] = row(5, "live version");
        let view = compose_view(&older, &live, ResidentLimits::default());
        assert_eq!(
            view.rows.iter().map(|r| r.id.0).collect::<Vec<_>>(),
            (1..=8).collect::<Vec<_>>()
        );
        assert!(Arc::ptr_eq(&view.rows[4], &live[0]));
    }

    #[test]
    fn paging_older_stays_inside_the_resident_limits() {
        let limits = ResidentLimits {
            rows: 8,
            bytes: usize::MAX,
        };
        let mut older = rows(20..=25);
        prepend_page(&mut older, rows(10..=19), limits);
        assert_eq!(older.len(), 8);
        assert_eq!(older.first().unwrap().id.0, 10);
        assert_eq!(older.last().unwrap().id.0, 17, "newest paged rows dropped");
    }

    fn heavy(id: u64) -> Arc<Row> {
        row(id, &"x".repeat(2000))
    }

    #[test]
    fn what_the_panel_retains_is_counted_by_unique_allocations_and_stays_inside_the_budget() {
        let limits = ResidentLimits {
            rows: 40,
            bytes: 40 * 1024,
        };
        let live: Vec<Arc<Row>> = (1000..1030).map(heavy).collect();
        let mut older: Vec<Arc<Row>> = Vec::new();
        let mut view = compose_view(&older, &live, limits);
        // The live window is shared with the composed view: one allocation each.
        assert_eq!(
            unique_retained(&[&older, &view.rows]).0,
            live.len(),
            "shared rows count once"
        );
        // Page "far back" many times: neither the paged history nor what is laid out may
        // ever exceed the budget, measured by unique allocations (not only the view).
        let mut next_id = 999;
        for _ in 0..30 {
            let page: Vec<Arc<Row>> = (next_id - 24..=next_id).map(heavy).collect();
            next_id -= 25;
            prepend_page(&mut older, page, limits);
            view = compose_view(&older, &live, limits);
            let (rows_held, bytes_held) = unique_retained(&[&older, &view.rows]);
            assert!(rows_held <= limits.rows, "{rows_held} rows");
            assert!(bytes_held <= limits.bytes, "{bytes_held} bytes");
            let (older_rows, older_bytes) = unique_retained(&[&older]);
            assert!(older_rows <= limits.rows && older_bytes <= limits.bytes);
        }
        // Far back, the newest rows are what gets hidden, never the reader's position.
        assert!(view.hidden_newer > 0);
    }

    #[test]
    fn a_page_overlapping_the_window_is_not_duplicated() {
        let mut older = rows(20..=22);
        prepend_page(&mut older, rows(18..=21), ResidentLimits::default());
        assert_eq!(
            older.iter().map(|r| r.id.0).collect::<Vec<_>>(),
            vec![18, 19, 20, 21, 22]
        );
    }

    #[test]
    fn a_page_is_requested_only_near_the_head_and_never_twice() {
        let view = rows(30..=60);
        let near = Viewport { first_visible: 2 };
        let far = Viewport { first_visible: 20 };
        assert_eq!(
            page_request(&view, near, true, false),
            Some((30, PAGE_ROWS))
        );
        assert_eq!(page_request(&view, far, true, false), None);
        assert_eq!(page_request(&view, near, true, true), None, "one in flight");
        assert_eq!(
            page_request(&view, near, false, false),
            None,
            "log exhausted"
        );
        assert_eq!(page_request(&[], near, true, false), None);
    }

    #[test]
    fn a_page_never_exceeds_what_the_workflow_serves() {
        const { assert!(PAGE_ROWS <= MAX_HISTORY_PAGE) };
        const { assert!(PAGE_ROWS > 0) };
    }
}
