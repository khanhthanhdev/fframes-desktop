//! Immutable presentation model of the native agent workflow.
//!
//! Everything a UI renders is a value here: rows (messages, tool cards, permission
//! cards, notices, errors, validation/outcome cards), the explicit task phase, the
//! changed-file summary, validation coverage/diagnostics, conflict paths, queue, review
//! policy, Undo and recovery state. Rows are `Arc`-shared so a UI can diff two snapshots
//! by pointer: a tool update replaces the *same* row id in place, never appends.
//!
//! Rows are the persisted unit (`conversation.jsonl`, see [`super::log`]); the resident
//! window is bounded by [`MAX_RESIDENT_ROWS`] / [`MAX_RESIDENT_ROW_BYTES`] and older rows
//! are paged from disk.
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use studio_agent_spike::{
    DiscoveryReport, McpStdioSupport,
    driver::{AgentCapabilityInfo, AgentOptions, PermissionChoice, ToolStatus},
};
use studio_engine::{
    AgentTaskId, DraftState, ReviewPolicy, TaskScope, TaskState,
    candidate_validation::{ChangedPath, FailureKind, ReportDiagnostic, ValidationStage},
    edit_transaction::RetainedVariant,
};
use studio_project::ProjectId;

/// Most rows kept resident; older rows are paged from the conversation log.
pub const MAX_RESIDENT_ROWS: usize = 400;
/// Resident byte budget: the Stage 1 transcript limit (4 MiB).
pub const MAX_RESIDENT_ROW_BYTES: usize = 4 * 1024 * 1024;
/// Allocation cost charged per row on top of its text (as the driver's transcript does).
pub const ROW_OVERHEAD_BYTES: usize = 96;
/// One streaming message row never grows past this (the driver's delta-event bound).
pub const MAX_ROW_TEXT_BYTES: usize = 16 * 1024;
/// Largest page [`super::AgentWorkflow::history_page`] returns.
pub const MAX_HISTORY_PAGE: usize = 100;
/// Bound on every error/diagnostic message carried in a row or card.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024;
/// Diagnostics/warnings/paths shown inline on cards (totals are always carried).
pub const MAX_CARD_ITEMS: usize = 32;
/// Queued follow-up briefs.
pub const MAX_QUEUED_BRIEFS: usize = 8;
/// Accepted revisions listed in the snapshot (newest last).
pub const MAX_HISTORY_ENTRIES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RowId(pub u64);

/// One ordered conversation row. `id` is monotonic per project (it survives restarts);
/// updating a card re-publishes the same id with new content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: RowId,
    pub task: Option<AgentTaskId>,
    pub at_unix: u64,
    pub kind: RowKind,
}

impl Row {
    /// Bytes charged against [`MAX_RESIDENT_ROW_BYTES`].
    pub fn estimated_bytes(&self) -> usize {
        ROW_OVERHEAD_BYTES + self.kind.payload_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserSource {
    /// The brief that started a task.
    Brief,
    /// A clarification answer sent into the same provider session.
    Clarification,
    /// The automatic repair context the app sent (structured, bounded).
    Repair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "row", rename_all = "snake_case")]
pub enum RowKind {
    User {
        text: String,
        source: UserSource,
    },
    Agent {
        text: String,
        turn: u64,
        streaming: bool,
    },
    Thought {
        text: String,
        turn: u64,
        streaming: bool,
    },
    Tool(ToolCard),
    Permission(PermissionCard),
    Notice {
        level: NoticeLevel,
        text: String,
    },
    Error(StructuredError),
    Validation(Box<ValidationCard>),
    Changes(ChangeCard),
    Outcome(OutcomeCard),
}

impl RowKind {
    fn payload_bytes(&self) -> usize {
        match self {
            Self::User { text, .. } | Self::Notice { text, .. } => text.len(),
            Self::Agent { text, .. } | Self::Thought { text, .. } => text.len(),
            Self::Tool(card) => {
                card.call_id.len()
                    + card.title.as_ref().map_or(0, String::len)
                    + card.kind.as_ref().map_or(0, String::len)
            }
            Self::Permission(card) => {
                card.title.len()
                    + card.tool_call_id.len()
                    + card
                        .options
                        .iter()
                        .map(|o| o.option_id.len() + o.name.len() + o.kind.len())
                        .sum::<usize>()
            }
            Self::Error(error) => error.title.len() + error.detail.len(),
            Self::Validation(card) => {
                card.summary.len()
                    + card
                        .errors
                        .iter()
                        .map(|d| d.message.len() + d.key.len())
                        .sum::<usize>()
                    + card.warnings.iter().map(String::len).sum::<usize>()
            }
            Self::Changes(card) => card.entries.iter().map(|e| e.path.len() + 16).sum(),
            Self::Outcome(card) => card.summary.len(),
        }
    }

