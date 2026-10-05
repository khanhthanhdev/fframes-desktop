//! Normalized, bounded agent events and the structured failure model.

use parking_lot::{Condvar, Mutex};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

/// A coalesced message delta never grows past this, so 256 slots bound memory.
pub const MAX_DELTA_EVENT_BYTES: usize = 16 * 1024;
const MAX_LABEL_BYTES: usize = 1024;

pub fn truncate_label(text: &str) -> String {
    if text.len() <= MAX_LABEL_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_LABEL_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    Agent,
    Thought,
    /// Replayed user text while restoring a session.
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReasonKind {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

impl StopReasonKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EndTurn => "end_turn",
            Self::MaxTokens => "max_tokens",
            Self::MaxTurnRequests => "max_turn_requests",
            Self::Refusal => "refusal",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolEvent {
    pub tool_call_id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<ToolStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionChoice {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

/// Correlation id for one permission request, unique within a driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PermissionId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionPrompt {
    pub id: PermissionId,
    pub turn: u64,
    pub tool_call_id: String,
    pub title: String,
    pub options: Vec<PermissionChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionResolution {
    Selected {
        option_id: String,
    },
    /// Closed as cancelled: user Stop, prompt cancel, turn end or transport loss.
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionReply {
    Select(String),
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigValue {
    pub value: String,
    pub name: String,
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigKind {
    Select {
        current: String,
        values: Vec<ConfigValue>,
    },
    Boolean {
        current: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub kind: ConfigKind,
}

/// Value for [`ConfigOption`] updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OptionValue {
    Value(String),
    Boolean(bool),
}

/// Options the adapter advertised for the session; nothing else can be set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOptions {
    pub current_mode: Option<String>,
    pub modes: Vec<ModeOption>,
    pub config: Vec<ConfigOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCapabilityInfo {
    pub load_session: bool,
    pub resume_session: bool,
    pub prompt_image: bool,
    pub prompt_audio: bool,
    pub prompt_embedded_context: bool,
    pub mcp_http: bool,
    pub mcp_sse: bool,
    /// ACP v1 has no stdio capability flag: stdio is the mandatory baseline transport.
    /// `true` when the driver is configured to send stdio MCP servers
    /// ([`super::McpStdioSupport::Baseline`]); `false` when the host disabled them.
    pub mcp_stdio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitializedInfo {
    pub agent_name: String,
    pub agent_version: String,
    /// Always 1: any other negotiated version is rejected.
    pub protocol_version: u16,
    pub capabilities: AgentCapabilityInfo,
    pub auth_methods: Vec<String>,
    /// Initialization never proves authentication; only session creation does.
    pub authenticated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptOutcome {
    pub turn: u64,
    pub stop_reason: StopReasonKind,
    /// The correlated `session/prompt` response carried this reason.
    pub authoritative: bool,
    /// The client sent `session/cancel` during this turn.
    pub cancel_requested: bool,
    /// Permission requests still open when the turn ended (closed as cancelled).
    pub unresolved_permissions: u32,
}

impl PromptOutcome {
    /// Only a clean, uncancelled `end_turn` makes a task eligible to quiesce.
    pub fn eligible_for_quiescence(&self) -> bool {
        self.authoritative
            && self.stop_reason == StopReasonKind::EndTurn
            && !self.cancel_requested
            && self.unresolved_permissions == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Spawn,
    Initialize,
    Authenticate,
    Session,
    Prompt,
    CancelAck,
    Idle,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureKind {
    SpawnFailed,
    /// The adapter file exists but its interpreter/runtime could not be started.
    MissingRuntime,
    ProcessExited,
    MalformedMessage,
    TruncatedMessage,
    OversizedMessage,
    OversizedOutbound,
    ProtocolViolation,
    UnsupportedVersion,
    AuthRequired,
    AuthRejected,
    SessionRejected,
    PromptRejected,
    RestoreUnsupported,
    DeadlineExceeded,
    WriteBlocked,
    EventQueueOverflow,
    /// The SDK stopped dispatching frames the adapter sent; ingress credit never returned.
    IngressStalled,
    CounterExhausted,
    Internal,
}

/// Structured, already-redacted failure; `message`/`detail` never hold secret values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{kind:?} during {phase:?}: {message}")]
pub struct AgentFailure {
    pub kind: FailureKind,
    pub phase: Phase,
    pub message: String,
    /// JSON-RPC error code reported by the adapter, when there was one.
    pub code: Option<i64>,
    pub exit_code: Option<i32>,
    /// Redacted stderr excerpt or other diagnostic context.
    pub detail: Option<String>,
}

impl AgentFailure {
    pub fn new(kind: FailureKind, phase: Phase, message: impl Into<String>) -> Self {
        Self {
            kind,
            phase,
            message: message.into(),
            code: None,
            exit_code: None,
            detail: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedInfo {
    pub late_events_rejected: u64,
    pub clean: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEventKind {
    Initialized(InitializedInfo),
    SessionReady {
        session_id: String,
        restored: bool,
    },
    Options(AgentOptions),
    /// Batched text; consecutive deltas of one role coalesce into one event.
    MessageDelta {
        role: MessageRole,
        text: String,
    },
    ToolCall(ToolEvent),
    ToolUpdate(ToolEvent),
    PermissionRequested(PermissionPrompt),
    PermissionClosed {
        id: PermissionId,
        resolution: PermissionResolution,
    },
    PromptFinished(PromptOutcome),
    /// The adapter refused a mode/config change; the session continues unchanged.
    OptionRejected {
        option: String,
        message: String,
        code: i64,
    },
    Failure(AgentFailure),
    Closed(ClosedInfo),
}

impl AgentEventKind {
    fn is_terminal(&self) -> bool {
        matches!(self, Self::Failure(_) | Self::Closed(_))
    }
}

/// Provider/session/task/sequence identity carried by every event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub provider: String,
    pub task: String,
    pub session: Option<String>,
    /// Strictly increasing per driver; coalesced deltas keep their first sequence.
    pub sequence: u64,
    pub kind: AgentEventKind,
}

#[derive(Debug)]
pub(crate) enum PushOutcome {
    Queued,
    /// Dropped because a terminal event was already queued.
    Closed,
    /// Capacity exhausted; a terminal overflow failure was queued instead.
    Overflow,
}

struct QueueInner {
    events: VecDeque<AgentEvent>,
    next_sequence: u64,
    session: Option<String>,
    /// Overflowed: only terminal events are accepted from here on.
    overflowed: bool,
    /// `Closed` was queued: nothing more is accepted.
    closed: bool,
}

/// Bounded FIFO. Deltas coalesce; every other event is critical and is never
/// dropped: when the queue is full the producer is sealed with a visible failure.
pub struct EventQueue {
    provider: String,
    task: String,
    capacity: usize,
    inner: Mutex<QueueInner>,
    ready: Condvar,
}

impl EventQueue {
    pub fn new(provider: String, task: String, capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            provider,
            task,
            capacity: capacity.max(2),
            inner: Mutex::new(QueueInner {
                events: VecDeque::new(),
                next_sequence: 1,
                session: None,
                overflowed: false,
                closed: false,
            }),
            ready: Condvar::new(),
        })
    }

    pub fn set_session(&self, session: &str) {
        self.inner.lock().session = Some(session.to_owned());
    }

    pub fn session(&self) -> Option<String> {
        self.inner.lock().session.clone()
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn task(&self) -> &str {
        &self.task
    }

    pub fn len(&self) -> usize {
        self.inner.lock().events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn push(&self, kind: AgentEventKind) -> PushOutcome {
        let mut inner = self.inner.lock();
        let terminal = kind.is_terminal();
        if inner.closed || (inner.overflowed && !terminal) {
            return PushOutcome::Closed;
        }
        if let AgentEventKind::MessageDelta { role, text } = &kind
            && let Some(AgentEvent {
                kind:
                    AgentEventKind::MessageDelta {
                        role: last_role,
                        text: last_text,
                    },
                ..
            }) = inner.events.back_mut()
            && last_role == role
            && last_text.len() + text.len() <= MAX_DELTA_EVENT_BYTES
        {
            last_text.push_str(text);
            self.ready.notify_all();
            return PushOutcome::Queued;
        }
        // Terminal events use reserved slots beyond the cap.
        if !terminal && inner.events.len() >= self.capacity {
            let failure = AgentFailure::new(
                FailureKind::EventQueueOverflow,
                Phase::Idle,
                format!(
                    "event queue exceeded {} unread events; consumer too slow",
                    self.capacity
                ),
            );
            self.append(&mut inner, AgentEventKind::Failure(failure));
            inner.overflowed = true;
            return PushOutcome::Overflow;
        }
        let closing = matches!(kind, AgentEventKind::Closed(_));
        self.append(&mut inner, kind);
        inner.closed = closing;
        PushOutcome::Queued
    }

    fn append(&self, inner: &mut QueueInner, kind: AgentEventKind) {
        let sequence = inner.next_sequence;
        inner.next_sequence = sequence.saturating_add(1);
        let event = AgentEvent {
            provider: self.provider.clone(),
            task: self.task.clone(),
            session: inner.session.clone(),
            sequence,
            kind,
        };
        inner.events.push_back(event);
        self.ready.notify_all();
    }

    pub fn try_next(&self) -> Option<AgentEvent> {
        self.inner.lock().events.pop_front()
    }

    pub fn next(&self, timeout: Duration) -> Option<AgentEvent> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.inner.lock();
        loop {
            if let Some(event) = inner.events.pop_front() {
                return Some(event);
            }
            if self.ready.wait_until(&mut inner, deadline).timed_out() {
                return inner.events.pop_front();
            }
        }
    }

    pub fn drain(&self, max: usize) -> Vec<AgentEvent> {
        let mut inner = self.inner.lock();
        let take = max.min(inner.events.len());
        inner.events.drain(..take).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(text: &str) -> AgentEventKind {
        AgentEventKind::MessageDelta {
            role: MessageRole::Agent,
            text: text.into(),
        }
    }

    #[test]
    fn deltas_batch_into_one_event_with_a_single_sequence() {
        let queue = EventQueue::new("p".into(), "t".into(), 8);
        for _ in 0..1000 {
            assert!(matches!(queue.push(delta("ab")), PushOutcome::Queued));
        }
        assert_eq!(queue.len(), 1000 * 2 / MAX_DELTA_EVENT_BYTES + 1);
        let first = queue.try_next().unwrap();
        assert_eq!(first.sequence, 1);
        assert_eq!(first.provider, "p");
        assert_eq!(first.task, "t");
    }

    #[test]
    fn delta_after_a_critical_event_starts_a_new_event_and_sequences_increase() {
        let queue = EventQueue::new("p".into(), "t".into(), 8);
        queue.push(delta("a"));
        queue.push(AgentEventKind::ToolCall(ToolEvent {
            tool_call_id: "1".into(),
            title: None,
            kind: None,
            status: None,
        }));
        queue.push(delta("b"));
        let seqs: Vec<u64> = queue.drain(10).iter().map(|e| e.sequence).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
    }

    #[test]
    fn critical_overflow_fails_visibly_instead_of_dropping() {
        let queue = EventQueue::new("p".into(), "t".into(), 4);
        for i in 0..4 {
            assert!(matches!(
                queue.push(AgentEventKind::ToolCall(ToolEvent {
                    tool_call_id: i.to_string(),
                    title: None,
                    kind: None,
                    status: None,
                })),
                PushOutcome::Queued
            ));
        }
        let overflow = queue.push(AgentEventKind::PromptFinished(PromptOutcome {
            turn: 1,
            stop_reason: StopReasonKind::EndTurn,
            authoritative: true,
            cancel_requested: false,
            unresolved_permissions: 0,
        }));
        assert!(matches!(overflow, PushOutcome::Overflow));
        assert!(matches!(queue.push(delta("x")), PushOutcome::Closed));
        let events = queue.drain(100);
        assert_eq!(events.len(), 5);
        assert!(matches!(
            &events[4].kind,
            AgentEventKind::Failure(f) if f.kind == FailureKind::EventQueueOverflow
        ));
        // Terminal events are still accepted after an overflow, then nothing more.
        assert!(matches!(
            queue.push(AgentEventKind::Closed(ClosedInfo {
                late_events_rejected: 0,
                clean: false
            })),
            PushOutcome::Queued
        ));
        assert!(matches!(queue.push(delta("late")), PushOutcome::Closed));
    }

    #[test]
    fn eligibility_requires_clean_authoritative_end_turn() {
        let mut outcome = PromptOutcome {
            turn: 1,
            stop_reason: StopReasonKind::EndTurn,
            authoritative: true,
            cancel_requested: false,
            unresolved_permissions: 0,
        };
        assert!(outcome.eligible_for_quiescence());
        outcome.cancel_requested = true;
        assert!(!outcome.eligible_for_quiescence());
        outcome.cancel_requested = false;
        outcome.stop_reason = StopReasonKind::Refusal;
        assert!(!outcome.eligible_for_quiescence());
        outcome.stop_reason = StopReasonKind::EndTurn;
        outcome.unresolved_permissions = 1;
        assert!(!outcome.eligible_for_quiescence());
    }
}
