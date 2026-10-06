use crate::{
    AgentTask, AgentTaskContext, AgentTaskId, AgentTaskManager, CaptureTicket, DraftState,
    DraftStore, EngineError, FinishedTask, JobKind, JobResult, JobState, OpenSession, OperationTag,
    ProjectState, QuiescenceEvidence, RepairDecision, TaskError, TaskIdentity, TaskSourceBase,
    TaskState, WriterGeneration, WriterGoneEvidence, WriterObservation,
    agent_task::{ScopeFacts, validate_brief},
    app_paths::AppPaths,
    candidate_validation::{
        CapturedCandidate, FailureKind, NextStep, ValidationReport,
        capture_candidate_scoped_observed, next_step,
    },
    edit_transaction::{
        ApplyGate, ConflictReport, Executor, Interrupt, NoHooks, RecoveryReport, TransactionHooks,
    },
    journal::{Journal, TaskJournal},
    store::{Record, Store},
};
use notify::Watcher;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use studio_project::{OpenProject, checkpoint::Checkpoints, revision::FileKind};

mod preset;
mod promotion;
pub use preset::{PresetMutation, PresetOutcome};
pub use promotion::{CompletionOutcome, Promotion, UndoPreparation};

/// What replaying the task journal found and did on open, and what still blocks
/// source mutation.
#[derive(Debug, Clone, Default)]
pub struct RecoveryStatus {
    pub report: RecoveryReport,
    /// Conflicted transactions awaiting an explicit resolution. While any exists, new
    /// Apply, Undo and asset-import mutations are refused.
    pub unresolved: Vec<ConflictReport>,
    /// Set when a durable journal (task or lifecycle) failed, or a fault was injected:
    /// no further source mutation until the project is reopened and recovered. A
    /// failing SQLite projection alone is only a note, never a suspension.
    pub suspended: Option<String>,
    /// The SQLite task-revision projection was missing or stale and was rebuilt from the
    /// journal.
    pub projection_rebuilt: bool,
}

/// The fences a task freezes at launch, each rechecked separately before Apply.
#[derive(Debug, Clone)]
pub(crate) struct TaskFences {
    pub(crate) task: AgentTaskId,
    pub(crate) history_head: Option<String>,
    pub(crate) saved: crate::SavedHistoryFence,
}

/// A single background owner per project. Every completion rescans; watcher events are hints only.
pub struct Controller {
    pub project: OpenProject,
    record: Record,
    store: Store,
    journal: Journal,
    /// Durable file-set transactions (Apply/Undo); the source of truth for task history.
    tasks: TaskJournal,
    hooks: Arc<dyn TransactionHooks>,
    apply_gate: Option<ApplyGate>,
    mechanism_override: Option<crate::edit_transaction::NoClobber>,
    recovery_status: RecoveryStatus,
    /// What the active agent task's launch froze: the task-history head and the saved
    /// (checkpoint) history.
    agent_fences: Option<TaskFences>,
    projection_stale: bool,
    pub checkpoints: Checkpoints,
    _lock: File,
    watcher: Option<notify::RecommendedWatcher>,
    dirty: Arc<AtomicBool>,
    pub history_was_available: bool,
    pub recovery_notice: Option<String>,
    pub processes: studio_bootstrap::ProcessTreeManager,
    operations: studio_bootstrap::ProcessTreeManager,
    /// Process scope of the active (or last) agent task. Every task owns its own scope,
    /// created only once its lease and draft are installed and sealed when it ends.
    agent_scope: Option<AgentScope>,
    agent: AgentTaskManager,
    drafts: DraftStore,
}

/// The process scope of one agent task, keyed by the task that owns it so cleanup can
/// never reach a successor's processes.
struct AgentScope {
    owner: AgentTaskId,
    processes: studio_bootstrap::ProcessTreeManager,
}

impl Controller {
    pub fn open(root: &Path, paths: &AppPaths) -> Result<Self, EngineError> {
        Self::open_with_hooks(root, paths, Arc::new(NoHooks))
    }

