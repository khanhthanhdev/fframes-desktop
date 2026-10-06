//! Promotion of validated candidates: Apply, Undo, review policy and recovery control.
//!
//! Everything here runs under `&mut Controller`, which is the project mutation gate: a
//! second Apply, Undo, asset import or reconcile cannot interleave with a publication.
//! Source writes only happen through [`crate::edit_transaction`]'s durable protocol; the
//! in-memory task history and the SQLite projection advance only after the journal holds
//! the durable commit.
use super::Controller;
use crate::{
    AgentTaskId, EngineError, PromotionAuthorization, ReviewPolicy, SavedHistoryFence, TaskError,
    TaskIdentity, TaskSourceBase, TaskState, UndoStatus,
    candidate_validation::{CapturedCandidate, NextStep, ValidationReport, capture_undo_candidate},
    edit_transaction::{
        ApplyGate, Boundary, Executor, Fault, FileState, Interrupt, Outcome, PlanError,
        PromotionError, RecordSeed, TaskEvent, TaskRevisionRecord, TransactionIntent,
        TransactionKind, invert, mode_conflicts, plan, prompt_summary, touched_conflicts,
    },
};
use sha2::{Digest, Sha256};
use std::{fs, sync::Arc};
use studio_project::lifecycle::atomic_write;

/// A published task revision.
#[derive(Debug, Clone)]
pub struct Promotion {
    /// The durable history entry; the source now equals `record.published`.
    pub record: TaskRevisionRecord,
    /// The fresh install authorization for a staged preview of the published bytes.
    /// `None` when the source changed again before it could be issued; the accepted
    /// source then needs an ordinary preview build.
    pub authorization: Option<PromotionAuthorization>,
    /// Non-fatal observations (a retained recovery slot, a stale projection, ...).
    pub notes: Vec<String>,
}

/// What [`Controller::complete_validated_task`] did with a validation report.
#[derive(Debug)]
pub enum CompletionOutcome {
    /// The report passed and the project's policy applied it.
    Applied(Box<Promotion>),
    /// The report passed; manual review keeps the candidate until
    /// [`Controller::apply_candidate`] is called.
    AwaitingReview,
    /// Repair, retention or failure routing (see [`NextStep`]); nothing was published.
    Routed(NextStep),
}

/// An Undo candidate: the current source with the inverse of one accepted revision
/// applied, ready for the Stage 2 validation pipeline.
///
/// It is bound to the controller session that formed it (the project, the open session
/// and the task and saved-history fences it saw) and refuses to publish anywhere else
/// or after any of them moved.
#[derive(Debug)]
pub struct UndoPreparation {
    target: String,
    summary: String,
    captured: CapturedCandidate,
    expected_history: Option<String>,
    saved: SavedHistoryFence,
    remove_dirs: Vec<String>,
    reverses: Vec<crate::FileDelta>,
}

impl UndoPreparation {
    /// The history entry being reversed.
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn summary(&self) -> &str {
        &self.summary
    }
    /// The candidate to validate (`run_candidate_validation` / `validate_candidate`).
    pub fn captured(&self) -> &CapturedCandidate {
        &self.captured
    }
}

struct Publication<'a> {
    captured: &'a CapturedCandidate,
    report: &'a ValidationReport,
    seed: RecordSeed,
    expected_history: Option<String>,
    saved: SavedHistoryFence,
}

fn authorizes(
    report: &ValidationReport,
    captured: &CapturedCandidate,
) -> Result<(), PromotionError> {
    let refuse = |why: &str| Err(PromotionError::Unauthorized(why.to_owned()));
    if !report.passed() {
        return refuse("the validation report did not pass");
    }
    if let Some(gap) = report.acceptance_gap() {
        return Err(PromotionError::Unauthorized(gap));
    }
    if report.task() != captured.identity() {
        return refuse("the report belongs to another task");
    }
    if report.candidate() != captured.candidate() {
        return refuse("the report validated another candidate revision");
    }
    if report.source_base() != captured.source_base() {
        return refuse("the report was formed from another source base");
    }
    if report.manifest_sha256() != captured.manifest_sha256() {
        return refuse("the report names another checkpoint manifest");
    }
    Ok(())
}

