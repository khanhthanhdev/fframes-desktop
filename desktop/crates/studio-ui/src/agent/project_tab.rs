//! The Changes tab: the review policy (chosen in Settings → Agent), Undo, recovery and the
//! accepted edits, as panel sections.

use super::*;
use crate::kit::{FormField, List, ListRow, PanelSection};

impl ConversationPanel {
    pub(super) fn project_tab(
        &mut self,
        snapshot: Arc<WorkflowSnapshot>,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let mut column = PanelSection::column();

        // Review policy: read here, chosen in Settings → Agent.
        let mut review = PanelSection::new(&theme, "Review")
            .description("Change it in Settings → Agent.")
            .child(label_text(match snapshot.review_policy {
                ReviewPolicy::AutoApply => "Validated changes are applied automatically.",
                ReviewPolicy::ManualReview => "Validated changes wait for your Apply or Discard.",
            }));
        if let Some(task) = snapshot.task.as_ref().filter(|t| !t.phase.is_terminal()) {
            review = review.child(self.line(format!(
                "The running task uses: {}",
                match task.review_policy {
                    ReviewPolicy::AutoApply => "automatic Apply",
                    ReviewPolicy::ManualReview => "manual review",
                }
            )));
        }
        column = column.child(review);

        // Undo and the accepted-awaiting-preview state.
        let (undo_line, undo_enabled) = match &controls.undo {
            UndoControl::Enabled { summary, .. } => (format!("Newest agent edit: {summary}"), true),
            UndoControl::Disabled(reason) => (reason.clone(), false),
        };
        let mut undo = PanelSection::new(&theme, "Undo")
            .child(self.line(undo_line))
            .child(FormField::row(
                &theme,
                "Undo",
                self.action_button(
                    "agent-undo",
                    "Undo last agent edit",
                    undo_enabled,
                    Emphasis::Neutral,
                    cx,
                    |this, _, cx| {
                        if let Some(w) = this.workflow().cloned() {
                            let r = w.undo(None);
                            this.command(r, cx);
                        }
                    },
                ),
            ));
        if let Some(handoff) = &snapshot.handoff {
            undo = undo.child(label_text(match &handoff.state {
                HandoffState::AwaitingPreview { reason } => format!(
                    "Accepted {} is awaiting its preview{}. The preview still shows the previous revision.",
                    &handoff.published[..handoff.published.len().min(12)],
                    reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()
                ),
                HandoffState::Adopted => format!(
                    "Accepted {} · the preview is switching to it.",
                    &handoff.published[..handoff.published.len().min(12)]
                ),
                HandoffState::Displayed => format!(
                    "Accepted {} · displayed in the preview.",
                    &handoff.published[..handoff.published.len().min(12)]
                ),
            }));
        }
        column = column.child(undo);

        // Recovery.
        let recovery = &snapshot.recovery;
        if !recovery.rolled_back.is_empty()
            || !recovery.unresolved_conflicts.is_empty()
            || recovery.suspended.is_some()
            || recovery.draft.as_ref().is_some_and(draft_needs_attention)
            || recovery.notice.is_some()
            || controls.export_draft
        {
            let mut section = PanelSection::new(&theme, "Recovery");
            if let Some(reason) = &recovery.suspended {
                section = section.child(label_text(format!(
                    "Source changes are suspended until the project is reopened: {reason}"
                )));
            }
            if let Some(notice) = &recovery.notice {
                section = section.child(self.line(notice.clone()));
            }
            for id in recovery.rolled_back.iter().take(5) {
                section = section.child(self.line(format!(
                    "An unfinished edit ({}) was rolled back when the project opened; your files are as they were.",
                    &id[..id.len().min(8)]
                )));
            }
            for conflict in &recovery.unresolved_conflicts {
                section = section.child(self.conflict_block(conflict));
            }
            if let Some(draft) = recovery.draft.as_ref().filter(|d| draft_needs_attention(d)) {
                section = section.child(label_text(match &draft.state {
                    _ if draft.interrupted_by_restart => {
                        "A task was interrupted when the app last closed. Its working copy was kept and is locked until you confirm nothing is writing there.".to_owned()
                    }
                    DraftState::UnsafeWriter { reason, .. } => format!(
                        "The agent's working copy is locked because a writer may still run: {reason}"
                    ),
                    DraftState::Retained { reason, .. } => format!(
                        "The last task's working copy was kept ({reason}). It is archived before the next task starts."
                    ),
                    _ => "The agent's working copy is retained.".to_owned(),
                }));
                section = section.child(
                    self.line(
                        studio_bootstrap::external_path(&draft.draft)
                            .display()
                            .to_string(),
                    ),
                );
                if controls.acknowledge_writer_gone {
                    section = section.child(
                        FormField::row(
                            &theme,
                            "Writer",
                            self.action_button(
                                "agent-ack-writer",
                                "Nothing is writing; unlock the working copy",
                                true,
                                Emphasis::Neutral,
                                cx,
                                |this, _, cx| {
                                    if let Some(w) = this.workflow().cloned() {
                                        let r = w.acknowledge_writer_gone();
                                        this.command(r, cx);
                                    }
                                },
                            ),
                        )
                        .hint(
                            "Confirm only if you are sure no agent process is still writing there.",
                        ),
                    );
                }
            }
            if controls.export_draft {
                section = section.child(div().child(self.action_button(
                    "agent-export-draft",
                    "Export the retained draft…",
                    true,
                    Emphasis::Neutral,
                    cx,
                    |this, _, cx| this.export(Export::Draft, cx),
                )));
            }
            column = column.child(section);
        }

        // Accepted edits, as History-style rows.
        let mut rows = Vec::new();
        for entry in snapshot.history.iter().rev().take(10) {
            let mut row = ListRow::new(
                &theme,
                SharedString::from(format!("agent-edit-{}", entry.id)),
            )
            .title(format!(
                "{} · {} file{} · {}{}",
                entry.kind,
                entry.files,
                if entry.files == 1 { "" } else { "s" },
                &entry.published[..entry.published.len().min(12)],
                entry
                    .undoes
                    .as_ref()
                    .map(|u| format!(" · undoes {}", &u[..u.len().min(8)]))
                    .unwrap_or_default()
            ))
            .meta(entry.summary.clone());
            if entry.kind == "Apply" {
                row = row.trailing(self.revision_review_button(&entry.id, cx));
            }
            rows.push(row);
        }
        column = column.child(
            PanelSection::new(
                &theme,
                format!("Accepted agent edits ({})", snapshot.history.len()),
            )
            .child(
                List::new().children(rows).empty(
                    EmptyState::new(&theme, "No accepted edits")
                        .body("No agent edit has been accepted in this project yet."),
                ),
            ),
        );

        // Resources.
        let r = snapshot.resources;
        column = column.child(self.caption(&format!(
            "Resident messages {}/{} · {} KiB of {} KiB · {} processes · {} tool grants · {} tool workers",
            r.resident_rows,
            r.max_resident_rows,
            r.resident_bytes / 1024,
            r.max_resident_bytes / 1024,
            r.owned_processes,
            r.broker_grants,
            r.tool_workers
        )));
        div()
            .id("agent-project-tab")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(METRICS.space_3))
            .child(column)
            .into_any_element()
    }
}
