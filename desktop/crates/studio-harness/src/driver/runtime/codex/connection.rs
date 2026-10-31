//! The loop thread: runs the handshake, dispatches every stdout message in arrival order
//! and writes host requests, interrupts and approval answers.

use super::super::*;
use super::{Input, launch, rpc};
use crate::driver::{QuestionItem, QuestionPrompt};
use futures::SinkExt;
use serde_json::{Value, json};
use std::sync::mpsc as std_mpsc;
use std::time::Instant;

pub(super) struct Params {
    /// The thread's working directory: the task's draft.
    pub(super) cwd: PathBuf,
    pub(super) version: (u64, u64, u64),
    pub(super) mode: DriverMode,
    pub(super) resume: Option<String>,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) servers: Vec<McpStdioServer>,
    /// The user's own servers, stdio or HTTP, with the tools they turned off.
    pub(super) user_servers: Vec<UserMcpLaunch>,
    pub(super) mcp_stdio: bool,
    /// Where approval answers queue their lines.
    pub(super) writer: std_mpsc::SyncSender<Input>,
}

/// Moves stdout lines from the transport onto the loop's input queue.
pub(super) fn spawn_forwarder(
    lines: mpsc::Receiver<std::io::Result<String>>,
    input: std_mpsc::SyncSender<Input>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("codex-lines".into())
        .spawn(move || {
            for line in futures::executor::block_on_stream(lines) {
                let Ok(line) = line else { break };
                if input.send(Input::Line(line)).is_err() {
                    return;
                }
            }
            let _ = input.send(Input::Eof);
        })
        .expect("spawn codex line forwarder")
}

/// Whether an `mcpServer/elicitation/request` is Codex asking to run an MCP tool
/// (`_meta.codex_approval_kind`, codex-rs `protocol/src/mcp_approval_meta.rs`), not a
/// server's own question.
fn is_tool_call_approval(params: &Value) -> bool {
    params
        .pointer("/_meta/codex_approval_kind")
        .and_then(Value::as_str)
        == Some("mcp_tool_call")
}

/// The tool an MCP tool-call approval is for: `_meta.tool_name` (current codex-rs), or the
/// name in the message Codex 0.163 sends without it, which must be exactly
/// `Allow the <server> MCP server to run tool "<tool>"?` (codex-rs
/// `build_mcp_tool_approval_fallback_message`).
fn approval_tool<'a>(params: &'a Value, session_name: &str) -> Option<&'a str> {
    let named = str_at(params, "/_meta/tool_name");
    if !named.is_empty() {
        return Some(named);
    }
    str_at(params, "/message")
        .strip_prefix(&format!(
            "Allow the {session_name} MCP server to run tool \""
        ))?
        .strip_suffix("\"?")
        .filter(|tool| !tool.is_empty() && !tool.contains('"'))
}

/// The option ids a Codex approval offers.
const ACCEPT: &str = "accept";
const DECLINE: &str = "decline";

enum Answer {
    /// A command or file-change approval.
    Decision,
    /// An MCP tool-call approval (an elicitation): accepted for this call only.
    ToolCall,
    /// `item/permissions/requestApproval`: the profile granted on accept.
    Permissions(Value),
    Questions(Vec<rpc::WireQuestion>),
}

/// Answers one server request by queueing its line for the loop. Never blocks and never
/// takes `core` (it runs under it).
struct CodexSink {
    id: Value,
    answer: Answer,
    writer: std_mpsc::SyncSender<Input>,
}

impl PermissionSink for CodexSink {
    fn respond(self: Box<Self>, answer: WireAnswer) -> Result<(), SinkError> {
        let accepted = matches!(&answer, WireAnswer::Selected(option) if option == ACCEPT);
        // A request closed as cancelled is declined: Codex's `cancel` would also interrupt
        // the turn, and a real Stop sends `turn/interrupt` itself.
        let result = match (&self.answer, answer) {
            (Answer::Decision, _) => rpc::decision(accepted),
            (Answer::ToolCall, _) => rpc::tool_call_approval(accepted),
            (Answer::Permissions(profile), _) => rpc::permissions(accepted.then_some(profile)),
            (Answer::Questions(questions), WireAnswer::Answers(answers)) => {
                rpc::answers(questions, &answers)
            }
            (Answer::Questions(_), _) => json!({"answers": {}}),
        };
        self.writer
            .try_send(Input::Write(rpc::response(&self.id, result)))
            .map_err(|_| {
                SinkError::Failed(AgentFailure::new(
                    FailureKind::WriteBlocked,
                    Phase::Prompt,
                    "the Codex input queue is full",
                ))
            })
    }
}