    pub fn is_message(&self) -> bool {
        matches!(self, Self::Agent { .. } | Self::Thought { .. })
    }
}

/// A tool call of the agent. Updates for the same `call_id` replace this card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCard {
    pub call_id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<ToolStatus>,
    /// Updates folded into the card so far.
    pub updates: u32,
}

/// Correlates a permission reply with exactly one request of one writer epoch of one
/// task. A reply carrying anything else is stale.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PermissionRef {
    pub task: AgentTaskId,
    /// The engine's writer generation the request was raised in.
    pub writer: u64,
    pub request: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Open,
    Selected {
        option_id: String,
    },
    /// Closed as cancelled: Stop, turn end, provider loss or a reply the user withdrew.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionCard {
    pub reference: PermissionRef,
    pub turn: u64,
    pub tool_call_id: String,
    pub title: String,
    pub options: Vec<PermissionChoice>,
    pub state: PermissionState,
}

/// A failure with a user-facing next step. Every string is already redacted and bounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuredError {
    /// Stable machine code, e.g. `provider_failed`, `quiescence_blocked`, `source_conflict`.
    pub code: String,
    pub title: String,
    pub detail: String,
    /// What the user can do next.
    pub action: Option<String>,
    /// Where it happened (`Initialize`, `Prompt`, `Capture`, ...), when known.
    pub phase: Option<String>,
    /// The draft/candidate was retained.
    pub retained: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageView {
    pub total_frames: usize,
    /// The scope requested at submit (its label) and its half-open interval, kept apart
    /// from what validation actually rendered and inspected below.
    pub requested_scope: String,
    pub requested_interval: Option<[usize; 2]>,
    pub requested_frames: Vec<usize>,
    pub rendered_frame_indexes: Vec<usize>,
    pub rendered_frames: usize,
    pub inspected_frames: usize,
    pub boundary_frames: usize,
    pub playhead: usize,
    pub broadened: Option<String>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioView {
    pub silent: bool,
    pub peak: f32,
    pub sample_rate: u32,
    pub sample_count: u64,
    pub placement_verified: bool,
    pub checks_passed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureView {
    pub stage: ValidationStage,
    pub kind: FailureKind,
    pub summary: String,
}

/// Validation result: coverage, diagnostics and the build that proved it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationCard {
    pub passed: bool,
    pub summary: String,
    pub repair_count: u32,
    pub coverage: Option<CoverageView>,
    pub errors: Vec<ReportDiagnostic>,
    pub errors_total: usize,
    pub diagnostics_total: usize,
    pub warnings: Vec<String>,
    pub capability_gaps: Vec<String>,
    pub audio: Option<AudioView>,
    pub failure: Option<FailureView>,
    pub frames_rendered: usize,
    /// Short digest of the compile that validated the candidate.
    pub build_key: Option<String>,
    pub candidate: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceState {
    /// Being rendered on a background worker.
    Pending,
    Ready,
    /// Could not be produced; `EvidenceView::note` says why. Never a placeholder image.
    Unavailable,
    /// The owning task ended; its app-owned artifacts were removed.
    Released,
}

/// Which evidence render a background-worker failure belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvidenceKind {
    /// The frozen task base: part of the first prompt's context.
    Before,
    /// The validated candidate, rendered at the same frame indexes while it awaits review.
    After,
}

/// One app-owned PNG artifact (no path: the file lives in the app's artifact store and
/// dies with its task).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceArtifact {
    /// `selected`, `before-boundary` or `after-boundary`.
    pub label: String,
    /// The compiled frames rendered into the artifact (a strip lists several).
    pub frames: Vec<usize>,
    pub id: String,
    pub bytes: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

/// Actual frames rendered from one immutable revision (the frozen task base for the
/// before evidence, the validated candidate for the after evidence). Never predicted
/// pixels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceView {
    pub state: EvidenceState,
    /// Short source revision the artifacts were rendered from.
    pub revision: String,
    pub artifacts: Vec<EvidenceArtifact>,
    pub note: Option<String>,
}

/// Changed-file summary of a candidate against its task base.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeCard {
    pub entries: Vec<ChangedPath>,
    pub total: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    Accepted,
    Undone,
    AwaitingReview,
    Conflict,
    Failed,
    Cancelled,
    Interrupted,
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutcomeCard {
    pub kind: OutcomeKind,
    pub summary: String,
    pub published: Option<String>,
    pub draft_retained: bool,
}

// ---- task / workflow snapshot -------------------------------------------------------------

