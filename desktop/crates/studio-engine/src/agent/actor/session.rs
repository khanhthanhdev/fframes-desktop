//! Provider failure and stop, provider handoff, session restoration and new chats.

use super::*;

impl Actor {
    // ---- provider failure and stop ----------------------------------------------------------------------

    pub(super) fn on_provider_failure(&mut self, failure: AgentFailure) {
        let session_failure = failure.phase == studio_harness::driver::Phase::Session
            || failure.kind == studio_harness::driver::FailureKind::RestoreUnsupported;
        if session_failure && std::mem::take(&mut self.resume_attempt) {
            self.resume_failed = true;
            self.read_only_after_failed_resume();
        } else if session_failure {
            if let Ok(Some(mut manifest)) =
                super::session_store::SessionStore::load(&self.shared.paths, &self.shared.project)
            {
                manifest.state = super::session_store::SessionManifestState::Unsafe;
                let _ = super::session_store::SessionStore::save(
                    &self.shared.paths,
                    &self.shared.project,
                    &manifest,
                );
                let redacted_id = manifest.redacted_id();
                self.session_restore = Some(SessionRestoreView {
                    provider_id: manifest.provider_id,
                    redacted_session_id: redacted_id,
                    is_resumable: false,
                    last_source_revision: manifest.last_source_revision,
                    notice: Some(
                        "Previous session restoration failed; fresh session available.".to_string(),
                    ),
                });
            }
            self.pending_resume_session = None;
            self.notice("Session restoration failed; next task will start with a fresh session.");
            self.touch();
        }
        let error = present::provider_error(&failure, true);
        self.fail_with(error);
    }

    /// The agent refused or failed to resume the stored session: say so and fall back to the
    /// read-only rule (the next prompt starts a fresh agent session in this stored session).
    /// The stored manifest is left as it was, so a transient failure does not end resumption.
    fn read_only_after_failed_resume(&mut self) {
        let manifest = SessionStore::load(&self.shared.paths, &self.shared.project)
            .ok()
            .flatten();
        self.session_restore = Some(SessionRestoreView {
            provider_id: manifest
                .as_ref()
                .map(|m| m.provider_id.clone())
                .unwrap_or_default(),
            redacted_session_id: manifest
                .as_ref()
                .map_or_else(|| "none".to_owned(), SessionManifest::redacted_id),
            is_resumable: false,
            last_source_revision: manifest
                .map(|m| m.last_source_revision)
                .unwrap_or_default(),
            notice: Some(
                "Previous session restoration failed; the next prompt starts a fresh agent session."
                    .to_string(),
            ),
        });
        self.notice(
            "The agent could not resume the stored session. The stored messages stay read-only; the next prompt starts a fresh agent session in this conversation.",
        );
        self.touch();
    }

    /// Ends the task `Failed`: the adapter is reaped, the draft retained, no writer left.
    pub(super) fn fail_with(&mut self, error: StructuredError) {
        if let Some(active) = &mut self.active {
            active.failed = true;
        }
        self.finish_streaming();
        self.finalize(
            Some((TaskState::Failed, error.title.clone())),
            TaskPhase::Failed,
            OutcomeKind::Failed,
            Some(error),
        );
    }

    pub(super) fn stop(&mut self) {
        let cleared = self.queue.len();
        if cleared > 0 {
            self.shared.registry.lock().pending -= cleared;
            self.queue.clear();
            self.notice(format!("Stop cancelled {cleared} queued brief(s)."));
        }
        if let Some(undo) = &self.undo {
            if undo.gate.cancel() {
                undo.scopes.0.shutdown(Duration::ZERO);
                undo.scopes.1.shutdown(Duration::ZERO);
                self.notice("Stopping the Undo; nothing was changed.");
            } else {
                self.notice("The Undo is being published and cannot be stopped; it will finish and can be undone again.");
            }
            return;
        }
        let Some(active) = &mut self.active else {
            if cleared == 0 {
                self.notice("Nothing is running.");
            }
            return;
        };
        if active.rejecting && active.phase == TaskPhase::Validating {
            // Stop ends only the check of the remaining files; the candidate under review
            // is untouched and still waits for the user.
            active.gate.cancel();
            if let Some((compiler, worker)) = &active.pipeline_scopes {
                compiler.shutdown(Duration::ZERO);
                worker.shutdown(Duration::ZERO);
            }
            self.notice("Stopping; the candidate under review is unchanged.");
            return;
        }
        active.stop_requested = true;
        let phase = active.phase;
        if let Some(task) = &mut self.task {
            task.stop_requested = true;
        }
        match phase {
            TaskPhase::Starting
            | TaskPhase::Editing
            | TaskPhase::Repairing
            | TaskPhase::WaitingPermission
            | TaskPhase::WaitingClarification
            | TaskPhase::WaitingBackground => {
                self.finish_streaming();
                self.finalize(
                    Some((TaskState::Cancelled, "stopped by user".into())),
                    TaskPhase::Cancelled,
                    OutcomeKind::Cancelled,
                    None,
                );
            }
            TaskPhase::Quiescing | TaskPhase::Capturing | TaskPhase::Validating => {
                active.gate.cancel();
                if let Some((compiler, worker)) = &active.pipeline_scopes {
                    compiler.shutdown(Duration::ZERO);
                    worker.shutdown(Duration::ZERO);
                }
                self.notice("Stopping; the working copy is kept.");
            }
            TaskPhase::AwaitingEvidence => {
                active.evidence_cancel.store(true, Ordering::Release);
                self.finalize(
                    Some((
                        TaskState::Cancelled,
                        "stopped while capturing candidate evidence".into(),
                    )),
                    TaskPhase::Cancelled,
                    OutcomeKind::Cancelled,
                    None,
                );
            }
            TaskPhase::Promoting => {
                if active.gate.cancel() {
                    self.notice("Stopping before the edit is published.");
                } else {
                    self.notice(
                        "The edit is being published; that step is short and cannot be cancelled. Once it ends, check the result and use Undo if you do not want it.",
                    );
                }
            }
            TaskPhase::AwaitingReview => {
                active.stop_requested = false;
                if let Some(task) = &mut self.task {
                    task.stop_requested = false;
                }
                self.notice("Nothing is running; the candidate waits for your review. Use Discard to drop it.");
            }
            _ => {}
        }
    }

