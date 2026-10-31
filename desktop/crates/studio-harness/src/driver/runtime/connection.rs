//! The connection task: spawns the adapter, drives the ACP session and runs commands.

use super::*;

pub(super) struct DriveParams {
    pub(super) cwd: PathBuf,
    pub(super) mode: DriverMode,
    pub(super) resume: Option<String>,
    pub(super) auth_method: Option<String>,
    pub(super) mcp_servers: Vec<McpServer>,
    /// The user's HTTP servers: sent only when the agent advertises `mcpCapabilities.http`.
    pub(super) mcp_http_servers: Vec<McpServer>,
    /// Names of the user's servers among `mcp_servers` (stdio) and `mcp_http_servers`.
    pub(super) user_stdio_names: Vec<String>,
    pub(super) user_http_names: Vec<String>,
    pub(super) mcp_stdio: bool,
}

/// The servers a session gets: the stdio ones, and the HTTP ones only when the agent said
/// at `initialize` that it takes HTTP servers (it may refuse the session otherwise).
pub(super) fn session_servers(params: &DriveParams, agent_takes_http: bool) -> Vec<McpServer> {
    let mut servers = params.mcp_servers.clone();
    if agent_takes_http {
        servers.extend(params.mcp_http_servers.iter().cloned());
    }
    servers
}

/// The user's servers (by name) that [`session_servers`] sends.
pub(super) fn session_user_servers(params: &DriveParams, agent_takes_http: bool) -> Vec<String> {
    let mut names = params.user_stdio_names.clone();
    if agent_takes_http {
        names.extend(params.user_http_names.iter().cloned());
    }
    names
}

pub(super) enum Plan {
    New,
    Resume(String),
    Load(String),
}

/// Length of the standard base64 encoding of `bytes` bytes (with padding).
pub(super) fn base64_len(bytes: usize) -> Option<usize> {
    bytes.div_ceil(3).checked_mul(4)
}

/// Counts serialized bytes without buffering them.
pub(super) struct ByteCount(usize);

impl std::io::Write for ByteCount {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn json_len(value: &impl serde::Serialize) -> Option<usize> {
    let mut count = ByteCount(0);
    serde_json::to_writer(&mut count, value).ok()?;
    Some(count.0)
}

/// Bytes of the line (without its newline) the SDK writes for `request`: the JSON-RPC
/// envelope it builds around the serialized params. The SDK's request ids are always
/// UUID strings, so a placeholder of the same length gives the exact size. `usize::MAX`
/// when it cannot be serialized, which never fits.
pub(super) fn prompt_line_bytes(request: &PromptRequest) -> usize {
    const SDK_REQUEST_ID: &str = "00000000-0000-0000-0000-000000000000";
    let envelope = RawJsonRpcMessage::request(
        request.method().to_owned(),
        serde_json::json!({}),
        RequestId::Str(SDK_REQUEST_ID.to_owned()),
    )
    .ok()
    .and_then(|message| json_len(&message));
    // The empty `{}` params in the envelope are replaced by the real ones.
    match (envelope, json_len(request)) {
        (Some(envelope), Some(params)) => (envelope - 2).saturating_add(params),
        _ => usize::MAX,
    }
}

pub(super) fn wire_server(server: &McpStdioServer) -> McpServer {
    McpServer::Stdio(
        McpServerStdio::new(server.name.clone(), server.command.clone())
            .args(server.args.clone())
            .env(
                server
                    .env
                    .iter()
                    .map(|(name, value)| EnvVariable::new(name.clone(), value.clone()))
                    .collect(),
            ),
    )
}

/// ACP v1 `McpServer::Http`: `{"type":"http","name","url","headers":[{"name","value"}]}`.
pub(super) fn wire_http_server(server: &McpHttpServer) -> McpServer {
    McpServer::Http(
        McpServerHttp::new(server.name.clone(), server.url.clone()).headers(
            server
                .headers
                .iter()
                .map(|(name, value)| HttpHeader::new(name.clone(), value.clone()))
                .collect(),
        ),
    )
}

/// Whether a prompt error is the Claude adapter reporting a turn that ended with only
/// server-side tool calls (`srvtoolu_` ids: tools the model API runs itself, such as the
/// advisor) left without a result. The client could not have supplied one, and the adapter
/// has already closed those calls as failed. The adapter tags this error with
/// `data.errorKind = "incomplete_tool_call"` and lists every unfinished id after the
/// message prefix; any other id, a missing tag or another code is a real failure.
pub(super) fn only_server_tools_unfinished(error: &RpcError) -> bool {
    const MARKER: &str = "Claude ended the turn without returning results for tool calls: ";
    const SERVER_TOOL: &str = "srvtoolu_";
    if i32::from(error.code) != i32::from(agent_client_protocol::ErrorCode::InternalError) {
        return false;
    }
    let tagged = error
        .data
        .as_ref()
        .and_then(|data| data.get("errorKind"))
        .and_then(serde_json::Value::as_str)
        == Some("incomplete_tool_call");
    if !tagged {
        return false;
    }
    let Some((_, ids)) = error.message.split_once(MARKER) else {
        return false;
    };
    let mut ids = ids
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|id| !id.is_empty())
        .peekable();
    ids.peek().is_some()
        && ids.all(|id| id.len() > SERVER_TOOL.len() && id.starts_with(SERVER_TOOL))
}

