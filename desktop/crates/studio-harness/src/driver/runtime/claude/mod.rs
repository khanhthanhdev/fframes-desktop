//! Claude Code over its `--print` stream-json control protocol.
//!
//! One `claude` process per session, started with a fixed session id so the session exists
//! before the first prompt. A synchronous loop thread owns stdout dispatch and every write;
//! host calls only latch state under `core` and queue a line or a wake-up for it.

mod connection;
mod launch;
mod wire;

use super::*;
use crate::driver::{AgentDriver, AgentWire, DriverCapabilities, QuestionAnswer, SteeringMode};
use std::sync::mpsc as std_mpsc;

/// Lines and host requests waiting for the loop thread.
const INPUT_QUEUE: usize = 128;
/// After a background task finished with no turn open, the time Claude has to start its
/// own turn (`system/init`) before the driver stops waiting for it. Claude starts that turn
/// about a second after the notification (S5: 49.3 s notification, 50.1 s init).
pub(super) const CONTINUE_WAIT: Duration = Duration::from_secs(15);
/// How long a turn end is held while a steer sent into the turn was not yet taken up.
pub(super) const STEER_CONFIRM_WAIT: Duration = Duration::from_secs(30);
/// The Messages API's limit for one base64 image.
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

pub(super) enum Input {
    Line(String),
    /// Stdout closed (or failed; the reader already latched that failure).
    Eof,
    /// A line from a host call, written in order with the loop's own.
    Write(String),
    /// Look at latched state (a pending cancel).
    Wake,
    Shutdown,
}

/// What host calls and the loop both read: Claude's background work and the steers that
/// are still on their way into the running turn.
#[derive(Default)]
pub(super) struct WireState {
    /// Background tasks Claude reported as running (`background_tasks_changed`).
    pub(super) background: u32,
    /// A background task finished with no turn open: Claude is about to continue, or is
    /// continuing, in a turn of its own.
    pub(super) continuing: bool,
    /// Top-level tool calls started and not yet answered.
    pub(super) open_tools: HashSet<String>,
    /// Uuids of steers written but not yet replayed back by Claude.
    pub(super) pending_steers: HashSet<String>,
}

/// Owner of one Claude Code process and session. Dropping stops it.
pub struct ClaudeDriver {
    shared: Arc<Shared>,
    input: Mutex<Option<std_mpsc::SyncSender<Input>>>,
    state: Arc<Mutex<WireState>>,
    main: Option<JoinHandle<()>>,
    watchdog: Option<JoinHandle<()>>,
    io: Vec<JoinHandle<()>>,
    /// The MCP and settings files; removed when the driver goes away.
    _private: tempfile::TempDir,
}

fn spawn_refused(message: impl Into<String>) -> AgentFailure {
    AgentFailure::new(FailureKind::SpawnFailed, Phase::Spawn, message)
}

