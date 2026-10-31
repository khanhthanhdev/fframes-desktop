//! Native agent workflow: the production task orchestrator for one open project.
//!
//! One [`AgentWorkflow`] owns, per open project, everything an agent task needs and
//! nothing a UI toolkit: adapter discovery/readiness (an explicit probe, never on open),
//! the engine task lifecycle (`begin_agent_task`, writer start/observe, quiescence,
//! capture), the ACP driver in the stable draft cwd with the task MCP server
//! (`ProjectToolBackend` + `ToolBroker` + `WriterGate`), prompt/queued follow-ups/Stop,
//! permission and clarification replies, validation through the shared build service,
//! exactly one automatic repair turn, the review policy (auto Apply or manual review),
//! Undo, interrupted-task recovery surfaced from the engine on open, and the guarded
//! post-commit preview handoff.
//!
//! # Threads and ownership
//!
//! * **Actor thread** (`studio-workflow`): serializes every command, pumps the live
//!   driver (~20 ms tick), owns the state machine, and publishes immutable
//!   [`WorkflowSnapshot`]s (batched, at most every 25 ms unless a state changes).
//! * **Job threads**: adapter probe, the quiesce -> capture -> validate pipeline,
//!   publication and Undo. Stop reaches them out of band (task [`jobs::Gate`] + process
//!   scopes) so it is actionable in every state; publication has a short uncancellable
//!   boundary after which Stop only reports and requests recovery.
//! * The caller owns the [`Controller`] inside an `Arc<Mutex<_>>`; the workflow takes the
//!   lock only for short engine calls and for capture/publication on job threads.
//!   No UI thread ever blocks on filesystem, RPC, hashing, compile or inspection work.
//!
//! GPUI never sees this module's internals: it polls [`AgentWorkflow::snapshot`] (or is
//! woken by the optional `notify` callback) and sends commands.
mod actor;
pub mod approvals;
mod attachments;
mod auto_answer;
mod composer;
mod composer_ops;
pub mod evidence_preview;
mod jobs;
pub mod log;
pub mod model;
mod present;
pub mod probe_cache;
pub mod session_store;
mod tools;

pub use approvals::ApprovalRule;
pub use attachments::{
    AttachmentRequest, FileIdentity, PreparedAttachment, StageToken, StagedAttachment,
};
pub use composer::{ComposerView, OpenQuestion, SteerSupport};
pub use jobs::{JobFault, JobFaults, PreviewHandoff, PromotionHandoff};
pub use model::*;
pub use studio_harness::driver::{
    CommandInfo, PlanEntry, PlanStatus, QuestionAnswer, QuestionItem, QuestionOption,
};
pub use studio_proto::PermissionPolicy;
pub use tools::BuildSettings;

use crate::{
    AgentTaskId, Controller, ReviewPolicy, TaskScope, agent_task::validate_brief,
    app_paths::AppPaths,
};
use actor::{Actor, Msg};
use fframes_studio_protocol::PreviewIdentity;
use image::RgbaImage;
use jobs::Gate;
use log::{LogError, RowLimits, RowStore};
use parking_lot::{Condvar, Mutex};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc::{Sender, channel},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use studio_bootstrap::ProcessTreeManager;
use studio_harness::{
    AdapterConfig, AgentWire, DriverLimits, ExecutableSearch, McpStdioSupport, OwnershipSource,
    driver::OptionValue,
};
use studio_project::ProjectId;

/// The one adapter a project talks to (provider-owned setup; values of auth variables are
/// never stored here, only their names).
#[derive(Clone)]
pub struct AdapterSettings {
    /// Label carried by driver events and rows.
    pub provider: String,
    pub adapter: AdapterConfig,
    /// Where the writer's ownership comes from at each launch: the containment the
    /// supervisor verified for the spawned adapter (production), or a test injection.
    /// Anything but a contained ownership blocks candidate capture.
    pub ownership: OwnershipSource,
    /// Host policy for stdio MCP (`Unsupported` = CLI route only).
    pub mcp: McpStdioSupport,
    /// Advertised authentication method to select explicitly, if the adapter needs one.
    pub auth_method: Option<String>,
    /// The protocol the adapter speaks.
    pub wire: AgentWire,
}

/// Where the task MCP route lives.
#[derive(Clone)]
pub struct ToolSettings {
    /// Private runtime directory for the broker socket and capability files. Unix socket
    /// paths are short (~100 bytes): keep it near the root (see [`short_runtime_dir`]).
    pub runtime_dir: PathBuf,
    /// The `studio-mcp` helper (see `agent_tools::sibling_binary`). Without it (or when
    /// host policy disables stdio MCP) no MCP server is offered.
    pub studio_mcp: Option<PathBuf>,
    /// The `studio-tools` command-line helper. When set, every task whose tools start is
    /// told the exact command line (the same task capability as MCP); without it and
    /// without MCP the agent gets no project tools and the snapshot says so.
    pub studio_tools: Option<PathBuf>,
}

/// Looks up the value of an adapter auth variable by name.
pub type AuthEnv = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// A short, per-process runtime directory path for broker sockets (not created).
pub fn short_runtime_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.as_os_str().len() < 48)
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!(
        "fft-{}-{}",
        std::process::id(),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    ))
}