impl Controller {
    // ---- policy / status --------------------------------------------------------------

    /// The project's review policy (default: automatic Apply after validation). App-local:
    /// see [`ReviewPolicy::SCOPE_NOTICE`].
    pub fn review_policy(&self) -> ReviewPolicy {
        self.record.state.review_policy()
    }

    pub fn set_review_policy(&mut self, policy: ReviewPolicy) -> Result<(), EngineError> {
        let mut record = self.record.clone();
        record.state.set_review_policy(policy);
        self.persist(record)
    }

    /// Whether this platform and the project's filesystem proved atomic no-replace
    /// publication (`renameat2`). Auto-Apply must stay disabled unless this is `Ready`.
    pub fn apply_gate(&mut self) -> &ApplyGate {
        if self.apply_gate.is_none() {
            self.apply_gate = Some(match self.mechanism_override {
                // A filesystem with only the link-based pair cannot move a live name safely.
                Some(crate::edit_transaction::NoClobber::Link) => {
                    ApplyGate::Blocked(crate::edit_transaction::LINK_REFUSED.into())
                }
                _ => crate::edit_transaction::probe_apply_gate(&self.project.root),
            });
        }
        self.apply_gate.as_ref().expect("just probed")
    }

    /// Probes again (after the user changed the folder's filesystem or permissions).
    pub fn refresh_apply_gate(&mut self) -> &ApplyGate {
        self.apply_gate = None;
        self.apply_gate()
    }

    /// Administratively disables (or restores) Apply and Undo, for example when the
    /// transaction layer is rolled back: `Blocked` refuses every new publication while
    /// task history, recovery of unfinished transactions and checkpoint exports keep
    /// working. `None` returns to the real filesystem probe.
    pub fn override_apply_gate(&mut self, gate: Option<ApplyGate>) {
        self.apply_gate = gate;
    }

    /// Simulates a filesystem that offers only `mechanism`: `Link` (the two-syscall
    /// link-then-unlink pair, which can unlink an editor's replacement) blocks the
    /// gate, `Renameat2` is the real probe. `None` restores the real probe.
    pub fn force_publication_mechanism(
        &mut self,
        mechanism: Option<crate::edit_transaction::NoClobber>,
    ) {
        self.mechanism_override = mechanism;
        self.apply_gate = None;
    }

    pub fn recovery_status(&self) -> &super::RecoveryStatus {
        &self.recovery_status
    }

    /// Fault-injection hooks for the next transactions (tests only in practice).
    pub fn set_transaction_hooks(&mut self, hooks: Arc<dyn crate::TransactionHooks>) {
        self.hooks = hooks;
    }

    /// Undo availability with an actionable reason when it is not available.
    pub fn undo_status(&mut self) -> UndoStatus {
        if let Some(reason) = &self.recovery_status.suspended {
            return UndoStatus::Unavailable {
                reason: format!("History storage failed ({reason}); reopen the project to recover"),
            };
        }
        if let Some(conflict) = self.recovery_status.unresolved.first() {
            return UndoStatus::Unavailable {
                reason: format!(
                    "An earlier edit stopped on a conflict ({conflict}); resolve it first"
                ),
            };
        }
        if let ApplyGate::Blocked(reason) = self.apply_gate() {
            return UndoStatus::Unavailable {
                reason: format!("Source publication is blocked: {reason}"),
            };
        }
        self.record.state.task_history().undo_status()
    }

    /// Refuses while a conflicted or suspended mutation is unresolved.
    pub(crate) fn ensure_mutable(&self) -> Result<(), EngineError> {
        if let Some(reason) = &self.recovery_status.suspended {
            return Err(PromotionError::Journal(reason.clone()).into());
        }
        if let Some(conflict) = self.recovery_status.unresolved.first() {
            return Err(PromotionError::Unresolved(conflict.to_string()).into());
        }
        Ok(())
    }

