//! The nine project tools an agent may call, behind one typed dispatcher.
//!
//! `project_context`, `timeline`, `render_frame`, `render_strip`, `inspect`,
//! `build_status`, `selection_context`, `source_lookup` and `style_context` are the whole
//! surface: there is no shell, no filesystem write, no
//! generalized filesystem access and no export tool. The app, the
//! `studio-tools` CLI and the `studio-mcp` stdio server all reach the same
//! [`ToolDispatcher`] through the app-owned local broker ([`broker`]); the facades hold
//! no compiler and never launch Cargo.
//!
//! Every call is bound to one task, its project and a revision ([`ToolBinding`]).
//! Replies are bounded (256 KiB of JSON text); images are app-owned artifacts
//! (id, path, hash, expiry, at most 8 MiB), never inline base64.
use fframes_studio_protocol::MAX_INSPECT_FRAMES;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use studio_engine::TaskIdentity;
use studio_project::SourceRevision;

pub mod backend;
pub mod broker;
pub mod client;
pub mod mcp;

pub const TOOL_NAMES: [&str; 9] = [
    "project_context",
    "timeline",
    "render_frame",
    "render_strip",
    "inspect",
    "build_status",
    "selection_context",
    "source_lookup",
    "style_context",
];
/// Queued tool calls (waiting for one of the tool workers) before `busy`.
pub const MAX_QUEUED_CALLS: usize = 16;
/// Concurrent tool executions; tool workers never use the displayed/thumbnail lanes.
pub const TOOL_WORKERS: usize = 2;
pub const MAX_IMAGE_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TEXT_REPLY_BYTES: usize = 256 * 1024;
/// One request line on the broker wire.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_STRIP_FRAMES: usize = 24;
pub const MAX_INSPECT_FRAMES_PER_CALL: usize = MAX_INSPECT_FRAMES;
pub const ARTIFACT_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_ERROR_CHARS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorCode {
    InvalidParams,
    MethodNotFound,
    Unauthorized,
    Expired,
    StaleTask,
    CrossProject,
    StaleRevision,
    Busy,
    NotFound,
    TooLarge,
    Unavailable,
    BuildFailed,
    Cancelled,
    Internal,
}

/// Same shape on the broker wire, the CLI (stderr/exit code) and MCP (`isError`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct ToolError {
    pub code: ToolErrorCode,
    pub message: String,
}