pub(super) fn initialized_info(init: &InitializeResponse, mcp_stdio: bool) -> InitializedInfo {
    let caps = &init.agent_capabilities;
    InitializedInfo {
        agent_name: init
            .agent_info
            .as_ref()
            .map_or_else(|| "unknown".to_owned(), |i| i.name.clone()),
        agent_version: init
            .agent_info
            .as_ref()
            .map_or_else(|| "unknown".to_owned(), |i| i.version.clone()),
        protocol_version: 1,
        capabilities: AgentCapabilityInfo {
            load_session: caps.load_session,
            resume_session: caps.session_capabilities.resume.is_some(),
            prompt_image: caps.prompt_capabilities.image,
            prompt_audio: caps.prompt_capabilities.audio,
            prompt_embedded_context: caps.prompt_capabilities.embedded_context,
            mcp_http: caps.mcp_capabilities.http,
            mcp_sse: caps.mcp_capabilities.sse,
            mcp_stdio,
        },
        auth_methods: init
            .auth_methods
            .iter()
            .map(|m| m.id().0.to_string())
            .collect(),
        authenticated: false,
    }
}

pub(super) type SessionReply = Result<
    (
        Option<String>,
        Option<SessionModeState>,
        Option<Vec<SessionConfigOption>>,
    ),
    RpcError,
>;