    /// Marks a conflicted transaction as resolved by the user (who inspected the retained
    /// variants). Nothing is deleted: the variants stay on disk under their exact
    /// app-generated names.
    pub fn resolve_conflict(&mut self, transaction: &str, note: &str) -> Result<(), EngineError> {
        let Some(position) = self
            .recovery_status
            .unresolved
            .iter()
            .position(|c| c.transaction == transaction)
        else {
            return Err(EngineError::Diagnostic(format!(
                "No unresolved conflict for transaction {transaction}"
            )));
        };
        self.tasks.append(
            transaction,
            &TaskEvent::Resolved {
                note: note.to_owned(),
            },
        )?;
        self.recovery_status.unresolved.remove(position);
        Ok(())
    }

    // ---- Apply ------------------------------------------------------------------------

    /// The engine-level entry for Stage 4: routes a validation report through
    /// [`Controller::agent_apply_validation`]; if it passes, applies it immediately
    /// (automatic policy) or leaves the task `CandidateReady` for an explicit
    /// [`Controller::apply_candidate`] (manual review).
    pub fn complete_validated_task(
        &mut self,
        captured: &CapturedCandidate,
        report: &ValidationReport,
    ) -> Result<CompletionOutcome, EngineError> {
        let identity = captured.identity().clone();
        let step = self.agent_apply_validation(&identity, report)?;
        match step {
            NextStep::Accept => match self.review_policy() {
                ReviewPolicy::AutoApply => {
                    let promotion = self.apply_candidate(captured, report)?;
                    Ok(CompletionOutcome::Applied(Box::new(promotion)))
                }
                ReviewPolicy::ManualReview => Ok(CompletionOutcome::AwaitingReview),
            },
            other => Ok(CompletionOutcome::Routed(other)),
        }
    }

    /// Publishes a validated candidate of the active task (`CandidateReady`) as an
    /// accepted task revision, or refuses with the candidate preserved.
    pub fn apply_candidate(
        &mut self,
        captured: &CapturedCandidate,
        report: &ValidationReport,
    ) -> Result<Promotion, EngineError> {
        self.apply_candidate_gated(captured, report, &|| true)
    }

