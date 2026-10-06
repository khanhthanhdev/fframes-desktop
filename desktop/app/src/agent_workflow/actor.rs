//! The workflow actor: one owned thread that serializes every command, pumps the live
//! ACP driver, owns the task state machine and publishes immutable snapshots.
//!
//! The actor never does long blocking work itself. The quiesce -> capture -> validate
//! pipeline, publication, Undo and adapter probing run on their own threads
//! (`jobs.rs`) and report back with one message; Stop reaches them out of band through
//! the task's [`Gate`] and process scopes, so it stays actionable in every state.
use super::{
    AdapterSettings, PermissionAnswer, Shared, ToolSettings, WorkflowError,
    jobs::{
        self, Env, Gate, HandoffOutcome, JobKind, PipelineEnd, PipelineInput, PromoteEnd, Retained,
        UndoEnd, observe_writer, reap_and_observe, run_pipeline, run_probe, run_promote, run_undo,
    },
    log::RowStore,
    model::*,
    present::{self, CliRoute, ToolRoute},
    tools::{BuildSettings, ToolPlacement, ToolRuntime, render_scope_evidence},
};
use crate::agent_tools::backend::ProjectToolBackend;
use crate::candidate_runner::RunScopes;
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
use studio_agent_spike::{
    AcpDriver, AdapterLaunch, AgentFailure, DiscoveryReport, DriverConfig, DriverMode,
    McpStdioServer, McpStdioSupport,
    driver::{
        AgentEvent, AgentEventKind, MessageRole, OptionValue, PermissionId, PermissionPrompt,
        PermissionReply, PermissionResolution, PromptImage, PromptOutcome, ToolEvent,
    },
};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{
    AgentTaskContext, AgentTaskId, DraftState, EngineError, PromotionError, ReviewPolicy,
    TaskScope, TaskState, WriterGeneration, WriterGoneEvidence,
    candidate_validation::{CapturedCandidate, ChangeSet, FailureContext},
    edit_transaction::ConflictReport,
};
use studio_project::revision::SourceInventory;

const PUBLISH_INTERVAL: Duration = Duration::from_millis(25);
const MAX_TOOL_INDEX: usize = 2048;
const MAX_SCOPE_PROMPT_IMAGES: usize = 3;
const MAX_SCOPE_PROMPT_IMAGE_BYTES: usize = 4 * 1024 * 1024;
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
    SetPolicy(ReviewPolicy),
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
        before: RowId,
        limit: usize,
        done: HistoryDone,
    },
    Close(Sender<()>),
    // ---- job results ----
    Progress(TaskPhase),
    UndoProgress(String),
    Probe(DiscoveryReport),
    Pipeline(PipelineEnd),
    Promote(PromoteEnd),
    Undone(UndoEnd),
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
}

