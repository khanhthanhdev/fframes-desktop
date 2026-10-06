//! The production [`ToolBackend`]: six read-only project tools over immutable revisions.
//!
//! * `Fixed` bindings answer for one immutable (candidate) revision.
//! * `Draft` bindings first take the [`WriterGate`], capture a labelled immutable
//!   snapshot of the task's stable draft into checkpoint objects (the capture verifies
//!   the tree twice), or return `busy` when the tree is changing. A changing tree is
//!   never compiled.
//! * Builds go through the shared [`BuildService`] (equal key => one compile) and run in
//!   tool-owned preview workers with their own process scope: they never use the
//!   displayed worker or the thumbnail lane. At most [`TOOL_WORKERS`] workers live.
//! * Images are app-owned PNG artifacts ([`ArtifactStore`]) with ids, hashes, expiry and
//!   size limits; nothing is returned inline.
//! * `build_status` only observes; it never captures, compiles or launches Cargo.
//!
//! Tool builds are explicitly unvalidated: only the acceptance validator qualifies a task.
use super::{
    ARTIFACT_TTL, ArtifactRef, BoundRevision, MAX_IMAGE_ARTIFACT_BYTES, RevisionInfo,
    RevisionLabel, TOOL_WORKERS, TaskLiveness, ToolBackend, ToolBinding, ToolCall, ToolError,
    ToolErrorCode, ToolReply, ToolRequest,
};
use crate::{
    build_service::{
        BuildError, BuildKey, BuildService, CompileEnvironment, CompileRequest, Subscriber,
        SubscriberKind,
    },
    preview_worker_client::PreviewWorkerClient,
    worker_client::allocate_worker_generation,
    worker_project::launch_preview_worker,
};
use fframes_studio_protocol::{
    MAX_PREVIEW_HEIGHT, MAX_PREVIEW_WIDTH, PreviewIdentity, PreviewTimelineResponse,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    any::Any,
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{
    AgentTaskId, TaskIdentity,
    candidate_validation::{
        CandidateError, RestoredRevision, capture_draft_revision, restore_revision,
    },
};
use studio_project::{ProjectId, SourceRevision, checkpoint::Checkpoints, revision::FileKind};
use studio_sdk::CompatibilityManifest;

const MAX_ARTIFACT_STORE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_OPEN_REVISIONS: usize = 4;
const MAX_CONTEXT_FILES: usize = 400;
/// Snapshot identities a task may assert again (see [`RevisionHistory`]).
pub const MAX_KNOWN_REVISIONS: usize = 64;
/// Identities `build_status` lists. Independent of (and smaller than) the retention bound,
/// so the reply size depends on neither the bound nor how many snapshots a task captured.
pub const STATUS_KNOWN_REVISIONS: usize = 16;
/// Strip width bound: keeps a 24-frame strip a few megabytes of PNG at most.
const MAX_STRIP_WIDTH: u32 = 4096;

fn tool_error(code: ToolErrorCode, message: impl Into<String>) -> ToolError {
    ToolError::new(code, message)
}

fn build_error(error: BuildError) -> ToolError {
    match error {
        BuildError::Failed(message) => tool_error(ToolErrorCode::BuildFailed, message),
        BuildError::Cancelled => tool_error(ToolErrorCode::Cancelled, "build cancelled"),
        BuildError::CacheFull { .. } => tool_error(ToolErrorCode::Busy, error.to_string()),
        other => tool_error(ToolErrorCode::Unavailable, other.to_string()),
    }
}

// ---- writer gate -------------------------------------------------------------------------

/// The project's draft writer gate. The orchestrator holds the guard while a provider
/// writer may modify the draft (from writer start until quiescence); tool calls that
/// need a fresh draft snapshot take it themselves and report contention as `busy`,
/// never waiting it out.
#[derive(Clone, Default)]
pub struct WriterGate(Arc<AtomicBool>);

pub struct WriterGuard(Arc<AtomicBool>);

impl WriterGate {
    pub fn try_acquire(&self) -> Option<WriterGuard> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| WriterGuard(self.0.clone()))
    }
}

impl Drop for WriterGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// A launching worker's claim on one of the [`TOOL_WORKERS`] slots.
struct LaunchSlot<'a>(&'a AtomicUsize);

impl Drop for LaunchSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

// ---- artifacts ---------------------------------------------------------------------------

