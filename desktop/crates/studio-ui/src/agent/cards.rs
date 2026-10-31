//! Transcript rows: the two voices, runs of activity, permissions, errors, validation,
//! changes and outcomes, built from the chat components in [`super::ui`].

use studio_harness::driver::PermissionChoice;

use super::markdown::{MarkdownRow, Voice};
use super::ui::metrics::{activity as activity_metrics, transcript as transcript_metrics};
use super::ui::{
    ActionCard, ActivityCounts, ActivityHeader, ActivityRow, ActivityState, AssistantMessage,
    ChangeMark, ChangedFile, ChangesCard, ROW_H, UserMessage, outcome_style, summary_parts,
};
use super::*;

/// A run of consecutive tool calls, reasoning and permission notes in the shown rows: one
/// summary line that opens into a row per step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ActivitySpan {
    /// Index of the first row in `view_rows`, and one past the last.
    pub start: usize,
    pub end: usize,
    /// The first row's id: the run's identity while rows are added to it.
    pub key: u64,
    pub open: bool,
}

/// Whether a row belongs to a run of activity.
fn is_activity(kind: &RowKind) -> bool {
    matches!(
        kind,
        RowKind::Tool(_) | RowKind::Thought { .. } | RowKind::Permission(_)
    )
}

/// Whether a step of a run is still going: then the run opens.
fn is_live(kind: &RowKind) -> bool {
    match kind {
        RowKind::Tool(card) => matches!(
            card.status,
            Some(ToolStatus::Pending | ToolStatus::InProgress)
        ),
        RowKind::Thought { streaming, .. } => *streaming,
        RowKind::Permission(card) => card.state == PermissionState::Open,
        _ => false,
    }
}

/// The runs of activity in `rows`. A run is open while a step in it is live, unless the
/// reader opened or closed it.
pub(super) fn activity_spans(
    rows: &[Arc<Row>],
    overrides: &HashMap<u64, bool>,
) -> Vec<ActivitySpan> {
    let mut spans = Vec::new();
    let mut ix = 0;
    while ix < rows.len() {
        if !is_activity(&rows[ix].kind) {
            ix += 1;
            continue;
        }
        let start = ix;
        while ix < rows.len() && is_activity(&rows[ix].kind) {
            ix += 1;
        }
        let key = rows[start].id.0;
        let live = rows[start..ix].iter().any(|row| is_live(&row.kind));
        spans.push(ActivitySpan {
            start,
            end: ix,
            key,
            open: overrides.get(&key).copied().unwrap_or(live),
        });
    }
    spans
}

impl ConversationPanel {
    pub(super) fn row_id(row: &Row) -> ElementId {
        ElementId::from(("agent-row", row.id.0))
    }

    /// The run of activity row `ix` belongs to.
    fn span_of(&self, ix: usize) -> Option<ActivitySpan> {
        self.activity
            .iter()
            .find(|span| span.start <= ix && ix < span.end)
            .copied()
    }

    /// Recomputes the runs of activity after the rows or an open state changed and
    /// re-measures every run whose rows or state are not what the list measured.
    pub(super) fn sync_activity(&mut self) {
        let spans = activity_spans(&self.view_rows, &self.activity_open);
        for span in &spans {
            if !self.activity.contains(span) {
                self.list.remeasure_items(span.start..span.end);
            }
        }
        self.activity = spans;
    }

    pub(super) fn toggle_activity(&mut self, key: u64, open: bool, cx: &mut Context<Self>) {
        self.activity_open.insert(key, open);
        self.sync_activity();
        cx.notify();
    }