    /// [`Controller::apply_candidate`] with an arbitration point: `enter` runs once,
    /// after every refusal check passed and immediately before the task moves to
    /// `Promoting` and the first durable mutation begins (the controller stays borrowed
    /// throughout). `false` declines with [`PromotionError::Declined`]: no source file,
    /// journal or history entry was written (only the content-addressed report copy may
    /// exist), the task stays `CandidateReady` and the candidate is preserved.
    pub fn apply_candidate_gated(
        &mut self,
        captured: &CapturedCandidate,
        report: &ValidationReport,
        enter: &dyn Fn() -> bool,
    ) -> Result<Promotion, EngineError> {
        self.ensure_mutable()?;
        let identity = captured.identity().clone();
        let task = self.agent.validate(&identity)?;
        if task.state() != TaskState::CandidateReady {
            return Err(TaskError::WrongState {
                expected: TaskState::CandidateReady,
                actual: task.state(),
            }
            .into());
        }
        authorizes(report, captured)?;
        if task.candidate() != Some(captured.candidate())
            || task.candidate_manifest() != Some(captured.manifest_sha256())
            || task.context().source_base != *captured.source_base()
        {
            return Err(TaskError::ReportMismatch.into());
        }
        let (expected_history, saved) = match &self.agent_fences {
            Some(fences) if fences.task == identity.task => {
                (fences.history_head.clone(), fences.saved.clone())
            }
            _ => return Err(TaskError::StaleIdentity.into()),
        };
        let context = task.context();
        let seed = RecordSeed {
            kind: TransactionKind::Apply,
            task: identity.task.0.clone(),
            generation: identity.generation,
            session: identity.session.clone(),
            task_base: captured.source_base().revision().clone(),
            prior_history: expected_history.clone(),
            prior_checkpoint: context.prior_checkpoint.clone(),
            undoes: None,
            prompt_summary: prompt_summary(&context.brief),
            build: report.build().cloned(),
            validation_report_sha256: String::new(),
            remove_dirs: Vec::new(),
            reverses: Vec::new(),
            preset: None,
        };
        let publication = Publication {
            captured,
            report,
            seed,
            expected_history,
            saved,
        };
        let intent = match self.preflight(&publication) {
            Ok(intent) => intent,
            Err(error) => {
                // A moved source or history, or an unplannable delta, ends the task
                // with the candidate and draft preserved. A blocked gate or unresolved
                // mutation leaves it ready so the user can retry after fixing that.
                let end = match &error {
                    EngineError::Promotion(
                        PromotionError::SourceChanged { .. }
                        | PromotionError::HistoryChanged
                        | PromotionError::SavedHistoryChanged
                        | PromotionError::Plan(PlanError::Conflict { .. }),
                    ) => Some(TaskState::Conflict),
                    EngineError::Promotion(PromotionError::Plan(
                        PlanError::NoChange | PlanError::TooManyChanges(_),
                    ))
                    | EngineError::Project(_)
                    | EngineError::Candidate(_) => Some(TaskState::Failed),
                    _ => None,
                };
                if let Some(state) = end {
                    // The task may already have ended (a failed rescan interrupts it);
                    // the original error is the one to report.
                    if self
                        .agent
                        .transition(&identity, TaskState::Promoting)
                        .is_ok()
                        || state != TaskState::Conflict
                    {
                        let _ = self.finish_agent_task(&identity, state, &error.to_string());
                    }
                }
                return Err(error);
            }
        };
        if !enter() {
            return Err(PromotionError::Declined.into());
        }
        self.agent
            .transition(&identity, TaskState::Promoting)
            .map_err(EngineError::from)?;
        match self.execute(&intent, identity.generation) {
            Ok(mut promotion) => {
                // The commit is durable: whatever the task bookkeeping does now, the
                // caller must still receive the committed promotion (with its install
                // authorization) to label and rebuild the accepted revision. A
                // bookkeeping failure (the draft marker could not be written) is a
                // note, and it suspends further mutation: reopening recovers the draft
                // state.
                if let Err(error) =
                    self.finish_agent_task(&identity, TaskState::Accepted, "applied")
                {
                    let note = format!(
                        "the revision is committed, but the task's draft bookkeeping failed ({error}); source changes are suspended until the project is reopened"
                    );
                    self.recovery_status
                        .suspended
                        .get_or_insert_with(|| format!("draft bookkeeping: {error}"));
                    self.recovery_notice = Some(note.clone());
                    promotion.notes.push(note);
                }
                Ok(promotion)
            }
            Err(error) => {
                let end = match &error {
                    EngineError::Promotion(PromotionError::Conflict(_)) => {
                        Some(TaskState::Conflict)
                    }
                    EngineError::Promotion(PromotionError::RolledBack(_)) => {
                        Some(TaskState::Failed)
                    }
                    EngineError::Promotion(PromotionError::Journal(_)) => {
                        Some(TaskState::Interrupted)
                    }
                    // A crashed process ends nothing: recovery decides on reopen.
                    EngineError::Promotion(PromotionError::Crashed) => None,
                    _ => Some(TaskState::Failed),
                };
                if let Some(state) = end {
                    let _ = self.finish_agent_task(&identity, state, &error.to_string());
                }
                Err(error)
            }
        }
    }

    // ---- Undo -------------------------------------------------------------------------

