//! The host handle that owns one adapter process and session.

use super::*;

/// Owner of one ACP v1 adapter process and session. Dropping stops it.
pub struct AcpDriver {
    pub(super) shared: Arc<Shared>,
    pub(super) commands: Mutex<Option<mpsc::Sender<Command>>>,
    pub(super) main: Option<JoinHandle<()>>,
    pub(super) watchdog: Option<JoinHandle<()>>,
    pub(super) io: Vec<JoinHandle<()>>,
}

impl AcpDriver {
    /// Spawns the adapter and starts the session handshake in the background.
    /// Observe progress with [`Self::wait_ready`] and the event queue.
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
        super::check_mcp_servers(&mcp_servers, &user_mcp_servers).map_err(|message| {
            AgentFailure::new(FailureKind::SpawnFailed, Phase::Spawn, message)
        })?;
        let redactor = Arc::new(Redactor::new(super::mcp_secrets(
            &launch.secrets,
            &mcp_servers,
            &user_mcp_servers,
        )));
        let spawned = spawn_owned(&launch, &launch.args, &cwd, processes, &redactor)?;
        let (shared, line_rx, sink, io) = Shared::start(
            spawned,
            Owner {
                provider,
                task,
                ownership,
                limits,
                redactor,
            },
        );
        let (command_tx, command_rx) = mpsc::channel(COMMAND_QUEUE);
        let user_names = |http: bool| -> Vec<String> {
            match mcp_stdio {
                McpStdioSupport::Baseline => user_mcp_servers
                    .iter()
                    .filter(|server| matches!(server.transport, McpTransport::Http(_)) == http)
                    .map(|server| server.name().to_owned())
                    .collect(),
                McpStdioSupport::Unsupported => Vec::new(),
            }
        };
        let params =
            DriveParams {
                cwd,
                mode,
                resume: resume_session,
                auth_method: launch.auth_method.clone(),
                // ACP cannot filter a server's tools: `disabled_tools` has no wire form here.
                mcp_servers: match mcp_stdio {
                    McpStdioSupport::Baseline => mcp_servers
                        .iter()
                        .map(wire_server)
                        .chain(user_mcp_servers.iter().filter_map(
                            |server| match &server.transport {
                                McpTransport::Stdio(stdio) => Some(wire_server(stdio)),
                                McpTransport::Http(_) => None,
                            },
                        ))
                        .collect(),
                    McpStdioSupport::Unsupported => Vec::new(),
                },
                mcp_http_servers: match mcp_stdio {
                    McpStdioSupport::Baseline => user_mcp_servers
                        .iter()
                        .filter_map(|server| match &server.transport {
                            McpTransport::Http(http) => Some(wire_http_server(http)),
                            McpTransport::Stdio(_) => None,
                        })
                        .collect(),
                    McpStdioSupport::Unsupported => Vec::new(),
                },
                user_stdio_names: user_names(false),
                user_http_names: user_names(true),
                mcp_stdio: mcp_stdio == McpStdioSupport::Baseline,
            };
        let watchdog = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("acp-watchdog".into())
                .spawn(move || run_watchdog(shared))
                .expect("spawn acp watchdog")
        };
        let main = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("acp-connection".into())
                .spawn(move || run_connection(shared, sink, Box::pin(line_rx), command_rx, params))
                .expect("spawn acp connection")
        };
        Ok(Self {
            shared,
            commands: Mutex::new(Some(command_tx)),
            main: Some(main),
            watchdog: Some(watchdog),
            io,
        })
    }

    pub fn provider(&self) -> &str {
        self.shared.queue.provider()
    }

    pub fn task(&self) -> &str {
        self.shared.queue.task()
    }

    /// Sanitized provider session id (secrets and token patterns redacted).
    pub fn session_id(&self) -> Option<String> {
        let raw = self.shared.core.lock().session.clone()?;
        Some(self.shared.clean(&raw))
    }

    /// The exact wire session id for restoration, but only when it carries no secret;
    /// an id that required redaction is never exposed.
    pub fn wire_session_id(&self) -> Option<String> {
        let raw = self.shared.core.lock().session.clone()?;
        (self.shared.redactor.redact(&raw) == raw).then_some(raw)
    }

    /// The writer ownership established at launch: the containment the supervisor
    /// verified for the spawned adapter, or the injected one. Later observations only
    /// demote it, in [`DriverOutcome::ownership`].
    pub fn ownership(&self) -> WriterOwnership {
        self.shared.ownership.clone()
    }

    pub fn status(&self) -> DriverStatus {
        self.shared.core.lock().status
    }

    pub fn initialized(&self) -> Option<InitializedInfo> {
        self.shared.core.lock().initialized.clone()
    }

    pub fn failure(&self) -> Option<AgentFailure> {
        self.shared.core.lock().failure.clone()
    }

    pub fn options(&self) -> AgentOptions {
        self.shared.options.lock().clone()
    }

    pub fn late_events_rejected(&self) -> u64 {
        self.shared.late.load(Ordering::SeqCst)
    }

    /// Requests for client services that were not advertised and were declined.
    pub fn declined_requests(&self) -> u64 {
        self.shared.declined.load(Ordering::SeqCst)
    }

    /// Adapter frames admitted but not yet dispatched by the SDK connection.
    pub fn ingress_in_flight(&self) -> usize {
        self.shared.ingress.in_flight()
    }

    /// Mode/config requests currently awaiting an adapter reply.
    pub fn outstanding_requests(&self) -> usize {
        self.shared.watchdog.outstanding()
    }

    pub fn last_prompt(&self) -> Option<PromptOutcome> {
        self.shared.core.lock().last_outcome.clone()
    }

    /// Redacted stderr tail. Only text that already passed redaction is retained, so
    /// polling between fragments of a secret never exposes a prefix of it.
    pub fn stderr_tail(&self) -> String {
        self.shared.stderr_text(self.shared.limits.max_stderr_bytes)
    }

    pub fn transcript_page(&self, from: u64, limit: usize) -> TranscriptPage {
        self.shared.transcript.lock().page(from, limit)
    }

    pub fn transcript_resident_bytes(&self) -> usize {
        self.shared.transcript.lock().resident_bytes()
    }

    pub fn next_event(&self, timeout: Duration) -> Option<AgentEvent> {
        self.shared.queue.next(timeout)
    }

    pub fn try_next_event(&self) -> Option<AgentEvent> {
        self.shared.queue.try_next()
    }

    pub fn drain_events(&self, max: usize) -> Vec<AgentEvent> {
        self.shared.queue.drain(max)
    }

    pub fn queued_events(&self) -> usize {
        self.shared.queue.len()
    }

    /// Blocks until the session (or, in `InitializeOnly` mode, initialization) is ready,
    /// the driver fails, or `timeout` passes.
    pub fn wait_ready(&self, timeout: Duration) -> Result<SessionInfo, AgentFailure> {
        self.shared.wait_ready(timeout)
    }

    /// Blocks until `turn` finishes, the driver fails, or `timeout` passes. A latched
    /// failure always wins over a recorded completion.
    pub fn wait_turn(&self, turn: u64, timeout: Duration) -> Result<PromptOutcome, AgentFailure> {
        self.shared.wait_turn(turn, timeout)
    }

    pub(super) fn send(&self, command: Command) -> Result<(), DriverError> {
        let mut guard = self.commands.lock();
        let sender = guard.as_mut().ok_or(DriverError::Closed)?;
        sender.try_send(command).map_err(|error| {
            if error.is_full() {
                DriverError::Busy("the adapter command queue is full")
            } else {
                DriverError::Closed
            }
        })
    }

    pub(super) fn require_usable(&self) -> Result<(), DriverError> {
        let core = self.shared.core.lock();
        if let Some(failure) = &core.failure {
            return Err(DriverError::Failed(failure.clone()));
        }
        match core.status {
            DriverStatus::Starting => Err(DriverError::NotReady),
            DriverStatus::Stopping | DriverStatus::Closed => Err(DriverError::Closed),
            DriverStatus::Ready | DriverStatus::Prompting => Ok(()),
        }
    }

    /// Starts one text prompt turn; returns its turn id.
    pub fn prompt(&self, text: &str) -> Result<u64, DriverError> {
        self.prompt_with_images(text, Vec::new())
    }

    /// Largest total of raw PNG bytes that `count` images can carry next to `text` in one
    /// prompt, however the bytes are split between the images. The bound is the wire
    /// message limit (`DriverLimits::max_message_bytes`) after base64 and JSON framing.
    /// 0 when no image fits, `count` is 0 or above the image limit, or there is no session.
    pub fn prompt_image_budget(&self, text: &str, count: usize) -> usize {
        if count == 0 || count > MAX_PROMPT_IMAGE_COUNT {
            return 0;
        }
        let Some(session) = self.shared.core.lock().session.clone() else {
            return 0;
        };
        let mut content = vec![ContentBlock::Text(TextContent::new(text))];
        content.extend((0..count).map(|_| ContentBlock::Image(ImageContent::new("", "image/png"))));
        let framing = prompt_line_bytes(&PromptRequest::new(session, content));
        image_budget(framing, count, self.shared.limits.max_message_bytes)
    }

    /// Starts a text prompt with bounded PNG evidence. Image content is rejected unless
    /// the adapter negotiated ACP's image prompt capability, and the whole request must
    /// fit one wire message (see [`Self::prompt_image_budget`]).
    pub fn prompt_with_images(
        &self,
        text: &str,
        images: Vec<PromptImage>,
    ) -> Result<u64, DriverError> {
        self.prompt_with_files(text, Vec::new(), images)
    }

    /// Whether the adapter advertised the `_session/steering` extension: the only way a
    /// message can reach a running ACP turn.
    pub(in crate::driver) fn steering_negotiated(&self) -> bool {
        self.shared.steering.load(Ordering::SeqCst)
    }

    /// Delivers `text` to the running turn through `_session/steering`. Refused when the
    /// adapter did not advertise the extension, no turn runs, or the turn is being stopped.
    pub fn steer_turn(&self, text: &str) -> Result<(), DriverError> {
        self.require_usable()?;
        if !self.steering_negotiated() {
            return Err(DriverError::Unsupported(
                "this agent cannot take a message during a turn",
            ));
        }
        validate_prompt(text, &[])?;
        match &self.shared.core.lock().turn {
            Some(turn) if !turn.cancel_requested => {}
            _ => return Err(DriverError::NoActiveTurn),
        }
        self.send(Command::Steer {
            text: text.to_owned(),
        })
    }

    /// Starts a prompt that also points the agent at files (ACP resource links, which every
    /// agent must accept) and carries bounded PNG evidence. See
    /// [`Self::prompt_with_images`] for the image rules.
    pub fn prompt_with_files(
        &self,
        text: &str,
        files: Vec<PromptFile>,
        images: Vec<PromptImage>,
    ) -> Result<u64, DriverError> {
        validate_prompt(text, &images)?;
        if files.len() > MAX_PROMPT_FILE_COUNT {
            return Err(DriverError::InvalidPrompt("too many attached files"));
        }
        if files.iter().any(|file| {
            file.name().trim().is_empty() || file.path().is_some_and(|path| !path.is_absolute())
        }) {
            return Err(DriverError::InvalidPrompt(
                "an attached file needs a name and an absolute path",
            ));
        }
        if files.iter().any(PromptFile::is_embedded)
            && !self
                .initialized()
                .is_some_and(|info| info.capabilities.prompt_embedded_context)
        {
            return Err(DriverError::InvalidPrompt(
                "the adapter did not negotiate embedded file content",
            ));
        }
        let mut file_blocks = Vec::with_capacity(files.len());
        for file in &files {
            file_blocks.push(file_block(file)?);
        }
        if !images.is_empty()
            && !self
                .initialized()
                .is_some_and(|info| info.capabilities.prompt_image)
        {
            return Err(DriverError::InvalidPrompt(
                "the adapter did not negotiate image prompts",
            ));
        }
        // The whole request must fit one wire message, or the transport would fail the
        // driver when it is written. Images are measured encoded before anything is
        // encoded, then the exact line is measured; a rejection changes no state.
        let max = self.shared.limits.max_message_bytes;
        let encoded = images.iter().try_fold(0usize, |total, image| {
            total.checked_add(base64_len(image.data.len())?)
        });
        if !encoded.is_some_and(|bytes| fits_wire(bytes, max)) {
            return Err(DriverError::InvalidPrompt(
                "prompt images exceed the message size limit",
            ));
        }
        let mut content = Vec::with_capacity(images.len() + 1);
        content.push(ContentBlock::Text(TextContent::new(text)));
        content.extend(file_blocks);
        content.extend(images.into_iter().map(|image| {
            ContentBlock::Image(ImageContent::new(STANDARD.encode(image.data), "image/png"))
        }));
        // Without a session the status check below refuses the prompt anyway.
        let session = self.shared.core.lock().session.clone();
        if let Some(session) = session {
            let request = PromptRequest::new(session, content);
            if !fits_wire(prompt_line_bytes(&request), max) {
                return Err(DriverError::InvalidPrompt(
                    "prompt exceeds the message size limit",
                ));
            }
            content = request.prompt;
        }
        let turn = self.shared.begin_turn()?;
        if let Err(error) = self.send(Command::Prompt { turn, content }) {
            self.shared.abandon_turn(turn);
            return Err(error);
        }
        Ok(turn)
    }

    /// Stops the active turn. The cancellation is latched before this call returns: the
    /// turn can no longer complete as uncancelled and its permissions are closed. The
    /// adapter keeps running and must answer with an authoritative `cancelled`. Repeated
    /// calls coalesce into one turn-bound notification.
    pub fn cancel_prompt(&self) -> Result<(), DriverError> {
        if self.shared.latch_cancel(false)?.is_some() {
            // The notification itself rides the latched state; a full queue only delays
            // the wake-up because queued commands wake the loop anyway.
            let _ = self.send(Command::Wake);
        }
        Ok(())
    }

    /// Writer-containment qualification only: kills the adapter's own process (not its
    /// tree) through the handle it was started with, as a provider crash would. The
    /// driver then observes the closed transport like any crash.
    pub fn crash_adapter_for_qualification(&self) -> std::io::Result<()> {
        self.shared.child.lock().child_mut().kill()
    }

    /// Processes the adapter started in its task scope so far (`None` where the platform
    /// does not record them or the record is incomplete).
    pub fn observed_descendants(&self) -> Option<usize> {
        self.shared.child.lock().observed_descendants()
    }

    /// Answers one pending permission request. Duplicate, late or unknown ids fail.
    pub fn reply_permission(
        &self,
        id: PermissionId,
        reply: PermissionReply,
    ) -> Result<(), DriverError> {
        self.shared.reply_permission(id, reply)
    }

    pub fn set_mode(&self, mode_id: &str) -> Result<(), DriverError> {
        self.require_usable()?;
        let raw = self.shared.resolve_mode(mode_id)?;
        let key = self.admit_request()?;
        self.send(Command::SetMode {
            raw,
            label: mode_id.to_owned(),
            key,
        })
        .inspect_err(|_| self.shared.watchdog.untrack(key))
    }

    pub fn set_config_option(&self, id: &str, value: OptionValue) -> Result<(), DriverError> {
        self.require_usable()?;
        let (raw_id, value) = self.shared.resolve_option(id, &value)?;
        let key = self.admit_request()?;
        self.send(Command::SetConfig {
            raw_id,
            label: id.to_owned(),
            value,
            key,
        })
        .inspect_err(|_| self.shared.watchdog.untrack(key))
    }

    /// Admits one adapter request with a reply deadline, or refuses when the bound of
    /// outstanding requests is reached.
    pub(super) fn admit_request(&self) -> Result<u64, DriverError> {
        self.shared
            .watchdog
            .track_request(self.shared.limits.request_timeout, MAX_OUTSTANDING_REQUESTS)
            .ok_or(DriverError::Busy(
                "too many mode/config requests await an adapter reply",
            ))
    }

    /// Stops the adapter: closes requests as cancelled, drains for the graceful period,
    /// force-kills the owned tree and verifies the group is empty.
    pub fn shutdown(mut self) -> DriverOutcome {
        self.stop()
    }

    pub(super) fn stop(&mut self) -> DriverOutcome {
        let threads = Threads {
            main: self.main.take(),
            watchdog: self.watchdog.take(),
            io: std::mem::take(&mut self.io),
        };
        self.shared.stop_owned(
            || {
                let _ = self.send(Command::Shutdown);
                // Dropping the sender also ends the command loop if the queue was full.
                self.commands.lock().take();
            },
            threads,
        )
    }
}

