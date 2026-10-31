//! The loop thread: writes `initialize`, dispatches every stdout frame in arrival order
//! and writes host lines, interrupts and permission answers.

use super::super::*;
use super::{CONTINUE_WAIT, Input, STEER_CONFIRM_WAIT, WireState, wire};
use crate::driver::{QuestionItem, QuestionPrompt};
use futures::SinkExt;
use serde_json::Value;
use std::sync::mpsc as std_mpsc;

pub(super) struct Params {
    pub(super) mcp_stdio: bool,
    pub(super) resumed: bool,
    /// The user's servers written into the launch's MCP config, by name.
    pub(super) user_servers: Vec<String>,
    /// Where permission answers queue their lines.
    pub(super) writer: std_mpsc::SyncSender<Input>,
}

/// Moves stdout lines from the transport onto the loop's input queue.
pub(super) fn spawn_forwarder(
    lines: mpsc::Receiver<std::io::Result<String>>,
    input: std_mpsc::SyncSender<Input>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("claude-lines".into())
        .spawn(move || {
            for line in futures::executor::block_on_stream(lines) {
                let Ok(line) = line else { break };
                if input.send(Input::Line(line)).is_err() {
                    return;
                }
            }
            let _ = input.send(Input::Eof);
        })
        .expect("spawn claude line forwarder")
}

/// Answers one `can_use_tool` request by queueing its line for the loop. Never blocks and
/// never takes `core` (it runs under it).
struct ClaudeSink {
    request_id: String,
    input: Value,
    questions: Vec<QuestionItem>,
    writer: std_mpsc::SyncSender<Input>,
}

/// The option ids a Claude permission request offers.
const ALLOW: &str = "allow";
const DENY: &str = "deny";

impl PermissionSink for ClaudeSink {
    fn respond(self: Box<Self>, answer: WireAnswer) -> Result<(), SinkError> {
        let line = match answer {
            WireAnswer::Selected(option) if option == ALLOW => wire::allow_line(&self.request_id),
            WireAnswer::Selected(_) => {
                wire::deny_line(&self.request_id, "The user declined this tool call.")
            }
            WireAnswer::Cancelled => {
                wire::deny_line(&self.request_id, "The request was cancelled.")
            }
            WireAnswer::Answers(answers) => {
                wire::answers_line(&self.request_id, &self.input, &self.questions, &answers)
            }
        };
        self.writer.try_send(Input::Write(line)).map_err(|_| {
            SinkError::Failed(AgentFailure::new(
                FailureKind::WriteBlocked,
                Phase::Prompt,
                "the Claude Code input queue is full",
            ))
        })
    }
}

struct Connection {
    shared: Arc<Shared>,
    state: Arc<Mutex<WireState>>,
    sink: LineSink,
    params: Params,
    initialized: bool,
    next_request: u64,
    /// The turn the agent started by itself, if the open turn is one.
    autonomous: Option<u64>,
    /// Waiting for Claude to start its own turn after a background task finished.
    continue_deadline: Option<Instant>,
    /// A turn end held until the steers sent into it are taken up.
    held_result: Option<(Value, Instant)>,
    /// The command list last sent, so an unchanged `init` frame sends nothing.
    commands: Vec<CommandInfo>,
}

pub(super) fn run(
    shared: Arc<Shared>,
    state: Arc<Mutex<WireState>>,
    sink: LineSink,
    input: std_mpsc::Receiver<Input>,
    params: Params,
) {
    let mut connection = Connection {
        shared: shared.clone(),
        state,
        sink,
        params,
        initialized: false,
        next_request: 1,
        autonomous: None,
        continue_deadline: None,
        held_result: None,
        commands: Vec::new(),
    };
    let result = connection.drive(&input);
    // Closing stdin lets Claude exit by itself; teardown then verifies the tree.
    drop(connection);
    match result {
        Err(failure) => shared.fail(failure),
        Ok(()) if !shared.is_stopping() && shared.core.lock().failure.is_none() => {
            // The input channel closed without a stop request: the owner is gone.
            shared.core.lock().stopping = true;
        }
        Ok(()) => {}
    }
    shared.finish();
}