impl ToolError {
    pub fn new(code: ToolErrorCode, message: impl Into<String>) -> Self {
        let message: String = message.into();
        Self {
            code,
            message: message.chars().take(MAX_ERROR_CHARS).collect(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ToolErrorCode::InvalidParams, message)
    }
}

/// What a capability is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolBinding {
    pub task: TaskIdentity,
    pub revision: BoundRevision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoundRevision {
    /// The task's mutable stable draft: each call acquires the writer gate and captures
    /// a labelled immutable draft revision (or returns `busy`).
    Draft,
    /// One immutable revision (a captured candidate).
    Fixed(SourceRevision),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionLabel {
    /// An immutable capture of the mutable draft taken for this call.
    DraftSnapshot,
    /// The quiesced candidate under validation.
    Candidate,
    /// The task's frozen source base: reported by `build_status` and, for a fixed binding
    /// to exactly that revision, by the before-evidence renders.
    TaskBase,
}

/// The exact immutable revision a reply describes. `validated` is always false for tool
/// builds: only the acceptance validator can qualify a task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionInfo {
    pub id: String,
    pub label: RevisionLabel,
    pub validated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// App-owned id (`art-<hex>`).
    pub id: String,
    pub media_type: String,
    /// Absolute path under the app artifact store; expires with the artifact.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub expires_at_unix: u64,
}

/// Common optional assertions: a caller may state which project/revision it believes it
/// is talking to; a mismatch is rejected instead of silently answered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Assertions {
    pub project: Option<String>,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FrameSelection {
    List(Vec<usize>),
    /// `count` evenly spaced frames from `start` to `end` inclusive.
    Range {
        start: usize,
        end: usize,
        count: usize,
    },
}

impl FrameSelection {
    /// Explicit, ascending, deduplicated frames. `total_frames` bounds every index.
    pub fn resolve(&self, total_frames: usize) -> Result<Vec<usize>, ToolError> {
        let mut frames = match self {
            Self::List(list) => list.clone(),
            Self::Range { start, end, count } => {
                if *count == 1 {
                    vec![*start]
                } else {
                    (0..*count)
                        .map(|i| start + (end - start) * i / (count - 1))
                        .collect()
                }
            }
        };
        if let Some(bad) = frames.iter().find(|f| **f >= total_frames) {
            return Err(ToolError::invalid(format!(
                "frame {bad} is outside the compiled timeline (0..{total_frames})"
            )));
        }
        frames.sort_unstable();
        frames.dedup();
        Ok(frames)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolCall {
    ProjectContext,
    Timeline,
    RenderFrame {
        frame: usize,
        scale: f64,
    },
    RenderStrip {
        start: usize,
        end: usize,
        count: usize,
        scale: f64,
    },
    Inspect(FrameSelection),
    BuildStatus,
    SelectionContext {
        frame: usize,
        seek_serial: u64,
        identity: Option<fframes_studio_protocol::EditorObjectIdentity>,
        frame_geometry_digest: Option<String>,
    },
    SourceLookup {
        path: Option<studio_project::ProjectPath>,
        symbol: Option<String>,
        marker: Option<String>,
        frame: Option<usize>,
        seek_serial: Option<u64>,
        identity: Option<fframes_studio_protocol::EditorObjectIdentity>,
        frame_geometry_digest: Option<String>,
    },
    StyleContext {
        token: Option<String>,
        frame: Option<usize>,
        seek_serial: Option<u64>,
        identity: Option<fframes_studio_protocol::EditorObjectIdentity>,
        frame_geometry_digest: Option<String>,
    },
}

impl ToolCall {
    pub fn name(&self) -> &'static str {
        match self {
            Self::ProjectContext => "project_context",
            Self::Timeline => "timeline",
            Self::RenderFrame { .. } => "render_frame",
            Self::RenderStrip { .. } => "render_strip",
            Self::Inspect(_) => "inspect",
            Self::BuildStatus => "build_status",
            Self::SelectionContext { .. } => "selection_context",
            Self::SourceLookup { .. } => "source_lookup",
            Self::StyleContext { .. } => "style_context",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolRequest {
    pub assertions: Assertions,
    pub call: ToolCall,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommonOnly {
    project: Option<String>,
    revision: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameParams {
    project: Option<String>,
    revision: Option<String>,
    frame: u64,
    scale: Option<f64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StripParams {
    project: Option<String>,
    revision: Option<String>,
    start: u64,
    end: u64,
    count: u64,
    scale: Option<f64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InspectParams {
    project: Option<String>,
    revision: Option<String>,
    frames: Option<Vec<u64>>,
    start: Option<u64>,
    end: Option<u64>,
    count: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionParams {
    project: Option<String>,
    revision: Option<String>,
    frame: u64,
    seek_serial: u64,
    scene_instance_key: Option<String>,
    component_key: Option<String>,
    object_key: Option<String>,
    repeat_key: Option<String>,
    frame_geometry_digest: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceLookupParams {
    project: Option<String>,
    revision: Option<String>,
    path: Option<String>,
    symbol: Option<String>,
    marker: Option<String>,
    frame: Option<u64>,
    seek_serial: Option<u64>,
    scene_instance_key: Option<String>,
    component_key: Option<String>,
    object_key: Option<String>,
    repeat_key: Option<String>,
    frame_geometry_digest: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StyleParams {
    project: Option<String>,
    revision: Option<String>,
    token: Option<String>,
    frame: Option<u64>,
    seek_serial: Option<u64>,
    scene_instance_key: Option<String>,
    component_key: Option<String>,
    object_key: Option<String>,
    repeat_key: Option<String>,
    frame_geometry_digest: Option<String>,
}

pub const DEFAULT_RENDER_SCALE: f64 = 0.5;

fn parsed<T: for<'de> Deserialize<'de>>(params: &Value) -> Result<T, ToolError> {
    let params = if params.is_null() {
        json!({})
    } else {
        params.clone()
    };
    serde_json::from_value(params).map_err(|e| ToolError::invalid(e.to_string()))
}

fn index(value: u64, what: &str) -> Result<usize, ToolError> {
    // Frame indexes beyond a billion cannot name a bounded preview timeline.
    if value > 1_000_000_000 {
        return Err(ToolError::invalid(format!("{what} is out of range")));
    }
    Ok(value as usize)
}

fn scale_of(scale: Option<f64>) -> Result<f64, ToolError> {
    let scale = scale.unwrap_or(DEFAULT_RENDER_SCALE);
    if !scale.is_finite() || scale <= 0. || scale > 1. {
        return Err(ToolError::invalid("scale must be within (0, 1]"));
    }
    Ok(scale)
}

impl ToolRequest {
    /// Parse and bound one call. Unknown methods and unknown fields are rejected; ranges
    /// are bounded here, before any backend work.
    pub fn parse(method: &str, params: &Value) -> Result<Self, ToolError> {
        if !params.is_object() && !params.is_null() {
            return Err(ToolError::invalid("params must be an object"));
        }
        let (assertions, call) = match method {
            "project_context" | "timeline" | "build_status" => {
                let p: CommonOnly = parsed(params)?;
                let call = match method {
                    "project_context" => ToolCall::ProjectContext,
                    "timeline" => ToolCall::Timeline,
                    _ => ToolCall::BuildStatus,
                };
                (
                    Assertions {
                        project: p.project,
                        revision: p.revision,
                    },
                    call,
                )
            }
            "render_frame" => {
                let p: FrameParams = parsed(params)?;
                (
                    Assertions {
                        project: p.project,
                        revision: p.revision,
                    },
                    ToolCall::RenderFrame {
                        frame: index(p.frame, "frame")?,
                        scale: scale_of(p.scale)?,
                    },
                )
            }
            "render_strip" => {
                let p: StripParams = parsed(params)?;
                let (start, end) = (index(p.start, "start")?, index(p.end, "end")?);
                if end < start {
                    return Err(ToolError::invalid("end must not precede start"));
                }
                if p.count == 0 || p.count as usize > MAX_STRIP_FRAMES {
                    return Err(ToolError::invalid(format!(
                        "count must be 1..={MAX_STRIP_FRAMES}"
                    )));
                }
                (
                    Assertions {
                        project: p.project,
                        revision: p.revision,
                    },
                    ToolCall::RenderStrip {
                        start,
                        end,
                        count: p.count as usize,
                        scale: scale_of(p.scale)?,
                    },
                )
            }
            "inspect" => {
                let p: InspectParams = parsed(params)?;
                let selection = match (p.frames, p.start, p.end, p.count) {
                    (Some(frames), None, None, None) => {
                        if frames.is_empty() || frames.len() > MAX_INSPECT_FRAMES_PER_CALL {
                            return Err(ToolError::invalid(format!(
                                "frames must list 1..={MAX_INSPECT_FRAMES_PER_CALL} indexes"
                            )));
                        }
                        FrameSelection::List(
                            frames
                                .into_iter()
                                .map(|f| index(f, "frame"))
                                .collect::<Result<_, _>>()?,
                        )
                    }
                    (None, Some(start), Some(end), Some(count)) => {
                        let (start, end) = (index(start, "start")?, index(end, "end")?);
                        if end < start || count == 0 || count as usize > MAX_INSPECT_FRAMES_PER_CALL
                        {
                            return Err(ToolError::invalid(format!(
                                "range needs start <= end and count 1..={MAX_INSPECT_FRAMES_PER_CALL}"
                            )));
                        }
                        FrameSelection::Range {
                            start,
                            end,
                            count: count as usize,
                        }
                    }
                    _ => {
                        return Err(ToolError::invalid(
                            "inspect needs either `frames` or all of `start`, `end`, `count`",
                        ));
                    }
                };
                (
                    Assertions {
                        project: p.project,
                        revision: p.revision,
                    },
                    ToolCall::Inspect(selection),
                )
            }
            "selection_context" => {
                let p: SelectionParams = parsed(params)?;
                let identity = match (
                    p.scene_instance_key,
                    p.component_key,
                    p.object_key,
                    p.repeat_key,
                ) {
                    (None, None, None, None) => None,
                    (Some(scene), Some(component), Some(object), Some(repeat)) => {
                        let values = [&scene, &component, &object, &repeat];
                        if values.iter().any(|value| {
                            value.is_empty()
                                || value.len() > 256
                                || value.chars().any(char::is_control)
                        }) {
                            return Err(ToolError::invalid(
                                "selection identity key is empty, oversized, or contains control characters",
                            ));
                        }
                        Some(fframes_studio_protocol::EditorObjectIdentity {
                            scene_instance_key: scene,
                            component_key: component,
                            object_key: object,
                            repeat_key: repeat,
                        })
                    }
                    _ => {
                        return Err(ToolError::invalid(
                            "selection identity needs all four semantic key fields or none",
                        ));
                    }
                };
                if p.frame_geometry_digest.as_ref().is_some_and(|digest| {
                    digest.len() != 64
                        || !digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                }) {
                    return Err(ToolError::invalid(
                        "frame_geometry_digest must be a lowercase SHA-256 digest",
                    ));
                }
                (
                    Assertions {
                        project: p.project,
                        revision: p.revision,
                    },
                    ToolCall::SelectionContext {
                        frame: index(p.frame, "frame")?,
                        seek_serial: p.seek_serial,
                        identity,
                        frame_geometry_digest: p.frame_geometry_digest,
                    },
                )
            }
            "source_lookup" => {
                let p: SourceLookupParams = parsed(params)?;
                let (path, symbol) = match (p.path, p.symbol) {
                    (Some(path), Some(symbol)) => (
                        Some(
                            studio_project::ProjectPath::try_from(path)
                                .map_err(|error| ToolError::invalid(error.to_string()))?,
                        ),
                        Some(symbol),
                    ),
                    (None, None) => (None, None),
                    _ => {
                        return Err(ToolError::invalid(
                            "explicit source lookup requires both path and symbol",
                        ));
                    }
                };
                if symbol.as_ref().is_some_and(|symbol| {
                    symbol.is_empty() || symbol.len() > 512 || symbol.chars().any(char::is_control)
                }) || p.marker.as_ref().is_some_and(|marker| {
                    marker.is_empty() || marker.len() > 256 || marker.chars().any(char::is_control)
                }) || p.frame_geometry_digest.as_ref().is_some_and(|digest| {
                    digest.len() != 64
                        || !digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                }) {
                    return Err(ToolError::invalid(
                        "source symbol, marker, or geometry digest is invalid",
                    ));
                }
                let identity = match (
                    p.scene_instance_key,
                    p.component_key,
                    p.object_key,
                    p.repeat_key,
                ) {
                    (None, None, None, None) => None,
                    (Some(scene), Some(component), Some(object), Some(repeat)) => {
                        let values = [&scene, &component, &object, &repeat];
                        if values.iter().any(|value| {
                            value.is_empty()
                                || value.len() > 256
                                || value.chars().any(char::is_control)
                        }) {
                            return Err(ToolError::invalid("invalid semantic object key"));
                        }
                        Some(fframes_studio_protocol::EditorObjectIdentity {
                            scene_instance_key: scene,
                            component_key: component,
                            object_key: object,
                            repeat_key: repeat,
                        })
                    }
                    _ => {
                        return Err(ToolError::invalid(
                            "selection lookup needs all four identity keys",
                        ));
                    }
                };
                let (frame, seek_serial) = match (p.frame, p.seek_serial, identity.as_ref()) {
                    (Some(frame), Some(seek_serial), Some(_)) => {
                        (Some(index(frame, "frame")?), Some(seek_serial))
                    }
                    (None, None, None) => (None, None),
                    _ => {
                        return Err(ToolError::invalid(
                            "selection lookup requires frame, seek_serial, and all identity keys",
                        ));
                    }
                };
                let explicit = path.is_some() && symbol.is_some();
                let selected = identity.is_some();
                if explicit == selected
                    || (p.marker.is_some() && !explicit)
                    || (p.frame_geometry_digest.is_some() && !selected)
                {
                    return Err(ToolError::invalid(
                        "provide either path+symbol or an exact frame/seek/identity selection",
                    ));
                }
                (
                    Assertions {
                        project: p.project,
                        revision: p.revision,
                    },
                    ToolCall::SourceLookup {
                        path,
                        symbol,
                        marker: p.marker,
                        frame,
                        seek_serial,
                        identity,
                        frame_geometry_digest: p.frame_geometry_digest,
                    },
                )
            }
            "style_context" => {
                let p: StyleParams = parsed(params)?;
                if p.token.as_ref().is_some_and(|token| {
                    token.is_empty() || token.len() > 128 || token.chars().any(char::is_control)
                }) {
                    return Err(ToolError::invalid(
                        "token filter is empty, oversized, or contains control characters",
                    ));
                }
                let identity = match (
                    p.scene_instance_key,
                    p.component_key,
                    p.object_key,
                    p.repeat_key,
                ) {
                    (None, None, None, None) => None,
                    (Some(scene), Some(component), Some(object), Some(repeat)) => {
                        let values = [&scene, &component, &object, &repeat];
                        if values.iter().any(|value| {
                            value.is_empty()
                                || value.len() > 256
                                || value.chars().any(char::is_control)
                        }) {
                            return Err(ToolError::invalid("invalid semantic object key"));
                        }
                        Some(fframes_studio_protocol::EditorObjectIdentity {
                            scene_instance_key: scene,
                            component_key: component,
                            object_key: object,
                            repeat_key: repeat,
                        })
                    }
                    _ => {
                        return Err(ToolError::invalid(
                            "style lookup needs all four identity keys",
                        ));
                    }
                };
                let (frame, seek_serial) = match (p.frame, p.seek_serial, identity.as_ref()) {
                    (Some(frame), Some(seek_serial), Some(_)) => {
                        (Some(index(frame, "frame")?), Some(seek_serial))
                    }
                    (None, None, None) => (None, None),
                    _ => {
                        return Err(ToolError::invalid(
                            "object style lookup requires frame, seek_serial, and all identity keys",
                        ));
                    }
                };
                if p.frame_geometry_digest.as_ref().is_some_and(|digest| {
                    digest.len() != 64
                        || !digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                }) || (p.frame_geometry_digest.is_some() && identity.is_none())
                {
                    return Err(ToolError::invalid("invalid frame geometry digest"));
                }
                (
                    Assertions {
                        project: p.project,
                        revision: p.revision,
                    },
                    ToolCall::StyleContext {
                        token: p.token,
                        frame,
                        seek_serial,
                        identity,
                        frame_geometry_digest: p.frame_geometry_digest,
                    },
                )
            }
            other => {
                return Err(ToolError::new(
                    ToolErrorCode::MethodNotFound,
                    format!(
                        "unknown tool `{}`",
                        other.chars().take(64).collect::<String>()
                    ),
                ));
            }
        };
        Ok(Self { assertions, call })
    }
}

/// The successful result of one call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolReply {
    pub revision: RevisionInfo,
    pub result: Value,
    #[serde(default)]
    pub artifacts: Vec<ArtifactRef>,
}

/// Executes validated requests against the project. The production implementation is
/// `backend::ProjectToolBackend`; the broker and facades only see this trait.
pub trait ToolBackend: Send + Sync + 'static {
    fn execute(
        &self,
        binding: &ToolBinding,
        request: &ToolRequest,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<ToolReply, ToolError>;
}

/// Whether a task can still use its capability. Stale or closed tasks are refused.
pub trait TaskLiveness: Send + Sync + 'static {
    fn is_live(&self, task: &TaskIdentity) -> bool;
}

/// The single typed entry point for the app, CLI and MCP facades.
#[derive(Clone)]
pub struct ToolDispatcher {
    backend: Arc<dyn ToolBackend>,
}

impl ToolDispatcher {
    pub fn new(backend: Arc<dyn ToolBackend>) -> Self {
        Self { backend }
    }

    /// Parse, execute and bound one call. The returned JSON is
    /// `{"revision":{..},"result":..,"artifacts":[..]}` and at most
    /// [`MAX_TEXT_REPLY_BYTES`] of text.
    pub fn dispatch(
        &self,
        binding: &ToolBinding,
        method: &str,
        params: &Value,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Value, ToolError> {
        let request = ToolRequest::parse(method, params)?;
        if let Some(project) = &request.assertions.project
            && *project != String::from(binding.task.project.clone())
        {
            return Err(ToolError::new(
                ToolErrorCode::CrossProject,
                "the request names a different project than this capability",
            ));
        }
        if let (Some(asserted), BoundRevision::Fixed(bound)) =
            (&request.assertions.revision, &binding.revision)
            && asserted != bound.as_str()
        {
            return Err(ToolError::new(
                ToolErrorCode::StaleRevision,
                "the request names a different revision than this capability",
            ));
        }
        if cancelled() {
            return Err(ToolError::new(ToolErrorCode::Cancelled, "call cancelled"));
        }
        let reply = self.backend.execute(binding, &request, cancelled)?;
        if let Some(asserted) = &request.assertions.revision
            && *asserted != reply.revision.id
            && matches!(binding.revision, BoundRevision::Draft)
        {
            // A draft snapshot is only comparable to a revision this task already saw.
            // The backend answers for the requested revision or refuses (stale).
            return Err(ToolError::new(
                ToolErrorCode::StaleRevision,
                "the requested revision is not the one this call was answered for",
            ));
        }
        for artifact in &reply.artifacts {
            if artifact.bytes as usize > MAX_IMAGE_ARTIFACT_BYTES {
                return Err(ToolError::new(
                    ToolErrorCode::TooLarge,
                    "image artifact exceeds 8 MiB",
                ));
            }
        }
        let value = serde_json::to_value(&reply)
            .map_err(|e| ToolError::new(ToolErrorCode::Internal, e.to_string()))?;
        let size = serde_json::to_vec(&value).map_or(usize::MAX, |bytes| bytes.len());
        if size > MAX_TEXT_REPLY_BYTES {
            return Err(ToolError::new(
                ToolErrorCode::TooLarge,
                format!(
                    "reply of {size} bytes exceeds the {MAX_TEXT_REPLY_BYTES} byte text limit; narrow the request"
                ),
            ));
        }
        Ok(value)
    }
}

/// Machine-readable descriptions of the tools (used by `studio-mcp tools/list` and
/// the CLI help). Parameters mirror [`ToolRequest::parse`].
pub fn tool_descriptions() -> Vec<Value> {
    let common = json!({
        "project": {"type": "string", "description": "Optional assertion: project id this call expects."},
        "revision": {"type": "string", "description": "Optional assertion: 64-hex revision this call expects."},
    });
    let with = |extra: Value, required: &[&str]| {
        let mut properties = common.as_object().cloned().unwrap_or_default();
        if let Some(extra) = extra.as_object() {
            properties.extend(extra.clone());
        }
        json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
    };
    let source_lookup_schema = with(
        json!({
            "path": {"type": "string"},
            "symbol": {"type": "string"},
            "marker": {"type": "string"},
            "frame": {"type": "integer", "minimum": 0},
            "seek_serial": {"type": "integer", "minimum": 0},
            "scene_instance_key": {"type": "string"},
            "component_key": {"type": "string"},
            "object_key": {"type": "string"},
            "repeat_key": {"type": "string"},
            "frame_geometry_digest": {"type": "string", "pattern": "^[a-f0-9]{64}$"}
        }),
        &[],
    );
    let style_context_schema = with(
        json!({
            "token": {"type": "string", "description": "Optional exact token name filter."},
            "frame": {"type": "integer", "minimum": 0},
            "seek_serial": {"type": "integer", "minimum": 0},
            "scene_instance_key": {"type": "string"},
            "component_key": {"type": "string"},
            "object_key": {"type": "string"},
            "repeat_key": {"type": "string"},
            "frame_geometry_digest": {"type": "string", "pattern": "^[a-f0-9]{64}$"}
        }),
        &[],
    );
    vec![
        json!({"name": "project_context", "description": "Project manifest summary, source file inventory and the revision this call describes. Read-only.", "inputSchema": with(json!({}), &[])}),
        json!({"name": "timeline", "description": "Compiled timeline: size, fps, frame count, scenes and audio tracks of the current revision. Builds are unvalidated until the acceptance validator runs.", "inputSchema": with(json!({}), &[])}),
        json!({"name": "render_frame", "description": "Render one frame to a PNG artifact (id, path, hash, expiry; at most 8 MiB).", "inputSchema": with(json!({"frame": {"type": "integer", "minimum": 0}, "scale": {"type": "number", "exclusiveMinimum": 0, "maximum": 1}}), &["frame"])}),
        json!({"name": "render_strip", "description": "Render up to 24 evenly spaced frames from start to end into one PNG strip artifact.", "inputSchema": with(json!({"start": {"type": "integer", "minimum": 0}, "end": {"type": "integer", "minimum": 0}, "count": {"type": "integer", "minimum": 1, "maximum": MAX_STRIP_FRAMES}, "scale": {"type": "number", "exclusiveMinimum": 0, "maximum": 1}}), &["start", "end", "count"])}),
        json!({"name": "inspect", "description": "Inspection diagnostics (missing assets, unsupported shaders, ...) for up to 256 frames: list `frames`, or `start`/`end`/`count`.", "inputSchema": with(json!({"frames": {"type": "array", "items": {"type": "integer", "minimum": 0}, "maxItems": MAX_INSPECT_FRAMES_PER_CALL}, "start": {"type": "integer", "minimum": 0}, "end": {"type": "integer", "minimum": 0}, "count": {"type": "integer", "minimum": 1, "maximum": MAX_INSPECT_FRAMES_PER_CALL}}), &[])}),
        json!({"name": "build_status", "description": "Observe compile state for this project. Never launches Cargo.", "inputSchema": with(json!({}), &[])}),
        json!({"name": "selection_context", "description": "Read the validated semantic objects on an exact source-revision frame and optional frozen object key. Supply the displayed frame and seek serial; include frame_geometry_digest when validating a frozen selection. It does not infer source locations.", "inputSchema": with(json!({"frame": {"type": "integer", "minimum": 0}, "seek_serial": {"type": "integer", "minimum": 0}, "scene_instance_key": {"type": "string"}, "component_key": {"type": "string"}, "object_key": {"type": "string"}, "repeat_key": {"type": "string"}, "frame_geometry_digest": {"type": "string", "pattern": "^[a-f0-9]{64}$"}}), &["frame", "seek_serial"])}),
        json!({"name": "source_lookup", "description": "Provide exactly one query: either a project-relative path and symbol (optionally a marker), or a complete frame/seek/object identity tuple. The parser rejects mixed or incomplete forms. Reads hash-verified Rust syntax; no macro expansion, type resolution, code execution, or live filesystem access.", "inputSchema": source_lookup_schema}),
        json!({"name": "style_context", "description": "Read the active preset identity, typed token values and explicitly registered bindings for this immutable revision. Optionally filter by token, and optionally provide a complete frame/seek/object identity tuple; partial tuples are rejected.", "inputSchema": style_context_schema}),
    ]
}

/// The task-specific MCP stdio server entry for ACP `session/new`: the `studio-mcp`
/// binary plus the capability FILE path. The secret lives only inside that owner-only
/// file; it is never an argument or an environment value.
pub fn mcp_server_for(
    grant: &broker::ToolGrant,
    studio_mcp: &std::path::Path,
) -> Result<studio_agent_spike::McpStdioServer, studio_agent_spike::McpConfigError> {
    studio_agent_spike::McpStdioServer::new(
        "fframes-studio",
        studio_mcp,
        vec![
            "--capability".to_owned(),
            grant.capability_file.to_string_lossy().into_owned(),
        ],
        Vec::new(),
    )
}

/// A helper binary shipped next to the running executable (`studio-tools`, `studio-mcp`).
pub fn sibling_binary(name: &str) -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe
        .parent()?
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    path.is_file().then_some(path)
}