    /// When the row was written, for the hover lane.
    fn row_time(&self, row: &Row) -> Option<SharedString> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let time = history::relative_time(now, row.at_unix);
        (!time.is_empty()).then(|| time.into())
    }

    pub(super) fn row_element(
        &mut self,
        ix: usize,
        row: &Arc<Row>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        if let Some(span) = self.span_of(ix) {
            return self.activity_element(ix, row, span, cx);
        }
        let entity = cx.entity();
        let element = match &row.kind {
            // The user's messages are bubbles on the right; the agent answers on the left
            // without a frame, and its tool calls run as quiet lines between them.
            RowKind::User { text, source } => {
                let id = row.id.0;
                let mut message = UserMessage::new(&theme, id, text.clone()).expanded(
                    self.messages_open.contains(&id),
                    move |_, cx| {
                        entity.update(cx, |this, cx| this.toggle_message(id, cx));
                    },
                );
                if let Some(time) = self.row_time(row) {
                    message = message.meta(time);
                }
                match source {
                    UserSource::Brief => message,
                    UserSource::Clarification => message.caption("Your answer"),
                    UserSource::Followup => message.caption("Follow-up"),
                    UserSource::Repair => {
                        message.caption("Automatic repair context (sent by Studio)")
                    }
                }
                .into_any_element()
            }
            RowKind::Agent {
                text, streaming, ..
            } => {
                let mut message = AssistantMessage::new(
                    &theme,
                    row.id.0,
                    MarkdownRow::new(row, Voice::Answer).project_root(self.project_root.clone()),
                );
                if !streaming {
                    message = message.copy(text.clone());
                    if let Some(time) = self.row_time(row) {
                        message = message.meta(time);
                    }
                }
                message.into_any_element()
            }
            RowKind::Notice { level, text } => Notice::new(
                Self::row_id(row),
                match level {
                    NoticeLevel::Info => UiTone::Info,
                    NoticeLevel::Warning => UiTone::Warning,
                },
                text.clone(),
            )
            .into_any_element(),
            RowKind::Error(error) => self.error_card(row, error),
            RowKind::Validation(card) => self.validation_card(row, card),
            RowKind::Changes(card) => self.changes_card(card, cx),
            RowKind::Outcome(card) => self.outcome_line(row, card),
            // Activity rows are drawn by their run above.
            RowKind::Tool(_) | RowKind::Thought { .. } | RowKind::Permission(_) => {
                div().into_any_element()
            }
        };
        Self::row_shell().child(element).into_any_element()
    }

    /// Shows all or the first lines of a long user message, re-measuring that row only.
    fn toggle_message(&mut self, id: u64, cx: &mut Context<Self>) {
        if !self.messages_open.remove(&id) {
            self.messages_open.insert(id);
        }
        if let Some(ix) = self.view_rows.iter().position(|row| row.id.0 == id) {
            self.list.remeasure_items(ix..ix + 1);
        }
        cx.notify();
    }

    /// One row of a run of activity. The run's first row carries the summary line; a
    /// closed run's other rows take no space.
    fn activity_element(
        &mut self,
        ix: usize,
        row: &Arc<Row>,
        span: ActivitySpan,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let first = ix == span.start;
        let last = ix + 1 == span.end;
        let mut element = div().w_full().flex().flex_col();
        if first {
            let mut counts = ActivityCounts::default();
            for member in &self.view_rows[span.start..span.end] {
                match &member.kind {
                    RowKind::Tool(card) => counts.add_tool(
                        card.kind.as_deref(),
                        card.status == Some(ToolStatus::Failed),
                    ),
                    RowKind::Thought { .. } => counts.thoughts += 1,
                    RowKind::Permission(_) => counts.permissions += 1,
                    _ => (),
                }
            }
            let entity = cx.entity();
            let (key, open) = (span.key, span.open);
            // No spinner on the summary: the working trailer under the last row is the
            // turn's one progress cue, and a streaming thought shows its "Thinking…" badge.
            element = element.child(
                ActivityHeader::new(&theme, ("agent-activity", key), summary_parts(&counts))
                    .open(open)
                    .on_toggle(move |_, cx| {
                        entity.update(cx, |this, cx| this.toggle_activity(key, !open, cx));
                    }),
            );
        }
        if span.open {
            element = element.child(self.activity_row(row, cx));
        }
        if last || (first && !span.open) {
            element = element.pb(px(transcript_metrics::ROW_GAP));
        }
        element.into_any_element()
    }

    /// The row of one step in an open run.
    fn activity_row(&mut self, row: &Arc<Row>, cx: &mut Context<Self>) -> AnyElement {
        match &row.kind {
            RowKind::Tool(card) => self.tool_card(row, card).into_any_element(),
            RowKind::Permission(card) => self.permission_card(card).into_any_element(),
            RowKind::Thought {
                text, streaming, ..
            } => self.thought_row(row, text, *streaming, cx),
            _ => div().into_any_element(),
        }
    }

    /// Reasoning as a step: "Thought" and its first line; the text shows while it streams or
    /// while the reader keeps it open.
    fn thought_row(
        &self,
        row: &Arc<Row>,
        text: &str,
        streaming: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = row.id.0;
        let open = streaming || self.thoughts_open.contains(&id);
        let entity = cx.entity();
        let first_line = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("")
            .to_owned();
        let mut step = ActivityRow::new(
            &self.theme,
            ("agent-thought", id),
            Icon::Brain,
            "Thought",
            first_line,
        );
        if streaming {
            // The "Thinking…" badge on the fixed-height row, not a spinner beside the
            // working trailer's.
            step = step.trailing(Badge::new("Thinking…"));
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(step.on_toggle(open, move |_, cx| {
                entity.update(cx, |this, cx| {
                    this.toggle_thought(id, !open);
                    cx.notify();
                });
            }))
            .children(open.then(|| {
                // The rule runs under the step's icon: the step's inset plus its own padding.
                div()
                    .pl(px(activity_metrics::NESTED_INSET + METRICS.space_2))
                    .pr(px(METRICS.space_2))
                    .pb(px(METRICS.space_1))
                    .child(
                        MarkdownRow::new(row, Voice::Reasoning)
                            .project_root(self.project_root.clone()),
                    )
            }))
            .into_any_element()
    }

    /// Records the expansion of a reasoning row and re-measures that row only.
    pub(super) fn toggle_thought(&mut self, id: u64, open: bool) {
        if open {
            self.thoughts_open.insert(id);
        } else {
            self.thoughts_open.remove(&id);
        }
        if let Some(ix) = self.view_rows.iter().position(|row| row.id.0 == id) {
            self.list.remeasure_items(ix..ix + 1);
        }
    }

    /// A tool call as one step: what kind, on what, and how it went. The full title is in
    /// the tooltip when the line truncates it.
    pub(super) fn tool_card(&self, row: &Row, card: &ToolCard) -> ActivityRow {
        let (verb, glyph) = tool_verb(card.kind.as_deref());
        let title = card.title.clone().unwrap_or_else(|| card.call_id.clone());
        // Agents usually start the title with the verb ("Read src/main.rs"): then the
        // title's own first word is the verb, not a repeat of it.
        let (verb, title) = match title.split_once(' ') {
            Some((first, rest)) if first.eq_ignore_ascii_case(verb) => {
                (first.to_owned(), rest.to_owned())
            }
            _ => (verb.to_owned(), title),
        };
        ActivityRow::new(&self.theme, ("agent-tool", row.id.0), glyph, verb, title)
            .state(tool_state(card.status))
    }

    /// Whether `card` is an agent question, which has its own card above the composer.
    pub(super) fn is_question(&self, card: &PermissionCard) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .composer
                .questions
                .iter()
                .any(|question| question.reference == card.reference)
        })
    }

    fn permission_key(card: &PermissionCard) -> String {
        format!(
            "{}-{}-{}",
            &card.reference.task.0[..card.reference.task.0.len().min(8)],
            card.reference.writer,
            card.reference.request
        )
    }

    /// The step a permission request leaves in the transcript. An open request is answered
    /// in its card above the composer ([`Self::permission_request`]); the row only records it.
    pub(super) fn permission_card(&self, card: &PermissionCard) -> ActivityRow {
        let theme = self.theme;
        let key = Self::permission_key(card);
        if let Some(policy) = card.answered_by {
            let state = if card.state == PermissionState::Cancelled {
                "closed before the agent took it"
            } else if card.remember.is_some() {
                "saved for this project"
            } else {
                policy.label()
            };
            return ActivityRow::new(
                &theme,
                SharedString::from(format!("perm-auto-{key}")),
                Icon::CircleCheck,
                "Allowed:",
                card.title.clone(),
            )
            .trailing(self.caption(state));
        }
        match &card.state {
            PermissionState::Open if self.is_question(card) => ActivityRow::new(
                &theme,
                SharedString::from(format!("perm-notice-{key}")),
                Icon::Info,
                "Question:",
                card.title.clone(),
            )
            .state(ActivityState::Waiting)
            .trailing(self.caption("answer below")),
            // The pinned request shows the command; the row stays one line until answered.
            PermissionState::Open => ActivityRow::new(
                &theme,
                SharedString::from(format!("perm-open-{key}")),
                Icon::AlertTriangle,
                "Waiting for you:",
                card.title.clone(),
            )
            .state(ActivityState::Waiting),
            PermissionState::Selected { option_id } => ActivityRow::new(
                &theme,
                SharedString::from(format!("perm-notice-{key}")),
                Icon::CircleCheck,
                format!(
                    "You chose {}:",
                    card.options
                        .iter()
                        .find(|o| &o.option_id == option_id)
                        .map_or(option_id.as_str(), |o| o.name.as_str())
                ),
                card.title.clone(),
            ),
            PermissionState::Cancelled => ActivityRow::new(
                &theme,
                SharedString::from(format!("perm-notice-{key}")),
                Icon::X,
                "Closed without an answer:",
                card.title.clone(),
            ),
        }
    }

    /// The open permission request pinned above the composer, with the agent's options.
    /// Decline (closes the request and ends the turn) is offered only when the agent gives
    /// no way to refuse; otherwise it would sit next to the agent's own Reject.
    pub(super) fn permission_request(
        &mut self,
        card: &PermissionCard,
        cx: &mut Context<Self>,
    ) -> ActionCard {
        let pending = self.replies.is_pending(&card.reference);
        let key = Self::permission_key(card);
        let mut notice = ActionCard::new(
            &self.theme,
            SharedString::from(format!("perm-request-{key}")),
            UiTone::Info,
            card.title.clone(),
        )
        .permission()
        .body(if pending {
            "Answer sent…"
        } else {
            "The agent asks for permission to go on."
        });
        if !pending {
            for choice in request_choices(&card.options, card.remember.is_some()) {
                let reference = card.reference.clone();
                let answer = choice.answer;
                let remember = card.remember.clone().filter(|_| choice.remember);
                notice = notice.action(self.action_button(
                    format!("perm-{key}-{}", choice.id),
                    choice.label,
                    true,
                    if choice.primary {
                        Emphasis::Primary
                    } else {
                        Emphasis::Neutral
                    },
                    cx,
                    move |this, _, cx| match &remember {
                        Some(rule) => {
                            this.answer_and_remember(reference.clone(), answer.clone(), rule, cx)
                        }
                        None => this.answer_permission(reference.clone(), answer.clone(), cx),
                    },
                ));
            }
        }
        notice
    }

    /// "Always in this project": answers the request with `answer` (the agent's
    /// `allow_once`) and, once the answer is accepted, saves `rule` for the open project.
    fn answer_and_remember(
        &mut self,
        reference: PermissionRef,
        answer: PermissionAnswer,
        rule: &studio_engine::agent::ApprovalRule,
        cx: &mut Context<Self>,
    ) {
        let (Some(attached), Some(snapshot)) = (&self.attached, &self.snapshot) else {
            return;
        };
        let workflow = attached.workflow.clone();
        let open = snapshot.open_permissions();
        if let Err(refusal) = self.replies.begin(&reference, &open) {
            self.say(refusal.to_string(), cx);
            return;
        }
        let result = workflow.reply_permission(&reference, answer);
        if self.command(result, cx).is_some() {
            let result = workflow.remember_approval(rule.clone());
            self.command(result, cx);
        }
        cx.notify();
    }

    /// The rules saved for the open project with "Always in this project", or `None`
    /// without an open project. For Settings → Agent → Permissions.
    pub fn approval_rules(&self) -> Option<Vec<studio_engine::agent::ApprovalRule>> {
        self.workflow().map(|workflow| workflow.approval_rules())
    }

    /// Removes a saved rule from the open project; the running task stops using it from
    /// its next request.
    pub fn forget_approval(
        &mut self,
        rule: &studio_engine::agent::ApprovalRule,
        cx: &mut Context<Self>,
    ) {
        if let Some(workflow) = self.workflow().cloned() {
            let result = workflow.forget_approval(rule);
            self.command(result, cx);
            cx.notify();
        }
    }

    pub(super) fn error_card(&self, row: &Row, error: &StructuredError) -> AnyElement {
        let mut lines = vec![error.detail.clone()];
        if let Some(a) = &error.action {
            lines.push(format!("Next: {a}"));
        }
        if let Some(p) = &error.phase {
            lines.push(format!("During {p} · code {}", error.code));
        }
        if error.retained {
            lines.push("The working copy and candidate were kept.".to_owned());
        }
        let body = lines.join("\n");
        ActionCard::new(
            &self.theme,
            Self::row_id(row),
            UiTone::Danger,
            error.title.clone(),
        )
        .copy(format!("{}\n{body}", error.title))
        .body(body)
        .into_any_element()
    }

    pub(super) fn validation_card(&self, row: &Row, card: &ValidationCard) -> AnyElement {
        let mut lines: Vec<String> = Vec::new();
        if let Some(c) = &card.coverage {
            lines.push(format!(
                "Coverage: {} of {} frames rendered, {} inspected, {} boundary frames, playhead {}{}{}",
                c.rendered_frames,
                c.total_frames,
                c.inspected_frames,
                c.boundary_frames,
                c.playhead,
                c.broadened
                    .as_ref()
                    .map(|b| format!(", broadened: {b}"))
                    .unwrap_or_default(),
                if c.complete { "" } else { " · PARTIAL" }
            ));
        }
        if let Some(a) = &card.audio {
            lines.push(format!(
                "Audio: {}, peak {:.2}, {} Hz, placement {}",
                if a.silent { "silent" } else { "present" },
                a.peak,
                a.sample_rate,
                if a.placement_verified {
                    "verified"
                } else {
                    "not verified"
                }
            ));
        }
        for error in card.errors.iter().take(6) {
            lines.push(format!("{:?} · {}", error.stage, error.message));
        }
        if card.errors_total > card.errors.len().min(6) {
            let more = card.errors_total - card.errors.len().min(6);
            lines.push(format!(
                "{more} more error{}",
                if more == 1 { "" } else { "s" }
            ));
        }
        if !card.warnings.is_empty() {
            lines.push(format!(
                "{} warning{}: {}",
                card.warnings.len(),
                if card.warnings.len() == 1 { "" } else { "s" },
                card.warnings
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" · ")
            ));
        }
        if !card.capability_gaps.is_empty() {
            lines.push(format!(
                "Not verified by this preview: {}",
                card.capability_gaps.join(", ")
            ));
        }
        if let Some(f) = &card.failure {
            lines.push(format!(
                "{:?} failure in {:?}: {}",
                f.kind, f.stage, f.summary
            ));
        }
        lines.push(format!(
            "Candidate {}{}{} · repair {}",
            &card.candidate[..card.candidate.len().min(12)],
            card.build_key
                .as_ref()
                .map(|k| format!(" · build {}", &k[..k.len().min(12)]))
                .unwrap_or_default(),
            format_args!(" · {} diagnostics", card.diagnostics_total),
            card.repair_count
        ));
        let (tone, head) = if card.passed {
            (UiTone::Success, "Validation passed")
        } else {
            (UiTone::Danger, "Validation failed")
        };
        let title = format!("{head} · {}", card.summary);
        let body = lines.join("\n");
        ActionCard::new(&self.theme, Self::row_id(row), tone, title.clone())
            .copy(format!("{title}\n{body}"))
            .body(body)
            .into_any_element()
    }

    pub(super) fn changes_card(&mut self, card: &ChangeCard, cx: &mut Context<Self>) -> AnyElement {
        use studio_engine::candidate_validation::ChangeKind;
        let files = card
            .entries
            .iter()
            .map(|entry| ChangedFile {
                mark: match entry.change {
                    ChangeKind::Added => ChangeMark::Added,
                    ChangeKind::Modified => ChangeMark::Modified,
                    ChangeKind::Deleted => ChangeMark::Deleted,
                    ChangeKind::Mode => ChangeMark::Mode,
                },
                path: entry.path.clone().into(),
            })
            .collect();
        let total = card.total.max(card.entries.len());
        let mut changes = ChangesCard::new(&self.theme, total, files);
        if let Some(button) = self.candidate_review_button(card, cx) {
            changes = changes.action(button);
        }
        changes.into_any_element()
    }

    /// The end of a task as one line: its outcome in the shared vocabulary and the summary.
    /// The status line and the error card above it carry the detail.
    fn outcome_line(&self, row: &Row, card: &studio_engine::agent::OutcomeCard) -> AnyElement {
        let theme = self.theme;
        let (title, tone) = outcome_style(card.kind);
        let mut lines = vec![card.summary.clone()];
        if let Some(p) = &card.published {
            lines.push(format!("Revision {}", &p[..p.len().min(12)]));
        }
        if card.draft_retained {
            lines.push("The agent's working copy was kept.".to_owned());
        }
        let summary: SharedString = lines.join(" ").into();
        let tip: SharedString = format!("{title} · {summary}").into();
        div()
            .id(Self::row_id(row))
            .h(px(ROW_H))
            .flex()
            .items_center()
            .gap(px(METRICS.space_2))
            .tooltip_show_delay(crate::kit::TOOLTIP_SHOW_DELAY)
            .tooltip(move |_, cx| crate::kit::Tooltip::new(&theme, tip.clone()).view(cx))
            .child(StatusDot::new(tone))
            .child(
                text_style(div().flex_none(), TextStyle::Label)
                    .text_color(theme.colors.text_1)
                    .child(title),
            )
            .child(
                text_style(div().flex_1().min_w_0().truncate(), TextStyle::Small)
                    .text_color(theme.colors.text_2)
                    .child(summary),
            )
            .into_any_element()
    }

    // ---- review --------------------------------------------------------------------------------

    pub(super) fn review_buttons(
        &mut self,
        review: &ReviewControls,
        cx: &mut Context<Self>,
    ) -> Vec<Button> {
        vec![
            self.action_button(
                "agent-export-candidate",
                "Export candidate…",
                true,
                Emphasis::Neutral,
                cx,
                |this, _, cx| this.export(Export::Candidate, cx),
            ),
            self.action_button(
                "agent-discard",
                "Discard",
                true,
                Emphasis::Danger,
                cx,
                |this, _, cx| {
                    if let Some(w) = this.workflow().cloned() {
                        let r = w.discard();
                        this.command(r, cx);
                    }
                },
            ),
            self.action_button(
                "agent-apply",
                if review.apply_note.is_some() {
                    "Retry Apply"
                } else {
                    "Apply"
                },
                true,
                Emphasis::Primary,
                cx,
                |this, _, cx| this.apply(cx),
            ),
        ]
    }

    pub(super) fn conflict_block(&self, conflict: &ConflictView) -> gpui::Div {
        let list = |title: &str, paths: &[String]| {
            let mut block = div().flex().flex_col();
            if !paths.is_empty() {
                block = block.child(self.line(title.to_owned()));
                for path in paths.iter().take(8) {
                    block = block.child(self.line(format!("  {path}")));
                }
                if paths.len() > 8 {
                    block = block.child(self.line(format!("  …and {} more", paths.len() - 8)));
                }
            }
            block
        };
        div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .child(label(&self.theme, format!("Conflict: {}", conflict.reason)))
            .child(list(
                "Changed in the project meanwhile:",
                &conflict.external_paths,
            ))
            .child(list("The candidate changes:", &conflict.candidate_paths))
            .child(list("Changed on both sides:", &conflict.overlapping))
            .children((!conflict.retained_variants.is_empty()).then(|| {
                let mut block = div().flex().flex_col();
                block = block.child(self.line("Preserved variants (nothing was overwritten):"));
                for variant in conflict.retained_variants.iter().take(8) {
                    block =
                        block.child(self.line(format!("  {} · {}", variant.path, variant.role)));
                }
                block
            }))
    }
}