struct StoredArtifact {
    path: PathBuf,
    bytes: u64,
    expires: Instant,
    task: AgentTaskId,
}

/// App-owned artifact directory: ids, expiry and size limits. The only way a caller gets
/// a path is through an [`ArtifactRef`] it was handed.
pub struct ArtifactStore {
    root: PathBuf,
    ttl: Duration,
    max_total: u64,
    entries: Mutex<HashMap<String, StoredArtifact>>,
}

impl ArtifactStore {
    pub fn new(root: &Path, ttl: Duration) -> Result<Self, ToolError> {
        Self::with_limit(root, ttl, MAX_ARTIFACT_STORE_BYTES)
    }

    pub fn with_limit(root: &Path, ttl: Duration, max_total: u64) -> Result<Self, ToolError> {
        let internal = |e: std::io::Error| tool_error(ToolErrorCode::Internal, e.to_string());
        // Artifacts are disposable: whatever a previous app run left is removed.
        if std::fs::symlink_metadata(root).is_ok_and(|m| m.is_symlink()) {
            return Err(tool_error(
                ToolErrorCode::Internal,
                "artifact root must not be a symlink",
            ));
        }
        let _ = std::fs::remove_dir_all(root);
        std::fs::create_dir_all(root).map_err(internal)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
                .map_err(internal)?;
        }
        Ok(Self {
            root: std::fs::canonicalize(root).map_err(internal)?,
            ttl,
            max_total,
            entries: Mutex::new(HashMap::new()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn sweep(&self, entries: &mut HashMap<String, StoredArtifact>) {
        let now = Instant::now();
        entries.retain(|_, a| {
            let keep = a.expires > now;
            if !keep {
                let _ = std::fs::remove_file(&a.path);
            }
            keep
        });
    }

    /// Store one PNG. Larger than 8 MiB is refused.
    pub fn put_png(
        &self,
        task: &AgentTaskId,
        bytes: &[u8],
        width: u32,
        height: u32,
    ) -> Result<ArtifactRef, ToolError> {
        if bytes.len() > MAX_IMAGE_ARTIFACT_BYTES {
            return Err(tool_error(
                ToolErrorCode::TooLarge,
                format!(
                    "the rendered image is {} bytes; the limit is {MAX_IMAGE_ARTIFACT_BYTES}. Request a smaller scale or fewer frames",
                    bytes.len()
                ),
            ));
        }
        let mut entries = self.entries.lock();
        self.sweep(&mut entries);
        let mut total: u64 = entries.values().map(|a| a.bytes).sum();
        while total + bytes.len() as u64 > self.max_total {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, a)| a.expires)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            if let Some(gone) = entries.remove(&oldest) {
                total -= gone.bytes;
                let _ = std::fs::remove_file(gone.path);
            }
        }
        let id = format!("art-{}", uuid::Uuid::new_v4().simple());
        let path = self.root.join(format!("{id}.png"));
        write_private(&path, bytes)
            .map_err(|e| tool_error(ToolErrorCode::Internal, e.to_string()))?;
        let expires = Instant::now() + self.ttl;
        entries.insert(
            id.clone(),
            StoredArtifact {
                path: path.clone(),
                bytes: bytes.len() as u64,
                expires,
                task: task.clone(),
            },
        );
        Ok(ArtifactRef {
            id,
            media_type: "image/png".into(),
            path: path.to_string_lossy().into_owned(),
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            width,
            height,
            expires_at_unix: SystemTime::now()
                .checked_add(self.ttl)
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs()),
        })
    }