/// What a request the driver sent is waiting for.
enum Pending {
    Initialize,
    AccountRead,
    ConfigRead,
    Thread {
        resume: bool,
    },
    /// `slash`: a slash command's request, whose refusal ends only its turn.
    TurnStart {
        turn: u64,
        slash: bool,
    },
    /// The steer's text, to report with the answer.
    Steer(String),
    Interrupt,
}

/// The running Codex turn.
#[derive(Default)]
struct CodexTurn {
    /// Codex's turn id, from the request's reply or `turn/started`.
    id: Option<String>,
    /// The id `turn/started` announced. A review announces another id than its reply
    /// and is interrupted by this one; its `turn/completed` carries the reply's.
    announced: Option<String>,
    /// `turn/started` arrived: only now does `turn/interrupt` find the turn.
    started: bool,
    /// Stop was latched before the turn started.
    interrupt_due: bool,
    /// A steer asked before `turn/started`, with the time it was asked: sent once the turn
    /// is announced, refused (and never sent later) if the turn ends or the request
    /// timeout passes first.
    parked_steer: Option<(String, Instant)>,
}

struct Connection {
    shared: Arc<Shared>,
    sink: LineSink,
    params: Params,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    thread: Option<String>,
    turn: CodexTurn,
    /// Tool items of the open turn that are not completed yet.
    open_items: HashSet<String>,
    /// Command items of the open turn still running.
    running: HashSet<String>,
    /// Commands that outlived their turn: background work until their `item/completed`.
    background: HashSet<String>,
    /// File paths of each file-change item, for its approval title.
    file_changes: HashMap<String, Vec<String>>,
    /// Agent messages that streamed deltas (their completed text is not repeated).
    streamed: HashSet<String>,
    /// Servers Codex knows by another name (session name → own name).
    renamed: std::collections::BTreeMap<String, String>,
}

pub(super) fn run(
    shared: Arc<Shared>,
    sink: LineSink,
    input: std_mpsc::Receiver<Input>,
    params: Params,
) {
    let mut connection = Connection {
        shared: shared.clone(),
        sink,
        params,
        next_id: 1,
        pending: HashMap::new(),
        thread: None,
        turn: CodexTurn::default(),
        open_items: HashSet::new(),
        running: HashSet::new(),
        background: HashSet::new(),
        file_changes: HashMap::new(),
        streamed: HashSet::new(),
        renamed: Default::default(),
    };
    let result = connection.drive(&input);
    // Closing stdin ends the app-server; teardown then verifies the tree.
    drop(connection);
    match result {
        Err(failure) => shared.fail(failure),
        Ok(()) if !shared.is_stopping() && shared.core.lock().failure.is_none() => {
            shared.core.lock().stopping = true;
        }
        Ok(()) => {}
    }
    shared.finish();
}

fn violation(message: impl Into<String>) -> AgentFailure {
    AgentFailure::new(FailureKind::ProtocolViolation, Phase::Prompt, message)
}

fn str_at<'a>(value: &'a Value, pointer: &str) -> &'a str {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

