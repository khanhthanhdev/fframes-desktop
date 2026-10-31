//! The validation pipeline, review and publication of a candidate.

use super::*;

impl Actor {
    // ---- the pipeline -------------------------------------------------------------------------------------

    pub(super) fn start_pipeline(&mut self) {
        let Some(env) = self.env() else {
            self.fail_with(present::simple_error(
                "sdk_unavailable",
                "No compatible SDK is installed",
                "The SDK settings were removed while the task ran.",
                None,
                true,
            ));
            return;
        };
        let gate_of_tools = self.tools.as_ref().map(|t| t.gate().clone());
        let playhead = self
            .shared
            .playhead
            .load(std::sync::atomic::Ordering::Relaxed);
        let root = self.shared.root_scope.clone();
        let Some(active) = &mut self.active else {
            return;
        };
        let Some(driver) = active.driver.take() else {
            return;
        };
        self.driver_epoch += 1;
        // The turn that led here already settled any waiting message.
        self.composer.retire();
        let identity = active.ctx.identity.clone();
        if let Some(tools) = &self.tools {
            tools.revoke(&identity);
        }
        let compiler = root.sub_manager();
        let worker = root.sub_manager();
        active.pipeline_scopes = Some((compiler.clone(), worker.clone()));
        // Each job gets its own gate: a Stop that raced the end of one job can never
        // cancel the next.
        let gate = Gate::new();
        active.gate = gate.clone();
        let repair_used = self.task.as_ref().map_or(0, |t| t.repair.used);
        let input = PipelineInput {
            env: env.clone(),
            identity,
            driver,
            session: active.session.clone(),
            writer: active.writer.expect("a writer was started"),
            gate_of_tools,
            scopes: RunScopes { compiler, worker },
            gate: gate.clone(),
            playhead,
            repair_used,
        };
        self.set_phase(TaskPhase::Quiescing);
        self.set_inflight(Some(gate));
        let refused = self.admit(JobKind::Pipeline, input, move |input| {
            let tx = input.env.tx.clone();
            let end = run_pipeline(input);
            let _ = tx.send(Msg::Pipeline(end));
        });
        if let Err((message, input)) = refused {
            self.set_inflight(None);
            // The adapter is reaped and its writer facts reach the engine before the
            // task fails: they cannot be recovered once the process is gone.
            let PipelineInput {
                env,
                identity,
                driver,
                ..
            } = input;
            let _ = reap_and_observe(&env, &identity, driver);
            self.fail_with(present::simple_error(
                "thread_failed",
                "A background worker could not be started",
                &message,
                Some("Close other applications to free system resources, then start the task again; the working copy was kept."),
                true,
            ));
        }
    }

    pub(super) fn on_progress(&mut self, phase: TaskPhase) {
        let in_flight = self.active.as_ref().is_some_and(|a| {
            matches!(
                a.phase,
                TaskPhase::Quiescing | TaskPhase::Capturing | TaskPhase::Validating
            )
        });
        if in_flight && !self.closing {
            self.set_phase(phase);
        }
    }

    pub(super) fn show_validation(&mut self, changes: ChangeCard, card: ValidationCard) {
        self.push_row(RowKind::Changes(changes.clone()));
        self.push_row(RowKind::Validation(Box::new(card.clone())));
        if let Some(task) = &mut self.task {
            task.changes = Some(changes);
            task.validation = Some(card);
        }
    }