/// The explicit state of the task as the user experiences it. The engine's coarser
/// [`TaskState`] is carried next to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPhase {
    /// Accepted by the workflow, waiting for the earlier task to release the writer.
    Queued,
    /// Launching the adapter and creating its session.
    Starting,
    Editing,
    WaitingPermission,
    /// The agent asked a question and ended its turn without changing the draft.
    WaitingClarification,
    /// Reaping the adapter and proving quiescence.
    Quiescing,
    Capturing,
    Validating,
    /// The one automatic repair turn is running.
    Repairing,
    /// Manual review: the validated candidate is retained for Apply/discard/export.
    AwaitingReview,
    /// Automatic Apply waits for truthful candidate evidence to settle.
    AwaitingEvidence,
    /// The short uncancellable publication boundary.
    Promoting,
    Accepted,
    Conflict,
    Failed,
    Cancelled,
    Interrupted,
}

impl TaskPhase {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Accepted | Self::Conflict | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    /// Stop does something in this phase (it is a no-op notice in the others).
    pub fn stoppable(self) -> bool {
        !self.is_terminal() && self != Self::AwaitingReview
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Starting => "Starting the agent",
            Self::Editing => "Editing",
            Self::WaitingPermission => "Waiting for your permission",
            Self::WaitingClarification => "Waiting for your answer",
            Self::Quiescing => "Finishing the agent",
            Self::Capturing => "Capturing the result",
            Self::Validating => "Validating",
            Self::Repairing => "Repairing",
            Self::AwaitingReview => "Waiting for your review",
            Self::AwaitingEvidence => "Capturing candidate evidence",
            Self::Promoting => "Applying",
            Self::Accepted => "Accepted",
            Self::Conflict => "Conflict",
            Self::Failed => "Failed",
            Self::Cancelled => "Stopped",
            Self::Interrupted => "Interrupted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairView {
    pub used: u32,
    pub max: u32,
    /// The repair turn is running (phase `Repairing`).
    pub in_progress: bool,
    /// The bounded structured context the repair turn received.
    pub context_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriterView {
    pub generation: u64,
    /// `process-group-contained` / `detached` / `unknown`.
    pub ownership: String,
    pub provider_session: Option<String>,
}

/// Why paths conflicted and which variants were preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictView {
    pub reason: String,
    pub transaction: Option<String>,
    /// Paths that changed in the project since the task started.
    pub external_paths: Vec<String>,
    /// Paths the candidate changes.
    pub candidate_paths: Vec<String>,
    /// Paths changed on both sides.
    pub overlapping: Vec<String>,
    pub retained_variants: Vec<RetainedVariant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewView {
    pub candidate: String,
    /// Apply is currently refused (blocked publication gate, unresolved mutation): the
    /// candidate stays retained and Apply can be retried.
    pub apply_blocked: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskView {
    pub id: AgentTaskId,
    pub generation: u64,
    pub phase: TaskPhase,
    pub engine_state: TaskState,
    pub brief: String,
    /// Short source-base revision the task was frozen against.
    pub source_base: String,
    /// Requested whole-project/scene/range scope frozen at submit.
    pub scope: TaskScope,
    /// Actual frames of the frozen source base around the scope; `None` for whole-project
    /// tasks submitted without a displayed compiled timeline.
    pub before: Option<EvidenceView>,
    /// Actual frames of the validated candidate at the same frame indexes.
    pub after: Option<EvidenceView>,
    /// Visible when the evidence reaches the agent only as references: this build never
    /// sends image blocks, whatever the adapter advertises.
    pub image_limitation: Option<String>,
    pub draft: PathBuf,
    /// Review policy frozen when this task started; later changes affect later tasks only.
    pub review_policy: ReviewPolicy,
    pub repair: RepairView,
    pub turns: u32,
    pub writer: Option<WriterView>,
    pub changes: Option<ChangeCard>,
    pub validation: Option<ValidationCard>,
    pub review: Option<ReviewView>,
    pub conflict: Option<ConflictView>,
    pub error: Option<StructuredError>,
    pub stop_requested: bool,
    pub reason: Option<String>,
    pub started_unix: u64,
    pub ended_unix: Option<u64>,
    /// Ownership of the stable draft once the task ended.
    pub draft_state: Option<DraftState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedBrief {
    pub id: u64,
    pub summary: String,
    pub queued_unix: u64,
    pub scope: Option<String>,
    pub stale_scope: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AdapterReadiness {
    NotConfigured,
    /// Configured but never probed: project open never launches an agent.
    Unchecked,
    Checking,
    Checked {
        report: DiscoveryReport,
        at_unix: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdapterView {
    pub provider: Option<String>,
    pub executable: Option<String>,
    pub readiness: AdapterReadiness,
    /// Configured writer-ownership label (`process-group-contained`, `unknown`, ...).
    pub writer_ownership: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpView {
    /// Host policy: stdio MCP servers are only sent when enabled.
    pub policy_enabled: bool,
    /// The `studio-mcp` helper binary was found.
    pub binary_available: bool,
    /// The current task's session was offered the project tools over MCP.
    pub active: bool,
    /// The current task was told about the `studio-tools` command-line route.
    pub cli_active: bool,
    /// The current task's capability file (a path only; the secret stays inside it). It
    /// exists whenever any tool route is offered and dies with the task.
    pub capability_file: Option<std::path::PathBuf>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UndoView {
    Available { target: String, summary: String },
    Unavailable { reason: String },
    InProgress { phase: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftNotice {
    pub state: DraftState,
    /// The previous session ended while this task ran (found on open).
    pub interrupted_by_restart: bool,
    /// The user may confirm that nothing is writing and unlock the draft.
    pub can_acknowledge: bool,
    pub draft: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RecoveryView {
    /// Transactions the open-time recovery rolled back.
    pub rolled_back: Vec<String>,
    pub unresolved_conflicts: Vec<ConflictView>,
    /// Source mutation is suspended until the project is reopened.
    pub suspended: Option<String>,
    pub draft: Option<DraftNotice>,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffKind {
    Apply,
    Undo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HandoffState {
    /// Committed; the preview is not (yet) showing it.
    AwaitingPreview { reason: Option<String> },
    /// The staged preview was adopted by the coordinator.
    Adopted,
    /// The preview displays the published revision.
    Displayed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffView {
    pub kind: HandoffKind,
    pub published: String,
    pub state: HandoffState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub kind: String,
    pub summary: String,
    pub files: usize,
    pub published: String,
    pub undoes: Option<String>,
    pub committed_unix: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ResourceView {
    pub resident_rows: usize,
    pub resident_bytes: usize,
    pub max_resident_rows: usize,
    pub max_resident_bytes: usize,
    pub queue_len: usize,
    pub open_permissions: usize,
    /// Live processes under the project's process scope (adapter, workers, compiles).
    pub owned_processes: usize,
    pub broker_grants: usize,
    pub tool_workers: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffChoice {
    ContinueDraft,
    RestartFromAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchPendingView {
    pub target_provider: String,
    pub draft_revision: Option<String>,
    pub source_revision: String,
    pub accepted_revision: String,
    pub outgoing_task_brief: Option<String>,
    pub retained_queue_count: usize,
    pub handoff_context: super::session_store::BoundedHandoffContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRestoreView {
    pub provider_id: String,
    pub redacted_session_id: String,
    pub is_resumable: bool,
    pub last_source_revision: String,
    pub notice: Option<String>,
}

/// A complete immutable view; a new one is published after every batch of changes.
#[derive(Debug, Clone)]
pub struct WorkflowSnapshot {
    /// Strictly increasing; use with `AgentWorkflow::wait_changed`.
    pub revision: u64,
    pub project: ProjectId,
    pub adapter: AdapterView,
    /// Capabilities the last initialized session advertised (limits to show).
    pub capabilities: Option<AgentCapabilityInfo>,
    /// Modes/config the adapter advertised for the live session; nothing else is settable.
    pub options: AgentOptions,
    pub mcp: McpView,
    pub task: Option<TaskView>,
    pub queue: Vec<QueuedBrief>,
    /// Briefs from an outgoing provider retained across a switch; requires explicit transfer or clear.
    pub retained_queue: Vec<QueuedBrief>,
    /// Newest `MAX_RESIDENT_ROWS` rows (bounded by bytes too), ordered by id.
    pub rows: Vec<Arc<Row>>,
    /// Older rows exist in the conversation log (`history_page`).
    pub older_rows: bool,
    /// The policy the next task will freeze.
    pub review_policy: ReviewPolicy,
    pub undo: UndoView,
    pub recovery: RecoveryView,
    pub handoff: Option<HandoffView>,
    pub history: Vec<HistoryEntry>,
    pub resources: ResourceView,
    pub closed: bool,
    pub switch_pending: Option<SwitchPendingView>,
    pub session_restore: Option<SessionRestoreView>,
}

impl WorkflowSnapshot {
    pub const REVIEW_POLICY_NOTICE: &'static str = ReviewPolicy::SCOPE_NOTICE;

    pub fn row(&self, id: RowId) -> Option<&Arc<Row>> {
        self.rows.iter().find(|r| r.id == id)
    }

    /// Open permission cards, oldest first.
    pub fn open_permissions(&self) -> Vec<&PermissionCard> {
        self.rows
            .iter()
            .filter_map(|row| match &row.kind {
                RowKind::Permission(card) if card.state == PermissionState::Open => Some(card),
                _ => None,
            })
            .collect()
    }

    pub fn active_phase(&self) -> Option<TaskPhase> {
        self.task.as_ref().map(|t| t.phase)
    }
}

/// Host policy for stdio MCP, as the model reports it.
pub fn mcp_policy_enabled(policy: McpStdioSupport) -> bool {
    policy == McpStdioSupport::Baseline
}