    /// Resolve an id to its path: well-formed id, known, unexpired, a regular file inside
    /// the store with no symlink component.
    pub fn resolve(&self, id: &str) -> Result<PathBuf, ToolError> {
        let well_formed = id
            .strip_prefix("art-")
            .is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()));
        if !well_formed {
            return Err(tool_error(
                ToolErrorCode::InvalidParams,
                "malformed artifact id",
            ));
        }
        let mut entries = self.entries.lock();
        self.sweep(&mut entries);
        let entry = entries
            .get(id)
            .ok_or_else(|| tool_error(ToolErrorCode::NotFound, "unknown or expired artifact"))?;
        let meta = std::fs::symlink_metadata(&entry.path)
            .map_err(|_| tool_error(ToolErrorCode::NotFound, "artifact file is gone"))?;
        let canonical = std::fs::canonicalize(&entry.path)
            .map_err(|_| tool_error(ToolErrorCode::NotFound, "artifact file is gone"))?;
        if meta.is_symlink() || !meta.is_file() || !canonical.starts_with(&self.root) {
            return Err(tool_error(
                ToolErrorCode::Unauthorized,
                "artifact path escapes the app store",
            ));
        }
        Ok(canonical)
    }

    /// Reads a bounded PNG only when it belongs to `task`. This is used to attach
    /// revision-bound evidence to an ACP prompt without exposing its app-private path.
    pub fn read_png_for_task(
        &self,
        task: &AgentTaskId,
        id: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ToolError> {
        let well_formed = id
            .strip_prefix("art-")
            .is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit()));
        if !well_formed {
            return Err(ToolError::new(
                ToolErrorCode::InvalidParams,
                "malformed artifact id",
            ));
        }
        let mut entries = self.entries.lock();
        self.sweep(&mut entries);
        let entry = entries.get(id).ok_or_else(|| {
            ToolError::new(ToolErrorCode::NotFound, "unknown or expired artifact")
        })?;
        if &entry.task != task {
            return Err(ToolError::new(
                ToolErrorCode::Unauthorized,
                "artifact belongs to another task",
            ));
        }
        if entry.bytes > max_bytes as u64 {
            return Err(ToolError::new(
                ToolErrorCode::TooLarge,
                "artifact exceeds the prompt image limit",
            ));
        }
        let meta = std::fs::symlink_metadata(&entry.path)
            .map_err(|_| ToolError::new(ToolErrorCode::NotFound, "artifact file is gone"))?;
        let canonical = std::fs::canonicalize(&entry.path)
            .map_err(|_| ToolError::new(ToolErrorCode::NotFound, "artifact file is gone"))?;
        if meta.is_symlink() || !meta.is_file() || !canonical.starts_with(&self.root) {
            return Err(ToolError::new(
                ToolErrorCode::Unauthorized,
                "artifact path escapes the app store",
            ));
        }
        let bytes = std::fs::read(canonical)
            .map_err(|_| ToolError::new(ToolErrorCode::NotFound, "artifact file is gone"))?;
        if bytes.len() as u64 != entry.bytes || bytes.len() > max_bytes {
            return Err(ToolError::new(
                ToolErrorCode::TooLarge,
                "artifact size changed or exceeds the prompt image limit",
            ));
        }
        Ok(bytes)
    }

    pub fn remove_task(&self, task: &AgentTaskId) {
        let mut entries = self.entries.lock();
        entries.retain(|_, a| {
            let keep = &a.task != task;
            if !keep {
                let _ = std::fs::remove_file(&a.path);
            }
            keep
        });
    }

    pub fn clear(&self) {
        let mut entries = self.entries.lock();
        for (_, a) in entries.drain() {
            let _ = std::fs::remove_file(a.path);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

// ---- backend -----------------------------------------------------------------------------

pub struct ToolBackendConfig {
    pub project_id: ProjectId,
    pub service: BuildService,
    pub sdk: PathBuf,
    pub compatibility: CompatibilityManifest,
    pub builds: PathBuf,
    /// The project's history root (the checkpoint object store).
    pub history: PathBuf,
    /// App-owned artifact directory (recreated empty at start).
    pub artifacts: PathBuf,
    /// Parent process scope; every tool worker gets its own child scope.
    pub processes: ProcessTreeManager,
    pub gate: WriterGate,
}

/// The snapshot identities a task's draft calls have been answered for, newest last.
///
/// Retention policy: at most [`MAX_KNOWN_REVISIONS`] identities per task, in recency
/// order. Naming a retained identity again (an assertion that resolves) refreshes it;
/// when a new identity would exceed the bound the least recently used one is evicted.
/// An evicted identity is no longer "produced for this task" as far as assertions go and
/// is answered `stale_revision`, exactly like a stranger's. Membership, insertion and
/// refresh are O(bound); memory is O(bound) however many snapshots the task captures.
#[derive(Debug, Default)]
pub struct RevisionHistory {
    retained: VecDeque<String>,
    /// Identities evicted over the task's life.
    evicted: u64,
}

/// What `build_status` reports of a [`RevisionHistory`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KnownSummary {
    /// The most recent identities, oldest first.
    pub recent: Vec<String>,
    /// Identities ever retained (one evicted and captured again counts again).
    pub total: u64,
}

impl RevisionHistory {
    /// Records `revision` as the most recent identity, evicting the oldest past the bound.
    pub fn remember(&mut self, revision: &str) {
        if let Some(at) = self.retained.iter().position(|r| r == revision) {
            if let Some(existing) = self.retained.remove(at) {
                self.retained.push_back(existing);
            }
            return;
        }
        if self.retained.len() >= MAX_KNOWN_REVISIONS {
            self.retained.pop_front();
            self.evicted += 1;
        }
        self.retained.push_back(revision.to_owned());
    }

    pub fn contains(&self, revision: &str) -> bool {
        self.retained.iter().any(|r| r == revision)
    }

    /// Identities currently retained (never more than [`MAX_KNOWN_REVISIONS`]).
    pub fn len(&self) -> usize {
        self.retained.len()
    }

    pub fn is_empty(&self) -> bool {
        self.retained.is_empty()
    }

    /// Identities evicted so far.
    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    /// The newest `limit` identities (oldest first) plus the distinct total ever recorded.
    pub fn summary(&self, limit: usize) -> KnownSummary {
        let skip = self.retained.len().saturating_sub(limit);
        KnownSummary {
            recent: self.retained.iter().skip(skip).cloned().collect(),
            total: self.evicted + self.retained.len() as u64,
        }
    }
}

struct TaskResources {
    identity: TaskIdentity,
    draft: PathBuf,
    base: SourceRevision,
    /// Immutable revisions this task's calls have been answered for (bounded).
    known: RevisionHistory,
}

struct Opened {
    restored: Arc<RestoredRevision>,
    revision: String,
}

struct ToolWorker {
    client: PreviewWorkerClient,
    scope: ProcessTreeManager,
    timeline: PreviewTimelineResponse,
}

impl Drop for ToolWorker {
    fn drop(&mut self) {
        self.client.shutdown();
        self.scope.shutdown(Duration::ZERO);
    }
}

struct PoolEntry {
    task: AgentTaskId,
    revision: String,
    worker: Arc<Mutex<ToolWorker>>,
    last_used: Instant,
}

pub struct ProjectToolBackend {
    config: ToolBackendConfig,
    checkpoints: Checkpoints,
    artifacts: ArtifactStore,
    tasks: Mutex<HashMap<AgentTaskId, TaskResources>>,
    opened: Mutex<Vec<(AgentTaskId, Arc<Opened>)>>,
    pool: Mutex<Vec<PoolEntry>>,
    /// Workers being compiled/launched; they count against [`TOOL_WORKERS`].
    launching: AtomicUsize,
    closed: AtomicBool,
}

impl ProjectToolBackend {
    pub fn new(config: ToolBackendConfig) -> Result<Self, ToolError> {
        let checkpoints = Checkpoints::new(&config.history)
            .map_err(|e| tool_error(ToolErrorCode::Internal, e.to_string()))?;
        let artifacts = ArtifactStore::new(&config.artifacts, ARTIFACT_TTL)?;
        Ok(Self {
            config,
            checkpoints,
            artifacts,
            tasks: Mutex::new(HashMap::new()),
            opened: Mutex::new(Vec::new()),
            pool: Mutex::new(Vec::new()),
            launching: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
        })
    }

    pub fn writer_gate(&self) -> &WriterGate {
        &self.config.gate
    }

    pub fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }

    /// Number of live tool workers (never more than [`TOOL_WORKERS`]).
    pub fn worker_count(&self) -> usize {
        self.pool.lock().len()
    }

    /// Makes `identity` a task this backend answers for. `draft` is the stable draft.
    pub fn register_task(&self, identity: &TaskIdentity, draft: PathBuf, base: SourceRevision) {
        self.tasks.lock().insert(
            identity.task.clone(),
            TaskResources {
                identity: identity.clone(),
                draft,
                base,
                known: RevisionHistory::default(),
            },
        );
    }

    /// The task ended: its workers, restored trees and artifacts are released.
    pub fn unregister_task(&self, task: &AgentTaskId) {
        self.tasks.lock().remove(task);
        self.release(|t| t == task);
        self.artifacts.remove_task(task);
    }

    /// Releases every consumer. Later calls fail as stale.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.tasks.lock().clear();
        self.release(|_| true);
        self.artifacts.clear();
    }

    fn release(&self, matches: impl Fn(&AgentTaskId) -> bool) {
        let workers: Vec<PoolEntry> = {
            let mut pool = self.pool.lock();
            let (gone, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut *pool)
                .into_iter()
                .partition(|e| matches(&e.task));
            *pool = keep;
            gone
        };
        drop(workers);
        self.opened.lock().retain(|(t, _)| !matches(t));
    }

    fn task(&self, binding: &ToolBinding) -> Result<(PathBuf, SourceRevision), ToolError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(tool_error(
                ToolErrorCode::StaleTask,
                "tool backend is closed",
            ));
        }
        if binding.task.project != self.config.project_id {
            return Err(tool_error(
                ToolErrorCode::CrossProject,
                "the task belongs to another project",
            ));
        }
        let tasks = self.tasks.lock();
        match tasks.get(&binding.task.task) {
            Some(t) if t.identity == binding.task => Ok((t.draft.clone(), t.base.clone())),
            _ => Err(tool_error(
                ToolErrorCode::StaleTask,
                "the task is not active (stale or closed)",
            )),
        }
    }

    fn remember(&self, task: &AgentTaskId, revision: &str) {
        if let Some(t) = self.tasks.lock().get_mut(task) {
            t.known.remember(revision);
        }
    }

    fn is_known(&self, task: &AgentTaskId, revision: &str) -> bool {
        self.tasks
            .lock()
            .get(task)
            .is_some_and(|t| t.known.contains(revision))
    }

    /// Resolve the immutable revision a call answers for.
    fn resolve(
        &self,
        binding: &ToolBinding,
        request: &ToolRequest,
        draft: &Path,
        base: &SourceRevision,
    ) -> Result<(Arc<Opened>, RevisionLabel), ToolError> {
        let task = &binding.task.task;
        let (revision, label) = match &binding.revision {
            // The frozen task base is evidence of the *before* state, never a candidate.
            BoundRevision::Fixed(revision) if revision == base => {
                (revision.clone(), RevisionLabel::TaskBase)
            }
            BoundRevision::Fixed(revision) => (revision.clone(), RevisionLabel::Candidate),
            BoundRevision::Draft => {
                if let Some(asserted) = &request.assertions.revision {
                    // A previously answered snapshot may be asked about again; anything
                    // else is not a revision this task knows.
                    if !self.is_known(task, asserted) {
                        return Err(tool_error(
                            ToolErrorCode::StaleRevision,
                            "the requested revision was not produced for this task",
                        ));
                    }
                    let revision =
                        SourceRevision::try_from(asserted.clone()).map_err(ToolError::invalid)?;
                    (revision, RevisionLabel::DraftSnapshot)
                } else {
                    let Some(_gate) = self.config.gate.try_acquire() else {
                        return Err(tool_error(
                            ToolErrorCode::Busy,
                            "the draft writer gate is held; retry shortly",
                        ));
                    };
                    let revision =
                        capture_draft_revision(&self.checkpoints, draft).map_err(|e| match e {
                            CandidateError::DraftChanged => tool_error(
                                ToolErrorCode::Busy,
                                "the draft is changing; retry once the writer pauses",
                            ),
                            other => tool_error(ToolErrorCode::Unavailable, other.to_string()),
                        })?;
                    (revision, RevisionLabel::DraftSnapshot)
                }
            }
        };
        let id = revision.as_str().to_owned();
        if let Some((_, opened)) = self
            .opened
            .lock()
            .iter()
            .find(|(t, o)| t == task && o.revision == id)
        {
            self.remember(task, &id);
            return Ok((opened.clone(), label));
        }
        let restored = restore_revision(&self.checkpoints, &revision)
            .map_err(|e| tool_error(ToolErrorCode::Unavailable, e.to_string()))?;
        // The agent may have edited studio.json: never serve another project's identity.
        if restored.project.manifest.project_id != self.config.project_id {
            return Err(tool_error(
                ToolErrorCode::CrossProject,
                "the revision's studio.json names a different project",
            ));
        }
        let opened = Arc::new(Opened {
            restored: Arc::new(restored),
            revision: id.clone(),
        });
        let mut cache = self.opened.lock();
        cache.push((task.clone(), opened.clone()));
        while cache.len() > MAX_OPEN_REVISIONS {
            cache.remove(0);
        }
        drop(cache);
        self.remember(task, &id);
        Ok((opened, label))
    }

    fn worker(
        &self,
        task: &TaskIdentity,
        opened: &Opened,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Arc<Mutex<ToolWorker>>, ToolError> {
        // Reserve capacity before compiling or launching anything, so concurrent calls
        // can never overshoot the worker bound even transiently.
        let reservation = {
            let mut evicted = Vec::new();
            let mut pool = self.pool.lock();
            if let Some(entry) = pool
                .iter_mut()
                .find(|e| e.task == task.task && e.revision == opened.revision)
            {
                entry.last_used = Instant::now();
                return Ok(entry.worker.clone());
            }
            while pool.len() + self.launching.load(Ordering::Acquire) >= TOOL_WORKERS {
                // Only an idle worker (no call holds it) may be recycled.
                let idle = pool
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| Arc::strong_count(&e.worker) == 1)
                    .min_by_key(|(_, e)| e.last_used)
                    .map(|(i, _)| i);
                match idle {
                    Some(i) => evicted.push(pool.remove(i)),
                    None => {
                        return Err(tool_error(
                            ToolErrorCode::Busy,
                            "all tool workers are in use",
                        ));
                    }
                }
            }
            self.launching.fetch_add(1, Ordering::AcqRel);
            drop(pool);
            drop(evicted);
            LaunchSlot(&self.launching)
        };
        let project = &opened.restored.project;
        let environment = CompileEnvironment::resolve(
            &self.config.sdk,
            &self.config.compatibility,
            &self.config.builds,
        )
        .map_err(|e| tool_error(ToolErrorCode::Unavailable, e))?;
        let key = BuildKey::worker(project, &environment);
        let request = CompileRequest {
            project: project.clone(),
            environment,
            retained: Some(opened.restored.clone() as Arc<dyn Any + Send + Sync>),
        };
        let subscriber =
            Subscriber::new(SubscriberKind::Tool, format!("tool task {}", task.task.0));
        let lease = self
            .config
            .service
            .subscribe(key, request, subscriber)
            .map_err(build_error)?
            .wait(&|| cancelled() || self.closed.load(Ordering::Acquire))
            .map_err(build_error)?;
        let scope = self.config.processes.sub_manager();
        let identity = PreviewIdentity {
            project_id: String::from(task.project.clone()),
            open_session: uuid::Uuid::from_bytes(task.session.0).to_string(),
            source_revision: opened.revision.clone(),
            worker_generation: allocate_worker_generation(),
        };
        let mut client = launch_preview_worker(lease.into_build(), identity, &scope)
            .map_err(|e| tool_error(ToolErrorCode::BuildFailed, e))?;
        let timeline = client
            .timeline()
            .map_err(|e| tool_error(ToolErrorCode::BuildFailed, e.to_string()))?;
        let worker = Arc::new(Mutex::new(ToolWorker {
            client,
            scope,
            timeline,
        }));
        let mut pool = self.pool.lock();
        // The task may have ended while this worker launched. Checked under the pool
        // lock: `unregister_task` removes the task first and sweeps the pool second, so
        // a worker that passes this check is always swept later.
        if self.closed.load(Ordering::Acquire)
            || !self
                .tasks
                .lock()
                .get(&task.task)
                .is_some_and(|t| t.identity == *task)
        {
            drop(pool);
            return Err(tool_error(
                ToolErrorCode::StaleTask,
                "the task ended while its worker was starting",
            ));
        }
        pool.push(PoolEntry {
            task: task.task.clone(),
            revision: opened.revision.clone(),
            worker: worker.clone(),
            last_used: Instant::now(),
        });
        drop(pool);
        drop(reservation);
        Ok(worker)
    }

    /// Stores a PNG artifact for a live task only: a task that ended while the frame was
    /// rendering leaves nothing behind.
    fn store_png(
        &self,
        binding: &ToolBinding,
        bytes: &[u8],
        width: u32,
        height: u32,
    ) -> Result<ArtifactRef, ToolError> {
        let task = &binding.task.task;
        let artifact = self.artifacts.put_png(task, bytes, width, height)?;
        if !self.is_live(&binding.task) {
            self.artifacts.remove_task(task);
            return Err(tool_error(
                ToolErrorCode::StaleTask,
                "the task ended while the image was rendering",
            ));
        }
        Ok(artifact)
    }

    fn info(&self, opened: &Opened, label: RevisionLabel) -> RevisionInfo {
        RevisionInfo {
            id: opened.revision.clone(),
            label,
            validated: false,
        }
    }

    fn project_context(
        &self,
        task: &TaskIdentity,
        base: &SourceRevision,
        opened: &Opened,
        label: RevisionLabel,
    ) -> ToolReply {
        let project = &opened.restored.project;
        let files: Vec<Value> = project
            .inventory
            .files
            .iter()
            .take(MAX_CONTEXT_FILES)
            .map(|f| {
                json!({
                    "path": f.path.as_str(),
                    "kind": f.kind,
                    "size": f.size,
                    "sha256": f.sha256,
                    "executable": f.executable,
                })
            })
            .collect();
        let paths_of = |kind: FileKind| -> Vec<&str> {
            project
                .inventory
                .files
                .iter()
                .filter(|f| f.kind == kind)
                .take(MAX_CONTEXT_FILES)
                .map(|f| f.path.as_str())
                .collect()
        };
        ToolReply {
            revision: self.info(opened, label),
            result: json!({
                "project": {
                    "id": String::from(project.manifest.project_id.clone()),
                    "display": project.manifest.display,
                    "sdk": project.manifest.sdk,
                    "entry": project.manifest.entry,
                    "worker_available": project.worker_available,
                    "video_hints": project.manifest.video_hints,
                },
                "task": {
                    "id": task.task.0,
                    "generation": task.generation,
                    "base_revision": base.as_str(),
                },
                "files": files,
                "files_total": project.inventory.files.len(),
                "files_truncated": project.inventory.files.len() > MAX_CONTEXT_FILES,
                "assets": paths_of(FileKind::Media),
                "instructions": paths_of(FileKind::Instructions),
            }),
            artifacts: vec![],
        }
    }

    fn build_status(&self, binding: &ToolBinding, base: &SourceRevision) -> ToolReply {
        let project_id = String::from(self.config.project_id.clone());
        let (id, label, known): (String, RevisionLabel, KnownSummary) = match &binding.revision {
            BoundRevision::Fixed(r) => (
                r.as_str().to_owned(),
                RevisionLabel::Candidate,
                KnownSummary {
                    recent: vec![r.as_str().to_owned()],
                    total: 1,
                },
            ),
            BoundRevision::Draft => (
                base.as_str().to_owned(),
                RevisionLabel::TaskBase,
                self.tasks
                    .lock()
                    .get(&binding.task.task)
                    .map(|t| t.known.summary(STATUS_KNOWN_REVISIONS))
                    .unwrap_or_default(),
            ),
        };
        let builds = self.config.service.project_status(&project_id);
        let stats = self.config.service.stats();
        ToolReply {
            revision: RevisionInfo {
                id,
                label,
                validated: false,
            },
            result: json!({
                "builds": builds,
                "known_revisions": known.recent,
                "known_revisions_total": known.total,
                "known_revisions_truncated": (known.recent.len() as u64) < known.total,
                "service": stats,
                "tool_workers": self.worker_count(),
                "validated": false,
                "note": "Tool builds are unvalidated until the acceptance validator runs; this call never starts a compile.",
            }),
            artifacts: vec![],
        }
    }
}

