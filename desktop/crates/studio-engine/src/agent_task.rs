//! Agent task identities, lifecycle, stable draft, writer lease and quiescence gate.
//!
//! A task is separate from the Build/Checkpoint jobs of [`crate::ProjectState`]: it
//! never advances the accepted checkpoint. Its source base is captured from the
//! current (possibly dirty) project bytes, independent of any older checkpoint, and
//! the artifact it produces is a differently typed [`CandidateRevision`].

use crate::OpenSession;
use crate::TaskScope;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use studio_bootstrap::{ScopeObservation, TerminationReport, WriterOwnership};
use studio_project::{
    ProjectId, SourceRevision,
    checkpoint::Checkpoints,
    lifecycle::{atomic_write, sync_directory},
    revision::SourceFile,
};

/// Automatic repair attempts allowed per task (provisional engineering default).
pub const MAX_AUTOMATIC_REPAIRS: u32 = 1;
const MAX_BRIEF_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentTaskId(pub String);

impl AgentTaskId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for AgentTaskId {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything that makes a task result attributable to exactly one task of one open
/// session. A result carrying another session or generation is stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskIdentity {
    pub task: AgentTaskId,
    pub project: ProjectId,
    pub session: OpenSession,
    /// Monotonic per open session; every new task gets the next value.
    pub generation: u64,
}

/// The source bytes the stable draft was materialized from. Captured from the current
/// project, never reconciled from an older checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSourceBase(SourceRevision);

impl TaskSourceBase {
    pub(crate) fn new(revision: SourceRevision) -> Self {
        Self(revision)
    }
    pub fn revision(&self) -> &SourceRevision {
        &self.0
    }
}

/// Immutable bytes proposed by a task. Deliberately a different type from
/// [`TaskSourceBase`] and from the M1 saved checkpoint: relabelling one as the
/// other is a compile error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateRevision(SourceRevision);

impl CandidateRevision {
    /// `revision` must name an immutable capture of the quiesced draft.
    pub fn new(revision: SourceRevision) -> Self {
        Self(revision)
    }
    pub fn revision(&self) -> &SourceRevision {
        &self.0
    }
}

