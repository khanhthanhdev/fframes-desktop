//! The workflow actor: one owned thread that serializes every command, pumps the live
//! ACP driver, owns the task state machine and publishes immutable snapshots.
//!
//! The actor never does long blocking work itself. The quiesce -> capture -> validate
//! pipeline, publication, Undo and adapter probing run on their own threads
//! (`jobs.rs`) and report back with one message; Stop reaches them out of band through
//! the task's [`Gate`] and process scopes, so it stays actionable in every state.

mod apply;
mod composer;
mod finish;
mod prompt;
mod rows;
mod session;
mod turn;

use super::{
    AdapterSettings, PermissionAnswer, PreparedAttachment, Shared, ToolSettings, WorkflowError,
    jobs::{
        self, Env, Gate, HandoffOutcome, JobKind, PipelineEnd, PipelineInput, ProbeInput,
        PromoteEnd, Retained, UndoEnd, observe_writer, reap_and_observe, run_pipeline, run_probe,
        run_promote, run_undo,
    },
    jobs::{RejectEnd, RejectInput, run_reject, run_revert_path},
    log::RowStore,
    model::*,
    present::{self, CliRoute, ToolRoute},
    probe_cache::{AdapterProbeCache, ProbeKey},
    session_store::{self, ManifestStatus, SessionManifest, SessionManifestState, SessionStore},
    tools::{BuildSettings, ToolPlacement, ToolRuntime, render_scope_evidence},
};
use crate::agent_tools::backend::ProjectToolBackend;
use crate::services::candidate_runner::RunScopes;
use crate::{
    AgentTaskContext, AgentTaskId, DraftState, EngineError, PromotionError, ReviewPolicy,
    TaskScope, TaskState, WriterGeneration, WriterGoneEvidence,
    candidate_validation::{CapturedCandidate, ChangeSet, FailureContext},
    edit_transaction::ConflictReport,
};
use fframes_studio_protocol::PreviewIdentity;
use parking_lot::Mutex;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender},
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use studio_bootstrap::ProcessTreeManager;
use studio_harness::{
    AdapterLaunch, AgentDriver, AgentFailure, DiscoveryReport, DriverConfig, DriverMode,
    McpStdioServer, McpStdioSupport,
    driver::{
        AgentEvent, AgentEventKind, MessageRole, OptionValue, PermissionChoice, PermissionId,
        PermissionPrompt, PermissionReply, PermissionResolution, PromptImage, PromptOutcome,
        QuestionAnswer, QuestionPrompt, ToolEvent,
    },
};
use studio_project::revision::SourceInventory;

const PUBLISH_INTERVAL: Duration = Duration::from_millis(25);
/// How often the actor compares the live count of owned processes with the published one.
const OWNED_RECHECK_INTERVAL: Duration = Duration::from_millis(250);
const MAX_TOOL_INDEX: usize = 2048;
const MAX_SCOPE_PROMPT_IMAGES: usize = 3;
/// Controller commands that may wait behind a running job.
const MAX_DEFERRED: usize = 32;

/// Receives one history page (or why it could not be read).
pub(crate) type HistoryDone = Box<dyn FnOnce(Result<Vec<Arc<Row>>, WorkflowError>) + Send>;

