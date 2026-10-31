//! The live turn: driver pumping, scope evidence, prompts, agent events and permissions.

use super::*;
use crate::agent::OpenQuestion;
use studio_harness::driver::{DriverError, PromptFile};

pub(super) fn prompt_failure(error: DriverError) -> AgentFailure {
    match error {
        DriverError::Failed(failure) => failure,
        other => AgentFailure::new(
            studio_harness::driver::FailureKind::PromptRejected,
            studio_harness::driver::Phase::Prompt,
            other.to_string(),
        ),
    }
}

impl Actor {
    // ---- pumping the live driver ---------------------------------------------------------------------

    pub(super) fn pump(&mut self) {
        // Everything below belongs to the driver that is live NOW. Handling one of its
        // events can end the task and start a queued successor in the same call: the
        // epoch changes then, and nothing drained from the old driver (events or its
        // latched failure) may touch the new one.
        let epoch = self.driver_epoch;
        let Some(active) = &mut self.active else {
            return;
        };
        let Some(driver) = &active.driver else {
            return;
        };
        let task = active.ctx.identity.task.0.clone();
        // Drain everything queued (bounded per tick so commands are never starved): the
        // driver's 256-slot queue seals the session if a producer outruns its consumer.
        let mut events = Vec::new();
        for _ in 0..16 {
            let batch = driver.drain_events(256);
            let done = batch.len() < 256;
            events.extend(batch);
            if done {
                break;
            }
        }
        let failure = driver.failure();
        let starting =
            active.phase == TaskPhase::Starting || (active.in_repair && active.session.is_none());
        let ready = if starting && failure.is_none() {
            driver.wait_ready(Duration::ZERO).ok()
        } else {
            None
        };
        for event in events {
            if event.task != task {
                continue;
            }
            self.on_event(event);
            // The first row the log cannot keep ends the task at once: draining the rest
            // of the batch against a failing disk is what lets the driver's queue overflow.
            self.settle_log_loss();
            if self.driver_epoch != epoch || self.active.as_ref().is_none_or(|a| a.driver.is_none())
            {
                return;
            }
        }
        if self.driver_epoch != epoch {
            return;
        }
        if let Some(active) = &self.active
            && let Some(failure) = failure
            && !active.failed
            && active.driver.is_some()
        {
            self.on_provider_failure(failure);
            return;
        }
        if let Some(info) = ready
            && self
                .active
                .as_ref()
                .is_some_and(|a| a.session.is_none() && a.driver.is_some())
        {
            self.on_ready(info);
        }
    }

    pub(super) fn on_ready(&mut self, info: studio_harness::driver::SessionInfo) {
        let Some(active) = &self.active else {
            return;
        };
        let identity = active.ctx.identity.clone();
        self.capabilities = Some(info.initialized.capabilities.clone());
        if let Some(driver) = &active.driver {
            self.options = driver.options();
        }
        let Some(session) = info.session_id.clone() else {
            // A session id marks readiness for us; an adapter without one cannot run a task.
            let error = present::simple_error(
                "provider_no_session",
                "The agent did not create a session",
                "The adapter initialized but returned no session id.",
                Some("Check readiness in the agent setup."),
                true,
            );
            self.fail_with(error);
            return;
        };
        // The controller guard is a statement of its own: a refusal runs cleanup that
        // locks the controller again.
        let recorded = self
            .shared
            .controller
            .lock()
            .agent_provider_session(&identity, &session);
        if let Err(error) = recorded {
            let error = present::engine_error(&error, true);
            self.fail_with(error);
            return;
        }
        if let Some(active) = &mut self.active {
            active.session = Some(session.clone());
        }
        if let Some(task) = &mut self.task
            && let Some(writer) = &mut task.writer
        {
            writer.provider_session = Some(session);
        }
        if let Some(active) = &mut self.active
            && active.before_pending
            && matches!(active.pending_prompt, Some((_, PromptKind::Brief)))
        {
            // The first prompt carries the before evidence: wait for the render.
            active.prompt_held = true;
            self.touch();
            return;
        }
        self.dispatch_pending_prompt();
    }