    /// Forms the Undo candidate for `target` (default: the newest agent edit still in
    /// effect): verifies only the paths that edit touched still hold its after state,
    /// captures the current full source, applies the inverse delta and captures the
    /// merged result. Unrelated edits are preserved. The candidate must then pass the
    /// Stage 2 validation before [`Controller::undo_task`] publishes it.
    pub fn prepare_undo(&mut self, target: Option<&str>) -> Result<UndoPreparation, EngineError> {
        self.ensure_mutable()?;
        if let ApplyGate::Blocked(reason) = self.apply_gate().clone() {
            return Err(PromotionError::GateBlocked(reason).into());
        }
        self.reconcile()?;
        let entry = self
            .record
            .state
            .task_history()
            .undo_target(target)
            .map_err(PromotionError::UndoUnavailable)?
            .clone();
        let current = self.project.inventory.clone();
        let mut conflicts = touched_conflicts(&entry.changes, &current);
        // Content can match while the permission bits do not: the exact inverse is only
        // guarded where the file still has the bits the edit left.
        for found in mode_conflicts(&self.project.root, &entry.changes) {
            if !conflicts.iter().any(|c| c.path == found.path) {
                conflicts.push(found);
            }
        }
        if !conflicts.is_empty() {
            return Err(PromotionError::UndoConflict(conflicts).into());
        }
        // The current full source, as immutable objects.
        let base = self.checkpoints.capture(&self.project.root)?;
        if base != current.revision {
            return Err(crate::StateError::StaleResult.into());
        }
        let draft = self
            .checkpoints
            .draft(&base, &format!("undo-{}", uuid::Uuid::new_v4().simple()))?;
        let formed = (|| -> Result<CapturedCandidate, EngineError> {
            // Dependency order: deletions first, then the directories they emptied, then
            // the writes. A file may become a directory (or the reverse) in one edit, so
            // a path can only be written after whatever occupied it is gone.
            let inverse = invert(&entry.changes);
            for change in inverse.iter().filter(|c| c.after.is_none()) {
                let target = draft.join(&change.path);
                match fs::remove_file(&target) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e.into()),
                }
                // The inventory has no directories: an emptied one must go so a file
                // can take its name.
                let mut parent = target.parent();
                while let Some(dir) = parent
                    && dir != draft
                    && dir.starts_with(&draft)
                    && fs::remove_dir(dir).is_ok()
                {
                    parent = dir.parent();
                }
            }
            for change in inverse.iter().filter(|c| c.after.is_some()) {
                let state = change.after.as_ref().expect("filtered");
                self.write_object(&draft.join(&change.path), state)?;
            }
            let generation = self.agent.allocate_generation()?;
            let identity = TaskIdentity {
                task: AgentTaskId::new(),
                project: self.project.manifest.project_id.clone(),
                session: self.record.state.session().clone(),
                generation,
            };
            let source_base = TaskSourceBase::new(base.clone());
            Ok(capture_undo_candidate(
                &identity,
                &source_base,
                &draft,
                &self.checkpoints,
            )?)
        })();
        let _ = fs::remove_dir_all(&draft);
        let captured = formed?;
        if captured.candidate().revision() == &base {
            return Err(PromotionError::UndoUnavailable(
                "undoing this edit would not change the current source".into(),
            )
            .into());
        }
        Ok(UndoPreparation {
            summary: format!("Undo: {}", entry.prompt_summary),
            target: entry.id,
            captured,
            expected_history: self.record.state.task_history().head_id(),
            saved: self.record.state.saved_history_fence(),
            remove_dirs: entry.created_dirs,
            reverses: entry.changes,
        })
    }

    fn write_object(&self, target: &std::path::Path, state: &FileState) -> Result<(), EngineError> {
        fs::create_dir_all(target.parent().expect("draft paths have parents"))?;
        match fs::remove_file(target) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        self.checkpoints
            .copy_object(&state.sha256, state.size, &mut file)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                target,
                fs::Permissions::from_mode(if state.executable { 0o755 } else { 0o644 }),
            )?;
        }
        Ok(())
    }

    /// Publishes a validated Undo candidate as a new task-history transition. The
    /// source, history and validation fences are rechecked first; any change since
    /// [`Controller::prepare_undo`] refuses and preserves everything.
    pub fn undo_task(
        &mut self,
        preparation: &UndoPreparation,
        report: &ValidationReport,
    ) -> Result<Promotion, EngineError> {
        self.undo_task_gated(preparation, report, &|| true)
    }

    /// [`Controller::undo_task`] with an arbitration point: `enter` runs once, after every
    /// refusal check passed and immediately before the first durable mutation (the
    /// controller stays borrowed throughout). `false` declines with
    /// [`PromotionError::Declined`]; nothing was written and the task is untouched.
    pub fn undo_task_gated(
        &mut self,
        preparation: &UndoPreparation,
        report: &ValidationReport,
        enter: &dyn Fn() -> bool,
    ) -> Result<Promotion, EngineError> {
        self.ensure_mutable()?;
        // The preparation belongs to the controller session that formed it. A reopened
        // or foreign controller never publishes it, and nothing (report, intent,
        // source) is touched before that is known.
        let captured = &preparation.captured;
        let identity = captured.identity();
        if identity.project != *self.record.state.project()
            || identity.session != *self.record.state.session()
        {
            return Err(PromotionError::StalePreparation(
                "it was prepared in another session or project; prepare the Undo again".into(),
            )
            .into());
        }
        if self.record.state.task_history().head_id() != preparation.expected_history {
            return Err(PromotionError::HistoryChanged.into());
        }
        if self.record.state.saved_history_fence() != preparation.saved {
            return Err(PromotionError::SavedHistoryChanged.into());
        }
        authorizes(report, captured)?;
        let seed = RecordSeed {
            kind: TransactionKind::Undo,
            task: identity.task.0.clone(),
            generation: identity.generation,
            session: identity.session.clone(),
            task_base: captured.source_base().revision().clone(),
            prior_history: preparation.expected_history.clone(),
            prior_checkpoint: self.record.state.accepted().clone(),
            undoes: Some(preparation.target.clone()),
            prompt_summary: preparation.summary.clone(),
            build: report.build().cloned(),
            validation_report_sha256: String::new(),
            remove_dirs: preparation.remove_dirs.clone(),
            reverses: preparation.reverses.clone(),
            preset: None,
        };
        let publication = Publication {
            captured,
            report,
            seed,
            expected_history: preparation.expected_history.clone(),
            saved: preparation.saved.clone(),
        };
        let intent = self.preflight(&publication)?;
        if !enter() {
            return Err(PromotionError::Declined.into());
        }
        self.execute(&intent, identity.generation)
    }

    // ---- shared publication ----------------------------------------------------------

    /// Everything that can be refused without changing anything: fences, verified
    /// objects, the immutable report and the transaction plan.
    fn preflight(&mut self, p: &Publication<'_>) -> Result<TransactionIntent, EngineError> {
        self.ensure_mutable()?;
        // Reconcile project identity and the current source under the mutation gate.
        self.reconcile()?;
        let current = self.project.inventory.clone();
        let expected = p.captured.source_base().revision();
        if &current.revision != expected {
            return Err(PromotionError::SourceChanged {
                expected: expected.as_str()[..12].to_owned(),
                found: current.revision.as_str()[..12].to_owned(),
            }
            .into());
        }
        if self.record.state.task_history().head_id() != p.expected_history {
            return Err(PromotionError::HistoryChanged.into());
        }
        // The saved (checkpoint) history is a separate fence: a checkpoint taken while
        // the candidate was being validated changes what "prior checkpoint" means.
        if self.record.state.saved_history_fence() != p.saved {
            return Err(PromotionError::SavedHistoryChanged.into());
        }
        if let ApplyGate::Blocked(reason) = self.apply_gate().clone() {
            return Err(PromotionError::GateBlocked(reason).into());
        }
        // Before and after objects must both be complete and intact.
        self.checkpoints.load(&current.revision)?;
        let candidate = self.checkpoints.load(p.captured.candidate().revision())?;
        let hash = self.persist_report(p.report)?;
        let mut seed = p.seed.clone();
        seed.validation_report_sha256 = hash;
        Ok(plan(
            &self.project.root,
            &current,
            &candidate,
            seed,
            self.mechanism_override,
        )
        .map_err(PromotionError::from)?)
    }

    /// Stores the immutable report content-addressed under the project's history.
    fn persist_report(&self, report: &ValidationReport) -> Result<String, EngineError> {
        let bytes = serde_json::to_vec(report)?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let dir = self.checkpoints.root.join("reports");
        fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{hash}.json"));
        if !path.exists() {
            atomic_write(&path, &bytes)?;
        }
        Ok(hash)
    }

    fn execute(
        &mut self,
        intent: &TransactionIntent,
        generation: u64,
    ) -> Result<Promotion, EngineError> {
        let record = self.run_intent(intent)?;
        self.after_commit(&record, generation)
    }

    /// Runs one planned file-set transaction through the durable executor and maps its
    /// terminal outcome. Returns the committed record; the caller decides what the
    /// commit means (task history for Apply/Undo, nothing for a Studio preset change).
    pub(super) fn run_intent(
        &mut self,
        intent: &TransactionIntent,
    ) -> Result<TaskRevisionRecord, EngineError> {
        let hooks = self.hooks.clone();
        let outcome = {
            let mut executor = Executor::new(
                &self.project.root,
                &self.checkpoints,
                &mut self.tasks,
                &*hooks,
            )?;
            executor.execute(intent)
        };
        match outcome {
            Ok(Outcome::Committed(record)) => {
                // The commit is durable, but a failed bookkeeping append after it (the
                // journal then refuses everything) must not look like a healthy journal.
                if self.tasks.is_failed() {
                    let message = "the task journal failed after the commit; reopen the project to collect recovery files";
                    self.recovery_status.suspended = Some(message.into());
                    self.recovery_notice =
                        Some(format!("{message}. Source changes are suspended."));
                }
                Ok(*record)
            }
            Ok(Outcome::RolledBack(reason)) => Err(PromotionError::RolledBack(reason).into()),
            Ok(Outcome::Conflicted(report)) => {
                self.recovery_status.unresolved.push((*report).clone());
                Err(PromotionError::Conflict(report).into())
            }
            Err(Interrupt::Crash) => {
                self.recovery_status.suspended = Some("interrupted at an injected boundary".into());
                Err(PromotionError::Crashed.into())
            }
            Err(Interrupt::Journal(message)) => {
                self.recovery_status.suspended = Some(message.clone());
                self.recovery_notice = Some(format!(
                    "Task journal failed: {message}. Source changes are suspended; reopen the project to recover."
                ));
                Err(PromotionError::Journal(message).into())
            }
        }
    }

    /// A controller-side boundary. An injected crash suspends the controller like a
    /// process death; an injected I/O fault is returned for the caller to handle.
    pub(super) fn controller_boundary(
        &mut self,
        boundary: Boundary,
    ) -> Result<Option<String>, EngineError> {
        match self.hooks.at(&boundary) {
            Ok(()) => Ok(None),
            Err(Fault::Crash) => {
                self.recovery_status.suspended = Some("interrupted at an injected boundary".into());
                Err(PromotionError::Crashed.into())
            }
            Err(Fault::Io(kind)) => Ok(Some(format!("injected {kind:?} at {boundary:?}"))),
        }
    }

    /// The commit is durable. Only now does in-memory state and the SQLite projection
    /// advance; a failure here leaves the acceptance in place and the projection to be
    /// rebuilt from the journal.
    fn after_commit(
        &mut self,
        record: &TaskRevisionRecord,
        generation: u64,
    ) -> Result<Promotion, EngineError> {
        let mut notes = Vec::new();
        self.record.state.record_task_revision(record.clone());
        if let Some(fault) = self.controller_boundary(Boundary::Database { after: false })? {
            self.projection_stale = true;
            notes.push(format!("history database not updated: {fault}"));
        } else {
            // Rescans the source (it is now the published revision), invalidating
            // everything derived from the old source, and persists the record.
            if let Err(error) = self.reconcile() {
                notes.push(format!("source rescan after publication failed: {error}"));
            }
            if let Err(error) = self.persist(self.record.clone()) {
                self.projection_stale = true;
                notes.push(format!("history database not updated: {error}"));
            }
            self.controller_boundary(Boundary::Database { after: true })?;
        }
        if let Some(fault) = self.controller_boundary(Boundary::Projection { after: false })? {
            self.projection_stale = true;
            notes.push(format!("task projection not updated: {fault}"));
        } else {
            let entries = self.record.state.task_history().entries().to_vec();
            match self
                .store
                .replace_task_revisions(self.record.state.project(), &entries)
            {
                Ok(()) => (),
                Err(error) => {
                    self.projection_stale = true;
                    notes.push(format!("task projection not updated: {error}"));
                }
            }
            self.controller_boundary(Boundary::Projection { after: true })?;
        }
        // The crash-injection path above never reaches here; a stale projection is
        // retried by the next persist.
        let authorization = self
            .record
            .state
            .authorize_promotion(record, generation)
            .ok();
        if let Some(authorization) = &authorization {
            self.record.state.set_promotion(authorization.clone());
        }
        Ok(Promotion {
            record: record.clone(),
            authorization,
            notes,
        })
    }
}
