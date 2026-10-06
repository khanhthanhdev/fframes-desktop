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
mod jobs;
pub mod log;
pub mod model;
mod present;
mod tools;

pub use jobs::{JobFault, JobFaults, PreviewHandoff, PromotionHandoff};
pub use model::*;
pub use tools::BuildSettings;

use actor::{Actor, Msg};
use fframes_studio_protocol::PreviewIdentity;
use gpui::RenderImage;
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
use studio_agent_spike::{
    AdapterConfig, DriverLimits, ExecutableSearch, McpStdioSupport, driver::OptionValue,
};
use studio_bootstrap::{ProcessTreeManager, WriterOwnership};
use studio_engine::{
    AgentTaskId, Controller, ReviewPolicy, TaskScope, agent_task::validate_brief,
    app_paths::AppPaths,
};
use studio_project::ProjectId;

/// See [`AdapterSettings::ownership_probe`].
pub type OwnershipProbe = Arc<dyn Fn() -> WriterOwnership + Send + Sync>;

/// The one adapter a project talks to (provider-owned setup; values of auth variables are
/// never stored here, only their names).
#[derive(Clone)]
pub struct AdapterSettings {
    /// Label carried by driver events and rows.
    pub provider: String,
    pub adapter: AdapterConfig,
    /// How this adapter's writer descendants are owned. Only an explicit qualification
    /// result may be `ProcessGroupContained`; anything else blocks candidate capture.
    pub writer_ownership: WriterOwnership,
    /// Re-derives the ownership at every task launch (on the workflow's own thread), for a
    /// host that qualifies adapters from evidence: a replaced executable or ledger then
    /// never keeps a stale qualification. When present its answer replaces
    /// `writer_ownership` for that launch; `writer_ownership` is what is shown before one.
    pub ownership_probe: Option<OwnershipProbe>,
    /// Host policy for stdio MCP (`Unsupported` = CLI route only).
    pub mcp: McpStdioSupport,
    /// Advertised authentication method to select explicitly, if the adapter needs one.
    pub auth_method: Option<String>,
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
}

impl Registry {
    /// Authoritative, not snapshot-derived: submissions in flight count as busy.
    pub(crate) fn busy(&self) -> bool {
        self.pending > 0 || self.running > 0
    }
}

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
    images: HashMap<String, Option<Arc<RenderImage>>>,
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
        const MAX_ENCODED_BYTES: usize = crate::evidence_preview::MAX_ENCODED_BYTES;
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

    fn complete(&mut self, task: &AgentTaskId, id: &str, image: Arc<RenderImage>) -> bool {
        if self.task.as_ref() != Some(task) {
            return false;
        }
        let Some(slot) = self.images.get_mut(id) else {
            return false;
        };
        *slot = Some(image);
        true
    }

    fn image(&self, task: &AgentTaskId, id: &str) -> Option<Arc<RenderImage>> {
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
    /// Bounded UI-only image handles; never serialized or added to snapshots.
    evidence_images: Mutex<EvidenceImageCache>,
    pub changed: Condvar,
    pub registry: Mutex<Registry>,
    /// The cancellation gate of the job the actor is running (pipeline, publication or
    /// Undo), so `stop()` can cancel it without waiting for the actor.
    pub inflight: Mutex<Option<Arc<Gate>>>,
    pub live: Arc<tools::LiveTasks>,
    pub playhead: AtomicUsize,
    pub next_submission: AtomicU64,
    pub tx: Sender<Msg>,
}

/// The per-project agent workflow. Dropping it closes it.
pub struct AgentWorkflow {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl AgentWorkflow {
    /// Starts the actor for the project `controller` owns. Reads the conversation log and
    /// the engine's recovery state; launches nothing (no adapter probe, no agent).
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
        let store = RowStore::open(
            present::log_path(&config.paths, &project),
            config.row_limits,
        )
        .map_err(|e: LogError| WorkflowError::History(e.to_string()))?;
        let (tx, rx) = channel();
        let empty = Arc::new(WorkflowSnapshot {
            revision: 0,
            project: project.clone(),
            adapter: AdapterView {
                provider: None,
                executable: None,
                readiness: AdapterReadiness::NotConfigured,
                writer_ownership: None,
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
            rows: Vec::new(),
            older_rows: false,
            review_policy: ReviewPolicy::default(),
            undo: UndoView::Unavailable {
                reason: "starting".into(),
            },
            recovery: RecoveryView::default(),
            handoff: None,
            history: Vec::new(),
            resources: ResourceView::default(),
            closed: false,
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
            snapshot: Mutex::new(empty),
            evidence_images: Mutex::new(EvidenceImageCache::default()),
            changed: Condvar::new(),
            registry: Mutex::new(Registry::default()),
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

    /// A presentation image for a task-owned evidence artifact, if its bounded decode
    /// has completed. PNG bytes and app-private paths never leave the workflow worker.
    pub(crate) fn evidence_image(
        &self,
        task: &AgentTaskId,
        artifact_id: &str,
    ) -> Option<Arc<RenderImage>> {
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
        self.submit_with_scope(brief, None)
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
        self.submit_with_scope(brief, Some(scope))
    }

    fn submit_with_scope(
        &self,
        brief: &str,
        scope: Option<TaskScope>,
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

    /// App-local review policy. The running task keeps the policy it started with.
    pub fn set_review_policy(&self, policy: ReviewPolicy) -> Result<(), WorkflowError> {
        self.send(Msg::SetPolicy(policy))
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