    pub(super) fn on_pipeline(&mut self, end: PipelineEnd) {
        self.set_inflight(None);
        let Some(active) = &mut self.active else {
            return;
        };
        let stop = active.stop_requested || self.closing || active.gate.is_cancelled();
        active.pipeline_scopes = None;
        match end {
            PipelineEnd::ProviderFailed(failure) => {
                if stop {
                    self.cancel_or_interrupt();
                } else {
                    self.on_provider_failure(failure);
                }
            }
            PipelineEnd::Blocked(error) | PipelineEnd::Error(error) => {
                if stop {
                    self.cancel_or_interrupt();
                } else {
                    let error = present::engine_error(&error, true);
                    self.fail_with(error);
                }
            }
            PipelineEnd::Cancelled => self.cancel_or_interrupt(),
            PipelineEnd::Candidate {
                retained,
                changes,
                card,
            } => {
                self.show_validation(changes, *card);
                if stop {
                    drop(retained);
                    self.cancel_or_interrupt();
                    return;
                }
                let active = self.active.as_mut().expect("active");
                let policy = active.policy;
                active.retained = Some(retained);
                match policy {
                    ReviewPolicy::AutoApply => self.await_auto_apply_evidence(),
                    ReviewPolicy::ManualReview => self.await_review(None),
                }
            }
            PipelineEnd::Repair {
                context,
                changes,
                card,
            } => {
                self.show_validation(changes, *card);
                if stop {
                    self.cancel_or_interrupt();
                } else {
                    self.begin_repair(*context);
                }
            }
            PipelineEnd::Retained {
                reason,
                changes,
                card,
            } => {
                let summary = card.summary.clone();
                self.show_validation(changes, *card);
                let error = present::simple_error(
                    "validation_failed",
                    "The edit failed validation",
                    &format!("{summary} ({reason})"),
                    Some(
                        "The working copy and candidate were kept; start a new task or export the draft to inspect it.",
                    ),
                    true,
                );
                // The engine already ended the task `Failed`.
                self.finalize(None, TaskPhase::Failed, OutcomeKind::Failed, Some(error));
            }
        }
    }

    pub(super) fn cancel_or_interrupt(&mut self) {
        if self.closing {
            self.finalize(
                Some((TaskState::Interrupted, "project closed".into())),
                TaskPhase::Interrupted,
                OutcomeKind::Interrupted,
                None,
            );
        } else {
            self.finalize(
                Some((TaskState::Cancelled, "stopped by user".into())),
                TaskPhase::Cancelled,
                OutcomeKind::Cancelled,
                None,
            );
        }
    }

    pub(super) fn await_review(&mut self, blocked: Option<String>) {
        let Some(active) = &self.active else {
            return;
        };
        let candidate = active
            .retained
            .as_ref()
            .map(|r| present::short(r.captured.candidate().revision().as_str()))
            .unwrap_or_default();
        let first = blocked.is_none();
        let candidate_revision = active
            .retained
            .as_ref()
            .map(|r| r.captured.candidate().revision().clone());
        if first && let Some(revision) = candidate_revision {
            self.start_evidence(EvidenceKind::After, revision);
        }
        if let Some(task) = &mut self.task {
            task.review = Some(ReviewView {
                candidate: candidate.clone(),
                apply_blocked: blocked,
            });
        }
        self.set_phase(TaskPhase::AwaitingReview);
        if first {
            self.push_row(RowKind::Outcome(OutcomeCard {
                kind: OutcomeKind::AwaitingReview,
                summary: format!(
                    "Candidate {candidate} passed validation and is waiting for your review: Apply, discard or export it."
                ),
                published: None,
                draft_retained: true,
            }));
        }
    }

    pub(super) fn await_auto_apply_evidence(&mut self) {
        let Some(active) = &self.active else {
            return;
        };
        let revision = active
            .retained
            .as_ref()
            .map(|retained| retained.captured.candidate().revision().clone());
        let Some(revision) = revision else {
            return;
        };
        if active.ctx.scope.compiled.is_none() {
            self.start_promote();
            return;
        }
        if let Some(active) = &mut self.active {
            active.promote_after_evidence = true;
        }
        self.set_phase(TaskPhase::AwaitingEvidence);
        self.start_evidence(EvidenceKind::After, revision);
    }