    // ---- provider handoff & restoration --------------------------------------------------------------------

    pub(super) fn initiate_switch(&mut self, target_provider: String) {
        if self.switch_pending.is_some() || self.switch_target.is_some() {
            self.warn("A provider switch is already pending. Choose continuation or cancel.");
            return;
        }

        self.switch_target = Some(target_provider);

        // Retain queued briefs across the switch: they must not be silently executed
        // on the new provider without explicit confirmation.
        self.retained_queue.extend(self.queue.drain(..));

        if self.undo.is_some() {
            self.notice("Waiting for in-flight Undo to complete before switching providers.");
            return;
        }

        if let Some(active) = &mut self.active {
            active.stop_requested = true;
            if let Some(task) = &mut self.task {
                task.stop_requested = true;
            }

            match active.phase {
                TaskPhase::Promoting => {
                    if !active.gate.cancel() {
                        self.notice("The edit is being published; waiting for publication before switching providers.");
                        return;
                    }
                    self.notice("Stopping publication before switching providers.");
                }
                TaskPhase::AwaitingReview => {
                    self.warn("A candidate is awaiting review. Apply or Discard the candidate before switching providers.");
                    self.switch_target = None;
                    self.queue.extend(self.retained_queue.drain(..));
                    self.touch();
                }
                TaskPhase::Quiescing | TaskPhase::Capturing | TaskPhase::Validating => {
                    active.gate.cancel();
                    if let Some((compiler, worker)) = &active.pipeline_scopes {
                        compiler.shutdown(Duration::ZERO);
                        worker.shutdown(Duration::ZERO);
                    }
                    self.notice("Stopping validation; waiting for background pipeline before switching providers.");
                }
                TaskPhase::AwaitingEvidence => {
                    active.evidence_cancel.store(true, Ordering::Release);
                    self.finalize(
                        Some((
                            TaskState::Cancelled,
                            "stopped while capturing candidate evidence".into(),
                        )),
                        TaskPhase::Cancelled,
                        OutcomeKind::Cancelled,
                        None,
                    );
                }
                TaskPhase::Starting
                | TaskPhase::Editing
                | TaskPhase::Repairing
                | TaskPhase::WaitingPermission
                | TaskPhase::WaitingClarification
                | TaskPhase::WaitingBackground => {
                    active.gate.cancel();
                    if let Some(driver) = &active.driver {
                        let _ = driver.cancel_turn();
                    }
                    if let Some((compiler, worker)) = &active.pipeline_scopes {
                        compiler.shutdown(Duration::ZERO);
                        worker.shutdown(Duration::ZERO);
                    }
                    self.finish_streaming();
                    if let Some(active) = &self.active
                        && let Some(tools) = &self.tools
                    {
                        tools.end_task(&active.ctx.identity);
                    }
                    self.finalize(
                        Some((TaskState::Cancelled, "switched provider".into())),
                        TaskPhase::Cancelled,
                        OutcomeKind::Cancelled,
                        None,
                    );
                }
                _ => {}
            }
        } else {
            // Idle: execute switch teardown directly
            self.finish_switch_teardown();
        }
    }