    /// Sends the waiting prompt (the Brief moves the engine to Editing first).
    pub(super) fn dispatch_pending_prompt(&mut self) {
        let Some(active) = &self.active else {
            return;
        };
        let identity = active.ctx.identity.clone();
        let Some((prompt, kind)) = self
            .active
            .as_mut()
            .and_then(|active| active.pending_prompt.take())
        else {
            return;
        };
        if matches!(&kind, PromptKind::Brief) {
            let moved = self
                .shared
                .controller
                .lock()
                .agent_task_transition(&identity, TaskState::Editing);
            if let Err(error) = moved {
                let error = present::engine_error(&error, true);
                self.fail_with(error);
                return;
            }
        }
        // What the user attached wins the image cap; the before evidence takes what is left.
        let (files, user_images) = if matches!(&kind, PromptKind::Brief) {
            self.take_first_attachments(&prompt)
        } else {
            (Vec::new(), Vec::new())
        };
        let reserved = (
            user_images.len(),
            user_images.iter().map(PromptImage::byte_len).sum(),
        );
        let (mut images, image_limitation) = if matches!(&kind, PromptKind::Brief) {
            self.before_prompt_images(&identity.task, &prompt, reserved)
        } else {
            (Vec::new(), None)
        };
        images.splice(0..0, user_images);
        if let Some(task) = &mut self.task
            && task.scope.compiled.is_some()
        {
            task.image_limitation = image_limitation.clone();
        }
        let mut prompt = prompt;
        if matches!(&kind, PromptKind::Brief)
            && let Some(before) = self.task.as_ref().and_then(|task| task.before.as_ref())
        {
            prompt.push_str(&present::before_evidence_prompt(
                before,
                image_limitation.as_deref(),
            ));
        }
        self.send_prompt_files(&prompt, files, images);
    }

    // ---- scope evidence ------------------------------------------------------------------------

    pub(super) fn set_evidence(&mut self, kind: EvidenceKind, view: EvidenceView) {
        if let Some(task) = &mut self.task {
            match kind {
                EvidenceKind::Before => task.before = Some(view),
                EvidenceKind::After => task.after = Some(view),
            }
        }
    }