    /// [`Self::open`] with fault-injection hooks that also observe crash recovery.
    pub fn open_with_hooks(
        root: &Path,
        paths: &AppPaths,
        hooks: Arc<dyn TransactionHooks>,
    ) -> Result<Self, EngineError> {
        fn open_source(
            root: &Path,
        ) -> Result<(OpenProject, Option<String>), studio_project::ProjectError> {
            match studio_project::open(root) {
                Ok(p) => Ok((p, None)),
                Err(e) => {
                    let err_msg = e.to_string();
                    let recovery = studio_project::open_for_recovery(root).map_err(|_| e)?;
                    Ok((recovery, Some(err_msg)))
                }
            }
        }
        let (mut project, mut source_degraded) = open_source(root)?;
        if paths.contains_source(&project.root) {
            return Err(EngineError::Diagnostic(
                "App data must be outside portable source; choose a separate project folder".into(),
            ));
        }
        let history = paths.project(&project.manifest.project_id);
        fs::create_dir_all(&history)?;
        studio_project::lifecycle::sync_directory(history.parent().unwrap())?;
        studio_project::lifecycle::sync_directory(&paths.data)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(history.join("owner.lock"))?;
        lock.try_lock().map_err(|_| {
            EngineError::Diagnostic(
                "Project is already open in another controller; close it there first".into(),
            )
        })?;
        // Compatibility and identity are decided from read-only looks at the database
        // and both journals before anything is migrated, quarantined or replayed: an
        // unsupported (newer or damaged) history, a foreign identity or a duplicate
        // project ID is refused with every byte of the app-local history untouched.
        let previous =
            Self::preflight_history(&paths.database(), &history, &project.manifest.project_id)?;
        if let Some(previous) = &previous {
            if previous.state.project() != &project.manifest.project_id {
                return Err(EngineError::Diagnostic(
                    "History project identity mismatch; repair local history".into(),
                ));
            }
            if previous.location != project.root && previous.location.exists() {
                return Err(EngineError::Diagnostic(format!(
                    "Duplicate project ID: original still exists at {}. Open the original or explicitly assign this copy an independent identity",
                    previous.location.display()
                )));
            }
        }
        let mut store = Store::open(&paths.database())?;
        let (mut journal, replay) = Journal::open(history.join("lifecycle.jsonl"))?;
        let checkpoints = Checkpoints::new(&history)?;
        // Transactions are replayed before normal source state is trusted: an
        // unfinished Apply/Undo is rolled back where provably safe, never rolled forward.
        let (mut tasks, task_replay) = TaskJournal::open(history.join("tasks.jsonl"))?;
        let recovery = {
            let mut executor = Executor::new(&project.root, &checkpoints, &mut tasks, &*hooks)?;
            executor
                .recover(&task_replay.transactions)
                .map_err(|interrupt| match interrupt {
                    Interrupt::Crash => EngineError::from(crate::PromotionError::Crashed),
                    Interrupt::Journal(message) => EngineError::Diagnostic(format!(
                        "Task journal during recovery: {message}; repair storage and reopen"
                    )),
                })?
        };
        if recovery.touched_source {
            (project, source_degraded) = open_source(root)?;
        }
        let task_records = task_replay.committed();
        let history_was_available = previous.is_some();
        if let Some(previous) = &previous {
            // A relocated project must still hold what the history last saw (judged
            // after recovery, which may have rolled an interrupted edit back).
            if previous.location != project.root
                && !project.inventory.matches_revision(previous.state.source())
            {
                return Err(EngineError::Diagnostic("Relocated project content differs from last known source; preserve both and explicitly choose an independent copy".into()));
            }
            checkpoints.load(previous.state.accepted())?;
        }
        let mut record = if let Some(previous) = previous {
            Record {
                state: previous.state.reopened(project.inventory.revision.clone()),
                location: project.root.clone(),
                name: project.manifest.display.name.clone(),
                ..previous
            }
        } else {
            let accepted = checkpoints.capture(&project.root)?;
            let mut state = ProjectState::opening(
                project.manifest.project_id.clone(),
                OpenSession::new(),
                project.inventory.revision.clone(),
                accepted,
            );
            state.finish_open(Ok(()))?;
            Record {
                location: project.root.clone(),
                name: project.manifest.display.name.clone(),
                state,
                draft: None,
                sdk_path: None,
            }
        };
        let projection_rebuilt = match store.task_revisions(&project.manifest.project_id) {
            Ok(rows) => rows != task_records,
            Err(_) => true,
        };
        // The task journal is authoritative for task history: it is rebuilt below,
        // after every other substitution of the lifecycle state.
        // An uncommitted job intent may have published a draft; never advance accepted from it.
        if let Some(pending) = replay.pending
            && matches!(
                pending.state.job(),
                JobState::Queued(_) | JobState::Running(_) | JobState::CancelRequested(_)
            )
        {
            let mut recovered = pending.state.reopened(project.inventory.revision.clone());
            // Pending job intents do not alter accepted; check that invariant against committed history.
            if recovered.accepted() != record.state.accepted() {
                recovered = record.state.clone();
            }
            record.state = recovered;
            record.draft = pending.draft;
        }
        record
            .state
            .set_task_history(crate::TaskHistory::from_records(task_records.clone()));
        let tx = journal.intent(&record)?;
        journal.commit(&tx)?;
        store.save(&record)?;
        if projection_rebuilt {
            store.replace_task_revisions(&project.manifest.project_id, &task_records)?;
        }
        let dirty = Arc::new(AtomicBool::new(false));
        let flag = dirty.clone();
        let mut watcher =
            notify::recommended_watcher(move |event: Result<notify::Event, notify::Error>| {
                // Reading source during reconciliation must not invalidate its own
                // install fence. Writes still emit Create/Modify/Remove events.
                if !matches!(event, Ok(ref e) if matches!(e.kind, notify::EventKind::Access(_))) {
                    flag.store(true, Ordering::Release);
                }
            })
            .map_err(|e| {
                EngineError::Diagnostic(format!(
                    "Filesystem watcher: {e}; refresh manually and retry"
                ))
            })?;
        watcher
            .watch(&project.root, notify::RecursiveMode::Recursive)
            .map_err(|e| {
                EngineError::Diagnostic(format!(
                    "Watch {}: {e}; refresh manually and retry",
                    project.root.display()
                ))
            })?;
        let processes = studio_bootstrap::ProcessTreeManager::new();
        let operations = processes.sub_manager();
        let drafts = DraftStore::new(paths, &project.manifest.project_id);
        // A draft still marked active belongs to a session that never released it.
        drafts.recover_after_restart()?;
        let agent = AgentTaskManager::new(
            project.manifest.project_id.clone(),
            record.state.session().clone(),
        );
        let mut notices: Vec<String> = Vec::new();
        if let Some(p) = &replay.quarantined_tail {
            notices.push(format!(
                "Truncated journal tail preserved at {}",
                p.display()
            ));
        }
        if let Some(p) = &task_replay.quarantined_tail {
            notices.push(format!(
                "Truncated task journal tail preserved at {}",
                p.display()
            ));
        }
        if !recovery.rolled_back.is_empty() {
            notices.push(format!(
                "{} unfinished agent edit(s) were rolled back on open; the source is unchanged by them.",
                recovery.rolled_back.len()
            ));
        }
        if !recovery.conflicts.is_empty() {
            notices.push(format!(
                "{} agent edit(s) stopped on a conflict and retained every variant; source changes (Apply, Undo, imports) stay blocked until resolved.",
                recovery.conflicts.len()
            ));
        }
        if let Some(err) = &source_degraded {
            notices.push(format!(
                "Source degraded ({err}). Safe checkpoint export is available."
            ));
        }
        let recovery_notice = (!notices.is_empty()).then(|| notices.join(" "));
        let mut controller = Self {
            project,
            record,
            store,
            journal,
            tasks,
            hooks,
            apply_gate: None,
            mechanism_override: None,
            recovery_status: RecoveryStatus {
                unresolved: recovery.conflicts.clone(),
                report: recovery,
                suspended: None,
                projection_rebuilt,
            },
            agent_fences: None,
            projection_stale: false,
            checkpoints,
            _lock: lock,
            watcher: Some(watcher),
            dirty,
            history_was_available,
            recovery_notice,
            processes,
            operations,
            agent_scope: None,
            agent,
            drafts,
        };
        controller.heal_manifest_reference();
        Ok(controller)
    }
    /// Read-only look at everything app-local that decides whether this project can be
    /// opened: the database (version), the lifecycle journal and the task journal
    /// (formats, interior damage). Nothing is created, migrated, quarantined or
    /// replayed. Returns the last known record of the project.
    fn preflight_history(
        database: &Path,
        history: &Path,
        project: &studio_project::ProjectId,
    ) -> Result<Option<Record>, EngineError> {
        let lifecycle = Journal::inspect(&history.join("lifecycle.jsonl"))?;
        TaskJournal::inspect(&history.join("tasks.jsonl"))?;
        let stored = if database.exists() {
            let readable = Store::open_read_only(database)?;
            let version = readable.version()?;
            if version > crate::store::SCHEMA_VERSION {
                return Err(EngineError::NewerFormat {
                    what: format!("{} (history database)", database.display()),
                    found: version,
                    supported: crate::store::SCHEMA_VERSION,
                });
            }
            if version >= 1 {
                readable.get(project)?
            } else {
                None
            }
        } else {
            None
        };
        Ok(lifecycle.committed.or(stored))
    }