fn violation(message: impl Into<String>) -> AgentFailure {
    AgentFailure::new(FailureKind::ProtocolViolation, Phase::Prompt, message)
}

impl Connection {
    fn drive(&mut self, input: &std_mpsc::Receiver<Input>) -> Result<(), AgentFailure> {
        self.shared.set_phase(Phase::Initialize);
        self.write(wire::initialize_line())?;
        loop {
            let deadline = match (self.continue_deadline, &self.held_result) {
                (Some(a), Some((_, b))) => Some(a.min(*b)),
                (a, b) => a.or(b.as_ref().map(|(_, at)| *at)),
            };
            let next = match deadline {
                Some(at) => {
                    match input.recv_timeout(at.saturating_duration_since(Instant::now())) {
                        Ok(next) => next,
                        Err(std_mpsc::RecvTimeoutError::Timeout) => {
                            self.on_deadlines()?;
                            continue;
                        }
                        Err(std_mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                    }
                }
                None => match input.recv() {
                    Ok(next) => next,
                    Err(_) => return Ok(()),
                },
            };
            match next {
                Input::Line(line) => {
                    self.shared.ingress.release_one();
                    if self.shared.is_stopping() {
                        self.shared.late.fetch_add(1, Ordering::SeqCst);
                        continue;
                    }
                    let frame: Value = serde_json::from_str(&line).map_err(|error| {
                        AgentFailure::new(
                            FailureKind::MalformedMessage,
                            self.shared.phase(),
                            format!("message is not valid JSON: {error}"),
                        )
                    })?;
                    self.on_frame(frame)?;
                }
                Input::Eof => {
                    return if self.shared.is_stopping() {
                        Ok(())
                    } else {
                        Err(self.shared.process_exit_failure())
                    };
                }
                Input::Write(line) => self.write(line)?,
                Input::Wake => self.send_cancel()?,
                Input::Shutdown => {
                    let _ = self.shared.latch_cancel(true);
                    let _ = self.send_cancel();
                    self.shared.close_permissions();
                    return Ok(());
                }
            }
        }
    }

    fn write(&mut self, line: String) -> Result<(), AgentFailure> {
        futures::executor::block_on(self.sink.send(line)).map_err(|error| {
            // The sink latched oversize and blocked writes itself; a closed pipe is an exit.
            self.shared.core.lock().failure.clone().unwrap_or_else(|| {
                AgentFailure::new(
                    FailureKind::ProcessExited,
                    self.shared.phase(),
                    format!("Claude Code stdin closed: {error}"),
                )
            })
        })
    }

    /// A latched Stop is written here, bound to the turn that was stopped.
    fn send_cancel(&mut self) -> Result<(), AgentFailure> {
        if self.shared.take_cancel_to_send().is_some() {
            let id = format!("fframes_interrupt_{}", self.next_request);
            self.next_request += 1;
            self.write(wire::interrupt_line(&id))?;
        }
        Ok(())
    }

    fn on_deadlines(&mut self) -> Result<(), AgentFailure> {
        let now = Instant::now();
        if self.continue_deadline.is_some_and(|at| at <= now) {
            // Claude did not continue: nothing is pending any more.
            self.continue_deadline = None;
            self.state.lock().continuing = false;
            self.emit_background();
        }
        if self.held_result.as_ref().is_some_and(|(_, at)| *at <= now) {
            let (result, _) = self.held_result.take().expect("checked");
            // The steers were never taken up; the turn ends as Claude reported it.
            self.state.lock().pending_steers.clear();
            self.finish(&result)?;
        }
        Ok(())
    }

    fn emit_background(&self) {
        let (tasks, continuing) = {
            let state = self.state.lock();
            (state.background, state.continuing)
        };
        self.shared
            .emit(AgentEventKind::Background { tasks, continuing });
    }

    fn turn_open(&self) -> bool {
        self.shared.core.lock().turn.is_some()
    }

    fn check_session(&self, frame: &Value) -> Result<(), AgentFailure> {
        let Some(id) = frame.get("session_id").and_then(Value::as_str) else {
            return Ok(());
        };
        if self.shared.core.lock().session.as_deref() != Some(id) {
            return Err(violation("Claude Code reported another session id"));
        }
        Ok(())
    }

    fn on_frame(&mut self, frame: Value) -> Result<(), AgentFailure> {
        match frame
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "control_response" => self.on_control_response(&frame),
            "control_request" => self.on_control_request(&frame),
            "control_cancel_request" => {
                if let Some(id) = frame.get("request_id").and_then(Value::as_str) {
                    self.shared.withdraw_pending(id);
                }
                Ok(())
            }
            "system" => self.on_system(&frame),
            "stream_event" => {
                self.on_stream_event(&frame);
                Ok(())
            }
            "assistant" => {
                self.on_assistant(&frame);
                Ok(())
            }
            "user" => {
                self.on_user(&frame);
                Ok(())
            }
            "result" => self.on_result(frame),
            // Rate limits, keep-alives and anything newer carry no workflow authority.
            _ => Ok(()),
        }
    }