pub struct WorkflowConfig {
    pub paths: AppPaths,
    pub adapter: Option<AdapterSettings>,
    /// Explicit search roots; the global `PATH` is never consulted implicitly.
    pub search: ExecutableSearch,
    /// Looks up the values of the adapter's configured auth variables (default: process
    /// environment). Values become redaction secrets.
    pub auth_env: AuthEnv,
    /// `None` until an SDK is installed; submitting then reports `SdkUnavailable`.
    pub build: Option<BuildSettings>,
    pub tools: Option<ToolSettings>,
    pub limits: DriverLimits,
    pub probe_timeout: Duration,
    /// Receives the staged preview + promotion after a commit (see [`PreviewHandoff`]).
    pub handoff: Option<Arc<dyn PreviewHandoff>>,
    /// Called (on the actor thread) after every published snapshot; keep it cheap.
    pub notify: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Driver polling interval.
    pub tick: Duration,
    pub row_limits: RowLimits,
    /// Failure injection for background jobs (tests only).
    #[doc(hidden)]
    pub job_faults: Option<JobFaults>,
    pub ownership_policy: Option<crate::containment::OwnershipPolicy>,
}

impl WorkflowConfig {
    pub fn new(paths: AppPaths) -> Self {
        Self {
            paths,
            adapter: None,
            search: ExecutableSearch::default(),
            auth_env: Arc::new(|name| std::env::var(name).ok()),
            build: None,
            tools: None,
            limits: DriverLimits::default(),
            probe_timeout: Duration::from_secs(30),
            handoff: None,
            notify: None,
            tick: Duration::from_millis(20),
            row_limits: RowLimits::default(),
            job_faults: None,
            ownership_policy: None,
        }
    }
}

/// A reply to one permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionAnswer {
    /// One of the offered option ids.
    Select(String),
    Cancel,
}

/// Why a command was refused before it reached the actor.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkflowError {
    #[error("the agent workflow is closed")]
    Closed,
    #[error("no agent is configured")]
    NotConfigured,
    #[error("no compatible SDK is installed")]
    SdkUnavailable,
    #[error("{0}")]
    InvalidBrief(String),
    #[error("invalid task scope: {0}")]
    InvalidScope(String),
    #[error("{} briefs are already queued", MAX_QUEUED_BRIEFS)]
    QueueFull,
    #[error("that permission request is not current (stale or from another agent session)")]
    StalePermission,
    #[error("that permission request was already answered")]
    DuplicatePermission,
    #[error("`{0}` is not an option the permission request offered")]
    UnknownChoice(String),
    #[error("no task is waiting for an answer")]
    NotWaiting,
    #[error("the agent did not advertise `{0}`")]
    UnknownOption(String),
    #[error("{0}")]
    Busy(&'static str),
    #[error("{0}")]
    History(String),
    #[error("there is no candidate waiting for review")]
    NoCandidate,
    /// The files of a candidate or revision could not be read, or the request names none.
    #[error("{0}")]
    Review(String),
    /// The composer asked for something the current agent or state cannot do; the text
    /// says why and is meant to be shown.
    #[error("{0}")]
    Refused(String),
}

#[derive(Default)]
pub(crate) struct Registry {
    /// Permission requests that can still be answered, with the option ids each offered.
    pub open: HashMap<PermissionRef, Vec<String>>,
    answered: VecDeque<PermissionRef>,
    /// The task whose clarification is open.
    pub clarification: Option<AgentTaskId>,
    /// Briefs accepted but not yet started (in the channel or the queue).
    pub pending: usize,
    /// Operations the actor is running: a task and/or an Undo.
    pub running: usize,
    /// The composer state as last published: what the synchronous checks of the
    /// composer operations consult.
    pub composer: ComposerView,
}

impl Registry {
    /// Authoritative, not snapshot-derived: submissions in flight count as busy.
    pub(crate) fn busy(&self) -> bool {
        self.pending > 0 || self.running > 0
    }
}

/// Why the conversation cannot be replaced, by what blocks it: a candidate under review, an
/// Undo, queued briefs, a running task.
const BUSY_NEW_SESSION: [&str; 4] = [
    "Apply or Discard the candidate before starting a new chat.",
    "Wait for the Undo to finish before starting a new chat.",
    "Cancel or clear the queued briefs before starting a new chat.",
    "Stop the running task or wait for it to finish before starting a new chat.",
];
const BUSY_OPEN_SESSION: [&str; 4] = [
    "Apply or Discard the candidate before opening another session.",
    "Wait for the Undo to finish before opening another session.",
    "Cancel or clear the queued briefs before opening another session.",
    "Stop the running task or wait for it to finish before opening another session.",
];

pub(crate) struct Settings {
    pub adapter: Option<AdapterSettings>,
    pub build: Option<BuildSettings>,
    pub tools: Option<ToolSettings>,
}

/// Presentation-only thumbnails for the single task currently shown in the workflow.
/// They intentionally stay outside `WorkflowSnapshot`, which is serialized and logged.
#[derive(Default)]
struct EvidenceImageCache {
    task: Option<AgentTaskId>,
    encoded_bytes: usize,
    images: HashMap<String, Option<Arc<RgbaImage>>>,
}

impl EvidenceImageCache {
    fn begin_task(&mut self, task: AgentTaskId) {
        self.task = Some(task);
        self.encoded_bytes = 0;
        self.images.clear();
    }

    /// Reserves an entry and returns its read budget. Failed/expired artifacts remain
    /// reserved so repeated snapshots cannot retry them without bound.
    fn reserve(&mut self, task: &AgentTaskId, id: &str, bytes: u64) -> Option<usize> {
        const MAX_IMAGES: usize = 6;
        const MAX_ENCODED_BYTES: usize = crate::agent::evidence_preview::MAX_ENCODED_BYTES;
        let bytes = usize::try_from(bytes).ok()?;
        if self.task.as_ref() != Some(task)
            || self.images.contains_key(id)
            || self.images.len() >= MAX_IMAGES
            || bytes == 0
            || bytes > MAX_ENCODED_BYTES.saturating_sub(self.encoded_bytes)
        {
            return None;
        }
        self.encoded_bytes += bytes;
        self.images.insert(id.to_owned(), None);
        Some(bytes)
    }

