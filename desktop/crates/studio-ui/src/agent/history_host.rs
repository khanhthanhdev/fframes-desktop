//! The History tab: the stored sessions of the open project, listed off the UI thread,
//! and what picking one does.

use std::time::{SystemTime, UNIX_EPOCH};

use studio_engine::agent::session_store::history::SessionSummary;

use super::history::{HistoryEvent, SessionEntry};
use super::*;

impl ConversationPanel {
    /// Reads the session list on a background thread and shows the newest answer.
    pub(super) fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(workflow) = self.workflow().cloned() else {
            return;
        };
        self.host.sessions_serial += 1;
        let serial = self.host.sessions_serial;
        let listing = cx
            .background_executor()
            .spawn(async move { workflow.sessions() });
        cx.spawn(async move |this, cx| {
            let result = listing.await;
            let _ = this.update(cx, |panel, cx| {
                if panel.host.sessions_serial == serial {
                    panel.sessions_loaded(result, cx);
                }
            });
        })
        .detach();
    }

    fn sessions_loaded(
        &mut self,
        result: Result<Vec<SessionSummary>, studio_engine::agent::WorkflowError>,
        cx: &mut Context<Self>,
    ) {
        let Some(summaries) = self.command(result, cx) else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let entries = summaries.iter().map(SessionEntry::from).collect();
        self.host
            .history
            .update(cx, |history, cx| history.set_entries(entries, now, cx));
    }

    /// Opening or starting a session returns to the chat with the composer focused: the
    /// history row or button that was clicked is gone from the tree, and keys would
    /// otherwise have nowhere to go.
    pub(super) fn on_history_event(
        &mut self,
        event: HistoryEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workflow) = self.workflow().cloned() else {
            return;
        };
        let result = match event {
            HistoryEvent::Open(id) => workflow.open_session(&id),
            HistoryEvent::NewSession => workflow.new_session(),
        };
        if self.command(result, cx).is_some() {
            self.tab = Tab::Chat;
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    pub(super) fn history_tab(&self) -> AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .p(px(METRICS.space_3))
            .child(self.host.history.clone())
            .into_any_element()
    }
}