/// Whole-project context captured when a task starts. No scoped M4 packet and no M5
/// retrieval: only the brief, the current source and what already exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTaskContext {
    pub identity: TaskIdentity,
    pub source_base: TaskSourceBase,
    /// Immutable whole-project/scene/range scope frozen at submission.
    pub scope: TaskScope,
    /// The M1 saved checkpoint at task start; informational, never the draft base.
    pub prior_checkpoint: SourceRevision,
    pub assets: Vec<SourceFile>,
    pub instructions: Vec<SourceFile>,
    pub brief: String,
    /// The stable draft directory; the agent's working directory for every task.
    pub draft: PathBuf,
    /// Where the previous retained draft was archived before this refresh, if any.
    pub archived_previous: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskState {
    ContextReady,
    Editing,
    Waiting,
    Quiescing,
    Validating,
    RepairNeeded,
    CandidateReady,
    Promoting,
    Accepted,
    Conflict,
    Failed,
    Cancelled,
    Interrupted,
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::Conflict | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// The complete transition table; everything not listed is illegal.
    pub fn can_transition_to(self, next: Self) -> bool {
        use TaskState::*;
        matches!(
            (self, next),
            (ContextReady, Editing | Failed | Cancelled | Interrupted)
                | (
                    Editing,
                    Waiting | Quiescing | Failed | Cancelled | Interrupted
                )
                | (Waiting, Editing | Failed | Cancelled | Interrupted)
                | (Quiescing, Validating | Failed | Cancelled | Interrupted)
                | (
                    Validating,
                    CandidateReady | RepairNeeded | Failed | Cancelled | Interrupted
                )
                | (RepairNeeded, Editing | Failed | Cancelled | Interrupted)
                | (CandidateReady, Promoting | Failed | Cancelled | Interrupted)
                | (Promoting, Accepted | Conflict | Failed | Interrupted)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairBudget {
    max: u32,
    used: u32,
}

impl RepairBudget {
    pub fn automatic() -> Self {
        Self {
            max: MAX_AUTOMATIC_REPAIRS,
            used: 0,
        }
    }
    pub fn used(&self) -> u32 {
        self.used
    }
    pub fn remaining(&self) -> u32 {
        self.max - self.used
    }
    fn consume(&mut self) -> Result<u32, TaskError> {
        if self.used >= self.max {
            return Err(TaskError::RepairExhausted);
        }
        self.used += 1;
        Ok(self.used)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairDecision {
    /// The task is `RepairNeeded`; `begin_repair` starts attempt `attempt`.
    Repair { attempt: u32 },
    /// No automatic repair is left; the task was failed.
    Exhausted,
}

/// How the last prompt turn ended, as reported by the ACP driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnCompletion {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    /// No authoritative completion: stopped, timed out or failed mid-turn.
    None,
}

impl TurnCompletion {
    /// Maps an ACP stop reason string; unknown reasons are not completions.
    pub fn from_stop_reason(reason: &str) -> Self {
        match reason {
            "end_turn" => Self::EndTurn,
            "max_tokens" => Self::MaxTokens,
            "max_turn_requests" => Self::MaxTurnRequests,
            "refusal" => Self::Refusal,
            "cancelled" => Self::Cancelled,
            _ => Self::None,
        }
    }
}

/// Identifies one provider writer epoch of a task. Every writer start and every repair
/// begins a new generation, so evidence or tickets gathered for an earlier writer can
/// never be applied to a later one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WriterGeneration(u64);

impl WriterGeneration {
    pub fn value(self) -> u64 {
        self.0
    }
}

/// What the ACP driver observed of the writer (its `DriverOutcome`). Strictly a
/// demotion channel: it can lower the recorded ownership (an escape was seen, the
/// adapter is not qualified) but never raise it, and it never supplies termination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriterObservation {
    /// Ownership as the driver last reported it (already demoted on an observed escape).
    pub ownership: WriterOwnership,
    /// Descendants the driver saw leave the task process group.
    pub escaped_pids: Vec<u32>,
}

/// Facts required before candidate capture. Protocol completion, source scans and
/// quiet periods never substitute for any of them. The evidence is bound to the
/// current task, provider session and writer generation; termination is *not* part of
/// it because the controller derives that from the task's own process scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuiescenceEvidence {
    /// The task this evidence was gathered for.
    pub identity: TaskIdentity,
    /// The provider session of the completing turn (`None` if none was recorded).
    pub provider_session: Option<String>,
    /// The writer generation returned by `agent_writer_started`.
    pub writer: WriterGeneration,
    pub completion: TurnCompletion,
    /// The user/app sent a cancel during the completing turn.
    pub cancel_requested: bool,
    /// Permission requests that were still open when the turn ended.
    pub unresolved_requests: u32,
    /// The driver's view of the writer; can only demote the recorded ownership.
    pub observed: WriterObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum QuiescenceBlock {
    #[error("the evidence belongs to another task, session or generation")]
    ForeignEvidence,
    #[error("the evidence names writer generation {found}, the current writer is {current}")]
    StaleWriter { found: u64, current: u64 },
    #[error("the evidence names a different provider session than the task's")]
    ProviderSessionMismatch,
    #[error("no writer was started in this task epoch")]
    NoWriter,
    #[error("no authoritative turn completion was received")]
    NoAuthoritativeCompletion,
    #[error("the turn ended with {0:?}, not end_turn")]
    NotEndTurn(TurnCompletion),
    #[error("the turn was cancelled by the client")]
    CancelRequested,
    #[error("{0} permission request(s) were still open at completion")]
    UnresolvedRequests(u32),
    #[error("the adapter process was not verified terminated")]
    WriterNotTerminated,
    #[error("process group members survived termination: {0:?}")]
    GroupSurvivors(Vec<u32>),
    #[error(
        "adapter writer ownership is {0}; only a process-group-contained adapter can be captured"
    )]
    UnqualifiedOwnership(&'static str),
}

/// Authoritative `end_turn` + verified writer termination + qualified ownership.
/// `ownership` and `termination` are the manager's recorded, sticky values, never
/// caller assertions.
pub fn evaluate_quiescence(
    evidence: &QuiescenceEvidence,
    ownership: &WriterOwnership,
    termination: &TerminationReport,
) -> Result<(), QuiescenceBlock> {
    match evidence.completion {
        TurnCompletion::None => return Err(QuiescenceBlock::NoAuthoritativeCompletion),
        TurnCompletion::EndTurn => {}
        other => return Err(QuiescenceBlock::NotEndTurn(other)),
    }
    if evidence.cancel_requested {
        return Err(QuiescenceBlock::CancelRequested);
    }
    if evidence.unresolved_requests > 0 {
        return Err(QuiescenceBlock::UnresolvedRequests(
            evidence.unresolved_requests,
        ));
    }
    if !termination.group_empty {
        return Err(if termination.remaining.is_empty() {
            QuiescenceBlock::WriterNotTerminated
        } else {
            QuiescenceBlock::GroupSurvivors(termination.remaining.clone())
        });
    }
    if !termination.direct_child_exited {
        return Err(QuiescenceBlock::WriterNotTerminated);
    }
    if !ownership.is_qualified() {
        return Err(QuiescenceBlock::UnqualifiedOwnership(ownership.label()));
    }
    Ok(())
}

/// Proof that quiescence was established for one writer generation of one task;
/// required to capture a candidate. Only [`AgentTaskManager::complete_quiescence`] can
/// create one. Deliberately not `Clone`: it is single-use and bound to the stored gate
/// state (writer generation + nonce), so it cannot be replayed after a repair, a new
/// writer, a failed quiescence or a newer ticket.
#[derive(Debug)]
pub struct CaptureTicket {
    identity: TaskIdentity,
    base: TaskSourceBase,
    draft: PathBuf,
    generation: WriterGeneration,
    nonce: u64,
}

impl CaptureTicket {
    pub fn identity(&self) -> &TaskIdentity {
        &self.identity
    }
    pub fn source_base(&self) -> &TaskSourceBase {
        &self.base
    }
    /// The quiesced draft; no owned writer remains.
    pub fn draft(&self) -> &Path {
        &self.draft
    }
    pub fn writer(&self) -> WriterGeneration {
        self.generation
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskError {
    #[error("agent task identity is stale (task, session or generation changed)")]
    StaleIdentity,
    #[error("no agent task is active")]
    NoActiveTask,
    #[error("invalid agent task transition {from:?} -> {to:?}")]
    InvalidTransition { from: TaskState, to: TaskState },
    #[error("transition to {0:?} requires its dedicated operation")]
    GatedTransition(TaskState),
    #[error("operation requires task state {expected:?} but the task is {actual:?}")]
    WrongState {
        expected: TaskState,
        actual: TaskState,
    },
    #[error("agent task {} already holds this project's writer lease", .0.0)]
    LeaseHeld(AgentTaskId),
    #[error("the automatic repair budget is exhausted")]
    RepairExhausted,
    #[error("agent task identity counter exhausted")]
    CounterExhausted,
    #[error("{0}")]
    InvalidBrief(&'static str),
    #[error(
        "the agent draft is in use by a live provider session; stop it before starting another task"
    )]
    DraftBusy,
    #[error("the agent draft is retained: {0}")]
    DraftUnsafe(String),
    #[error("candidate capture is blocked: {0}")]
    QuiescenceBlocked(QuiescenceBlock),
    #[error(
        "the capture ticket is not valid for the current writer (replayed, superseded or issued before a writer start, repair or failed quiescence)"
    )]
    StaleTicket,
    #[error("no writer was started in this task epoch")]
    NoWriter,
    #[error("the validation report is not for this task's recorded candidate")]
    ReportMismatch,
    #[error("agent draft storage: {0}; check disk space and permissions")]
    Storage(String),
}

fn storage(error: impl std::fmt::Display) -> TaskError {
    TaskError::Storage(error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WriterRecord {
    generation: WriterGeneration,
    /// Sticky: only ever lowered by observations, never raised.
    ownership: WriterOwnership,
    /// Derived from the task's own process scope by the controller, never supplied by
    /// a caller.
    termination: Option<TerminationReport>,
}

/// How much a writer ownership model can be trusted; lower is worse.
fn trust_rank(ownership: &WriterOwnership) -> u8 {
    match ownership {
        WriterOwnership::Detached => 0,
        WriterOwnership::Unknown => 1,
        WriterOwnership::ProcessGroupContained { .. } => 2,
    }
}

/// The less trusted of two ownership models; ties keep `current`.
fn demote(current: &WriterOwnership, observed: &WriterOwnership) -> WriterOwnership {
    if trust_rank(observed) < trust_rank(current) {
        observed.clone()
    } else {
        current.clone()
    }
}

/// Facts the controller derived from a task's own process scope: the combined
/// termination of its trees, the escapes seen before they were killed, and whether
/// any tree was alive when the scope was torn down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeFacts {
    pub(crate) termination: TerminationReport,
    pub(crate) escaped: Vec<u32>,
    pub(crate) had_live: bool,
}

impl ScopeFacts {
    /// No scope could be observed: nothing is verified, so the gate stays shut.
    pub(crate) fn unobserved() -> Self {
        Self {
            termination: TerminationReport {
                forced: false,
                direct_child_exited: false,
                group_empty: false,
                remaining: Vec::new(),
            },
            escaped: Vec::new(),
            had_live: true,
        }
    }
}

impl From<ScopeObservation> for ScopeFacts {
    fn from(observation: ScopeObservation) -> Self {
        Self {
            had_live: observation.live_children > 0,
            termination: observation.termination,
            escaped: observation.escaped,
        }
    }
}

/// One agent task. Mutated only through [`AgentTaskManager`], which checks identity.
#[derive(Debug, Clone)]
pub struct AgentTask {
    context: AgentTaskContext,
    state: TaskState,
    repair: RepairBudget,
    provider_session: Option<String>,
    writer: Option<WriterRecord>,
    /// Current writer epoch; bumped by every writer start and repair.
    epoch: u64,
    /// The single outstanding capture ticket nonce, if one was issued in this epoch.
    issued_ticket: Option<u64>,
    next_nonce: u64,
    candidate: Option<CandidateRevision>,
    /// SHA-256 of the captured checkpoint manifest of `candidate`.
    candidate_manifest: Option<String>,
    source_invalidated: bool,
    reason: Option<String>,
}

impl AgentTask {
    fn new(context: AgentTaskContext) -> Self {
        Self {
            context,
            state: TaskState::ContextReady,
            repair: RepairBudget::automatic(),
            provider_session: None,
            writer: None,
            epoch: 0,
            issued_ticket: None,
            next_nonce: 0,
            candidate: None,
            candidate_manifest: None,
            source_invalidated: false,
            reason: None,
        }
    }
    pub fn identity(&self) -> &TaskIdentity {
        &self.context.identity
    }
    pub fn context(&self) -> &AgentTaskContext {
        &self.context
    }
    pub fn state(&self) -> TaskState {
        self.state
    }
    pub fn repair(&self) -> &RepairBudget {
        &self.repair
    }
    pub fn provider_session(&self) -> Option<&str> {
        self.provider_session.as_deref()
    }
    pub fn candidate(&self) -> Option<&CandidateRevision> {
        self.candidate.as_ref()
    }
    /// Manifest hash recorded with the candidate at capture.
    pub fn candidate_manifest(&self) -> Option<&str> {
        self.candidate_manifest.as_deref()
    }
    pub fn writer_ownership(&self) -> Option<&WriterOwnership> {
        self.writer.as_ref().map(|w| &w.ownership)
    }
    /// The current writer generation, if a writer was started in this epoch.
    pub fn writer_generation(&self) -> Option<WriterGeneration> {
        self.writer.as_ref().map(|w| w.generation)
    }
    /// The project source became unreadable/invalid while the task ran. Sticky: an
    /// edit back to the base bytes never revives the task.
    pub fn source_invalidated(&self) -> bool {
        self.source_invalidated
    }
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
    /// No owned writer can still be modifying the draft: either none ever started,
    /// or its termination was verified under a qualified ownership model.
    pub fn writer_safe(&self) -> bool {
        match &self.writer {
            None => true,
            Some(w) => {
                w.ownership.is_qualified() && w.termination.as_ref().is_some_and(|t| t.verified())
            }
        }
    }
    pub(crate) fn unsafe_reason(&self) -> String {
        match &self.writer {
            None => "no writer started".to_owned(),
            Some(w) if !w.ownership.is_qualified() => format!(
                "adapter writer ownership is {}; a detached or unknown writer may still modify the draft",
                w.ownership.label()
            ),
            Some(_) => {
                "adapter termination was not verified; a writer may still be running".to_owned()
            }
        }
    }

    /// Starts a new writer epoch: every outstanding ticket and all earlier evidence die.
    fn bump_epoch(&mut self) -> Result<WriterGeneration, TaskError> {
        self.epoch = self
            .epoch
            .checked_add(1)
            .ok_or(TaskError::CounterExhausted)?;
        self.issued_ticket = None;
        Ok(WriterGeneration(self.epoch))
    }

    /// Lowers the recorded ownership with a driver observation; escapes force `Detached`.
    fn demote_with(&mut self, observed: &WriterObservation) {
        if let Some(writer) = self.writer.as_mut() {
            writer.ownership = demote(&writer.ownership, &observed.ownership);
            if !observed.escaped_pids.is_empty() {
                writer.ownership = WriterOwnership::Detached;
            }
        }
    }

    /// Applies termination/escape facts derived from the task's own process scope.
    fn apply_scope_facts(&mut self, facts: ScopeFacts) {
        match self.writer.as_mut() {
            Some(writer) => {
                if !facts.escaped.is_empty() {
                    writer.ownership = WriterOwnership::Detached;
                }
                writer.termination = Some(facts.termination);
            }
            // Processes lived in the task scope but no writer was ever registered for
            // them: their ownership model is unproven.
            None if facts.had_live
                || !facts.escaped.is_empty()
                || !facts.termination.verified() =>
            {
                let ownership = if facts.escaped.is_empty() {
                    WriterOwnership::Unknown
                } else {
                    WriterOwnership::Detached
                };
                self.writer = Some(WriterRecord {
                    generation: WriterGeneration(self.epoch),
                    ownership,
                    termination: Some(facts.termination),
                });
            }
            None => {}
        }
    }
}

/// What the controller must record about the draft when a task ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedTask {
    pub identity: TaskIdentity,
    pub state: TaskState,
    pub writer_safe: bool,
    pub reason: String,
    pub unsafe_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WriterLease {
    task: AgentTaskId,
}

/// Serialized owner of the single active task and the project's one writer lease.
/// Lives inside the per-project controller, so every call is already exclusive.
#[derive(Debug)]
pub struct AgentTaskManager {
    project: ProjectId,
    session: OpenSession,
    next_generation: u64,
    active: Option<AgentTask>,
    lease: Option<WriterLease>,
    last_finished: Option<AgentTask>,
}

/// Reserved identity for a task that is still being prepared.
#[derive(Debug)]
pub(crate) struct TaskReservation {
    pub(crate) identity: TaskIdentity,
}

impl AgentTaskManager {
    pub fn new(project: ProjectId, session: OpenSession) -> Self {
        Self {
            project,
            session,
            next_generation: 0,
            active: None,
            lease: None,
            last_finished: None,
        }
    }

    /// Starts allocation at `next` (for counter exhaustion tests).
    pub fn with_next_generation(mut self, next: u64) -> Self {
        self.next_generation = next;
        self
    }

    pub fn lease_holder(&self) -> Option<&AgentTaskId> {
        self.lease.as_ref().map(|l| &l.task)
    }

    pub fn active(&self) -> Option<&AgentTask> {
        self.active.as_ref()
    }

    /// The active task, or the most recently finished one.
    pub fn current(&self) -> Option<&AgentTask> {
        self.active.as_ref().or(self.last_finished.as_ref())
    }

    pub(crate) fn reserve(&mut self) -> Result<TaskReservation, TaskError> {
        if let Some(lease) = &self.lease {
            return Err(TaskError::LeaseHeld(lease.task.clone()));
        }
        let generation = self.next_generation;
        self.next_generation = generation
            .checked_add(1)
            .ok_or(TaskError::CounterExhausted)?;
        Ok(TaskReservation {
            identity: TaskIdentity {
                task: AgentTaskId::new(),
                project: self.project.clone(),
                session: self.session.clone(),
                generation,
            },
        })
    }

    /// Allocates a fresh generation for an identity that owns no writer lease (Undo).
    pub(crate) fn allocate_generation(&mut self) -> Result<u64, TaskError> {
        let generation = self.next_generation;
        self.next_generation = generation
            .checked_add(1)
            .ok_or(TaskError::CounterExhausted)?;
        Ok(generation)
    }

    /// Installs a prepared task and takes the writer lease.
    pub(crate) fn begin(&mut self, context: AgentTaskContext) -> Result<(), TaskError> {
        if let Some(lease) = &self.lease {
            return Err(TaskError::LeaseHeld(lease.task.clone()));
        }
        self.lease = Some(WriterLease {
            task: context.identity.task.clone(),
        });
        self.last_finished = None;
        self.active = Some(AgentTask::new(context));
        Ok(())
    }

    fn check(&self, identity: &TaskIdentity) -> Result<&AgentTask, TaskError> {
        let task = self.active.as_ref().ok_or(TaskError::NoActiveTask)?;
        if task.identity() != identity || identity.session != self.session {
            return Err(TaskError::StaleIdentity);
        }
        Ok(task)
    }

    fn check_mut(&mut self, identity: &TaskIdentity) -> Result<&mut AgentTask, TaskError> {
        self.check(identity)?;
        Ok(self.active.as_mut().expect("checked"))
    }

    /// Non-terminal transitions that need no extra proof. Candidate registration,
    /// repair, and terminal states use their dedicated operations.
    pub fn transition(
        &mut self,
        identity: &TaskIdentity,
        next: TaskState,
    ) -> Result<(), TaskError> {
        let task = self.check_mut(identity)?;
        if next.is_terminal() {
            return Err(TaskError::GatedTransition(next));
        }
        if !task.state.can_transition_to(next) {
            return Err(TaskError::InvalidTransition {
                from: task.state,
                to: next,
            });
        }
        if matches!(next, TaskState::Validating | TaskState::RepairNeeded)
            || next == TaskState::CandidateReady
            || (task.state == TaskState::RepairNeeded && next == TaskState::Editing)
        {
            return Err(TaskError::GatedTransition(next));
        }
        task.state = next;
        Ok(())
    }

    /// Validates the identity against the active task without touching anything.
    pub(crate) fn validate(&self, identity: &TaskIdentity) -> Result<&AgentTask, TaskError> {
        self.check(identity)
    }

    /// Records that a provider writer was started for this task and how its writers
    /// are owned, beginning a new writer generation. All earlier evidence and every
    /// outstanding capture ticket die. Ownership is sticky within the task: a writer
    /// already recorded as less trusted keeps its demotion.
    pub fn writer_started(
        &mut self,
        identity: &TaskIdentity,
        ownership: WriterOwnership,
    ) -> Result<WriterGeneration, TaskError> {
        let task = self.check_mut(identity)?;
        let generation = task.bump_epoch()?;
        let ownership = match &task.writer {
            Some(previous) => demote(&previous.ownership, &ownership),
            None => ownership,
        };
        task.writer = Some(WriterRecord {
            generation,
            ownership,
            termination: None,
        });
        Ok(generation)
    }

    /// Applies the driver's view of the writer (its `DriverOutcome`). It can only lower
    /// the recorded ownership; it never proves termination.
    pub fn observe_writer(
        &mut self,
        identity: &TaskIdentity,
        observed: &WriterObservation,
    ) -> Result<(), TaskError> {
        let task = self.check_mut(identity)?;
        if task.writer.is_none() {
            return Err(TaskError::NoWriter);
        }
        task.demote_with(observed);
        Ok(())
    }

    pub fn record_provider_session(
        &mut self,
        identity: &TaskIdentity,
        session: &str,
    ) -> Result<(), TaskError> {
        self.check_mut(identity)?.provider_session = Some(session.to_owned());
        Ok(())
    }

    /// Records termination and escapes the controller derived from this task's own
    /// process scope at teardown. Escapes demote the ownership for good.
    pub(crate) fn record_scope_facts(
        &mut self,
        identity: &TaskIdentity,
        facts: ScopeFacts,
    ) -> Result<(), TaskError> {
        self.check_mut(identity)?.apply_scope_facts(facts);
        Ok(())
    }

    /// Evaluates the quiescence gate. `observe` seals the task's process scope against
    /// further spawns and observes it; it runs only after the identity, state, writer
    /// and evidence-binding checks pass, so foreign or stale evidence never reaches or
    /// affects a scope. The recorded ownership is sticky (evidence can only demote it)
    /// and termination comes from `observe`, never from the caller. Every attempt
    /// retires the outstanding ticket; only a pass yields a new one.
    pub(crate) fn complete_quiescence(
        &mut self,
        identity: &TaskIdentity,
        evidence: &QuiescenceEvidence,
        observe: impl FnOnce() -> ScopeFacts,
    ) -> Result<CaptureTicket, TaskError> {
        let task = self.check_mut(identity)?;
        if task.state != TaskState::Quiescing {
            return Err(TaskError::WrongState {
                expected: TaskState::Quiescing,
                actual: task.state,
            });
        }
        task.issued_ticket = None;
        let block = TaskError::QuiescenceBlocked;
        if evidence.identity != *identity {
            return Err(block(QuiescenceBlock::ForeignEvidence));
        }
        let Some(writer) = task.writer.as_ref() else {
            return Err(block(QuiescenceBlock::NoWriter));
        };
        if evidence.writer != writer.generation || writer.generation.0 != task.epoch {
            return Err(block(QuiescenceBlock::StaleWriter {
                found: evidence.writer.0,
                current: writer.generation.0,
            }));
        }
        if evidence.provider_session != task.provider_session {
            return Err(block(QuiescenceBlock::ProviderSessionMismatch));
        }
        task.demote_with(&evidence.observed);
        task.apply_scope_facts(observe());
        let writer = task.writer.as_ref().expect("writer checked above");
        let termination = writer
            .termination
            .as_ref()
            .expect("scope facts always record termination for a registered writer");
        evaluate_quiescence(evidence, &writer.ownership, termination).map_err(block)?;
        let nonce = task.next_nonce;
        task.next_nonce = nonce.checked_add(1).ok_or(TaskError::CounterExhausted)?;
        task.issued_ticket = Some(nonce);
        Ok(CaptureTicket {
            identity: task.context.identity.clone(),
            base: task.context.source_base.clone(),
            draft: task.context.draft.clone(),
            generation: WriterGeneration(task.epoch),
            nonce,
        })
    }

    /// Registers the immutable candidate captured under `ticket` and starts validation.
    /// The ticket is single-use and must match the stored writer generation and nonce.
    pub fn record_candidate(
        &mut self,
        ticket: CaptureTicket,
        candidate: CandidateRevision,
        manifest_sha256: String,
    ) -> Result<(), TaskError> {
        self.check_ticket(&ticket)?;
        let task = self.check_mut(&ticket.identity)?;
        task.issued_ticket = None;
        task.candidate = Some(candidate);
        task.candidate_manifest = Some(manifest_sha256);
        task.state = TaskState::Validating;
        Ok(())
    }

    /// Whether `ticket` is still the single outstanding ticket of the current writer
    /// epoch of its task. Nothing changes.
    pub(crate) fn check_ticket(&self, ticket: &CaptureTicket) -> Result<(), TaskError> {
        let task = self.check(&ticket.identity)?;
        if task.state != TaskState::Quiescing {
            return Err(TaskError::WrongState {
                expected: TaskState::Quiescing,
                actual: task.state,
            });
        }
        if ticket.generation.0 != task.epoch
            || task.issued_ticket != Some(ticket.nonce)
            || !task.writer_safe()
        {
            return Err(TaskError::StaleTicket);
        }
        Ok(())
    }

    /// A passing validation report for the recorded candidate: `Validating` ->
    /// `CandidateReady`.
    pub(crate) fn candidate_validated(
        &mut self,
        identity: &TaskIdentity,
        candidate: &CandidateRevision,
    ) -> Result<(), TaskError> {
        let task = self.check_mut(identity)?;
        if task.state != TaskState::Validating {
            return Err(TaskError::WrongState {
                expected: TaskState::Validating,
                actual: task.state,
            });
        }
        if task.candidate.as_ref() != Some(candidate) {
            return Err(TaskError::ReportMismatch);
        }
        task.state = TaskState::CandidateReady;
        Ok(())
    }

    /// Validation failed: spend the repair budget or fail the task.
    pub fn validation_failed(
        &mut self,
        identity: &TaskIdentity,
        reason: &str,
    ) -> Result<RepairDecision, TaskError> {
        let task = self.check_mut(identity)?;
        if task.state != TaskState::Validating {
            return Err(TaskError::WrongState {
                expected: TaskState::Validating,
                actual: task.state,
            });
        }
        task.candidate = None;
        task.candidate_manifest = None;
        task.issued_ticket = None;
        if task.repair.remaining() == 0 {
            self.finish(identity, TaskState::Failed, reason)?;
            return Ok(RepairDecision::Exhausted);
        }
        let task = self.check_mut(identity)?;
        task.state = TaskState::RepairNeeded;
        task.reason = Some(reason.to_owned());
        Ok(RepairDecision::Repair {
            attempt: task.repair.used() + 1,
        })
    }

    /// Starts the automatic repair attempt as a new writer epoch: the previous writer
    /// record, evidence and every ticket are discarded, so a new writer needs fresh
    /// quiescence evidence. `ready` runs after validation and before any mutation (the
    /// controller seals and verifies the previous process scope there); if it fails the
    /// task stays `RepairNeeded`, untouched.
    pub(crate) fn begin_repair(
        &mut self,
        identity: &TaskIdentity,
        ready: impl FnOnce() -> Result<(), TaskError>,
    ) -> Result<u32, TaskError> {
        let task = self.check_mut(identity)?;
        if task.state != TaskState::RepairNeeded {
            return Err(TaskError::WrongState {
                expected: TaskState::RepairNeeded,
                actual: task.state,
            });
        }
        if task.repair.remaining() == 0 {
            return Err(TaskError::RepairExhausted);
        }
        if task.epoch == u64::MAX {
            return Err(TaskError::CounterExhausted);
        }
        ready()?;
        let attempt = task.repair.consume()?;
        task.bump_epoch()?;
        task.state = TaskState::Editing;
        task.writer = None;
        Ok(attempt)
    }

    /// Validates that `finish(identity, terminal, ..)` would succeed, without changing
    /// anything. The controller calls this before touching any process scope.
    pub(crate) fn check_finish(
        &self,
        identity: &TaskIdentity,
        terminal: TaskState,
    ) -> Result<(), TaskError> {
        let task = self.check(identity)?;
        if !terminal.is_terminal() || !task.state.can_transition_to(terminal) {
            return Err(TaskError::InvalidTransition {
                from: task.state,
                to: terminal,
            });
        }
        Ok(())
    }

    /// Settles the task in a terminal state and releases the writer lease.
    pub fn finish(
        &mut self,
        identity: &TaskIdentity,
        terminal: TaskState,
        reason: &str,
    ) -> Result<FinishedTask, TaskError> {
        self.check_finish(identity, terminal)?;
        let task = self.check_mut(identity)?;
        task.state = terminal;
        task.reason = Some(reason.to_owned());
        task.issued_ticket = None;
        let task = self.active.take().expect("checked");
        self.lease = None;
        let finished = FinishedTask {
            identity: task.context.identity.clone(),
            state: terminal,
            writer_safe: task.writer_safe(),
            reason: reason.to_owned(),
            unsafe_reason: (!task.writer_safe()).then(|| task.unsafe_reason()),
        };
        self.last_finished = Some(task);
        Ok(finished)
    }

    /// The project source became invalid: the active task is interrupted and can never
    /// resume, even if the source later returns to its base bytes.
    pub(crate) fn invalidate_source(&mut self, reason: &str) -> Option<FinishedTask> {
        let task = self.active.as_mut()?;
        task.source_invalidated = true;
        let identity = task.context.identity.clone();
        self.finish(&identity, TaskState::Interrupted, reason).ok()
    }
}

// ---- stable draft ------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DraftState {
    /// A task owns the draft; seen after a restart it means the app ended abnormally.
    Active { task: String },
    /// A failed/cancelled/interrupted task's draft; archived before the next refresh.
    Retained { task: String, reason: String },
    /// The task's bytes were published; the draft may be replaced.
    Accepted { task: String },
    /// A writer may still be running; never refreshed until acknowledged.
    UnsafeWriter { task: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftSnapshot {
    pub base: SourceRevision,
    pub state: DraftState,
}

/// Evidence that no writer can still be modifying a draft marked `UnsafeWriter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriterGoneEvidence {
    /// The user explicitly confirmed that nothing is writing to the draft.
    UserConfirmed,
    /// Verified termination under a qualified process-group model.
    QualifiedAndVerified {
        ownership: WriterOwnership,
        termination: TerminationReport,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedDraft {
    pub path: PathBuf,
    pub archived: Option<PathBuf>,
}

/// The stable per-project agent working directory plus its durable ownership marker.
#[derive(Debug, Clone)]
pub struct DraftStore {
    dir: PathBuf,
    marker: PathBuf,
    archive: PathBuf,
}

impl DraftStore {
    pub fn new(paths: &crate::app_paths::AppPaths, project: &ProjectId) -> Self {
        Self {
            dir: paths.agent_draft(project),
            marker: paths.agent_draft_state(project),
            archive: paths.agent_archive(project),
        }
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn archive_dir(&self) -> &Path {
        &self.archive
    }

    pub fn snapshot(&self) -> Result<Option<DraftSnapshot>, TaskError> {
        match fs::read(&self.marker) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| {
                TaskError::DraftUnsafe(format!("draft ownership marker is unreadable ({e})"))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(storage(e)),
        }
    }

    fn write(&self, snapshot: &DraftSnapshot) -> Result<(), TaskError> {
        let parent = self.marker.parent().expect("marker has a parent");
        fs::create_dir_all(parent).map_err(storage)?;
        atomic_write(
            &self.marker,
            &serde_json::to_vec(snapshot).map_err(storage)?,
        )
        .map_err(storage)
    }

    /// After a restart an `Active` marker means the previous session never released the
    /// lease: a writer may still be running, so the draft becomes unsafe.
    pub fn recover_after_restart(&self) -> Result<(), TaskError> {
        if let Some(mut snapshot) = self.snapshot()?
            && let DraftState::Active { task } = &snapshot.state
        {
            snapshot.state = DraftState::UnsafeWriter {
                task: task.clone(),
                reason: "the previous session ended without releasing the writer lease; its adapter may still be running".into(),
            };
            self.write(&snapshot)?;
        }
        Ok(())
    }

    /// Updates the marker for the draft's current owner (task end, acknowledgement).
    pub fn set_state(&self, state: DraftState) -> Result<(), TaskError> {
        let mut snapshot = self
            .snapshot()?
            .ok_or_else(|| TaskError::Storage("draft ownership marker is missing".into()))?;
        snapshot.state = state;
        self.write(&snapshot)
    }

    /// Clears an `UnsafeWriter` marker given evidence that no writer remains. The
    /// draft becomes `Retained` and is archived before its next refresh.
    pub fn acknowledge_writer_gone(&self, evidence: &WriterGoneEvidence) -> Result<(), TaskError> {
        let Some(snapshot) = self.snapshot()? else {
            return Ok(());
        };
        let DraftState::UnsafeWriter { task, .. } = snapshot.state else {
            return Ok(());
        };
        match evidence {
            WriterGoneEvidence::UserConfirmed => {}
            WriterGoneEvidence::QualifiedAndVerified {
                ownership,
                termination,
            } => {
                if !ownership.is_qualified() || !termination.verified() {
                    return Err(TaskError::DraftUnsafe(
                        "evidence does not prove the writer is gone".into(),
                    ));
                }
            }
        }
        self.set_state(DraftState::Retained {
            task,
            reason: "writer confirmed gone".into(),
        })
    }

    /// Materializes the stable draft from `base` for `task`.
    ///
    /// Never runs under a live provider session or while an unknown writer may
    /// survive; a retained failed draft is archived first, not overwritten.
    /// Materializes the stable draft from `content_revision` for `task`, setting its
    /// source base to `base`.
    ///
    /// Never runs under a live provider session or while an unknown writer may
    /// survive; a retained failed draft is archived first, not overwritten.
    pub fn prepare_from(
        &self,
        checkpoints: &Checkpoints,
        content_revision: &studio_project::SourceRevision,
        base: &TaskSourceBase,
        task: &AgentTaskId,
        session_live: bool,
    ) -> Result<PreparedDraft, TaskError> {
        if session_live {
            return Err(TaskError::DraftBusy);
        }
        let previous = self.snapshot()?;
        let mut archived = None;
        match previous.as_ref().map(|s| &s.state) {
            Some(DraftState::Active { task }) => {
                return Err(TaskError::DraftUnsafe(format!(
                    "task {task} still owns the draft and a writer may be running"
                )));
            }
            Some(DraftState::UnsafeWriter { reason, .. }) => {
                return Err(TaskError::DraftUnsafe(reason.clone()));
            }
            Some(DraftState::Retained { task, .. }) if self.dir.exists() => {
                archived = Some(self.archive_current(task)?);
            }
            // No marker but a directory: an interrupted refresh that never ran a writer.
            None if self.dir.exists() => {
                archived = Some(self.archive_current("unmarked")?);
            }
            Some(DraftState::Accepted { .. }) if self.dir.exists() => {
                fs::remove_dir_all(&self.dir).map_err(storage)?;
            }
            _ => {}
        }
        let parent = self.dir.parent().expect("draft has a parent");
        fs::create_dir_all(parent).map_err(storage)?;
        let staging = format!("agent-{}", task.0);
        let staged = checkpoints
            .draft(content_revision, &staging)
            .map_err(storage)?;
        if let Err(error) = fs::rename(&staged, &self.dir) {
            let _ = fs::remove_dir_all(&staged);
            return Err(storage(error));
        }
        sync_directory(parent).map_err(storage)?;
        self.write(&DraftSnapshot {
            base: base.revision().clone(),
            state: DraftState::Active {
                task: task.0.clone(),
            },
        })?;
        Ok(PreparedDraft {
            path: self.dir.clone(),
            archived,
        })
    }

    /// Materializes the stable draft from `base` for `task`.
    pub fn prepare(
        &self,
        checkpoints: &Checkpoints,
        base: &TaskSourceBase,
        task: &AgentTaskId,
        session_live: bool,
    ) -> Result<PreparedDraft, TaskError> {
        self.prepare_from(checkpoints, base.revision(), base, task, session_live)
    }

    fn archive_current(&self, owner: &str) -> Result<PathBuf, TaskError> {
        fs::create_dir_all(&self.archive).map_err(storage)?;
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut target = self.archive.join(format!("{secs}-{owner}"));
        let mut counter = 1;
        while target.exists() {
            target = self.archive.join(format!("{secs}-{owner}-{counter}"));
            counter += 1;
        }
        fs::rename(&self.dir, &target).map_err(storage)?;
        sync_directory(&self.archive).map_err(storage)?;
        Ok(target)
    }
}

/// Reads whole-project brief limits shared by every entry point.
pub fn validate_brief(brief: &str) -> Result<String, TaskError> {
    let trimmed = brief.trim();
    if trimmed.is_empty() {
        return Err(TaskError::InvalidBrief("the task brief is empty"));
    }
    if trimmed.len() > MAX_BRIEF_BYTES {
        return Err(TaskError::InvalidBrief(
            "the task brief exceeds 64 KiB; shorten it",
        ));
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_bootstrap::GroupMembership;

    fn manager() -> AgentTaskManager {
        AgentTaskManager::new(
            ProjectId::try_from("project-1".to_owned()).unwrap(),
            OpenSession::new(),
        )
    }

    fn revision(c: char) -> SourceRevision {
        SourceRevision::try_from(c.to_string().repeat(64)).unwrap()
    }

    fn context(m: &mut AgentTaskManager) -> AgentTaskContext {
        let reservation = m.reserve().unwrap();
        AgentTaskContext {
            identity: reservation.identity,
            source_base: TaskSourceBase::new(revision('a')),
            scope: TaskScope::whole_project("project-1", revision('a').as_str()),
            prior_checkpoint: revision('b'),
            assets: vec![],
            instructions: vec![],
            brief: "make it blue".into(),
            draft: PathBuf::from("/draft"),
            archived_previous: None,
        }
    }

    fn qualified() -> WriterOwnership {
        WriterOwnership::ProcessGroupContained {
            qualification: "test".into(),
        }
    }

    fn clean_termination() -> TerminationReport {
        TerminationReport {
            forced: false,
            direct_child_exited: true,
            group_empty: true,
            remaining: vec![],
        }
    }

    fn clean_facts() -> ScopeFacts {
        ScopeFacts {
            termination: clean_termination(),
            escaped: vec![],
            had_live: false,
        }
    }

    fn evidence_with(id: &TaskIdentity, writer: WriterGeneration) -> QuiescenceEvidence {
        QuiescenceEvidence {
            identity: id.clone(),
            provider_session: None,
            writer,
            completion: TurnCompletion::EndTurn,
            cancel_requested: false,
            unresolved_requests: 0,
            observed: WriterObservation {
                ownership: qualified(),
                escaped_pids: vec![],
            },
        }
    }

    /// Clean end-turn evidence bound to the task's current writer generation.
    fn evidence_for(m: &AgentTaskManager, id: &TaskIdentity) -> QuiescenceEvidence {
        evidence_with(id, m.active().unwrap().writer_generation().unwrap())
    }

    fn quiesce(m: &mut AgentTaskManager, id: &TaskIdentity) -> Result<CaptureTicket, TaskError> {
        let evidence = evidence_for(m, id);
        m.complete_quiescence(id, &evidence, clean_facts)
    }

    fn started(m: &mut AgentTaskManager) -> TaskIdentity {
        let context = context(m);
        let identity = context.identity.clone();
        m.begin(context).unwrap();
        m.writer_started(&identity, qualified()).unwrap();
        identity
    }

    #[test]
    fn transition_table_matches_the_documented_lifecycle() {
        use TaskState::*;
        let all = [
            ContextReady,
            Editing,
            Waiting,
            Quiescing,
            Validating,
            RepairNeeded,
            CandidateReady,
            Promoting,
            Accepted,
            Conflict,
            Failed,
            Cancelled,
            Interrupted,
        ];
        for terminal in all.iter().filter(|s| s.is_terminal()) {
            for next in all {
                assert!(!terminal.can_transition_to(next), "{terminal:?} is final");
            }
        }
        for from in all.iter().filter(|s| !s.is_terminal()) {
            assert!(
                from.can_transition_to(Interrupted),
                "{from:?} can be interrupted"
            );
        }
        assert!(ContextReady.can_transition_to(Editing));
        assert!(Editing.can_transition_to(Quiescing));
        assert!(Quiescing.can_transition_to(Validating));
        assert!(Validating.can_transition_to(RepairNeeded));
        assert!(RepairNeeded.can_transition_to(Editing));
        assert!(CandidateReady.can_transition_to(Promoting));
        assert!(Promoting.can_transition_to(Accepted));
        assert!(Promoting.can_transition_to(Conflict));
        // No shortcuts around validation, quiescence or promotion.
        assert!(!Editing.can_transition_to(Validating));
        assert!(!Editing.can_transition_to(CandidateReady));
        assert!(!Quiescing.can_transition_to(CandidateReady));
        assert!(!Validating.can_transition_to(Promoting));
        assert!(!CandidateReady.can_transition_to(Accepted));
        assert!(!ContextReady.can_transition_to(Quiescing));
        assert!(!Waiting.can_transition_to(Quiescing));
        assert!(!Promoting.can_transition_to(Cancelled));
    }

    #[test]
    fn quiescence_requires_end_turn_verified_termination_and_qualified_ownership() {
        let id = TaskIdentity {
            task: AgentTaskId::new(),
            project: ProjectId::try_from("project-1".to_owned()).unwrap(),
            session: OpenSession::new(),
            generation: 0,
        };
        let clean = evidence_with(&id, WriterGeneration(1));
        let eval = |e: &QuiescenceEvidence, o: &WriterOwnership, t: &TerminationReport| {
            evaluate_quiescence(e, o, t)
        };
        assert_eq!(eval(&clean, &qualified(), &clean_termination()), Ok(()));
        let mut e = clean.clone();
        e.completion = TurnCompletion::None;
        assert_eq!(
            eval(&e, &qualified(), &clean_termination()),
            Err(QuiescenceBlock::NoAuthoritativeCompletion)
        );
        for completion in [
            TurnCompletion::Refusal,
            TurnCompletion::Cancelled,
            TurnCompletion::MaxTokens,
            TurnCompletion::MaxTurnRequests,
        ] {
            let mut e = clean.clone();
            e.completion = completion;
            assert_eq!(
                eval(&e, &qualified(), &clean_termination()),
                Err(QuiescenceBlock::NotEndTurn(completion))
            );
        }
        let mut e = clean.clone();
        e.cancel_requested = true;
        assert_eq!(
            eval(&e, &qualified(), &clean_termination()),
            Err(QuiescenceBlock::CancelRequested)
        );
        let mut e = clean.clone();
        e.unresolved_requests = 2;
        assert_eq!(
            eval(&e, &qualified(), &clean_termination()),
            Err(QuiescenceBlock::UnresolvedRequests(2))
        );
        let mut t = clean_termination();
        t.group_empty = false;
        t.remaining = vec![7];
        assert_eq!(
            eval(&clean, &qualified(), &t),
            Err(QuiescenceBlock::GroupSurvivors(vec![7]))
        );
        let mut t = clean_termination();
        t.direct_child_exited = false;
        assert_eq!(
            eval(&clean, &qualified(), &t),
            Err(QuiescenceBlock::WriterNotTerminated)
        );
        for ownership in [WriterOwnership::Unknown, WriterOwnership::Detached] {
            assert_eq!(
                eval(&clean, &ownership, &clean_termination()),
                Err(QuiescenceBlock::UnqualifiedOwnership(ownership.label()))
            );
        }
        let _ = GroupMembership::Empty;
    }

    #[test]
    fn stop_reasons_map_and_unknown_reasons_are_not_completions() {
        assert_eq!(
            TurnCompletion::from_stop_reason("end_turn"),
            TurnCompletion::EndTurn
        );
        assert_eq!(
            TurnCompletion::from_stop_reason("refusal"),
            TurnCompletion::Refusal
        );
        assert_eq!(
            TurnCompletion::from_stop_reason("bogus"),
            TurnCompletion::None
        );
    }

    #[test]
    fn one_writer_lease_per_project_is_released_when_the_task_ends() {
        let mut m = manager();
        let id = started(&mut m);
        assert_eq!(m.lease_holder(), Some(&id.task));
        assert!(matches!(m.reserve(), Err(TaskError::LeaseHeld(t)) if t == id.task));
        m.finish(&id, TaskState::Cancelled, "user stop").unwrap();
        assert_eq!(m.lease_holder(), None);
        assert!(m.active().is_none());
        assert_eq!(m.current().unwrap().state(), TaskState::Cancelled);
        m.reserve().expect("lease is free again");
    }

    #[test]
    fn stale_identities_are_rejected_across_generations_and_sessions() {
        let mut m = manager();
        let first = started(&mut m);
        m.finish(&first, TaskState::Cancelled, "done").unwrap();
        let second = started(&mut m);
        assert_eq!(second.generation, first.generation + 1);
        assert_eq!(
            m.transition(&first, TaskState::Editing),
            Err(TaskError::StaleIdentity)
        );
        let mut other_session = second.clone();
        other_session.session = OpenSession::new();
        assert_eq!(
            m.transition(&other_session, TaskState::Editing),
            Err(TaskError::StaleIdentity)
        );
        let mut other_generation = second.clone();
        other_generation.generation += 1;
        assert_eq!(
            m.transition(&other_generation, TaskState::Editing),
            Err(TaskError::StaleIdentity)
        );
        assert_eq!(m.transition(&second, TaskState::Editing), Ok(()));
    }

    #[test]
    fn identity_counter_exhaustion_is_an_error_not_a_wraparound() {
        let mut m = manager().with_next_generation(u64::MAX - 1);
        let last = m.reserve().unwrap();
        assert_eq!(last.identity.generation, u64::MAX - 1);
        assert_eq!(m.reserve().err(), Some(TaskError::CounterExhausted));
        assert_eq!(
            m.reserve().err(),
            Some(TaskError::CounterExhausted),
            "stays exhausted"
        );
    }

    #[test]
    fn dedicated_operations_guard_validation_repair_and_terminal_states() {
        let mut m = manager();
        let id = started(&mut m);
        m.transition(&id, TaskState::Editing).unwrap();
        assert_eq!(
            m.transition(&id, TaskState::Validating),
            Err(TaskError::InvalidTransition {
                from: TaskState::Editing,
                to: TaskState::Validating
            })
        );
        assert_eq!(
            m.transition(&id, TaskState::Failed),
            Err(TaskError::GatedTransition(TaskState::Failed))
        );
        assert_eq!(
            m.transition(&id, TaskState::CandidateReady),
            Err(TaskError::InvalidTransition {
                from: TaskState::Editing,
                to: TaskState::CandidateReady
            })
        );
        assert_eq!(
            m.begin_repair(&id, || Ok(())),
            Err(TaskError::WrongState {
                expected: TaskState::RepairNeeded,
                actual: TaskState::Editing
            })
        );
        m.transition(&id, TaskState::Waiting).unwrap();
        m.transition(&id, TaskState::Editing).unwrap();
        m.transition(&id, TaskState::Quiescing).unwrap();
        assert_eq!(
            m.transition(&id, TaskState::Validating),
            Err(TaskError::GatedTransition(TaskState::Validating))
        );
        assert!(matches!(
            m.finish(&id, TaskState::Accepted, "no"),
            Err(TaskError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn capture_requires_the_gate_and_a_blocked_gate_leaves_the_task_quiescing() {
        let mut m = manager();
        let id = started(&mut m);
        assert!(matches!(
            quiesce(&mut m, &id),
            Err(TaskError::WrongState {
                expected: TaskState::Quiescing,
                ..
            })
        ));
        m.transition(&id, TaskState::Editing).unwrap();
        m.transition(&id, TaskState::Quiescing).unwrap();
        // The scope still shows a live member: whatever the evidence claims, no ticket.
        let evidence = evidence_for(&m, &id);
        let live = ScopeFacts {
            termination: TerminationReport {
                forced: false,
                direct_child_exited: false,
                group_empty: false,
                remaining: vec![4242],
            },
            escaped: vec![],
            had_live: true,
        };
        assert_eq!(
            m.complete_quiescence(&id, &evidence, || live).err(),
            Some(TaskError::QuiescenceBlocked(
                QuiescenceBlock::GroupSurvivors(vec![4242])
            ))
        );
        assert_eq!(m.active().unwrap().state(), TaskState::Quiescing);
        assert!(
            !m.active().unwrap().writer_safe(),
            "a live member keeps the draft unsafe"
        );
        let ticket = quiesce(&mut m, &id).unwrap();
        assert_eq!(ticket.source_base().revision(), &revision('a'));
        m.record_candidate(ticket, CandidateRevision::new(revision('c')), "m".into())
            .unwrap();
        assert_eq!(m.active().unwrap().state(), TaskState::Validating);
        assert_eq!(
            m.active().unwrap().candidate().unwrap().revision(),
            &revision('c')
        );
        assert!(m.active().unwrap().writer_safe());
    }

    #[test]
    fn repair_budget_allows_exactly_one_automatic_repair_then_fails() {
        let mut m = manager();
        let id = started(&mut m);
        let capture = |m: &mut AgentTaskManager| {
            m.transition(&id, TaskState::Quiescing).unwrap();
            let ticket = quiesce(m, &id).unwrap();
            m.record_candidate(ticket, CandidateRevision::new(revision('c')), "m".into())
                .unwrap();
        };
        m.transition(&id, TaskState::Editing).unwrap();
        capture(&mut m);
        assert_eq!(
            m.validation_failed(&id, "compile error").unwrap(),
            RepairDecision::Repair { attempt: 1 }
        );
        assert_eq!(m.active().unwrap().state(), TaskState::RepairNeeded);
        assert_eq!(
            m.active().unwrap().repair().remaining(),
            1,
            "spent only when the repair starts"
        );
        assert_eq!(m.begin_repair(&id, || Ok(())).unwrap(), 1);
        m.writer_started(&id, qualified()).unwrap();
        assert_eq!(m.active().unwrap().repair().remaining(), 0);
        assert!(
            m.active().unwrap().candidate().is_none(),
            "a failed candidate is dropped"
        );
        capture(&mut m);
        assert_eq!(
            m.validation_failed(&id, "still broken").unwrap(),
            RepairDecision::Exhausted
        );
        assert!(m.active().is_none());
        assert_eq!(m.current().unwrap().state(), TaskState::Failed);
        assert_eq!(m.lease_holder(), None);
    }

    #[test]
    fn repair_requires_fresh_quiescence_evidence() {
        let mut m = manager();
        let id = started(&mut m);
        m.transition(&id, TaskState::Editing).unwrap();
        m.transition(&id, TaskState::Quiescing).unwrap();
        let ticket = quiesce(&mut m, &id).unwrap();
        m.record_candidate(ticket, CandidateRevision::new(revision('c')), "m".into())
            .unwrap();
        m.validation_failed(&id, "x").unwrap();
        m.begin_repair(&id, || Ok(())).unwrap();
        assert!(
            m.active().unwrap().writer_ownership().is_none(),
            "old evidence is discarded"
        );
        m.writer_started(&id, WriterOwnership::Unknown).unwrap();
        assert!(
            !m.active().unwrap().writer_safe(),
            "a new writer is unproven until it is reaped"
        );
    }

    /// Builds a second, independent ticket value for the current gate state so replay of
    /// a copy can be exercised (the public type is deliberately not `Clone`).
    fn duplicate(ticket: &CaptureTicket) -> CaptureTicket {
        CaptureTicket {
            identity: ticket.identity.clone(),
            base: ticket.base.clone(),
            draft: ticket.draft.clone(),
            generation: ticket.generation,
            nonce: ticket.nonce,
        }
    }

    #[test]
    fn a_copied_ticket_cannot_authorize_a_candidate_from_a_repair_writer() {
        let mut m = manager();
        let id = started(&mut m);
        m.transition(&id, TaskState::Editing).unwrap();
        m.transition(&id, TaskState::Quiescing).unwrap();
        let first = quiesce(&mut m, &id).unwrap();
        let replay = duplicate(&first);
        m.record_candidate(first, CandidateRevision::new(revision('c')), "m".into())
            .unwrap();
        m.validation_failed(&id, "x").unwrap();
        m.begin_repair(&id, || Ok(())).unwrap();
        // An unverified repair writer is now modifying the draft, then the task is put
        // back into Quiescing without any fresh quiescence evaluation.
        m.writer_started(&id, WriterOwnership::Unknown).unwrap();
        m.transition(&id, TaskState::Quiescing).unwrap();
        assert_eq!(
            m.record_candidate(replay, CandidateRevision::new(revision('d')), "m".into()),
            Err(TaskError::StaleTicket)
        );
        assert_eq!(m.active().unwrap().state(), TaskState::Quiescing);
        assert!(m.active().unwrap().candidate().is_none());
    }

    #[test]
    fn tickets_are_single_use_and_superseded_by_every_new_gate_event() {
        let mut m = manager();
        let id = started(&mut m);
        m.transition(&id, TaskState::Editing).unwrap();
        m.transition(&id, TaskState::Quiescing).unwrap();
        let candidate = || CandidateRevision::new(revision('c'));

        // A newer ticket supersedes every ticket issued before it.
        let older = quiesce(&mut m, &id).unwrap();
        let newer = quiesce(&mut m, &id).unwrap();
        assert_eq!(
            m.record_candidate(older, candidate(), "m".into()),
            Err(TaskError::StaleTicket)
        );

        // A failed quiescence attempt retires the outstanding ticket.
        let mut blocked = evidence_for(&m, &id);
        blocked.cancel_requested = true;
        assert!(m.complete_quiescence(&id, &blocked, clean_facts).is_err());
        assert_eq!(
            m.record_candidate(newer, candidate(), "m".into()),
            Err(TaskError::StaleTicket)
        );

        // Starting a writer retires the outstanding ticket as well.
        let fresh = quiesce(&mut m, &id).unwrap();
        m.writer_started(&id, qualified()).unwrap();
        assert_eq!(
            m.record_candidate(fresh, candidate(), "m".into()),
            Err(TaskError::StaleTicket)
        );

        // A ticket works exactly once.
        let ticket = quiesce(&mut m, &id).unwrap();
        let again = duplicate(&ticket);
        m.record_candidate(ticket, candidate(), "m".into()).unwrap();
        assert!(m.record_candidate(again, candidate(), "m".into()).is_err());
    }

    #[test]
    fn capture_tickets_are_not_clone() {
        // Fails to compile (ambiguous impl) if `CaptureTicket` ever becomes `Clone`.
        trait AmbiguousIfClone<A> {
            fn check() {}
        }
        impl<T: ?Sized> AmbiguousIfClone<()> for T {}
        impl<T: Clone> AmbiguousIfClone<u8> for T {}
        <CaptureTicket as AmbiguousIfClone<_>>::check();
    }

    #[test]
    fn evidence_for_another_task_generation_or_session_never_reaches_the_gate() {
        let mut m = manager();
        let id = started(&mut m);
        m.transition(&id, TaskState::Editing).unwrap();
        m.transition(&id, TaskState::Quiescing).unwrap();
        let observed = std::cell::Cell::new(false);
        let observe = || {
            observed.set(true);
            clean_facts()
        };

        let mut foreign = evidence_for(&m, &id);
        foreign.identity.task = AgentTaskId::new();
        assert_eq!(
            m.complete_quiescence(&id, &foreign, observe).err(),
            Some(TaskError::QuiescenceBlocked(
                QuiescenceBlock::ForeignEvidence
            ))
        );
        let mut other_generation = evidence_for(&m, &id);
        other_generation.identity.generation += 1;
        assert!(matches!(
            m.complete_quiescence(&id, &other_generation, observe).err(),
            Some(TaskError::QuiescenceBlocked(
                QuiescenceBlock::ForeignEvidence
            ))
        ));

        let mut stale_writer = evidence_for(&m, &id);
        let old = stale_writer.writer;
        m.writer_started(&id, qualified()).unwrap();
        stale_writer.writer = old;
        assert!(matches!(
            m.complete_quiescence(&id, &stale_writer, observe).err(),
            Some(TaskError::QuiescenceBlocked(
                QuiescenceBlock::StaleWriter { .. }
            ))
        ));

        let mut other_session = evidence_for(&m, &id);
        other_session.provider_session = Some("session-from-elsewhere".into());
        assert_eq!(
            m.complete_quiescence(&id, &other_session, observe).err(),
            Some(TaskError::QuiescenceBlocked(
                QuiescenceBlock::ProviderSessionMismatch
            ))
        );
        assert!(
            !observed.get(),
            "the scope is never consulted for bad evidence"
        );
    }

    #[test]
    fn quiescence_requires_a_writer_record_in_the_current_epoch() {
        let mut m = manager();
        let context = context(&mut m);
        let id = context.identity.clone();
        m.begin(context).unwrap();
        m.transition(&id, TaskState::Editing).unwrap();
        m.transition(&id, TaskState::Quiescing).unwrap();
        // No writer was started: nothing the caller supplies can stand in for one.
        let evidence = evidence_with(&id, WriterGeneration(0));
        assert_eq!(
            m.complete_quiescence(&id, &evidence, clean_facts).err(),
            Some(TaskError::QuiescenceBlocked(QuiescenceBlock::NoWriter))
        );
        assert_eq!(m.active().unwrap().state(), TaskState::Quiescing);
    }

    #[test]
    fn ownership_demotion_is_sticky_and_evidence_can_never_upgrade_it() {
        let mut m = manager();
        let id = started(&mut m);
        m.transition(&id, TaskState::Editing).unwrap();
        m.transition(&id, TaskState::Quiescing).unwrap();
        m.observe_writer(
            &id,
            &WriterObservation {
                ownership: WriterOwnership::Detached,
                escaped_pids: vec![],
            },
        )
        .unwrap();
        assert_eq!(
            m.active().unwrap().writer_ownership(),
            Some(&WriterOwnership::Detached)
        );
        // Evidence (and a fresh writer start) that claim qualification change nothing.
        let mut claims_qualified = evidence_for(&m, &id);
        claims_qualified.observed.ownership = qualified();
        assert_eq!(
            m.complete_quiescence(&id, &claims_qualified, clean_facts)
                .err(),
            Some(TaskError::QuiescenceBlocked(
                QuiescenceBlock::UnqualifiedOwnership("detached")
            ))
        );
        m.writer_started(&id, qualified()).unwrap();
        assert_eq!(
            m.active().unwrap().writer_ownership(),
            Some(&WriterOwnership::Detached),
            "a later writer start cannot launder the demotion"
        );
        m.observe_writer(
            &id,
            &WriterObservation {
                ownership: qualified(),
                escaped_pids: vec![],
            },
        )
        .unwrap();
        assert_eq!(
            m.active().unwrap().writer_ownership(),
            Some(&WriterOwnership::Detached)
        );
    }

    #[test]
    fn driver_observed_escapes_and_scope_escapes_demote_a_qualified_writer() {
        let mut m = manager();
        let id = started(&mut m);
        m.transition(&id, TaskState::Editing).unwrap();
        m.observe_writer(
            &id,
            &WriterObservation {
                ownership: qualified(),
                escaped_pids: vec![4242],
            },
        )
        .unwrap();
        assert_eq!(
            m.active().unwrap().writer_ownership(),
            Some(&WriterOwnership::Detached)
        );

        let mut m = manager();
        let id = started(&mut m);
        m.record_scope_facts(
            &id,
            ScopeFacts {
                escaped: vec![7],
                ..clean_facts()
            },
        )
        .unwrap();
        let task = m.active().unwrap();
        assert_eq!(task.writer_ownership(), Some(&WriterOwnership::Detached));
        assert!(
            !task.writer_safe(),
            "a clean count never outvotes an escape"
        );
    }

    #[test]
    fn source_invalidation_interrupts_for_good() {
        let mut m = manager();
        let id = started(&mut m);
        m.transition(&id, TaskState::Editing).unwrap();
        let finished = m.invalidate_source("source became invalid").unwrap();
        assert_eq!(finished.state, TaskState::Interrupted);
        assert!(m.current().unwrap().source_invalidated());
        assert_eq!(
            m.transition(&id, TaskState::Quiescing),
            Err(TaskError::NoActiveTask)
        );
        assert_eq!(m.lease_holder(), None);
        assert!(m.invalidate_source("again").is_none());
    }

    #[test]
    fn brief_is_trimmed_and_bounded() {
        assert_eq!(validate_brief("  hi \n").unwrap(), "hi");
        assert!(validate_brief("   ").is_err());
        assert!(validate_brief(&"x".repeat(MAX_BRIEF_BYTES + 1)).is_err());
    }

    #[test]
    fn candidate_and_base_are_distinct_types() {
        // Compile-time distinctness: neither converts into the other.
        fn takes_base(_: &TaskSourceBase) {}
        fn takes_candidate(_: &CandidateRevision) {}
        takes_base(&TaskSourceBase::new(revision('a')));
        takes_candidate(&CandidateRevision::new(revision('a')));
    }
}