    fn complete(&mut self, task: &AgentTaskId, id: &str, image: Arc<RgbaImage>) -> bool {
        if self.task.as_ref() != Some(task) {
            return false;
        }
        let Some(slot) = self.images.get_mut(id) else {
            return false;
        };
        *slot = Some(image);
        true
    }

    fn image(&self, task: &AgentTaskId, id: &str) -> Option<Arc<RgbaImage>> {
        (self.task.as_ref() == Some(task))
            .then(|| self.images.get(id).and_then(Clone::clone))
            .flatten()
    }
}

pub(crate) struct Shared {
    pub project: ProjectId,
    pub controller: Arc<Mutex<Controller>>,
    /// The project's process scope (clone; reading it never takes the controller lock).
    pub root_scope: ProcessTreeManager,
    pub paths: AppPaths,
    pub search: ExecutableSearch,
    pub auth_env: AuthEnv,
    pub limits: DriverLimits,
    pub probe_timeout: Duration,
    pub handoff: Option<Arc<dyn PreviewHandoff>>,
    pub notify: Option<Arc<dyn Fn() + Send + Sync>>,
    pub tick: Duration,
    pub job_faults: Option<JobFaults>,
    pub settings: Mutex<Settings>,
    pub snapshot: Mutex<Arc<WorkflowSnapshot>>,
    /// The project's saved "Always in this project" rules, as in its approvals file; the
    /// lock also orders the file's writes. A task freezes a copy when it starts.
    pub approvals: Mutex<Vec<ApprovalRule>>,
    /// Bounded UI-only image handles; never serialized or added to snapshots.
    evidence_images: Mutex<EvidenceImageCache>,
    pub changed: Condvar,
    pub registry: Mutex<Registry>,
    /// Outside files waiting for the user's confirmation (see `attachments::stage`).
    pub staged: attachments::StageStore,
    /// Attachment checks running on their own threads.
    pub preparing: AtomicUsize,
    /// The cancellation gate of the job the actor is running (pipeline, publication or
    /// Undo), so `stop()` can cancel it without waiting for the actor.
    pub inflight: Mutex<Option<Arc<Gate>>>,
    pub live: Arc<tools::LiveTasks>,
    pub playhead: AtomicUsize,
    pub next_submission: AtomicU64,
    pub tx: Sender<Msg>,
    pub ownership_policy: crate::containment::OwnershipPolicy,
}

/// The per-project agent workflow. Dropping it closes it.
pub struct AgentWorkflow {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl AgentWorkflow {
    /// Starts the actor for the project `controller` owns. Creates the project's first stored
    /// session when it has none, reads the current session's conversation log and the engine's
    /// recovery state; launches nothing (no adapter probe, no agent).
    ///
    /// This replays the conversation log and refreshes the engine's recovery view before
    /// it returns, so it BLOCKS the caller for that long (and for the controller lock):
    /// call it from a background thread, never from a UI event or render path. Use
    /// [`Self::open_detached`] to do that without writing the thread yourself.
    pub fn open(
        config: WorkflowConfig,
        controller: Arc<Mutex<Controller>>,
    ) -> Result<Self, WorkflowError> {
        let (project, root_scope) = {
            let c = controller.lock();
            (c.project.manifest.project_id.clone(), c.processes.clone())
        };
        // The first session exists before the log opens.
        let startup = session_store::SessionStore::prepare(&config.paths, &project)
            .map_err(WorkflowError::History)?;
        let store = RowStore::open_with_floor(startup.log, config.row_limits, startup.row_floor)
            .map_err(|e: LogError| WorkflowError::History(e.to_string()))?;
        let approvals = approvals::load(&config.paths, &project);
        let (tx, rx) = channel();
        let empty = Arc::new(WorkflowSnapshot {
            revision: 0,
            project: project.clone(),
            adapter: AdapterView {
                provider: None,
                executable: None,
                readiness: AdapterReadiness::NotConfigured,
                last_launch: None,
            },
            capabilities: None,
            options: Default::default(),
            mcp: McpView {
                policy_enabled: false,
                binary_available: false,
                active: false,
                cli_active: false,
                capability_file: None,
                note: None,
            },
            task: None,
            queue: Vec::new(),
            retained_queue: Vec::new(),
            rows: Vec::new(),
            composer: Default::default(),
            older_rows: false,
            chat: 0,
            review_policy: ReviewPolicy::default(),
            undo: UndoView::Unavailable {
                reason: "starting".into(),
            },
            recovery: RecoveryView::default(),
            handoff: None,
            history: Vec::new(),
            resources: ResourceView::default(),
            closed: false,
            switch_pending: None,
            session_restore: None,
        });
        let shared = Arc::new(Shared {
            project,
            controller,
            root_scope,
            paths: config.paths,
            search: config.search,
            auth_env: config.auth_env,
            limits: config.limits,
            probe_timeout: config.probe_timeout,
            handoff: config.handoff,
            notify: config.notify,
            tick: config.tick,
            job_faults: config.job_faults,
            settings: Mutex::new(Settings {
                adapter: config.adapter,
                build: config.build,
                tools: config.tools,
            }),
            ownership_policy: config
                .ownership_policy
                .unwrap_or(crate::containment::OwnershipPolicy::Validated),
            snapshot: Mutex::new(empty),
            approvals: Mutex::new(approvals),
            evidence_images: Mutex::new(EvidenceImageCache::default()),
            changed: Condvar::new(),
            registry: Mutex::new(Registry::default()),
            staged: attachments::StageStore::default(),
            preparing: AtomicUsize::new(0),
            inflight: Mutex::new(None),
            live: Arc::new(tools::LiveTasks::default()),
            playhead: AtomicUsize::new(0),
            next_submission: AtomicU64::new(1),
            tx,
        });
        // The actor publishes revision 1 before this returns.
        let actor = Actor::new(shared.clone(), rx, store);
        let thread = std::thread::Builder::new()
            .name("studio-workflow".into())
            .spawn(move || actor.run())
            .map_err(|e| WorkflowError::History(e.to_string()))?;
        Ok(Self {
            shared,
            thread: Mutex::new(Some(thread)),
        })
    }

