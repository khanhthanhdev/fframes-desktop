use crate::edit_transaction::{TaskRevisionRecord, TransactionKind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use studio_project::{ProjectId, SourceRevision};

/// Fresh random token on every open, including reopening the same project.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OpenSession(pub [u8; 16]);
impl OpenSession {
    pub fn new() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }
}
impl Default for OpenSession {
    fn default() -> Self {
        Self::new()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationId(pub u64);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationTag {
    pub project: ProjectId,
    pub session: OpenSession,
    pub base_source: SourceRevision,
    pub operation: OperationId,
    pub generation: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpenState {
    Opening,
    Ready,
    Error(String),
    Closed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Idle,
    Queued(OperationTag),
    Running(OperationTag),
    CancelRequested(OperationTag),
    Succeeded(OperationTag),
    Failed(OperationTag, String),
    Interrupted(OperationTag),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobKind {
    Build,
    Checkpoint,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobResult {
    Built(SourceRevision),
    Checkpointed(SourceRevision),
    Failed(String),
}
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StateError {
    #[error("invalid state transition")]
    InvalidTransition,
    #[error("stale operation result")]
    StaleResult,
    #[error("identity counter exhausted")]
    CounterExhausted,
}
/// Immutable proposed output; its existence never advances the accepted checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    revision: SourceRevision,
    tag: OperationTag,
}
impl Candidate {
    pub fn revision(&self) -> &SourceRevision {
        &self.revision
    }
    pub fn tag(&self) -> &OperationTag {
        &self.tag
    }
}

/// How a validated task candidate reaches the source.
///
/// Stored in the app-local history only: it does **not** travel with the portable
/// project folder, so a copied or restored project starts from the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewPolicy {
    /// Apply immediately after required validation passes.
    #[default]
    AutoApply,
    /// Keep the validated candidate until the user explicitly applies it.
    ManualReview,
}

impl ReviewPolicy {
    /// Shown wherever the setting is offered.
    pub const SCOPE_NOTICE: &'static str = "This review setting is stored on this computer for this project. It does not travel with the project folder; a copy or restored project starts with automatic Apply.";
}

/// A validated task revision list with Undo bookkeeping.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskHistory {
    entries: Vec<TaskRevisionRecord>,
}

/// Why Undo is not currently possible, phrased as the next action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoStatus {
    Available { target: String, summary: String },
    Unavailable { reason: String },
}

impl TaskHistory {
    pub fn from_records(entries: Vec<TaskRevisionRecord>) -> Self {
        Self { entries }
    }
    pub fn entries(&self) -> &[TaskRevisionRecord] {
        &self.entries
    }
    pub fn head(&self) -> Option<&TaskRevisionRecord> {
        self.entries.last()
    }
    pub fn head_id(&self) -> Option<String> {
        self.head().map(|r| r.id.clone())
    }
    pub fn get(&self, id: &str) -> Option<&TaskRevisionRecord> {
        self.entries.iter().find(|r| r.id == id)
    }
    pub(crate) fn push(&mut self, record: TaskRevisionRecord) {
        self.entries.push(record);
    }
    /// Ids whose effect is currently reversed: targeted by an Undo that is itself not
    /// reversed (undoing an Undo puts the original edit back in effect).
    fn reversed(&self) -> HashSet<&str> {
        let mut targeted: HashSet<&str> = HashSet::new();
        for entry in self.entries.iter().rev() {
            if entry.kind == TransactionKind::Undo
                && !targeted.contains(entry.id.as_str())
                && let Some(target) = entry.undoes.as_deref()
            {
                targeted.insert(target);
            }
        }
        targeted
    }
    /// The entry Undo would reverse: the requested one, or the newest agent edit that
    /// is still in effect.
    pub fn undo_target(&self, requested: Option<&str>) -> Result<&TaskRevisionRecord, String> {
        let reversed = self.reversed();
        match requested {
            Some(id) => {
                let entry = self.get(id).ok_or_else(|| {
                    format!("revision {id} is not in this project's task history")
                })?;
                if reversed.contains(entry.id.as_str()) {
                    Err(format!("\"{}\" was already undone", entry.prompt_summary))
                } else {
                    Ok(entry)
                }
            }
            None => {
                if self.entries.is_empty() {
                    return Err("No agent edit has been accepted in this project yet".into());
                }
                self.entries
                    .iter()
                    .rev()
                    .find(|e| e.kind == TransactionKind::Apply && !reversed.contains(e.id.as_str()))
                    .ok_or_else(|| "Every accepted agent edit has already been undone".into())
            }
        }
    }
    pub fn undo_status(&self) -> UndoStatus {
        match self.undo_target(None) {
            Ok(target) => UndoStatus::Available {
                target: target.id.clone(),
                summary: target.prompt_summary.clone(),
            },
            Err(reason) => UndoStatus::Unavailable { reason },
        }
    }
}

/// Explicit relationship between a validated candidate and the revision that was
/// published from it. It is what allows a staged preview (prepared against the
/// candidate bytes) to replace the displayed one: neither the task base nor an
/// `Operation` tag derived from it can stand in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionAuthorization {
    tag: OperationTag,
    transaction: String,
    candidate: SourceRevision,
    published: SourceRevision,
    state_generation: u64,
}

