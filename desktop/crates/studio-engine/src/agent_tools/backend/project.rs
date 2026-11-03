//! [`ProjectToolBackend`]: configuration, revision history, tool workers and the
//! [`ToolBackend`] implementation.
use super::writer_gate::LaunchSlot;
use super::*;

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
    source_indexes: VecDeque<Arc<SourceIndex>>,
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
                source_indexes: VecDeque::new(),
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

    fn source_index(
        &self,
        task: &AgentTaskId,
        opened: &Opened,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Arc<SourceIndex>, ToolError> {
        if let Some(index) = self.tasks.lock().get(task).and_then(|resources| {
            resources
                .source_indexes
                .iter()
                .find(|index| index.revision.as_str() == opened.revision)
                .cloned()
        }) {
            return Ok(index);
        }

        let inventory = &opened.restored.project.inventory;
        let mut inputs = Vec::new();
        let mut admitted_files = 0usize;
        let mut admitted_bytes = 0usize;
        let mut truncated_files = 0usize;
        let mut truncated_bytes = 0usize;
        for file in inventory
            .files
            .iter()
            .filter(|file| file.kind == FileKind::Rust)
        {
            if cancelled() {
                return Err(tool_error(
                    ToolErrorCode::Cancelled,
                    "source indexing cancelled",
                ));
            }
            let file_bytes = usize::try_from(file.size).unwrap_or(usize::MAX);
            if file_bytes > MAX_INDEX_FILE_BYTES
                || admitted_files >= MAX_INDEX_RUST_FILES
                || admitted_bytes.saturating_add(file_bytes) > MAX_INDEX_TOTAL_BYTES
            {
                truncated_files += 1;
                truncated_bytes = truncated_bytes.saturating_add(file_bytes);
                continue;
            }
            let mut bytes = Vec::with_capacity(file_bytes);
            self.checkpoints
                .copy_object(&file.sha256, file.size, &mut bytes)
                .map_err(|error| tool_error(ToolErrorCode::Unavailable, error.to_string()))?;
            admitted_files += 1;
            admitted_bytes += file_bytes;
            inputs.push(SourceIndexInput {
                path: file.path.clone(),
                kind: file.kind,
                expected_sha256: file.sha256.clone(),
                expected_size: file.size,
                bytes,
            });
        }
        let mut index = SourceIndex::build(inventory.revision.clone(), inputs, &|| cancelled())
            .map_err(source_lookup_error)?;
        index.truncated_files += truncated_files;
        index.truncated_bytes += truncated_bytes;
        let index = Arc::new(index);
        let mut tasks = self.tasks.lock();
        let resources = tasks.get_mut(task).ok_or_else(|| {
            tool_error(
                ToolErrorCode::StaleTask,
                "the task ended during source indexing",
            )
        })?;
        Ok(retain_source_index(&mut resources.source_indexes, index))
    }

    fn source_lookup(
        &self,
        binding: &ToolBinding,
        opened: &Opened,
        label: RevisionLabel,
        request: SourceLookupRequest<'_>,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<ToolReply, ToolError> {
        let SourceLookupRequest {
            path,
            symbol,
            marker,
            object_identity,
            registered_style_tokens,
        } = request;
        let source = opened
            .restored
            .project
            .inventory
            .files
            .iter()
            .find(|file| &file.path == path && file.kind == FileKind::Rust)
            .ok_or_else(|| {
                tool_error(
                    ToolErrorCode::NotFound,
                    "Rust source path is not in this immutable revision",
                )
            })?;
        let index = self.source_index(&binding.task.task, opened, cancelled)?;
        let anchor = SourceAnchor {
            path: path.clone(),
            symbol: symbol.to_owned(),
            expected_sha256: source.sha256.clone(),
            marker: marker.map(str::to_owned),
        };
        let (lookup, on_demand) = match index.lookup(&anchor) {
            Ok(lookup) => (lookup, false),
            Err(studio_project::source_index::SourceIndexError::InvalidAnchor(message))
                if message == "source file is not indexed" =>
            {
                if let Some(diagnostic) = source_parse_diagnostic(&index, path) {
                    return Err(tool_error(
                        ToolErrorCode::Unavailable,
                        format!("Rust source could not be indexed: {diagnostic}"),
                    ));
                }
                if source.size > MAX_INDEX_FILE_BYTES as u64 {
                    return Err(tool_error(
                        ToolErrorCode::TooLarge,
                        "requested Rust file exceeds the 1 MiB on-demand limit",
                    ));
                }
                let mut bytes = Vec::with_capacity(source.size as usize);
                self.checkpoints
                    .copy_object(&source.sha256, source.size, &mut bytes)
                    .map_err(|error| tool_error(ToolErrorCode::Unavailable, error.to_string()))?;
                let one_file = SourceIndex::build(
                    opened.restored.project.inventory.revision.clone(),
                    vec![SourceIndexInput {
                        path: source.path.clone(),
                        kind: FileKind::Rust,
                        expected_sha256: source.sha256.clone(),
                        expected_size: source.size,
                        bytes,
                    }],
                    &|| cancelled(),
                )
                .map_err(source_lookup_error)?;
                if let Some(diagnostic) = source_parse_diagnostic(&one_file, path) {
                    return Err(tool_error(
                        ToolErrorCode::Unavailable,
                        format!("Rust source could not be indexed: {diagnostic}"),
                    ));
                }
                (one_file.lookup(&anchor).map_err(source_lookup_error)?, true)
            }
            Err(error) => return Err(source_lookup_error(error)),
        };
        Ok(ToolReply {
            revision: self.info(opened, label),
            result: json!({
                "schema_version": studio_project::source_index::SOURCE_INDEX_SCHEMA_VERSION,
                "object_identity": object_identity,
                "source_anchor": {"path": path, "symbol": symbol, "marker": marker},
                "registered_style_tokens": registered_style_tokens,
                "index_status": if on_demand { "on_demand_file" } else { "revision_index" },
                "revision": lookup.revision,
                "snippets": lookup.snippets,
                "helper_candidates": lookup.helper_candidates,
                "ambiguous": lookup.ambiguous,
                "truncated": lookup.truncated,
                "diagnostics": lookup.diagnostics,
                "limitations": ["Syntax-only candidates do not prove type resolution, macro expansion, runtime dispatch, or cfg activation."],
            }),
            artifacts: vec![],
        })
    }

    fn style_context(
        &self,
        opened: &Opened,
        label: RevisionLabel,
        token_filter: Option<&str>,
        registered_bindings: Option<(&fframes_studio_protocol::EditorObjectIdentity, &[String])>,
    ) -> Result<ToolReply, ToolError> {
        let project = &opened.restored.project;
        let info = self.info(opened, label);
        let Some(style) =
            crate::preset_state::read_project_style(&project.root, &project.inventory)
        else {
            if project.manifest.preset.is_some() {
                return Err(tool_error(
                    ToolErrorCode::Unavailable,
                    "the applied preset snapshot is invalid or incomplete in this immutable revision",
                ));
            }
            return Ok(ToolReply {
                revision: info,
                result: json!({
                    "status": "no_active_preset",
                    "preset": null,
                    "tokens_sha256": null,
                    "tokens": [],
                    "bindings": registered_bindings.map_or_else(Vec::<Value>::new, |(_, names)| {
                        unresolved_style_bindings(names, token_filter)
                    }),
                    "binding_status": if registered_bindings.is_some_and(|(_, names)| !names.is_empty()) { "registered_unresolved" } else if registered_bindings.is_some() { "none_registered" } else { "not_registered" },
                    "object_identity": registered_bindings.map(|(identity, _)| identity),
                    "diagnostics": [],
                }),
                artifacts: vec![],
            });
        };
        let tokens_file = project
            .inventory
            .files
            .iter()
            .find(|file| file.path.as_str() == "style/tokens.json")
            .ok_or_else(|| {
                tool_error(
                    ToolErrorCode::Unavailable,
                    "resolved style tokens are missing from the immutable inventory",
                )
            })?;
        if tokens_file.size > 8 * 1024 * 1024 {
            return Err(tool_error(
                ToolErrorCode::TooLarge,
                "resolved style token snapshot exceeds the 8 MiB limit",
            ));
        }
        let mut bytes = Vec::with_capacity(tokens_file.size as usize);
        tokens_file
            .path
            .open_file(&project.root)
            .map_err(|error| tool_error(ToolErrorCode::Unavailable, error.to_string()))?
            .take(tokens_file.size.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| tool_error(ToolErrorCode::Unavailable, error.to_string()))?;
        if bytes.len() as u64 != tokens_file.size
            || format!("{:x}", Sha256::digest(&bytes)) != tokens_file.sha256
            || style.tokens_sha256.as_deref() != Some(tokens_file.sha256.as_str())
        {
            return Err(tool_error(
                ToolErrorCode::StaleRevision,
                "resolved token snapshot no longer matches the immutable source inventory",
            ));
        }
        let snapshot = studio_presets::ResolvedSnapshot::from_runtime_json(&bytes)
            .map_err(|error| tool_error(ToolErrorCode::Unavailable, error.to_string()))?;
        let tokens: Vec<Value> = snapshot
            .tokens()
            .iter()
            .filter(|(name, _)| token_filter.is_none_or(|filter| name.as_str() == filter))
            .map(|(name, value)| {
                let origin = style
                    .overridden
                    .iter()
                    .find(|(overridden, _, _)| overridden == name.as_str())
                    .map(|(_, layer, _)| layer.as_str())
                    .unwrap_or("preset_default");
                json!({
                    "name": name.as_str(),
                    "type": name.kind().as_str(),
                    "value": value,
                    "origin": origin,
                })
            })
            .collect();
        let bindings: Vec<Value> = registered_bindings
            .map(|(_, names)| {
                names
                    .iter()
                    .filter(|name| token_filter.is_none_or(|filter| name.as_str() == filter))
                    .map(|name| {
                        let Some((token_name, value)) = snapshot
                            .tokens()
                            .iter()
                            .find(|(token_name, _)| token_name.as_str() == name.as_str())
                        else {
                            return json!({"token": name, "status": "not_resolved"});
                        };
                        let origin = style
                            .overridden
                            .iter()
                            .find(|(overridden, _, _)| overridden == name.as_str())
                            .map(|(_, layer, _)| layer.as_str())
                            .unwrap_or("preset_default");
                        json!({
                            "token": name,
                            "status": "resolved",
                            "type": token_name.kind().as_str(),
                            "value": value,
                            "origin": origin,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(ToolReply {
            revision: info,
            result: json!({
                "status": "active_preset",
                "preset": {"id": style.identity.id, "sha256": style.identity.hash},
                "tokens_sha256": style.tokens_sha256,
                "tokens": tokens,
                "bindings": bindings,
                "binding_status": if registered_bindings.is_some_and(|(_, names)| !names.is_empty()) { "registered" } else if registered_bindings.is_some() { "none_registered" } else { "not_registered" },
                "object_identity": registered_bindings.map(|(identity, _)| identity),
                "diagnostics": style.diagnostics,
                "snapshot_diagnostics": snapshot.diagnostics().iter().map(ToString::to_string).collect::<Vec<_>>(),
            }),
            artifacts: vec![],
        })
    }

    fn selection_context(
        &self,
        opened: &Opened,
        label: RevisionLabel,
        worker: &mut ToolWorker,
        request: SelectionContextRequest<'_>,
    ) -> Result<ToolReply, ToolError> {
        let SelectionContextRequest {
            frame_index,
            seek_serial,
            wanted,
            expected_geometry_digest,
        } = request;
        if frame_index >= worker.timeline.total_frames {
            return Err(tool_error(
                ToolErrorCode::InvalidParams,
                format!(
                    "frame {frame_index} is outside the compiled timeline (0..{})",
                    worker.timeline.total_frames
                ),
            ));
        }
        let frame = worker
            .client
            .frame(frame_index, seek_serial, effective(&worker.timeline, 1.0))
            .map_err(|error| tool_error(ToolErrorCode::Unavailable, error.to_string()))?;
        let metadata = frame.response.editor_metadata.as_ref();
        if let Some(expected) = expected_geometry_digest
            && metadata.is_none_or(|metadata| metadata.frame_geometry_digest != expected)
        {
            return Err(tool_error(
                ToolErrorCode::StaleRevision,
                "displayed-frame geometry digest no longer matches the immutable source revision",
            ));
        }
        let selected = match (wanted, metadata) {
            (Some(identity), Some(metadata)) => Some(
                metadata
                    .objects
                    .iter()
                    .find(|object| &object.identity == identity)
                    .ok_or_else(|| {
                        tool_error(
                            ToolErrorCode::NotFound,
                            "selected semantic identity is absent from the requested source frame",
                        )
                    })?,
            ),
            (Some(_), None) => {
                return Err(tool_error(
                    ToolErrorCode::Unavailable,
                    "this preview worker does not provide semantic editor metadata",
                ));
            }
            (None, _) => None,
        };
        let active_scenes: Vec<_> = worker
            .timeline
            .scenes
            .iter()
            .filter(|scene| scene.start_frame <= frame_index && frame_index < scene.end_frame)
            .map(|scene| {
                json!({
                    "instance_id": scene.instance_id,
                    "editor_instance_key": scene.editor_instance_key,
                    "name": scene.name,
                    "start_frame": scene.start_frame,
                    "end_frame": scene.end_frame,
                })
            })
            .collect();
        let status = metadata.map_or("unavailable", |metadata| match metadata.status {
            EditorFrameStatus::Supported => "supported",
            EditorFrameStatus::Unannotated => "unannotated",
            EditorFrameStatus::Invalid => "invalid",
        });
        let object_count = metadata.map_or(0, |metadata| metadata.objects.len());
        let objects = metadata.map(|metadata| {
            metadata
                .objects
                .iter()
                .take(MAX_SELECTION_CONTEXT_OBJECTS)
                .collect::<Vec<_>>()
        });
        Ok(ToolReply {
            revision: self.info(opened, label),
            result: json!({
                "preview_identity": worker.client.identity(),
                "frame": frame_index,
                "seek_serial": seek_serial,
                "request_id": frame.response.envelope.request_id,
                "video_dimensions": metadata.map(|metadata| [metadata.video_width, metadata.video_height]),
                "raster_dimensions": [frame.response.header.width, frame.response.header.height],
                "editor_index_digest": metadata.map(|metadata| &metadata.editor_index_digest),
                "frame_geometry_digest": metadata.map(|metadata| &metadata.frame_geometry_digest),
                "status": status,
                "reason": metadata.and_then(|metadata| metadata.reason.as_deref()),
                "selected": selected,
                "objects": objects,
                "object_count": object_count,
                "objects_truncated": object_count > MAX_SELECTION_CONTEXT_OBJECTS,
                "active_scenes": active_scenes,
                "source_anchor_status": if selected.and_then(|object| object.source_anchor.as_ref()).is_some() { "registered" } else { "not_registered" },
                "registered_style_tokens": selected.map(|object| &object.style_tokens),
                "note": "Geometry is recomputed from this immutable revision and exact frame/seek request. It is not evidence that the live canvas is unchanged unless the supplied geometry digest matches.",
            }),
            artifacts: vec![],
        })
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
        let reads_frozen_context = matches!(
            request.call,
            ToolCall::SelectionContext { .. }
                | ToolCall::SourceLookup { .. }
                | ToolCall::StyleContext { .. }
        );
        let base_asserted = request
            .assertions
            .revision
            .as_deref()
            .is_none_or(|revision| revision == base.as_str());
        let mut resolution_binding = binding.clone();
        if reads_frozen_context && base_asserted && matches!(binding.revision, BoundRevision::Draft)
        {
            // Context tools default to the immutable task base, so source/style/selection
            // queries remain available while the writer owns the mutable draft.
            resolution_binding.revision = BoundRevision::Fixed(base.clone());
        }
        let (opened, label) = self.resolve(&resolution_binding, request, &draft, &base)?;
        if let ToolCall::ProjectContext = request.call {
            return Ok(self.project_context(&binding.task, &base, &opened, label));
        }
        if let ToolCall::Captions {
            path,
            offset,
            limit,
        } = &request.call
        {
            let project = &opened.restored.project;
            return Ok(ToolReply {
                revision: self.info(&opened, label),
                result: super::captions::captions(
                    &project.root,
                    &project.inventory.files,
                    path.as_ref(),
                    *offset,
                    *limit,
                    cancelled,
                )?,
                artifacts: vec![],
            });
        }
        if let ToolCall::SourceLookup {
            path,
            symbol,
            marker,
            frame,
            seek_serial,
            identity,
            frame_geometry_digest,
        } = &request.call
        {
            let (path, symbol, marker, registered_style_tokens) = match (path, symbol) {
                (Some(path), Some(symbol)) => (path.clone(), symbol.clone(), marker.clone(), None),
                (None, None) => {
                    let (Some(frame), Some(seek_serial), Some(identity)) =
                        (frame, seek_serial, identity.as_ref())
                    else {
                        return Err(tool_error(
                            ToolErrorCode::InvalidParams,
                            "source lookup requires an explicit anchor or selected object",
                        ));
                    };
                    let worker = self.worker(&binding.task, &opened, cancelled)?;
                    let mut worker = worker.lock();
                    let context = self.selection_context(
                        &opened,
                        label,
                        &mut worker,
                        SelectionContextRequest {
                            frame_index: *frame,
                            seek_serial: *seek_serial,
                            wanted: Some(identity),
                            expected_geometry_digest: frame_geometry_digest.as_deref(),
                        },
                    )?;
                    let selected = &context.result["selected"];
                    let source_anchor: EditorSourceAnchor = serde_json::from_value(
                        selected["source_anchor"].clone(),
                    )
                    .map_err(|_| {
                        tool_error(
                            ToolErrorCode::NotFound,
                            "selected object has no registered source anchor",
                        )
                    })?;
                    let registered_style_tokens: Vec<String> =
                        serde_json::from_value(selected["style_tokens"].clone()).map_err(|_| {
                            tool_error(
                                ToolErrorCode::Unavailable,
                                "selected object has invalid registered style bindings",
                            )
                        })?;
                    let path = ProjectPath::try_from(source_anchor.path)
                        .map_err(|error| tool_error(ToolErrorCode::NotFound, error.to_string()))?;
                    (
                        path,
                        source_anchor.symbol,
                        source_anchor.marker,
                        Some(registered_style_tokens),
                    )
                }
                _ => {
                    return Err(tool_error(
                        ToolErrorCode::InvalidParams,
                        "source lookup requires both path and symbol",
                    ));
                }
            };
            return self.source_lookup(
                binding,
                &opened,
                label,
                SourceLookupRequest {
                    path: &path,
                    symbol: &symbol,
                    marker: marker.as_deref(),
                    object_identity: identity.as_ref(),
                    registered_style_tokens: registered_style_tokens.as_deref(),
                },
                cancelled,
            );
        }
        if let ToolCall::StyleContext {
            token,
            frame,
            seek_serial,
            identity,
            frame_geometry_digest,
        } = &request.call
        {
            let registered_bindings = if let Some(identity) = identity {
                let (Some(frame), Some(seek_serial)) = (frame, seek_serial) else {
                    return Err(tool_error(
                        ToolErrorCode::InvalidParams,
                        "object style lookup requires frame and seek serial",
                    ));
                };
                let worker = self.worker(&binding.task, &opened, cancelled)?;
                let mut worker = worker.lock();
                let context = self.selection_context(
                    &opened,
                    label,
                    &mut worker,
                    SelectionContextRequest {
                        frame_index: *frame,
                        seek_serial: *seek_serial,
                        wanted: Some(identity),
                        expected_geometry_digest: frame_geometry_digest.as_deref(),
                    },
                )?;
                let bindings: Vec<String> =
                    serde_json::from_value(context.result["selected"]["style_tokens"].clone())
                        .map_err(|_| {
                            tool_error(
                                ToolErrorCode::Unavailable,
                                "selected object has invalid registered style bindings",
                            )
                        })?;
                Some((identity.clone(), bindings))
            } else {
                None
            };
            let binding_refs = registered_bindings
                .as_ref()
                .map(|(identity, names)| (identity, names.as_slice()));
            return self.style_context(&opened, label, token.as_deref(), binding_refs);
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
                let frames = crate::agent_tools::FrameSelection::Range {
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
            ToolCall::SelectionContext {
                frame,
                seek_serial,
                identity,
                frame_geometry_digest,
            } => self.selection_context(
                &opened,
                label,
                &mut worker,
                SelectionContextRequest {
                    frame_index: *frame,
                    seek_serial: *seek_serial,
                    wanted: identity.as_ref(),
                    expected_geometry_digest: frame_geometry_digest.as_deref(),
                },
            ),
            ToolCall::ProjectContext
            | ToolCall::BuildStatus
            | ToolCall::SourceLookup { .. }
            | ToolCall::StyleContext { .. }
            | ToolCall::Captions { .. } => unreachable!("handled above"),
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