    pub(super) fn finish_switch_teardown(&mut self) {
        let Some(target_provider) = self.switch_target.clone() else {
            return;
        };

        // 1. Wait for in-flight Undo to finish
        if self.undo.is_some() {
            return;
        }

        // 2. Check draft state from engine:
        let draft_state = self.shared.controller.lock().agent_draft_store().snapshot();
        match draft_state.ok().flatten().map(|s| s.state) {
            Some(DraftState::UnsafeWriter { reason, .. }) => {
                // Writer escaped or was not observed gone: block handoff, retain draft!
                self.switch_target = None;
                self.notice(format!(
                    "Provider switch blocked: writer termination could not be verified ({reason}). Working copy retained."
                ));
                self.touch();
                return;
            }
            Some(DraftState::Retained { .. }) | None => {}
            Some(DraftState::Active { .. }) => {
                // Writer is still marked active; cannot capture yet.
                return;
            }
            _ => {}
        }

        // 3. Drain tool snapshots using WriterGate before capture
        let _drain_guard = if let Some(tools) = &self.tools {
            match tools.gate().try_acquire() {
                Some(guard) => Some(guard),
                None => {
                    // Tool snapshot is actively executing: wait for it to drain before capturing!
                    self.notice(
                        "Waiting for in-flight tool snapshot to drain before capturing draft.",
                    );
                    return;
                }
            }
        } else {
            None
        };

        // We can now finalize the switch teardown
        self.switch_target = None;
        // 4. Capture outgoing draft immutably to checkpoints
        let draft_revision = match self.shared.controller.lock().capture_outgoing_draft() {
            Ok(rev) => Some(rev.to_string()),
            Err(_) => None,
        };

        // 5. Capture source and accepted revision IDs
        let (source_revision, accepted_revision) = {
            let ctrl = self.shared.controller.lock();
            let source = ctrl.current_source_revision().to_string();
            let accepted = ctrl.state().accepted().to_string();
            (source, accepted)
        };

        let outgoing_brief = self.task.as_ref().map(|t| t.brief.clone());
        let handoff_context = self.task.as_ref().map_or_else(
            || {
                super::session_store::BoundedHandoffContext::new(
                    source_revision.clone(),
                    draft_revision.clone(),
                    "No active outgoing task".into(),
                    None,
                    None,
                    None,
                    Vec::new(),
                )
            },
            |task| {
                let mut diagnostics = task
                    .validation
                    .as_ref()
                    .into_iter()
                    .flat_map(|card| {
                        card.errors
                            .iter()
                            .map(|diagnostic| diagnostic.message.clone())
                            .chain(card.warnings.iter().cloned())
                    })
                    .collect::<Vec<_>>();
                if let Some(error) = &task.error {
                    diagnostics.push(error.detail.clone());
                }
                if let Some(conflict) = &task.conflict {
                    diagnostics.push(conflict.reason.clone());
                }
                let outcome = Some(format!(
                    "{}{}",
                    task.phase.label(),
                    task.reason
                        .as_deref()
                        .map(|reason| format!(": {reason}"))
                        .unwrap_or_default()
                ));
                let style = task
                    .scope
                    .style_snapshot
                    .as_ref()
                    .and_then(|style| serde_json::to_string(style).ok());
                super::session_store::BoundedHandoffContext::new(
                    source_revision.clone(),
                    draft_revision.clone(),
                    task.brief.clone(),
                    outcome,
                    Some(task.scope.clone()),
                    style,
                    diagnostics,
                )
            },
        );
        let view = SwitchPendingView {
            target_provider: target_provider.clone(),
            draft_revision,
            source_revision,
            accepted_revision,
            outgoing_task_brief: outgoing_brief,
            retained_queue_count: self.retained_queue.len(),
            handoff_context,
        };

        self.notice(format!(
            "Provider switch ready for {target_provider}. Choose whether to continue draft or restart from accepted."
        ));
        self.switch_pending = Some(view);
        self.touch();
    }