impl ClaudeDriver {
    /// Checks the Claude Code version, writes the private launch files, spawns the process
    /// and starts the `initialize` handshake in the background.
    pub fn start(
        config: DriverConfig,
        processes: &ProcessTreeManager,
    ) -> Result<Self, AgentFailure> {
        let DriverConfig {
            provider,
            task,
            cwd,
            launch,
            limits,
            resume_session,
            ownership,
            mode: _,
            mcp_servers,
            user_mcp_servers,
            mcp_stdio,
        } = config;
        super::check_mcp_servers(&mcp_servers, &user_mcp_servers).map_err(spawn_refused)?;
        let redactor = Arc::new(Redactor::new(super::mcp_secrets(
            &launch.secrets,
            &mcp_servers,
            &user_mcp_servers,
        )));
        let user = launch::split_user_args(&launch.args).map_err(spawn_refused)?;
        launch::version_gate(&launch.executable, &user.prefix, &launch.env)
            .map_err(|message| spawn_refused(redactor.redact(&message)))?;
        let (servers, user_servers) = match mcp_stdio {
            McpStdioSupport::Baseline => (mcp_servers, user_mcp_servers),
            McpStdioSupport::Unsupported => (Vec::new(), Vec::new()),
        };
        let private = tempfile::Builder::new()
            .prefix("fframes-claude-")
            .tempdir()
            .map_err(|error| spawn_refused(format!("cannot create the launch files: {error}")))?;
        let mcp_path = private.path().join("mcp.json");
        let settings_path = private.path().join("settings.json");
        for (path, value) in [
            (&mcp_path, launch::mcp_config_json(&servers, &user_servers)),
            (
                &settings_path,
                launch::settings_json(&servers, &user_servers),
            ),
        ] {
            std::fs::write(path, value.to_string()).map_err(|error| {
                spawn_refused(format!("cannot write the launch files: {error}"))
            })?;
        }
        let user_server_names = user_servers
            .iter()
            .map(|server| server.name().to_owned())
            .collect();
        let resumed = resume_session.is_some();
        let session = match resume_session {
            Some(id) => launch::SessionStart::Resume(id),
            None => launch::SessionStart::New(uuid::Uuid::new_v4().to_string()),
        };
        let args = launch::wire_args(&user, &session, &mcp_path, &settings_path);
        let spawned = spawn_owned(&launch, &args, &cwd, processes, &redactor)?;
        let (shared, line_rx, sink, mut io) = Shared::start(
            spawned,
            Owner {
                provider,
                task,
                ownership,
                limits,
                redactor,
            },
        );
        shared.core.lock().session = Some(session.id().to_owned());
        let (input_tx, input_rx) = std_mpsc::sync_channel(INPUT_QUEUE);
        io.push(connection::spawn_forwarder(line_rx, input_tx.clone()));
        let state = Arc::new(Mutex::new(WireState::default()));
        let watchdog = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("claude-watchdog".into())
                .spawn(move || run_watchdog(shared))
                .expect("spawn claude watchdog")
        };
        let main = {
            let shared = shared.clone();
            let state = state.clone();
            let writer = input_tx.clone();
            std::thread::Builder::new()
                .name("claude-connection".into())
                .spawn(move || {
                    connection::run(
                        shared,
                        state,
                        sink,
                        input_rx,
                        connection::Params {
                            mcp_stdio: mcp_stdio == McpStdioSupport::Baseline,
                            resumed,
                            user_servers: user_server_names,
                            writer,
                        },
                    )
                })
                .expect("spawn claude connection")
        };
        Ok(Self {
            shared,
            input: Mutex::new(Some(input_tx)),
            state,
            main: Some(main),
            watchdog: Some(watchdog),
            io,
            _private: private,
        })
    }

    fn send(&self, input: Input) -> Result<(), DriverError> {
        let guard = self.input.lock();
        let sender = guard.as_ref().ok_or(DriverError::Closed)?;
        sender.try_send(input).map_err(|error| match error {
            std_mpsc::TrySendError::Full(_) => {
                DriverError::Busy("the Claude Code input queue is full")
            }
            std_mpsc::TrySendError::Disconnected(_) => DriverError::Closed,
        })
    }

    fn validate_images(&self, text: &str, images: &[PromptImage]) -> Result<(), DriverError> {
        validate_prompt(text, images)?;
        if images
            .iter()
            .any(|image| image.data.len() > MAX_IMAGE_BYTES)
        {
            return Err(DriverError::InvalidPrompt(
                "a prompt image is larger than 5 MiB",
            ));
        }
        Ok(())
    }

    /// The exact prompt line, refused when it does not fit one wire message.
    fn user_line(
        &self,
        uuid: &str,
        text: &str,
        images: &[PromptImage],
        priority: Option<&str>,
    ) -> Result<String, DriverError> {
        let max = self.shared.limits.max_message_bytes;
        let encoded = images.iter().try_fold(0usize, |total, image| {
            total.checked_add(base64_len(image.data.len())?)
        });
        if !encoded.is_some_and(|bytes| fits_wire(bytes, max)) {
            return Err(DriverError::InvalidPrompt(
                "prompt images exceed the message size limit",
            ));
        }
        let data: Vec<String> = images
            .iter()
            .map(|image| STANDARD.encode(&image.data))
            .collect();
        let blocks: Vec<wire::Image<'_>> = data.iter().map(|data| wire::Image { data }).collect();
        let line = wire::user_line(uuid, text, &blocks, priority);
        if !fits_wire(line.len(), max) {
            return Err(DriverError::InvalidPrompt(
                "prompt exceeds the message size limit",
            ));
        }
        Ok(line)
    }

    fn refuse_while_background(&self) -> Result<(), DriverError> {
        let state = self.state.lock();
        if state.background > 0 || state.continuing {
            return Err(DriverError::Busy(
                "Claude Code is still working on a background task",
            ));
        }
        Ok(())
    }

    fn stop(&mut self) -> DriverOutcome {
        let threads = Threads {
            main: self.main.take(),
            watchdog: self.watchdog.take(),
            io: std::mem::take(&mut self.io),
        };
        self.shared.stop_owned(
            || {
                let _ = self.send(Input::Shutdown);
                self.input.lock().take();
            },
            threads,
        )
    }
}

impl Drop for ClaudeDriver {
    fn drop(&mut self) {
        if self.main.is_some() {
            let _ = self.stop();
        }
    }
}