    // ---- observation -----------------------------------------------------------------------------

    /// The newest immutable snapshot.
    pub fn snapshot(&self) -> Arc<WorkflowSnapshot> {
        self.shared.snapshot.lock().clone()
    }

    /// A decoded RGBA thumbnail for a task-owned evidence artifact, if its bounded decode
    /// has completed. PNG bytes and app-private paths never leave the workflow worker.
    pub fn evidence_image(&self, task: &AgentTaskId, artifact_id: &str) -> Option<Arc<RgbaImage>> {
        self.shared.evidence_images.lock().image(task, artifact_id)
    }

    /// Blocks until a snapshot newer than `since` exists (or `timeout`), returning the
    /// newest one.
    pub fn wait_changed(&self, since: u64, timeout: Duration) -> Arc<WorkflowSnapshot> {
        let deadline = Instant::now() + timeout;
        let mut guard = self.shared.snapshot.lock();
        while guard.revision <= since && !guard.closed {
            if self
                .shared
                .changed
                .wait_until(&mut guard, deadline)
                .timed_out()
            {
                break;
            }
        }
        guard.clone()
    }

    /// Waits until `predicate` holds for a snapshot; `None` on timeout.
    pub fn wait_for(
        &self,
        timeout: Duration,
        predicate: impl Fn(&WorkflowSnapshot) -> bool,
    ) -> Option<Arc<WorkflowSnapshot>> {
        let deadline = Instant::now() + timeout;
        let mut guard = self.shared.snapshot.lock();
        loop {
            if predicate(&guard) {
                return Some(guard.clone());
            }
            if self
                .shared
                .changed
                .wait_until(&mut guard, deadline)
                .timed_out()
            {
                return predicate(&guard).then(|| guard.clone());
            }
        }
    }

    /// Older rows from the conversation log: rows with id below `before`, ascending,
    /// the newest `limit` of them (at most [`MAX_HISTORY_PAGE`]). Reads from disk on a
    /// worker thread and blocks the CALLER until it is done: call it from a background
    /// thread, never from a UI event or render path (use [`Self::history_page_with`]).
    pub fn history_page(
        &self,
        before: RowId,
        limit: usize,
    ) -> Result<Vec<Arc<Row>>, WorkflowError> {
        let (reply, rx) = channel();
        self.history_page_with(before, limit, move |page| {
            let _ = reply.send(page);
        })?;
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|_| WorkflowError::Closed)?
    }

    /// [`Self::history_page`] without blocking the caller: `done` runs on a worker
    /// thread with the page (or the read error). The file read never runs on the actor
    /// thread either, so a slow or stuck disk delays only this page.
    pub fn history_page_with(
        &self,
        before: RowId,
        limit: usize,
        done: impl FnOnce(Result<Vec<Arc<Row>>, WorkflowError>) + Send + 'static,
    ) -> Result<(), WorkflowError> {
        self.send(Msg::History {
            chat: self.snapshot().chat,
            before,
            limit,
            done: Box::new(done),
        })
    }

    // ---- settings -----------------------------------------------------------------------------------

    /// Replaces one setting if and only if the workflow is idle. The check and the
    /// replacement happen under the registry lock, which the actor also holds whenever a
    /// task or an Undo starts or ends, so a just-submitted task (not yet in any snapshot)
    /// is refused too; the change message is queued before the lock is released, so it is
    /// dispatched before anything submitted afterwards.
    fn change_settings(&self, change: impl FnOnce(&mut Settings)) -> Result<(), WorkflowError> {
        self.open_check()?;
        let registry = self.shared.registry.lock();
        if registry.busy() {
            return Err(WorkflowError::Busy("finish or stop the running task first"));
        }
        change(&mut self.shared.settings.lock());
        self.send(Msg::SettingsChanged)?;
        drop(registry);
        Ok(())
    }

    /// Replaces the adapter configuration (readiness returns to `Unchecked`).
    pub fn set_adapter(&self, adapter: Option<AdapterSettings>) -> Result<(), WorkflowError> {
        self.change_settings(|settings| settings.adapter = adapter)
    }

    /// Replaces the SDK/build settings (task tools restart with them).
    pub fn set_build(&self, build: Option<BuildSettings>) -> Result<(), WorkflowError> {
        self.change_settings(|settings| settings.build = build)
    }

    pub fn set_tools(&self, tools: Option<ToolSettings>) -> Result<(), WorkflowError> {
        self.change_settings(|settings| settings.tools = tools)
    }

    /// The preview playhead validation renders its representative frame around.
    pub fn set_playhead(&self, frame: usize) {
        self.shared.playhead.store(frame, Ordering::Relaxed);
    }

    /// Updates the exact preview currently displayed by the shell. Queued scoped
    /// submissions are refused if this identity changes before they start.
    pub fn set_displayed_preview_identity(
        &self,
        identity: Option<PreviewIdentity>,
    ) -> Result<(), WorkflowError> {
        self.send(Msg::SetDisplayedPreviewIdentity(identity))
    }

    // ---- commands ---------------------------------------------------------------------------------------

    fn send(&self, message: Msg) -> Result<(), WorkflowError> {
        self.shared
            .tx
            .send(message)
            .map_err(|_| WorkflowError::Closed)
    }