    /// Appends a lifecycle snapshot. Any failure (the journal refuses everything until
    /// reopened after a partial write) suspends source mutation for good: unlike a
    /// stale SQLite projection, an unrecoverable durable journal is not a note.
    fn lifecycle_intent(&mut self, record: &Record) -> Result<String, EngineError> {
        self.journal
            .intent(record)
            .inspect_err(|error| self.suspend_for_lifecycle(error))
    }

    fn lifecycle_commit(&mut self, transaction: &str) -> Result<(), EngineError> {
        self.journal
            .commit(transaction)
            .inspect_err(|error| self.suspend_for_lifecycle(error))
    }

    fn suspend_for_lifecycle(&mut self, error: &EngineError) {
        if self.recovery_status.suspended.is_none() {
            self.recovery_status.suspended = Some(format!("lifecycle journal: {error}"));
        }
        self.recovery_notice = Some(format!(
            "Lifecycle journal failed: {error}. Source changes are suspended; repair storage and reopen the project to recover."
        ));
    }

    /// Makes the next lifecycle append fail exactly like a full disk. Tests only.
    #[doc(hidden)]
    #[cfg(unix)]
    pub fn simulate_lifecycle_full_disk(&mut self) -> std::io::Result<()> {
        self.journal.simulate_full_disk()
    }

    pub fn state(&self) -> &ProjectState {
        &self.record.state
    }
    /// Compiler/candidate work is cancellable without terminating displayed workers.
    pub fn operation_processes(&self) -> studio_bootstrap::ProcessTreeManager {
        self.operations.clone()
    }
    pub fn draft(&self) -> Option<&Path> {
        self.record.draft.as_deref()
    }
    pub fn sdk_path(&self) -> Option<&Path> {
        self.record.sdk_path.as_deref()
    }
    /// Recovery exports a separate folder; current source and imported Git are never replaced.
    pub fn restore_as_copy(
        &mut self,
        destination: &Path,
        expected: &studio_project::SourceRevision,
    ) -> Result<(), EngineError> {
        self.reconcile()?;
        if self.state().source() != expected {
            return Err(crate::StateError::StaleResult.into());
        }
        self.export_checkpoint(self.state().accepted(), destination)
    }

