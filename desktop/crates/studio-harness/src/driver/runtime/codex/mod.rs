//! Codex over `codex app-server` (JSON-RPC on stdio).
//!
//! One `codex app-server` process and thread per session. A synchronous loop thread owns
//! stdout dispatch and every write; host calls only latch state under `core` and queue
//! their request for it.

mod connection;
mod launch;
mod rpc;

use super::*;
use crate::driver::{AgentDriver, AgentWire, DriverCapabilities, QuestionAnswer, SteeringMode};
use std::sync::mpsc as std_mpsc;

/// Lines and host requests waiting for the loop thread.
const INPUT_QUEUE: usize = 128;
/// Codex reads prompt images from files; each is capped like the Messages API's.
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

pub(super) enum Input {
    Line(String),
    /// Stdout closed (or failed; the reader already latched that failure).
    Eof,
    /// A line from a host call (a permission answer), written in order with the loop's own.
    Write(String),
    /// Look at latched state (a pending cancel).
    Wake,
    Shutdown,
    /// Start the turn `turn` (already opened in `core`).
    Prompt {
        turn: u64,
        text: String,
        images: Vec<PathBuf>,
    },
    /// Steer the running turn; Codex's answer arrives as a `SteerResult` event.
    Steer {
        text: String,
    },
}

/// Owner of one Codex app-server process and thread. Dropping stops it.
pub struct CodexDriver {
    shared: Arc<Shared>,
    input: Mutex<Option<std_mpsc::SyncSender<Input>>>,
    main: Option<JoinHandle<()>>,
    watchdog: Option<JoinHandle<()>>,
    io: Vec<JoinHandle<()>>,
    /// Prompt image files; removed when the driver goes away.
    private: tempfile::TempDir,
}

fn spawn_refused(message: impl Into<String>) -> AgentFailure {
    AgentFailure::new(FailureKind::SpawnFailed, Phase::Spawn, message)
}

impl CodexDriver {
    /// Checks the Codex version, spawns `codex app-server` and starts the handshake in
    /// the background.
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
            mode,
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
        let version = launch::version_gate(&launch.executable, &user.prefix, &launch.env)
            .map_err(|message| spawn_refused(redactor.redact(&message)))?;
        // Codex has no per-server pre-approval to withhold: the user's servers run beside
        // the studio's and their calls follow Codex's own approval rules.
        let (servers, user_servers) = match mcp_stdio {
            McpStdioSupport::Baseline => (mcp_servers, user_mcp_servers),
            McpStdioSupport::Unsupported => (Vec::new(), Vec::new()),
        };
        let private = tempfile::Builder::new()
            .prefix("fframes-codex-")
            .tempdir()
            .map_err(|error| spawn_refused(format!("cannot create the image folder: {error}")))?;
        let args = launch::wire_args(&user);
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
        let (input_tx, input_rx) = std_mpsc::sync_channel(INPUT_QUEUE);
        io.push(connection::spawn_forwarder(line_rx, input_tx.clone()));
        let watchdog = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("codex-watchdog".into())
                .spawn(move || run_watchdog(shared))
                .expect("spawn codex watchdog")
        };
        let main = {
            let shared = shared.clone();
            let writer = input_tx.clone();
            let params = connection::Params {
                cwd,
                version,
                mode,
                resume: resume_session,
                model: user.flag("--model").map(str::to_owned),
                effort: user.flag("--effort").map(str::to_owned),
                servers,
                user_servers,
                mcp_stdio: mcp_stdio == McpStdioSupport::Baseline,
                writer,
            };
            std::thread::Builder::new()
                .name("codex-connection".into())
                .spawn(move || connection::run(shared, sink, input_rx, params))
                .expect("spawn codex connection")
        };
        Ok(Self {
            shared,
            input: Mutex::new(Some(input_tx)),
            main: Some(main),
            watchdog: Some(watchdog),
            io,
            private,
        })
    }

    fn send(&self, input: Input) -> Result<(), DriverError> {
        let guard = self.input.lock();
        let sender = guard.as_ref().ok_or(DriverError::Closed)?;
        sender.try_send(input).map_err(|error| match error {
            std_mpsc::TrySendError::Full(_) => DriverError::Busy("the Codex input queue is full"),
            std_mpsc::TrySendError::Disconnected(_) => DriverError::Closed,
        })
    }

    fn validate_images(text: &str, images: &[PromptImage]) -> Result<(), DriverError> {
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

    /// Writes the turn's images where Codex reads them (`localImage`).
    fn write_images(&self, turn: u64, images: &[PromptImage]) -> Result<Vec<PathBuf>, DriverError> {
        images
            .iter()
            .enumerate()
            .map(|(index, image)| {
                let path = self.private.path().join(format!("turn-{turn}-{index}.png"));
                std::fs::write(&path, &image.data)
                    .map(|()| path)
                    .map_err(|_| DriverError::InvalidPrompt("a prompt image could not be saved"))
            })
            .collect()
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

impl Drop for CodexDriver {
    fn drop(&mut self) {
        if self.main.is_some() {
            let _ = self.stop();
        }
    }
}

impl AgentDriver for CodexDriver {
    fn wire(&self) -> AgentWire {
        AgentWire::CodexAppServer
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            deterministic_turn_end: true,
            stops_turn_in_place: true,
            steering: SteeringMode::StepBoundary,
            images: true,
            // The app server takes text and local images only.
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

    fn prompt_image_budget(&self, _text: &str, count: usize) -> usize {
        if count == 0 || count > MAX_PROMPT_IMAGE_COUNT {
            return 0;
        }
        MAX_IMAGE_BYTES.saturating_mul(count)
    }

    fn prompt(&self, text: &str, images: Vec<PromptImage>) -> Result<u64, DriverError> {
        Self::validate_images(text, &images)?;
        let turn = self.shared.begin_turn()?;
        let sent = self.write_images(turn, &images).and_then(|images| {
            self.send(Input::Prompt {
                turn,
                text: text.to_owned(),
                images,
            })
        });
        if let Err(error) = sent {
            self.shared.abandon_turn(turn);
            return Err(error);
        }
        Ok(turn)
    }

    fn steer(&self, text: &str) -> Result<(), DriverError> {
        Self::validate_images(text, &[])?;
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
        self.send(Input::Steer {
            text: text.to_owned(),
        })
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
            "Codex sessions run with the studio's approval policy",
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