    fn open_check(&self) -> Result<(), WorkflowError> {
        if self.snapshot().closed {
            Err(WorkflowError::Closed)
        } else {
            Ok(())
        }
    }

    /// Probes the configured adapter once (executable/runtime/protocol/auth readiness).
    /// The only way an agent is launched outside a task.
    pub fn check_adapter(&self) -> Result<(), WorkflowError> {
        self.open_check()?;
        if self.shared.settings.lock().adapter.is_none() {
            return Err(WorkflowError::NotConfigured);
        }
        self.send(Msg::CheckAdapter)
    }

    /// Submits a brief. It starts at once when nothing runs, and queues behind the
    /// running task otherwise (the stable draft is refreshed when it starts). Returns the
    /// submission id shown in the queue.
    pub fn submit(&self, brief: &str) -> Result<u64, WorkflowError> {
        self.submit_with_scope(brief, None, Vec::new())
    }

    /// Submits a brief bound to the timeline/source identity selected in the UI.
    pub fn submit_scoped(&self, brief: &str, scope: TaskScope) -> Result<u64, WorkflowError> {
        scope
            .validate()
            .map_err(|error| WorkflowError::InvalidScope(error.to_string()))?;
        if scope.project_id != String::from(self.shared.project.clone()) {
            return Err(WorkflowError::InvalidScope(
                "scope belongs to another project".into(),
            ));
        }
        self.submit_with_scope(brief, Some(scope), Vec::new())
    }

    fn submit_with_scope(
        &self,
        brief: &str,
        scope: Option<TaskScope>,
        attachments: Vec<PreparedAttachment>,
    ) -> Result<u64, WorkflowError> {
        self.open_check()?;
        let brief =
            validate_brief(brief).map_err(|e| WorkflowError::InvalidBrief(e.to_string()))?;
        {
            let settings = self.shared.settings.lock();
            if settings.adapter.is_none() {
                return Err(WorkflowError::NotConfigured);
            }
            if settings.build.is_none() {
                return Err(WorkflowError::SdkUnavailable);
            }
        }
        {
            let mut registry = self.shared.registry.lock();
            if registry.pending >= MAX_QUEUED_BRIEFS {
                return Err(WorkflowError::QueueFull);
            }
            registry.pending += 1;
        }
        let id = self.shared.next_submission.fetch_add(1, Ordering::Relaxed);
        if let Err(error) = self.send(Msg::Submit {
            id,
            brief,
            scope: scope.map(Box::new),
            attachments,
        }) {
            self.shared.registry.lock().pending -= 1;
            return Err(error);
        }
        Ok(id)
    }

    pub fn cancel_queued(&self, id: u64) -> Result<(), WorkflowError> {
        self.send(Msg::CancelQueued(id))
    }

    /// Answers one open permission request. A reply that is stale (another task or
    /// agent session, or already closed) or a duplicate is rejected here, immediately,
    /// and never reaches the agent.
    pub fn reply_permission(
        &self,
        reference: &PermissionRef,
        answer: PermissionAnswer,
    ) -> Result<(), WorkflowError> {
        {
            let mut registry = self.shared.registry.lock();
            // The offered choices live in the registry, not in the transcript window: an
            // evicted card can still be answered, and a bad choice is refused without
            // consuming the request.
            match registry.open.get(reference) {
                Some(offered) => {
                    if let PermissionAnswer::Select(option) = &answer
                        && !offered.contains(option)
                    {
                        return Err(WorkflowError::UnknownChoice(option.clone()));
                    }
                    // The offered choices belong to the first question only.
                    if matches!(answer, PermissionAnswer::Select(_))
                        && registry
                            .composer
                            .questions
                            .iter()
                            .any(|q| &q.reference == reference && q.items.len() != 1)
                    {
                        return Err(WorkflowError::Refused(
                            "That request does not have exactly one question; answer each one with answer_questions or decline it."
                                .into(),
                        ));
                    }
                    registry.open.remove(reference);
                    registry.answered.push_back(reference.clone());
                    while registry.answered.len() > 64 {
                        registry.answered.pop_front();
                    }
                }
                None if registry.answered.contains(reference) => {
                    return Err(WorkflowError::DuplicatePermission);
                }
                None => return Err(WorkflowError::StalePermission),
            }
        }
        self.send(Msg::Reply {
            reference: reference.clone(),
            answer,
        })
    }

    /// Answers the agent's question: a subsequent prompt in the same provider session.
    pub fn reply_clarification(&self, task: &AgentTaskId, text: &str) -> Result<(), WorkflowError> {
        let text = validate_brief(text).map_err(|e| WorkflowError::InvalidBrief(e.to_string()))?;
        {
            let mut registry = self.shared.registry.lock();
            if registry.clarification.as_ref() != Some(task) {
                return Err(WorkflowError::NotWaiting);
            }
            registry.clarification = None;
        }
        self.send(Msg::Clarify {
            task: task.clone(),
            text,
        })
    }

    /// Selects an advertised mode. Unadvertised ids are refused.
    pub fn set_mode(&self, mode: &str) -> Result<(), WorkflowError> {
        if !self.snapshot().options.modes.iter().any(|m| m.id == mode) {
            return Err(WorkflowError::UnknownOption(mode.to_owned()));
        }
        self.send(Msg::SetMode(mode.to_owned()))
    }

    /// Sets an advertised config option.
    pub fn set_config_option(&self, id: &str, value: OptionValue) -> Result<(), WorkflowError> {
        if !self.snapshot().options.config.iter().any(|c| c.id == id) {
            return Err(WorkflowError::UnknownOption(id.to_owned()));
        }
        self.send(Msg::SetConfig(id.to_owned(), value))
    }