    /// Safe checkpoint export that requires only an intact checkpoint in history, not healthy current source.
    pub fn export_checkpoint(
        &self,
        revision: &studio_project::SourceRevision,
        destination: &Path,
    ) -> Result<(), EngineError> {
        if destination.exists() {
            return Err(EngineError::Diagnostic(
                "Restore destination already exists; choose a new folder".into(),
            ));
        }
        let recovered = self
            .checkpoints
            .draft(revision, &format!("restore-{}", uuid::Uuid::new_v4()))?;
        studio_project::checkpoint::copy_draft(&recovered, destination)?;
        // A restored portable copy is independent of the original's mutable app-local history.
        studio_project::lifecycle::assign_independent_identity(destination)?;
        Ok(())
    }
    pub fn select_sdk(&mut self, path: PathBuf) -> Result<(), EngineError> {
        let manifest_path = path.join("compatibility.json");
        let json = fs::read_to_string(&manifest_path).map_err(|e| {
            EngineError::Diagnostic(format!(
                "{}: SDK manifest unavailable: {e}; select a complete compatible installed SDK",
                manifest_path.display()
            ))
        })?;
        let manifest = studio_sdk::CompatibilityManifest::from_json_str(&json).map_err(|e| {
            EngineError::Diagnostic(format!(
                "{}: SDK manifest: {e}; select a compatible installed SDK",
                manifest_path.display()
            ))
        })?;
        if self.project.manifest.sdk != crate::build_materialization::sdk_pin(&manifest) {
            return Err(EngineError::Diagnostic(
                "SDK pin mismatch; select the project's compatible SDK".into(),
            ));
        }
        let report = studio_sdk::Doctor::verify_candidate_sdk_with_processes(
            &path,
            &manifest,
            Some(&self.processes),
        );
        if !report.is_ready() {
            return Err(EngineError::Diagnostic(format!(
                "SDK unavailable: {}; repair prerequisites or select a complete SDK",
                report.format_summary()
            )));
        }
        let mut record = self.record.clone();
        record.sdk_path = Some(path);
        self.persist(record)
    }
    pub fn changed_hint(&self) -> bool {
        self.dirty.load(Ordering::Acquire)
    }
    pub fn reconcile(&mut self) -> Result<(), EngineError> {
        // Clear only when reconciliation actually starts. A busy native picker must
        // not consume the hint, and events arriving during the scan remain pending.
        self.dirty.store(false, Ordering::Release);
        let project = match studio_project::open(&self.project.root) {
            Ok(p) if p.manifest.project_id == *self.state().project() => p,
            result => {
                let identity_error = || {
                    EngineError::Diagnostic(
                        "Project ID changed externally; close and reopen explicitly".into(),
                    )
                };
                let error = match result {
                    Ok(_) => identity_error(),
                    Err(e) => match studio_project::open_for_recovery(&self.project.root) {
                        Ok(p) if p.manifest.project_id == *self.state().project() => {
                            self.record
                                .state
                                .reconcile_source(p.inventory.revision.clone())?;
                            self.project = p;
                            e.into()
                        }
                        Ok(_) => identity_error(),
                        Err(_) => e.into(),
                    },
                };
                // Invalidate in memory before persistence: even a history write failure
                // must not allow an edit-back to resurrect an old completion.
                self.record.state.invalidate_source()?;
                self.operations.shutdown(Duration::from_millis(300));
                // An agent task started from now-unreadable source is interrupted for good.
                self.interrupt_agent_task("project source became invalid", true);
                self.recovery_notice = Some(format!(
                    "Source reconciliation failed ({error}). Repair source and retry; safe checkpoint export is available."
                ));
                self.persist(self.record.clone())?;
                return Err(error);
            }
        };
        let previous = self.state().source().clone();
        self.record
            .state
            .reconcile_source(project.inventory.revision.clone())?;
        self.project = project;
        if self.state().source() != &previous {
            self.operations.shutdown(Duration::from_millis(300));
            self.persist(self.record.clone())?;
        }
        Ok(())
    }
    pub fn copy_asset(&mut self, source: &Path) -> Result<(), EngineError> {
        self.copy_asset_with_cancel(source, || false)
    }
    pub fn copy_asset_with_cancel(
        &mut self,
        source: &Path,
        cancelled: impl Fn() -> bool,
    ) -> Result<(), EngineError> {
        self.ensure_mutable()?;
        self.reconcile()?;
        if matches!(
            self.state().job(),
            JobState::Queued(_) | JobState::Running(_) | JobState::CancelRequested(_)
        ) {
            return Err(EngineError::Diagnostic(
                "Wait for or cancel the current operation before importing an asset".into(),
            ));
        }
        studio_project::assets::copy_asset_with_cancel(&self.project, source, cancelled)?;
        self.reconcile()
    }
    pub fn begin_job(&mut self, kind: JobKind) -> Result<OperationTag, EngineError> {
        self.begin_job_observed(kind, |_| {})
    }
    /// Boundary observer is for fault-injection tests; it runs after each durable boundary.
    pub fn begin_job_observed(
        &mut self,
        kind: JobKind,
        observe: impl Fn(&str),
    ) -> Result<OperationTag, EngineError> {
        self.reconcile()?;
        let mut record = self.record.clone();
        let tag = record.state.queue(kind)?;
        self.operations.shutdown(Duration::ZERO);
        self.operations = self.processes.sub_manager();
        let name = format!(
            "{}-{}",
            uuid::Uuid::from_bytes(tag.session.0),
            tag.operation.0
        );
        record.draft = Some(self.checkpoints.root.join("drafts").join(&name));
        let result: Result<(), EngineError> = (|| {
            let tx = self.lifecycle_intent(&record)?;
            observe("intent");
            let base = self.checkpoints.capture(&self.project.root)?;
            if base != tag.base_source {
                return Err(crate::StateError::StaleResult.into());
            }
            observe("objects");
            self.checkpoints.draft(&base, &name)?;
            observe("draft");
            self.lifecycle_commit(&tx)?;
            self.record = record.clone();
            observe("commit");
            self.store.save(&self.record)?;
            observe("database");
            let mut record = self.record.clone();
            record.state.start(&tag)?;
            self.persist(record)?;
            observe("running");
            Ok(())
        })();
        if let Err(error) = result {
            // A pre-commit failure may leave a partial draft. Retain the attempt
            // and settle it without advancing accepted or reusing its operation ID.
            if self.record.state.active_tag() != Some(&tag) {
                self.record = record;
            }
            self.fail_current_job(&tag, error.to_string())?;
            return Err(error);
        }
        Ok(tag)
    }
    pub fn complete(&mut self, tag: &OperationTag, result: JobResult) -> Result<(), EngineError> {
        self.reconcile()?;
        if let JobResult::Checkpointed(revision) = &result {
            self.checkpoints.load(revision)?;
        }
        let mut record = self.record.clone();
        record
            .state
            .complete(tag, self.project.inventory.revision.clone(), result)?;
        self.persist(record)
    }
    pub fn checkpoint(&mut self) -> Result<(), EngineError> {
        self.checkpoint_observed(|_| {})
    }
    pub fn checkpoint_observed(&mut self, observe: impl Fn(&str)) -> Result<(), EngineError> {
        let tag = self.begin_job_observed(JobKind::Checkpoint, &observe)?;
        observe("job_started");
        let result = self
            .checkpoints
            .capture(&self.project.root)
            .map_err(EngineError::from)
            .and_then(|revision| self.complete(&tag, JobResult::Checkpointed(revision)));
        if let Err(error) = result {
            self.fail_current_job(&tag, error.to_string())?;
            return Err(error);
        }
        Ok(())
    }