    pub(super) fn choose_handoff_continuation(&mut self, choice: HandoffChoice) {
        let Some(pending) = self.switch_pending.take() else {
            self.warn("No provider switch is pending.");
            return;
        };
        let selected_revision = match choice {
            HandoffChoice::ContinueDraft => pending
                .draft_revision
                .as_deref()
                .unwrap_or(&pending.accepted_revision),
            HandoffChoice::RestartFromAccepted => &pending.accepted_revision,
        };
        let draft_choice = match handoff_draft_choice(&pending, selected_revision) {
            Ok(choice) => choice,
            Err(error) => {
                self.warn(format!("Provider switch blocked: {error}."));
                self.switch_pending = Some(pending);
                self.touch();
                return;
            }
        };

        // Switch adapter settings to target provider
        let target_prov = pending.target_provider.clone();
        let mut registry = match crate::providers::host::load_registry(&self.shared.paths) {
            Ok(Some(reg)) => reg,
            Ok(None) => crate::providers::provider_profiles::ProviderRegistry::default(),
            Err(e) => {
                self.warn(format!(
                    "Provider switch blocked: could not load registry ({e})."
                ));
                self.switch_pending = Some(pending);
                self.touch();
                return;
            }
        };

        let Some(profile) = registry
            .profiles
            .iter()
            .find(|p| p.id == target_prov || p.label == target_prov)
            .cloned()
        else {
            self.warn(format!(
                "Provider switch to {} blocked: profile not found in registry.",
                target_prov
            ));
            self.switch_pending = Some(pending);
            self.touch();
            return;
        };

        // Containment is verified when the new adapter launches, not here.
        let settings = profile.adapter.resolve(&self.shared.ownership_policy);
        if let Err(error) = registry.select_profile(&profile.id) {
            self.warn(format!(
                "Provider switch blocked: could not select profile ({error})."
            ));
            self.switch_pending = Some(pending);
            self.touch();
            return;
        }
        if let Err(error) = crate::providers::host::save_registry(&self.shared.paths, &registry) {
            self.warn(format!(
                "Provider switch blocked: could not save the selected profile ({error})."
            ));
            self.switch_pending = Some(pending);
            self.touch();
            return;
        }
        self.shared.settings.lock().adapter = Some(settings);
        self.composer.forget_commands();
        self.settings_changed();

        self.next_draft_choice = Some(draft_choice);
        self.next_handoff_context = Some(pending.handoff_context.clone());
        match choice {
            HandoffChoice::ContinueDraft => {
                self.notice(format!(
                    "Continuing from the selected outgoing revision ({}); current source and accepted history will be rechecked before launch.",
                    selected_revision
                ));
            }
            HandoffChoice::RestartFromAccepted => {
                self.notice("Restarting from the exact accepted source revision; current source and accepted history will be rechecked before launch.");
            }
        }

        if !self.retained_queue.is_empty() {
            self.notice(format!(
                "Provider switch complete. {} queued brief(s) from previous provider retained; choose Transfer or Clear.",
                self.retained_queue.len()
            ));
        }

        self.touch();
        self.start_next();
    }

    pub(super) fn cancel_switch(&mut self) {
        if self.switch_pending.is_some() || self.switch_target.is_some() {
            self.switch_pending = None;
            self.switch_target = None;
            self.queue.extend(self.retained_queue.drain(..));
            self.notice("Provider switch cancelled; remaining on current provider.");
            self.touch();
            self.start_next();
        }
    }

    pub(super) fn transfer_retained_queue(&mut self) {
        let count = self.retained_queue.len();
        if count == 0 {
            self.notice("No retained queued briefs to transfer.");
            return;
        }
        self.queue.extend(self.retained_queue.drain(..));
        self.notice(format!(
            "Transferred {count} queued brief(s) to new provider."
        ));
        self.touch();
        self.start_next();
    }

    pub(super) fn clear_retained_queue(&mut self) {
        let count = self.retained_queue.len();
        if count == 0 {
            self.notice("No retained queued briefs to clear.");
            return;
        }
        self.retained_queue.clear();
        {
            let mut r = self.shared.registry.lock();
            r.pending = r.pending.saturating_sub(count);
        }
        self.notice(format!("Cleared {count} retained queued brief(s)."));
        self.touch();
    }

    /// A fresh agent session without an id that can be stored replaces the one the manifest
    /// names, so the old id must not be offered for resumption any more.
    fn forget_stored_wire_id(&self) {
        let (paths, project) = (&self.shared.paths, &self.shared.project);
        if let Ok(Some(mut manifest)) = SessionStore::load(paths, project)
            && manifest.wire_session_id.take().is_some()
        {
            let _ = SessionStore::save(paths, project, &manifest);
        }
    }

    /// The notes about the task's tools the snapshot shows: those made at task start, then
    /// those made when the session said which of the user's servers it got.
    pub(super) fn tool_notes_view(&self) -> Option<ToolNotes> {
        let notes: Vec<ToolNote> = self
            .tool_notes
            .0
            .iter()
            .chain(&self.session_notes)
            .cloned()
            .collect();
        (!notes.is_empty()).then_some(ToolNotes(notes))
    }

    /// Notes the user's servers the session did not get (`sent` is the driver's report),
    /// replacing those of an earlier session of the task (a repair starts a new one). An
    /// agent that takes no MCP servers already has its note from the task start.
    pub(super) fn note_session_servers(&mut self, sent: &[String]) {
        let Some(active) = &self.active else {
            return;
        };
        let mut notes = Vec::new();
        if active.adapter.mcp == McpStdioSupport::Baseline {
            for server in &active.user_mcp {
                let name = server.name();
                if !sent.iter().any(|sent| sent == name) {
                    let why = match server.transport {
                        studio_harness::McpTransport::Http(_) => {
                            "this agent does not support HTTP MCP servers."
                        }
                        studio_harness::McpTransport::Stdio(_) => {
                            "the agent's session did not take it."
                        }
                    };
                    notes.push(ToolNote::new(
                        ToolNoteKind::ServerLeftOut {
                            name: name.to_owned(),
                        },
                        format!("The MCP server `{name}` was left out: {why}"),
                    ));
                } else if active.adapter.wire == studio_harness::AgentWire::Acp
                    && !server.disabled_tools.is_empty()
                {
                    // No ACP agent can leave out single tools of a server.
                    notes.push(ToolNote::other(format!(
                        "ACP agents get every tool of `{name}`, including the ones turned off."
                    )));
                }
            }
        }
        if notes != self.session_notes {
            self.session_notes = notes;
            self.touch();
        }
    }