    /// Stop, in every state: the prompt is cancelled (open permissions close as
    /// cancelled), the adapter tree is reaped, the working copy is retained, queued
    /// briefs are dropped. During publication the short commit cannot be cancelled;
    /// Stop then reports it and the result is recoverable with Undo.
    pub fn stop(&self) -> Result<(), WorkflowError> {
        // Cancellation reaches an in-flight job at once, without waiting for the actor
        // to dispatch: the job notices it at its next checkpoint (including while it
        // waits for the controller). The actor then performs the state transition and
        // the cleanup it owns.
        if let Some(gate) = self.shared.inflight.lock().as_ref() {
            gate.cancel();
        }
        self.send(Msg::Stop)
    }

    /// Manual review: publish the retained candidate.
    pub fn apply(&self) -> Result<(), WorkflowError> {
        self.send(Msg::Apply)
    }

    /// Manual review: drop the candidate (the working copy is kept).
    pub fn discard(&self) -> Result<(), WorkflowError> {
        self.send(Msg::Discard)
    }

    /// Manual review: export the candidate as an independent project folder.
    pub fn export_candidate(&self, destination: &Path) -> Result<(), WorkflowError> {
        self.send(Msg::ExportCandidate(destination.to_owned()))
    }

    /// Exports the retained working copy (after the task ended and no writer may remain).
    pub fn export_draft(&self, destination: &Path) -> Result<(), WorkflowError> {
        self.send(Msg::ExportDraft(destination.to_owned()))
    }

    /// Undo the newest accepted agent edit still in effect (or `target`): a candidate is
    /// formed from the current source, validated, published and handed to the preview.
    pub fn undo(&self, target: Option<&str>) -> Result<(), WorkflowError> {
        self.open_check()?;
        // Counted as pending until the actor starts it, so a settings change racing the
        // request is refused like one racing a submitted brief.
        self.shared.registry.lock().pending += 1;
        if let Err(error) = self.send(Msg::Undo(target.map(str::to_owned))) {
            let mut registry = self.shared.registry.lock();
            registry.pending = registry.pending.saturating_sub(1);
            return Err(error);
        }
        Ok(())
    }

    /// Before and after text of every file the candidate under review changes. Reads the
    /// checkpoint objects: call it off the UI thread.
    pub fn candidate_file_diffs(
        &self,
    ) -> Result<Vec<crate::candidate_validation::FileDiff>, WorkflowError> {
        self.open_check()?;
        let source = self.shared.controller.lock().candidate_diff_source();
        source
            .ok_or(WorkflowError::NoCandidate)?
            .read()
            .map_err(|e| WorkflowError::Review(e.to_string()))
    }

    /// Before and after text of every file an accepted revision changed. Reads the
    /// checkpoint objects: call it off the UI thread.
    pub fn revision_file_diffs(
        &self,
        revision: &str,
    ) -> Result<Vec<crate::candidate_validation::FileDiff>, WorkflowError> {
        self.open_check()?;
        let source = self.shared.controller.lock().revision_diff_source(revision);
        source
            .ok_or_else(|| WorkflowError::Review(format!("{revision} is not in the history")))?
            .read()
            .map_err(|e| WorkflowError::Review(e.to_string()))
    }

    /// Manual review: leave `paths` out of the candidate. The remaining files become a new
    /// candidate that must pass validation before it can be applied; if they do not, the
    /// candidate under review is unchanged (an error row says why). Rejecting every file
    /// discards the candidate.
    pub fn reject_files(&self, paths: &[String]) -> Result<(), WorkflowError> {
        if paths.is_empty() {
            return Err(WorkflowError::Review(
                "choose at least one file to reject".into(),
            ));
        }
        self.send(Msg::RejectFiles(paths.to_vec()))
    }

    /// Reverts one file of the accepted revision `revision`: an Undo candidate that
    /// touches only `path` is validated and published. Refused (with a reason in the
    /// transcript) when the file changed after the revision or was already reverted.
    pub fn revert_path(&self, revision: &str, path: &str) -> Result<(), WorkflowError> {
        self.open_check()?;
        self.shared.registry.lock().pending += 1;
        let message = Msg::RevertPath {
            target: revision.to_owned(),
            path: path.to_owned(),
        };
        if let Err(error) = self.send(message) {
            let mut registry = self.shared.registry.lock();
            registry.pending = registry.pending.saturating_sub(1);
            return Err(error);
        }
        Ok(())
    }

    /// App-local review policy. The running task keeps the policy it started with.
    pub fn set_review_policy(&self, policy: ReviewPolicy) -> Result<(), WorkflowError> {
        self.send(Msg::SetPolicy(policy))
    }

    /// How permission requests are answered. The running task keeps the policy it started with.
    pub fn set_permission_policy(
        &self,
        policy: studio_proto::PermissionPolicy,
    ) -> Result<(), WorkflowError> {
        self.send(Msg::SetPermissionPolicy(policy))
    }

    /// The rules saved for this project with "Always in this project", oldest first.
    pub fn approval_rules(&self) -> Vec<ApprovalRule> {
        self.shared.approvals.lock().clone()
    }

    /// Saves `rule` for this project (stamped with the time) and hands the rules to the
    /// running task, which uses them from its next request. Writes a small file in the
    /// app data.
    pub fn remember_approval(&self, rule: ApprovalRule) -> Result<(), WorkflowError> {
        if !rule.is_valid() {
            return Err(WorkflowError::Refused(
                "That request cannot be saved as a rule.".into(),
            ));
        }
        self.change_approvals(|rules| {
            if !rules.iter().any(|kept| kept.same_as(&rule)) {
                rules.push(ApprovalRule {
                    saved_unix: approvals::now_unix(),
                    ..rule
                });
            }
        })
    }