    fn on_control_response(&mut self, frame: &Value) -> Result<(), AgentFailure> {
        let response = frame.get("response").unwrap_or(&Value::Null);
        // Claude echoes the answers it was sent; only the initialize reply is awaited.
        if response.get("request_id").and_then(Value::as_str) != Some(wire::INITIALIZE_ID)
            || self.initialized
        {
            return Ok(());
        }
        if response.get("subtype").and_then(Value::as_str) != Some("success") {
            let message = response
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Claude Code refused to initialize");
            return Err(AgentFailure::new(
                FailureKind::ProtocolViolation,
                Phase::Initialize,
                message,
            ));
        }
        let body = response.get("response").unwrap_or(&Value::Null);
        let version = body
            .get("claude_code_version")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if let Some(parsed) = super::launch::parse_version(version) {
            super::launch::check_version(parsed).map_err(|message| {
                AgentFailure::new(FailureKind::UnsupportedVersion, Phase::Initialize, message)
            })?;
        }
        let info = InitializedInfo {
            agent_name: "Claude Code".to_owned(),
            agent_version: self.shared.clean(version),
            protocol_version: 1,
            capabilities: AgentCapabilityInfo {
                load_session: false,
                resume_session: true,
                prompt_image: true,
                prompt_audio: false,
                prompt_embedded_context: false,
                // Its MCP config takes streamable-HTTP servers.
                mcp_http: true,
                mcp_sse: false,
                mcp_stdio: self.params.mcp_stdio,
            },
            auth_methods: Vec::new(),
            // Initialization never proves a login; the first turn does.
            authenticated: false,
        };
        self.initialized = true;
        let session = self.shared.core.lock().session.clone().unwrap_or_default();
        let exported = self.shared.clean(&session);
        {
            let mut core = self.shared.core.lock();
            core.initialized = Some(info.clone());
            core.restored = self.params.resumed;
        }
        self.shared.queue.set_session(&exported);
        self.shared.emit(AgentEventKind::Initialized(info));
        self.shared.emit(AgentEventKind::SessionReady {
            session_id: exported,
            restored: self.params.resumed,
            user_mcp_servers: self.params.user_servers.clone(),
        });
        self.shared.watchdog.disarm();
        self.shared.set_ready();
        Ok(())
    }