pub(crate) enum Msg {
    CheckAdapter,
    SettingsChanged,
    Submit {
        id: u64,
        brief: String,
        scope: Option<Box<TaskScope>>,
        attachments: Vec<PreparedAttachment>,
    },
    /// Deliver text to the running turn (the agent negotiated it).
    Steer(String),
    /// Cancel the running turn and prompt again with this message.
    SendNow {
        text: String,
        attachments: Vec<PreparedAttachment>,
    },
    /// Send this message at the next step boundary of the running turn.
    SendAtBoundary {
        text: String,
        attachments: Vec<PreparedAttachment>,
    },
    CancelHeld,
    AnswerQuestions {
        reference: PermissionRef,
        answers: Vec<QuestionAnswer>,
    },
    SetDisplayedPreviewIdentity(Option<PreviewIdentity>),
    CancelQueued(u64),
    Reply {
        reference: PermissionRef,
        answer: PermissionAnswer,
    },
    Clarify {
        task: AgentTaskId,
        text: String,
    },
    SetMode(String),
    SetConfig(String, OptionValue),
    Stop,
    Apply,
    Discard,
    ExportCandidate(PathBuf),
    ExportDraft(PathBuf),
    Undo(Option<String>),
    /// Leave these files out of the candidate under review.
    RejectFiles(Vec<String>),
    /// Revert one file of an accepted revision.
    RevertPath {
        target: String,
        path: String,
    },
    SetPolicy(ReviewPolicy),
    SetPermissionPolicy(studio_proto::PermissionPolicy),
    /// The project's saved rules changed: the running task uses them from its next request.
    SetApprovals(Vec<super::ApprovalRule>),
    SetAgentTools(crate::agent_settings::AgentToolSettings),
    PreviewDisplayed(String),
    HandoffResolved {
        published: String,
        result: Result<(), String>,
    },
    AcknowledgeWriterGone,
    ResolveConflict {
        transaction: String,
        note: String,
    },
    Refresh,
    History {
        /// The conversation the page was asked for (`WorkflowSnapshot::chat`).
        chat: u64,
        before: RowId,
        limit: usize,
        done: HistoryDone,
    },
    InitiateSwitch {
        target_provider: String,
    },
    ChooseHandoffContinuation(HandoffChoice),
    CancelSwitch,
    TransferRetainedQueue,
    ClearRetainedQueue,
    RestoreSession,
    DismissRestoration,
    NewChat,
    /// Switch to a stored session (an id the workflow already checked).
    OpenSession(String),
    TestHoldToolsGate(Sender<Option<crate::agent_tools::backend::WriterGuard>>),
    Close(Sender<()>),
    // ---- job results ----
    Progress(TaskPhase),
    UndoProgress(String),
    /// An adapter check finished; `generation` is the `probe_generation` it ran for.
    Probe {
        generation: u64,
        report: DiscoveryReport,
    },
    Pipeline(PipelineEnd),
    Promote(PromoteEnd),
    Undone(UndoEnd),
    Rejected(RejectEnd),
    /// Publication committed: sent BEFORE the preview sink runs, so an acknowledgement
    /// the sink triggers can never overtake the revision it is about.
    Committed {
        published: String,
        kind: HandoffKind,
    },
    /// Actual frames around the frozen scope, rendered from one immutable revision.
    Evidence {
        task: AgentTaskId,
        kind: EvidenceKind,
        view: EvidenceView,
    },
    /// A job panicked (or was never admitted): its operation must settle.
    JobAborted {
        kind: JobKind,
        message: String,
    },
}

/// Bounds the background evidence render (build + a few frames) so a hung build can
/// never hold the first prompt back indefinitely.
const EVIDENCE_TIMEOUT: Duration = Duration::from_secs(150);

/// What the agent was given to reach the project tools in one session.
#[derive(Default)]
struct ToolOffer {
    mcp: Option<McpStdioServer>,
    cli: Option<CliRoute>,
    capability: Option<PathBuf>,
    /// The project tools the capability allows.
    tools: Vec<&'static str>,
}

impl ToolOffer {
    fn route(&self) -> ToolRoute {
        ToolRoute {
            mcp: self.mcp.is_some(),
            cli: self.cli.clone(),
            tools: self.tools.iter().map(|tool| (*tool).to_owned()).collect(),
        }
    }
}

enum PromptKind {
    Brief,
    Repair,
}