    /// Removes `rule` from this project; the running task stops using it from its next
    /// request.
    pub fn forget_approval(&self, rule: &ApprovalRule) -> Result<(), WorkflowError> {
        self.change_approvals(|rules| rules.retain(|kept| !kept.same_as(rule)))
    }

    fn change_approvals(
        &self,
        change: impl FnOnce(&mut Vec<ApprovalRule>),
    ) -> Result<(), WorkflowError> {
        let rules = {
            let mut saved = self.shared.approvals.lock();
            let mut rules = saved.clone();
            change(&mut rules);
            approvals::save(&self.shared.paths, &self.shared.project, &rules)
                .map_err(WorkflowError::Refused)?;
            *saved = rules.clone();
            rules
        };
        self.send(Msg::SetApprovals(rules))
    }

    /// The user's own MCP servers and turned-off project tools. The running task keeps the
    /// settings it started with.
    pub fn set_agent_tools(
        &self,
        settings: crate::agent_settings::AgentToolSettings,
    ) -> Result<(), WorkflowError> {
        self.send(Msg::SetAgentTools(settings))
    }

    /// The preview now shows `revision` (clears the awaiting-preview label).
    pub fn preview_displayed(&self, revision: &str) -> Result<(), WorkflowError> {
        self.send(Msg::PreviewDisplayed(revision.to_owned()))
    }

    /// A [`PreviewHandoff`] that accepted the staged preview but completes the adoption
    /// elsewhere (the UI thread) reports how it ended: `Ok` = adopted, `Err` = the
    /// accepted revision awaits an ordinary preview build, with the reason.
    pub fn report_handoff(
        &self,
        published: &str,
        result: Result<(), String>,
    ) -> Result<(), WorkflowError> {
        self.send(Msg::HandoffResolved {
            published: published.to_owned(),
            result,
        })
    }

    /// The user confirms that nothing writes into a locked working copy.
    pub fn acknowledge_writer_gone(&self) -> Result<(), WorkflowError> {
        self.send(Msg::AcknowledgeWriterGone)
    }

    pub fn resolve_conflict(&self, transaction: &str, note: &str) -> Result<(), WorkflowError> {
        self.send(Msg::ResolveConflict {
            transaction: transaction.to_owned(),
            note: note.to_owned(),
        })
    }

    /// Recomputes Undo/history/recovery from the engine (after external changes).
    pub fn refresh(&self) -> Result<(), WorkflowError> {
        self.send(Msg::Refresh)
    }

    /// Initiates a controlled handoff to another provider.
    pub fn initiate_switch(&self, target_provider: &str) -> Result<(), WorkflowError> {
        self.open_check()?;
        self.send(Msg::InitiateSwitch {
            target_provider: target_provider.to_string(),
        })
    }

    /// Chooses whether to continue the captured draft or restart fresh from accepted source.
    pub fn choose_handoff_continuation(&self, choice: HandoffChoice) -> Result<(), WorkflowError> {
        self.open_check()?;
        self.send(Msg::ChooseHandoffContinuation(choice))
    }

    /// Cancels a pending provider switch.
    pub fn cancel_switch(&self) -> Result<(), WorkflowError> {
        self.open_check()?;
        self.send(Msg::CancelSwitch)
    }

    /// Explicitly transfers retained briefs from an outgoing provider to the incoming provider.
    pub fn transfer_retained_queue(&self) -> Result<(), WorkflowError> {
        self.open_check()?;
        self.send(Msg::TransferRetainedQueue)
    }

    /// Discards retained briefs from an outgoing provider without executing them.
    pub fn clear_retained_queue(&self) -> Result<(), WorkflowError> {
        self.open_check()?;
        self.send(Msg::ClearRetainedQueue)
    }

    /// Requests restoration of a previously saved session.
    pub fn restore_session(&self) -> Result<(), WorkflowError> {
        self.open_check()?;
        self.send(Msg::RestoreSession)
    }

    /// Dismisses a saved session and clears its manifest.
    pub fn dismiss_restoration(&self) -> Result<(), WorkflowError> {
        self.open_check()?;
        self.send(Msg::DismissRestoration)
    }

    /// Starts an empty conversation while nothing is running: it becomes a new stored session
    /// and the current one stays in the session list, resumable. Row ids keep counting up
    /// across sessions. Undo history, the review policy and the project are kept. Refused
    /// with a warning row while a task, an undo, a provider switch, queued briefs or an
    /// unresolved conflict still belong to the current conversation.
    pub fn new_chat(&self) -> Result<(), WorkflowError> {
        self.open_check()?;
        self.send(Msg::NewChat)
    }

    /// [`Self::new_chat`], refused at once with [`WorkflowError::Busy`] (the reason to show)
    /// while a task, an undo or a queued brief is running. A conflict the actor alone knows
    /// of is still refused there, with a warning row.
    pub fn new_session(&self) -> Result<(), WorkflowError> {
        self.open_check()?;
        self.refuse_while_busy(&BUSY_NEW_SESSION)?;
        self.send(Msg::NewChat)
    }

    /// The project's stored sessions, most recently updated first, each with its title,
    /// agent, outcome and whether it is the current one and can be resumed.
    ///
    /// Reads the log of every session changed since the last listing: call it off the UI
    /// thread, not on every keystroke.
    /// The newest rows of the running session reach disk with the next snapshot, so its
    /// entry can lag the transcript by a moment.
    pub fn sessions(&self) -> Result<Vec<session_store::history::SessionSummary>, WorkflowError> {
        self.open_check()?;
        session_store::SessionStore::list_sessions(&self.shared.paths, &self.shared.project)
            .map_err(WorkflowError::History)
    }