impl Connection {
    fn drive(&mut self, input: &std_mpsc::Receiver<Input>) -> Result<(), AgentFailure> {
        self.shared.set_phase(Phase::Initialize);
        self.request(Pending::Initialize, "initialize", rpc::initialize_params())?;
        loop {
            let next = match self.turn.parked_steer.as_ref() {
                // A parked steer must not wait for a turn announcement that never comes.
                Some((_, asked)) => {
                    let wait = self
                        .shared
                        .limits
                        .request_timeout
                        .saturating_sub(asked.elapsed());
                    match input.recv_timeout(wait) {
                        Ok(next) => next,
                        Err(std_mpsc::RecvTimeoutError::Timeout) => {
                            if let Some((text, _)) = self.turn.parked_steer.take() {
                                self.steer_result(
                                    text,
                                    Err("Codex did not announce the turn in time"),
                                );
                            }
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
                    let message: Value = serde_json::from_str(&line).map_err(|error| {
                        AgentFailure::new(
                            FailureKind::MalformedMessage,
                            self.shared.phase(),
                            format!("message is not valid JSON: {error}"),
                        )
                    })?;
                    self.on_message(message)?;
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
                Input::Prompt { turn, text, images } => self.start_turn(turn, &text, &images)?,
                Input::Steer { text } => self.steer(text)?,
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
            self.shared.core.lock().failure.clone().unwrap_or_else(|| {
                AgentFailure::new(
                    FailureKind::ProcessExited,
                    self.shared.phase(),
                    format!("Codex stdin closed: {error}"),
                )
            })
        })
    }

    fn request(
        &mut self,
        pending: Pending,
        method: &str,
        params: Value,
    ) -> Result<(), AgentFailure> {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, pending);
        self.write(rpc::request(id, method, params))
    }

    fn thread_id(&self) -> String {
        self.thread.clone().unwrap_or_default()
    }

    // ---- host requests ---------------------------------------------------------------

    fn start_turn(
        &mut self,
        turn: u64,
        text: &str,
        images: &[PathBuf],
    ) -> Result<(), AgentFailure> {
        if self.shared.core.lock().turn.as_ref().map(|t| t.id) != Some(turn) {
            // Stopped before it was sent.
            return Ok(());
        }
        self.turn = CodexTurn::default();
        if images.is_empty()
            && let Some((method, params)) = rpc::slash_command(&self.thread_id(), text)
        {
            return self.request(Pending::TurnStart { turn, slash: true }, method, params);
        }
        let images: Vec<&std::path::Path> = images.iter().map(PathBuf::as_path).collect();
        let mut params = json!({
            "threadId": self.thread_id(),
            "input": rpc::input(text, &images),
            "approvalPolicy": launch::APPROVAL_POLICY,
            "sandboxPolicy": launch::sandbox_policy(),
            "summary": "auto",
        });
        if let Some(effort) = &self.params.effort {
            params["effort"] = Value::from(effort.clone());
        }
        self.request(
            Pending::TurnStart { turn, slash: false },
            "turn/start",
            params,
        )
    }

    /// Reports what became of one steer.
    fn steer_result(&self, text: String, outcome: Result<(), &str>) {
        let (accepted, message) = match outcome {
            Ok(()) => (true, String::new()),
            Err(message) => (false, message.to_owned()),
        };
        self.shared.emit(AgentEventKind::SteerResult {
            text,
            accepted,
            message,
        });
    }

    fn steer(&mut self, text: String) -> Result<(), AgentFailure> {
        // Only a turn Codex announced and that is not being stopped can take a steer.
        let open = self
            .shared
            .core
            .lock()
            .turn
            .as_ref()
            .is_some_and(|turn| !turn.cancel_requested);
        if open && !self.turn.started && self.turn.parked_steer.is_none() {
            self.turn.parked_steer = Some((text, Instant::now()));
            return Ok(());
        }
        let Some(expected) = self.turn.id.clone().filter(|_| self.turn.started && open) else {
            self.steer_result(
                text,
                Err("The agent is not running a turn that takes a message"),
            );
            return Ok(());
        };
        let params = json!({
            "threadId": self.thread_id(),
            "expectedTurnId": expected,
            "input": rpc::input(&text, &[]),
        });
        self.request(Pending::Steer(text), "turn/steer", params)
    }

    /// A latched Stop is written once Codex announced the turn.
    fn send_cancel(&mut self) -> Result<(), AgentFailure> {
        if self.shared.take_cancel_to_send().is_some() {
            self.turn.interrupt_due = true;
        }
        if !self.turn.interrupt_due || !self.turn.started {
            return Ok(());
        }
        let Some(turn) = self.turn.announced.clone().or_else(|| self.turn.id.clone()) else {
            return Ok(());
        };
        self.turn.interrupt_due = false;
        let params = json!({"threadId": self.thread_id(), "turnId": turn});
        self.request(Pending::Interrupt, "turn/interrupt", params)
    }

    // ---- dispatch --------------------------------------------------------------------

    fn on_message(&mut self, message: Value) -> Result<(), AgentFailure> {
        let method = message.get("method").and_then(Value::as_str);
        match (method, message.get("id")) {
            (Some(method), Some(id)) => {
                let id = id.clone();
                self.on_server_request(method.to_owned(), id, &message)
            }
            (Some(method), None) => self.on_notification(method, &message),
            (None, Some(id)) => self.on_response(id, &message),
            (None, None) => Ok(()),
        }
    }

    fn rpc_error(&self, message: &Value) -> Option<(String, Option<i64>)> {
        let error = message.get("error")?;
        let text = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Codex refused the request");
        Some((
            self.shared.clean(text),
            error.get("code").and_then(Value::as_i64),
        ))
    }

    fn on_response(&mut self, id: &Value, message: &Value) -> Result<(), AgentFailure> {
        let Some(pending) = id
            .as_u64()
            .or_else(|| id.as_str().and_then(|s| s.parse().ok()))
            .and_then(|id| self.pending.remove(&id))
        else {
            return Ok(());
        };
        let error = self.rpc_error(message);
        let result = message.get("result").unwrap_or(&Value::Null);
        let refused = |kind, phase, (text, code): (String, Option<i64>)| {
            let mut failure = AgentFailure::new(kind, phase, text);
            failure.code = code;
            failure
        };
        match pending {
            Pending::Initialize => {
                if let Some(error) = error {
                    return Err(refused(
                        FailureKind::ProtocolViolation,
                        Phase::Initialize,
                        error,
                    ));
                }
                // `userAgent` is `<client>/<codex version> (<platform>)`.
                let agent = str_at(result, "/userAgent");
                let reported = agent
                    .split_once('/')
                    .and_then(|(_, rest)| native_launch::parse_version(rest));
                if let Some(version) = reported {
                    self.params.version = version;
                    launch::check_version(version).map_err(|message| {
                        AgentFailure::new(
                            FailureKind::UnsupportedVersion,
                            Phase::Initialize,
                            message,
                        )
                    })?;
                }
                self.write(rpc::notification("initialized"))?;
                self.request(
                    Pending::AccountRead,
                    "account/read",
                    json!({"refreshToken": false}),
                )
            }
            Pending::AccountRead => {
                // Only whether an account exists is read; its email never leaves here.
                let account = result.get("account").filter(|a| !a.is_null());
                let required = result.get("requiresOpenaiAuth").and_then(Value::as_bool);
                if error.is_none() && account.is_none() && required == Some(true) {
                    return Err(AgentFailure::new(
                        FailureKind::AuthRequired,
                        Phase::Initialize,
                        "Codex is not signed in; run `codex login`",
                    ));
                }
                self.on_initialized(account.is_some())
            }
            Pending::ConfigRead => {
                let names: Vec<String> = result
                    .pointer("/config/mcp_servers")
                    .and_then(Value::as_object)
                    .map(|servers| servers.keys().cloned().collect())
                    .unwrap_or_default();
                self.open_thread(&names)
            }
            Pending::Thread { resume } => match error {
                Some(_) if resume => {
                    // The thread is gone: a new one, reported as not restored.
                    self.params.resume = None;
                    self.request(
                        Pending::ConfigRead,
                        "config/read",
                        self.config_read_params(),
                    )
                }
                Some(error) => Err(refused(FailureKind::SessionRejected, Phase::Session, error)),
                None => self.on_thread(result, resume),
            },
            Pending::TurnStart { turn, slash } => {
                if let Some(error) = error {
                    self.shared.finish_turn(
                        turn,
                        StopReasonKind::EndTurn,
                        true,
                        TurnOrigin::Prompted,
                    );
                    // A refused slash command (nothing to review, say) leaves the thread
                    // as it was; a refused prompt means the session cannot take turns.
                    if slash {
                        return Ok(());
                    }
                    return Err(refused(FailureKind::PromptRejected, Phase::Prompt, error));
                }
                // The reply's id is the one `turn/completed` carries, even when
                // `turn/started` came first with another (a review).
                if let Some(id) = result.pointer("/turn/id").and_then(Value::as_str) {
                    self.turn.id = Some(id.to_owned());
                }
                self.send_cancel()
            }
            Pending::Steer(text) => {
                self.steer_result(
                    text,
                    match error {
                        None => Ok(()),
                        Some(_) => Err("Codex did not take the message into the turn"),
                    },
                );
                Ok(())
            }
            Pending::Interrupt => Ok(()),
        }
    }

    fn config_read_params(&self) -> Value {
        json!({"cwd": self.params.cwd, "includeLayers": false})
    }

    fn on_initialized(&mut self, authenticated: bool) -> Result<(), AgentFailure> {
        let info = InitializedInfo {
            agent_name: "Codex".to_owned(),
            agent_version: native_launch::version_label(self.params.version),
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
            authenticated,
        };
        self.shared.core.lock().initialized = Some(info.clone());
        self.shared.emit(AgentEventKind::Initialized(info));
        if self.params.mode == DriverMode::InitializeOnly {
            self.shared.watchdog.disarm();
            self.shared.set_ready();
            return Ok(());
        }
        self.shared.set_phase(Phase::Session);
        self.request(
            Pending::ConfigRead,
            "config/read",
            self.config_read_params(),
        )
    }

    fn open_thread(&mut self, configured: &[String]) -> Result<(), AgentFailure> {
        let mut params = json!({
            "cwd": self.params.cwd,
            "approvalPolicy": launch::APPROVAL_POLICY,
            "sandbox": "workspace-write",
        });
        let thread =
            launch::thread_config(configured, &self.params.servers, &self.params.user_servers);
        params["config"] = thread.config;
        self.renamed = thread.renamed;
        if let Some(model) = &self.params.model {
            params["model"] = Value::from(model.clone());
        }
        match self.params.resume.clone() {
            Some(thread) => {
                params["threadId"] = Value::from(thread);
                self.request(Pending::Thread { resume: true }, "thread/resume", params)
            }
            None => self.request(Pending::Thread { resume: false }, "thread/start", params),
        }
    }

    fn on_thread(&mut self, result: &Value, restored: bool) -> Result<(), AgentFailure> {
        let Some(thread) = result.pointer("/thread/id").and_then(Value::as_str) else {
            return Err(violation("Codex started a thread without an id"));
        };
        self.thread = Some(thread.to_owned());
        let exported = self.shared.clean(thread);
        {
            let mut core = self.shared.core.lock();
            core.session = Some(thread.to_owned());
            core.restored = restored;
        }
        self.shared.queue.set_session(&exported);
        self.shared.emit(AgentEventKind::SessionReady {
            session_id: exported,
            restored,
            user_mcp_servers: self
                .params
                .user_servers
                .iter()
                .map(|server| server.name().to_owned())
                .collect(),
        });
        self.shared.emit(AgentEventKind::AvailableCommands(vec![
            CommandInfo {
                name: "compact".to_owned(),
                description: "Summarize the conversation to free up context".to_owned(),
                input_hint: None,
            },
            CommandInfo {
                name: "review".to_owned(),
                description: "Review the working tree changes".to_owned(),
                input_hint: Some("optional review instructions".to_owned()),
            },
        ]));
        self.shared.watchdog.disarm();
        self.shared.set_ready();
        Ok(())
    }

    // ---- server requests -------------------------------------------------------------

    fn on_server_request(
        &mut self,
        method: String,
        id: Value,
        message: &Value,
    ) -> Result<(), AgentFailure> {
        let params = message.get("params").unwrap_or(&Value::Null);
        let mut item = str_at(params, "/itemId").to_owned();
        let mut kind = Some("other");
        let mut locations = Vec::new();
        let (title, answer, question) = match method.as_str() {
            "item/commandExecution/requestApproval" => {
                let command = str_at(params, "/command");
                let title = if command.is_empty() {
                    "Run a command".to_owned()
                } else {
                    format!("Run: {command}")
                };
                kind = Some("execute");
                (title, Answer::Decision, None)
            }
            "item/fileChange/requestApproval" => {
                let paths = self.file_changes.get(&item).cloned().unwrap_or_default();
                let title = if paths.is_empty() {
                    "Change files".to_owned()
                } else {
                    format!("Edit: {}", paths.join(", "))
                };
                kind = Some("edit");
                locations = paths.iter().map(std::path::PathBuf::from).collect();
                (title, Answer::Decision, None)
            }
            "item/permissions/requestApproval" => {
                let reason = str_at(params, "/reason");
                let title = if reason.is_empty() {
                    "Grant extra permissions".to_owned()
                } else {
                    format!("Grant extra permissions: {reason}")
                };
                let profile = params.get("permissions").cloned().unwrap_or(Value::Null);
                (title, Answer::Permissions(profile), None)
            }
            "item/tool/requestUserInput" => {
                let questions = rpc::parse_questions(params);
                if questions.is_empty() {
                    // Nothing to ask: answer with no answers so the agent is not left
                    // waiting for a reply nobody can give.
                    return self.write(rpc::response(&id, json!({"answers": {}})));
                }
                let items: Vec<QuestionItem> = questions.iter().map(|q| q.item.clone()).collect();
                (
                    "Codex has a question".to_owned(),
                    Answer::Questions(questions),
                    Some(items),
                )
            }
            "mcpServer/elicitation/request" if is_tool_call_approval(params) => {
                let session_name = str_at(params, "/serverName");
                let server = self
                    .renamed
                    .get(session_name)
                    .map_or(session_name, String::as_str)
                    .to_owned();
                if self.params.servers.iter().any(|own| own.name == server) {
                    // Studio's own tools are pre-approved, as Claude's launch allows
                    // `mcp__<studio server>`: once, never remembered by Codex.
                    return self.write(rpc::response(&id, rpc::tool_call_approval(true)));
                }
                // Named as Claude names MCP tools, so policies and saved rules treat
                // both alike; without a tool name the request has no kind to match.
                let tool = approval_tool(params, session_name).unwrap_or_default();
                let title = if tool.is_empty() {
                    kind = None;
                    let message = str_at(params, "/message");
                    if message.is_empty() {
                        format!("Use a tool of the {server} server")
                    } else {
                        message.to_owned()
                    }
                } else {
                    format!("mcp__{server}__{tool}")
                };
                item = id.to_string();
                (title, Answer::ToolCall, None)
            }
            _ => {
                // Other elicitations, dynamic tools, token refresh and attestation are
                // never enabled; an unanswered request would hang the server.
                self.shared.declined.fetch_add(1, Ordering::SeqCst);
                let line =
                    rpc::error_response(&id, rpc::METHOD_NOT_FOUND, "not supported by this client");
                return self.write(line);
            }
        };
        self.shared.flush_before_event();
        self.shared.note_turn_activity();
        let sink = Box::new(CodexSink {
            id: id.clone(),
            answer,
            writer: self.params.writer.clone(),
        });
        let options = match question {
            Some(_) => Vec::new(),
            None => vec![
                (ACCEPT.to_owned(), ACCEPT.to_owned()),
                (DECLINE.to_owned(), DECLINE.to_owned()),
            ],
        };
        let admitted = self
            .shared
            .admit_pending(id.to_string(), options, sink, question.is_some())
            .map_err(violation)?;
        let Some((id, turn)) = admitted else {
            return Ok(());
        };
        let event = match question {
            Some(questions) => AgentEventKind::Question(QuestionPrompt {
                id,
                turn,
                title,
                questions: questions
                    .into_iter()
                    .map(|q| QuestionItem {
                        header: self.shared.clean(&q.header),
                        question: self.shared.clean(&q.question),
                        multi_select: q.multi_select,
                        options: q
                            .options
                            .into_iter()
                            .map(|o| crate::driver::QuestionOption {
                                label: self.shared.clean(&o.label),
                                description: self.shared.clean(&o.description),
                            })
                            .collect(),
                    })
                    .collect(),
            }),
            None => AgentEventKind::PermissionRequested(PermissionPrompt {
                id,
                turn,
                tool_call_id: self.shared.clean(&item),
                title: self.shared.clean(&title),
                options: vec![
                    PermissionChoice {
                        option_id: ACCEPT.to_owned(),
                        name: "Allow".to_owned(),
                        kind: "allow_once".to_owned(),
                    },
                    PermissionChoice {
                        option_id: DECLINE.to_owned(),
                        name: "Decline".to_owned(),
                        kind: "reject_once".to_owned(),
                    },
                ],
                kind: kind.map(str::to_owned),
                locations,
            }),
        };
        self.shared.emit(event);
        Ok(())
    }

    // ---- notifications ---------------------------------------------------------------

    fn turn_open(&self) -> bool {
        self.shared.core.lock().turn.is_some()
    }

    /// Whether a notification belongs to this session's thread. Other threads are
    /// subagents: their traffic only shows that the turn is alive.
    fn own_thread(&self, params: &Value) -> bool {
        let thread = params.get("threadId").and_then(Value::as_str);
        let own = thread.is_none() || thread == self.thread.as_deref();
        if !own && self.turn_open() {
            self.shared.note_turn_activity();
        }
        own
    }

    fn on_notification(&mut self, method: &str, message: &Value) -> Result<(), AgentFailure> {
        let params = message.get("params").unwrap_or(&Value::Null);
        if !self.own_thread(params) {
            return Ok(());
        }
        match method {
            "turn/started" => self.on_turn_started(params),
            "turn/completed" => self.on_turn_completed(params),
            "item/agentMessage/delta" => {
                self.streamed.insert(str_at(params, "/itemId").to_owned());
                self.on_text(MessageRole::Agent, str_at(params, "/delta"));
                Ok(())
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                self.on_text(MessageRole::Thought, str_at(params, "/delta"));
                Ok(())
            }
            "item/started" => {
                self.on_item_started(params.get("item").unwrap_or(&Value::Null));
                Ok(())
            }
            "item/completed" => {
                self.on_item_completed(params.get("item").unwrap_or(&Value::Null));
                Ok(())
            }
            "turn/plan/updated" => {
                if self.turn_open() {
                    self.shared.note_turn_activity();
                    self.shared.flush_before_event();
                    let entries = plan_entries(&self.shared, params);
                    self.shared.emit(AgentEventKind::Plan(entries));
                }
                Ok(())
            }
            "serverRequest/resolved" => {
                if let Some(id) = params.get("requestId") {
                    self.shared.withdraw_pending(&id.to_string());
                }
                Ok(())
            }
            // Status, settings, usage, account and MCP startup updates carry no workflow
            // authority.
            _ => Ok(()),
        }
    }

    fn on_text(&mut self, role: MessageRole, text: &str) {
        if text.is_empty() {
            return;
        }
        if !self.turn_open() {
            self.shared.late.fetch_add(1, Ordering::SeqCst);
            return;
        }
        self.shared.note_turn_activity();
        self.shared.push_text(role, text);
    }

    fn on_turn_started(&mut self, params: &Value) -> Result<(), AgentFailure> {
        let id = str_at(params, "/turn/id");
        // One turn of this thread runs at a time, so the first announcement after the
        // request is its turn, whatever id it carries.
        if !self.turn_open() || self.turn.started {
            self.shared.late.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        }
        if self.turn.id.is_none() {
            self.turn.id = Some(id.to_owned());
        }
        self.turn.announced = Some(id.to_owned());
        self.turn.started = true;
        self.shared.note_turn_activity();
        if let Some((text, _)) = self.turn.parked_steer.take() {
            self.steer(text)?;
        }
        self.send_cancel()
    }

    fn on_item_started(&mut self, item: &Value) {
        let id = str_at(item, "/id").to_owned();
        let kind = str_at(item, "/type");
        let (title, tool_kind) = match kind {
            "commandExecution" => {
                self.running.insert(id.clone());
                (format!("Run: {}", str_at(item, "/command")), "execute")
            }
            "fileChange" => {
                let paths: Vec<String> = item
                    .get("changes")
                    .and_then(Value::as_array)
                    .map(|changes| {
                        changes
                            .iter()
                            .map(|c| str_at(c, "/path").to_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                let title = format!("Edit: {}", paths.join(", "));
                self.file_changes.insert(id.clone(), paths);
                (title, "edit")
            }
            "mcpToolCall" => {
                let server = str_at(item, "/server");
                let server = self.renamed.get(server).map_or(server, String::as_str);
                (format!("{server}: {}", str_at(item, "/tool")), "other")
            }
            "webSearch" => (format!("Web search: {}", str_at(item, "/query")), "fetch"),
            _ => return,
        };
        if !self.turn_open() {
            self.running.remove(&id);
            self.shared.late.fetch_add(1, Ordering::SeqCst);
            return;
        }
        self.open_items.insert(id.clone());
        self.shared.note_turn_activity();
        self.shared.flush_before_event();
        self.shared.emit(AgentEventKind::ToolCall(ToolEvent {
            tool_call_id: self.shared.clean(&id),
            title: Some(self.shared.clean(&title)),
            kind: Some(tool_kind.to_owned()),
            status: Some(ToolStatus::Pending),
        }));
    }

    fn on_item_completed(&mut self, item: &Value) {
        let id = str_at(item, "/id");
        if str_at(item, "/type") == "agentMessage" && !self.streamed.remove(id) {
            // A message that never streamed arrives whole.
            let text = str_at(item, "/text").to_owned();
            self.on_text(MessageRole::Agent, &text);
            return;
        }
        if self.background.remove(id) {
            // A command that outlived its turn finished.
            self.emit_background();
            return;
        }
        self.running.remove(id);
        if !self.open_items.remove(id) || !self.turn_open() {
            return;
        }
        self.shared.note_turn_activity();
        self.shared.flush_before_event();
        let failed = matches!(str_at(item, "/status"), "failed" | "declined");
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

    fn emit_background(&self) {
        let tasks = u32::try_from(self.background.len()).unwrap_or(u32::MAX);
        self.shared.emit(AgentEventKind::Background {
            tasks,
            continuing: false,
        });
    }

    fn on_turn_completed(&mut self, params: &Value) -> Result<(), AgentFailure> {
        let codex_turn = params.get("turn").unwrap_or(&Value::Null);
        let id = str_at(codex_turn, "/id");
        let turn = {
            let core = self.shared.core.lock();
            core.turn
                .as_ref()
                .map(|turn| (turn.id, turn.cancel_requested))
        };
        let ours = [&self.turn.id, &self.turn.announced]
            .iter()
            .any(|known| known.as_deref() == Some(id));
        let Some((turn, cancelled)) = turn.filter(|_| ours) else {
            self.shared.late.fetch_add(1, Ordering::SeqCst);
            return Ok(());
        };
        let status = str_at(codex_turn, "/status");
        let error_info = codex_turn.pointer("/error/codexErrorInfo");
        if status == "failed" && error_info.and_then(Value::as_str) == Some("unauthorized") {
            return Err(AgentFailure::new(
                FailureKind::AuthRequired,
                Phase::Prompt,
                "Codex is not signed in; run `codex login`",
            ));
        }
        let (stop_reason, incomplete) = match status {
            _ if cancelled => (StopReasonKind::Cancelled, false),
            "interrupted" => (StopReasonKind::Cancelled, false),
            "completed" => (StopReasonKind::EndTurn, false),
            _ => (StopReasonKind::EndTurn, true),
        };
        // Commands still running keep changing the draft: they are background work, and
        // the engine must learn that before the turn's end.
        let running = std::mem::take(&mut self.running);
        if !running.is_empty() {
            self.background.extend(running);
            self.emit_background();
        }
        self.open_items.clear();
        self.streamed.clear();
        self.file_changes.clear();
        if let Some((text, _)) = self.turn.parked_steer.take() {
            self.steer_result(text, Err("The turn ended before the message could be sent"));
        }
        self.turn = CodexTurn::default();
        self.shared
            .finish_turn(turn, stop_reason, incomplete, TurnOrigin::Prompted);
        Ok(())
    }
}

fn plan_entries(shared: &Shared, params: &Value) -> Vec<PlanEntry> {
    params
        .get("plan")
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .map(|step| PlanEntry {
                    text: shared.clean(str_at(step, "/step")),
                    status: match str_at(step, "/status") {
                        "completed" => PlanStatus::Completed,
                        "inProgress" => PlanStatus::InProgress,
                        _ => PlanStatus::Pending,
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}