/// The content block of one attached file: a resource link, or the content itself as an
/// embedded text (valid UTF-8) or binary resource.
fn file_block(file: &PromptFile) -> Result<ContentBlock, DriverError> {
    if let Some(path) = file.path() {
        let uri = file_uri(path).ok_or(DriverError::InvalidPrompt(
            "an attached file path is not valid text",
        ))?;
        return Ok(ContentBlock::ResourceLink(ResourceLink::new(
            file.name(),
            uri,
        )));
    }
    let uri = format!("attachment:///{}", percent_encode(file.name()));
    let bytes = file.content_bytes();
    let resource = match std::str::from_utf8(bytes) {
        Ok(text) => {
            EmbeddedResourceResource::TextResourceContents(TextResourceContents::new(text, uri))
        }
        Err(_) => EmbeddedResourceResource::BlobResourceContents(BlobResourceContents::new(
            STANDARD.encode(bytes),
            uri,
        )),
    };
    Ok(ContentBlock::Resource(EmbeddedResource::new(resource)))
}

/// The `file://` URI of an absolute path, percent-encoded. A Windows verbatim prefix,
/// drive letters and UNC shares (`file://server/share/...`) are handled so the same link
/// works on every host. `None` when the path is not valid UTF-8: a link must name exactly
/// the file that was checked, never a lossy lookalike.
fn file_uri(path: &std::path::Path) -> Option<String> {
    let text = path.to_str()?;
    let slashed = text.replace('\\', "/");
    let (host, rest) = if let Some(unc) = slashed.strip_prefix("//?/UNC/") {
        split_share(unc)
    } else if let Some(plain) = slashed.strip_prefix("//?/") {
        (String::new(), plain.to_owned())
    } else if let Some(unc) = slashed.strip_prefix("//") {
        split_share(unc)
    } else {
        (String::new(), slashed)
    };
    let mut uri = format!("file://{}", percent_encode_host(&host));
    if !rest.starts_with('/') {
        uri.push('/');
    }
    uri.push_str(&percent_encode(&rest));
    Some(uri)
}