pub(super) async fn drive(
    shared: Arc<Shared>,
    cx: ConnectionTo<Agent>,
    mut commands: mpsc::Receiver<Command>,
    params: DriveParams,
) -> Result<(), AgentFailure> {
    shared.set_phase(Phase::Initialize);
    let init = cx
        .send_request(
            InitializeRequest::new(ProtocolVersion::V1)
                // Optional client services (fs, terminal) are declined by omission.
                .client_capabilities(ClientCapabilities::new())
                .client_info(Implementation::new(CLIENT_NAME, env!("CARGO_PKG_VERSION"))),
        )
        .block_task()
        .await
        .map_err(|e| shared.rpc_failure(Phase::Initialize, e))?;
    if init.protocol_version != ProtocolVersion::V1 {
        return Err(AgentFailure::new(
            FailureKind::UnsupportedVersion,
            Phase::Initialize,
            format!(
                "adapter negotiated ACP protocol version {}; only version 1 is supported",
                init.protocol_version
            ),
        ));
    }
    shared.steering.store(
        init.agent_capabilities
            .meta
            .as_ref()
            .and_then(|meta| meta.get(STEERING_CAPABILITY))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        Ordering::SeqCst,
    );
    let raw_info = initialized_info(&init, params.mcp_stdio);
    shared.claude_adapter.store(
        CLAUDE_ADAPTER_NAMES.contains(&raw_info.agent_name.as_str()),
        Ordering::SeqCst,
    );
    // Wire metadata is adapter-controlled text: only a sanitized copy is exported.
    let mut info = raw_info.clone();
    info.agent_name = shared.clean(&raw_info.agent_name);
    info.agent_version = shared.clean(&raw_info.agent_version);
    info.auth_methods = raw_info
        .auth_methods
        .iter()
        .map(|m| shared.clean(m))
        .collect();
    shared.core.lock().initialized = Some(info.clone());
    shared.emit(AgentEventKind::Initialized(info.clone()));
    if params.mode == DriverMode::InitializeOnly {
        shared.watchdog.disarm();
        shared.set_ready();
        return command_loop(&shared, &cx, &mut commands).await;
    }
    if let Some(method) = &params.auth_method {
        shared.set_phase(Phase::Authenticate);
        if !raw_info.auth_methods.contains(method) {
            return Err(AgentFailure::new(
                FailureKind::AuthRejected,
                Phase::Authenticate,
                format!(
                    "authentication method '{}' is not advertised by the adapter",
                    shared.clean(method)
                ),
            ));
        }
        cx.send_request(AuthenticateRequest::new(method.clone()))
            .block_task()
            .await
            .map_err(|e| shared.rpc_failure(Phase::Authenticate, e))?;
    }
    shared.set_phase(Phase::Session);
    let plan = match &params.resume {
        None => Plan::New,
        Some(id) if info.capabilities.resume_session => Plan::Resume(id.clone()),
        Some(id) if info.capabilities.load_session => Plan::Load(id.clone()),
        Some(_) => {
            return Err(AgentFailure::new(
                FailureKind::RestoreUnsupported,
                Phase::Session,
                "adapter did not negotiate session restoration; start a new session instead",
            ));
        }
    };
    let servers = session_servers(&params, raw_info.capabilities.mcp_http);
    let (done_tx, done_rx) = futures::channel::oneshot::channel::<SessionReply>();
    let restored = !matches!(plan, Plan::New);
    match plan {
        Plan::New => {
            cx.send_request(
                NewSessionRequest::new(params.cwd.clone()).mcp_servers(servers.clone()),
            )
            .on_receiving_result(move |result| {
                let _ =
                    done_tx
                        .send(result.map(|r| {
                            (Some(r.session_id.0.to_string()), r.modes, r.config_options)
                        }));
                async { Ok(()) }
            })
            .map_err(|e| shared.rpc_failure(Phase::Session, e))?;
        }
        Plan::Resume(id) => {
            shared.core.lock().session = Some(id.clone());
            cx.send_request(
                ResumeSessionRequest::new(id, params.cwd.clone()).mcp_servers(servers.clone()),
            )
            .on_receiving_result(move |result| {
                let _ = done_tx.send(result.map(|r| (None, r.modes, r.config_options)));
                async { Ok(()) }
            })
            .map_err(|e| shared.rpc_failure(Phase::Session, e))?;
        }
        Plan::Load(id) => {
            {
                let mut core = shared.core.lock();
                core.session = Some(id.clone());
                core.restoring = true;
            }
            cx.send_request(
                LoadSessionRequest::new(id, params.cwd.clone()).mcp_servers(servers.clone()),
            )
            .on_receiving_result(move |result| {
                let _ = done_tx.send(result.map(|r| (None, r.modes, r.config_options)));
                async { Ok(()) }
            })
            .map_err(|e| shared.rpc_failure(Phase::Session, e))?;
        }
    }
    let reply = done_rx.await.map_err(|_| {
        AgentFailure::new(
            FailureKind::SessionRejected,
            Phase::Session,
            "session request was dropped before a response",
        )
    })?;
    let (new_id, modes, config) = reply.map_err(|e| shared.rpc_failure(Phase::Session, e))?;
    {
        let mut core = shared.core.lock();
        if let Some(id) = new_id {
            core.session = Some(id);
        }
        core.restoring = false;
        core.restored = restored;
    }
    let session_id = shared
        .core
        .lock()
        .session
        .clone()
        .expect("session set by plan");
    // Hosts only ever see the sanitized id; the wire id stays inside `core`.
    let exported_session = shared.clean(&session_id);
    shared.queue.set_session(&exported_session);
    if let Some(modes) = &modes {
        shared.apply_modes(modes);
    }
    if let Some(config) = &config {
        shared.apply_config(config);
    }
    shared.emit(AgentEventKind::SessionReady {
        session_id: exported_session,
        restored,
        user_mcp_servers: session_user_servers(&params, raw_info.capabilities.mcp_http),
    });
    let snapshot = shared.options.lock().clone();
    if !snapshot.modes.is_empty() || !snapshot.config.is_empty() {
        shared.emit(AgentEventKind::Options(snapshot));
    }
    shared.watchdog.disarm();
    shared.set_ready();
    command_loop(&shared, &cx, &mut commands).await
}