/// One button of an open permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestChoice {
    /// The end of the button id `perm-{key}-{id}`: the agent's option id, or `decline`.
    id: String,
    /// The agent's own label for its option.
    label: String,
    /// Allowing options are the primary actions, every one of them.
    primary: bool,
    answer: PermissionAnswer,
    /// "Always in this project": the answer also saves the request's rule.
    remember: bool,
}

/// The id ending of the "Always in this project" button.
const REMEMBER_ID: &str = "always-in-project";

/// The buttons of an open permission request, left to right: refusals first and the allowing
/// choices last (the primary action sits rightmost). Decline (closes the request and ends the
/// turn) leads only when the agent gives no way to refuse; otherwise it would sit next to the
/// agent's own Reject. With `remember` (the request has a safe rule) "Always in this project"
/// comes between them, answering with the agent's `allow_once`; never without one.
fn request_choices(options: &[PermissionChoice], remember: bool) -> Vec<RequestChoice> {
    let mut choices = Vec::with_capacity(options.len() + 2);
    if !options.iter().any(|o| o.kind.starts_with("reject")) {
        choices.push(RequestChoice {
            id: "decline".to_owned(),
            label: "Decline".to_owned(),
            primary: false,
            answer: PermissionAnswer::Cancel,
            remember: false,
        });
    }
    let mut options: Vec<_> = options.iter().collect();
    options.sort_by_key(|option| option.kind.starts_with("allow"));
    let refusals = options
        .iter()
        .filter(|option| !option.kind.starts_with("allow"))
        .count();
    choices.extend(options.iter().map(|option| RequestChoice {
        id: option.option_id.clone(),
        label: option.name.clone(),
        primary: option.kind.starts_with("allow"),
        answer: PermissionAnswer::Select(option.option_id.clone()),
        remember: false,
    }));
    if remember && let Some(once) = options.iter().find(|option| option.kind == "allow_once") {
        let at = choices.len() - (options.len() - refusals);
        choices.insert(
            at,
            RequestChoice {
                id: REMEMBER_ID.to_owned(),
                label: "Always in this project".to_owned(),
                primary: false,
                answer: PermissionAnswer::Select(once.option_id.clone()),
                remember: true,
            },
        );
    }
    choices
}