    fn on_control_request(&mut self, frame: &Value) -> Result<(), AgentFailure> {
        let request_id = frame
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let request = frame.get("request").unwrap_or(&Value::Null);
        if request.get("subtype").and_then(Value::as_str) != Some("can_use_tool") {
            // Hooks and SDK MCP servers are never registered, so nothing else is expected;
            // an unanswered request would hang the CLI.
            self.shared.declined.fetch_add(1, Ordering::SeqCst);
            return self.write(wire::control_error(
                &request_id,
                "not supported by this client",
            ));
        }
        self.shared.flush_before_event();
        let tool = request
            .get("tool_name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let input = request.get("input").cloned().unwrap_or(Value::Null);
        let tool_call_id = self.shared.clean(
            request
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        if tool == "AskUserQuestion" {
            let questions = wire::parse_questions(&input);
            // Claude keys its answers by question text: no question, or two with the same
            // text (or none), cannot be answered unambiguously. Decline at once so the
            // agent is told instead of waiting for an answer nobody can give.
            if let Some(reason) = wire::unanswerable_questions(&questions) {
                let _ = self
                    .params
                    .writer
                    .try_send(Input::Write(wire::deny_line(&request_id, reason)));
                return Ok(());
            }
            let exported: Vec<QuestionItem> = questions
                .iter()
                .map(|question| QuestionItem {
                    header: self.shared.clean(&question.header),
                    question: self.shared.clean(&question.question),
                    multi_select: question.multi_select,
                    options: question
                        .options
                        .iter()
                        .map(|option| crate::driver::QuestionOption {
                            label: self.shared.clean(&option.label),
                            description: self.shared.clean(&option.description),
                        })
                        .collect(),
                })
                .collect();
            let sink = Box::new(ClaudeSink {
                request_id: request_id.clone(),
                input,
                questions,
                writer: self.params.writer.clone(),
            });
            return match self
                .shared
                .admit_pending(request_id, Vec::new(), sink, true)
            {
                Err(message) => Err(violation(message)),
                Ok(None) => Ok(()),
                Ok(Some((id, turn))) => {
                    self.shared.emit(AgentEventKind::Question(QuestionPrompt {
                        id,
                        turn,
                        title: "Claude has a question".to_owned(),
                        questions: exported,
                    }));
                    Ok(())
                }
            };
        }
        let display = request
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(tool);
        let title = match request.get("description").and_then(Value::as_str) {
            Some(description) if !description.is_empty() => format!("{display}: {description}"),
            _ => display.to_owned(),
        };
        let (kind, locations) = wire::tool_kind_and_paths(tool, &input);
        let sink = Box::new(ClaudeSink {
            request_id: request_id.clone(),
            input,
            questions: Vec::new(),
            writer: self.params.writer.clone(),
        });
        let options = vec![
            (ALLOW.to_owned(), ALLOW.to_owned()),
            (DENY.to_owned(), DENY.to_owned()),
        ];
        match self.shared.admit_pending(request_id, options, sink, false) {
            Err(message) => Err(violation(message)),
            Ok(None) => Ok(()),
            Ok(Some((id, turn))) => {
                self.shared
                    .emit(AgentEventKind::PermissionRequested(PermissionPrompt {
                        id,
                        turn,
                        tool_call_id,
                        title: self.shared.clean(&title),
                        options: vec![
                            PermissionChoice {
                                option_id: ALLOW.to_owned(),
                                name: "Allow".to_owned(),
                                kind: "allow_once".to_owned(),
                            },
                            PermissionChoice {
                                option_id: DENY.to_owned(),
                                name: "Deny".to_owned(),
                                kind: "reject_once".to_owned(),
                            },
                        ],
                        kind: Some(kind),
                        locations,
                    }));
                Ok(())
            }
        }
    }

    fn on_system(&mut self, frame: &Value) -> Result<(), AgentFailure> {
        self.check_session(frame)?;
        match frame
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "init" => {
                self.on_commands(frame);
                // A held turn end is superseded: the steer started another step of it.
                if self.held_result.take().is_some() {
                    return Ok(());
                }
                if !self.turn_open() {
                    // Claude continues by itself after a background task finished.
                    self.continue_deadline = None;
                    let turn = self.shared.begin_turn().map_err(|error| {
                        violation(format!(
                            "Claude Code started a turn that cannot open: {error}"
                        ))
                    })?;
                    self.state.lock().continuing = true;
                    self.autonomous = Some(turn);
                    self.shared.emit(AgentEventKind::TurnStarted {
                        turn,
                        origin: TurnOrigin::Autonomous,
                    });
                }
                Ok(())
            }
            "background_tasks_changed" => {
                let tasks = frame
                    .get("tasks")
                    .and_then(Value::as_array)
                    .map_or(0, |tasks| u32::try_from(tasks.len()).unwrap_or(u32::MAX));
                self.state.lock().background = tasks;
                self.emit_background();
                Ok(())
            }
            "task_notification" => {
                if !self.turn_open() {
                    self.state.lock().continuing = true;
                    self.continue_deadline = Instant::now().checked_add(CONTINUE_WAIT);
                    self.emit_background();
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// The slash commands Claude Code lists in its `init` frame. Each entry is a bare name
    /// or an object with a name, description and argument hint; the list is sent again
    /// only when it changed.
    fn on_commands(&mut self, frame: &Value) {
        let Some(listed) = frame.get("slash_commands").and_then(Value::as_array) else {
            return;
        };
        let commands: Vec<CommandInfo> = listed
            .iter()
            .filter_map(|entry| {
                let (name, description, hint) = match entry {
                    Value::String(name) => (name.as_str(), "", None),
                    Value::Object(_) => (
                        entry.get("name").and_then(Value::as_str)?,
                        entry
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        entry
                            .get("argumentHint")
                            .or_else(|| entry.get("argument_hint"))
                            .and_then(Value::as_str)
                            .filter(|hint| !hint.is_empty()),
                    ),
                    _ => return None,
                };
                let name = self.shared.clean(name.trim_start_matches('/'));
                CommandInfo::is_valid_name(&name).then(|| CommandInfo {
                    name,
                    description: self.shared.clean(description),
                    input_hint: hint.map(|hint| self.shared.clean(hint)),
                })
            })
            .take(MAX_COMMANDS)
            .collect();
        if commands == self.commands {
            return;
        }
        self.commands = commands.clone();
        self.shared.flush_before_event();
        self.shared
            .emit(AgentEventKind::AvailableCommands(commands));
    }

    /// Whether a frame belongs to the main conversation (not a subagent's). Subagent
    /// frames only show that the turn is alive.
    fn top_level(&self, frame: &Value) -> bool {
        let top = frame.get("parent_tool_use_id").is_none_or(Value::is_null);
        if !top && self.turn_open() {
            self.shared.note_turn_activity();
        }
        top
    }

    fn on_stream_event(&mut self, frame: &Value) {
        if !self.top_level(frame) {
            return;
        }
        let Some(delta) = frame
            .get("event")
            .filter(|event| {
                event.get("type").and_then(Value::as_str) == Some("content_block_delta")
            })
            .and_then(|event| event.get("delta"))
        else {
            return;
        };
        let (role, text) = match delta.get("type").and_then(Value::as_str) {
            Some("text_delta") => (MessageRole::Agent, delta.get("text")),
            Some("thinking_delta") => (MessageRole::Thought, delta.get("thinking")),
            _ => return,
        };
        let Some(text) = text.and_then(Value::as_str).filter(|text| !text.is_empty()) else {
            return;
        };
        if !self.turn_open() {
            self.shared.late.fetch_add(1, Ordering::SeqCst);
            return;
        }
        self.shared.note_turn_activity();
        self.shared.push_text(role, text);
    }

    fn on_assistant(&mut self, frame: &Value) {
        if !self.top_level(frame) {
            return;
        }
        let Some(blocks) = frame.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            if !self.turn_open() {
                self.shared.late.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            self.shared.note_turn_activity();
            let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
            let name = block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let input = block.get("input").unwrap_or(&Value::Null);
            self.shared.flush_before_event();
            if name == "TodoWrite" {
                self.shared
                    .emit(AgentEventKind::Plan(plan_entries(&self.shared, input)));
                continue;
            }
            self.state.lock().open_tools.insert(id.to_owned());
            let title = match input.get("description").and_then(Value::as_str) {
                Some(description) if !description.is_empty() => format!("{name}: {description}"),
                _ => name.to_owned(),
            };
            self.shared.emit(AgentEventKind::ToolCall(ToolEvent {
                tool_call_id: self.shared.clean(id),
                title: Some(self.shared.clean(&title)),
                kind: Some(tool_kind(name).to_owned()),
                status: Some(ToolStatus::Pending),
            }));
        }
    }

    fn on_user(&mut self, frame: &Value) {
        if frame.get("isReplay").and_then(Value::as_bool) == Some(true) {
            if let Some(uuid) = frame.get("uuid").and_then(Value::as_str) {
                let confirmed = self.state.lock().pending_steers.remove(uuid);
                // The steer was taken up after Claude reported the end: the turn goes on.
                if confirmed {
                    self.held_result = None;
                }
            }
            return;
        }
        if !self.top_level(frame) {
            return;
        }
        let Some(blocks) = frame.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let id = block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !self.state.lock().open_tools.remove(id) || !self.turn_open() {
                continue;
            }
            self.shared.note_turn_activity();
            self.shared.flush_before_event();
            let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
            self.shared.emit(AgentEventKind::ToolUpdate(ToolEvent {
                tool_call_id: self.shared.clean(id),
                title: None,
                kind: None,
                status: Some(if failed {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Completed
                }),
            }));
        }
    }

    fn on_result(&mut self, frame: Value) -> Result<(), AgentFailure> {
        self.check_session(&frame)?;
        if !self.turn_open() {
            self.shared.late.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        if !self.state.lock().pending_steers.is_empty() {
            // A steer is still queued in Claude: it will continue the turn.
            self.held_result = Instant::now()
                .checked_add(STEER_CONFIRM_WAIT)
                .map(|at| (frame, at));
            return Ok(());
        }
        self.finish(&frame)
    }

    fn finish(&mut self, result: &Value) -> Result<(), AgentFailure> {
        let (turn, cancelled) = {
            let core = self.shared.core.lock();
            match &core.turn {
                Some(turn) => (turn.id, turn.cancel_requested),
                None => return Ok(()),
            }
        };
        let text = result
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let is_error = result.get("is_error").and_then(Value::as_bool) == Some(true);
        if is_error && text.contains("Not logged in") {
            return Err(AgentFailure::new(
                FailureKind::AuthRequired,
                Phase::Prompt,
                "Claude Code is not signed in; run `claude` and log in",
            ));
        }
        let subtype = result
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (stop_reason, incomplete) = if cancelled {
            (StopReasonKind::Cancelled, false)
        } else if subtype == "error_max_turns" {
            (StopReasonKind::MaxTurnRequests, false)
        } else if is_error || subtype != "success" {
            (StopReasonKind::EndTurn, true)
        } else {
            match result.get("stop_reason").and_then(Value::as_str) {
                Some("max_tokens") => (StopReasonKind::MaxTokens, false),
                Some("refusal") => (StopReasonKind::Refusal, false),
                _ => (StopReasonKind::EndTurn, false),
            }
        };
        let origin = if self.autonomous.take() == Some(turn) {
            TurnOrigin::Autonomous
        } else {
            TurnOrigin::Prompted
        };
        {
            let mut state = self.state.lock();
            state.open_tools.clear();
            state.pending_steers.clear();
        }
        self.shared
            .finish_turn(turn, stop_reason, incomplete, origin);
        if origin == TurnOrigin::Autonomous {
            self.state.lock().continuing = false;
            self.emit_background();
        }
        Ok(())
    }
}

/// The ACP tool kind closest to a Claude Code tool.
fn tool_kind(name: &str) -> &'static str {
    match name {
        "Read" | "NotebookRead" => "read",
        "Edit" | "Write" | "NotebookEdit" | "MultiEdit" => "edit",
        "Glob" | "Grep" => "search",
        "Bash" | "PowerShell" | "BashOutput" => "execute",
        "WebFetch" | "WebSearch" => "fetch",
        "Task" | "Agent" => "think",
        _ => "other",
    }
}

fn plan_entries(shared: &Shared, input: &Value) -> Vec<PlanEntry> {
    input
        .get("todos")
        .and_then(Value::as_array)
        .map(|todos| {
            todos
                .iter()
                .map(|todo| PlanEntry {
                    text: shared.clean(
                        todo.get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    ),
                    status: match todo.get("status").and_then(Value::as_str) {
                        Some("completed") => PlanStatus::Completed,
                        Some("in_progress") => PlanStatus::InProgress,
                        _ => PlanStatus::Pending,
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}