    fn fail_current_job(&mut self, tag: &OperationTag, reason: String) -> Result<(), EngineError> {
        if self.record.state.active_tag() != Some(tag) {
            return Ok(());
        }
        // Failure cannot install bytes. Settle queued as well as running jobs even
        // if source or history is unavailable, then attempt durable recovery state.
        self.record.state.fail(tag, reason)?;
        self.persist(self.record.clone())
    }
    pub fn cancel(&mut self) -> Result<(), EngineError> {
        let mut record = self.record.clone();
        record.state.request_cancel()?;
        self.persist(record)?;
        self.operations.shutdown(Duration::from_millis(300));
        let mut record = self.record.clone();
        record.state.interrupt()?;
        self.persist(record)
    }
    fn persist(&mut self, record: Record) -> Result<(), EngineError> {
        // The durable lifecycle journal first: its failure is a suspension.
        let tx = self.lifecycle_intent(&record)?;
        self.lifecycle_commit(&tx)?;
        self.record = record;
        // SQLite is a projection of the journals: its failure is a note only.
        let result = self.store.save(&self.record);
        if let Err(error) = &result {
            self.recovery_notice = Some(format!(
                "History persistence failed: {error}; recovery required. Preserve history, repair storage and retry."
            ));
        } else if self.projection_stale {
            let entries = self.record.state.task_history().entries().to_vec();
            if self
                .store
                .replace_task_revisions(self.record.state.project(), &entries)
                .is_ok()
            {
                self.projection_stale = false;
            }
        }
        result
    }
    // ---- agent tasks -------------------------------------------------------------------
    //
    // Agent tasks are separate from the Build/Checkpoint jobs above: they never queue a
    // job, never advance `accepted`, and are exclusively owned through `&mut self`.

    /// The process scope of the *active* task. Spawn the provider here so that stopping
    /// the task (or a source failure, close, drop) reaps its whole tree. Each task owns
    /// its scope: it only exists once the lease and draft are installed, is replaced
    /// on repair, and is sealed for good when the task ends. A stale identity gets no
    /// scope.
    pub fn agent_processes(
        &self,
        identity: &TaskIdentity,
    ) -> Result<studio_bootstrap::ProcessTreeManager, EngineError> {
        self.agent.validate(identity)?;
        self.scope_of(identity)
            .ok_or_else(|| TaskError::StaleIdentity.into())
    }

    fn scope_of(&self, identity: &TaskIdentity) -> Option<studio_bootstrap::ProcessTreeManager> {
        self.agent_scope
            .as_ref()
            .filter(|scope| scope.owner == identity.task)
            .map(|scope| scope.processes.clone())
    }

    /// The active task, or the most recently finished one.
    pub fn agent_task(&self) -> Option<&AgentTask> {
        self.agent.current()
    }