pub(super) async fn command_loop(
    shared: &Arc<Shared>,
    cx: &ConnectionTo<Agent>,
    commands: &mut mpsc::Receiver<Command>,
) -> Result<(), AgentFailure> {
    let internal = |shared: &Shared, error: RpcError| shared.rpc_failure(shared.phase(), error);
    loop {
        // A latched Stop is written here, bound to the turn that was stopped.
        if let Some(session) = shared.take_cancel_to_send() {
            cx.send_notification(CancelNotification::new(session))
                .map_err(|e| internal(shared, e))?;
        }
        let incoming = std::pin::pin!(cx.incoming_closed());
        let next = std::pin::pin!(commands.next());
        match futures::future::select(next, incoming).await {
            Either::Left((None, _)) => return Ok(()),
            Either::Right(_) => {
                return if shared.is_stopping() {
                    Ok(())
                } else {
                    Err(shared.process_exit_failure())
                };
            }
            Either::Left((Some(command), _)) => match command {
                Command::Wake => {}
                Command::Prompt { turn, content } => {
                    let session = shared.core.lock().session.clone().ok_or_else(|| {
                        AgentFailure::new(FailureKind::Internal, Phase::Idle, "no session")
                    })?;
                    let sh = shared.clone();
                    cx.send_request(PromptRequest::new(session, content))
                        // Runs in dispatch order, so updates after the response are late.
                        .on_receiving_result(move |result| {
                            sh.on_prompt_result(turn, result);
                            async { Ok(()) }
                        })
                        .map_err(|e| internal(shared, e))?;
                }
                Command::SetMode { raw, label, key } => {
                    let session = shared.core.lock().session.clone().unwrap_or_default();
                    let sh = shared.clone();
                    cx.send_request(SetSessionModeRequest::new(session, raw))
                        .on_receiving_result(move |result| {
                            sh.on_option_result(&label, key, result.map(|_| None), true);
                            async { Ok(()) }
                        })
                        .map_err(|e| internal(shared, e))?;
                }
                Command::SetConfig {
                    raw_id,
                    label,
                    value,
                    key,
                } => {
                    let session = shared.core.lock().session.clone().unwrap_or_default();
                    let sh = shared.clone();
                    let wire = match value {
                        OptionValue::Value(v) => SessionConfigOptionValue::value_id(v),
                        OptionValue::Boolean(b) => SessionConfigOptionValue::boolean(b),
                    };
                    cx.send_request(SetSessionConfigOptionRequest::new(session, raw_id, wire))
                        .on_receiving_result(move |result| {
                            sh.on_option_result(
                                &label,
                                key,
                                result.map(|r| Some(r.config_options)),
                                false,
                            );
                            async { Ok(()) }
                        })
                        .map_err(|e| internal(shared, e))?;
                }
                Command::Steer { text } => {
                    let session = shared.core.lock().session.clone().unwrap_or_default();
                    let request = UntypedMessage::new(
                        STEERING_METHOD,
                        serde_json::json!({ "sessionId": session, "text": text }),
                    )
                    .map_err(|e| internal(shared, e))?;
                    let sh = shared.clone();
                    cx.send_request(request)
                        .on_receiving_result(move |result| {
                            let (accepted, message) = match result {
                                Ok(_) => (true, String::new()),
                                Err(error) => (false, sh.clean(&error.message)),
                            };
                            sh.emit(AgentEventKind::SteerResult {
                                text,
                                accepted,
                                message,
                            });
                            async { Ok(()) }
                        })
                        .map_err(|e| internal(shared, e))?;
                }
                Command::Shutdown => {
                    // Stop latches cancellation itself; deliver the notification best effort.
                    let _ = shared.latch_cancel(true);
                    if let Some(session) = shared.take_cancel_to_send() {
                        let _ = cx.send_notification(CancelNotification::new(session));
                    }
                    shared.close_permissions();
                    return Ok(());
                }
            },
        }
    }
}