/// How a tool call's status reads as a step.
fn tool_state(status: Option<ToolStatus>) -> ActivityState {
    match status {
        Some(ToolStatus::Pending) => ActivityState::Pending,
        Some(ToolStatus::InProgress) => ActivityState::Running,
        Some(ToolStatus::Completed) => ActivityState::Done,
        Some(ToolStatus::Failed) => ActivityState::Failed,
        None => ActivityState::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_engine::agent::RowId;

    fn row(id: u64, kind: RowKind) -> Arc<Row> {
        Arc::new(Row {
            id: RowId(id),
            task: None,
            at_unix: 0,
            kind,
        })
    }

    fn tool(id: u64, status: ToolStatus) -> Arc<Row> {
        row(
            id,
            RowKind::Tool(ToolCard {
                call_id: format!("call-{id}"),
                title: Some("Read a.rs".into()),
                kind: Some("read".into()),
                status: Some(status),
                updates: 0,
            }),
        )
    }

    fn user(id: u64) -> Arc<Row> {
        row(
            id,
            RowKind::User {
                text: "hi".into(),
                source: UserSource::Brief,
            },
        )
    }

    #[test]
    fn consecutive_steps_form_one_run_that_is_open_while_live() {
        let rows = vec![
            user(1),
            tool(2, ToolStatus::Completed),
            tool(3, ToolStatus::Completed),
            user(4),
            tool(5, ToolStatus::InProgress),
        ];
        let spans = activity_spans(&rows, &HashMap::new());
        assert_eq!(
            spans,
            [
                ActivitySpan {
                    start: 1,
                    end: 3,
                    key: 2,
                    open: false
                },
                ActivitySpan {
                    start: 4,
                    end: 5,
                    key: 5,
                    open: true
                },
            ]
        );
        // The reader's choice wins over the default.
        let overrides = HashMap::from([(2, true), (5, false)]);
        let open: Vec<bool> = activity_spans(&rows, &overrides)
            .iter()
            .map(|span| span.open)
            .collect();
        assert_eq!(open, [true, false]);
    }

    fn option(id: &str, name: &str, kind: &str) -> PermissionChoice {
        PermissionChoice {
            option_id: id.into(),
            name: name.into(),
            kind: kind.into(),
        }
    }

    /// `(id, label, primary)` of each button, left to right.
    fn buttons(options: &[PermissionChoice]) -> Vec<(String, String, bool)> {
        request_choices(options, false)
            .into_iter()
            .map(|choice| (choice.id, choice.label, choice.primary))
            .collect()
    }

    #[test]
    fn refusals_come_before_every_allow_and_each_allow_is_primary() {
        // The agent's order and labels are its own: allows first, unusual names.
        let options = [
            option("a1", "Yes, this once", "allow_once"),
            option("a2", "Always for this folder", "allow_always"),
            option("r1", "No, and tell it why", "reject_once"),
            option("r2", "Never", "reject_always"),
        ];
        assert_eq!(
            buttons(&options),
            [
                ("r1".into(), "No, and tell it why".into(), false),
                ("r2".into(), "Never".into(), false),
                ("a1".into(), "Yes, this once".into(), true),
                ("a2".into(), "Always for this folder".into(), true),
            ]
        );
        let answers: Vec<_> = request_choices(&options, false)
            .into_iter()
            .map(|choice| choice.answer)
            .collect();
        assert_eq!(
            answers,
            ["r1", "r2", "a1", "a2"].map(|id| PermissionAnswer::Select(id.into()))
        );
    }

    #[test]
    fn decline_leads_only_when_the_agent_offers_no_refusal() {
        let options = [option("go", "Allow", "allow_once")];
        let choices = request_choices(&options, false);
        assert_eq!(
            buttons(&options),
            [
                ("decline".into(), "Decline".into(), false),
                ("go".into(), "Allow".into(), true),
            ]
        );
        assert_eq!(choices[0].answer, PermissionAnswer::Cancel);
        // With a refusal of its own the agent's Reject stands alone.
        let options = [
            option("go", "Allow", "allow_once"),
            option("no", "Reject", "reject_once"),
        ];
        assert!(buttons(&options).iter().all(|(id, ..)| id != "decline"));
        // No options at all still leaves a way out.
        assert_eq!(buttons(&[]), [("decline".into(), "Decline".into(), false)]);
    }

    #[test]
    fn always_in_this_project_sits_between_refusals_and_allows_and_answers_once() {
        let options = [
            option("a2", "Always", "allow_always"),
            option("a1", "Allow", "allow_once"),
            option("r1", "Reject", "reject_once"),
        ];
        let choices = request_choices(&options, true);
        let ids: Vec<_> = choices.iter().map(|choice| choice.id.as_str()).collect();
        assert_eq!(ids, ["r1", REMEMBER_ID, "a2", "a1"]);
        let project = &choices[1];
        assert_eq!(project.label, "Always in this project");
        assert!(project.remember && !project.primary);
        // The agent's single allow, never its `allow_always`.
        assert_eq!(project.answer, PermissionAnswer::Select("a1".into()));
        assert_eq!(choices.iter().filter(|choice| choice.remember).count(), 1);
        // Not offered without a safe rule, nor without an `allow_once` to answer with.
        assert!(
            request_choices(&options, false)
                .iter()
                .all(|choice| !choice.remember)
        );
        let always_only = [
            option("a2", "Always", "allow_always"),
            option("r1", "Reject", "reject_once"),
        ];
        assert!(
            request_choices(&always_only, true)
                .iter()
                .all(|choice| !choice.remember && choice.id != REMEMBER_ID)
        );
        // With no refusal of the agent's own, Decline still leads.
        let ids: Vec<_> = request_choices(&[option("go", "Allow", "allow_once")], true)
            .into_iter()
            .map(|choice| choice.id)
            .collect();
        assert_eq!(ids, ["decline", REMEMBER_ID, "go"]);
    }

    #[test]
    fn tool_statuses_read_as_step_states() {
        assert_eq!(
            tool_state(Some(ToolStatus::InProgress)),
            ActivityState::Running
        );
        assert_eq!(tool_state(Some(ToolStatus::Failed)), ActivityState::Failed);
        assert_eq!(tool_state(Some(ToolStatus::Completed)), ActivityState::Done);
        assert_eq!(tool_state(None), ActivityState::Pending);
    }
}