    /// Ownership state of the stable agent draft, if one was ever prepared.
    pub fn agent_draft_state(&self) -> Result<Option<DraftState>, EngineError> {
        Ok(self.drafts.snapshot()?.map(|s| s.state))
    }

    pub fn agent_draft_store(&self) -> &DraftStore {
        &self.drafts
    }

    /// Captures the whole-project context and reserves the stable draft and writer lease.
    ///
    /// The source base is the project's current bytes, including dirty imports, captured
    /// here independently of `accepted`; the draft is materialized from it, never from an
    /// older checkpoint.
    pub fn begin_agent_task(&mut self, brief: &str) -> Result<AgentTaskContext, EngineError> {
        self.begin_agent_task_observed(brief, |_| {})
    }

    /// Starts a task with the exact compiled scope captured by the native UI at submit.
    /// The scope is checked against the current project/source before refreshing the draft.
    pub fn begin_agent_task_scoped(
        &mut self,
        brief: &str,
        scope: crate::TaskScope,
    ) -> Result<AgentTaskContext, EngineError> {
        self.begin_agent_task_scoped_observed(brief, Some(scope), |_| {})
    }

    /// [`Self::begin_agent_task`] reporting its internal steps: `agent_scope_checked`
    /// fires after the previous scope was sealed and observed empty, immediately before
    /// the draft is refreshed.
    pub fn begin_agent_task_observed(
        &mut self,
        brief: &str,
        observe: impl Fn(&str),
    ) -> Result<AgentTaskContext, EngineError> {
        self.begin_agent_task_scoped_observed(brief, None, observe)
    }

    /// Shared implementation for legacy whole-project and revision-bound scoped starts.
    pub fn begin_agent_task_scoped_observed(
        &mut self,
        brief: &str,
        submitted_scope: Option<crate::TaskScope>,
        observe: impl Fn(&str),
    ) -> Result<AgentTaskContext, EngineError> {
        let brief = validate_brief(brief)?;
        self.reconcile()?;
        let base = self.checkpoints.capture(&self.project.root)?;
        if base != self.project.inventory.revision {
            // The source moved between the scan and the capture; retry.
            return Err(crate::StateError::StaleResult.into());
        }
        let source_base = TaskSourceBase::new(base);
        let mut scope = match submitted_scope {
            Some(scope) => {
                scope.validate()?;
                if scope.project_id != String::from(self.project.manifest.project_id.clone())
                    || scope.source_revision != source_base.revision().as_str()
                {
                    return Err(crate::TaskScopeError::Identity.into());
                }
                scope
            }
            None => crate::TaskScope::whole_project(
                String::from(self.project.manifest.project_id.clone()),
                source_base.revision().as_str(),
            ),
        };
        if scope.style_snapshot.is_none() {
            scope.style_snapshot =
                crate::preset_state::style_identity(&self.project.root, &self.project.inventory);
        }
        if scope.compiled.is_some() {
            scope.resolve_scene_sources(&self.project.root, &self.project.inventory.files)?;
            scope.validate()?;
        }
        let reservation = self.agent.reserve()?;
        // The previous task's scope must not be able to spawn while the draft is checked
        // and refreshed: seal it (serialized with every spawn, including clones held by
        // background threads) before looking at it. A new scope is only handed out after
        // the draft and the lease are installed.
        let live = match &self.agent_scope {
            Some(previous) => {
                previous.processes.seal();
                previous.processes.active_count() > 0
            }
            None => false,
        };
        observe("agent_scope_checked");
        let prepared = self.drafts.prepare(
            &self.checkpoints,
            &source_base,
            &reservation.identity.task,
            live,
        )?;
        let files = &self.project.inventory.files;
        let context = AgentTaskContext {
            identity: reservation.identity,
            source_base,
            scope,
            prior_checkpoint: self.state().accepted().clone(),
            assets: files
                .iter()
                .filter(|f| f.kind == FileKind::Media)
                .cloned()
                .collect(),
            instructions: files
                .iter()
                .filter(|f| f.kind == FileKind::Instructions)
                .cloned()
                .collect(),
            brief,
            draft: prepared.path,
            archived_previous: prepared.archived,
        };
        if let Err(error) = self.agent.begin(context.clone()) {
            // Nothing owns the freshly prepared draft; keep it reusable.
            let _ = self.drafts.set_state(DraftState::Retained {
                task: context.identity.task.0.clone(),
                reason: "task could not start".into(),
            });
            return Err(error.into());
        }
        self.agent_fences = Some(TaskFences {
            task: context.identity.task.clone(),
            history_head: self.record.state.task_history().head_id(),
            saved: self.record.state.saved_history_fence(),
        });
        self.agent_scope = Some(AgentScope {
            owner: context.identity.task.clone(),
            processes: self.processes.sub_manager(),
        });
        Ok(context)
    }

    /// Records that the provider writer started and which writer-ownership model applies.
    /// Returns the writer generation the quiescence evidence must name. Starting a
    /// writer invalidates every earlier evidence and ticket; ownership demotions
    /// recorded earlier in the task are sticky.
    pub fn agent_writer_started(
        &mut self,
        identity: &TaskIdentity,
        ownership: studio_bootstrap::WriterOwnership,
    ) -> Result<WriterGeneration, EngineError> {
        Ok(self.agent.writer_started(identity, ownership)?)
    }