struct Active {
    ctx: AgentTaskContext,
    phase: TaskPhase,
    /// Frozen when the task started.
    policy: ReviewPolicy,
    /// Frozen when the task started.
    permission_policy: studio_proto::PermissionPolicy,
    /// The project's saved rules, frozen when the task started; replaced only by the
    /// user's own changes to them ([`Msg::SetApprovals`]).
    approvals: Vec<super::ApprovalRule>,
    /// Frozen when the task started: the project tools offered and the user's servers.
    agent_tools: crate::agent_settings::AgentToolSettings,
    /// The user's own MCP servers every session of this task launches.
    user_mcp: Vec<studio_harness::UserMcpLaunch>,
    adapter: AdapterSettings,
    launch: AdapterLaunch,
    /// Cancels the job in flight; replaced for every job so a Stop that raced the end of
    /// one job can never cancel the next.
    gate: Arc<Gate>,
    driver: Option<Box<dyn AgentDriver>>,
    writer: Option<WriterGeneration>,
    session: Option<String>,
    mcp: Option<McpStdioServer>,
    cli: Option<CliRoute>,
    /// The task's capability file while any tool route is offered.
    capability: Option<PathBuf>,
    /// The scopes validation's compile/worker processes live in.
    pipeline_scopes: Option<(ProcessTreeManager, ProcessTreeManager)>,
    retained: Option<Box<Retained>>,
    /// A reject job is validating the files left after a rejection: Stop cancels only it.
    rejecting: bool,
    in_repair: bool,
    pending_prompt: Option<(String, PromptKind)>,
    /// What the user attached to the brief; it travels with the first prompt.
    first_attachments: Vec<PreparedAttachment>,
    /// The conversation row of the brief, which names what was attached once it is sent.
    brief_row: Option<RowId>,
    /// The before evidence is still rendering: the first prompt waits for it.
    before_pending: bool,
    /// The session is ready and the first prompt is waiting for `before_pending`.
    prompt_held: bool,
    /// Stops this task's evidence jobs (task end, close).
    evidence_cancel: Arc<AtomicBool>,
    turn: Option<u64>,
    open_permissions: HashSet<u64>,
    /// Open requests (a subset of `open_permissions`) that are questions, with their
    /// question count: the user's choice is sent back as an answer, not a permission.
    open_questions: HashMap<u64, usize>,
    engine_waiting: bool,
    /// Background tasks the agent reported still running, and whether it is about to
    /// continue (or is continuing) in a turn of its own. Capture waits for both to clear.
    background: (u32, bool),
    /// A turn ended cleanly while background work was live: capture waits for it.
    capture_deferred: bool,
    failed: bool,
    stop_requested: bool,
    /// Publication is running and cannot be cancelled.
    promoting: bool,
    /// AutoApply is waiting for the candidate's evidence render to settle.
    promote_after_evidence: bool,
}

struct Queued {
    id: u64,
    brief: String,
    queued_unix: u64,
    scope: Option<TaskScope>,
    attachments: Vec<PreparedAttachment>,
}

struct UndoRun {
    gate: Arc<Gate>,
    scopes: (ProcessTreeManager, ProcessTreeManager),
}

struct Stream {
    role: MessageRole,
    turn: u64,
    row: RowId,
}