    pub(super) fn begin_repair(&mut self, context: FailureContext) {
        let Some(active) = &self.active else {
            return;
        };
        let identity = active.ctx.identity.clone();
        // The guard is a statement of its own: a refusal runs cleanup that locks the
        // controller again.
        let begun = self.shared.controller.lock().agent_begin_repair(&identity);
        let attempt = match begun {
            Ok(attempt) => attempt,
            Err(error) => {
                let error = present::engine_error(&error, true);
                self.fail_with(error);
                return;
            }
        };
        // The old capability died with the old session; the repair gets a fresh one over
        // the same routes.
        let (build, tools_settings) = {
            let settings = self.shared.settings.lock();
            (settings.build.clone(), settings.tools.clone())
        };
        let Some(mut active) = self.active.take() else {
            return;
        };
        let offer = match &build {
            Some(build) => self.offer_tools(
                build,
                &active.adapter,
                tools_settings.as_ref(),
                &active.ctx,
                &active.agent_tools.enabled_tools(),
            ),
            None => ToolOffer::default(),
        };
        let route = offer.route();
        active.mcp = offer.mcp;
        active.cli = offer.cli;
        active.capability = offer.capability;
        active.in_repair = true;
        let prompt = present::repair_prompt(&context, &route);
        active.pending_prompt = Some((prompt.clone(), PromptKind::Repair));
        self.active = Some(active);
        if let Some(task) = &mut self.task {
            task.repair = RepairView {
                used: attempt,
                max: crate::MAX_AUTOMATIC_REPAIRS,
                in_progress: true,
                context_summary: Some(present::message(&context.summary)),
            };
            task.review = None;
        }
        self.push_row(RowKind::User {
            text: present::bounded(&prompt, MAX_ROW_TEXT_BYTES),
            source: UserSource::Repair,
        });
        if let Err(error) = self.spawn_writer() {
            self.fail_with(*error);
        }
    }

    // ---- publication ---------------------------------------------------------------------------------------

    pub(super) fn apply(&mut self) {
        let Some(active) = &mut self.active else {
            self.warn("There is no candidate to apply.");
            return;
        };
        if active.phase != TaskPhase::AwaitingReview || active.retained.is_none() {
            self.warn("There is no candidate waiting for review.");
            return;
        }
        if let Some(task) = &mut self.task
            && let Some(review) = &mut task.review
        {
            review.apply_blocked = None;
        }
        self.start_promote();
    }

    pub(super) fn start_promote(&mut self) {
        let Some(env) = self.env() else {
            self.fail_with(present::simple_error(
                "sdk_unavailable",
                "No compatible SDK is installed",
                "The SDK settings were removed while the task ran.",
                None,
                true,
            ));
            return;
        };
        let sink = self.shared.handoff.clone();
        let Some(active) = &mut self.active else {
            return;
        };
        let Some(retained) = active.retained.take() else {
            return;
        };
        active.promoting = true;
        let gate = Gate::new();
        active.gate = gate.clone();
        self.set_phase(TaskPhase::Promoting);
        self.set_inflight(Some(gate.clone()));
        let tx = env.tx.clone();
        let refused = self.admit(
            JobKind::Promote,
            (env, retained, gate, sink),
            move |(env, retained, gate, sink)| {
                let end = run_promote(env, *retained, gate, sink);
                let _ = tx.send(Msg::Promote(end));
            },
        );
        if let Err((message, (_, retained, _, _))) = refused {
            // Nothing was published: the candidate stays available for another Apply.
            self.set_inflight(None);
            if let Some(active) = &mut self.active {
                active.promoting = false;
                active.retained = Some(retained);
            }
            self.error_row(present::simple_error(
                "thread_failed",
                "A background worker could not be started",
                &message,
                Some("Close other applications to free system resources, then press Apply again."),
                true,
            ));
            self.await_review(Some(present::message(&message)));
        }
    }

