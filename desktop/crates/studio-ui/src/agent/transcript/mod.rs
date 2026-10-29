//! The transcript list: composing the view from the snapshot and paged-in history, paging and jumping to the latest row.

pub mod rows;
pub mod scroll;

use super::*;

impl ConversationPanel {
    pub(super) fn recompose(&mut self, _cx: &mut Context<Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let view = rows::compose_view(&self.older, &snapshot.rows, ResidentLimits::default());
        let keys: Vec<RowKey> = view.rows.iter().map(RowKey::of).collect();
        // Decided before the rows change: where the reader is now, not where the new
        // rows would leave them.
        let stick = keys != self.view_keys
            && scroll::sticks(
                self.list.is_following_tail(),
                scroll::distance_from_bottom(&self.list),
            );
        for op in rows::diff_rows(&self.view_keys, &keys) {
            match op {
                ListOp::Splice { at, old, new } => self.list.splice(at..at + old, new),
                ListOp::Remeasure { at, len } => self.list.remeasure_items(at..at + len),
                ListOp::Reset { len } => self.list.reset(len),
            }
        }
        if stick && !self.list.is_following_tail() {
            self.list.set_follow_mode(FollowMode::Tail);
        }
        self.view_rows = view.rows;
        self.view_keys = keys;
        self.sync_activity();
        self.hidden_newer = view.hidden_newer;
    }

    pub(super) fn log_has_older(&self) -> bool {
        if self.older.is_empty() {
            self.snapshot.as_ref().is_some_and(|s| s.older_rows) && !self.older_exhausted
        } else {
            !self.older_exhausted
        }
    }

    pub(super) fn maybe_request_page(&mut self) {
        let Some(first_visible) = self.viewport.get() else {
            return;
        };
        if let Some((before, limit)) = rows::page_request(
            &self.view_rows,
            Viewport { first_visible },
            self.log_has_older(),
            self.page.loading.load(Ordering::Acquire),
        ) {
            self.request_page(before, limit);
        }
    }

    pub(super) fn request_page(&mut self, before: u64, limit: usize) {
        let Some(attached) = &self.attached else {
            return;
        };
        if self.page.loading.swap(true, Ordering::AcqRel) {
            return;
        }
        let slot = self.page.clone();
        let serial = attached.serial;
        let chat = self.chat;
        let started = attached.workflow.history_page_with(
            studio_engine::agent::RowId(before),
            limit,
            move |result| {
                *slot.done.lock() = Some((serial, chat, result.map_err(|e| e.to_string())));
                if let Some(wake) = slot.wake.lock().clone() {
                    wake();
                }
            },
        );
        if let Err(error) = started {
            self.page.loading.store(false, Ordering::Release);
            self.notice = Some(error.to_string());
        }
    }

    pub(super) fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        self.older.clear();
        self.older_exhausted = false;
        self.recompose(cx);
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        cx.notify();
    }
}