impl ToolOffer {
    fn route(&self) -> ToolRoute {
        ToolRoute {
            mcp: self.mcp.is_some(),
            cli: self.cli.clone(),
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
    adapter: AdapterSettings,
    launch: AdapterLaunch,
    /// Cancels the job in flight; replaced for every job so a Stop that raced the end of
    /// one job can never cancel the next.
    gate: Arc<Gate>,
    driver: Option<AcpDriver>,
    writer: Option<WriterGeneration>,
    session: Option<String>,
    mcp: Option<McpStdioServer>,
    cli: Option<CliRoute>,
    /// The task's capability file while any tool route is offered.
    capability: Option<PathBuf>,
    /// The scopes validation's compile/worker processes live in.
    pipeline_scopes: Option<(ProcessTreeManager, ProcessTreeManager)>,
    retained: Option<Box<Retained>>,
    in_repair: bool,
    pending_prompt: Option<(String, PromptKind)>,
    /// The before evidence is still rendering: the first prompt waits for it.
    before_pending: bool,
    /// The session is ready and the first prompt is waiting for `before_pending`.
    prompt_held: bool,
    /// Stops this task's evidence jobs (task end, close).
    evidence_cancel: Arc<AtomicBool>,
    turn: Option<u64>,
    open_permissions: HashSet<u64>,
    engine_waiting: bool,
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
    capabilities: Option<studio_agent_spike::driver::AgentCapabilityInfo>,
    options: studio_agent_spike::driver::AgentOptions,
    mcp_note: Option<String>,
    task: Option<TaskView>,
    policy: ReviewPolicy,
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
    dirty: bool,
    last_publish: Instant,
    revision: u64,
    closing: bool,
    log_failed: bool,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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
            crate::evidence_preview::decode_png(&bytes, (width, height))
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
        TaskPhase::Editing | TaskPhase::Repairing => TaskState::Editing,
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
                writer_ownership: None,
            },
            capabilities: None,
            options: Default::default(),
            mcp_note: None,
            task: None,
            policy: ReviewPolicy::default(),
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
            dirty: true,
            last_publish: Instant::now(),
            revision: 0,
            closing: false,
            log_failed: false,
        };
        actor.settings_changed();
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
            self.settle_log_loss();
            self.reap_jobs();
            if self.dirty && self.last_publish.elapsed() >= PUBLISH_INTERVAL {
                self.publish(false);
            }
        }
        self.shutdown_all(None);
    }

    // ---- rows ---------------------------------------------------------------------------------

    fn touch(&mut self) {
        self.dirty = true;
    }

    fn task_id(&self) -> Option<AgentTaskId> {
        self.active
            .as_ref()
            .map(|a| a.ctx.identity.task.clone())
            .or_else(|| self.task.as_ref().map(|t| t.id.clone()))
    }

    /// Replaces every configured credential value in `text`.
    fn scrub_str(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "[REDACTED]");
            }
        }
        out
    }

    fn scrub_json(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                if self.secrets.iter().any(|s| text.contains(s.as_str())) {
                    *text = self.scrub_str(text);
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(|v| self.scrub_json(v)),
            serde_json::Value::Object(map) => map.values_mut().for_each(|v| self.scrub_json(v)),
            _ => {}
        }
    }

    /// `value` with every nested string scrubbed of the configured credential values
    /// (`None` only if it cannot be re-read, in which case nothing may be shown).
    fn scrub<T: serde::Serialize + serde::de::DeserializeOwned>(&self, value: T) -> Option<T> {
        if self.secrets.is_empty() {
            return Some(value);
        }
        let mut json = serde_json::to_value(&value).ok()?;
        self.scrub_json(&mut json);
        serde_json::from_value(json).ok()
    }

    fn scrub_kind(&self, kind: RowKind) -> RowKind {
        // Streamed agent text was scrubbed by the driver with the same secrets.
        if kind.is_message() {
            return kind;
        }
        self.scrub(kind).unwrap_or_else(|| RowKind::Notice {
            level: NoticeLevel::Warning,
            text: "[REDACTED]".into(),
        })
    }

    /// Persisting dropped unsaved rows to keep the memory bound: remember it so the
    /// running task is stopped visibly instead of streaming into the void.
    fn note_log_loss(&mut self) {
        if self.store.take_lost() > 0 {
            self.log_lost = true;
        }
    }

    fn push_row(&mut self, kind: RowKind) -> RowId {
        if !kind.is_message() {
            // Anything but more text ends the message being streamed.
            self.finish_streaming();
        }
        let kind = self.scrub_kind(kind);
        let id = self.store.next_id();
        let row = Row {
            id,
            task: self.task_id(),
            at_unix: now_unix(),
            kind,
        };
        self.store.upsert(Arc::new(row));
        self.note_log_loss();
        self.touch();
        id
    }

    /// Rewrites one row, wherever its latest version lives: a row that already left the
    /// resident window is read back from the log and its new version is persisted under
    /// the same id (never a second card).
    fn update_row(&mut self, id: RowId, change: impl FnOnce(&mut RowKind)) -> bool {
        let Some(row) = self.store.latest(id) else {
            return false;
        };
        let mut next = (*row).clone();
        change(&mut next.kind);
        next.kind = self.scrub_kind(next.kind);
        self.store.upsert(Arc::new(next));
        self.note_log_loss();
        self.touch();
        true
    }

    fn notice(&mut self, text: impl AsRef<str>) {
        let text = present::message(text.as_ref());
        self.push_row(RowKind::Notice {
            level: NoticeLevel::Info,
            text,
        });
    }

    fn warn(&mut self, text: impl AsRef<str>) {
        let text = present::message(text.as_ref());
        self.push_row(RowKind::Notice {
            level: NoticeLevel::Warning,
            text,
        });
    }

    fn error_row(&mut self, error: StructuredError) {
        if let Some(task) = &mut self.task
            && self.active.is_some()
        {
            task.error = Some(error.clone());
        }
        self.push_row(RowKind::Error(error));
    }

    fn append_message(&mut self, role: MessageRole, text: &str) {
        if role == MessageRole::User || text.is_empty() {
            return;
        }
        let turn = self.active.as_ref().and_then(|a| a.turn).unwrap_or(0);
        let mut rest = text;
        while !rest.is_empty() {
            if let Some(stream) = &self.stream
                && stream.role == role
                && stream.turn == turn
                && let Some(row) = self.store.get(stream.row)
            {
                let len = match &row.kind {
                    RowKind::Agent { text, .. } | RowKind::Thought { text, .. } => text.len(),
                    _ => usize::MAX,
                };
                if len < MAX_ROW_TEXT_BYTES {
                    let (head, tail) = split_at_boundary(rest, MAX_ROW_TEXT_BYTES - len);
                    if !head.is_empty() {
                        let id = stream.row;
                        let head = head.to_owned();
                        self.update_row(id, |kind| {
                            if let RowKind::Agent { text, .. } | RowKind::Thought { text, .. } =
                                kind
                            {
                                text.push_str(&head);
                            }
                        });
                        rest = tail;
                        continue;
                    }
                }
            }
            let (head, tail) = split_at_boundary(rest, MAX_ROW_TEXT_BYTES);
            let text = head.to_owned();
            let kind = match role {
                MessageRole::Thought => RowKind::Thought {
                    text,
                    turn,
                    streaming: true,
                },
                _ => RowKind::Agent {
                    text,
                    turn,
                    streaming: true,
                },
            };
            let id = self.push_row(kind);
            self.stream = Some(Stream {
                role,
                turn,
                row: id,
            });
            rest = tail;
        }
    }

    fn finish_streaming(&mut self) {
        if let Some(stream) = self.stream.take() {
            self.update_row(stream.row, |kind| {
                if let RowKind::Agent { streaming, .. } | RowKind::Thought { streaming, .. } = kind
                {
                    *streaming = false;
                }
            });
        }
    }

    fn upsert_tool(&mut self, event: ToolEvent) {
        if let Some(&row) = self.tool_rows.get(&event.tool_call_id) {
            let event = event.clone();
            if self.update_row(row, |kind| {
                if let RowKind::Tool(card) = kind {
                    if event.title.is_some() {
                        card.title = event.title.clone();
                    }
                    if event.kind.is_some() {
                        card.kind = event.kind.clone();
                    }
                    if event.status.is_some() {
                        card.status = event.status;
                    }
                    card.updates += 1;
                }
            }) {
                return;
            }
        }
        while self.tool_rows.len() >= MAX_TOOL_INDEX {
            match self.tool_order.pop_front() {
                Some(oldest) => {
                    self.tool_rows.remove(&oldest);
                }
                None => {
                    self.tool_rows.clear();
                    break;
                }
            }
        }
        let id = self.push_row(RowKind::Tool(ToolCard {
            call_id: event.tool_call_id.clone(),
            title: event.title,
            kind: event.kind,
            status: event.status,
            updates: 0,
        }));
        self.tool_order.push_back(event.tool_call_id.clone());
        self.tool_rows.insert(event.tool_call_id, id);
    }

    // ---- snapshot ---------------------------------------------------------------------------------

    fn publish(&mut self, force: bool) {
        if !force && !self.dirty {
            return;
        }
        if let Err(error) = self.store.flush() {
            if !self.log_failed {
                self.log_failed = true;
                self.warn(format!(
                    "The conversation could not be saved ({error}); it stays visible until the app closes."
                ));
            }
        } else {
            self.log_failed = false;
        }
        self.revision += 1;
        let rows = self.store.resident();
        // Open requests are tracked outside the transcript window: an evicted card is
        // still open (and answerable).
        let open_permissions = self.permission_rows.len();
        let (settings_adapter, tools_settings) = {
            let settings = self.shared.settings.lock();
            (settings.adapter.clone(), settings.tools.clone())
        };
        let policy_enabled = settings_adapter
            .as_ref()
            .is_some_and(|a| a.mcp == McpStdioSupport::Baseline);
        let binary_available = tools_settings
            .as_ref()
            .and_then(|t| t.studio_mcp.as_ref())
            .is_some_and(|p| p.is_file());
        let live = self.active.as_ref().filter(|a| a.driver.is_some());
        let mcp_active = live.is_some_and(|a| a.mcp.is_some());
        let cli_active = live.is_some_and(|a| a.cli.is_some());
        let capability_file = live.and_then(|a| a.capability.clone());
        let task = self.task.clone().and_then(|task| self.scrub(task));
        let history: Vec<HistoryEntry> = self
            .history
            .iter()
            .cloned()
            .map(|mut entry| {
                entry.summary = self.scrub_str(&entry.summary);
                entry
            })
            .collect();
        let snapshot = WorkflowSnapshot {
            revision: self.revision,
            project: self.shared.project.clone(),
            adapter: self.adapter.clone(),
            capabilities: self.capabilities.clone(),
            options: self.options.clone(),
            mcp: McpView {
                policy_enabled,
                binary_available,
                active: mcp_active,
                cli_active,
                capability_file,
                note: self.mcp_note.clone(),
            },
            task,
            queue: self
                .queue
                .iter()
                .map(|q| QueuedBrief {
                    id: q.id,
                    summary: self.scrub_str(&present::brief_summary(&q.brief)),
                    queued_unix: q.queued_unix,
                    scope: q.scope.as_ref().map(TaskScope::label),
                    stale_scope: q.scope.as_ref().is_some_and(|scope| {
                        scope.compiled.is_some()
                            && !self
                                .displayed_preview_identity
                                .as_ref()
                                .is_some_and(|identity| scope.is_current_preview(identity))
                    }),
                })
                .collect(),
            older_rows: self.store.has_older(),
            resources: ResourceView {
                resident_rows: self.store.resident_len(),
                resident_bytes: self.store.resident_bytes(),
                max_resident_rows: MAX_RESIDENT_ROWS,
                max_resident_bytes: MAX_RESIDENT_ROW_BYTES,
                queue_len: self.queue.len(),
                open_permissions,
                owned_processes: self.shared.root_scope.active_count(),
                broker_grants: self.tools.as_ref().map_or(0, ToolRuntime::grants),
                tool_workers: self.tools.as_ref().map_or(0, ToolRuntime::workers),
            },
            rows,
            review_policy: self.policy,
            undo: match &self.undo {
                Some(_) => match &self.undo_view {
                    UndoView::InProgress { .. } => self.undo_view.clone(),
                    _ => UndoView::InProgress {
                        phase: "Undo".into(),
                    },
                },
                None => self.undo_view.clone(),
            },
            recovery: self.recovery.clone(),
            handoff: self.handoff.clone().and_then(|h| self.scrub(h)),
            history,
            closed: self.closing,
        };
        *self.shared.snapshot.lock() = Arc::new(snapshot);
        self.shared.changed.notify_all();
        if let Some(notify) = &self.shared.notify {
            notify();
        }
        self.dirty = false;
        self.last_publish = Instant::now();
    }

    fn settings_changed(&mut self) {
        let settings = self.shared.settings.lock().adapter.clone();
        self.adapter = match &settings {
            None => AdapterView {
                provider: None,
                executable: None,
                readiness: AdapterReadiness::NotConfigured,
                writer_ownership: None,
            },
            Some(adapter) => AdapterView {
                provider: Some(adapter.provider.clone()),
                executable: Some(adapter.adapter.executable.clone()),
                readiness: AdapterReadiness::Unchecked,
                writer_ownership: Some(adapter.writer_ownership.label().to_owned()),
            },
        };
        if let Some(tools) = self.tools.take() {
            tools.shutdown();
        }
        self.touch();
    }

    /// Recomputes everything derived from the controller. Only called while no job may
    /// hold the controller for long.
    fn refresh_idle(&mut self) {
        let controller = self.shared.controller.clone();
        let mut c = controller.lock();
        self.policy = c.review_policy();
        if self.undo.is_none() {
            self.undo_view = match c.undo_status() {
                studio_engine::UndoStatus::Available { target, summary } => {
                    UndoView::Available { target, summary }
                }
                studio_engine::UndoStatus::Unavailable { reason } => {
                    UndoView::Unavailable { reason }
                }
            };
        }
        let entries = c.state().task_history().entries();
        let skip = entries.len().saturating_sub(MAX_HISTORY_ENTRIES);
        self.history = entries[skip..]
            .iter()
            .map(|r| HistoryEntry {
                id: r.id.clone(),
                kind: format!("{:?}", r.kind),
                summary: present::message(&r.prompt_summary),
                files: r.changes.len(),
                published: present::short(r.published.as_str()),
                undoes: r.undoes.clone(),
                committed_unix: r.committed_unix,
            })
            .collect();
        let status = c.recovery_status();
        let unresolved = status
            .unresolved
            .iter()
            .map(conflict_of_report)
            .collect::<Vec<_>>();
        let rolled_back = status.report.rolled_back.clone();
        let suspended = status.suspended.clone();
        let notice = c.recovery_notice.clone();
        let draft_path = c.agent_draft_store().path().to_path_buf();
        let draft = c.agent_draft_state().ok().flatten();
        drop(c);
        self.recovery = RecoveryView {
            rolled_back,
            unresolved_conflicts: unresolved,
            suspended,
            notice,
            draft: draft.map(|state| DraftNotice {
                interrupted_by_restart: self.interrupted_task.is_some()
                    && matches!(&state, DraftState::UnsafeWriter { task, .. }
                        if Some(task) == self.interrupted_task.as_ref().map(|t| &t.0)),
                can_acknowledge: matches!(state, DraftState::UnsafeWriter { .. }),
                state,
                draft: draft_path,
            }),
        };
        self.touch();
    }

    /// An `UnsafeWriter` draft left by a previous session means a task was running when
    /// the app ended: surface it once as an interrupted task.
    fn detect_interrupted(&mut self) {
        let state = self
            .shared
            .controller
            .lock()
            .agent_draft_state()
            .ok()
            .flatten();
        let Some(DraftState::UnsafeWriter { task, reason }) = state else {
            return;
        };
        if !reason.contains("previous session ended") {
            return;
        }
        let id = AgentTaskId(task);
        let already = self.store.resident().iter().any(|row| {
            row.task.as_ref() == Some(&id)
                && matches!(&row.kind, RowKind::Outcome(card) if card.kind == OutcomeKind::Interrupted)
        });
        self.interrupted_task = Some(id.clone());
        if already {
            return;
        }
        let task = Some(id);
        let row_id = self.store.next_id();
        self.store.upsert(Arc::new(Row {
            id: row_id,
            task,
            at_unix: now_unix(),
            kind: RowKind::Outcome(OutcomeCard {
                kind: OutcomeKind::Interrupted,
                summary: "The previous session ended while an agent task was running. Its working copy was kept and is locked until you confirm that no agent is still writing to it.".into(),
                published: None,
                draft_retained: true,
            }),
        }));
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
            Msg::CheckAdapter => self.check_adapter(),
            Msg::SettingsChanged => self.settings_changed(),
            Msg::Submit { id, brief, scope } => self.submit(id, brief, scope.map(|scope| *scope)),
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
            Msg::SetPolicy(policy) => self.set_policy(policy),
            Msg::PreviewDisplayed(revision) => self.preview_displayed(&revision),
            Msg::HandoffResolved { published, result } => self.handoff_resolved(&published, result),
            Msg::AcknowledgeWriterGone => self.acknowledge_writer_gone(),
            Msg::ResolveConflict { transaction, note } => {
                self.resolve_conflict(&transaction, &note)
            }
            Msg::Refresh => self.refresh_idle(),
            Msg::History {
                before,
                limit,
                done,
            } => self.history_page(before, limit, done),
            Msg::Progress(phase) => self.on_progress(phase),
            Msg::UndoProgress(phase) => {
                if self.undo.is_some() {
                    self.undo_view = UndoView::InProgress { phase };
                    self.touch();
                }
            }
            Msg::Probe(report) => {
                self.probing = false;
                self.adapter.readiness = AdapterReadiness::Checked {
                    report,
                    at_unix: now_unix(),
                };
                self.touch();
            }
            Msg::Pipeline(end) => self.on_pipeline(end),
            Msg::Promote(end) => self.on_promote(end),
            Msg::Undone(end) => self.on_undone(end),
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

    // ---- adapter probe ----------------------------------------------------------------------------

    fn check_adapter(&mut self) {
        let Some(adapter) = self.shared.settings.lock().adapter.clone() else {
            return;
        };
        if self.probing {
            return;
        }
        self.probing = true;
        self.adapter.readiness = AdapterReadiness::Checking;
        self.touch();
        let scratch = self
            .shared
            .paths
            .agent(&self.shared.project)
            .join(format!("probe-{}", uuid::Uuid::new_v4().simple()));
        let search = self.shared.search.clone();
        let timeout = self.shared.probe_timeout;
        let processes = self.shared.root_scope.sub_manager();
        let tx = self.shared.tx.clone();
        let payload = (
            adapter.adapter,
            search,
            adapter.auth_method,
            scratch,
            timeout,
            processes,
        );
        let refused = self.admit(
            JobKind::Probe,
            payload,
            move |(config, search, method, scratch, timeout, processes)| {
                let report = run_probe(config, search, method, scratch, timeout, processes);
                let _ = tx.send(Msg::Probe(report));
            },
        );
        if let Err((message, _)) = refused {
            self.probing = false;
            self.adapter.readiness = AdapterReadiness::Unchecked;
            self.error_row(present::simple_error(
                "thread_failed",
                "A background worker could not be started",
                &message,
                Some("Close other applications to free system resources, then check again."),
                false,
            ));
        }
    }

    // ---- starting tasks -------------------------------------------------------------------------------

    fn submit(&mut self, id: u64, brief: String, scope: Option<TaskScope>) {
        if self.active.is_some() || self.undo.is_some() || !self.queue.is_empty() {
            let scope_label = scope
                .as_ref()
                .map(TaskScope::label)
                .map(|label| format!(" · {label}"))
                .unwrap_or_default();
            self.queue.push_back(Queued {
                id,
                brief: brief.clone(),
                queued_unix: now_unix(),
                scope,
            });
            self.notice(format!(
                "Queued: {}{scope_label}",
                present::brief_summary(&brief)
            ));
            return;
        }
        self.begin_task(brief, scope);
        self.sync_registry(1);
    }

    /// Starts queued briefs until one actually starts a task (or the queue is empty).
    fn start_next(&mut self) {
        while self.active.is_none() && self.undo.is_none() && !self.closing {
            let Some(next) = self.queue.pop_front() else {
                break;
            };
            let started = self.begin_task(next.brief, next.scope);
            self.sync_registry(1);
            if started {
                break;
            }
        }
    }

    fn begin_task(&mut self, brief: String, scope: Option<TaskScope>) -> bool {
        if let Some(scope) = &scope {
            if let Err(error) = scope.validate() {
                self.error_row(present::simple_error(
                    "invalid_task_scope",
                    "The selected scope is invalid",
                    &error.to_string(),
                    Some("Reselect the scene or frame range, then send the brief again."),
                    false,
                ));
                return false;
            }
            if scope.compiled.is_some()
                && !self
                    .displayed_preview_identity
                    .as_ref()
                    .is_some_and(|identity| scope.is_current_preview(identity))
            {
                self.error_row(present::simple_error(
                    "stale_task_scope",
                    "The selected scope is stale",
                    "The displayed preview changed after this brief was submitted. The saved scene/range was not remapped to the new timeline.",
                    Some("Reselect the scene or frame range and send the brief again."),
                    false,
                ));
                return false;
            }
        }
        let (adapter, build, tools_settings) = {
            let settings = self.shared.settings.lock();
            (
                settings.adapter.clone(),
                settings.build.clone(),
                settings.tools.clone(),
            )
        };
        let Some(adapter) = adapter else {
            self.error_row(present::simple_error(
                "adapter_not_configured",
                "No agent is configured",
                "Configure an ACP adapter in the agent setup first.",
                Some("Open the agent setup and choose an adapter."),
                false,
            ));
            return false;
        };
        let Some(build) = build else {
            self.error_row(present::simple_error(
                "sdk_unavailable",
                "No compatible SDK is installed",
                "Validation compiles the edit against the SDK; none is selected.",
                Some("Install or select the SDK in setup."),
                false,
            ));
            return false;
        };
        // Everything that can fail without side effects comes before the task exists.
        let auth_env = self.shared.auth_env.clone();
        let launch =
            match AdapterLaunch::resolve_with_env(&adapter.adapter, &self.shared.search, |name| {
                auth_env(name)
            }) {
                Ok(launch) => match &adapter.auth_method {
                    Some(method) => launch.with_auth_method(method.clone()),
                    None => launch,
                },
                Err(error) => {
                    self.error_row(present::simple_error(
                        "adapter_missing",
                        "The agent executable was not found",
                        &error.to_string(),
                        Some("Check the adapter configuration, then check readiness again."),
                        false,
                    ));
                    return false;
                }
            };
        let begun = match scope {
            Some(scope) => self
                .shared
                .controller
                .lock()
                .begin_agent_task_scoped(&brief, scope),
            None => self.shared.controller.lock().begin_agent_task(&brief),
        };
        let context = match begun {
            Ok(context) => context,
            Err(error) => {
                self.error_row(present::engine_error(&error, false));
                self.refresh_idle();
                return false;
            }
        };
        let identity = context.identity.clone();
        self.shared.live.set(Some(identity.clone()));
        let policy = self.shared.controller.lock().review_policy();
        self.policy = policy;
        self.secrets = launch
            .secrets
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect();
        self.secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        self.secrets.dedup();
        let offer = self.offer_tools(&build, &adapter, tools_settings.as_ref(), &context);
        self.shared
            .evidence_images
            .lock()
            .begin_task(identity.task.clone());
        self.task = Some(TaskView {
            id: identity.task.clone(),
            generation: identity.generation,
            phase: TaskPhase::Starting,
            engine_state: TaskState::ContextReady,
            brief: present::brief_summary(&context.brief),
            source_base: present::short(context.source_base.revision().as_str()),
            scope: context.scope.clone(),
            before: None,
            after: None,
            // Image delivery is not known until the adapter finishes negotiating ACP
            // capabilities. Do not present the text-only fallback before then.
            image_limitation: None,
            draft: context.draft.clone(),
            review_policy: policy,
            repair: RepairView {
                used: 0,
                max: studio_engine::MAX_AUTOMATIC_REPAIRS,
                in_progress: false,
                context_summary: None,
            },
            turns: 0,
            writer: None,
            changes: None,
            validation: None,
            review: None,
            conflict: None,
            error: None,
            stop_requested: false,
            reason: None,
            started_unix: now_unix(),
            ended_unix: None,
            draft_state: None,
        });
        if let Some(previous) = &context.archived_previous {
            self.notice(format!(
                "The previous retained working copy was archived to {}",
                previous.display()
            ));
        }
        let route = offer.route();
        let prompt = present::first_prompt(&context, &route);
        self.active = Some(Active {
            ctx: context,
            phase: TaskPhase::Starting,
            policy,
            adapter,
            launch,
            gate: Gate::new(),
            driver: None,
            writer: None,
            session: None,
            mcp: offer.mcp,
            cli: offer.cli,
            capability: offer.capability,
            pipeline_scopes: None,
            retained: None,
            in_repair: false,
            pending_prompt: Some((prompt, PromptKind::Brief)),
            before_pending: false,
            prompt_held: false,
            evidence_cancel: Arc::new(AtomicBool::new(false)),
            turn: None,
            open_permissions: HashSet::new(),
            engine_waiting: false,
            failed: false,
            stop_requested: false,
            promoting: false,
            promote_after_evidence: false,
        });
        self.push_row(RowKind::User {
            text: present::bounded(&brief, MAX_ROW_TEXT_BYTES),
            source: UserSource::Brief,
        });
        // The accepted-awaiting-preview state belongs to the accepted source, not to the
        // task that follows it: only a new durable promotion replaces it.
        self.options = Default::default();
        self.start_before_evidence();
        if let Err(error) = self.spawn_writer() {
            self.fail_with(*error);
        }
        self.touch();
        true
    }

    fn ensure_tools(&mut self, build: &BuildSettings, runtime_dir: PathBuf) -> Result<(), String> {
        if self.tools.is_some() {
            return Ok(());
        }
        let project = self.shared.project.clone();
        let processes = self.shared.root_scope.sub_manager();
        let runtime = ToolRuntime::start(
            ToolPlacement {
                history: self.shared.paths.project(&project),
                artifacts: self.shared.paths.agent(&project).join("artifacts"),
                builds: self.shared.paths.builds(),
                runtime_dir,
                project,
            },
            build,
            processes,
            self.shared.live.clone(),
        )?;
        self.tools = Some(runtime);
        Ok(())
    }

    /// Registers the task with the tool broker and decides which routes the agent gets.
    /// One capability (a file path; the secret stays inside it) serves both the MCP
    /// server and the `studio-tools` command, so it is granted whenever any route is
    /// offered, whatever host policy says about MCP, and dies with the task.
    fn offer_tools(
        &mut self,
        build: &BuildSettings,
        adapter: &AdapterSettings,
        tools: Option<&ToolSettings>,
        context: &AgentTaskContext,
    ) -> ToolOffer {
        let mut offer = ToolOffer::default();
        self.mcp_note = None;
        let Some(tools) = tools else {
            self.mcp_note = Some(
                "No project tool helpers are configured; the agent has no project tools in this task."
                    .into(),
            );
            return offer;
        };
        let policy_allows = adapter.mcp == McpStdioSupport::Baseline;
        let mcp_binary = tools.studio_mcp.clone().filter(|p| p.is_file());
        let cli_binary = tools.studio_tools.clone().filter(|p| p.is_file());
        let want_mcp = policy_allows && mcp_binary.is_some();
        if !want_mcp && cli_binary.is_none() {
            self.mcp_note = Some(if policy_allows {
                "Neither the MCP helper nor the `studio-tools` command was found; the agent has no project tools in this task.".to_owned()
            } else {
                "Project tools over MCP are disabled by host policy and no `studio-tools` command was found; the agent has no project tools in this task.".to_owned()
            });
            return offer;
        }
        if let Err(error) = self.ensure_tools(build, tools.runtime_dir.clone()) {
            self.mcp_note = Some(format!(
                "Project tools are unavailable ({error}); the agent has no project tools in this task."
            ));
            return offer;
        }
        let runtime = self.tools.as_ref().expect("tools started");
        let grant = match runtime.grant(context) {
            Ok(grant) => grant,
            Err(error) => {
                self.mcp_note = Some(format!(
                    "Project tools are unavailable for this task ({error})."
                ));
                return offer;
            }
        };
        let mut notes: Vec<String> = Vec::new();
        if let (true, Some(binary)) = (want_mcp, &mcp_binary) {
            match ToolRuntime::mcp_server(&grant, binary) {
                Ok(server) => offer.mcp = Some(server),
                Err(error) => notes.push(format!("The MCP route could not be prepared ({error}).")),
            }
        }
        if let Some(command) = cli_binary {
            offer.cli = Some(CliRoute {
                command,
                capability_file: grant.capability_file.clone(),
            });
        }
        if offer.mcp.is_none() && offer.cli.is_some() {
            if !policy_allows {
                notes.push("Project tools over MCP are disabled by host policy; the `studio-tools` command is offered instead.".into());
            } else if mcp_binary.is_none() {
                notes.push(
                    "The MCP helper was not found; the `studio-tools` command is offered instead."
                        .into(),
                );
            }
        }
        if offer.mcp.is_none() && offer.cli.is_none() {
            runtime.end_task(&context.identity);
            notes.push("The agent has no project tools in this task.".into());
        } else {
            offer.capability = Some(grant.capability_file);
        }
        if !notes.is_empty() {
            self.mcp_note = Some(notes.join(" "));
        }
        offer
    }

    /// Starts (or, for the repair, restarts) the adapter in the task's own scope.
    fn spawn_writer(&mut self) -> Result<(), Box<StructuredError>> {
        let controller = self.shared.controller.clone();
        let limits = self.shared.limits.clone();
        let active = self.active.as_mut().expect("active task");
        let identity = active.ctx.identity.clone();
        let scope = controller.lock().agent_processes(&identity);
        let scope = scope.map_err(|e| Box::new(present::engine_error(&e, true)))?;
        // Qualification is re-derived at every launch when the host supplies a probe
        // (hashing the executable can take a moment; this is the workflow's own thread).
        let ownership = match &active.adapter.ownership_probe {
            Some(probe) => probe(),
            None => active.adapter.writer_ownership.clone(),
        };
        let config = DriverConfig {
            provider: active.adapter.provider.clone(),
            task: identity.task.0.clone(),
            cwd: active.ctx.draft.clone(),
            launch: active.launch.clone(),
            limits,
            resume_session: None,
            writer_ownership: ownership.clone(),
            mode: DriverMode::Full,
            mcp_servers: active.mcp.iter().cloned().collect(),
            mcp_stdio: active.adapter.mcp,
        };
        let driver = AcpDriver::start(config, &scope)
            .map_err(|f| Box::new(present::provider_error(&f, true)))?;
        let started = controller
            .lock()
            .agent_writer_started(&identity, ownership.clone());
        let generation = match started {
            Ok(generation) => generation,
            Err(error) => {
                let _ = driver.shutdown();
                return Err(Box::new(present::engine_error(&error, true)));
            }
        };
        active.driver = Some(driver);
        active.writer = Some(generation);
        active.session = None;
        active.turn = None;
        active.failed = false;
        active.open_permissions.clear();
        active.engine_waiting = false;
        // A pump of an earlier driver must not act on this one.
        self.driver_epoch += 1;
        let ownership = ownership.label().to_owned();
        let phase = if active.in_repair {
            TaskPhase::Repairing
        } else {
            TaskPhase::Starting
        };
        if let Some(task) = &mut self.task {
            task.writer = Some(WriterView {
                generation: generation.value(),
                ownership,
                provider_session: None,
            });
        }
        self.set_phase(phase);
        Ok(())
    }

    fn set_phase(&mut self, phase: TaskPhase) {
        if let Some(active) = &mut self.active {
            active.phase = phase;
        }
        let repairing = self
            .active
            .as_ref()
            .is_some_and(|a| a.in_repair && a.driver.is_some());
        if let Some(task) = &mut self.task {
            task.phase = phase;
            task.engine_state = engine_state_of(phase);
            task.repair.in_progress = repairing && phase == TaskPhase::Repairing;
        }
        self.touch();
    }

    // ---- pumping the live driver ---------------------------------------------------------------------

    fn pump(&mut self) {
        // Everything below belongs to the driver that is live NOW. Handling one of its
        // events can end the task and start a queued successor in the same call: the
        // epoch changes then, and nothing drained from the old driver (events or its
        // latched failure) may touch the new one.
        let epoch = self.driver_epoch;
        let Some(active) = &mut self.active else {
            return;
        };
        let Some(driver) = &active.driver else {
            return;
        };
        let task = active.ctx.identity.task.0.clone();
        // Drain everything queued (bounded per tick so commands are never starved): the
        // driver's 256-slot queue seals the session if a producer outruns its consumer.
        let mut events = Vec::new();
        for _ in 0..16 {
            let batch = driver.drain_events(256);
            let done = batch.len() < 256;
            events.extend(batch);
            if done {
                break;
            }
        }
        let failure = driver.failure();
        let starting =
            active.phase == TaskPhase::Starting || (active.in_repair && active.session.is_none());
        let ready = if starting && failure.is_none() {
            driver.wait_ready(Duration::ZERO).ok()
        } else {
            None
        };
        for event in events {
            if event.task != task {
                continue;
            }
            self.on_event(event);
            // The first row the log cannot keep ends the task at once: draining the rest
            // of the batch against a failing disk is what lets the driver's queue overflow.
            self.settle_log_loss();
            if self.driver_epoch != epoch || self.active.as_ref().is_none_or(|a| a.driver.is_none())
            {
                return;
            }
        }
        if self.driver_epoch != epoch {
            return;
        }
        if let Some(active) = &self.active
            && let Some(failure) = failure
            && !active.failed
            && active.driver.is_some()
        {
            self.on_provider_failure(failure);
            return;
        }
        if let Some(info) = ready
            && self
                .active
                .as_ref()
                .is_some_and(|a| a.session.is_none() && a.driver.is_some())
        {
            self.on_ready(info);
        }
    }

    fn on_ready(&mut self, info: studio_agent_spike::driver::SessionInfo) {
        let Some(active) = &self.active else {
            return;
        };
        let identity = active.ctx.identity.clone();
        self.capabilities = Some(info.initialized.capabilities.clone());
        if let Some(driver) = &active.driver {
            self.options = driver.options();
        }
        let Some(session) = info.session_id.clone() else {
            // A session id marks readiness for us; an adapter without one cannot run a task.
            let error = present::simple_error(
                "provider_no_session",
                "The agent did not create a session",
                "The adapter initialized but returned no session id.",
                Some("Check readiness in the agent setup."),
                true,
            );
            self.fail_with(error);
            return;
        };
        // The controller guard is a statement of its own: a refusal runs cleanup that
        // locks the controller again.
        let recorded = self
            .shared
            .controller
            .lock()
            .agent_provider_session(&identity, &session);
        if let Err(error) = recorded {
            let error = present::engine_error(&error, true);
            self.fail_with(error);
            return;
        }
        if let Some(active) = &mut self.active {
            active.session = Some(session.clone());
        }
        if let Some(task) = &mut self.task
            && let Some(writer) = &mut task.writer
        {
            writer.provider_session = Some(session);
        }
        if let Some(active) = &mut self.active
            && active.before_pending
            && matches!(active.pending_prompt, Some((_, PromptKind::Brief)))
        {
            // The first prompt carries the before evidence: wait for the render.
            active.prompt_held = true;
            self.touch();
            return;
        }
        self.dispatch_pending_prompt();
    }

    /// Sends the waiting prompt (the Brief moves the engine to Editing first).
    fn dispatch_pending_prompt(&mut self) {
        let Some(active) = &self.active else {
            return;
        };
        let identity = active.ctx.identity.clone();
        let Some((prompt, kind)) = self
            .active
            .as_mut()
            .and_then(|active| active.pending_prompt.take())
        else {
            return;
        };
        if matches!(&kind, PromptKind::Brief) {
            let moved = self
                .shared
                .controller
                .lock()
                .agent_task_transition(&identity, TaskState::Editing);
            if let Err(error) = moved {
                let error = present::engine_error(&error, true);
                self.fail_with(error);
                return;
            }
        }
        let (images, image_limitation) = if matches!(&kind, PromptKind::Brief) {
            self.before_prompt_images(&identity.task)
        } else {
            (Vec::new(), None)
        };
        if let Some(task) = &mut self.task
            && task.scope.compiled.is_some()
        {
            task.image_limitation = image_limitation.clone();
        }
        let mut prompt = prompt;
        if matches!(&kind, PromptKind::Brief)
            && let Some(before) = self.task.as_ref().and_then(|task| task.before.as_ref())
        {
            prompt.push_str(&present::before_evidence_prompt(
                before,
                image_limitation.as_deref(),
            ));
        }
        self.send_prompt(&prompt, images);
    }

    // ---- scope evidence ------------------------------------------------------------------------

    fn set_evidence(&mut self, kind: EvidenceKind, view: EvidenceView) {
        if let Some(task) = &mut self.task {
            match kind {
                EvidenceKind::Before => task.before = Some(view),
                EvidenceKind::After => task.after = Some(view),
            }
        }
    }

    /// Loads the bounded, task-owned before PNGs only when the negotiated adapter can
    /// consume ACP image content. Any failure falls back to the text artifact references.
    fn before_prompt_images(&self, task: &AgentTaskId) -> (Vec<PromptImage>, Option<String>) {
        let Some(task_view) = self.task.as_ref().filter(|view| &view.id == task) else {
            return (Vec::new(), None);
        };
        if task_view.scope.compiled.is_none() {
            return (Vec::new(), None);
        }
        let Some(before) = task_view.before.as_ref() else {
            return (
                Vec::new(),
                Some("Before image evidence was unavailable; the agent receives text only.".into()),
            );
        };
        if before.state != EvidenceState::Ready {
            return (
                Vec::new(),
                Some("Before image evidence was unavailable; the agent receives text only.".into()),
            );
        }
        let supports_images = self
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.prompt_image);
        if !supports_images {
            return (Vec::new(), Some(present::IMAGE_LIMITATION.to_owned()));
        }
        if before.artifacts.len() > MAX_SCOPE_PROMPT_IMAGES {
            return (
                Vec::new(),
                Some(
                    "Before image evidence exceeded the attachment limit; text references only."
                        .into(),
                ),
            );
        }
        let Some(backend) = self.tools.as_ref().map(ToolRuntime::backend) else {
            return (
                Vec::new(),
                Some("The app could not load before images; the agent receives text only.".into()),
            );
        };
        let mut total_bytes = 0usize;
        let mut images = Vec::with_capacity(before.artifacts.len());
        for artifact in &before.artifacts {
            let remaining = MAX_SCOPE_PROMPT_IMAGE_BYTES.saturating_sub(total_bytes);
            let Ok(bytes) = backend
                .artifacts()
                .read_png_for_task(task, &artifact.id, remaining)
            else {
                return (
                    Vec::new(),
                    Some("Before images exceeded the prompt budget or expired; text references only.".into()),
                );
            };
            total_bytes += bytes.len();
            images.push(PromptImage::png(bytes));
        }
        if images.is_empty() {
            return (
                images,
                Some(
                    "Before image evidence contained no images; the agent receives text only."
                        .into(),
                ),
            );
        }
        (images, None)
    }

    /// Starts the background render of the frozen base around the task's scope. The
    /// first prompt waits for it; an unavailable render is shown and the prompt says so.
    fn start_before_evidence(&mut self) {
        let Some(active) = &self.active else {
            return;
        };
        if active.ctx.scope.compiled.is_none() {
            return;
        }
        let revision = active.ctx.source_base.revision().clone();
        if let Some(active) = &mut self.active {
            active.before_pending = true;
        }
        self.start_evidence(EvidenceKind::Before, revision);
    }

    fn start_evidence(&mut self, kind: EvidenceKind, revision: studio_project::SourceRevision) {
        let Some(active) = &self.active else {
            return;
        };
        let Some(compiled) = active.ctx.scope.compiled.clone() else {
            return;
        };
        let identity = active.ctx.identity.clone();
        let cancel = active.evidence_cancel.clone();
        let short = present::short(revision.as_str());
        let shared = self.shared.clone();
        let unavailable = |note: String| EvidenceView {
            state: EvidenceState::Unavailable,
            revision: short.clone(),
            artifacts: Vec::new(),
            note: Some(present::message(&note)),
        };
        let backend = match (&self.tools, &active.capability) {
            (Some(tools), Some(_)) => tools.backend(),
            _ => {
                let note = self
                    .mcp_note
                    .clone()
                    .unwrap_or_else(|| "project tools are not available for this task".into());
                self.on_evidence(identity.task.clone(), kind, unavailable(note));
                return;
            }
        };
        self.set_evidence(
            kind,
            EvidenceView {
                state: EvidenceState::Pending,
                revision: short.clone(),
                artifacts: Vec::new(),
                note: None,
            },
        );
        let tx = self.shared.tx.clone();
        let task = identity.task.clone();
        let refused = self.admit(
            JobKind::Evidence {
                task: task.clone(),
                kind,
            },
            (backend, identity, revision, compiled, cancel, tx),
            move |(backend, identity, revision, compiled, cancel, tx)| {
                let deadline = Instant::now() + EVIDENCE_TIMEOUT;
                let view =
                    render_scope_evidence(&backend, &identity, &revision, &compiled, &|| {
                        cancel.load(Ordering::Acquire) || Instant::now() > deadline
                    });
                let artifacts =
                    (view.state == EvidenceState::Ready).then(|| view.artifacts.clone());
                let task = identity.task.clone();
                let previews = artifacts.map_or_else(Vec::new, |artifacts| {
                    read_evidence_preview_bytes(&shared, &backend, &task, &artifacts)
                });
                // AutoApply may promote and finalize as soon as the actor receives this
                // message. Decode first so the terminal snapshot cannot outrun its images.
                decode_evidence_previews(&shared, &task, previews);
                let _ = tx.send(Msg::Evidence {
                    task: identity.task.clone(),
                    kind,
                    view,
                });
            },
        );
        if let Err((message, _)) = refused {
            self.on_evidence(task, kind, unavailable(message));
        }
    }

    fn on_evidence(&mut self, task: AgentTaskId, kind: EvidenceKind, view: EvidenceView) {
        let Some(active) = &mut self.active else {
            return;
        };
        if active.ctx.identity.task != task {
            return;
        }
        let settled = view.state != EvidenceState::Pending;
        self.set_evidence(kind, view.clone());
        if kind == EvidenceKind::Before && settled {
            let Some(active) = &mut self.active else {
                return;
            };
            active.before_pending = false;
            if std::mem::take(&mut active.prompt_held) {
                self.dispatch_pending_prompt();
            }
        }
        self.touch();
        let promote = kind == EvidenceKind::After
            && settled
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.promote_after_evidence);
        if promote {
            if let Some(active) = &mut self.active {
                active.promote_after_evidence = false;
            }
            let stopped = self.closing
                || self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.stop_requested || active.gate.is_cancelled());
            if stopped {
                self.cancel_or_interrupt();
            } else {
                self.start_promote();
            }
        }
    }

    fn send_prompt(&mut self, text: &str, images: Vec<PromptImage>) {
        let Some(active) = &mut self.active else {
            return;
        };
        let Some(driver) = &active.driver else {
            return;
        };
        let prompt_result = if images.is_empty() {
            driver.prompt(text)
        } else {
            driver.prompt_with_images(text, images)
        };
        match prompt_result {
            Ok(turn) => {
                active.turn = Some(turn);
                self.tool_rows.clear();
                self.tool_order.clear();
                self.stream = None;
                let phase = if active.in_repair {
                    TaskPhase::Repairing
                } else {
                    TaskPhase::Editing
                };
                if let Some(task) = &mut self.task {
                    task.turns += 1;
                }
                self.set_phase(phase);
            }
            Err(error) => {
                let failure = match error {
                    studio_agent_spike::driver::DriverError::Failed(f) => f,
                    other => AgentFailure::new(
                        studio_agent_spike::driver::FailureKind::PromptRejected,
                        studio_agent_spike::driver::Phase::Prompt,
                        other.to_string(),
                    ),
                };
                self.on_provider_failure(failure);
            }
        }
    }

    fn on_event(&mut self, event: AgentEvent) {
        match event.kind {
            AgentEventKind::Initialized(info) => self.capabilities = Some(info.capabilities),
            AgentEventKind::SessionReady { .. } => {}
            AgentEventKind::Options(options) => {
                self.options = options;
                self.touch();
            }
            AgentEventKind::MessageDelta { role, text } => self.append_message(role, &text),
            AgentEventKind::ToolCall(tool) | AgentEventKind::ToolUpdate(tool) => {
                self.upsert_tool(tool)
            }
            AgentEventKind::PermissionRequested(prompt) => self.on_permission(prompt),
            AgentEventKind::PermissionClosed { id, resolution } => {
                self.on_permission_closed(id, resolution)
            }
            AgentEventKind::PromptFinished(outcome) => self.on_turn_finished(outcome),
            AgentEventKind::OptionRejected {
                option, message, ..
            } => {
                self.warn(format!(
                    "The agent refused the option change `{option}`: {message}"
                ));
            }
            AgentEventKind::Failure(failure) => self.on_provider_failure(failure),
            AgentEventKind::Closed(_) => {}
        }
    }

    fn engine_transition(&mut self, next: TaskState) {
        let Some(active) = &self.active else {
            return;
        };
        let identity = active.ctx.identity.clone();
        let result = self
            .shared
            .controller
            .lock()
            .agent_task_transition(&identity, next);
        if let Err(error) = result {
            self.warn(format!("Task state could not move to {next:?}: {error}"));
        }
    }

    fn on_permission(&mut self, prompt: PermissionPrompt) {
        let Some(active) = &mut self.active else {
            return;
        };
        let writer = active.writer.map_or(0, WriterGeneration::value);
        let reference = PermissionRef {
            task: active.ctx.identity.task.clone(),
            writer,
            request: prompt.id.0,
        };
        active.open_permissions.insert(prompt.id.0);
        let need_wait = !active.engine_waiting;
        active.engine_waiting = true;
        let offered: Vec<String> = prompt.options.iter().map(|o| o.option_id.clone()).collect();
        self.shared
            .registry
            .lock()
            .open
            .insert(reference.clone(), offered);
        let row = self.push_row(RowKind::Permission(PermissionCard {
            reference,
            turn: prompt.turn,
            tool_call_id: prompt.tool_call_id,
            title: present::message(&prompt.title),
            options: prompt.options,
            state: PermissionState::Open,
        }));
        self.permission_rows.insert((writer, prompt.id.0), row);
        if need_wait {
            self.engine_transition(TaskState::Waiting);
        }
        self.set_phase(TaskPhase::WaitingPermission);
    }

    fn on_permission_closed(&mut self, id: PermissionId, resolution: PermissionResolution) {
        let Some(active) = &mut self.active else {
            return;
        };
        let writer = active.writer.map_or(0, WriterGeneration::value);
        let reference = PermissionRef {
            task: active.ctx.identity.task.clone(),
            writer,
            request: id.0,
        };
        active.open_permissions.remove(&id.0);
        let none_left = active.open_permissions.is_empty();
        let in_repair = active.in_repair;
        let was_waiting = active.engine_waiting;
        {
            let mut registry = self.shared.registry.lock();
            // A close the user did not answer (Stop, turn end, provider loss) is stale now.
            registry.open.remove(&reference);
        }
        let state = match resolution {
            PermissionResolution::Selected { option_id } => PermissionState::Selected { option_id },
            PermissionResolution::Cancelled => PermissionState::Cancelled,
        };
        // The row may have left the transcript window: it is updated wherever it lives,
        // and the open-request index never outgrows what is actually open.
        if let Some(row) = self.permission_rows.remove(&(writer, id.0)) {
            self.update_row(row, |kind| {
                if let RowKind::Permission(card) = kind {
                    card.state = state;
                }
            });
        }
        if none_left && was_waiting {
            if let Some(active) = &mut self.active {
                active.engine_waiting = false;
            }
            self.engine_transition(TaskState::Editing);
            self.set_phase(if in_repair {
                TaskPhase::Repairing
            } else {
                TaskPhase::Editing
            });
        }
    }

    fn reply_permission(&mut self, reference: PermissionRef, answer: PermissionAnswer) {
        let Some(active) = &self.active else {
            self.warn("That permission request is no longer current.");
            return;
        };
        let current = active.writer.map_or(0, WriterGeneration::value);
        if active.ctx.identity.task != reference.task || current != reference.writer {
            self.warn(
                "That permission request belongs to an earlier agent session; it was ignored.",
            );
            return;
        }
        let Some(driver) = &active.driver else {
            return;
        };
        let reply = match answer {
            PermissionAnswer::Select(option) => PermissionReply::Select(option),
            PermissionAnswer::Cancel => PermissionReply::Cancel,
        };
        if let Err(error) = driver.reply_permission(PermissionId(reference.request), reply) {
            self.warn(format!("The permission reply was not delivered: {error}"));
        }
    }

    fn driver_call(
        &mut self,
        call: impl FnOnce(&AcpDriver) -> Result<(), studio_agent_spike::driver::DriverError>,
    ) {
        let result = match self.active.as_ref().and_then(|a| a.driver.as_ref()) {
            Some(driver) => call(driver),
            None => {
                self.warn("There is no live agent session to change.");
                return;
            }
        };
        if let Err(error) = result {
            self.warn(format!("The option change was refused: {error}"));
        }
    }

    fn on_turn_finished(&mut self, outcome: PromptOutcome) {
        self.finish_streaming();
        let Some(active) = &mut self.active else {
            return;
        };
        active.turn = None;
        if !outcome.eligible_for_quiescence() {
            let reason = if outcome.cancel_requested {
                "was cancelled".to_owned()
            } else if outcome.unresolved_permissions > 0 {
                format!(
                    "ended with {} permission request(s) unanswered",
                    outcome.unresolved_permissions
                )
            } else {
                format!("ended with `{}`", outcome.stop_reason.as_str())
            };
            let error = present::simple_error(
                "turn_not_completed",
                "The agent did not finish its edit",
                &format!("The agent's turn {reason}; nothing was captured."),
                Some("Send the brief again, perhaps more specifically; the working copy was kept."),
                true,
            );
            self.fail_with(error);
            return;
        }
        let in_repair = active.in_repair;
        let draft = active.ctx.draft.clone();
        let base = active.ctx.source_base.revision().clone();
        let unchanged = !in_repair
            && SourceInventory::scan(&draft).is_ok_and(|inventory| inventory.revision == base);
        if unchanged {
            // The agent ended its turn without touching the draft: a question (ACP v1
            // has no question RPC). The answer is a subsequent prompt in the same session.
            let identity = active.ctx.identity.clone();
            let need_wait = !active.engine_waiting;
            active.engine_waiting = true;
            if need_wait {
                self.engine_transition(TaskState::Waiting);
            }
            self.shared.registry.lock().clarification = Some(identity.task);
            self.set_phase(TaskPhase::WaitingClarification);
            self.notice(
                "The agent ended its turn without changing any file. Answer it, or press Stop.",
            );
            return;
        }
        self.start_pipeline();
    }

    fn clarify(&mut self, task: AgentTaskId, text: String) {
        let Some(active) = &mut self.active else {
            self.warn("No task is waiting for an answer.");
            return;
        };
        if active.ctx.identity.task != task || active.phase != TaskPhase::WaitingClarification {
            self.warn("No task is waiting for an answer.");
            return;
        }
        active.engine_waiting = false;
        self.engine_transition(TaskState::Editing);
        self.push_row(RowKind::User {
            text: present::bounded(&text, MAX_ROW_TEXT_BYTES),
            source: UserSource::Clarification,
        });
        self.send_prompt(&text, Vec::new());
    }

    // ---- provider failure and stop ----------------------------------------------------------------------

    fn on_provider_failure(&mut self, failure: AgentFailure) {
        let error = present::provider_error(&failure, true);
        self.fail_with(error);
    }

    /// Ends the task `Failed`: the adapter is reaped, the draft retained, no writer left.
    fn fail_with(&mut self, error: StructuredError) {
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

    fn stop(&mut self) {
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
            | TaskPhase::WaitingClarification => {
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

    // ---- the pipeline -------------------------------------------------------------------------------------

    fn start_pipeline(&mut self) {
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

    fn on_progress(&mut self, phase: TaskPhase) {
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

    fn show_validation(&mut self, changes: ChangeCard, card: ValidationCard) {
        self.push_row(RowKind::Changes(changes.clone()));
        self.push_row(RowKind::Validation(Box::new(card.clone())));
        if let Some(task) = &mut self.task {
            task.changes = Some(changes);
            task.validation = Some(card);
        }
    }

    fn on_pipeline(&mut self, end: PipelineEnd) {
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

    fn cancel_or_interrupt(&mut self) {
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

    fn await_review(&mut self, blocked: Option<String>) {
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

    fn await_auto_apply_evidence(&mut self) {
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

    fn begin_repair(&mut self, context: FailureContext) {
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
            Some(build) => {
                self.offer_tools(build, &active.adapter, tools_settings.as_ref(), &active.ctx)
            }
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
                max: studio_engine::MAX_AUTOMATIC_REPAIRS,
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

    fn apply(&mut self) {
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

    fn start_promote(&mut self) {
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

    fn on_promote(&mut self, end: PromoteEnd) {
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
    fn on_committed(&mut self, published: &str, kind: HandoffKind) {
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
    fn on_job_aborted(&mut self, kind: JobKind, message: &str) {
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
            JobKind::Undo => {
                if let Some(run) = self.undo.take() {
                    run.scopes.0.shutdown(Duration::ZERO);
                }
                self.error_row(error);
                self.undo_settled();
            }
        }
    }

    fn record_promotion(
        &mut self,
        promotion: &studio_engine::Promotion,
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
    fn handoff_resolved(&mut self, published: &str, result: Result<(), String>) {
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

    fn preview_displayed(&mut self, revision: &str) {
        if self.handoff_published.as_deref() == Some(revision)
            && let Some(handoff) = &mut self.handoff
        {
            handoff.state = HandoffState::Displayed;
            self.handoff_acked = true;
            self.touch();
        }
    }

    fn conflict_view(
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
        } else if let EngineError::Promotion(PromotionError::Plan(
            studio_engine::PlanError::Conflict { path, .. },
        )) = error
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

    fn discard(&mut self) {
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

    fn export_candidate(&mut self, dest: &Path) {
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

    fn export_draft(&mut self, dest: &Path) {
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

    // ---- Undo / policy / recovery ------------------------------------------------------------------------------

    fn start_undo(&mut self, target: Option<String>) {
        if self.active.is_some() || self.undo.is_some() {
            self.warn("Undo needs the agent to be idle.");
            return;
        }
        let Some(env) = self.env() else {
            self.error_row(present::simple_error(
                "sdk_unavailable",
                "No compatible SDK is installed",
                "Undo validates the restored source against the SDK; none is selected.",
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
            phase: "Preparing the Undo".into(),
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
            (env, target, scopes, gate, sink),
            move |(env, target, scopes, gate, sink)| {
                let end = run_undo(
                    env,
                    target,
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
        if let Err((message, (_, _, scopes, _, _))) = refused {
            // Nothing started, so nothing can be waiting for a result.
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

    fn on_undone(&mut self, end: UndoEnd) {
        self.set_inflight(None);
        self.undo = None;
        match end {
            UndoEnd::Committed {
                promotion,
                handoff,
                scopes,
            } => {
                scopes.compiler.shutdown(Duration::ZERO);
                self.record_promotion(&promotion, &handoff, HandoffKind::Undo);
            }
            UndoEnd::Cancelled => self.notice("Undo was stopped; nothing was changed."),
            UndoEnd::ValidationFailed(card) => {
                let summary = card.summary.clone();
                self.push_row(RowKind::Validation(card));
                self.error_row(present::simple_error(
                    "undo_validation_failed",
                    "The Undo did not validate",
                    &summary,
                    Some("Nothing was changed."),
                    false,
                ));
            }
            UndoEnd::Failed(error) => self.error_row(present::engine_error(&error, false)),
        }
        self.undo_settled();
    }

    /// An Undo ended in any way: the registry, the views and the queue move on.
    fn undo_settled(&mut self) {
        self.sync_registry(0);
        self.refresh_idle();
        self.start_next();
    }

    fn set_policy(&mut self, policy: ReviewPolicy) {
        let result = self.shared.controller.lock().set_review_policy(policy);
        match result {
            Ok(()) => {
                self.policy = policy;
                if self.active.is_some() {
                    self.notice("The review setting changed; it applies to the next task, not the one running.");
                }
                self.touch();
            }
            Err(error) => self.error_row(present::engine_error(&error, false)),
        }
    }

    fn acknowledge_writer_gone(&mut self) {
        if self.active.is_some() {
            self.warn("Wait for the running task to end first.");
            return;
        }
        let result = self
            .shared
            .controller
            .lock()
            .acknowledge_agent_writer_gone(&WriterGoneEvidence::UserConfirmed);
        match result {
            Ok(()) => {
                self.interrupted_task = None;
                self.notice("The working copy was unlocked; it is archived before the next task refreshes it.");
            }
            Err(error) => self.error_row(present::engine_error(&error, true)),
        }
        self.refresh_idle();
    }

    fn resolve_conflict(&mut self, transaction: &str, note: &str) {
        let result = self
            .shared
            .controller
            .lock()
            .resolve_conflict(transaction, note);
        match result {
            Ok(()) => self.notice("The conflict was marked resolved; every variant stays on disk."),
            Err(error) => self.error_row(present::engine_error(&error, false)),
        }
        self.refresh_idle();
    }

    // ---- finishing -------------------------------------------------------------------------------------------------

    /// Ends the active task: reaps the adapter and every owned consumer, records the
    /// terminal state in the engine (when `finish` names one; `None` means the engine
    /// already ended it), and publishes the outcome.
    fn finalize(
        &mut self,
        finish: Option<(TaskState, String)>,
        phase: TaskPhase,
        kind: OutcomeKind,
        error: Option<StructuredError>,
    ) {
        let Some(mut active) = self.active.take() else {
            return;
        };
        self.driver_epoch += 1;
        let identity = active.ctx.identity.clone();
        // Closing a driver closes its open permissions as cancelled.
        if let Some(driver) = active.driver.take() {
            let _ = driver.cancel_prompt();
            if let Some(turn) = active.turn {
                // Give the adapter the chance to acknowledge the cancel (its permission
                // requests are already closed as cancelled); the driver kills the tree
                // itself if it does not answer within its cancel-ack deadline.
                let _ = driver.wait_turn(turn, Duration::from_secs(5));
            }
            let leftovers = driver.drain_events(256);
            for event in leftovers {
                if let AgentEventKind::MessageDelta { role, text } = event.kind {
                    self.append_message(role, &text);
                }
            }
            // What the driver saw of the writer (an escaped descendant, an unproven
            // ownership) reaches the engine before the task ends: it cannot be recovered
            // once the adapter process is gone, and the draft is classified from it.
            let outcome = driver.shutdown();
            if let Err(error) = observe_writer(&self.shared.controller, &identity, &outcome) {
                self.warn(format!(
                    "The agent's final process observation could not be recorded ({error}); the working copy stays locked until you confirm that nothing writes to it."
                ));
            }
        }
        if let Some((compiler, worker)) = active.pipeline_scopes.take() {
            compiler.shutdown(Duration::ZERO);
            worker.shutdown(Duration::ZERO);
        }
        if let Some(retained) = active.retained.take() {
            retained.scopes.compiler.shutdown(Duration::ZERO);
            retained.scopes.worker.shutdown(Duration::ZERO);
            drop(retained);
        }
        let mut engine_error = None;
        if let Some((state, reason)) = &finish {
            let result = self
                .shared
                .controller
                .lock()
                .finish_agent_task(&identity, *state, reason);
            if let Err(error) = result {
                let ended = self
                    .shared
                    .controller
                    .lock()
                    .agent_task()
                    .is_some_and(|t| t.state().is_terminal());
                if !ended {
                    engine_error = Some(present::engine_error(&error, true));
                }
            }
        }
        active.evidence_cancel.store(true, Ordering::Release);
        if let Some(tools) = &self.tools {
            tools.end_task(&identity);
        }
        // The task's own app-owned artifacts are gone with it; the view keeps their
        // ids and hashes as a record, marked released.
        if let Some(task) = &mut self.task {
            for view in [&mut task.before, &mut task.after].into_iter().flatten() {
                if matches!(view.state, EvidenceState::Ready | EvidenceState::Pending) {
                    view.state = EvidenceState::Released;
                }
            }
        }
        self.shared.live.set(None);
        // Every card still open is stale now.
        {
            let mut registry = self.shared.registry.lock();
            registry.open.retain(|r, _| r.task != identity.task);
            registry.clarification = None;
        }
        // Cards still open, wherever they live (the transcript window is not the index).
        let open_cards: Vec<RowId> = self.permission_rows.drain().map(|(_, row)| row).collect();
        for id in open_cards {
            self.update_row(id, |kind| {
                if let RowKind::Permission(card) = kind
                    && card.state == PermissionState::Open
                {
                    card.state = PermissionState::Cancelled;
                }
            });
        }
        self.set_inflight(None);
        self.finish_streaming();
        let draft_state = self
            .shared
            .controller
            .lock()
            .agent_draft_state()
            .ok()
            .flatten();
        let retained_draft = !matches!(draft_state, Some(DraftState::Accepted { .. }));
        // The task row stays visible until the next task replaces it. Rows pushed below
        // belong to it, so keep the id attached while pushing.
        let reason = finish
            .as_ref()
            .map(|(_, r)| r.clone())
            .or_else(|| error.as_ref().map(|e| e.title.clone()));
        if let Some(task) = &mut self.task {
            task.phase = phase;
            task.engine_state = engine_state_of(phase);
            task.reason = reason.clone();
            task.ended_unix = Some(now_unix());
            task.draft_state = draft_state;
            task.review = None;
            task.repair.in_progress = false;
            if error.is_some() {
                task.error = error.clone();
            }
        }
        if let Some(error) = &error {
            self.push_row(RowKind::Error(error.clone()));
        }
        if let Some(error) = engine_error {
            self.push_row(RowKind::Error(error));
        }
        let summary = match kind {
            OutcomeKind::Accepted => "The edit was applied.".to_owned(),
            OutcomeKind::Cancelled => "Stopped. The working copy was kept.".to_owned(),
            OutcomeKind::Discarded => {
                "The candidate was discarded; the working copy was kept.".to_owned()
            }
            OutcomeKind::Interrupted => {
                "The task was interrupted; the working copy was kept.".to_owned()
            }
            OutcomeKind::Conflict => {
                "The project changed while the task ran; the candidate and working copy were kept."
                    .to_owned()
            }
            _ => "The task ended without applying an edit; the working copy was kept.".to_owned(),
        };
        if kind != OutcomeKind::Accepted {
            self.push_row(RowKind::Outcome(OutcomeCard {
                kind,
                summary,
                published: None,
                draft_retained: retained_draft,
            }));
        }
        self.refresh_idle();
        self.touch();
        self.sync_registry(0);
        self.publish(true);
        self.start_next();
    }

    // ---- close ---------------------------------------------------------------------------------------------------------

    fn shutdown_all(&mut self, done: Option<Sender<()>>) {
        self.closing = true;
        self.deferred.clear();
        let cleared = self.queue.len();
        if cleared > 0 {
            self.shared.registry.lock().pending -= cleared;
            self.queue.clear();
        }
        // Cancel what can be cancelled, then let every job finish: a publication that has
        // begun is short and completes (its result is settled below).
        if let Some(undo) = &self.undo {
            undo.gate.cancel();
            undo.scopes.0.shutdown(Duration::ZERO);
            undo.scopes.1.shutdown(Duration::ZERO);
        }
        if let Some(active) = &mut self.active {
            active.gate.cancel();
            active.evidence_cancel.store(true, Ordering::Release);
            if let Some((compiler, worker)) = &active.pipeline_scopes {
                compiler.shutdown(Duration::ZERO);
                worker.shutdown(Duration::ZERO);
            }
        }
        for job in std::mem::take(&mut self.jobs) {
            let _ = job.join();
        }
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Msg::Pipeline(end) => self.on_pipeline(end),
                Msg::Promote(end) => self.on_promote(end),
                Msg::Undone(end) => self.on_undone(end),
                Msg::Committed { published, kind } => self.on_committed(&published, kind),
                Msg::JobAborted { kind, message } => self.on_job_aborted(kind, &message),
                Msg::Evidence { task, kind, view } => self.on_evidence(task, kind, view),
                Msg::Probe(_) => self.probing = false,
                Msg::Close(other) => {
                    let _ = other.send(());
                }
                _ => {}
            }
        }
        if self.active.is_some() {
            self.finalize(
                Some((TaskState::Interrupted, "project closed".into())),
                TaskPhase::Interrupted,
                OutcomeKind::Interrupted,
                None,
            );
        }
        if let Some(tools) = self.tools.take() {
            tools.shutdown();
        }
        if let Some(build) = self.shared.settings.lock().build.clone() {
            build
                .service
                .close_project(String::from(self.shared.project.clone()).as_str());
        }
        self.shared.live.set(None);
        {
            let mut registry = self.shared.registry.lock();
            registry.open.clear();
            registry.clarification = None;
        }
        self.dirty = true;
        self.publish(true);
        if let Some(done) = done {
            let _ = done.send(());
        }
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