impl PromotionAuthorization {
    /// The preview tag a ready preview must carry: base source = the published revision.
    pub fn tag(&self) -> &OperationTag {
        &self.tag
    }
    pub fn transaction(&self) -> &str {
        &self.transaction
    }
    pub fn candidate(&self) -> &SourceRevision {
        &self.candidate
    }
    pub fn published(&self) -> &SourceRevision {
        &self.published
    }
    pub fn state_generation(&self) -> u64 {
        self.state_generation
    }
    /// Valid only for the very session, source and generation it was issued for; any
    /// later source change (including an external edit) revokes it.
    pub fn is_current(&self, state: &ProjectState) -> bool {
        self.tag.project == state.project
            && self.tag.session == state.session
            && self.published == *state.source()
            && self.state_generation == state.generation
            && self.tag.base_source == self.published
    }
}

/// The saved (M1 checkpoint) history as a task or Undo preparation saw it. Apply and Undo
/// refuse when it moved, even if the source bytes and the task-history head did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedHistoryFence {
    accepted: SourceRevision,
    generation: u64,
}

impl SavedHistoryFence {
    /// The saved checkpoint at the time the fence was taken.
    pub fn accepted(&self) -> &SourceRevision {
        &self.accepted
    }
}

/// Owned by one open-project controller; no process-global mutable state.
/// Accepted means saved checkpoint bytes, never proof of a successful compilation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectState {
    project: ProjectId,
    session: OpenSession,
    open: OpenState,
    source: SourceRevision,
    accepted: SourceRevision,
    candidate: Option<Candidate>,
    built: Option<SourceRevision>,
    generation: u64,
    next_operation: u64,
    job: JobState,
    kind: Option<JobKind>,
    /// App-local: how a validated task candidate reaches the source. Legacy records
    /// without the field read as the default (automatic Apply).
    #[serde(default)]
    review_policy: ReviewPolicy,
    /// Validated task revisions (Apply and Undo) in commit order, distinct from the saved
    /// checkpoint `accepted`. An in-memory view of the task journal (the source of
    /// truth): it is never written into lifecycle snapshots, which would otherwise grow
    /// with every accepted revision until a snapshot exceeded the journal's entry limit.
    /// Records written by earlier builds still carry an embedded list; it is read for
    /// compatibility and replaced by the journal's on open.
    #[serde(default, skip_serializing)]
    task_history: TaskHistory,
    /// Counts changes of the saved checkpoint (`accepted`): the saved-history fence.
    #[serde(default)]
    saved_generation: u64,
    /// The explicit candidate-to-published relationship that lets a staged preview of
    /// the published bytes be installed. Session-local, never persisted.
    #[serde(skip)]
    promotion: Option<PromotionAuthorization>,
}
impl ProjectState {
    pub fn opening(
        project: ProjectId,
        session: OpenSession,
        source: SourceRevision,
        accepted: SourceRevision,
    ) -> Self {
        Self {
            project,
            session,
            open: OpenState::Opening,
            source,
            accepted,
            candidate: None,
            built: None,
            generation: 0,
            next_operation: 0,
            job: JobState::Idle,
            kind: None,
            review_policy: ReviewPolicy::default(),
            task_history: TaskHistory::default(),
            saved_generation: 0,
            promotion: None,
        }
    }
    pub fn review_policy(&self) -> ReviewPolicy {
        self.review_policy
    }
    pub(crate) fn set_review_policy(&mut self, policy: ReviewPolicy) {
        self.review_policy = policy;
    }
    pub fn task_history(&self) -> &TaskHistory {
        &self.task_history
    }
    /// The saved-history fence: which checkpoint is current and how often the saved
    /// history changed. Independent of the source revision and of the task-history head.
    pub fn saved_history_fence(&self) -> SavedHistoryFence {
        SavedHistoryFence {
            accepted: self.accepted.clone(),
            generation: self.saved_generation,
        }
    }
    /// Replaces the projection (rebuild from the task journal on open).
    pub(crate) fn set_task_history(&mut self, history: TaskHistory) {
        self.task_history = history;
    }
    /// Records a durably committed task revision. The saved checkpoint (`accepted`) is
    /// deliberately untouched: a task revision proves a validated edit, a checkpoint
    /// proves saved bytes.
    pub(crate) fn record_task_revision(&mut self, record: TaskRevisionRecord) {
        self.task_history.push(record);
    }
    pub fn promotion(&self) -> Option<&PromotionAuthorization> {
        self.promotion.as_ref()
    }
    pub(crate) fn set_promotion(&mut self, authorization: PromotionAuthorization) {
        self.promotion = Some(authorization);
    }
    /// A fresh install authorization for a just-published task revision. It names the
    /// published candidate revision and the current session and source generation, and
    /// is rejected unless the live source is exactly the published revision.
    pub fn authorize_promotion(
        &self,
        record: &TaskRevisionRecord,
        task_generation: u64,
    ) -> Result<PromotionAuthorization, StateError> {
        if self.open != OpenState::Ready
            || record.session != self.session
            || record.published != self.source
            || record.published == record.task_base
            || self.task_history.get(&record.id) != Some(record)
        {
            return Err(StateError::StaleResult);
        }
        Ok(PromotionAuthorization {
            tag: OperationTag {
                project: self.project.clone(),
                session: self.session.clone(),
                base_source: record.published.clone(),
                operation: OperationId(task_generation),
                generation: task_generation,
            },
            transaction: record.id.clone(),
            candidate: record.candidate.clone(),
            published: record.published.clone(),
            state_generation: self.generation,
        })
    }
    pub fn open_state(&self) -> &OpenState {
        &self.open
    }
    pub fn source(&self) -> &SourceRevision {
        &self.source
    }
    pub fn accepted(&self) -> &SourceRevision {
        &self.accepted
    }
    pub fn candidate(&self) -> Option<&Candidate> {
        self.candidate.as_ref()
    }
    pub fn built(&self) -> Option<&SourceRevision> {
        self.built.as_ref()
    }
    pub fn job(&self) -> &JobState {
        &self.job
    }
    pub fn project(&self) -> &ProjectId {
        &self.project
    }
    pub fn session(&self) -> &OpenSession {
        &self.session
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Recovery refreshes the session and actual source; old operation tags remain historical only.
    pub fn reopened(&self, source: SourceRevision) -> Self {
        let mut state = Self::opening(
            self.project.clone(),
            OpenSession::new(),
            source,
            self.accepted.clone(),
        );
        state.open = OpenState::Ready;
        state.review_policy = self.review_policy;
        state.task_history = self.task_history.clone();
        state.saved_generation = self.saved_generation;
        state.job = match &self.job {
            JobState::Queued(tag) | JobState::Running(tag) | JobState::CancelRequested(tag) => {
                JobState::Interrupted(tag.clone())
            }
            job => job.clone(),
        };
        state
    }
    pub fn finish_open(&mut self, result: Result<(), String>) -> Result<(), StateError> {
        if self.open != OpenState::Opening {
            return Err(StateError::InvalidTransition);
        }
        self.open = match result {
            Ok(()) => OpenState::Ready,
            Err(e) => OpenState::Error(e),
        };
        Ok(())
    }
    /// Actual filesystem identity must be freshly reconciled before every completion.
    /// Edits retain accepted bytes, invalidate derived identities, and interrupt obsolete work.
    pub fn reconcile_source(&mut self, current: SourceRevision) -> Result<(), StateError> {
        if self.open != OpenState::Ready {
            return Err(StateError::InvalidTransition);
        }
        if current != self.source {
            self.invalidate_source()?;
            self.source = current;
        }
        Ok(())
    }
    /// A failed scan invalidates old work even if the source later returns to the same bytes.
    pub(crate) fn invalidate_source(&mut self) -> Result<(), StateError> {
        if self.open != OpenState::Ready {
            return Err(StateError::InvalidTransition);
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(StateError::CounterExhausted)?;
        self.candidate = None;
        self.built = None;
        self.promotion = None;
        if self.active_tag().is_some() {
            self.interrupt()?;
        }
        Ok(())
    }
    pub fn active_tag(&self) -> Option<&OperationTag> {
        match &self.job {
            JobState::Queued(t) | JobState::Running(t) | JobState::CancelRequested(t) => Some(t),
            _ => None,
        }
    }
    fn matches_current_source(&self, tag: &OperationTag) -> bool {
        tag.project == self.project
            && tag.session == self.session
            && tag.base_source == self.source
            && tag.generation == self.generation
    }
    pub fn queue(&mut self, kind: JobKind) -> Result<OperationTag, StateError> {
        if self.open != OpenState::Ready || self.active_tag().is_some() {
            return Err(StateError::InvalidTransition);
        }
        let next = self
            .next_operation
            .checked_add(1)
            .ok_or(StateError::CounterExhausted)?;
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(StateError::CounterExhausted)?;
        self.next_operation = next;
        self.generation = generation;
        let tag = OperationTag {
            project: self.project.clone(),
            session: self.session.clone(),
            base_source: self.source.clone(),
            operation: OperationId(next),
            generation,
        };
        self.job = JobState::Queued(tag.clone());
        self.kind = Some(kind);
        Ok(tag)
    }
    pub fn start(&mut self, tag: &OperationTag) -> Result<(), StateError> {
        if !matches!(&self.job, JobState::Queued(t) if t == tag) {
            return Err(StateError::InvalidTransition);
        }
        self.job = JobState::Running(tag.clone());
        Ok(())
    }
    pub fn request_cancel(&mut self) -> Result<(), StateError> {
        let tag = self
            .active_tag()
            .cloned()
            .ok_or(StateError::InvalidTransition)?;
        self.job = JobState::CancelRequested(tag);
        Ok(())
    }
    pub fn interrupt(&mut self) -> Result<(), StateError> {
        let tag = self
            .active_tag()
            .cloned()
            .ok_or(StateError::InvalidTransition)?;
        self.job = JobState::Interrupted(tag);
        Ok(())
    }
    /// Settle failed startup or execution without installing output or requiring a healthy scan.
    pub(crate) fn fail(&mut self, tag: &OperationTag, reason: String) -> Result<(), StateError> {
        if self.active_tag() != Some(tag) {
            return Err(StateError::StaleResult);
        }
        self.job = JobState::Failed(tag.clone(), reason);
        Ok(())
    }
    /// Compare-before-install guard. This changes model state only, never project files.
    /// The filesystem owner supplies a fresh inventory revision immediately before this call.
    pub fn complete(
        &mut self,
        tag: &OperationTag,
        current: SourceRevision,
        result: JobResult,
    ) -> Result<(), StateError> {
        self.reconcile_source(current)?;
        if !self.matches_current_source(tag)
            || !matches!(&self.job, JobState::Running(t) if t == tag)
        {
            return Err(StateError::StaleResult);
        }
        match (self.kind, result) {
            (_, JobResult::Failed(e)) => self.job = JobState::Failed(tag.clone(), e),
            (Some(JobKind::Build), JobResult::Built(revision)) if revision == tag.base_source => {
                self.built = Some(revision);
                self.job = JobState::Succeeded(tag.clone());
            }
            (Some(JobKind::Checkpoint), JobResult::Checkpointed(revision))
                if revision == tag.base_source =>
            {
                self.accepted = revision;
                self.saved_generation += 1;
                self.job = JobState::Succeeded(tag.clone());
            }
            _ => return Err(StateError::InvalidTransition),
        }
        Ok(())
    }
    /// Register immutable proposed bytes against the current session/base; never accept them.
    pub fn propose(
        &mut self,
        tag: &OperationTag,
        current: SourceRevision,
        revision: SourceRevision,
    ) -> Result<(), StateError> {
        self.reconcile_source(current)?;
        if !self.matches_current_source(tag)
            || !matches!(&self.job, JobState::Succeeded(t) if t == tag)
        {
            return Err(StateError::StaleResult);
        }
        self.candidate = Some(Candidate {
            revision,
            tag: tag.clone(),
        });
        Ok(())
    }
    pub fn close(&mut self) {
        if let Some(tag) = self.active_tag().cloned() {
            self.job = JobState::Interrupted(tag);
        }
        self.candidate = None;
        self.built = None;
        self.promotion = None;
        self.open = OpenState::Closed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit_transaction::{FileDelta, FileState};

    fn revision(c: char) -> SourceRevision {
        SourceRevision::try_from(c.to_string().repeat(64)).unwrap()
    }

    fn state() -> ProjectState {
        let mut state = ProjectState::opening(
            ProjectId::try_from("project-1".to_owned()).unwrap(),
            OpenSession::new(),
            revision('a'),
            revision('a'),
        );
        state.finish_open(Ok(())).unwrap();
        state
    }

    fn record(
        state: &ProjectState,
        id: &str,
        kind: TransactionKind,
        undoes: Option<&str>,
        base: char,
        published: char,
    ) -> TaskRevisionRecord {
        TaskRevisionRecord {
            id: id.into(),
            kind,
            task: format!("task-{id}"),
            generation: 7,
            session: state.session().clone(),
            task_base: revision(base),
            prior_history: None,
            prior_checkpoint: revision('a'),
            candidate: revision(published),
            published: revision(published),
            undoes: undoes.map(str::to_owned),
            prompt_summary: format!("edit {id}"),
            changes: vec![FileDelta {
                path: "a.txt".into(),
                before: None,
                after: Some(FileState {
                    sha256: "1".repeat(64),
                    size: 1,
                    executable: false,
                }),
                before_mode: None,
                after_mode: Some(0o644),
            }],
            created_dirs: vec![],
            build: None,
            validation_report_sha256: "0".repeat(64),
            committed_unix: 0,
            preset: None,
        }
    }

    #[test]
    fn legacy_records_without_the_new_fields_read_with_defaults() {
        let mut json = serde_json::to_value(state()).unwrap();
        let object = json.as_object_mut().unwrap();
        assert!(object.remove("review_policy").is_some());
        assert!(object.remove("saved_generation").is_some());
        assert!(!object.contains_key("promotion"));
        let legacy: ProjectState = serde_json::from_value(json).unwrap();
        assert_eq!(legacy.review_policy(), ReviewPolicy::AutoApply);
        assert!(legacy.task_history().entries().is_empty());
        assert_eq!(legacy.accepted(), &revision('a'));
        assert_eq!(legacy.saved_history_fence().generation, 0);
        // And the policy round-trips under its stable name.
        let mut manual = state();
        manual.set_review_policy(ReviewPolicy::ManualReview);
        let json = serde_json::to_value(&manual).unwrap();
        assert_eq!(json["review_policy"], "manual_review");
        let back: ProjectState = serde_json::from_value(json).unwrap();
        assert_eq!(back.review_policy(), ReviewPolicy::ManualReview);
        // A reopen keeps the app-local policy and the task history.
        assert_eq!(
            back.reopened(revision('a')).review_policy(),
            ReviewPolicy::ManualReview
        );
    }

    #[test]
    fn task_history_is_never_written_into_a_snapshot_but_embedded_legacy_lists_still_read() {
        let mut state = state();
        let applied = record(&state, "tx1", TransactionKind::Apply, None, 'a', 'b');
        state.record_task_revision(applied.clone());
        let json = serde_json::to_value(&state).unwrap();
        assert!(
            !json.as_object().unwrap().contains_key("task_history"),
            "snapshots must not grow with the history"
        );
        // A record written by an earlier build carries the list inline.
        let mut legacy = json;
        legacy.as_object_mut().unwrap().insert(
            "task_history".into(),
            serde_json::json!({ "entries": [serde_json::to_value(&applied).unwrap()] }),
        );
        let read: ProjectState = serde_json::from_value(legacy).unwrap();
        assert_eq!(read.task_history().entries(), &[applied]);
    }

    #[test]
    fn a_validated_task_revision_is_not_a_saved_checkpoint() {
        let mut state = state();
        let applied = record(&state, "tx1", TransactionKind::Apply, None, 'a', 'b');
        state.record_task_revision(applied);
        assert_eq!(
            state.accepted(),
            &revision('a'),
            "history never moves the checkpoint"
        );
        assert_eq!(state.task_history().entries().len(), 1);
        // Only a checkpoint job whose captured bytes equal its own base can advance it,
        // and the candidate/published revision is not that base.
        state.reconcile_source(revision('b')).unwrap();
        let tag = state.queue(JobKind::Checkpoint).unwrap();
        assert_eq!(tag.base_source, revision('b'));
        state.start(&tag).unwrap();
        assert_eq!(
            state.complete(&tag, revision('b'), JobResult::Checkpointed(revision('c'))),
            Err(StateError::InvalidTransition)
        );
        assert_eq!(state.accepted(), &revision('a'));
        state
            .complete(&tag, revision('b'), JobResult::Checkpointed(revision('b')))
            .unwrap();
        assert_eq!(state.accepted(), &revision('b'));
        assert_eq!(state.task_history().entries().len(), 1);
    }

    #[test]
    fn undo_targets_follow_what_is_still_in_effect() {
        let mut state = state();
        assert_eq!(
            state.task_history().undo_status(),
            UndoStatus::Unavailable {
                reason: "No agent edit has been accepted in this project yet".into()
            }
        );
        let a = record(&state, "a", TransactionKind::Apply, None, 'a', 'b');
        let b = record(&state, "b", TransactionKind::Apply, None, 'b', 'c');
        state.record_task_revision(a);
        state.record_task_revision(b);
        let target = |s: &ProjectState| s.task_history().undo_target(None).map(|r| r.id.clone());
        assert_eq!(target(&state), Ok("b".into()));
        // Undo b, then a: nothing left.
        let u1 = record(&state, "u1", TransactionKind::Undo, Some("b"), 'c', 'd');
        state.record_task_revision(u1);
        assert_eq!(target(&state), Ok("a".into()));
        assert!(state.task_history().undo_target(Some("b")).is_err());
        let u2 = record(&state, "u2", TransactionKind::Undo, Some("a"), 'd', 'e');
        state.record_task_revision(u2);
        let UndoStatus::Unavailable { reason } = state.task_history().undo_status() else {
            panic!("everything is undone");
        };
        assert!(reason.contains("already been undone"));
        // Undoing the Undo of b puts b back in effect.
        let redo = record(&state, "r1", TransactionKind::Undo, Some("u1"), 'e', 'f');
        state.record_task_revision(redo);
        assert_eq!(target(&state), Ok("b".into()));
        assert!(state.task_history().undo_target(Some("zzz")).is_err());
    }

    #[test]
    fn promotion_authorization_names_the_published_revision_and_dies_with_the_source() {
        let mut state = state();
        let applied = record(&state, "tx1", TransactionKind::Apply, None, 'a', 'b');
        // Not authorized until the history holds it and the source equals it.
        assert_eq!(
            state.authorize_promotion(&applied, 4),
            Err(StateError::StaleResult)
        );
        state.record_task_revision(applied.clone());
        assert_eq!(
            state.authorize_promotion(&applied, 4),
            Err(StateError::StaleResult),
            "the live source is still the base"
        );
        state.reconcile_source(revision('b')).unwrap();
        let authorization = state.authorize_promotion(&applied, 4).unwrap();
        assert_eq!(authorization.tag().base_source, revision('b'));
        assert_ne!(authorization.tag().base_source, applied.task_base);
        assert_eq!(authorization.tag().generation, 4);
        assert_eq!(authorization.tag().session, *state.session());
        assert!(authorization.is_current(&state));
        // A candidate that equals its base can never be "promoted".
        let same = record(&state, "tx2", TransactionKind::Apply, None, 'b', 'b');
        state.record_task_revision(same.clone());
        assert_eq!(
            state.authorize_promotion(&same, 5),
            Err(StateError::StaleResult)
        );
        // Another session cannot use it, and any later source change revokes it.
        let other = state.reopened(revision('b'));
        assert!(!authorization.is_current(&other));
        state.reconcile_source(revision('c')).unwrap();
        assert!(!authorization.is_current(&state));
    }
}
