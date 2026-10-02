use crate::{
    EngineError, JobKind, JobResult, JobState, OpenSession, OperationTag, ProjectState,
    app_paths::AppPaths,
    journal::Journal,
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
use studio_project::{OpenProject, checkpoint::Checkpoints};

/// A single background owner per project. Every completion rescans; watcher events are hints only.
pub struct Controller {
    pub project: OpenProject,
    record: Record,
    store: Store,
    journal: Journal,
    pub checkpoints: Checkpoints,
    _lock: File,
    watcher: Option<notify::RecommendedWatcher>,
    dirty: Arc<AtomicBool>,
    pub history_was_available: bool,
    pub recovery_notice: Option<String>,
    pub processes: studio_bootstrap::ProcessTreeManager,
}
impl Controller {
    pub fn open(root: &Path, paths: &AppPaths) -> Result<Self, EngineError> {
        let (project, source_degraded) = match studio_project::open(root) {
            Ok(p) => (p, None),
            Err(e) => {
                let err_msg = e.to_string();
                let recovery = studio_project::open_for_recovery(root).map_err(|_| e)?;
                (recovery, Some(err_msg))
            }
        };
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
        let mut store = Store::open(&paths.database())?;
        let (mut journal, replay) = Journal::open(history.join("lifecycle.jsonl"))?;
        let checkpoints = Checkpoints::new(&history)?;
        let previous = replay
            .committed
            .or(store.get(&project.manifest.project_id)?);
        let history_was_available = previous.is_some();
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
        let tx = journal.intent(&record)?;
        journal.commit(&tx)?;
        store.save(&record)?;
        let dirty = Arc::new(AtomicBool::new(false));
        let flag = dirty.clone();
        let mut watcher =
            notify::recommended_watcher(move |_: Result<notify::Event, notify::Error>| {
                flag.store(true, Ordering::Release);
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
        Ok(Self {
            project,
            record,
            store,
            journal,
            checkpoints,
            _lock: lock,
            watcher: Some(watcher),
            dirty,
            history_was_available,
            recovery_notice: replay
                .quarantined_tail
                .map(|p| format!("Truncated journal tail preserved at {}", p.display()))
                .or_else(|| {
                    source_degraded.map(|err| {
                        format!("Source degraded ({err}). Safe checkpoint export is available.")
                    })
                }),
            processes: studio_bootstrap::ProcessTreeManager::new(),
        })
    }
    pub fn state(&self) -> &ProjectState {
        &self.record.state
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
                self.processes.terminate_all(Duration::from_millis(300));
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
            self.processes.terminate_all(Duration::from_millis(300));
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
        let name = format!(
            "{}-{}",
            uuid::Uuid::from_bytes(tag.session.0),
            tag.operation.0
        );
        record.draft = Some(self.checkpoints.root.join("drafts").join(&name));
        let result: Result<(), EngineError> = (|| {
            let tx = self.journal.intent(&record)?;
            observe("intent");
            let base = self.checkpoints.capture(&self.project.root)?;
            if base != tag.base_source {
                return Err(crate::StateError::StaleResult.into());
            }
            observe("objects");
            self.checkpoints.draft(&base, &name)?;
            observe("draft");
            self.journal.commit(&tx)?;
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
        self.processes.terminate_all(Duration::from_millis(300));
        let mut record = self.record.clone();
        record.state.interrupt()?;
        self.persist(record)
    }
    fn persist(&mut self, record: Record) -> Result<(), EngineError> {
        let result = (|| {
            let tx = self.journal.intent(&record)?;
            self.journal.commit(&tx)?;
            self.record = record;
            self.store.save(&self.record)?;
            Ok(())
        })();
        if let Err(error) = &result {
            self.recovery_notice = Some(format!(
                "History persistence failed: {error}; recovery required. Preserve history, repair storage and retry."
            ));
        }
        result
    }
    pub fn close(&mut self) -> Result<(), EngineError> {
        self.watcher.take();
        self.processes.shutdown(Duration::from_millis(300));
        let mut record = self.record.clone();
        record.state.close();
        self.persist(record)
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.watcher.take();
        self.processes.shutdown(Duration::ZERO);
    }
}
