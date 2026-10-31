//! Production ACP v1 driver on the official SDK.
//!
//! One adapter process is owned per driver (inside a caller-supplied
//! [`ProcessTreeManager`] scope). The SDK connection runs on a dedicated owned
//! thread with `futures::executor`, never on a UI or audio thread. Hosts interact
//! through a synchronous handle and a bounded event queue.
//!
//! Concurrency contract: turn state, cancellation and permission requests all live under
//! the single `core` mutex, so "Stop", "grant" and "turn finished" are totally ordered.
//! Teardown after a failure never runs on a handle caller's thread: failures only latch
//! state and wake the reaper thread. Lock order: `core` is never held while taking
//! `child`; `child` may be held while taking `core`.

use super::{
    McpHttpServer, McpStdioServer, McpTransport, UserMcpLaunch,
    events::*,
    redact::{Redactor, StreamRedactor},
    transcript::{Transcript, TranscriptPage},
    transport::{
        self, INGRESS_MAX_BYTES, INGRESS_MAX_FRAMES, Ingress, LineSink, LineStream, fits_wire,
    },
};
use crate::{AgentSupervisor, SupervisorError, discovery::AdapterLaunch};
use agent_client_protocol::{
    Agent, Client, ConnectionTo, Dispatch, Error as RpcError, Handled, JsonRpcMessage, Lines,
    RawJsonRpcMessage, Responder, UntypedMessage,
    schema::{
        ProtocolVersion,
        v1::{
            AuthenticateRequest, AvailableCommandInput, BlobResourceContents, CancelNotification,
            ClientCapabilities, ContentBlock, EmbeddedResource, EmbeddedResourceResource,
            EnvVariable, HttpHeader, ImageContent, Implementation, InitializeRequest,
            InitializeResponse, LoadSessionRequest, McpServer, McpServerHttp, McpServerStdio,
            NewSessionRequest, PlanEntryStatus, PromptRequest, PromptResponse, RequestId,
            RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
            ResourceLink, ResumeSessionRequest, SelectedPermissionOutcome, SessionConfigKind,
            SessionConfigOption, SessionConfigOptionValue, SessionConfigSelectOptions,
            SessionModeState, SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
            SetSessionModeRequest, StopReason, TextContent, TextResourceContents, ToolCallStatus,
        },
    },
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::{StreamExt, channel::mpsc, future::Either};
use parking_lot::{Condvar, Mutex};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use studio_bootstrap::{
    EscapeScan, ProcessError, ProcessTreeManager, TerminationReport, TrackedChild, WriterOwnership,
};

const CLIENT_NAME: &str = "fframes-studio";
/// `agentInfo.name` of the Claude ACP adapter (its npm package name), current and former.
const CLAUDE_ADAPTER_NAMES: [&str; 2] = [
    "@agentclientprotocol/claude-agent-acp",
    "@zed-industries/claude-code-acp",
];
/// The ACP extension that delivers a message to the running turn: the adapter advertises
/// it as `agentCapabilities._meta.steering = true` and then accepts the request
/// `_session/steering { sessionId, text }`.
const STEERING_CAPABILITY: &str = "steering";
const STEERING_METHOD: &str = "_session/steering";
const MAX_PROMPT_BYTES: usize = 256 * 1024;
const MAX_PROMPT_IMAGE_COUNT: usize = 3;
const MAX_PROMPT_FILE_COUNT: usize = 16;
const MAX_PENDING_PERMISSIONS: usize = 16;
/// Plan entries and slash commands exported per update; the rest is dropped.
const MAX_PLAN_ENTRIES: usize = 64;
const MAX_COMMANDS: usize = 256;
const FAILURE_DETAIL_BYTES: usize = 2048;
/// Host commands accepted but not yet executed by the connection task.
const COMMAND_QUEUE: usize = 8;
/// Mode/config requests that may await an adapter reply at once.
pub const MAX_OUTSTANDING_REQUESTS: usize = 4;

/// Engineering bounds; defaults are the Stage 1 contract values.
#[derive(Debug, Clone)]
pub struct DriverLimits {
    pub max_message_bytes: usize,
    pub max_queued_events: usize,
    pub max_transcript_bytes: usize,
    pub max_stderr_bytes: usize,
    pub init_timeout: Duration,
    /// Idle time allowed during a turn. Only real agent activity restarts it: message and
    /// thought chunks, tool calls and their updates, plan updates and permission requests.
    /// Command lists, session info and option pushes do not.
    pub prompt_timeout: Duration,
    /// Hard cap on one turn from the moment it is sent, however active the adapter is.
    pub max_turn: Duration,
    /// Graceful drain before the owned tree is force-killed.
    pub shutdown_grace: Duration,
    /// Time an adapter has to answer a cancelled turn before it is killed.
    pub cancel_ack_timeout: Duration,
    /// Time an adapter has to answer a mode/config request.
    pub request_timeout: Duration,
}

impl Default for DriverLimits {
    fn default() -> Self {
        Self {
            max_message_bytes: 1024 * 1024,
            max_queued_events: 256,
            max_transcript_bytes: 4 * 1024 * 1024,
            max_stderr_bytes: 1024 * 1024,
            init_timeout: Duration::from_secs(30),
            prompt_timeout: Duration::from_secs(15 * 60),
            max_turn: Duration::from_secs(2 * 60 * 60),
            shutdown_grace: Duration::from_secs(2),
            cancel_ack_timeout: Duration::from_secs(2),
            request_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverMode {
    /// Initialize, authenticate (if configured), create/restore a session.
    Full,
    /// Stop after `initialize`; used by discovery when proving a session is unwanted.
    InitializeOnly,
}

/// Whether stdio MCP servers are sent in `session/new` / `session/load` /
/// `session/resume`. ACP v1 has no stdio capability flag (stdio is the mandatory
/// baseline transport; `McpCapabilities` only advertises `http`/`sse`, and a `stdio`
/// flag exists only in v2, which is never enabled), so the gate is host policy on top
/// of the already enforced v1 negotiation. HTTP and SSE servers are never sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpStdioSupport {
    #[default]
    Baseline,
    /// Always send an empty server list.
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct DriverConfig {
    pub provider: String,
    pub task: String,
    /// Absolute session working directory (the stable draft).
    pub cwd: PathBuf,
    pub launch: AdapterLaunch,
    pub limits: DriverLimits,
    /// Restore this session instead of creating one; fails unless negotiated.
    pub resume_session: Option<String>,
    /// Where the writer's ownership comes from.
    pub ownership: OwnershipSource,
    pub mode: DriverMode,
    /// Task-specific stdio MCP servers; env values are added to the redaction set.
    pub mcp_servers: Vec<McpStdioServer>,
    /// The user's own MCP servers (stdio or streamable HTTP). They launch beside
    /// [`Self::mcp_servers`] (and only when those may), but their tools are never
    /// pre-approved: every call asks. Their secrets join the redaction set.
    pub user_mcp_servers: Vec<UserMcpLaunch>,
    pub mcp_stdio: McpStdioSupport,
}

/// Checks every server before anything starts: each one's own invariants, and that no
/// two share a name (a user server must never replace the studio's).
pub(super) fn check_mcp_servers(
    studio: &[McpStdioServer],
    user: &[UserMcpLaunch],
) -> Result<(), String> {
    let mut names = std::collections::HashSet::new();
    let all = studio
        .iter()
        .map(|server| (server.name.as_str(), server.validate()))
        .chain(
            user.iter()
                .map(|server| (server.name(), server.transport.validate())),
        );
    for (name, valid) in all {
        valid.map_err(|error| format!("invalid MCP server configuration: {error}"))?;
        if !names.insert(name) {
            return Err(format!(
                "invalid MCP server configuration: two servers are named '{name}'"
            ));
        }
    }
    Ok(())
}

/// Every secret a driver must redact: the launch's own, the studio servers' environment
/// values and the user servers' environment or header values.
pub(super) fn mcp_secrets<'a>(
    launch: &'a [String],
    studio: &'a [McpStdioServer],
    user: &'a [UserMcpLaunch],
) -> impl Iterator<Item = String> + 'a {
    launch
        .iter()
        .cloned()
        .chain(studio.iter().flat_map(|s| s.secrets().map(str::to_owned)))
        .chain(user.iter().flat_map(|s| s.secrets().map(str::to_owned)))
}

/// Where a driver's writer ownership comes from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum OwnershipSource {
    /// The containment the process supervisor verified for the spawned adapter.
    #[default]
    Verified,
    /// Injected by a test harness without any verification.
    Injected(WriterOwnership),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverStatus {
    Starting,
    Ready,
    Prompting,
    Stopping,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub session_id: Option<String>,
    pub restored: bool,
    pub initialized: InitializedInfo,
}

/// Final facts about a closed driver; the inputs to the quiescence gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverOutcome {
    /// The `DriverConfig::task` binding this outcome belongs to.
    pub task: String,
    /// Sanitized provider session id, if a session was established.
    pub session: Option<String>,
    /// Verified termination of the owned task process group.
    pub termination: TerminationReport,
    /// Launch ownership (see [`AcpDriver::ownership`]), demoted to `Detached` if an
    /// escape was observed or containment could not be verified.
    pub ownership: WriterOwnership,
    pub escaped_pids: Vec<u32>,
    /// Why containment of the task's processes could not be verified (an escape scan or
    /// the verified termination failed), if it could not.
    pub containment_unverified: Option<String>,
    /// The last finished turn, if any. A latched failure always dominates it: consumers
    /// must refuse capture authority when `failure` is set.
    pub last_prompt: Option<PromptOutcome>,
    pub failure: Option<AgentFailure>,
    pub late_events_rejected: u64,
    pub exit_code: Option<i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("the agent session is closed")]
    Closed,
    #[error("{0}")]
    Failed(AgentFailure),
    #[error("the agent session is not ready")]
    NotReady,
    #[error("a prompt turn is already in progress")]
    TurnInProgress,
    #[error("no prompt turn is in progress")]
    NoActiveTurn,
    #[error("permission request {0:?} is not pending (duplicate, late or closed)")]
    UnknownPermission(PermissionId),
    #[error("{0}")]
    InvalidOption(String),
    #[error("{0}")]
    InvalidPrompt(&'static str),
    /// A bounded admission limit is full; retry after the adapter answers.
    #[error("{0}")]
    Busy(&'static str),
    /// The driver's wire cannot do this (for example steer a running turn).
    #[error("{0}")]
    Unsupported(&'static str),
}

#[derive(Debug)]
enum Command {
    Prompt {
        turn: u64,
        content: Vec<ContentBlock>,
    },
    /// Nudges the loop to look at latched state such as a pending cancel.
    Wake,
    SetMode {
        raw: String,
        label: String,
        key: u64,
    },
    SetConfig {
        raw_id: String,
        label: String,
        value: OptionValue,
        key: u64,
    },
    /// Delivers text to the running turn through the `_session/steering` extension.
    Steer {
        text: String,
    },
    Shutdown,
}

/// A bounded PNG attachment for an ACP prompt. The image is never added to the driver's
/// text transcript or persisted conversation log.
pub struct PromptImage {
    data: Vec<u8>,
}

impl PromptImage {
    pub fn png(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Size of the PNG bytes, for measuring against a prompt budget.
    pub fn byte_len(&self) -> usize {
        self.data.len()
    }
}

/// A file the agent is given: either a link to a location (an ACP resource link, never
/// the bytes) or the content itself (an ACP embedded resource). The host decides what
/// may be attached; the driver only carries it.
#[derive(Clone, PartialEq, Eq)]
pub struct PromptFile {
    name: String,
    source: FileSource,
}

#[derive(Clone, PartialEq, Eq)]
enum FileSource {
    Link(std::path::PathBuf),
    Content(Vec<u8>),
}

impl std::fmt::Debug for PromptFile {
    /// Never prints content.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut shown = f.debug_struct("PromptFile");
        shown.field("name", &self.name);
        match &self.source {
            FileSource::Link(path) => shown.field("path", path),
            FileSource::Content(bytes) => shown.field("bytes", &bytes.len()),
        };
        shown.finish()
    }
}

impl PromptFile {
    /// A link. `path` must be absolute; `name` is the label shown to the agent.
    pub fn new(name: impl Into<String>, path: std::path::PathBuf) -> Self {
        Self {
            name: name.into(),
            source: FileSource::Link(path),
        }
    }

    /// The content itself, for a file the agent cannot be trusted to open by path. The
    /// adapter must have negotiated embedded context.
    pub fn embedded(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            source: FileSource::Content(bytes),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The linked location; `None` for embedded content.
    pub fn path(&self) -> Option<&std::path::Path> {
        match &self.source {
            FileSource::Link(path) => Some(path),
            FileSource::Content(_) => None,
        }
    }

    fn content_bytes(&self) -> &[u8] {
        match &self.source {
            FileSource::Content(bytes) => bytes,
            FileSource::Link(_) => &[],
        }
    }

    pub fn is_embedded(&self) -> bool {
        matches!(self.source, FileSource::Content(_))
    }

    /// Size of the embedded content (0 for a link).
    pub fn content_len(&self) -> usize {
        match &self.source {
            FileSource::Content(bytes) => bytes.len(),
            FileSource::Link(_) => 0,
        }
    }
}

#[derive(Debug, Clone)]
struct Turn {
    id: u64,
    /// Latched by the host call itself, before any command is queued.
    cancel_requested: bool,
}

/// The answer a wire sends back for one pending request.
pub(in crate::driver) enum WireAnswer {
    /// The raw option id the user picked.
    Selected(String),
    Cancelled,
    /// Answers to a question request, in the order of its questions.
    Answers(Vec<crate::driver::QuestionAnswer>),
}

pub(in crate::driver) enum SinkError {
    Rpc(RpcError),
    Failed(AgentFailure),
}

/// Writes the answer to one pending request back to the adapter. Called under the `core`
/// lock, so it must never take it.
pub(in crate::driver) trait PermissionSink: Send {
    fn respond(self: Box<Self>, answer: WireAnswer) -> Result<(), SinkError>;
}

struct AcpResponder(Responder<RequestPermissionResponse>);

impl PermissionSink for AcpResponder {
    fn respond(self: Box<Self>, answer: WireAnswer) -> Result<(), SinkError> {
        let outcome = match answer {
            WireAnswer::Selected(raw) => {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(raw))
            }
            WireAnswer::Cancelled | WireAnswer::Answers(_) => RequestPermissionOutcome::Cancelled,
        };
        quiet(|| self.0.respond(RequestPermissionResponse::new(outcome))).map_err(SinkError::Rpc)
    }
}

struct PendingPermission {
    sink: Box<dyn PermissionSink>,
    /// `(exported, raw)` option ids; only exported ids are ever shown to hosts.
    options: Vec<(String, String)>,
    wire_id: String,
    /// A question request: answered with [`WireAnswer::Answers`], not an option.
    question: bool,
}

#[derive(Default)]
struct Permissions {
    next: u64,
    pending: HashMap<u64, PendingPermission>,
    /// JSON-RPC ids of in-flight permission requests; duplicates are protocol violations.
    wire_ids: HashSet<String>,
}

struct Core {
    status: DriverStatus,
    phase: Phase,
    session: Option<String>,
    restored: bool,
    restoring: bool,
    initialized: Option<InitializedInfo>,
    turn: Option<Turn>,
    next_turn: u64,
    last_outcome: Option<PromptOutcome>,
    failure: Option<AgentFailure>,
    stopping: bool,
    escaped: Vec<u32>,
    /// First reason containment could not be verified.
    containment_unverified: Option<String>,
    outcome: Option<DriverOutcome>,
    permissions: Permissions,
    /// Turn whose `session/cancel` notification still has to be written.
    cancel_to_send: Option<u64>,
}

/// Runs SDK calls with SDK diagnostics muted. The SDK traces whole wire messages (a
/// permission reply carries the raw option id) from whichever thread sends them, so every
/// call into it that is not already on the muted connection thread goes through here.
fn quiet<R>(f: impl FnOnce() -> R) -> R {
    tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), f)
}

impl Core {
    /// Answers every open permission request as cancelled, in one ordered step with
    /// whatever latched the caller's state change. Returns the closed ids.
    fn drain_permissions(&mut self) -> Vec<PermissionId> {
        let mut drained: Vec<(u64, PendingPermission)> = self.permissions.pending.drain().collect();
        drained.sort_by_key(|(id, _)| *id);
        self.permissions.wire_ids.clear();
        drained
            .into_iter()
            .map(|(id, pending)| {
                let _ = pending.sink.respond(WireAnswer::Cancelled);
                PermissionId(id)
            })
            .collect()
    }
}

struct Streams {
    agent: StreamRedactor,
    thought: StreamRedactor,
    user: StreamRedactor,
}

impl Streams {
    fn get(&mut self, role: MessageRole) -> &mut StreamRedactor {
        match role {
            MessageRole::Agent => &mut self.agent,
            MessageRole::Thought => &mut self.thought,
            MessageRole::User => &mut self.user,
        }
    }
}

const ROLES: [MessageRole; 3] = [MessageRole::Agent, MessageRole::Thought, MessageRole::User];

/// Incrementally sanitized stderr: only redacted text is ever retained or exposed.
struct StderrState {
    redactor: StreamRedactor,
    carry: Vec<u8>,
    tail: VecDeque<u8>,
}

impl StderrState {
    fn append(&mut self, released: &str, max: usize) {
        self.tail.extend(released.as_bytes());
        if self.tail.len() > max {
            let excess = self.tail.len() - max;
            self.tail.drain(..excess);
        }
    }
}

/// Exported-id -> wire-id tables; wire ids may carry secrets and never leave this module.
#[derive(Default)]
struct OptionIds {
    modes: HashMap<String, String>,
    config: HashMap<String, RawConfig>,
}

struct RawConfig {
    raw_id: String,
    values: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Deadline {
    at: Instant,
    phase: Phase,
    /// Hard limit `at` can never be extended past (the turn cap).
    cap: Option<Instant>,
}

struct WatchState {
    phase_deadline: Option<Deadline>,
    requests: HashMap<u64, Instant>,
    next_request: u64,
    kill: bool,
    stop: bool,
}

enum WatchAction {
    Kill,
    Phase(Phase),
    /// The prompt turn reached `DriverLimits::max_turn`.
    TurnCap,
    RequestTimedOut,
}

fn earliest(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Deadline ledger and reaper wake-up shared with the watchdog thread.
struct Watchdog {
    state: Mutex<WatchState>,
    changed: Condvar,
}

impl Watchdog {
    fn new() -> Self {
        Self {
            state: Mutex::new(WatchState {
                phase_deadline: None,
                requests: HashMap::new(),
                next_request: 0,
                kill: false,
                stop: false,
            }),
            changed: Condvar::new(),
        }
    }
    fn arm(&self, phase: Phase, after: Duration) {
        self.state.lock().phase_deadline = Instant::now().checked_add(after).map(|at| Deadline {
            at,
            phase,
            cap: None,
        });
        self.changed.notify_all();
    }
    /// Arms the prompt idle deadline under a hard cap of `max_turn` from now.
    fn arm_turn(&self, idle: Duration, max_turn: Duration) {
        let now = Instant::now();
        let cap = now.checked_add(max_turn);
        self.state.lock().phase_deadline =
            earliest(now.checked_add(idle), cap).map(|at| Deadline {
                at,
                phase: Phase::Prompt,
                cap,
            });
        self.changed.notify_all();
    }
    fn disarm(&self) {
        self.state.lock().phase_deadline = None;
        self.changed.notify_all();
    }
    /// Pushes an armed `phase` deadline to `after` from now, never past its cap; any
    /// other deadline is kept.
    fn extend(&self, phase: Phase, after: Duration) {
        let mut state = self.state.lock();
        if let Some(deadline) = &mut state.phase_deadline
            && deadline.phase == phase
            && let Some(at) = earliest(Instant::now().checked_add(after), deadline.cap)
        {
            deadline.at = at;
            self.changed.notify_all();
        }
    }
    fn stop(&self) {
        self.state.lock().stop = true;
        self.changed.notify_all();
    }
    fn request_kill(&self) {
        self.state.lock().kill = true;
        self.changed.notify_all();
    }
    /// Admits one outstanding request with a reply deadline, or refuses at `max`.
    fn track_request(&self, after: Duration, max: usize) -> Option<u64> {
        let mut state = self.state.lock();
        if state.requests.len() >= max {
            return None;
        }
        let key = state.next_request;
        state.next_request += 1;
        state.requests.insert(key, Instant::now() + after);
        self.changed.notify_all();
        Some(key)
    }
    fn untrack(&self, key: u64) {
        self.state.lock().requests.remove(&key);
        self.changed.notify_all();
    }
    fn outstanding(&self) -> usize {
        self.state.lock().requests.len()
    }
    fn next_action(&self) -> Option<WatchAction> {
        let mut state = self.state.lock();
        loop {
            if state.stop {
                return None;
            }
            if state.kill {
                state.kill = false;
                return Some(WatchAction::Kill);
            }
            let request = state
                .requests
                .iter()
                .min_by_key(|(_, at)| **at)
                .map(|(k, at)| (*k, *at));
            let phase = state.phase_deadline;
            let next = earliest(phase.map(|d| d.at), request.map(|(_, at)| at));
            let Some(at) = next else {
                self.changed.wait(&mut state);
                continue;
            };
            if Instant::now() >= at {
                if let Some(deadline) = phase
                    && deadline.at <= at
                {
                    state.phase_deadline = None;
                    return Some(if deadline.cap.is_some_and(|cap| deadline.at >= cap) {
                        WatchAction::TurnCap
                    } else {
                        WatchAction::Phase(deadline.phase)
                    });
                }
                if let Some((key, _)) = request {
                    state.requests.remove(&key);
                }
                return Some(WatchAction::RequestTimedOut);
            }
            let _ = self.changed.wait_until(&mut state, at);
        }
    }
}

pub(super) struct Shared {
    pub(super) limits: DriverLimits,
    ownership: WriterOwnership,
    redactor: Arc<Redactor>,
    queue: Arc<EventQueue>,
    child: Arc<Mutex<TrackedChild>>,
    core: Mutex<Core>,
    core_changed: Condvar,
    transcript: Mutex<Transcript>,
    stderr: Mutex<StderrState>,
    options: Mutex<AgentOptions>,
    option_ids: Mutex<OptionIds>,
    streams: Mutex<Streams>,
    ingress: Arc<Ingress>,
    late: AtomicU64,
    declined: AtomicU64,
    watchdog: Watchdog,
    forced_kill: AtomicBool,
    /// The adapter identified itself as the Claude ACP adapter at initialize.
    claude_adapter: AtomicBool,
    /// The adapter advertised the `_session/steering` extension at initialize. Read only
    /// through [`AcpDriver::steering_negotiated`].
    steering: AtomicBool,
}

/// Splits `text` into pieces of at most `max` bytes on char boundaries.
fn split_chunks(text: &str, max: usize) -> impl Iterator<Item = &str> {
    let mut rest = text;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut end = max.min(rest.len());
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (head, tail) = rest.split_at(end);
        rest = tail;
        Some(head)
    })
}

mod claude;
mod codex;
mod connection;
mod handle;
mod native_launch;
mod shared;

pub use claude::ClaudeDriver;
pub use codex::CodexDriver;
use connection::*;
pub use handle::AcpDriver;
use handle::{Owner, Threads, image_budget, spawn_owned, validate_prompt};

#[cfg(test)]
mod tests {
    use super::*;

    fn internal(message: &str) -> RpcError {
        RpcError::new(
            i32::from(agent_client_protocol::ErrorCode::InternalError),
            message,
        )
        .data(serde_json::json!({ "errorKind": "incomplete_tool_call" }))
    }

    #[test]
    fn a_user_server_cannot_take_the_name_of_a_studio_server() {
        let server = |name: &str| {
            McpStdioServer::new(name, std::env::current_exe().unwrap(), vec![], vec![]).unwrap()
        };
        let studio = [server("fframes-studio")];
        assert!(check_mcp_servers(&studio, &[server("github").into()]).is_ok());
        let error = check_mcp_servers(&studio, &[server("fframes-studio").into()]).unwrap_err();
        assert!(
            error.contains("two servers are named 'fframes-studio'"),
            "{error}"
        );
        let http = |name: &str, url: &str| {
            UserMcpLaunch::from(McpHttpServer {
                name: name.into(),
                url: url.into(),
                headers: vec![],
            })
        };
        let error = check_mcp_servers(&studio, &[http("fframes-studio", "https://x.test/mcp")])
            .unwrap_err();
        assert!(error.contains("two servers"), "{error}");
        let error = check_mcp_servers(&studio, &[http("docs", "file:///etc/passwd")]).unwrap_err();
        assert!(error.contains("http"), "{error}");
        assert!(check_mcp_servers(&studio, &[http("docs", "https://x.test/mcp")]).is_ok());
    }

    #[test]
    fn turn_activity_extends_only_the_armed_prompt_deadline() {
        let deadline = |watchdog: &Watchdog| watchdog.state.lock().phase_deadline;
        let watchdog = Watchdog::new();
        // Nothing armed: activity arms nothing.
        watchdog.extend(Phase::Prompt, Duration::from_secs(60));
        assert_eq!(deadline(&watchdog), None);

        watchdog.arm_turn(Duration::from_millis(10), Duration::from_secs(3600));
        let first = deadline(&watchdog).unwrap();
        watchdog.extend(Phase::Prompt, Duration::from_secs(60));
        let extended = deadline(&watchdog).unwrap();
        assert_eq!(extended.phase, Phase::Prompt);
        assert!(extended.at >= first.at + Duration::from_secs(59));

        // Another phase's deadline is never pushed back by turn activity.
        watchdog.arm(Phase::CancelAck, Duration::from_millis(10));
        let before = deadline(&watchdog);
        watchdog.extend(Phase::Prompt, Duration::from_secs(60));
        assert_eq!(deadline(&watchdog), before);
    }

    #[test]
    fn activity_never_extends_a_turn_past_its_cap() {
        let watchdog = Watchdog::new();
        watchdog.arm_turn(Duration::from_millis(10), Duration::from_millis(50));
        let armed = watchdog.state.lock().phase_deadline.unwrap();
        let cap = armed.cap.unwrap();
        assert!(armed.at < cap);
        watchdog.extend(Phase::Prompt, Duration::from_secs(60));
        assert_eq!(watchdog.state.lock().phase_deadline.unwrap().at, cap);
        assert!(matches!(watchdog.next_action(), Some(WatchAction::TurnCap)));

        // The idle deadline expiring first is an idle timeout, not the cap.
        watchdog.arm_turn(Duration::from_millis(10), Duration::from_secs(60));
        assert!(matches!(
            watchdog.next_action(),
            Some(WatchAction::Phase(Phase::Prompt))
        ));
        // A cap too large to represent leaves only the idle deadline.
        watchdog.arm_turn(Duration::from_millis(10), Duration::MAX);
        assert_eq!(watchdog.state.lock().phase_deadline.unwrap().cap, None);
    }

    #[test]
    fn prompt_lines_are_measured_exactly_as_the_sdk_writes_them() {
        let request = PromptRequest::new(
            "sess-1",
            vec![
                ContentBlock::Text(TextContent::new("quote \" and \u{1} and Ã©")),
                ContentBlock::Image(ImageContent::new("AAAA", "image/png")),
            ],
        );
        let untyped = request.to_untyped_message().unwrap();
        let line = serde_json::to_string(
            &RawJsonRpcMessage::request(
                untyped.method,
                untyped.params,
                RequestId::Str("123e4567-e89b-12d3-a456-426614174000".to_owned()),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(prompt_line_bytes(&request), line.len(), "{line}");
        assert_eq!(base64_len(0), Some(0));
        assert_eq!(base64_len(1), Some(4));
        assert_eq!(base64_len(3), Some(4));
        assert_eq!(base64_len(4), Some(8));
        assert_eq!(base64_len(usize::MAX), None);
    }

    #[test]
    fn only_unfinished_server_tools_end_the_turn_normally() {
        let prefix =
            "Internal error: Claude ended the turn without returning results for tool calls: ";
        assert!(only_server_tools_unfinished(&internal(&format!(
            "{prefix}srvtoolu_018rBjyoATEpEkALdkGDKJtB"
        ))));
        assert!(only_server_tools_unfinished(&internal(&format!(
            "{prefix}srvtoolu_a, srvtoolu_b"
        ))));
        // Every id counts, wherever the list wraps.
        assert!(only_server_tools_unfinished(&internal(&format!(
            "{prefix}srvtoolu_a,\nsrvtoolu_b"
        ))));
        assert!(!only_server_tools_unfinished(&internal(&format!(
            "{prefix}srvtoolu_a\ntoolu_b"
        ))));
        // A client tool that never finished is a real failure.
        assert!(!only_server_tools_unfinished(&internal(&format!(
            "{prefix}srvtoolu_a, toolu_b"
        ))));
        assert!(!only_server_tools_unfinished(&internal(&format!(
            "{prefix}srvtoolu_"
        ))));
        assert!(!only_server_tools_unfinished(&internal(prefix)));
        assert!(!only_server_tools_unfinished(&internal(
            "Internal error: boom"
        )));
        // The same text without the adapter's structured tag is not this report.
        let untagged = || {
            RpcError::new(
                i32::from(agent_client_protocol::ErrorCode::InternalError),
                format!("{prefix}srvtoolu_a"),
            )
        };
        assert!(!only_server_tools_unfinished(&untagged()));
        assert!(!only_server_tools_unfinished(
            &untagged().data(serde_json::json!({ "errorKind": "no_result" }))
        ));
        // Same text under another error code is not this adapter rejection.
        assert!(!only_server_tools_unfinished(
            &RpcError::new(
                i32::from(agent_client_protocol::ErrorCode::InvalidParams),
                format!("{prefix}srvtoolu_a"),
            )
            .data(serde_json::json!({ "errorKind": "incomplete_tool_call" }))
        ));
    }
}