    pub(super) fn on_session_ready(&mut self, _sanitized_id: String, restored: bool) {
        self.resume_attempt = false;
        let launch_digest = self
            .active
            .as_ref()
            .and_then(|active| session_launch_digest(active).ok());
        let Some(launch_digest) = launch_digest else {
            self.forget_stored_wire_id();
            self.warn("Session restoration disabled: exact adapter launch identity could not be verified.");
            self.session_restore = Some(SessionRestoreView {
                provider_id: self
                    .active
                    .as_ref()
                    .map(|active| active.adapter.provider.clone())
                    .unwrap_or_default(),
                redacted_session_id: "unavailable".to_string(),
                is_resumable: false,
                last_source_revision: self
                    .active
                    .as_ref()
                    .map(|active| active.ctx.source_base.revision().to_string())
                    .unwrap_or_default(),
                notice: Some("Exact adapter launch identity could not be verified".to_string()),
            });
            self.touch();
            return;
        };
        let wire_id = self
            .active
            .as_ref()
            .and_then(|a| a.driver.as_ref().and_then(|d| d.wire_session_id()));
        if let Some(wire_id) = wire_id {
            if let Some(active) = &self.active {
                let accepted_rev = self.shared.controller.lock().state().accepted().to_string();
                let now_ts = now_unix().to_string();
                let manifest = SessionManifest {
                    schema_version: session_store::SESSION_MANIFEST_SCHEMA_VERSION,
                    provider_id: active.adapter.provider.clone(),
                    wire_session_id: Some(wire_id),
                    project_id: self.shared.project.to_string(),
                    draft_path: active.ctx.draft.clone(),
                    launch_digest: launch_digest.clone(),
                    last_source_revision: active.ctx.source_base.revision().to_string(),
                    last_accepted_revision: accepted_rev,
                    context_schema: session_store::CONTEXT_SCHEMA_VERSION.to_string(),
                    capability_snapshot: self.capabilities.clone(),
                    transcript_boundary: self.store.sequence_boundary(),
                    state: SessionManifestState::Ready,
                    created_at: now_ts.clone(),
                    updated_at: now_ts,
                };
                match SessionStore::save(&self.shared.paths, &self.shared.project, &manifest) {
                    Ok(()) => {
                        let redacted_id = manifest.redacted_id();
                        self.session_restore = Some(SessionRestoreView {
                            provider_id: manifest.provider_id,
                            redacted_session_id: redacted_id,
                            is_resumable: true,
                            last_source_revision: manifest.last_source_revision,
                            notice: None,
                        });
                    }
                    Err(e) => {
                        self.warn_once(&format!(
                            "Session manifest could not be saved ({e}); session restoration is disabled."
                        ));
                        let redacted_id = manifest.redacted_id();
                        self.session_restore = Some(SessionRestoreView {
                            provider_id: manifest.provider_id,
                            redacted_session_id: redacted_id,
                            is_resumable: false,
                            last_source_revision: manifest.last_source_revision,
                            notice: Some(format!("Save failure: {e}")),
                        });
                    }
                }
            }
        } else {
            self.forget_stored_wire_id();
            self.notice("Session restoration disabled: provider session ID required credential redaction and cannot be safely resumed.");
            self.session_restore = Some(SessionRestoreView {
                provider_id: self
                    .active
                    .as_ref()
                    .map(|a| a.adapter.provider.clone())
                    .unwrap_or_default(),
                redacted_session_id: "none".to_string(),
                is_resumable: false,
                last_source_revision: self
                    .active
                    .as_ref()
                    .map(|a| a.ctx.source_base.revision().to_string())
                    .unwrap_or_default(),
                notice: Some("Session restoration disabled (redacted ID)".to_string()),
            });
        }
        if restored {
            self.notice(
                "Session restored from previous provider state; fresh task capabilities issued.",
            );
        }
        self.touch();
    }

    /// Why `manifest`'s agent session cannot be resumed by the configured agent, judged by the
    /// session's own provider and the capabilities stored with it; `None` when resuming may be
    /// tried. Unknown capabilities mean "try": a refusal at launch falls back to a fresh session.
    fn resume_blocker(&self, manifest: &SessionManifest) -> Option<String> {
        let configured = self.adapter.provider.as_ref();
        if let Some(configured) = configured.filter(|p| **p != manifest.provider_id) {
            Some(format!(
                "it was run by {} and the configured agent is {configured}",
                manifest.provider_id
            ))
        } else if !manifest.is_resumable() {
            Some("its agent session cannot be resumed".to_owned())
        } else if manifest
            .capability_snapshot
            .as_ref()
            .is_some_and(|caps| !caps.load_session && !caps.resume_session)
        {
            Some("the agent does not support resuming sessions".to_owned())
        } else {
            None
        }
    }