fn split_share(unc: &str) -> (String, String) {
    match unc.split_once('/') {
        Some((server, rest)) => (server.to_owned(), format!("/{rest}")),
        None => (unc.to_owned(), String::from("/")),
    }
}

fn percent_encode_host(host: &str) -> String {
    host.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(char::from(byte));
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Checks every wire shares: non-empty bounded text and at most a few non-empty PNGs.
pub(in crate::driver) fn validate_prompt(
    text: &str,
    images: &[PromptImage],
) -> Result<(), DriverError> {
    if text.trim().is_empty() {
        return Err(DriverError::InvalidPrompt("prompt is empty"));
    }
    if text.len() > MAX_PROMPT_BYTES {
        return Err(DriverError::InvalidPrompt("prompt exceeds the size limit"));
    }
    if images.len() > MAX_PROMPT_IMAGE_COUNT {
        return Err(DriverError::InvalidPrompt("too many prompt images"));
    }
    if images.iter().any(|image| image.data.is_empty()) {
        return Err(DriverError::InvalidPrompt("prompt image is empty"));
    }
    if images
        .iter()
        .any(|image| !image.data.starts_with(b"\x89PNG\r\n\x1a\n"))
    {
        return Err(DriverError::InvalidPrompt("prompt image is not a PNG"));
    }
    Ok(())
}

/// The largest total of raw image bytes `count` images can carry on a line whose framing
/// without any image data is `framing` bytes. Each image rounds up to whole 4-byte base64
/// groups, so 4 * ceil(n / 3) <= (4n + 8) / 3 and the total n must satisfy
/// 4n <= 3 * room - 8c.
pub(in crate::driver) fn image_budget(framing: usize, count: usize, max_message: usize) -> usize {
    let max_line = max_message.saturating_sub(1);
    let room = max_line.saturating_sub(framing);
    room.saturating_mul(3).saturating_sub(8 * count) / 4
}

impl Shared {
    /// Opens a turn when the session is ready and idle and arms its deadlines.
    pub(in crate::driver) fn begin_turn(&self) -> Result<u64, DriverError> {
        let id = {
            let mut core = self.core.lock();
            if let Some(failure) = &core.failure {
                return Err(DriverError::Failed(failure.clone()));
            }
            match core.status {
                DriverStatus::Ready if core.session.is_some() => {}
                DriverStatus::Prompting => return Err(DriverError::TurnInProgress),
                DriverStatus::Starting => return Err(DriverError::NotReady),
                _ => return Err(DriverError::Closed),
            }
            let id = core.next_turn;
            core.next_turn = id.checked_add(1).ok_or_else(|| {
                DriverError::Failed(AgentFailure::new(
                    FailureKind::CounterExhausted,
                    Phase::Prompt,
                    "turn counter exhausted",
                ))
            })?;
            core.turn = Some(Turn {
                id,
                cancel_requested: false,
            });
            core.status = DriverStatus::Prompting;
            core.phase = Phase::Prompt;
            id
        };
        self.watchdog
            .arm_turn(self.limits.prompt_timeout, self.limits.max_turn);
        Ok(id)
    }

    /// Takes back a turn whose prompt could not be sent: never leave a phantom turn that
    /// nothing will ever answer.
    pub(in crate::driver) fn abandon_turn(&self, turn: u64) {
        let mut core = self.core.lock();
        if core.turn.as_ref().map(|t| t.id) == Some(turn) {
            core.turn = None;
            if core.status == DriverStatus::Prompting {
                core.status = DriverStatus::Ready;
            }
        }
        drop(core);
        self.watchdog.disarm();
    }
}

/// The threads one driver owns besides the caller's.
pub(in crate::driver) struct Threads {
    pub(in crate::driver) main: Option<JoinHandle<()>>,
    pub(in crate::driver) watchdog: Option<JoinHandle<()>>,
    pub(in crate::driver) io: Vec<JoinHandle<()>>,
}

impl Shared {
    /// Stops the adapter: closes requests as cancelled, asks the connection to shut down
    /// (`request_shutdown`), drains for the graceful period, force-kills the owned tree
    /// and verifies the group is empty.
    pub(in crate::driver) fn stop_owned(
        &self,
        request_shutdown: impl FnOnce(),
        threads: Threads,
    ) -> DriverOutcome {
        {
            let mut core = self.core.lock();
            if core.status != DriverStatus::Closed {
                core.stopping = true;
                core.status = DriverStatus::Stopping;
            }
        }
        // Latch the stop before anything else: permissions close and the turn is flagged.
        let _ = self.latch_cancel(true);
        request_shutdown();
        let grace = self.limits.shutdown_grace;
        if !self.wait_closed(grace + Duration::from_secs(1)) {
            // The connection did not unwind by itself (for example it is blocked on a pipe
            // an escaped helper still holds open): force the owned tree down here.
            self.kill_now();
            self.wait_closed(grace + Duration::from_secs(1));
        }
        let existing = self.core.lock().outcome.clone();
        let outcome = match existing {
            Some(outcome) => outcome,
            None => {
                // Wedged connection: verify the tree here, from one consistent snapshot,
                // with the same sticky ownership demotion as the normal path.
                let termination = self.teardown();
                self.stderr_finish();
                let outcome = self.snapshot_outcome(termination, None);
                self.publish_closed(outcome)
            }
        };
        self.watchdog.stop();
        self.ingress.close();
        let Threads { main, watchdog, io } = threads;
        for handle in main.into_iter().chain(watchdog).chain(io) {
            // A pipe held open by an escaped helper must not wedge shutdown.
            join_when_finished(handle);
        }
        outcome
    }

    pub(in crate::driver) fn wait_closed(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut core = self.core.lock();
        while core.status != DriverStatus::Closed {
            if self
                .core_changed
                .wait_until(&mut core, deadline)
                .timed_out()
            {
                return core.status == DriverStatus::Closed;
            }
        }
        true
    }

    /// Blocks until the session (or, in `InitializeOnly` mode, initialization) is ready,
    /// the driver fails, or `timeout` passes.
    pub(in crate::driver) fn wait_ready(
        &self,
        timeout: Duration,
    ) -> Result<SessionInfo, AgentFailure> {
        let deadline = Instant::now() + timeout;
        let mut core = self.core.lock();
        loop {
            if let Some(failure) = &core.failure {
                return Err(failure.clone());
            }
            if core.status == DriverStatus::Closed {
                return Err(AgentFailure::new(
                    FailureKind::ProcessExited,
                    core.phase,
                    "driver closed before it became ready",
                ));
            }
            if core.status != DriverStatus::Starting
                && let Some(initialized) = &core.initialized
            {
                let initialized = initialized.clone();
                let restored = core.restored;
                let session = core.session.clone();
                drop(core);
                return Ok(SessionInfo {
                    session_id: session.map(|s| self.clean(&s)),
                    restored,
                    initialized,
                });
            }
            if self
                .core_changed
                .wait_until(&mut core, deadline)
                .timed_out()
            {
                return Err(AgentFailure::new(
                    FailureKind::DeadlineExceeded,
                    core.phase,
                    "timed out waiting for the adapter to become ready",
                ));
            }
        }
    }

    /// Blocks until `turn` finishes, the driver fails, or `timeout` passes. A latched
    /// failure always wins over a recorded completion.
    pub(in crate::driver) fn wait_turn(
        &self,
        turn: u64,
        timeout: Duration,
    ) -> Result<PromptOutcome, AgentFailure> {
        let deadline = Instant::now() + timeout;
        let mut core = self.core.lock();
        loop {
            if let Some(failure) = &core.failure {
                return Err(failure.clone());
            }
            if let Some(outcome) = &core.last_outcome
                && outcome.turn == turn
            {
                return Ok(outcome.clone());
            }
            if core.status == DriverStatus::Closed {
                return Err(AgentFailure::new(
                    FailureKind::ProcessExited,
                    core.phase,
                    "driver closed before the turn finished",
                ));
            }
            if self
                .core_changed
                .wait_until(&mut core, deadline)
                .timed_out()
            {
                return Err(AgentFailure::new(
                    FailureKind::DeadlineExceeded,
                    core.phase,
                    "timed out waiting for the turn to finish",
                ));
            }
        }
    }
}

/// Joins a thread that is finishing; a still-blocked thread is detached after a
/// short wait because it only holds shared state, never a lock.
pub(super) fn join_when_finished(handle: JoinHandle<()>) {
    let deadline = Instant::now() + Duration::from_millis(500);
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
}

impl Drop for AcpDriver {
    fn drop(&mut self) {
        if self.main.is_some() {
            let _ = self.stop();
        }
    }
}

/// The adapter process just spawned in its scope, with its pipes.
pub(in crate::driver) struct Spawned {
    child: Arc<Mutex<TrackedChild>>,
    stdin: std::process::ChildStdin,
    stdout: std::process::ChildStdout,
    stderr: std::process::ChildStderr,
}

/// Who owns a driver and how it is bounded.
pub(in crate::driver) struct Owner {
    pub(in crate::driver) provider: String,
    pub(in crate::driver) task: String,
    pub(in crate::driver) ownership: OwnershipSource,
    pub(in crate::driver) limits: DriverLimits,
    pub(in crate::driver) redactor: Arc<Redactor>,
}

/// Spawns the adapter inside the task's process scope (`cwd` must be an existing absolute
/// directory) and takes its pipes.
pub(in crate::driver) fn spawn_owned(
    launch: &AdapterLaunch,
    args: &[String],
    cwd: &std::path::Path,
    processes: &ProcessTreeManager,
    redactor: &Redactor,
) -> Result<Spawned, AgentFailure> {
    if !cwd.is_absolute() || !cwd.is_dir() {
        return Err(AgentFailure::new(
            FailureKind::SpawnFailed,
            Phase::Spawn,
            "session working directory must be an existing absolute directory",
        ));
    }
    let child = AgentSupervisor::new(processes.clone())
        .spawn_adapter(&launch.executable, args, cwd, Some(launch.env.clone()))
        .map_err(|error| spawn_failure(launch, error, redactor))?;
    let (stdin, stdout, stderr) = {
        let mut child = child.lock();
        let process = child.child_mut();
        (
            process.stdin.take(),
            process.stdout.take(),
            process.stderr.take(),
        )
    };
    let (Some(stdin), Some(stdout), Some(stderr)) = (stdin, stdout, stderr) else {
        let _ = child.lock().kill_forcefully();
        return Err(AgentFailure::new(
            FailureKind::SpawnFailed,
            Phase::Spawn,
            "adapter pipes were not available",
        ));
    };
    Ok(Spawned {
        child,
        stdin,
        stdout,
        stderr,
    })
}

/// A started driver's shared state, inbound line stream, outbound sink and io threads.
pub(in crate::driver) type Started = (
    Arc<Shared>,
    mpsc::Receiver<std::io::Result<String>>,
    LineSink,
    Vec<JoinHandle<()>>,
);

impl Shared {
    /// Builds the shared state for a spawned adapter, arms the initialize deadline and
    /// starts the stdout, stderr and stdin threads. Returns the inbound line stream, the
    /// outbound sink and the io threads.
    pub(in crate::driver) fn start(spawned: Spawned, owner: Owner) -> Started {
        let Spawned {
            child,
            stdin,
            stdout,
            stderr,
        } = spawned;
        let Owner {
            provider,
            task,
            ownership,
            limits,
            redactor,
        } = owner;
        let ownership = match ownership {
            OwnershipSource::Verified => child.lock().containment(),
            OwnershipSource::Injected(ownership) => ownership,
        };
        let queue = EventQueue::new(provider, task, limits.max_queued_events);
        let ingress = Ingress::new(INGRESS_MAX_FRAMES, INGRESS_MAX_BYTES);
        let shared = Arc::new(Shared {
            ownership,
            streams: Mutex::new(Streams {
                agent: StreamRedactor::new(redactor.clone()),
                thought: StreamRedactor::new(redactor.clone()),
                user: StreamRedactor::new(redactor.clone()),
            }),
            stderr: Mutex::new(StderrState {
                redactor: StreamRedactor::new(redactor.clone()),
                carry: Vec::new(),
                tail: VecDeque::new(),
            }),
            redactor,
            queue,
            child,
            core: Mutex::new(Core {
                status: DriverStatus::Starting,
                phase: Phase::Initialize,
                session: None,
                restored: false,
                restoring: false,
                initialized: None,
                turn: None,
                next_turn: 1,
                last_outcome: None,
                failure: None,
                stopping: false,
                escaped: Vec::new(),
                containment_unverified: None,
                outcome: None,
                permissions: Permissions::default(),
                cancel_to_send: None,
            }),
            core_changed: Condvar::new(),
            transcript: Mutex::new(Transcript::new(limits.max_transcript_bytes)),
            options: Mutex::new(AgentOptions::default()),
            option_ids: Mutex::new(OptionIds::default()),
            ingress: ingress.clone(),
            late: AtomicU64::new(0),
            declined: AtomicU64::new(0),
            watchdog: Watchdog::new(),
            forced_kill: AtomicBool::new(false),
            claude_adapter: AtomicBool::new(false),
            steering: AtomicBool::new(false),
            limits,
        });
        shared
            .watchdog
            .arm(Phase::Initialize, shared.limits.init_timeout);
        let (line_tx, line_rx) = mpsc::channel(32);
        let mut io = vec![
            transport::spawn_reader(shared.clone(), stdout, line_tx, ingress),
            transport::spawn_stderr(shared.clone(), stderr),
        ];
        let (sink, writer) = transport::spawn_writer(shared.clone(), stdin);
        io.push(writer);
        (shared, line_rx, sink, io)
    }
}

pub(super) fn spawn_failure(
    launch: &AdapterLaunch,
    error: SupervisorError,
    redactor: &Redactor,
) -> AgentFailure {
    let (kind, message) = match &error {
        SupervisorError::Process(ProcessError::SpawnFailed { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            if launch.executable.exists() {
                (
                    FailureKind::MissingRuntime,
                    format!(
                        "adapter {} exists but its runtime could not be started",
                        launch.executable.display()
                    ),
                )
            } else {
                (
                    FailureKind::SpawnFailed,
                    format!("adapter {} was not found", launch.executable.display()),
                )
            }
        }
        other => (FailureKind::SpawnFailed, other.to_string()),
    };
    AgentFailure::new(kind, Phase::Spawn, redactor.redact(&message))
}

#[cfg(test)]
mod uri_tests {
    use super::file_uri;
    use std::path::Path;

    #[test]
    fn drive_paths_and_verbatim_prefixes_become_file_uris() {
        assert_eq!(
            file_uri(Path::new(r"C:\my dir\a.txt")).as_deref(),
            Some("file:///C:/my%20dir/a.txt")
        );
        assert_eq!(
            file_uri(Path::new(r"\\?\C:\p\a.txt")).as_deref(),
            Some("file:///C:/p/a.txt")
        );
        assert_eq!(
            file_uri(Path::new("/home/u/a b.txt")).as_deref(),
            Some("file:///home/u/a%20b.txt")
        );
    }

    #[test]
    fn unc_shares_keep_their_host() {
        assert_eq!(
            file_uri(Path::new(r"\\?\UNC\srv\share\f.txt")).as_deref(),
            Some("file://srv/share/f.txt")
        );
        assert_eq!(
            file_uri(Path::new(r"\\srv\share\dir\f.txt")).as_deref(),
            Some("file://srv/share/dir/f.txt")
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_path_that_is_not_valid_text_has_no_uri() {
        use std::{ffi::OsString, os::windows::ffi::OsStringExt};
        let lone_surrogate = OsString::from_wide(&[0x43, 0x3a, 0x5c, 0xd800, 0x61]);
        assert_eq!(file_uri(Path::new(&lone_surrogate)), None);
    }
}