    /// Feeds the driver's view of the writer (from its `DriverOutcome`) into the task.
    /// It can only demote the recorded ownership; it never proves termination.
    pub fn agent_observe_writer(
        &mut self,
        identity: &TaskIdentity,
        observed: &WriterObservation,
    ) -> Result<(), EngineError> {
        Ok(self.agent.observe_writer(identity, observed)?)
    }

    pub fn agent_provider_session(
        &mut self,
        identity: &TaskIdentity,
        session: &str,
    ) -> Result<(), EngineError> {
        Ok(self.agent.record_provider_session(identity, session)?)
    }

    /// Non-terminal transition (`Editing`, `Waiting`, `Quiescing`, `Promoting`, ...).
    pub fn agent_task_transition(
        &mut self,
        identity: &TaskIdentity,
        next: TaskState,
    ) -> Result<(), EngineError> {
        Ok(self.agent.transition(identity, next)?)
    }

    /// Evaluates the quiescence gate; only a pass returns the capture ticket.
    ///
    /// The evidence must be bound to this task, its provider session and its current
    /// writer generation. Termination is derived here from the task's own process
    /// scope: the scope is sealed against spawns and observed, so a live member (or an
    /// observed escape) blocks the gate whatever the caller asserts. A block leaves
    /// the task `Quiescing` for the caller to fail, with the draft kept unsafe.
    pub fn agent_complete_quiescence(
        &mut self,
        identity: &TaskIdentity,
        evidence: &QuiescenceEvidence,
    ) -> Result<CaptureTicket, EngineError> {
        let scope = self.scope_of(identity);
        Ok(self.agent.complete_quiescence(identity, evidence, || {
            let Some(scope) = scope else {
                return ScopeFacts::unobserved();
            };
            scope.seal();
            scope.observe().into()
        })?)
    }

    /// Registers `captured` (produced from `ticket`) as the task's candidate and starts
    /// validation, binding its manifest hash for the report check.
    pub fn agent_record_candidate(
        &mut self,
        ticket: CaptureTicket,
        captured: &CapturedCandidate,
    ) -> Result<(), EngineError> {
        Ok(self.agent.record_candidate(
            ticket,
            captured.candidate().clone(),
            captured.manifest_sha256().to_owned(),
        )?)
    }

    /// Captures the quiesced draft as an immutable candidate and registers it, moving
    /// the task to `Validating`. The ticket must still be the single outstanding one of
    /// the current writer epoch *before* anything is captured; a changing draft fails
    /// with `DraftChanged` and leaves the task `Quiescing` for the caller to fail.
    pub fn agent_capture_candidate(
        &mut self,
        ticket: CaptureTicket,
    ) -> Result<CapturedCandidate, EngineError> {
        self.agent.check_ticket(&ticket)?;
        let scope = self
            .agent
            .validate(ticket.identity())?
            .context()
            .scope
            .clone();
        let captured =
            capture_candidate_scoped_observed(&ticket, &self.checkpoints, scope, &mut |_| {})?;
        self.agent.record_candidate(
            ticket,
            captured.candidate().clone(),
            captured.manifest_sha256().to_owned(),
        )?;
        Ok(captured)
    }

    /// Routes a validation report deterministically. The report must name this task and
    /// its recorded candidate. `Accept` moves the task to `CandidateReady` (promotion is
    /// Stage 3); `Repair` spends the single automatic repair (the caller then calls
    /// [`Self::agent_begin_repair`]); `Retain` ends the task `Failed` with the draft
    /// retained: environment failures never spend the repair, and a second source
    /// failure is terminal. A user Stop is not a validation result: finish the task
    /// `Cancelled` instead.
    pub fn agent_apply_validation(
        &mut self,
        identity: &TaskIdentity,
        report: &ValidationReport,
    ) -> Result<NextStep, EngineError> {
        let task = self.agent.validate(identity)?;
        if task.state() != TaskState::Validating {
            return Err(TaskError::WrongState {
                expected: TaskState::Validating,
                actual: task.state(),
            }
            .into());
        }
        // The report must be for exactly what this task captured: identity, candidate,
        // source base and checkpoint manifest. A report (however obtained) for anything
        // else never advances the task.
        if report.task() != identity
            || task.candidate() != Some(report.candidate())
            || report.source_base() != &task.context().source_base
            || task.candidate_manifest() != Some(report.manifest_sha256())
        {
            return Err(TaskError::ReportMismatch.into());
        }
        let step = next_step(report, task.repair());
        match &step {
            NextStep::Accept => {
                self.agent
                    .candidate_validated(identity, &report.candidate().clone())?;
            }
            NextStep::Repair { context, .. } => {
                self.agent_validation_failed(identity, &context.summary)?;
            }
            NextStep::Retain { reason, context } => {
                if context
                    .as_ref()
                    .is_some_and(|c| c.kind == FailureKind::Source)
                {
                    // Only reached with the repair budget spent: terminal.
                    self.agent_validation_failed(identity, reason)?;
                } else {
                    self.finish_agent_task(identity, TaskState::Failed, reason)?;
                }
            }
        }
        Ok(step)
    }