pub(super) fn run_connection(
    shared: Arc<Shared>,
    sink: LineSink,
    stream: LineStream,
    commands: mpsc::Receiver<Command>,
    params: DriveParams,
) {
    let notify_shared = shared.clone();
    let permission_shared = shared.clone();
    let decline_shared = shared.clone();
    let ingress_shared = shared.clone();
    let drive_shared = shared.clone();
    // SDK diagnostics trace whole wire lines (including outbound MCP environment values
    // and inbound text) before any application redaction. Nothing the SDK logs from this
    // connection may reach a host subscriber, so the connection thread runs under a
    // scoped null subscriber.
    let result =
        tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
            futures::executor::block_on(
                Client
                    .builder()
                    .name(CLIENT_NAME)
                    // First in the chain, so it observes every dispatched message in arrival
                    // order and returns the ingress credit its frame was admitted with.
                    .on_receive_dispatch(
                        async move |dispatch: Dispatch, _cx| {
                            ingress_shared.ingress.release_one();
                            Ok(Handled::No {
                                message: dispatch,
                                retry: false,
                            })
                        },
                        agent_client_protocol::on_receive_dispatch!(),
                    )
                    .on_receive_notification(
                        async move |notification: SessionNotification, _cx| {
                            notify_shared.on_update(notification)
                        },
                        agent_client_protocol::on_receive_notification!(),
                    )
                    .on_receive_request(
                        async move |request: RequestPermissionRequest, responder, _cx| {
                            permission_shared.on_permission_request(request, responder)
                        },
                        agent_client_protocol::on_receive_request!(),
                    )
                    // Optional client services (fs, terminal, ...) are not advertised. The
                    // SDK would leave such a request unanswered, hanging the adapter, so
                    // every remaining request is declined explicitly.
                    .on_receive_dispatch(
                        async move |dispatch: Dispatch, _cx| match dispatch {
                            Dispatch::Request(message, responder) => {
                                decline_shared.declined.fetch_add(1, Ordering::SeqCst);
                                responder
                                    .respond_with_error(
                                        RpcError::method_not_found()
                                            .data(message.method().to_owned()),
                                    )
                                    .map(|()| Handled::Yes)
                            }
                            other => Ok(Handled::No {
                                message: other,
                                retry: false,
                            }),
                        },
                        agent_client_protocol::on_receive_dispatch!(),
                    )
                    .connect_with(
                        Lines::new(sink, stream),
                        async move |cx: ConnectionTo<Agent>| match drive(
                            drive_shared.clone(),
                            cx,
                            commands,
                            params,
                        )
                        .await
                        {
                            Ok(()) => Ok(()),
                            Err(failure) => {
                                let message = failure.message.clone();
                                drive_shared.fail(failure);
                                Err(RpcError::internal_error().data(message))
                            }
                        },
                    ),
            )
        });
    if let Err(error) = result {
        let recorded = shared.core.lock().failure.is_some();
        if !recorded && !shared.is_stopping() {
            let failure = if agent_client_protocol::is_incoming_transport_closed(&error) {
                shared.process_exit_failure()
            } else {
                shared.rpc_failure(shared.phase(), error)
            };
            shared.fail(failure);
        }
    } else if !shared.is_stopping() && shared.core.lock().failure.is_none() {
        // The command channel closed without a stop request: the owner is gone.
        shared.core.lock().stopping = true;
    }
    shared.finish();
}

/// Watchdog and reaper thread: expires deadlines and performs failure-triggered kills so
/// that no handle method ever waits on process cleanup.
pub(super) fn run_watchdog(shared: Arc<Shared>) {
    while let Some(action) = shared.watchdog.next_action() {
        match action {
            WatchAction::Kill => shared.kill_now(),
            WatchAction::Phase(Phase::Prompt) => shared.fail(AgentFailure::new(
                FailureKind::DeadlineExceeded,
                Phase::Prompt,
                format!(
                    "the agent showed no activity for {:?} during the turn (idle timeout)",
                    shared.limits.prompt_timeout
                ),
            )),
            WatchAction::TurnCap => shared.fail(AgentFailure::new(
                FailureKind::DeadlineExceeded,
                Phase::Prompt,
                format!(
                    "the turn ran longer than the {:?} turn limit",
                    shared.limits.max_turn
                ),
            )),
            WatchAction::Phase(expired) => shared.fail(AgentFailure::new(
                FailureKind::DeadlineExceeded,
                expired,
                format!("{expired:?} deadline exceeded"),
            )),
            WatchAction::RequestTimedOut => shared.fail(AgentFailure::new(
                FailureKind::DeadlineExceeded,
                Phase::Idle,
                format!(
                    "the adapter did not answer a mode/config request within {:?}",
                    shared.limits.request_timeout
                ),
            )),
        }
    }
}