impl AgentDriver for ClaudeDriver {
    fn wire(&self) -> AgentWire {
        AgentWire::ClaudeStreamJson
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            deterministic_turn_end: true,
            stops_turn_in_place: true,
            steering: SteeringMode::StepBoundary,
            images: true,
            // Claude Code's stream-json input carries only text and image blocks.
            files: false,
        }
    }

    fn ownership(&self) -> WriterOwnership {
        self.shared.ownership.clone()
    }

    fn wire_session_id(&self) -> Option<String> {
        let raw = self.shared.core.lock().session.clone()?;
        (self.shared.redactor.redact(&raw) == raw).then_some(raw)
    }

    fn options(&self) -> AgentOptions {
        self.shared.options.lock().clone()
    }

    fn failure(&self) -> Option<AgentFailure> {
        self.shared.core.lock().failure.clone()
    }

    fn wait_ready(&self, timeout: Duration) -> Result<SessionInfo, AgentFailure> {
        self.shared.wait_ready(timeout)
    }

    fn wait_turn(&self, turn: u64, timeout: Duration) -> Result<PromptOutcome, AgentFailure> {
        self.shared.wait_turn(turn, timeout)
    }

    fn drain_events(&self, max: usize) -> Vec<AgentEvent> {
        self.shared.queue.drain(max)
    }

    fn prompt_image_budget(&self, text: &str, count: usize) -> usize {
        if count == 0 || count > MAX_PROMPT_IMAGE_COUNT {
            return 0;
        }
        let empty: Vec<wire::Image<'_>> = (0..count).map(|_| wire::Image { data: "" }).collect();
        let uuid = uuid::Uuid::nil().to_string();
        let framing = wire::user_line(&uuid, text, &empty, None).len();
        image_budget(framing, count, self.shared.limits.max_message_bytes)
            .min(MAX_IMAGE_BYTES.saturating_mul(count))
    }

    fn prompt(&self, text: &str, images: Vec<PromptImage>) -> Result<u64, DriverError> {
        self.validate_images(text, &images)?;
        self.refuse_while_background()?;
        let line = self.user_line(&uuid::Uuid::new_v4().to_string(), text, &images, None)?;
        let turn = self.shared.begin_turn()?;
        if let Err(error) = self.send(Input::Write(line)) {
            self.shared.abandon_turn(turn);
            return Err(error);
        }
        Ok(turn)
    }

    fn steer(&self, text: &str) -> Result<(), DriverError> {
        self.validate_images(text, &[])?;
        {
            let core = self.shared.core.lock();
            if let Some(failure) = &core.failure {
                return Err(DriverError::Failed(failure.clone()));
            }
            match &core.turn {
                Some(turn) if !turn.cancel_requested && !core.stopping => {}
                _ => return Err(DriverError::NoActiveTurn),
            }
        }
        let uuid = uuid::Uuid::new_v4().to_string();
        // "now" reaches the model at its next step; with a tool call in flight it waits
        // for the tool results instead of interrupting them.
        let mut state = self.state.lock();
        let priority = if state.open_tools.is_empty() {
            "now"
        } else {
            "next"
        };
        let line = self.user_line(&uuid, text, &[], Some(priority))?;
        state.pending_steers.insert(uuid.clone());
        drop(state);
        self.send(Input::Write(line)).inspect_err(|_| {
            self.state.lock().pending_steers.remove(&uuid);
        })?;
        // Claude takes the line as written; its replay only confirms the step.
        self.shared.emit(AgentEventKind::SteerResult {
            text: text.to_owned(),
            accepted: true,
            message: String::new(),
        });
        Ok(())
    }

    fn cancel_turn(&self) -> Result<(), DriverError> {
        if self.shared.latch_cancel(false)?.is_some() {
            let _ = self.send(Input::Wake);
        }
        Ok(())
    }

    fn reply_permission(
        &self,
        id: PermissionId,
        reply: PermissionReply,
    ) -> Result<(), DriverError> {
        self.shared.reply_permission(id, reply)
    }

    fn answer_question(
        &self,
        id: PermissionId,
        answers: Vec<QuestionAnswer>,
    ) -> Result<(), DriverError> {
        self.shared.answer_question(id, answers)
    }

    fn set_mode(&self, _mode_id: &str) -> Result<(), DriverError> {
        Err(DriverError::Unsupported(
            "Claude Code sessions run in the default permission mode",
        ))
    }

    fn set_config_option(&self, id: &str, _value: OptionValue) -> Result<(), DriverError> {
        Err(DriverError::InvalidOption(format!(
            "config option '{id}' was not advertised by the adapter"
        )))
    }

    fn shutdown(mut self: Box<Self>) -> DriverOutcome {
        self.stop()
    }
}
