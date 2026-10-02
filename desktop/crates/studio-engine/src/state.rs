use serde::{Deserialize, Serialize};
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
        }
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
        self.open = OpenState::Closed;
    }
}