    pub fn agent_validation_failed(
        &mut self,
        identity: &TaskIdentity,
        reason: &str,
    ) -> Result<RepairDecision, EngineError> {
        let decision = self.agent.validation_failed(identity, reason)?;
        if decision == RepairDecision::Exhausted {
            // The task already ended; its scope was sealed and verified empty by the
            // quiescence that preceded validation. Reap it anyway so nothing outlives it.
            if let Some(scope) = self.scope_of(identity) {
                scope.shutdown(Duration::from_millis(300));
            }
            if let Some(finished) = self.agent.current().map(finished_view) {
                self.settle_draft(&finished)?;
            }
        }
        Ok(decision)
    }

    /// Starts the automatic repair as a new writer epoch with a fresh process scope. The
    /// previous scope is sealed against spawns and must be verifiably empty first;
    /// otherwise nothing changes and the draft stays busy.
    pub fn agent_begin_repair(&mut self, identity: &TaskIdentity) -> Result<u32, EngineError> {
        let previous = self.scope_of(identity);
        let attempt = self.agent.begin_repair(identity, || {
            if let Some(previous) = &previous {
                previous.seal();
                if !previous.observe().is_clean() {
                    return Err(TaskError::DraftBusy);
                }
            }
            Ok(())
        })?;
        self.agent_scope = Some(AgentScope {
            owner: identity.task.clone(),
            processes: self.processes.sub_manager(),
        });
        Ok(attempt)
    }

    /// Ends the task, reaps any provider tree still in its scope, releases the writer
    /// lease and records how the draft may be reused.
    ///
    /// The identity and the requested terminal transition are validated before any
    /// process scope is touched, and only the identified task's own scope is torn down,
    /// so a late finish for an earlier task can never disturb its successor.
    pub fn finish_agent_task(
        &mut self,
        identity: &TaskIdentity,
        terminal: TaskState,
        reason: &str,
    ) -> Result<FinishedTask, EngineError> {
        self.agent.check_finish(identity, terminal)?;
        self.teardown_agent_scope(identity);
        let finished = self.agent.finish(identity, terminal, reason)?;
        self.settle_draft(&finished)?;
        Ok(finished)
    }

    /// Clears an `UnsafeWriter` draft after evidence that no writer remains.
    pub fn acknowledge_agent_writer_gone(
        &mut self,
        evidence: &WriterGoneEvidence,
    ) -> Result<(), EngineError> {
        Ok(self.drafts.acknowledge_writer_gone(evidence)?)
    }

    /// Seals and reaps the task's own scope and records what it showed. Escapes are read
    /// before anything is killed (a helper that left the process group is only visible
    /// while its parent lives); termination is whatever the scope's trees verifiably
    /// reported, never inferred from a process count. The facts only ever demote: an
    /// observed escape makes the writer `Detached` and the draft `UnsafeWriter`.
    fn teardown_agent_scope(&mut self, identity: &TaskIdentity) {
        let Some(scope) = self.scope_of(identity) else {
            return;
        };
        scope.seal();
        let before = scope.observe();
        let termination = scope.shutdown_verified(Duration::from_millis(300));
        let mut facts = ScopeFacts::from(before);
        facts.termination = termination.merged();
        let _ = self.agent.record_scope_facts(identity, facts);
    }

    fn settle_draft(&mut self, finished: &FinishedTask) -> Result<(), EngineError> {
        let task = finished.identity.task.0.clone();
        let state = if !finished.writer_safe {
            DraftState::UnsafeWriter {
                task,
                reason: finished
                    .unsafe_reason
                    .clone()
                    .unwrap_or_else(|| "writer ownership is unproven".into()),
            }
        } else if finished.state == TaskState::Accepted {
            DraftState::Accepted { task }
        } else {
            DraftState::Retained {
                task,
                reason: format!("{:?}: {}", finished.state, finished.reason),
            }
        };
        Ok(self.drafts.set_state(state)?)
    }

    /// Interrupts the active task (if any): kills its provider scope, retains the draft
    /// and releases the lease. `source_invalid` makes the interruption sticky.
    fn interrupt_agent_task(&mut self, reason: &str, source_invalid: bool) {
        let Some(identity) = self.agent.active().map(|t| t.identity().clone()) else {
            return;
        };
        self.teardown_agent_scope(&identity);
        let finished = if source_invalid {
            self.agent.invalidate_source(reason)
        } else {
            self.agent
                .finish(&identity, TaskState::Interrupted, reason)
                .ok()
        };
        if let Some(finished) = finished {
            // A marker write failure leaves `Active`, which restarts as unsafe: fail closed.
            let _ = self.settle_draft(&finished);
        }
    }

    pub fn close(&mut self) -> Result<(), EngineError> {
        self.watcher.take();
        self.interrupt_agent_task("project closed", false);
        self.processes.shutdown(Duration::from_millis(300));
        let mut record = self.record.clone();
        record.state.close();
        self.persist(record)
    }
}

fn finished_view(task: &AgentTask) -> FinishedTask {
    FinishedTask {
        identity: task.identity().clone(),
        state: task.state(),
        writer_safe: task.writer_safe(),
        reason: task.reason().unwrap_or_default().to_owned(),
        unsafe_reason: (!task.writer_safe()).then(|| task.unsafe_reason()),
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.watcher.take();
        self.interrupt_agent_task("controller dropped", false);
        self.processes.shutdown(Duration::ZERO);
    }
}