pub(crate) struct Actor {
    shared: Arc<Shared>,
    rx: Receiver<Msg>,
    store: RowStore,
    // ---- view state ----
    adapter: AdapterView,
    capabilities: Option<studio_harness::driver::AgentCapabilityInfo>,
    options: studio_harness::driver::AgentOptions,
    /// Notes about the task's tools made at task start.
    tool_notes: ToolNotes,
    /// Notes made once the session said which of the user's servers it got.
    session_notes: Vec<ToolNote>,
    task: Option<TaskView>,
    policy: ReviewPolicy,
    /// The user's permission policy, for the next task.
    permission_policy: studio_proto::PermissionPolicy,
    /// The user's MCP servers and turned-off project tools, for the next task.
    agent_tools: crate::agent_settings::AgentToolSettings,
    undo_view: UndoView,
    recovery: RecoveryView,
    handoff: Option<HandoffView>,
    handoff_published: Option<String>,
    history: Vec<HistoryEntry>,
    interrupted_task: Option<AgentTaskId>,
    // ---- machinery ----
    active: Option<Active>,
    undo: Option<UndoRun>,
    queue: VecDeque<Queued>,
    displayed_preview_identity: Option<PreviewIdentity>,
    tools: Option<ToolRuntime>,
    jobs: Vec<JoinHandle<()>>,
    stream: Option<Stream>,
    tool_rows: HashMap<String, RowId>,
    permission_rows: HashMap<(u64, u64), RowId>,
    /// Commands that needed the controller while a job held it; retried every tick.
    deferred: VecDeque<Msg>,
    /// Exact credential values of the configured adapter: scrubbed from every string the
    /// workflow itself persists or presents (the driver already scrubs its own).
    secrets: Vec<String>,
    /// Insertion order of `tool_rows`, so the oldest entry is the one dropped when full.
    tool_order: VecDeque<String>,
    /// Unsaved rows were dropped to keep the memory bound: the conversation can no
    /// longer be retained and the running task must stop visibly.
    log_lost: bool,
    /// Changes whenever a driver is installed or retired; a pump that outlives its
    /// driver sees the change and stops.
    driver_epoch: u64,
    /// An acknowledgement of the committed preview handoff already arrived.
    handoff_acked: bool,
    probing: bool,
    /// Changes whenever the adapter settings change; a check that ran for an older
    /// generation is dropped and the check runs again for the current settings.
    probe_generation: u64,
    dirty: bool,
    last_publish: Instant,
    /// The owned-process count of the last snapshot, and when the live count was last
    /// compared with it.
    published_owned: usize,
    owned_checked: Instant,
    revision: u64,
    closing: bool,
    log_failed: bool,
    switch_target: Option<String>,
    switch_pending: Option<SwitchPendingView>,
    session_restore: Option<SessionRestoreView>,
    /// The offer made when the project opened; while `session_restore` still equals it, a
    /// settings change makes the offer again for the new provider.
    startup_offer: Option<SessionRestoreView>,
    next_draft_choice: Option<crate::DraftPreparationChoice>,
    next_handoff_context: Option<super::session_store::BoundedHandoffContext>,
    pending_resume_session: Option<PendingResumeSession>,
    retained_queue: VecDeque<Queued>,
    /// New chat resets so far (`WorkflowSnapshot::chat`).
    chat: u64,
    /// The driver being started resumes a stored agent session.
    resume_attempt: bool,
    /// Resuming failed: the stored manifest stays as it was.
    resume_failed: bool,
    /// Composer state: commands, plan, open questions, follow-up delivery.
    composer: composer::ComposerRun,
}
#[derive(Debug, Clone)]
struct PendingResumeSession {
    provider_id: String,
    launch_digest: String,
    wire_session_id: String,
    draft_path: PathBuf,
    last_source_revision: String,
    last_accepted_revision: String,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn handoff_draft_choice(
    pending: &SwitchPendingView,
    content_revision: &str,
) -> Result<crate::DraftPreparationChoice, String> {
    let parse_revision = |value: &str, label: &str| {
        studio_project::SourceRevision::try_from(value.to_owned())
            .map_err(|_| format!("the {label} revision is invalid"))
    };
    Ok(crate::DraftPreparationChoice::FromHandoff {
        content_revision: parse_revision(content_revision, "selected content")?,
        expected_source_revision: parse_revision(&pending.source_revision, "source")?,
        expected_accepted_revision: parse_revision(&pending.accepted_revision, "accepted")?,
    })
}

/// The identity of the program this task launches and the wire it speaks, binding a
/// resumable session to both. An ACP digest is unchanged from before wires existed, so
/// saved ACP sessions still resume.
fn session_launch_digest(active: &Active) -> Result<String, String> {
    let digest = crate::containment::launch_identity(&active.launch)?.digest();
    Ok(match active.adapter.wire {
        studio_harness::AgentWire::Acp => digest,
        wire => format!("{}:{digest}", wire.as_str()),
    })
}

fn split_at_boundary(text: &str, max: usize) -> (&str, &str) {
    if text.len() <= max {
        return (text, "");
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.split_at(end)
}

/// Reads only the bounded, task-owned PNGs selected for the review panel. This runs on
/// the evidence job thread before publishing metadata so the artifacts cannot be
/// released before their bytes are copied.
fn read_evidence_preview_bytes(
    shared: &super::Shared,
    backend: &ProjectToolBackend,
    task: &AgentTaskId,
    artifacts: &[EvidenceArtifact],
) -> Vec<(String, Vec<u8>, u32, u32)> {
    let mut previews = Vec::new();
    for artifact in artifacts {
        let Some(max_bytes) =
            shared
                .evidence_images
                .lock()
                .reserve(task, &artifact.id, artifact.bytes)
        else {
            continue;
        };
        let Ok(bytes) = backend
            .artifacts()
            .read_png_for_task(task, &artifact.id, max_bytes)
        else {
            continue;
        };
        previews.push((artifact.id.clone(), bytes, artifact.width, artifact.height));
    }
    previews
}

fn decode_evidence_previews(
    shared: &super::Shared,
    task: &AgentTaskId,
    previews: Vec<(String, Vec<u8>, u32, u32)>,
) {
    let mut changed = false;
    for (id, bytes, width, height) in previews {
        let Ok((image, _decoded_bytes)) =
            crate::agent::evidence_preview::decode_png(&bytes, (width, height))
        else {
            continue;
        };
        changed |= shared.evidence_images.lock().complete(task, &id, image);
    }
    if changed && let Some(notify) = &shared.notify {
        notify();
    }
}

fn engine_state_of(phase: TaskPhase) -> TaskState {
    match phase {
        TaskPhase::Queued | TaskPhase::Starting => TaskState::ContextReady,
        TaskPhase::Editing | TaskPhase::Repairing | TaskPhase::WaitingBackground => {
            TaskState::Editing
        }
        TaskPhase::WaitingPermission | TaskPhase::WaitingClarification => TaskState::Waiting,
        TaskPhase::Quiescing | TaskPhase::Capturing => TaskState::Quiescing,
        TaskPhase::Validating => TaskState::Validating,
        TaskPhase::AwaitingReview => TaskState::CandidateReady,
        TaskPhase::AwaitingEvidence => TaskState::CandidateReady,
        TaskPhase::Promoting => TaskState::Promoting,
        TaskPhase::Accepted => TaskState::Accepted,
        TaskPhase::Conflict => TaskState::Conflict,
        TaskPhase::Failed => TaskState::Failed,
        TaskPhase::Cancelled => TaskState::Cancelled,
        TaskPhase::Interrupted => TaskState::Interrupted,
    }
}

impl Actor {
    pub(crate) fn new(shared: Arc<Shared>, rx: Receiver<Msg>, store: RowStore) -> Self {
        let mut actor = Self {
            shared,
            rx,
            store,
            adapter: AdapterView {
                provider: None,
                executable: None,
                readiness: AdapterReadiness::NotConfigured,
                last_launch: None,
            },
            capabilities: None,
            options: Default::default(),
            tool_notes: ToolNotes::default(),
            session_notes: Vec::new(),
            task: None,
            policy: ReviewPolicy::default(),
            permission_policy: studio_proto::PermissionPolicy::default(),
            agent_tools: Default::default(),
            undo_view: UndoView::Unavailable {
                reason: "No agent edit has been accepted in this project yet".into(),
            },
            recovery: RecoveryView::default(),
            handoff: None,
            handoff_published: None,
            history: Vec::new(),
            interrupted_task: None,
            active: None,
            undo: None,
            queue: VecDeque::new(),
            displayed_preview_identity: None,
            tools: None,
            jobs: Vec::new(),
            stream: None,
            tool_rows: HashMap::new(),
            permission_rows: HashMap::new(),
            deferred: VecDeque::new(),
            secrets: Vec::new(),
            tool_order: VecDeque::new(),
            log_lost: false,
            driver_epoch: 0,
            handoff_acked: false,
            probing: false,
            probe_generation: 0,
            dirty: true,
            last_publish: Instant::now(),
            published_owned: 0,
            owned_checked: Instant::now(),
            revision: 0,
            closing: false,
            log_failed: false,
            switch_target: None,
            switch_pending: None,
            session_restore: None,
            startup_offer: None,
            next_draft_choice: None,
            next_handoff_context: None,
            pending_resume_session: None,
            retained_queue: VecDeque::new(),
            chat: 0,
            resume_attempt: false,
            resume_failed: false,
            composer: Default::default(),
        };
        // The offer is judged against the configured provider, so the settings come first.
        actor.settings_changed();
        actor.offer_saved_session();
        actor.detect_interrupted();
        actor.refresh_idle();
        actor.publish(true);
        actor
    }

    pub(crate) fn run(mut self) {
        loop {
            match self.rx.recv_timeout(self.shared.tick) {
                Ok(message) => {
                    if self.handle(message) {
                        return;
                    }
                    let mut budget = 64;
                    while budget > 0
                        && let Ok(message) = self.rx.try_recv()
                    {
                        if self.handle(message) {
                            return;
                        }
                        budget -= 1;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            self.pump();
            self.retry_deferred();
            if self.switch_target.is_some()
                && self.active.is_none()
                && self.undo.is_none()
                && self.switch_pending.is_none()
            {
                self.finish_switch_teardown();
            }
            self.settle_log_loss();
            self.reap_jobs();
            self.recheck_owned_processes();
            if self.dirty && self.last_publish.elapsed() >= PUBLISH_INTERVAL {
                self.publish(false);
            }
        }
        self.shutdown_all(None);
    }

    /// Processes also start and end without an event of the workflow's own: a teardown
    /// finishes on its own thread, a preview build spawns under the same root. A moved
    /// count publishes a new snapshot so the published count does not go stale.
    fn recheck_owned_processes(&mut self) {
        if self.owned_checked.elapsed() < OWNED_RECHECK_INTERVAL {
            return;
        }
        self.owned_checked = Instant::now();
        if self.shared.root_scope.active_count() != self.published_owned {
            self.dirty = true;
        }
    }

    // ---- message dispatch ----------------------------------------------------------------------

    /// Commands that take the controller lock on the actor thread. While a job may hold
    /// the lock (capture, publication, Undo) they wait in `deferred` instead of blocking
    /// the actor, which must stay able to dispatch Stop and Close.
    fn needs_controller(message: &Msg) -> bool {
        matches!(
            message,
            Msg::Refresh
                | Msg::SetPolicy(_)
                | Msg::AcknowledgeWriterGone
                | Msg::ResolveConflict { .. }
                | Msg::ExportCandidate(_)
                | Msg::ExportDraft(_)
        )
    }

    fn controller_busy(&self) -> bool {
        self.undo.is_some()
            || self.active.as_ref().is_some_and(|a| {
                matches!(
                    a.phase,
                    TaskPhase::Quiescing
                        | TaskPhase::Capturing
                        | TaskPhase::Validating
                        | TaskPhase::Promoting
                )
            })
    }

    fn defer(&mut self, message: Msg) {
        if matches!(message, Msg::Refresh)
            && self.deferred.iter().any(|m| matches!(m, Msg::Refresh))
        {
            return;
        }
        if self.deferred.len() >= MAX_DEFERRED {
            self.warn("Too many commands are waiting for the running operation; the newest one was ignored.");
            return;
        }
        self.deferred.push_back(message);
    }

    fn retry_deferred(&mut self) {
        if self.deferred.is_empty() || self.closing || self.controller_busy() {
            return;
        }
        for message in std::mem::take(&mut self.deferred) {
            self.handle(message);
        }
    }

    /// Returns `true` when the actor must exit.
    fn handle(&mut self, message: Msg) -> bool {
        match message {
            Msg::Close(done) => {
                self.shutdown_all(Some(done));
                return true;
            }
            _ if self.closing => {}
            message if Self::needs_controller(&message) && self.controller_busy() => {
                self.defer(message)
            }
            Msg::CheckAdapter => self.check_adapter(true),
            Msg::SettingsChanged => {
                self.settings_changed();
                if self.startup_offer.is_some() && self.session_restore == self.startup_offer {
                    self.offer_saved_session();
                }
            }
            Msg::Submit {
                id,
                brief,
                scope,
                attachments,
            } => self.submit(id, brief, scope.map(|scope| *scope), attachments),
            Msg::Steer(text) => self.composer_steer(text),
            Msg::SendNow { text, attachments } => self.composer_send_now(text, attachments),
            Msg::SendAtBoundary { text, attachments } => {
                self.composer_send_at_boundary(text, attachments)
            }
            Msg::CancelHeld => self.composer_cancel_held(),
            Msg::AnswerQuestions { reference, answers } => {
                self.composer_answer_questions(reference, answers)
            }
            Msg::SetDisplayedPreviewIdentity(identity) => {
                self.displayed_preview_identity = identity;
                self.touch();
            }
            Msg::CancelQueued(id) => {
                if let Some(at) = self.queue.iter().position(|q| q.id == id) {
                    self.queue.remove(at);
                    self.shared.registry.lock().pending -= 1;
                    self.notice("The queued brief was cancelled.");
                }
            }
            Msg::Reply { reference, answer } => self.reply_permission(reference, answer),
            Msg::Clarify { task, text } => self.clarify(task, text),
            Msg::SetMode(mode) => self.driver_call(|driver| driver.set_mode(&mode)),
            Msg::SetConfig(id, value) => {
                self.driver_call(|driver| driver.set_config_option(&id, value.clone()))
            }
            Msg::Stop => self.stop(),
            Msg::Apply => self.apply(),
            Msg::Discard => self.discard(),
            Msg::ExportCandidate(dest) => self.export_candidate(&dest),
            Msg::ExportDraft(dest) => self.export_draft(&dest),
            Msg::Undo(target) => {
                self.start_undo(target);
                self.sync_registry(1);
            }
            Msg::RejectFiles(paths) => self.reject_files(paths),
            Msg::RevertPath { target, path } => {
                self.start_revert_path(target, path);
                self.sync_registry(1);
            }
            Msg::SetPolicy(policy) => self.set_policy(policy),
            Msg::SetPermissionPolicy(policy) => self.permission_policy = policy,
            Msg::SetApprovals(rules) => {
                if let Some(active) = &mut self.active {
                    active.approvals = rules;
                }
            }
            Msg::SetAgentTools(settings) => self.agent_tools = settings,
            Msg::PreviewDisplayed(revision) => self.preview_displayed(&revision),
            Msg::HandoffResolved { published, result } => self.handoff_resolved(&published, result),
            Msg::AcknowledgeWriterGone => self.acknowledge_writer_gone(),
            Msg::ResolveConflict { transaction, note } => {
                self.resolve_conflict(&transaction, &note)
            }
            Msg::InitiateSwitch { target_provider } => self.initiate_switch(target_provider),
            Msg::ChooseHandoffContinuation(choice) => self.choose_handoff_continuation(choice),
            Msg::CancelSwitch => self.cancel_switch(),
            Msg::TransferRetainedQueue => self.transfer_retained_queue(),
            Msg::ClearRetainedQueue => self.clear_retained_queue(),
            Msg::RestoreSession => self.restore_session(),
            Msg::DismissRestoration => self.dismiss_restoration(),
            Msg::NewChat => self.new_chat(),
            Msg::OpenSession(id) => self.open_session(&id),
            Msg::TestHoldToolsGate(tx) => {
                let guard = self.tools.as_ref().and_then(|t| t.gate().try_acquire());
                let _ = tx.send(guard);
            }
            Msg::Refresh => self.refresh_idle(),
            Msg::History {
                chat,
                before,
                limit,
                done,
            } => {
                if chat == self.chat {
                    self.history_page(before, limit, done);
                } else {
                    done(Err(WorkflowError::History(
                        "the conversation changed".to_owned(),
                    )));
                }
            }
            Msg::Progress(phase) => self.on_progress(phase),
            Msg::UndoProgress(phase) => {
                if self.undo.is_some() {
                    self.undo_view = UndoView::InProgress { phase };
                    self.touch();
                }
            }
            Msg::Probe { generation, report } => {
                self.probing = false;
                if generation == self.probe_generation {
                    self.adapter.readiness = AdapterReadiness::Checked {
                        report: Box::new(report),
                        at_unix: now_unix(),
                    };
                    self.touch();
                } else {
                    // The settings changed while it ran: the result describes another
                    // adapter. Check the current one instead.
                    self.check_adapter(false);
                }
            }
            Msg::Pipeline(end) => self.on_pipeline(end),
            Msg::Promote(end) => self.on_promote(end),
            Msg::Undone(end) => self.on_undone(end),
            Msg::Rejected(end) => self.on_rejected(end),
            Msg::Committed { published, kind } => self.on_committed(&published, kind),
            Msg::Evidence { task, kind, view } => self.on_evidence(task, kind, view),
            Msg::JobAborted { kind, message } => self.on_job_aborted(kind, &message),
        }
        // Job results that arrive while closing still have to be settled.
        false
    }

    /// One history page: everything in memory is captured here, the file read runs on its
    /// own thread so a slow or stuck disk never stalls the actor.
    fn history_page(&mut self, before: RowId, limit: usize, done: HistoryDone) {
        let job = self.store.page_job(before, limit);
        let slot = Arc::new(Mutex::new(Some(done)));
        let inner = slot.clone();
        let spawned = std::thread::Builder::new()
            .name("studio-workflow-history".into())
            .spawn(move || {
                let page = job.run().map_err(|e| WorkflowError::History(e.to_string()));
                if let Some(done) = inner.lock().take() {
                    done(page);
                }
            });
        if let Err(error) = spawned
            && let Some(done) = slot.lock().take()
        {
            done(Err(WorkflowError::History(error.to_string())));
        }
    }

    /// Admits one job thread. A refusal hands the payload back so the owning operation
    /// settles (and releases what the payload held) at once.
    fn admit<T: Send + 'static>(
        &mut self,
        kind: JobKind,
        payload: T,
        run: impl FnOnce(T) + Send + 'static,
    ) -> Result<(), (String, T)> {
        let handle = jobs::spawn_job(
            kind,
            self.shared.job_faults.as_ref(),
            self.shared.tx.clone(),
            payload,
            run,
        )?;
        self.jobs.push(handle);
        Ok(())
    }

    /// Reaps finished job threads (a worker that panicked already reported
    /// `JobAborted`; its join result carries nothing more).
    fn reap_jobs(&mut self) {
        let mut running = Vec::with_capacity(self.jobs.len());
        for job in std::mem::take(&mut self.jobs) {
            if job.is_finished() {
                let _ = job.join();
            } else {
                running.push(job);
            }
        }
        self.jobs = running;
    }

    /// The gate of the job in flight, reachable by `AgentWorkflow::stop` without the actor.
    fn set_inflight(&self, gate: Option<Arc<Gate>>) {
        *self.shared.inflight.lock() = gate;
    }

    /// Unsaved rows were dropped to keep the memory bound: the task cannot be recorded
    /// any more, so it ends visibly (the working copy is kept). Without a task there is
    /// nothing to stop and no further row is written (that would only drop more).
    fn settle_log_loss(&mut self) {
        if !std::mem::take(&mut self.log_lost) || self.closing || self.active.is_none() {
            return;
        }
        self.fail_with(present::simple_error(
            "conversation_unsaved",
            "The conversation can no longer be saved",
            "The conversation log could not be written and the in-memory bound was reached, so the task was stopped instead of continuing without a record.",
            Some("Free disk space or fix the project's data folder, then start the task again; the working copy was kept."),
            true,
        ));
    }

    /// Registry bookkeeping after a start attempt: `took` queued/pending submissions
    /// were consumed, and `running` is recomputed from what is actually in flight, in
    /// one critical section (settings changes read it).
    fn sync_registry(&self, took: usize) {
        let mut registry = self.shared.registry.lock();
        registry.pending = registry.pending.saturating_sub(took);
        registry.running = usize::from(self.active.is_some()) + usize::from(self.undo.is_some());
    }

    fn env(&self) -> Option<Env> {
        let build = self.shared.settings.lock().build.clone()?;
        Some(Env {
            controller: self.shared.controller.clone(),
            history: self.shared.paths.project(&self.shared.project),
            build,
            builds: self.shared.paths.builds(),
            tx: self.shared.tx.clone(),
        })
    }
}

fn conflict_of_report(report: &ConflictReport) -> ConflictView {
    ConflictView {
        reason: present::message(&report.reason),
        transaction: Some(report.transaction.clone()),
        external_paths: report.ops.iter().map(|op| op.path.clone()).collect(),
        candidate_paths: Vec::new(),
        overlapping: Vec::new(),
        retained_variants: report.variants.clone(),
    }
}