fn png(width: u32, height: u32, pixels: Vec<u8>) -> Result<Vec<u8>, ToolError> {
    let image = image::RgbaImage::from_raw(width, height, pixels).ok_or_else(|| {
        tool_error(
            ToolErrorCode::Internal,
            "frame buffer does not match its size",
        )
    })?;
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| tool_error(ToolErrorCode::Internal, e.to_string()))?;
    Ok(out.into_inner())
}

fn timeline_body(timeline: &PreviewTimelineResponse) -> Value {
    json!({
        "fps": timeline.fps,
        "width": timeline.width,
        "height": timeline.height,
        "total_frames": timeline.total_frames,
        "duration_seconds": timeline.duration_seconds,
        "scenes": timeline.scenes,
        "audio_tracks": timeline.audio_tracks,
    })
}

fn effective(timeline: &PreviewTimelineResponse, scale: f64) -> f64 {
    scale.min(
        (f64::from(MAX_PREVIEW_WIDTH) / timeline.width as f64)
            .min(f64::from(MAX_PREVIEW_HEIGHT) / timeline.height as f64)
            .min(1.),
    )
}

impl ToolBackend for ProjectToolBackend {
    fn execute(
        &self,
        binding: &ToolBinding,
        request: &ToolRequest,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<ToolReply, ToolError> {
        let (draft, base) = self.task(binding)?;
        if matches!(request.call, ToolCall::BuildStatus) {
            return Ok(self.build_status(binding, &base));
        }
        let (opened, label) = self.resolve(binding, request, &draft, &base)?;
        if let ToolCall::ProjectContext = request.call {
            return Ok(self.project_context(&binding.task, &base, &opened, label));
        }
        let worker = self.worker(&binding.task, &opened, cancelled)?;
        if cancelled() {
            return Err(tool_error(ToolErrorCode::Cancelled, "call cancelled"));
        }
        let mut worker = worker.lock();
        let timeline = worker.timeline.clone();
        let info = self.info(&opened, label);
        match &request.call {
            ToolCall::Timeline => Ok(ToolReply {
                revision: info,
                result: timeline_body(&timeline),
                artifacts: vec![],
            }),
            ToolCall::RenderFrame { frame, scale } => {
                if *frame >= timeline.total_frames {
                    return Err(ToolError::invalid(format!(
                        "frame {frame} is outside the compiled timeline (0..{})",
                        timeline.total_frames
                    )));
                }
                let rendered = worker
                    .client
                    .frame(*frame, 0, effective(&timeline, *scale))
                    .map_err(|e| tool_error(ToolErrorCode::Unavailable, e.to_string()))?;
                let (w, h) = (
                    rendered.response.header.width,
                    rendered.response.header.height,
                );
                let bytes = png(w, h, rendered.pixels)?;
                let artifact = self.store_png(binding, &bytes, w, h)?;
                Ok(ToolReply {
                    revision: info,
                    result: json!({"frame": frame, "width": w, "height": h}),
                    artifacts: vec![artifact],
                })
            }
            ToolCall::RenderStrip {
                start,
                end,
                count,
                scale,
            } => {
                let frames = super::FrameSelection::Range {
                    start: *start,
                    end: *end,
                    count: *count,
                }
                .resolve(timeline.total_frames)?;
                // Keep the whole strip within a bounded width.
                let per_tile =
                    (MAX_STRIP_WIDTH as f64 / frames.len() as f64) / timeline.width as f64;
                let scale = effective(&timeline, scale.min(per_tile.max(0.01)));
                let mut tiles = Vec::with_capacity(frames.len());
                for frame in &frames {
                    if cancelled() {
                        return Err(tool_error(ToolErrorCode::Cancelled, "call cancelled"));
                    }
                    tiles.push(
                        worker
                            .client
                            .frame(*frame, 0, scale)
                            .map_err(|e| tool_error(ToolErrorCode::Unavailable, e.to_string()))?,
                    );
                }
                let height = tiles
                    .iter()
                    .map(|t| t.response.header.height)
                    .max()
                    .unwrap_or(0);
                let width: u32 = tiles.iter().map(|t| t.response.header.width).sum();
                let mut strip = vec![0u8; width as usize * height as usize * 4];
                let mut x = 0usize;
                for tile in &tiles {
                    let (tw, th) = (
                        tile.response.header.width as usize,
                        tile.response.header.height as usize,
                    );
                    for row in 0..th {
                        let from = row * tw * 4;
                        let to = (row * width as usize + x) * 4;
                        strip[to..to + tw * 4].copy_from_slice(&tile.pixels[from..from + tw * 4]);
                    }
                    x += tw;
                }
                let bytes = png(width, height, strip)?;
                let artifact = self.store_png(binding, &bytes, width, height)?;
                Ok(ToolReply {
                    revision: info,
                    result: json!({"frames": frames, "tile_scale": scale, "width": width, "height": height}),
                    artifacts: vec![artifact],
                })
            }
            ToolCall::Inspect(selection) => {
                let frames = selection.resolve(timeline.total_frames)?;
                let response = worker
                    .client
                    .inspect(frames.clone())
                    .map_err(|e| tool_error(ToolErrorCode::Unavailable, e.to_string()))?;
                Ok(ToolReply {
                    revision: info,
                    result: json!({
                        "frames": frames,
                        "diagnostics": response.diagnostics,
                        "truncated": response.truncated,
                    }),
                    artifacts: vec![],
                })
            }
            ToolCall::ProjectContext | ToolCall::BuildStatus => unreachable!("handled above"),
        }
    }
}

impl TaskLiveness for ProjectToolBackend {
    fn is_live(&self, task: &TaskIdentity) -> bool {
        !self.closed.load(Ordering::Acquire)
            && self
                .tasks
                .lock()
                .get(&task.task)
                .is_some_and(|t| &t.identity == task)
    }
}