    pub(super) fn on_promote(&mut self, end: PromoteEnd) {
        self.set_inflight(None);
        let Some(active) = &mut self.active else {
            return;
        };
        active.promoting = false;
        let deferred_stop = active.stop_requested;
        match end {
            PromoteEnd::Committed {
                promotion,
                handoff,
                scopes,
            } => {
                // Validation's compile is over. The staged worker was handed to the sink
                // (which owns it from now on) or dropped there; its scope is not shut
                // down here because that would reap an adopted worker.
                scopes.compiler.shutdown(Duration::ZERO);
                self.record_promotion(&promotion, &handoff, HandoffKind::Apply);
                if deferred_stop {
                    self.warn("Stop arrived while the edit was being published. The publication completed and is in the history; use Undo to revert it.");
                }
                self.finalize(None, TaskPhase::Accepted, OutcomeKind::Accepted, None);
            }
            PromoteEnd::Cancelled(retained) => {
                drop(retained);
                self.cancel_or_interrupt();
            }
            PromoteEnd::Failed { error, retained } => {
                let engine_state = self
                    .shared
                    .controller
                    .lock()
                    .agent_task()
                    .map(|t| t.state());
                let still_ready = engine_state == Some(TaskState::CandidateReady);
                if still_ready && !self.closing {
                    // Blocked gate or an unresolved mutation: the candidate is retained.
                    let structured = present::engine_error(&error, true);
                    self.error_row(structured);
                    let active = self.active.as_mut().expect("active");
                    active.retained = Some(retained);
                    self.await_review(Some(present::message(&error.to_string())));
                    return;
                }
                let phase = match engine_state {
                    Some(TaskState::Conflict) => TaskPhase::Conflict,
                    Some(TaskState::Interrupted) => TaskPhase::Interrupted,
                    Some(TaskState::Failed) => TaskPhase::Failed,
                    _ => TaskPhase::Failed,
                };
                let structured = present::engine_error(&error, true);
                if phase == TaskPhase::Conflict
                    || matches!(
                        &error,
                        EngineError::Promotion(
                            PromotionError::SourceChanged { .. }
                                | PromotionError::HistoryChanged
                                | PromotionError::SavedHistoryChanged
                        )
                    )
                {
                    let view = self.conflict_view(&error, Some(&retained.captured));
                    if let Some(task) = &mut self.task {
                        task.conflict = Some(view);
                    }
                }
                drop(retained);
                let kind = match phase {
                    TaskPhase::Conflict => OutcomeKind::Conflict,
                    TaskPhase::Interrupted => OutcomeKind::Interrupted,
                    _ => OutcomeKind::Failed,
                };
                if engine_state.is_some_and(TaskState::is_terminal) {
                    self.finalize(None, phase, kind, Some(structured));
                } else {
                    // The engine left the task open (it should not): end it ourselves.
                    self.finalize(
                        Some((TaskState::Failed, structured.title.clone())),
                        TaskPhase::Failed,
                        OutcomeKind::Failed,
                        Some(structured),
                    );
                }
            }
        }
    }

    /// The publication is durable (the job sends this BEFORE it calls the preview sink):
    /// the revision's handoff identity exists from here on, so an acknowledgement the
    /// sink triggers synchronously finds it.
    pub(super) fn on_committed(&mut self, published: &str, kind: HandoffKind) {
        self.handoff_published = Some(published.to_owned());
        self.handoff_acked = false;
        self.handoff = Some(HandoffView {
            kind,
            published: present::short(published),
            state: HandoffState::AwaitingPreview { reason: None },
        });
        self.touch();
    }