    /// The view of a stored session that will not be resumed, with `text` as the reason.
    fn not_resumable_view(manifest: Option<&SessionManifest>, text: &str) -> SessionRestoreView {
        SessionRestoreView {
            provider_id: manifest.map(|m| m.provider_id.clone()).unwrap_or_default(),
            redacted_session_id: manifest.map_or_else(|| "none".to_owned(), |m| m.redacted_id()),
            is_resumable: false,
            last_source_revision: manifest
                .map(|m| m.last_source_revision.clone())
                .unwrap_or_default(),
            notice: Some(text.to_owned()),
        }
    }

    /// Queues the stored agent session of `manifest` for the next launch when the project is
    /// as the session left it. `Ok` says what will be restored, `Err` why a fresh agent session
    /// is used instead. Adds no rows.
    fn queue_resume(&mut self, manifest: &SessionManifest) -> Result<String, String> {
        let (project_id, draft_path, source_revision, accepted_revision, draft_state) = {
            let controller = self.shared.controller.lock();
            (
                self.shared.project.to_string(),
                controller.agent_draft_store().path().to_path_buf(),
                controller.current_source_revision().to_string(),
                controller.state().accepted().to_string(),
                controller.agent_draft_state(),
            )
        };
        if manifest.project_id != project_id {
            return Err(
                "Saved session belongs to a different project; a fresh session will be used."
                    .to_owned(),
            );
        }
        if manifest.draft_path != draft_path {
            return Err(
                "Saved session draft location changed; a fresh session will be used.".to_owned(),
            );
        }
        match draft_state {
            Ok(None) | Ok(Some(DraftState::Accepted { .. })) => {}
            Ok(Some(DraftState::UnsafeWriter { .. })) => {
                return Err("Saved session cannot be resumed while writer termination is unverified; preserve the draft and resolve recovery first.".to_owned());
            }
            Ok(Some(_)) => {
                return Err("Saved session has a retained draft that cannot be safely matched to native memory; a fresh session will be used.".to_owned());
            }
            Err(error) => {
                return Err(format!(
                    "Saved session cannot be resumed because draft safety could not be verified ({error}); a fresh session will be used."
                ));
            }
        }
        if manifest.last_source_revision != source_revision
            || manifest.last_accepted_revision != accepted_revision
        {
            return Err(
                "Saved session source or accepted revision changed; a fresh session will be used."
                    .to_owned(),
            );
        }
        let Some(wire_id) = manifest.wire_session_id.clone() else {
            return Err(
                "Saved session has no resumable session ID; a fresh session will be used."
                    .to_owned(),
            );
        };
        self.pending_resume_session = Some(PendingResumeSession {
            provider_id: manifest.provider_id.clone(),
            launch_digest: manifest.launch_digest.clone(),
            wire_session_id: wire_id,
            draft_path: manifest.draft_path.clone(),
            last_source_revision: manifest.last_source_revision.clone(),
            last_accepted_revision: manifest.last_accepted_revision.clone(),
        });
        Ok(format!(
            "Restoring session for provider {} (session {})",
            manifest.provider_id,
            manifest.redacted_id()
        ))
    }

    /// The user asked to restore the stored session of the open conversation.
    pub(super) fn restore_session(&mut self) {
        let manifest = match SessionStore::load(&self.shared.paths, &self.shared.project) {
            Ok(Some(manifest)) => manifest,
            Ok(None) => {
                self.notice(
                    "No saved session manifest found to restore; fresh session will be used.",
                );
                return;
            }
            Err(reason) => {
                let text = format!("Saved session cannot be restored: {reason}");
                self.session_restore = Some(Self::not_resumable_view(None, &text));
                self.notice(text);
                self.touch();
                return;
            }
        };
        if let Some(reason) = self.resume_blocker(&manifest) {
            let text =
                format!("Saved session cannot be resumed: {reason}; a fresh session will be used.");
            self.session_restore = Some(Self::not_resumable_view(Some(&manifest), &text));
            self.notice(text);
        } else {
            match self.queue_resume(&manifest) {
                Ok(text) | Err(text) => self.notice(text),
            }
        }
        self.touch();
    }

    pub(super) fn dismiss_restoration(&mut self) {
        if let Err(error) = SessionStore::clear(&self.shared.paths, &self.shared.project) {
            self.warn(format!(
                "The saved session could not be dismissed and stays on offer: {error}"
            ));
            self.touch();
            return;
        }
        self.session_restore = None;
        self.pending_resume_session = None;
        self.notice("Saved session dismissed.");
        self.touch();
    }