    /// Switches to a stored session: the transcript is reloaded from its stored log and, when
    /// the agent supports `session/resume` or `session/load` (or `--resume`, `thread/resume`),
    /// the next prompt resumes the agent's own session. Otherwise the stored rows are shown
    /// read-only with a notice and the next prompt starts a fresh agent session that still
    /// appends to this stored session. Opening the current session does nothing (the actor
    /// decides).
    ///
    /// Refused with [`WorkflowError::Busy`] (the reason names what is going on) while a task,
    /// an undo or a queued brief is running; other blockers (a conflict, a provider switch) are refused
    /// by the actor with a warning row. The switch itself happens on the actor thread: wait
    /// for `WorkflowSnapshot::chat` to change.
    pub fn open_session(&self, id: &str) -> Result<(), WorkflowError> {
        self.open_check()?;
        if !crate::app_paths::is_valid_session_id(id)
            || !self
                .shared
                .paths
                .agent_session_dir(&self.shared.project, id)
                .is_dir()
        {
            return Err(WorkflowError::History(format!("unknown session `{id}`")));
        }
        self.refuse_while_busy(&BUSY_OPEN_SESSION)?;
        self.send(Msg::OpenSession(id.to_owned()))
    }

    /// Refuses with the reason that names what is going on, judged from the last snapshot
    /// (the registry alone says whether something is).
    fn refuse_while_busy(&self, reasons: &[&'static str; 4]) -> Result<(), WorkflowError> {
        if !self.shared.registry.lock().busy() {
            return Ok(());
        }
        let snapshot = self.snapshot();
        let reviewing = snapshot
            .task
            .as_ref()
            .is_some_and(|task| task.phase == TaskPhase::AwaitingReview);
        let index = if reviewing {
            0
        } else if matches!(snapshot.undo, UndoView::InProgress { .. }) {
            1
        } else if snapshot.task.is_none() && !snapshot.queue.is_empty() {
            2
        } else {
            3
        };
        Err(WorkflowError::Busy(reasons[index]))
    }

    /// The project's process scope (the controller's, shut down when the project closes
    /// and on quit).
    pub fn process_scope(&self) -> ProcessTreeManager {
        self.shared.root_scope.clone()
    }

    #[doc(hidden)]
    pub fn test_hold_tools_gate(&self) -> Option<crate::agent_tools::backend::WriterGuard> {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = self.send(Msg::TestHoldToolsGate(tx));
        rx.recv().ok().flatten()
    }

    /// Cancels and reaps the task, broker, compiler subscribers and workers, then stops
    /// the actor. Idempotent. Does not close the caller's controller.
    pub fn close(&self) {
        let thread = self.thread.lock().take();
        let Some(thread) = thread else {
            return;
        };
        // Like Stop, closing reaches an in-flight job without waiting for the actor to
        // dispatch: the job notices at its next checkpoint, the actor then cleans up.
        if let Some(gate) = self.shared.inflight.lock().as_ref() {
            gate.cancel();
        }
        let (done, wait) = channel();
        if self.shared.tx.send(Msg::Close(done)).is_ok() {
            let _ = wait.recv_timeout(Duration::from_secs(60));
        }
        let _ = thread.join();
    }
}

impl AgentWorkflow {
    /// [`Self::open`] on its own thread: returns at once; `done` receives the workflow (or
    /// why it could not be opened) on that thread.
    pub fn open_detached(
        config: WorkflowConfig,
        controller: Arc<Mutex<Controller>>,
        done: impl FnOnce(Result<Self, WorkflowError>) + Send + 'static,
    ) {
        let slot = Arc::new(Mutex::new(Some(done)));
        let inner = slot.clone();
        let spawned = std::thread::Builder::new()
            .name("studio-workflow-open".into())
            .spawn(move || {
                if let Some(done) = inner.lock().take() {
                    done(Self::open(config, controller));
                }
            });
        if let Err(error) = spawned
            && let Some(done) = slot.lock().take()
        {
            done(Err(WorkflowError::History(error.to_string())));
        }
    }

    /// [`Self::close`] on its own thread, for callers that must not block (a UI thread).
    pub fn close_detached(self) {
        std::thread::spawn(move || self.close());
    }
}

impl Drop for AgentWorkflow {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod evidence_image_cache_tests {
    use super::*;

    #[test]
    fn reservations_are_task_owned_and_bounded_by_count_and_encoded_bytes() {
        let first = AgentTaskId("first-task".into());
        let second = AgentTaskId("second-task".into());
        let mut cache = EvidenceImageCache::default();
        cache.begin_task(first.clone());

        assert_eq!(
            cache.reserve(&first, "selected", 4 * 1024 * 1024),
            Some(4 * 1024 * 1024)
        );
        assert_eq!(
            cache.reserve(&first, "boundary", 4 * 1024 * 1024),
            Some(4 * 1024 * 1024)
        );
        assert_eq!(cache.reserve(&first, "overflow", 1), None);
        assert_eq!(cache.reserve(&first, "selected", 1), None);

        cache.begin_task(second.clone());
        assert_eq!(cache.reserve(&first, "stale", 1), None);
        assert_eq!(cache.reserve(&second, "selected", 1), Some(1));
    }

    #[test]
    fn reservations_are_limited_to_six_artifacts_per_task() {
        let task = AgentTaskId("bounded-task".into());
        let mut cache = EvidenceImageCache::default();
        cache.begin_task(task.clone());
        for index in 0..6 {
            assert_eq!(
                cache.reserve(&task, &format!("artifact-{index}"), 1),
                Some(1)
            );
        }
        assert_eq!(cache.reserve(&task, "seventh", 1), None);
    }
}