    /// A job thread panicked: the operation it owned would otherwise wait for a result
    /// that never comes (a task stuck Quiescing, Promoting or an Undo in progress).
    pub(super) fn on_job_aborted(&mut self, kind: JobKind, message: &str) {
        // Evidence jobs never own the task's cancellation gate. They may finish after a
        // Stop has started the next task, so they must not clear that task's in-flight gate.
        if !matches!(&kind, JobKind::Evidence { .. }) {
            self.set_inflight(None);
        }
        let error = present::simple_error(
            "worker_failed",
            "A background worker failed",
            message,
            Some("The working copy was kept; start the task again."),
            true,
        );
        match kind {
            JobKind::Probe => {
                self.probing = false;
                self.adapter.readiness = AdapterReadiness::Unchecked;
                self.error_row(error);
            }
            JobKind::Pipeline => {
                if self.active.is_some() {
                    self.fail_with(error);
                }
            }
            JobKind::Promote => {
                if self.active.is_none() {
                    return;
                }
                let state = self
                    .shared
                    .controller
                    .lock()
                    .agent_task()
                    .map(|t| t.state());
                if state == Some(TaskState::Accepted) {
                    // The commit was durable before the worker failed (in the preview
                    // sink): the task is accepted, the failure is only reported.
                    self.warn(format!(
                        "A background worker failed after the edit was applied ({message}); the edit is in the history."
                    ));
                    self.finalize(None, TaskPhase::Accepted, OutcomeKind::Accepted, None);
                } else {
                    self.fail_with(error);
                }
            }
            JobKind::Evidence { task, kind } => {
                if !self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.ctx.identity.task == task)
                {
                    return;
                }
                let Some(view) = self.task.as_ref().and_then(|task_view| match kind {
                    EvidenceKind::Before => task_view.before.as_ref(),
                    EvidenceKind::After => task_view.after.as_ref(),
                }) else {
                    return;
                };
                if view.state == EvidenceState::Pending {
                    self.on_evidence(
                        task,
                        kind,
                        EvidenceView {
                            state: EvidenceState::Unavailable,
                            revision: view.revision.clone(),
                            artifacts: Vec::new(),
                            note: Some(present::message(message)),
                        },
                    );
                }
            }
            JobKind::Reject => {
                // The task is still `Validating` the replacement; it ends here.
                if self.active.is_some() {
                    self.fail_with(error);
                }
            }
            JobKind::Undo => {
                if let Some(run) = self.undo.take() {
                    run.scopes.0.shutdown(Duration::ZERO);
                }
                self.error_row(error);
                self.undo_settled();
            }
        }
    }

    pub(super) fn record_promotion(
        &mut self,
        promotion: &crate::Promotion,
        handoff: &HandoffOutcome,
        kind: HandoffKind,
    ) {
        let published = promotion.record.published.as_str().to_owned();
        if self.handoff_published.as_deref() != Some(published.as_str()) {
            self.on_committed(&published, kind);
        }
        // What the sink returned only counts when no acknowledgement for this revision
        // (its own failure report, or the preview already showing it) came first.
        if !self.handoff_acked
            && let Some(view) = &mut self.handoff
        {
            view.state = match handoff {
                HandoffOutcome::Adopted => HandoffState::Adopted,
                HandoffOutcome::NoSink => HandoffState::AwaitingPreview { reason: None },
                HandoffOutcome::Failed(reason) => HandoffState::AwaitingPreview {
                    reason: Some(reason.clone()),
                },
            };
        }
        for note in &promotion.notes {
            self.warn(note);
        }
        self.push_row(RowKind::Outcome(OutcomeCard {
            kind: match kind {
                HandoffKind::Apply => OutcomeKind::Accepted,
                HandoffKind::Undo => OutcomeKind::Undone,
            },
            summary: format!(
                "{} {} file(s); source is now {}.",
                match kind {
                    HandoffKind::Apply => "Applied",
                    HandoffKind::Undo => "Undid",
                },
                promotion.record.changes.len(),
                present::short(&published)
            ),
            published: Some(present::short(&published)),
            draft_retained: false,
        }));
    }

    /// A sink that completes its adoption later (on its own thread) reports the outcome.
    pub(super) fn handoff_resolved(&mut self, published: &str, result: Result<(), String>) {
        if self.handoff_published.as_deref() != Some(published) {
            return;
        }
        if let Some(handoff) = &mut self.handoff
            && handoff.state != HandoffState::Displayed
        {
            handoff.state = match result {
                Ok(()) => HandoffState::Adopted,
                Err(reason) => HandoffState::AwaitingPreview {
                    reason: Some(present::message(&reason)),
                },
            };
            self.handoff_acked = true;
            self.touch();
        }
    }

    pub(super) fn preview_displayed(&mut self, revision: &str) {
        if self.handoff_published.as_deref() == Some(revision)
            && let Some(handoff) = &mut self.handoff
        {
            handoff.state = HandoffState::Displayed;
            self.handoff_acked = true;
            self.touch();
        }
    }

    pub(super) fn conflict_view(
        &mut self,
        error: &EngineError,
        captured: Option<&CapturedCandidate>,
    ) -> ConflictView {
        let candidate_paths: Vec<String> = captured
            .map(|c| c.changes().entries.iter().map(|e| e.path.clone()).collect())
            .unwrap_or_default();
        let mut external_paths = Vec::new();
        let mut variants = Vec::new();
        let mut transaction = None;
        let mut reason = present::message(&error.to_string());
        if let EngineError::Promotion(PromotionError::Conflict(report)) = error {
            let view = conflict_of_report(report);
            external_paths = view.external_paths;
            variants = view.retained_variants;
            transaction = view.transaction;
            reason = view.reason;
        } else if let EngineError::Promotion(PromotionError::Plan(crate::PlanError::Conflict {
            path,
            ..
        })) = error
        {
            external_paths.push(path.clone());
        }
        if let (Some(captured), true) = (captured, external_paths.is_empty()) {
            // The source moved: list what changed in the project since the task base.
            let controller = self.shared.controller.lock();
            if let Ok(base) = controller
                .checkpoints
                .load(captured.source_base().revision())
            {
                let changes = ChangeSet::between(&base, &controller.project.inventory);
                external_paths = changes.entries.iter().map(|e| e.path.clone()).collect();
            }
        }
        let overlapping = external_paths
            .iter()
            .filter(|p| candidate_paths.contains(p))
            .cloned()
            .collect();
        ConflictView {
            reason,
            transaction,
            external_paths,
            candidate_paths,
            overlapping,
            retained_variants: variants,
        }
    }

    pub(super) fn discard(&mut self) {
        let Some(active) = &self.active else {
            self.warn("There is no candidate to discard.");
            return;
        };
        if active.phase != TaskPhase::AwaitingReview {
            self.warn("There is no candidate waiting for review.");
            return;
        }
        self.finalize(
            Some((TaskState::Cancelled, "discarded by user".into())),
            TaskPhase::Cancelled,
            OutcomeKind::Discarded,
            None,
        );
    }

    // ---- reviewing file by file ------------------------------------------------------------------------

    /// Leaves `paths` out of the candidate under review. The remaining files are a new
    /// candidate that is validated like any other; only if it passes does it replace the
    /// one under review, otherwise the original stays exactly as it was.
    pub(super) fn reject_files(&mut self, paths: Vec<String>) {
        let Some(active) = &self.active else {
            self.warn("There is no candidate to review.");
            return;
        };
        let Some(retained) = &active.retained else {
            self.warn("There is no candidate waiting for review.");
            return;
        };
        if active.phase != TaskPhase::AwaitingReview {
            self.warn("There is no candidate waiting for review.");
            return;
        }
        if paths.is_empty() {
            self.warn("Choose at least one file to reject.");
            return;
        }
        let changes = retained.captured.changes();
        if !changes.truncated
            && changes
                .entries
                .iter()
                .all(|entry| paths.iter().any(|path| path == &entry.path))
        {
            self.notice("Every file was rejected, so the candidate was discarded.");
            self.discard();
            return;
        }
        let Some(env) = self.env() else {
            self.error_row(present::simple_error(
                "sdk_unavailable",
                "No compatible SDK is installed",
                "The remaining files are validated against the SDK; none is selected.",
                Some("Install or select the SDK in setup."),
                false,
            ));
            return;
        };
        let playhead = self
            .shared
            .playhead
            .load(std::sync::atomic::Ordering::Relaxed);
        let repair_used = self.task.as_ref().map_or(0, |t| t.repair.used);
        let root = self.shared.root_scope.clone();
        let Some(active) = &mut self.active else {
            return;
        };
        let Some(retained) = active.retained.take() else {
            return;
        };
        let (compiler, worker) = (root.sub_manager(), root.sub_manager());
        active.pipeline_scopes = Some((compiler.clone(), worker.clone()));
        active.rejecting = true;
        let gate = Gate::new();
        active.gate = gate.clone();
        self.set_phase(TaskPhase::Validating);
        self.set_inflight(Some(gate.clone()));
        let tx = env.tx.clone();
        let input = RejectInput {
            env,
            retained,
            rejected: paths,
            scopes: RunScopes { compiler, worker },
            gate,
            playhead,
            repair_used,
        };
        let refused = self.admit(JobKind::Reject, input, move |input| {
            let end = run_reject(input);
            let _ = tx.send(Msg::Rejected(end));
        });
        if let Err((message, input)) = refused {
            input.scopes.compiler.shutdown(Duration::ZERO);
            input.scopes.worker.shutdown(Duration::ZERO);
            self.set_inflight(None);
            if let Some(active) = &mut self.active {
                active.pipeline_scopes = None;
                active.rejecting = false;
            }
            self.error_row(present::simple_error(
                "thread_failed",
                "A background worker could not be started",
                &message,
                Some("Close other applications to free system resources, then try again; the candidate is unchanged."),
                true,
            ));
            self.resume_review(input.retained);
        }
    }

    /// The candidate is waiting for review again, exactly as before the operation that
    /// borrowed it. Stop during the operation only cancelled the operation; closing (or a
    /// pending provider switch) still ends the task.
    fn resume_review(&mut self, retained: Box<Retained>) {
        let stop = self.closing || self.active.as_ref().is_some_and(|a| a.stop_requested);
        let Some(active) = &mut self.active else {
            return;
        };
        active.retained = Some(retained);
        if stop {
            self.cancel_or_interrupt();
        } else {
            self.set_phase(TaskPhase::AwaitingReview);
        }
    }

    pub(super) fn on_rejected(&mut self, end: RejectEnd) {
        self.set_inflight(None);
        let Some(active) = &mut self.active else {
            return;
        };
        active.pipeline_scopes = None;
        active.rejecting = false;
        match end {
            RejectEnd::Replaced {
                retained,
                changes,
                card,
            } => {
                self.show_validation(changes, *card);
                let stop = self.closing || self.active.as_ref().is_some_and(|a| a.stop_requested);
                let Some(active) = &mut self.active else {
                    return;
                };
                let revision = retained.captured.candidate().revision().clone();
                active.retained = Some(retained);
                if stop {
                    self.cancel_or_interrupt();
                } else {
                    // The review continues with the remaining files; the reason Apply was
                    // blocked before (if any) still holds, so it is kept.
                    self.start_evidence(EvidenceKind::After, revision.clone());
                    if let Some(task) = &mut self.task {
                        let blocked = task.review.as_ref().and_then(|r| r.apply_blocked.clone());
                        task.review = Some(ReviewView {
                            candidate: present::short(revision.as_str()),
                            apply_blocked: blocked,
                        });
                    }
                    self.set_phase(TaskPhase::AwaitingReview);
                    self.notice("The rejected files were left out; the remaining files passed validation and wait for your review.");
                }
            }
            RejectEnd::Stranded { retained, error } => {
                drop(retained);
                self.fail_with(present::simple_error(
                    "reject_restore_failed",
                    "The candidate could not be restored after the rejection",
                    &error.to_string(),
                    Some("The working copy is kept. Start a new task to continue."),
                    false,
                ));
            }
            RejectEnd::Invalid { retained, card } => {
                let summary = card.summary.clone();
                self.push_row(RowKind::Validation(card));
                self.error_row(present::simple_error(
                    "reject_invalid",
                    "The remaining files do not validate",
                    &summary,
                    Some("The candidate is unchanged. A file you rejected is probably needed by one you kept; reject those together or apply them all."),
                    true,
                ));
                self.resume_review(retained);
            }
            RejectEnd::Refused { retained, error } => {
                self.error_row(present::engine_error(&error, true));
                self.resume_review(retained);
            }
            RejectEnd::Cancelled(retained) => {
                self.notice("Stopped; the candidate under review is unchanged.");
                self.resume_review(retained);
            }
        }
    }

    /// Reverts one file of an accepted revision: an Undo that touches only `path`.
    pub(super) fn start_revert_path(&mut self, target: String, path: String) {
        if self.active.is_some() || self.undo.is_some() {
            self.warn("Reverting a file needs the agent to be idle.");
            return;
        }
        let Some(env) = self.env() else {
            self.error_row(present::simple_error(
                "sdk_unavailable",
                "No compatible SDK is installed",
                "The reverted source is validated against the SDK; none is selected.",
                Some("Install or select the SDK in setup."),
                false,
            ));
            return;
        };
        let gate = Gate::new();
        let root = self.shared.root_scope.clone();
        let scopes = (root.sub_manager(), root.sub_manager());
        self.undo = Some(UndoRun {
            gate: gate.clone(),
            scopes: scopes.clone(),
        });
        self.sync_registry(0);
        self.undo_view = UndoView::InProgress {
            phase: "Preparing to revert the file".into(),
        };
        self.touch();
        self.set_inflight(Some(gate.clone()));
        let playhead = self
            .shared
            .playhead
            .load(std::sync::atomic::Ordering::Relaxed);
        let sink = self.shared.handoff.clone();
        let tx = env.tx.clone();
        let refused = self.admit(
            JobKind::Undo,
            (env, target, path, scopes, gate, sink),
            move |(env, target, path, scopes, gate, sink)| {
                let end = run_revert_path(
                    env,
                    target,
                    path,
                    RunScopes {
                        compiler: scopes.0,
                        worker: scopes.1,
                    },
                    gate,
                    playhead,
                    sink,
                );
                let _ = tx.send(Msg::Undone(end));
            },
        );
        if let Err((message, (_, _, _, scopes, _, _))) = refused {
            scopes.0.shutdown(Duration::ZERO);
            scopes.1.shutdown(Duration::ZERO);
            self.set_inflight(None);
            self.undo = None;
            self.error_row(present::simple_error(
                "thread_failed",
                "A background worker could not be started",
                &message,
                Some("Close other applications to free system resources, then try again; nothing was changed."),
                false,
            ));
            self.undo_settled();
        }
    }

    pub(super) fn export_candidate(&mut self, dest: &Path) {
        let Some(active) = &self.active else {
            self.warn("There is no candidate to export.");
            return;
        };
        let Some(retained) = &active.retained else {
            self.warn("There is no candidate to export.");
            return;
        };
        let revision = retained.captured.candidate().revision().clone();
        let result = self
            .shared
            .controller
            .lock()
            .export_checkpoint(&revision, dest);
        match result {
            Ok(()) => self.notice(format!(
                "Exported the candidate as an independent project to {}",
                dest.display()
            )),
            Err(error) => self.error_row(present::engine_error(&error, true)),
        }
    }

    pub(super) fn export_draft(&mut self, dest: &Path) {
        if self.active.is_some() {
            self.warn("Export the working copy after the task ends.");
            return;
        }
        let (path, state) = {
            let controller = self.shared.controller.lock();
            (
                controller.agent_draft_store().path().to_path_buf(),
                controller.agent_draft_state().ok().flatten(),
            )
        };
        match state {
            None => {
                self.warn("There is no working copy to export yet.");
                return;
            }
            Some(DraftState::UnsafeWriter { .. } | DraftState::Active { .. }) => {
                self.warn("The working copy is locked because an agent may still be writing to it; confirm that nothing is writing, then export.");
                return;
            }
            Some(_) => {}
        }
        if dest.exists() {
            self.warn("The export destination already exists; choose a new folder.");
            return;
        }
        let result = studio_project::checkpoint::copy_draft(&path, dest).and_then(|()| {
            studio_project::lifecycle::assign_independent_identity(dest).map(|_| ())
        });
        match result {
            Ok(()) => self.notice(format!(
                "Exported the working copy as an independent project to {}",
                dest.display()
            )),
            Err(error) => self.error_row(present::engine_error(&error.into(), true)),
        }
    }
}