    /// Why the open conversation cannot be replaced now, by a new chat or by another stored
    /// session (`action` completes "before ..."); `None` when it can.
    pub(super) fn switch_blocker(&self, action: &str) -> Option<String> {
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.phase == TaskPhase::AwaitingReview)
        {
            Some(format!("Apply or Discard the candidate before {action}."))
        } else if self.active.is_some() || self.undo.is_some() {
            Some(format!(
                "Stop the running task or wait for it to finish before {action}."
            ))
        } else if self.switch_target.is_some() || self.switch_pending.is_some() {
            Some(format!(
                "Finish or cancel the provider switch before {action}."
            ))
        } else if !self.queue.is_empty() || !self.retained_queue.is_empty() {
            Some(format!(
                "Cancel or clear the queued briefs before {action}."
            ))
        } else if !self.recovery.unresolved_conflicts.is_empty()
            || self.task.as_ref().is_some_and(|t| t.conflict.is_some())
        {
            Some(format!("Resolve the conflict before {action}."))
        } else {
            None
        }
    }

    /// Shows a warning row unless the transcript already holds the same text.
    pub(super) fn warn_once(&mut self, text: &str) {
        let text = present::message(text);
        let known =
            self.store.resident().iter().any(
                |row| matches!(&row.kind, RowKind::Notice { text: shown, .. } if *shown == text),
            );
        if !known {
            self.push_row(RowKind::Notice {
                level: NoticeLevel::Warning,
                text,
            });
        }
    }

    /// Offers the current stored session's manifest for restoration, as it stands at launch,
    /// judged against the configured provider. Made again when the settings change while the
    /// offer is still shown.
    pub(super) fn offer_saved_session(&mut self) {
        let manifest = match SessionStore::load(&self.shared.paths, &self.shared.project) {
            Ok(Some(manifest)) => manifest,
            Ok(None) => return,
            // An older Studio's manifest stays as it is; say why it is not offered.
            Err(error) => {
                self.warn_once(&error);
                return;
            }
        };
        let project_id = self.shared.project.to_string();
        let compatible = {
            let controller = self.shared.controller.lock();
            manifest.project_id == project_id
                && manifest.draft_path == controller.agent_draft_store().path()
                && manifest.last_source_revision == controller.current_source_revision().to_string()
                && manifest.last_accepted_revision == controller.state().accepted().to_string()
                && matches!(
                    controller.agent_draft_state(),
                    Ok(None) | Ok(Some(DraftState::Accepted { .. }))
                )
        };
        // The same gate as opening or restoring the session: the configured provider and the
        // stored capabilities decide before the project state does.
        let blocker = self.resume_blocker(&manifest);
        let is_resumable = blocker.is_none() && compatible;
        self.session_restore = Some(SessionRestoreView {
            provider_id: manifest.provider_id.clone(),
            redacted_session_id: manifest.redacted_id(),
            is_resumable,
            last_source_revision: manifest.last_source_revision.clone(),
            notice: Some(match blocker {
                _ if is_resumable => {
                    "Saved session found; adapter compatibility will be checked before launch."
                        .to_string()
                }
                Some(reason) => format!(
                    "The saved session cannot be resumed: {reason} (fresh session available)."
                ),
                None => "Previous session is incompatible with the current project state (fresh session available).".to_string(),
            }),
        });
        self.startup_offer = self.session_restore.clone();
    }

    /// Makes `next` the open conversation and forgets everything tied to the previous one:
    /// the streaming message, tool and permission rows, the task shown and any restoration
    /// on offer. `chat` changes so a view drops its caches and in-flight history pages.
    fn replace_conversation(&mut self, next: RowStore) {
        self.store = next;
        self.stream = None;
        self.tool_rows.clear();
        self.tool_order.clear();
        self.permission_rows.clear();
        self.task = None;
        self.session_restore = None;
        self.pending_resume_session = None;
        // A provider switch's handoff belongs to the conversation it was made in.
        self.next_handoff_context = None;
        self.next_draft_choice = None;
        self.resume_attempt = false;
        self.resume_failed = false;
        // The old agent session's commands and plan are not this conversation's.
        self.composer.forget_commands();
        self.composer.plan.clear();
        self.chat += 1;
    }

    /// Saves the conversation being left and returns the smallest row id the next log may hand
    /// out: after every id any session of the project used. The log is flushed and then closed
    /// (it is never held open); its last row id is recorded with the session so ids stay unique
    /// across restarts.
    fn leave_conversation(&mut self) -> Result<u64, String> {
        self.store
            .flush()
            .map_err(|e| format!("the current conversation could not be saved: {e}"))?;
        let (paths, project) = (&self.shared.paths, &self.shared.project);
        let last_row = self.store.resident().last().map(|row| row.id.0);
        if let (Some(current), Some(last)) =
            (SessionStore::current_session_id(paths, project), last_row)
        {
            SessionStore::record_last_row_id(paths, project, &current, last)?;
        }
        Ok((self.store.sequence_boundary() as u64)
            .max(SessionStore::project_row_floor(paths, project)))
    }

    /// Opens a log for a stored session whose row ids start at `floor` at least.
    fn open_session_log(&self, session: &str, floor: u64) -> Result<RowStore, String> {
        let path =
            SessionStore::session_log_path(&self.shared.paths, &self.shared.project, session)?;
        RowStore::open_with_floor(path, self.store.limits(), floor).map_err(|e| e.to_string())
    }

    /// The log of a new empty session, made current. The previous session keeps its manifest
    /// and stays resumable.
    fn begin_session(&mut self) -> Result<RowStore, String> {
        let floor = self.leave_conversation()?;
        let (paths, project) = (&self.shared.paths, &self.shared.project);
        let id = SessionStore::create_session_from(paths, project, None, now_unix(), floor)?;
        let opened = self.open_session_log(&id, floor).and_then(|next| {
            SessionStore::set_current(paths, project, &id)?;
            Ok(next)
        });
        if opened.is_err() {
            // An empty session nobody switched to would only clutter the list.
            let _ = std::fs::remove_dir_all(paths.agent_session_dir(project, &id));
        }
        opened
    }

    pub(super) fn new_chat(&mut self) {
        if let Some(reason) = self.switch_blocker("starting a new chat") {
            self.warn(reason);
            self.touch();
            return;
        }
        // Nothing was said in this conversation: a new chat would only add an empty session.
        if !self.store.has_older()
            && !self
                .store
                .resident()
                .iter()
                .any(|row| matches!(row.kind, RowKind::User { .. }))
        {
            return;
        }
        match self.begin_session() {
            Ok(next) => self.replace_conversation(next),
            Err(error) => self.warn(format!("A new chat could not be started: {error}")),
        }
        self.touch();
    }

    /// Switches to the stored session `id`: its log becomes the transcript (the stored log is
    /// the only source of rows) and its agent session is resumed when possible.
    pub(super) fn open_session(&mut self, id: &str) {
        if let Some(reason) = self.switch_blocker("opening another session") {
            self.warn(reason);
            self.touch();
            return;
        }
        let (paths, project) = (&self.shared.paths, &self.shared.project);
        if SessionStore::current_session_id(paths, project).as_deref() == Some(id) {
            return;
        }
        let known = SessionStore::session_log_path(paths, project, id)
            .is_ok_and(|_| paths.agent_session_dir(project, id).is_dir());
        if !known {
            self.warn(format!(
                "The session could not be opened: unknown session `{id}`"
            ));
            self.touch();
            return;
        }
        let manifest = SessionStore::session_manifest(paths, project, id);
        let refused = match SessionStore::manifest_status(paths, project, id) {
            ManifestStatus::Refused(reason) => Some(reason),
            _ => None,
        };
        let switched = self.leave_conversation().and_then(|floor| {
            let next = self.open_session_log(id, floor)?;
            SessionStore::set_current(&self.shared.paths, &self.shared.project, id)?;
            Ok(next)
        });
        match switched {
            Ok(next) => {
                self.replace_conversation(next);
                self.resume_or_read_only(manifest, refused);
            }
            Err(error) => self.warn(format!("The session could not be opened: {error}")),
        }
        self.touch();
    }

    /// After a switch: queues the stored agent session for resumption when the agent can
    /// resume or load sessions, and otherwise says that the stored rows are read-only. Opening a
    /// session adds nothing to its log: the outcome is shown through the restoration view, and
    /// a row is written only when something happens (the resume, or the fresh agent start).
    /// Nothing is launched here; the next prompt starts the agent.
    fn resume_or_read_only(&mut self, manifest: Option<SessionManifest>, refused: Option<String>) {
        let Some(manifest) = manifest else {
            let text = match refused {
                Some(reason) => format!(
                    "{reason} Its stored messages are shown read-only and the next prompt starts a fresh agent session."
                ),
                None => "This session has no saved agent session to resume. Its stored messages are shown read-only and the next prompt starts a fresh agent session.".to_owned(),
            };
            self.session_restore = Some(Self::not_resumable_view(None, &text));
            return;
        };
        let view = if let Some(reason) = self.resume_blocker(&manifest) {
            let text = format!(
                "This session cannot be resumed: {reason}. Its stored messages are shown read-only and the next prompt starts a fresh agent session."
            );
            Self::not_resumable_view(Some(&manifest), &text)
        } else {
            match self.queue_resume(&manifest) {
                Ok(_) => SessionRestoreView {
                    provider_id: manifest.provider_id.clone(),
                    redacted_session_id: manifest.redacted_id(),
                    is_resumable: true,
                    last_source_revision: manifest.last_source_revision.clone(),
                    notice: Some(
                        "Saved session found; adapter compatibility will be checked before launch."
                            .to_owned(),
                    ),
                },
                Err(reason) => Self::not_resumable_view(Some(&manifest), &reason),
            }
        };
        self.session_restore = Some(view);
    }
}