    /// Loads the bounded, task-owned before PNGs only when the negotiated adapter can
    /// consume ACP image content and they fit into one prompt next to `prompt` (the
    /// driver's wire budget). Any failure falls back to the text artifact references.
    pub(super) fn before_prompt_images(
        &self,
        task: &AgentTaskId,
        prompt: &str,
        // Images (count, PNG bytes) the user attached to the same prompt.
        reserved: (usize, usize),
    ) -> (Vec<PromptImage>, Option<String>) {
        let Some(task_view) = self.task.as_ref().filter(|view| &view.id == task) else {
            return (Vec::new(), None);
        };
        if task_view.scope.compiled.is_none() {
            return (Vec::new(), None);
        }
        let Some(before) = task_view.before.as_ref() else {
            return (
                Vec::new(),
                Some("Before image evidence was unavailable; the agent receives text only.".into()),
            );
        };
        if before.state != EvidenceState::Ready {
            return (
                Vec::new(),
                Some("Before image evidence was unavailable; the agent receives text only.".into()),
            );
        }
        let supports_images = self
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.prompt_image);
        if !supports_images {
            return (Vec::new(), Some(present::IMAGE_LIMITATION.to_owned()));
        }
        if before.artifacts.len() + reserved.0 > MAX_SCOPE_PROMPT_IMAGES {
            return (
                Vec::new(),
                Some(
                    "Before image evidence exceeded the attachment limit; text references only."
                        .into(),
                ),
            );
        }
        let (Some(backend), Some(driver)) = (
            self.tools.as_ref().map(ToolRuntime::backend),
            self.active.as_ref().and_then(|a| a.driver.as_ref()),
        ) else {
            return (
                Vec::new(),
                Some("The app could not load before images; the agent receives text only.".into()),
            );
        };
        // The text that goes with the images: its evidence section names no limitation.
        let text = format!("{prompt}{}", present::before_evidence_prompt(before, None));
        let budget = driver
            .prompt_image_budget(&text, before.artifacts.len() + reserved.0)
            .saturating_sub(reserved.1);
        let mut total_bytes = 0usize;
        let mut images = Vec::with_capacity(before.artifacts.len());
        for artifact in &before.artifacts {
            let remaining = budget.saturating_sub(total_bytes);
            let Ok(bytes) = backend
                .artifacts()
                .read_png_for_task(task, &artifact.id, remaining)
            else {
                return (
                    Vec::new(),
                    Some("Before images exceeded the prompt budget or expired; text references only.".into()),
                );
            };
            total_bytes += bytes.len();
            images.push(PromptImage::png(bytes));
        }
        if images.is_empty() {
            return (
                images,
                Some(
                    "Before image evidence contained no images; the agent receives text only."
                        .into(),
                ),
            );
        }
        (images, None)
    }

    /// Starts the background render of the frozen base around the task's scope. The
    /// first prompt waits for it; an unavailable render is shown and the prompt says so.
    pub(super) fn start_before_evidence(&mut self) {
        let Some(active) = &self.active else {
            return;
        };
        if active.ctx.scope.compiled.is_none() {
            return;
        }
        let revision = active.ctx.source_base.revision().clone();
        if let Some(active) = &mut self.active {
            active.before_pending = true;
        }
        self.start_evidence(EvidenceKind::Before, revision);
    }

    pub(super) fn start_evidence(
        &mut self,
        kind: EvidenceKind,
        revision: studio_project::SourceRevision,
    ) {
        let Some(active) = &self.active else {
            return;
        };
        let Some(compiled) = active.ctx.scope.compiled.clone() else {
            return;
        };
        let identity = active.ctx.identity.clone();
        let cancel = active.evidence_cancel.clone();
        let short = present::short(revision.as_str());
        let shared = self.shared.clone();
        let unavailable = |note: String| EvidenceView {
            state: EvidenceState::Unavailable,
            revision: short.clone(),
            artifacts: Vec::new(),
            note: Some(present::message(&note)),
        };
        let backend = match (&self.tools, &active.capability) {
            (Some(tools), Some(_)) => tools.backend(),
            _ => {
                let note = self.tool_notes_view().map_or_else(
                    || "project tools are not available for this task".into(),
                    |notes| notes.to_string(),
                );
                self.on_evidence(identity.task.clone(), kind, unavailable(note));
                return;
            }
        };
        self.set_evidence(
            kind,
            EvidenceView {
                state: EvidenceState::Pending,
                revision: short.clone(),
                artifacts: Vec::new(),
                note: None,
            },
        );
        let tx = self.shared.tx.clone();
        let task = identity.task.clone();
        let refused = self.admit(
            JobKind::Evidence {
                task: task.clone(),
                kind,
            },
            (backend, identity, revision, compiled, cancel, tx),
            move |(backend, identity, revision, compiled, cancel, tx)| {
                let deadline = Instant::now() + EVIDENCE_TIMEOUT;
                let view =
                    render_scope_evidence(&backend, &identity, &revision, &compiled, &|| {
                        cancel.load(Ordering::Acquire) || Instant::now() > deadline
                    });
                let artifacts =
                    (view.state == EvidenceState::Ready).then(|| view.artifacts.clone());
                let task = identity.task.clone();
                let previews = artifacts.map_or_else(Vec::new, |artifacts| {
                    read_evidence_preview_bytes(&shared, &backend, &task, &artifacts)
                });
                // AutoApply may promote and finalize as soon as the actor receives this
                // message. Decode first so the terminal snapshot cannot outrun its images.
                decode_evidence_previews(&shared, &task, previews);
                let _ = tx.send(Msg::Evidence {
                    task: identity.task.clone(),
                    kind,
                    view,
                });
            },
        );
        if let Err((message, _)) = refused {
            self.on_evidence(task, kind, unavailable(message));
        }
    }

    pub(super) fn on_evidence(
        &mut self,
        task: AgentTaskId,
        kind: EvidenceKind,
        view: EvidenceView,
    ) {
        let Some(active) = &mut self.active else {
            return;
        };
        if active.ctx.identity.task != task {
            return;
        }
        // Evidence rendered for a candidate a rejection has since replaced is stale.
        if kind == EvidenceKind::After
            && let Some(retained) = &active.retained
            && view.revision != present::short(retained.captured.candidate().revision().as_str())
        {
            return;
        }
        let settled = view.state != EvidenceState::Pending;
        self.set_evidence(kind, view.clone());
        if kind == EvidenceKind::Before && settled {
            let Some(active) = &mut self.active else {
                return;
            };
            active.before_pending = false;
            if std::mem::take(&mut active.prompt_held) {
                self.dispatch_pending_prompt();
            }
        }
        self.touch();
        let promote = kind == EvidenceKind::After
            && settled
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.promote_after_evidence);
        if promote {
            if let Some(active) = &mut self.active {
                active.promote_after_evidence = false;
            }
            let stopped = self.closing
                || self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.stop_requested || active.gate.is_cancelled());
            if stopped {
                self.cancel_or_interrupt();
            } else {
                self.start_promote();
            }
        }
    }

    pub(super) fn send_prompt(&mut self, text: &str, images: Vec<PromptImage>) {
        self.send_prompt_files(text, Vec::new(), images)
    }

    /// A prompt the driver refuses fails the task.
    pub(super) fn send_prompt_files(
        &mut self,
        text: &str,
        files: Vec<PromptFile>,
        images: Vec<PromptImage>,
    ) {
        if let Err(error) = self.try_send_prompt(text, files, images) {
            self.on_provider_failure(prompt_failure(error));
        }
    }

    /// Starts a turn and makes it the task's live turn; the caller decides what a
    /// refusal means.
    pub(super) fn try_send_prompt(
        &mut self,
        text: &str,
        files: Vec<PromptFile>,
        images: Vec<PromptImage>,
    ) -> Result<(), studio_harness::driver::DriverError> {
        let Some(active) = &mut self.active else {
            return Ok(());
        };
        let Some(driver) = &active.driver else {
            return Ok(());
        };
        let turn = if files.is_empty() {
            driver.prompt(text, images)?
        } else {
            driver.prompt_with_files(text, files, images)?
        };
        active.turn = Some(turn);
        self.tool_rows.clear();
        self.tool_order.clear();
        self.composer.open_tools.clear();
        self.stream = None;
        let phase = if active.in_repair {
            TaskPhase::Repairing
        } else {
            TaskPhase::Editing
        };
        if let Some(task) = &mut self.task {
            task.turns += 1;
        }
        self.set_phase(phase);
        Ok(())
    }

    pub(super) fn on_event(&mut self, event: AgentEvent) {
        match event.kind {
            AgentEventKind::Initialized(info) => self.capabilities = Some(info.capabilities),
            AgentEventKind::SessionReady {
                session_id,
                restored,
                user_mcp_servers,
            } => {
                self.note_session_servers(&user_mcp_servers);
                self.on_session_ready(session_id, restored);
            }
            AgentEventKind::Options(options) => {
                self.options = options;
                self.touch();
            }
            AgentEventKind::MessageDelta { role, text } => self.append_message(role, &text),
            AgentEventKind::ToolCall(tool) | AgentEventKind::ToolUpdate(tool) => {
                self.upsert_tool(tool)
            }
            AgentEventKind::PermissionRequested(prompt) => self.on_permission(prompt),
            AgentEventKind::Question(question) => self.on_question(question),
            AgentEventKind::PermissionClosed { id, resolution } => {
                self.on_permission_closed(id, resolution)
            }
            AgentEventKind::TurnStarted { turn, .. } => self.on_turn_started(turn),
            AgentEventKind::Background { tasks, continuing } => {
                self.on_background(tasks, continuing)
            }
            // Display only: the todo panel and the command list carry no workflow authority.
            AgentEventKind::Plan(entries) => {
                self.composer.plan = entries;
                self.touch();
            }
            AgentEventKind::AvailableCommands(commands) => {
                self.composer.commands = commands;
                self.touch();
            }
            AgentEventKind::PromptFinished(outcome) => self.on_turn_finished(outcome),
            AgentEventKind::SteerResult {
                text,
                accepted,
                message,
            } => self.on_steer_result(text, accepted, message),
            AgentEventKind::OptionRejected {
                option, message, ..
            } => {
                self.warn(format!(
                    "The agent refused the option change `{option}`: {message}"
                ));
            }
            AgentEventKind::Failure(failure) => self.on_provider_failure(failure),
            AgentEventKind::Closed(_) => {}
        }
    }

    pub(super) fn engine_transition(&mut self, next: TaskState) {
        let Some(active) = &self.active else {
            return;
        };
        let identity = active.ctx.identity.clone();
        let result = self
            .shared
            .controller
            .lock()
            .agent_task_transition(&identity, next);
        if let Err(error) = result {
            self.warn(format!("Task state could not move to {next:?}: {error}"));
        }
    }

    pub(super) fn on_permission(&mut self, prompt: PermissionPrompt) {
        let Some(active) = &mut self.active else {
            return;
        };
        let writer = active.writer.map_or(0, WriterGeneration::value);
        let reference = PermissionRef {
            task: active.ctx.identity.task.clone(),
            writer,
            request: prompt.id.0,
        };
        // A request the user's policy covers is answered here, through the driver like the
        // user's own reply; the row records it. Questions are always the user's.
        let auto = if active.open_questions.contains_key(&prompt.id.0) {
            None
        } else {
            super::super::auto_answer::auto_answer(
                active.permission_policy,
                &active.approvals,
                &prompt,
                &active.ctx.draft,
            )
        };
        if let Some(option) = auto {
            let policy = active.permission_policy;
            // A saved rule, not the policy, answered it: the row names the rule.
            let by_rule = super::super::auto_answer::answering_rule(
                policy,
                &active.approvals,
                &prompt,
                &active.ctx.draft,
            )
            .cloned();
            let delivered = active
                .driver
                .as_ref()
                .map(|driver| driver.reply_permission(prompt.id, PermissionReply::Select(option)));
            if matches!(delivered, Some(Ok(()))) {
                let row = self.push_row(RowKind::Permission(PermissionCard {
                    reference,
                    turn: prompt.turn,
                    tool_call_id: prompt.tool_call_id,
                    title: present::message(&prompt.title),
                    options: prompt.options,
                    state: PermissionState::Open,
                    answered_by: Some(policy),
                    remember: by_rule,
                }));
                self.permission_rows.insert((writer, prompt.id.0), row);
                return;
            }
            // Not delivered: the user answers it like any other request.
        }
        let Some(active) = &mut self.active else {
            return;
        };
        active.open_permissions.insert(prompt.id.0);
        // Questions are always the user's: no rule is offered for them.
        let remember = if active.open_questions.contains_key(&prompt.id.0) {
            None
        } else {
            super::super::ApprovalRule::for_request(&prompt, &active.ctx.draft)
        };
        let need_wait = !active.engine_waiting;
        active.engine_waiting = true;
        let offered: Vec<String> = prompt.options.iter().map(|o| o.option_id.clone()).collect();
        self.shared
            .registry
            .lock()
            .open
            .insert(reference.clone(), offered);
        let row = self.push_row(RowKind::Permission(PermissionCard {
            reference,
            turn: prompt.turn,
            tool_call_id: prompt.tool_call_id,
            title: present::message(&prompt.title),
            options: prompt.options,
            state: PermissionState::Open,
            answered_by: None,
            remember,
        }));
        self.permission_rows.insert((writer, prompt.id.0), row);
        if need_wait {
            self.engine_transition(TaskState::Waiting);
        }
        self.set_phase(TaskPhase::WaitingPermission);
    }

    /// A question waits like a permission request and shows as one: its first question's
    /// options are the choices, and the chosen one goes back as that question's answer.
    pub(super) fn on_question(&mut self, question: QuestionPrompt) {
        let Some(active) = &mut self.active else {
            return;
        };
        if question.questions.is_empty() {
            // Nothing to answer: close it at once so the agent does not wait for a card
            // nobody can fill in.
            if let Some(driver) = &active.driver {
                let _ = driver.reply_permission(question.id, PermissionReply::Cancel);
            }
            self.warn("The agent asked a question without any items; it was cancelled.");
            return;
        }
        active
            .open_questions
            .insert(question.id.0, question.questions.len());
        let reference = PermissionRef {
            task: active.ctx.identity.task.clone(),
            writer: active.writer.map_or(0, WriterGeneration::value),
            request: question.id.0,
        };
        self.composer.questions.push(OpenQuestion {
            reference,
            title: question.title.clone(),
            items: question.questions.clone(),
        });
        let first = question.questions.first();
        let title = match first {
            Some(item) if !item.question.is_empty() => {
                format!("{}: {}", question.title, item.question)
            }
            _ => question.title,
        };
        let options = first
            .map(|item| {
                item.options
                    .iter()
                    .map(|option| PermissionChoice {
                        option_id: option.label.clone(),
                        name: option.label.clone(),
                        kind: "answer".into(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.on_permission(PermissionPrompt {
            id: question.id,
            turn: question.turn,
            tool_call_id: String::new(),
            title,
            options,
            kind: None,
            locations: Vec::new(),
        });
    }

    pub(super) fn on_permission_closed(
        &mut self,
        id: PermissionId,
        resolution: PermissionResolution,
    ) {
        let Some(active) = &mut self.active else {
            return;
        };
        let writer = active.writer.map_or(0, WriterGeneration::value);
        let reference = PermissionRef {
            task: active.ctx.identity.task.clone(),
            writer,
            request: id.0,
        };
        active.open_permissions.remove(&id.0);
        active.open_questions.remove(&id.0);
        self.composer
            .questions
            .retain(|question| question.reference != reference);
        let none_left = active.open_permissions.is_empty();
        let in_repair = active.in_repair;
        let was_waiting = active.engine_waiting;
        {
            let mut registry = self.shared.registry.lock();
            // A close the user did not answer (Stop, turn end, provider loss) is stale now.
            registry.open.remove(&reference);
        }
        let state = match resolution {
            PermissionResolution::Selected { option_id } => PermissionState::Selected { option_id },
            PermissionResolution::Cancelled => PermissionState::Cancelled,
        };
        // The row may have left the transcript window: it is updated wherever it lives,
        // and the open-request index never outgrows what is actually open.
        if let Some(row) = self.permission_rows.remove(&(writer, id.0)) {
            self.update_row(row, |kind| {
                if let RowKind::Permission(card) = kind {
                    card.state = state;
                }
            });
        }
        if none_left && was_waiting {
            if let Some(active) = &mut self.active {
                active.engine_waiting = false;
            }
            self.engine_transition(TaskState::Editing);
            self.set_phase(if in_repair {
                TaskPhase::Repairing
            } else {
                TaskPhase::Editing
            });
        }
    }

    pub(super) fn reply_permission(&mut self, reference: PermissionRef, answer: PermissionAnswer) {
        let Some(active) = &self.active else {
            self.warn("That permission request is no longer current.");
            return;
        };
        let current = active.writer.map_or(0, WriterGeneration::value);
        if active.ctx.identity.task != reference.task || current != reference.writer {
            self.warn(
                "That permission request belongs to an earlier agent session; it was ignored.",
            );
            return;
        }
        let Some(driver) = &active.driver else {
            return;
        };
        let id = PermissionId(reference.request);
        let delivered = match (answer, active.open_questions.get(&reference.request)) {
            // The offered choices cover the first question only: a request with more
            // questions is answered through `answer_questions`, never padded.
            (PermissionAnswer::Select(option), Some(&1)) => driver.answer_question(
                id,
                vec![QuestionAnswer {
                    selected: vec![option],
                    text: None,
                }],
            ),
            (PermissionAnswer::Select(_), Some(_)) => {
                self.warn("That request asks several questions; each needs its own answer.");
                return;
            }
            (PermissionAnswer::Select(option), None) => {
                driver.reply_permission(id, PermissionReply::Select(option))
            }
            (PermissionAnswer::Cancel, _) => driver.reply_permission(id, PermissionReply::Cancel),
        };
        if let Err(error) = delivered {
            self.warn(format!("The permission reply was not delivered: {error}"));
        }
    }

    pub(super) fn driver_call(
        &mut self,
        call: impl FnOnce(&dyn AgentDriver) -> Result<(), studio_harness::driver::DriverError>,
    ) {
        let result = match self.active.as_ref().and_then(|a| a.driver.as_deref()) {
            Some(driver) => call(driver),
            None => {
                self.warn("There is no live agent session to change.");
                return;
            }
        };
        if let Err(error) = result {
            self.warn(format!("The option change was refused: {error}"));
        }
    }

    pub(super) fn on_turn_finished(&mut self, outcome: PromptOutcome) {
        self.finish_streaming();
        let Some(active) = &mut self.active else {
            return;
        };
        active.turn = None;
        // A redirect or a held message turns the end of this turn into the start of the
        // next; that is not a failed or finished edit.
        if self.composer_turn_finished(&outcome) {
            return;
        }
        if !outcome.eligible_for_quiescence() {
            let reason = if outcome.cancel_requested {
                "was cancelled".to_owned()
            } else if outcome.unresolved_permissions > 0 {
                format!(
                    "ended with {} permission request(s) unanswered",
                    outcome.unresolved_permissions
                )
            } else if outcome.reported_incomplete {
                "was reported incomplete by the adapter (some tool calls never got a result)"
                    .to_owned()
            } else if !outcome.authoritative {
                "ended without a stop reason from the adapter".to_owned()
            } else {
                format!("ended with `{}`", outcome.stop_reason.as_str())
            };
            let error = present::simple_error(
                "turn_not_completed",
                "The agent did not finish its edit",
                &format!("The agent's turn {reason}; nothing was captured."),
                Some("Send the brief again, perhaps more specifically; the working copy was kept."),
                true,
            );
            self.fail_with(error);
            return;
        }
        let Some(active) = &mut self.active else {
            return;
        };
        let (tasks, continuing) = active.background;
        if tasks > 0 || continuing {
            // Background work the agent started can still change the draft, and the agent
            // may continue by itself when it finishes: capture waits for both.
            active.capture_deferred = true;
            self.set_phase(TaskPhase::WaitingBackground);
            return;
        }
        self.capture_finished_turn();
    }

    /// The agent continued by itself after background work finished: its turn is the
    /// task's live turn and ends with its own outcome.
    pub(super) fn on_turn_started(&mut self, turn: u64) {
        let Some(active) = &mut self.active else {
            return;
        };
        active.turn = Some(turn);
        active.capture_deferred = false;
        let phase = if active.in_repair {
            TaskPhase::Repairing
        } else {
            TaskPhase::Editing
        };
        self.tool_rows.clear();
        self.tool_order.clear();
        self.stream = None;
        if let Some(task) = &mut self.task {
            task.turns += 1;
        }
        self.notice("The agent continued after a background task finished.");
        self.set_phase(phase);
    }

    pub(super) fn on_background(&mut self, tasks: u32, continuing: bool) {
        let Some(active) = &mut self.active else {
            return;
        };
        active.background = (tasks, continuing);
        let quiet = tasks == 0 && !continuing && active.turn.is_none();
        if quiet && std::mem::take(&mut active.capture_deferred) {
            self.capture_finished_turn();
        }
    }

    /// Captures after a clean turn end once no background work is live: a draft the agent
    /// never changed is a question, anything else goes to the pipeline.
    fn capture_finished_turn(&mut self) {
        let Some(active) = &mut self.active else {
            return;
        };
        let in_repair = active.in_repair;
        let draft = active.ctx.draft.clone();
        let base = active.ctx.source_base.revision().clone();
        let unchanged = !in_repair
            && SourceInventory::scan(&draft).is_ok_and(|inventory| inventory.revision == base);
        if unchanged {
            // The agent ended its turn without touching the draft: a question (ACP v1
            // has no question RPC). The answer is a subsequent prompt in the same session.
            let identity = active.ctx.identity.clone();
            let need_wait = !active.engine_waiting;
            active.engine_waiting = true;
            if need_wait {
                self.engine_transition(TaskState::Waiting);
            }
            self.shared.registry.lock().clarification = Some(identity.task);
            self.set_phase(TaskPhase::WaitingClarification);
            self.notice(
                "The agent ended its turn without changing any file. Answer it, or press Stop.",
            );
            return;
        }
        self.start_pipeline();
    }

    pub(super) fn clarify(&mut self, task: AgentTaskId, text: String) {
        let Some(active) = &mut self.active else {
            self.warn("No task is waiting for an answer.");
            return;
        };
        if active.ctx.identity.task != task || active.phase != TaskPhase::WaitingClarification {
            self.warn("No task is waiting for an answer.");
            return;
        }
        active.engine_waiting = false;
        self.engine_transition(TaskState::Editing);
        self.push_row(RowKind::User {
            text: present::bounded(&text, MAX_ROW_TEXT_BYTES),
            source: UserSource::Clarification,
        });
        self.send_prompt(&text, Vec::new());
    }
}
