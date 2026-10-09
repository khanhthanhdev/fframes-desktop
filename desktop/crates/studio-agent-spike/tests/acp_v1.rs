//! Real-subprocess transport scenarios for the SDK-based ACP v1 driver.
//!
//! The peer is `tests/acp-peer.py`: it proves transport, framing, lifecycle and
//! process-ownership behavior only and can never qualify authentication or a provider.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
#[cfg(unix)]
use studio_agent_spike::driver::DriverOutcome;
use studio_agent_spike::{
    AdapterConfig,
    discovery::{
        AdapterLaunch, AdapterStatus, ExecutableSearch, ProbeOptions, ResolveError, probe_adapter,
    },
    driver::{
        AcpDriver, AgentEvent, AgentEventKind, DriverConfig, DriverError, DriverLimits, DriverMode,
        FailureKind, McpConfigError, McpStdioServer, McpStdioSupport, MessageRole, OptionValue,
        PermissionReply, PermissionResolution, Phase, StopReasonKind,
    },
};
use studio_bootstrap::{ProcessTreeManager, WriterOwnership};

const SECRET: &str = "sentinel-credential-7f3a9c";
const MCP_SECRET: &str = "mcp-sentinel-env-5d2b81";
const WAIT: Duration = Duration::from_secs(20);

fn peer_script() -> String {
    format!("{}/tests/acp-peer.py", env!("CARGO_MANIFEST_DIR"))
}

fn search() -> ExecutableSearch {
    // The "GUI PATH" is passed explicitly; nothing consults the global PATH implicitly.
    ExecutableSearch {
        managed_dirs: vec![],
        gui_path: std::env::var_os("PATH"),
    }
}

fn launch(root: &Path, mode: &str) -> AdapterLaunch {
    let config = AdapterConfig {
        executable: "python3".into(),
        args: vec![peer_script(), mode.into(), root.to_string_lossy().into()],
        auth_env_names: vec!["ACP_SECRET".into()],
    };
    AdapterLaunch::resolve_with_env(&config, &search(), |name| {
        (name == "ACP_SECRET").then(|| SECRET.to_owned())
    })
    .expect("python3 resolves through the explicit search path")
}

fn config(root: &Path, mode: &str) -> DriverConfig {
    DriverConfig {
        provider: "fixture".into(),
        task: "task-1".into(),
        cwd: root.to_owned(),
        launch: launch(root, mode),
        limits: DriverLimits::default(),
        resume_session: None,
        writer_ownership: WriterOwnership::Unknown,
        mode: DriverMode::Full,
        mcp_servers: Vec::new(),
        mcp_stdio: McpStdioSupport::default(),
    }
}

fn start(root: &Path, mode: &str) -> (AcpDriver, ProcessTreeManager) {
    start_with(config(root, mode))
}

fn start_with(config: DriverConfig) -> (AcpDriver, ProcessTreeManager) {
    let manager = ProcessTreeManager::new();
    let driver = AcpDriver::start(config, &manager).expect("adapter spawns");
    (driver, manager)
}

fn ready(driver: &AcpDriver) {
    driver.wait_ready(WAIT).expect("session becomes ready");
}

fn collect_until(driver: &AcpDriver, mut done: impl FnMut(&AgentEvent) -> bool) -> Vec<AgentEvent> {
    let deadline = Instant::now() + WAIT;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        if let Some(event) = driver.next_event(Duration::from_millis(100)) {
            let stop = done(&event);
            events.push(event);
            if stop {
                return events;
            }
        }
    }
    panic!("expected event never arrived; saw {events:#?}");
}

fn wait_for<T>(mut poll: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(value) = poll() {
            return value;
        }
        assert!(Instant::now() < deadline, "condition never became true");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn failure_of(driver: &AcpDriver) -> studio_agent_spike::driver::AgentFailure {
    driver
        .wait_ready(WAIT)
        .err()
        .or_else(|| driver.failure())
        .expect("a failure")
}

fn read_json(root: &Path, name: &str) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join(name)).unwrap()).unwrap()
}

#[cfg(unix)]
fn pid_running(pid: i32) -> bool {
    // SAFETY: signal 0 only probes existence.
    let exists = unsafe { libc::kill(pid, 0) } == 0;
    exists
        && std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|stat| !stat.rsplit(") ").next().unwrap_or("").starts_with('Z'))
            .unwrap_or(false)
}

#[cfg(unix)]
fn kill_known_pid(pid: i32) {
    // SAFETY: only ever called with a pid this test read from its own fixture.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(unix)]
fn helper_pid(root: &Path) -> i32 {
    wait_for(|| {
        std::fs::read_to_string(root.join("helper.pid"))
            .ok()
            .and_then(|t| t.trim().parse().ok())
    })
}

// ---- negotiation, prompt and authoritative completion -------------------------------

#[test]
fn v1_is_negotiated_explicitly_and_a_turn_completes_authoritatively() {
    let root = tempfile::tempdir().unwrap();
    let (driver, manager) = start(root.path(), "write");
    let info = driver.wait_ready(WAIT).unwrap();
    assert_eq!(info.initialized.protocol_version, 1);
    assert!(
        !info.initialized.authenticated,
        "initialize never proves auth"
    );
    assert_eq!(info.session_id.as_deref(), Some("test-session"));
    assert!(!info.restored);
    assert_eq!(info.initialized.agent_name, "protocol-peer");

    let init = read_json(root.path(), "initialize.json");
    assert_eq!(init["protocolVersion"], 1);
    let capabilities = &init["clientCapabilities"];
    assert_ne!(capabilities["fs"]["readTextFile"], true);
    assert_ne!(capabilities["fs"]["writeTextFile"], true);
    assert_ne!(capabilities["terminal"], true);
    assert_eq!(init["clientInfo"]["name"], "fframes-studio");

    let turn = driver.prompt("Edit the title").unwrap();
    let outcome = driver.wait_turn(turn, WAIT).unwrap();
    assert_eq!(outcome.stop_reason, StopReasonKind::EndTurn);
    assert!(outcome.authoritative && outcome.eligible_for_quiescence());
    assert!(
        std::fs::read_to_string(root.path().join("main.rs"))
            .unwrap()
            .contains("agent edit")
    );
    assert_eq!(
        read_json(root.path(), "prompt.json")["sessionId"],
        "test-session"
    );

    let events = driver.drain_events(256);
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match &e.kind {
            AgentEventKind::Initialized(_) => "initialized",
            AgentEventKind::SessionReady { .. } => "session",
            AgentEventKind::MessageDelta { .. } => "delta",
            AgentEventKind::ToolCall(_) => "tool",
            AgentEventKind::ToolUpdate(_) => "tool-update",
            AgentEventKind::PromptFinished(_) => "finished",
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "initialized",
            "session",
            "delta",
            "tool",
            "tool-update",
            "finished"
        ]
    );
    assert!(events.windows(2).all(|w| w[0].sequence < w[1].sequence));
    assert!(
        events
            .iter()
            .all(|e| e.provider == "fixture" && e.task == "task-1")
    );
    assert_eq!(
        events[0].session, None,
        "session is unknown before creation"
    );
    assert!(
        events[2..]
            .iter()
            .all(|e| e.session.as_deref() == Some("test-session"))
    );
    assert!(matches!(
        &events[2].kind,
        AgentEventKind::MessageDelta { role: MessageRole::Agent, text } if text == "Hello world"
    ));
    let page = driver.transcript_page(0, 10);
    assert_eq!(page.entries[0].text, "Hello world");

    let outcome = driver.shutdown();
    assert!(outcome.termination.verified());
    assert_eq!(outcome.last_prompt.as_ref().map(|p| p.turn), Some(turn));
    assert!(outcome.failure.is_none());
    assert_eq!(manager.active_count(), 0);
}

#[test]
fn turns_reuse_one_session_and_reject_overlap() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "hang-prompt");
    ready(&driver);
    assert!(matches!(
        driver.prompt("  "),
        Err(DriverError::InvalidPrompt(_))
    ));
    driver.prompt("first").unwrap();
    assert!(matches!(
        driver.prompt("overlap"),
        Err(DriverError::TurnInProgress)
    ));
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "late");
    ready(&driver);
    let first = driver.prompt("one").unwrap();
    driver.wait_turn(first, WAIT).unwrap();
    let second = driver.prompt("two").unwrap();
    assert_eq!(second, first + 1);
    driver.wait_turn(second, WAIT).unwrap();
    assert_eq!(driver.session_id().as_deref(), Some("test-session"));
    drop(driver.shutdown());
}

#[test]
fn refusal_and_exhausted_limits_are_never_quiescence_eligible() {
    for (mode, expected) in [
        ("refuse", StopReasonKind::Refusal),
        ("max-tokens", StopReasonKind::MaxTokens),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (driver, _manager) = start(root.path(), mode);
        ready(&driver);
        let turn = driver.prompt("go").unwrap();
        let outcome = driver.wait_turn(turn, WAIT).unwrap();
        assert_eq!(outcome.stop_reason, expected);
        assert!(!outcome.eligible_for_quiescence());
        drop(driver.shutdown());
    }
}

#[test]
fn unsupported_version_and_unknown_stop_reason_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let (driver, manager) = start(root.path(), "version2");
    let failure = failure_of(&driver);
    assert_eq!(failure.kind, FailureKind::UnsupportedVersion);
    assert_eq!(failure.phase, Phase::Initialize);
    assert!(failure.message.contains("version 2"), "{failure:?}");
    let outcome = driver.shutdown();
    assert!(outcome.termination.verified());
    assert_eq!(manager.active_count(), 0);

    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "unknown-stop");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    let failure = driver.wait_turn(turn, WAIT).unwrap_err();
    assert_eq!(failure.kind, FailureKind::ProtocolViolation, "{failure:?}");
    assert!(
        driver.last_prompt().is_none(),
        "invalid stop reason never recorded"
    );
    drop(driver.shutdown());
}

// ---- framing failures ------------------------------------------------------------------

#[test]
fn malformed_truncated_and_oversized_messages_fail_the_driver_visibly() {
    for (mode, kind) in [
        ("garbage-init", FailureKind::MalformedMessage),
        ("truncated", FailureKind::TruncatedMessage),
        ("oversized", FailureKind::OversizedMessage),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (driver, manager) = start(root.path(), mode);
        let failure = failure_of(&driver);
        assert_eq!(failure.kind, kind, "{mode}: {failure:?}");
        assert_eq!(failure.phase, Phase::Initialize);
        let outcome = driver.shutdown();
        assert!(outcome.termination.verified(), "{mode}");
        assert_eq!(outcome.failure.map(|f| f.kind), Some(kind));
        assert_eq!(manager.active_count(), 0, "{mode}");
    }
}

#[test]
fn mid_turn_protocol_failures_stop_the_tree_and_surface_one_failure_event() {
    for (mode, kind) in [
        ("malformed-mid", FailureKind::MalformedMessage),
        ("truncated-mid", FailureKind::TruncatedMessage),
        ("oversized-mid", FailureKind::OversizedMessage),
        ("wrong-session", FailureKind::ProtocolViolation),
        ("crash-mid", FailureKind::ProcessExited),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (driver, manager) = start(root.path(), mode);
        ready(&driver);
        let turn = driver.prompt("go").unwrap();
        let failure = driver.wait_turn(turn, WAIT).unwrap_err();
        assert_eq!(failure.kind, kind, "{mode}: {failure:?}");
        assert_eq!(failure.phase, Phase::Prompt, "{mode}");
        if mode == "crash-mid" {
            assert_eq!(failure.exit_code, Some(7));
        }
        assert!(matches!(
            driver.prompt("again"),
            Err(DriverError::Failed(_)) | Err(DriverError::Closed)
        ));
        let outcome = driver.shutdown();
        assert!(outcome.termination.verified(), "{mode}");
        assert_eq!(manager.active_count(), 0, "{mode}");
    }
}

// ---- discovery and auth status -----------------------------------------------------------

fn probe(root: &Path, mode: &str, verify: bool, method: Option<&str>) -> AdapterStatus {
    let config = AdapterConfig {
        executable: "python3".into(),
        args: vec![peer_script(), mode.into(), root.to_string_lossy().into()],
        auth_env_names: vec![],
    };
    let mut options = ProbeOptions::new(root.to_owned());
    options.verify_session = verify;
    options.auth_method = method.map(str::to_owned);
    options.timeout = Duration::from_secs(10);
    let manager = ProcessTreeManager::new();
    let report = probe_adapter(&config, &search(), &options, &manager);
    assert_eq!(manager.active_count(), 0, "probe leaves no process behind");
    report.status
}

#[test]
fn discovery_reports_distinct_executable_runtime_protocol_and_auth_states() {
    let root = tempfile::tempdir().unwrap();
    assert!(matches!(
        probe(root.path(), "ok", true, None),
        AdapterStatus::Ready { ref agent, .. } if agent == "protocol-peer"
    ));
    assert!(matches!(
        probe(root.path(), "ok", false, None),
        AdapterStatus::AuthUnknown { .. }
    ));
    assert_eq!(
        probe(root.path(), "auth-required", true, None),
        AdapterStatus::AuthRequired {
            methods: vec!["key".into()]
        }
    );
    assert!(matches!(
        probe(root.path(), "auth-rejected", true, Some("key")),
        AdapterStatus::AuthRejected { .. }
    ));
    assert!(matches!(
        probe(root.path(), "auth-ok", true, Some("key")),
        AdapterStatus::Ready { .. }
    ));
    assert!(matches!(
        probe(root.path(), "version2", true, None),
        AdapterStatus::ProtocolMismatch { .. }
    ));
    assert!(matches!(
        probe(root.path(), "garbage-init", true, None),
        AdapterStatus::ProtocolMismatch { .. }
    ));
    assert!(matches!(
        probe(root.path(), "exit-early", true, None),
        AdapterStatus::Failed { ref failure }
            if failure.kind == FailureKind::ProcessExited && failure.exit_code == Some(3)
    ));
    // An unadvertised authentication method is rejected, never silently skipped.
    assert!(matches!(
        probe(root.path(), "ok", true, Some("key")),
        AdapterStatus::AuthRejected { .. }
    ));
}

#[test]
fn discovery_distinguishes_a_missing_executable_from_a_missing_runtime() {
    let root = tempfile::tempdir().unwrap();
    let config = AdapterConfig {
        executable: "no-such-adapter-binary".into(),
        args: vec![],
        auth_env_names: vec![],
    };
    let options = ProbeOptions::new(root.path().to_owned());
    let manager = ProcessTreeManager::new();
    let report = probe_adapter(&config, &search(), &options, &manager);
    assert!(matches!(
        report.status,
        AdapterStatus::MissingExecutable { .. }
    ));
    assert!(report.executable.is_none());
    assert!(matches!(
        AdapterLaunch::resolve(&config, &search()),
        Err(ResolveError::NotFound { .. })
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = root.path().join("adapter-needs-runtime");
        std::fs::write(&script, "#!/usr/bin/env definitely-missing-runtime-xyz\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = AdapterConfig {
            executable: script.to_string_lossy().into(),
            args: vec![],
            auth_env_names: vec![],
        };
        // A concurrent fork in a sibling test can briefly hold the freshly written
        // script open for writing (ETXTBSY); retry only that known test-side race.
        let mut report = probe_adapter(&config, &search(), &options, &manager);
        for _ in 0..40 {
            let busy = matches!(&report.status, AdapterStatus::Failed { failure }
                if failure.message.contains("Text file busy"));
            if !busy {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
            report = probe_adapter(&config, &search(), &options, &manager);
        }
        assert!(
            matches!(report.status, AdapterStatus::MissingRuntime { .. }),
            "{:?}",
            report.status
        );
        assert_eq!(report.executable.as_deref(), Some(script.as_path()));
    }
    assert_eq!(manager.active_count(), 0);
}

#[test]
fn authenticate_flow_requires_an_advertised_method_and_proves_auth_only_via_a_session() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "auth-ok");
    cfg.launch = cfg.launch.with_auth_method("key");
    let (driver, _manager) = start_with(cfg);
    let info = driver.wait_ready(WAIT).unwrap();
    assert_eq!(info.initialized.auth_methods, vec!["key".to_owned()]);
    assert!(
        read_json(root.path(), "calls.json")
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "authenticate")
    );
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "auth-required");
    let failure = failure_of(&driver);
    assert_eq!(failure.kind, FailureKind::AuthRequired);
    assert_eq!(failure.code, Some(-32000));
    assert_eq!(failure.phase, Phase::Session);
    drop(driver.shutdown());
}

// ---- permissions, cancel, stop, late events ----------------------------------------------------

#[test]
fn permission_replies_are_correlated_and_duplicates_or_late_replies_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "permission");
    ready(&driver);
    let turn = driver.prompt("edit").unwrap();
    let events = collect_until(&driver, |e| {
        matches!(e.kind, AgentEventKind::PermissionRequested(_))
    });
    let AgentEventKind::PermissionRequested(prompt) = &events.last().unwrap().kind else {
        unreachable!()
    };
    assert_eq!(prompt.turn, turn);
    assert_eq!(prompt.title, "Edit title");
    assert_eq!(prompt.options.len(), 2);
    let id = prompt.id;

    assert!(matches!(
        driver.reply_permission(id, PermissionReply::Select("nope".into())),
        Err(DriverError::InvalidOption(_))
    ));
    driver
        .reply_permission(id, PermissionReply::Select("allow".into()))
        .unwrap();
    assert!(matches!(
        driver.reply_permission(id, PermissionReply::Select("allow".into())),
        Err(DriverError::UnknownPermission(_))
    ));
    assert!(matches!(
        driver.reply_permission(
            studio_agent_spike::driver::PermissionId(999),
            PermissionReply::Cancel
        ),
        Err(DriverError::UnknownPermission(_))
    ));
    let outcome = driver.wait_turn(turn, WAIT).unwrap();
    assert!(outcome.eligible_for_quiescence());
    assert_eq!(
        read_json(root.path(), "permission-perm-1.json")["optionId"],
        "allow"
    );
    assert!(
        std::fs::read_to_string(root.path().join("main.rs"))
            .unwrap()
            .contains("Permitted edit")
    );
    drop(driver.shutdown());
}

#[test]
fn cancelling_a_turn_closes_permissions_and_is_never_eligible() {
    let root = tempfile::tempdir().unwrap();
    let (driver, manager) = start(root.path(), "permission");
    ready(&driver);
    let turn = driver.prompt("edit").unwrap();
    let events = collect_until(&driver, |e| {
        matches!(e.kind, AgentEventKind::PermissionRequested(_))
    });
    let AgentEventKind::PermissionRequested(prompt) = &events.last().unwrap().kind else {
        unreachable!()
    };
    let id = prompt.id;
    driver.cancel_prompt().unwrap();
    let outcome = driver.wait_turn(turn, WAIT).unwrap();
    assert_eq!(outcome.stop_reason, StopReasonKind::Cancelled);
    assert!(outcome.cancel_requested && !outcome.eligible_for_quiescence());
    let closed = collect_until(&driver, |e| {
        matches!(e.kind, AgentEventKind::PromptFinished(_))
    });
    assert!(closed.iter().any(|e| matches!(
        &e.kind,
        AgentEventKind::PermissionClosed { id: closed_id, resolution: PermissionResolution::Cancelled }
            if *closed_id == id
    )));
    assert!(matches!(
        driver.reply_permission(id, PermissionReply::Select("allow".into())),
        Err(DriverError::UnknownPermission(_))
    ));
    assert_eq!(
        read_json(root.path(), "permission-perm-1.json")["outcome"],
        "cancelled"
    );
    assert!(!root.path().join("main.rs").exists());
    assert!(matches!(
        driver.cancel_prompt(),
        Err(DriverError::NoActiveTurn)
    ));
    let outcome = driver.shutdown();
    assert!(outcome.termination.verified());
    assert_eq!(manager.active_count(), 0);
}

#[test]
fn stop_closes_pending_permissions_reaps_the_tree_and_rejects_later_replies() {
    let root = tempfile::tempdir().unwrap();
    let (driver, manager) = start(root.path(), "permission-two");
    ready(&driver);
    driver.prompt("edit").unwrap();
    let mut seen = 0;
    collect_until(&driver, |e| {
        if matches!(e.kind, AgentEventKind::PermissionRequested(_)) {
            seen += 1;
        }
        seen == 2
    });
    let start_stop = Instant::now();
    let outcome = driver.shutdown();
    assert!(start_stop.elapsed() < Duration::from_secs(8));
    assert!(outcome.termination.verified());
    assert_eq!(manager.active_count(), 0);
    assert!(
        outcome.last_prompt.is_none(),
        "stopped mid-turn: no completion"
    );
    assert!(!root.path().join("main.rs").exists());
}

#[test]
fn events_after_completion_are_rejected_counted_and_never_recorded() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "late");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    let outcome = driver.wait_turn(turn, WAIT).unwrap();
    assert!(outcome.eligible_for_quiescence());
    wait_for(|| (driver.late_events_rejected() >= 3).then_some(()));
    assert!(
        driver.failure().is_none(),
        "late traffic is not a protocol failure"
    );
    wait_for(|| root.path().join("late-perm.json").exists().then_some(()));
    assert_eq!(
        read_json(root.path(), "late-perm.json")["outcome"]["outcome"],
        "cancelled"
    );
    let text: String = driver
        .transcript_page(0, 100)
        .entries
        .iter()
        .map(|e| e.text.clone())
        .collect();
    assert!(text.contains("before") && !text.contains("late text"));
    let events = driver.drain_events(256);
    assert!(!events.iter().any(|e| matches!(
        &e.kind,
        AgentEventKind::ToolCall(t) if t.tool_call_id == "late"
    )));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, AgentEventKind::PermissionRequested(_)))
    );
    let outcome = driver.shutdown();
    assert!(outcome.late_events_rejected >= 3);
}

// ---- deadlines and blocked IO ---------------------------------------------------------------------

#[test]
fn deadlines_fail_visibly_and_reap_the_owned_tree() {
    let short = Duration::from_millis(400);
    type SetLimit = fn(&mut DriverLimits, Duration);
    let cases: [(&str, SetLimit, Phase); 3] = [
        ("hang-init", |l, d| l.init_timeout = d, Phase::Initialize),
        ("hang-prompt", |l, d| l.prompt_timeout = d, Phase::Prompt),
        (
            "hang-cancel",
            |l, d| l.cancel_ack_timeout = d,
            Phase::CancelAck,
        ),
    ];
    for (mode, set, phase) in cases {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = config(root.path(), mode);
        set(&mut cfg.limits, short);
        let (driver, manager) = start_with(cfg);
        let began = Instant::now();
        let failure = if mode == "hang-init" {
            driver.wait_ready(WAIT).unwrap_err()
        } else {
            ready(&driver);
            let turn = driver.prompt("go").unwrap();
            if mode == "hang-cancel" {
                std::thread::sleep(Duration::from_millis(100));
                driver.cancel_prompt().unwrap();
            }
            driver.wait_turn(turn, WAIT).unwrap_err()
        };
        assert_eq!(
            failure.kind,
            FailureKind::DeadlineExceeded,
            "{mode}: {failure:?}"
        );
        assert_eq!(failure.phase, phase, "{mode}");
        assert!(began.elapsed() < Duration::from_secs(10), "{mode}");
        let outcome = driver.shutdown();
        assert!(outcome.termination.verified(), "{mode}");
        assert_eq!(manager.active_count(), 0, "{mode}");
    }
}

#[test]
fn default_limits_match_the_stage_contract() {
    let limits = DriverLimits::default();
    assert_eq!(limits.max_message_bytes, 1024 * 1024);
    assert_eq!(limits.max_queued_events, 256);
    assert_eq!(limits.max_transcript_bytes, 4 * 1024 * 1024);
    assert_eq!(limits.max_stderr_bytes, 1024 * 1024);
    assert_eq!(limits.init_timeout, Duration::from_secs(30));
    assert_eq!(limits.prompt_timeout, Duration::from_secs(15 * 60));
    assert_eq!(limits.shutdown_grace, Duration::from_secs(2));
}

#[test]
fn a_non_reading_adapter_cannot_block_callers_and_stop_spam_coalesces_into_a_deadline_failure() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "stall-stdin");
    cfg.limits.cancel_ack_timeout = Duration::from_millis(600);
    let (driver, manager) = start_with(cfg);
    ready(&driver);
    driver.prompt(&"x".repeat(200 * 1024)).unwrap();
    let began = Instant::now();
    // The adapter has stopped reading. Stop is latched and coalesced at the call
    // boundary, so no number of calls can block or grow a queue.
    for _ in 0..5000 {
        let _ = driver.cancel_prompt();
    }
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "{:?}",
        began.elapsed()
    );
    let failure = wait_for(|| driver.failure());
    assert_eq!(failure.kind, FailureKind::DeadlineExceeded, "{failure:?}");
    assert_eq!(failure.phase, Phase::CancelAck);
    assert!(began.elapsed() < Duration::from_secs(10));
    let outcome = driver.shutdown();
    assert!(outcome.termination.verified());
    assert_eq!(manager.active_count(), 0);
}

// ---- bounded events and transcripts -----------------------------------------------------------------

#[test]
fn critical_event_overflow_fails_visibly_instead_of_dropping_events() {
    let root = tempfile::tempdir().unwrap();
    let (driver, manager) = start(root.path(), "flood-tools");
    ready(&driver);
    let turn = driver.prompt("flood").unwrap();
    // Nobody drains the queue: the 256-event bound must trip.
    let failure = driver.wait_turn(turn, WAIT).unwrap_err();
    assert_eq!(failure.kind, FailureKind::EventQueueOverflow, "{failure:?}");
    assert!(driver.queued_events() <= 256 + 4);
    let events = driver.drain_events(1000);
    assert!(events.iter().any(|e| matches!(
        &e.kind,
        AgentEventKind::Failure(f) if f.kind == FailureKind::EventQueueOverflow
    )));
    let outcome = driver.shutdown();
    assert!(outcome.termination.verified());
    assert_eq!(manager.active_count(), 0);
}

#[test]
fn message_deltas_batch_and_the_transcript_stays_bounded() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "flood-deltas");
    ready(&driver);
    let turn = driver.prompt("stream").unwrap();
    let outcome = driver.wait_turn(turn, Duration::from_secs(60)).unwrap();
    assert!(outcome.eligible_for_quiescence());
    assert!(
        driver.queued_events() <= 256,
        "deltas coalesce into few events"
    );
    assert!(driver.failure().is_none());
    let mut text = String::new();
    let mut cursor = Some(0);
    while let Some(from) = cursor {
        let page = driver.transcript_page(from, 64);
        text.extend(page.entries.iter().map(|e| e.text.as_str()));
        cursor = page.next;
    }
    assert!(text.starts_with("delta-0 delta-1 "));
    assert!(text.trim_end().ends_with("delta-19999"));
    assert!(driver.transcript_resident_bytes() <= 4 * 1024 * 1024 + 16 * 1024);
    drop(driver.shutdown());
}

// ---- redaction, declined services, restore, options ----------------------------------------------------

#[test]
fn configured_secret_values_never_reach_events_transcripts_stderr_or_failures() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "secret");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    driver.wait_turn(turn, WAIT).unwrap();
    wait_for(|| (!driver.stderr_tail().is_empty()).then_some(()));
    let events = driver.drain_events(256);
    let dump = format!(
        "{events:?} {:?} {}",
        driver.transcript_page(0, 100),
        driver.stderr_tail()
    );
    assert!(!dump.contains(SECRET), "secret leaked: {dump}");
    assert!(dump.contains("[REDACTED]"));
    let joined: String = driver
        .transcript_page(0, 100)
        .entries
        .iter()
        .map(|e| e.text.clone())
        .collect();
    assert!(joined.contains("The key is") && joined.contains("done"));
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "auth-rejected");
    cfg.launch = cfg.launch.with_auth_method("key");
    let (driver, _manager) = start_with(cfg);
    let failure = failure_of(&driver);
    assert_eq!(failure.kind, FailureKind::AuthRejected);
    assert!(!format!("{failure:?}").contains(SECRET));
    drop(driver.shutdown());
}

#[test]
fn optional_filesystem_and_terminal_services_are_declined() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "fs-request");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    driver.wait_turn(turn, WAIT).unwrap();
    for name in ["fs-1.json", "term-1.json"] {
        let response = read_json(root.path(), name);
        assert_eq!(response["error"]["code"], -32601, "{name}: {response}");
    }
    assert_eq!(driver.declined_requests(), 2);
    assert!(driver.failure().is_none());
    drop(driver.shutdown());
}

#[test]
fn restoration_requires_a_negotiated_capability() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "load");
    cfg.resume_session = Some("saved-session".into());
    let (driver, _manager) = start_with(cfg);
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(info.restored);
    assert_eq!(info.session_id.as_deref(), Some("saved-session"));
    assert_eq!(
        read_json(root.path(), "session-params.json")["method"],
        "session/load"
    );
    let text: Vec<String> = driver
        .transcript_page(0, 10)
        .entries
        .iter()
        .map(|e| format!("{:?}:{}", e.role, e.text))
        .collect();
    assert_eq!(text, ["User:replayed question", "Agent:replayed answer"]);
    assert!(
        !driver
            .drain_events(256)
            .iter()
            .any(|e| matches!(e.kind, AgentEventKind::MessageDelta { .. })),
        "replay goes to the transcript, not live events"
    );
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.resume_session = Some("saved-session".into());
    let (driver, _manager) = start_with(cfg);
    let failure = failure_of(&driver);
    assert_eq!(failure.kind, FailureKind::RestoreUnsupported);
    let calls = read_json(root.path(), "calls.json");
    assert!(
        !calls.as_array().unwrap().iter().any(|m| m == "session/new"),
        "no silent fresh session in place of a restore: {calls}"
    );
    drop(driver.shutdown());
}

#[test]
fn only_advertised_options_can_be_set_and_adapter_rejections_are_events() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "options");
    ready(&driver);
    let options = driver.options();
    assert_eq!(options.current_mode.as_deref(), Some("code"));
    assert_eq!(options.modes.len(), 2);
    assert_eq!(options.config[0].id, "model");

    assert!(matches!(
        driver.set_mode("nope"),
        Err(DriverError::InvalidOption(_))
    ));
    assert!(matches!(
        driver.set_config_option("unknown", OptionValue::Value("x".into())),
        Err(DriverError::InvalidOption(_))
    ));
    assert!(matches!(
        driver.set_config_option("model", OptionValue::Value("bogus".into())),
        Err(DriverError::InvalidOption(_))
    ));
    assert!(matches!(
        driver.set_config_option("model", OptionValue::Boolean(true)),
        Err(DriverError::InvalidOption(_))
    ));

    driver.set_mode("plan").unwrap();
    collect_until(
        &driver,
        |e| matches!(&e.kind, AgentEventKind::Options(o) if o.current_mode.as_deref() == Some("plan")),
    );
    assert_eq!(read_json(root.path(), "set-mode.json")["modeId"], "plan");
    driver
        .set_config_option("model", OptionValue::Value("deep".into()))
        .unwrap();
    collect_until(&driver, |e| {
        matches!(&e.kind, AgentEventKind::Options(o)
            if matches!(&o.config[0].kind, studio_agent_spike::driver::ConfigKind::Select { current, .. } if current == "deep"))
    });
    // Advertised but refused by the adapter: visible event, session unchanged.
    driver
        .set_config_option("model", OptionValue::Value("fast".into()))
        .unwrap();
    collect_until(
        &driver,
        |e| matches!(&e.kind, AgentEventKind::OptionRejected { option, .. } if option == "model"),
    );
    assert!(driver.failure().is_none());
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "ok");
    ready(&driver);
    assert!(matches!(
        driver.set_mode("code"),
        Err(DriverError::InvalidOption(_))
    ));
    drop(driver.shutdown());
}

// ---- process ownership -------------------------------------------------------------------------------

#[test]
#[cfg(unix)]
fn helpers_inside_the_task_group_are_reaped_and_group_cleanup_is_verified() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "spawn-helper");
    cfg.writer_ownership = WriterOwnership::ProcessGroupContained {
        qualification: "fixture".into(),
    };
    let (driver, manager) = start_with(cfg);
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    driver.wait_turn(turn, WAIT).unwrap();
    let helper = helper_pid(root.path());
    assert!(pid_running(helper));
    let outcome = driver.shutdown();
    assert!(outcome.termination.verified(), "{:?}", outcome.termination);
    assert!(outcome.escaped_pids.is_empty());
    assert!(outcome.ownership.is_qualified());
    assert!(!pid_running(helper), "owned helper survived group cleanup");
    assert_eq!(manager.active_count(), 0);
}

#[test]
#[cfg(unix)]
fn a_setsid_escaped_helper_demotes_ownership_and_is_not_covered_by_group_cleanup() {
    for mode in ["escape-helper", "escape-pipes"] {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = config(root.path(), mode);
        cfg.writer_ownership = WriterOwnership::ProcessGroupContained {
            qualification: "claimed".into(),
        };
        let (driver, manager) = start_with(cfg);
        ready(&driver);
        let turn = driver.prompt("go").unwrap();
        driver.wait_turn(turn, WAIT).unwrap();
        let helper = helper_pid(root.path());
        let stop = Instant::now();
        let outcome: DriverOutcome = driver.shutdown();
        let survived = pid_running(helper);
        // Clean up only the known, test-owned helper before asserting.
        kill_known_pid(helper);
        assert!(
            stop.elapsed() < Duration::from_secs(8),
            "{mode}: a helper holding the pipes must not wedge shutdown"
        );
        assert!(
            outcome.termination.verified(),
            "group cleanup is verifiable even though the helper escaped it: {mode}"
        );
        assert!(
            survived,
            "{mode}: escaped helper must outlive group cleanup"
        );
        assert_eq!(outcome.escaped_pids, vec![helper as u32], "{mode}");
        assert_eq!(
            outcome.ownership,
            WriterOwnership::Detached,
            "{mode}: qualification gate must fail"
        );
        assert!(!outcome.ownership.is_qualified());
        assert_eq!(manager.active_count(), 0);
    }
}

#[test]
fn dropping_the_driver_reaps_the_adapter() {
    let root = tempfile::tempdir().unwrap();
    let (driver, manager) = start(root.path(), "hang-prompt");
    ready(&driver);
    driver.prompt("go").unwrap();
    assert_eq!(manager.active_count(), 1);
    drop(driver);
    assert_eq!(manager.active_count(), 0);
}

#[test]
fn spawn_rejects_a_relative_or_missing_session_directory() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.cwd = PathBuf::from("relative/dir");
    let manager = ProcessTreeManager::new();
    let failure = AcpDriver::start(cfg, &manager).err().unwrap();
    assert_eq!(failure.kind, FailureKind::SpawnFailed);
    assert_eq!(manager.active_count(), 0);
}

fn project_mcp_server(root: &Path) -> McpStdioServer {
    McpStdioServer::new(
        "fframes-project",
        root.join("bin").join("studio-mcp"),
        vec!["--task".into(), "task-1".into()],
        vec![
            ("FFRAMES_MCP_TOKEN".into(), MCP_SECRET.into()),
            ("FFRAMES_TASK".into(), "task-1".into()),
        ],
    )
    .expect("valid server")
}

fn expected_wire(root: &Path) -> serde_json::Value {
    serde_json::json!([{
        "name": "fframes-project",
        "command": root.join("bin").join("studio-mcp"),
        "args": ["--task", "task-1"],
        "env": [
            {"name": "FFRAMES_MCP_TOKEN", "value": MCP_SECRET},
            {"name": "FFRAMES_TASK", "value": "task-1"},
        ],
    }])
}

#[test]
fn configured_stdio_servers_reach_session_new_and_restore_requests_verbatim() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    let (driver, _manager) = start_with(cfg);
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(info.initialized.capabilities.mcp_stdio);
    let params = read_json(root.path(), "session-params.json");
    assert_eq!(params["method"], "session/new");
    assert_eq!(params["mcpServers"], expected_wire(root.path()));
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "load");
    cfg.resume_session = Some("saved-session".into());
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    let (driver, _manager) = start_with(cfg);
    driver.wait_ready(WAIT).unwrap();
    let params = read_json(root.path(), "session-params.json");
    assert_eq!(params["method"], "session/load");
    assert_eq!(params["mcpServers"], expected_wire(root.path()));
    drop(driver.shutdown());
}

#[test]
fn unsupported_or_empty_stdio_configuration_sends_an_empty_list_and_reports_the_capability() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "ok");
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(info.initialized.capabilities.mcp_stdio);
    assert_eq!(
        read_json(root.path(), "session-params.json")["mcpServers"],
        serde_json::json!([])
    );
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    cfg.mcp_stdio = McpStdioSupport::Unsupported;
    let (driver, _manager) = start_with(cfg);
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(!info.initialized.capabilities.mcp_stdio);
    assert_eq!(
        read_json(root.path(), "session-params.json")["mcpServers"],
        serde_json::json!([])
    );
    let events = driver.drain_events(256);
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        AgentEventKind::Initialized(info) if !info.capabilities.mcp_stdio
    )));
    drop(driver.shutdown());
}

#[test]
fn mcp_env_values_never_reach_events_transcripts_stderr_failures_or_debug_output() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "mcp-secret");
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    let debug = format!("{cfg:?}");
    assert!(!debug.contains(MCP_SECRET), "config debug leaked: {debug}");
    assert!(debug.contains("FFRAMES_MCP_TOKEN"));
    let (driver, _manager) = start_with(cfg);
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    driver.wait_turn(turn, WAIT).unwrap();
    wait_for(|| (!driver.stderr_tail().is_empty()).then_some(()));
    let events = driver.drain_events(256);
    let dump = format!(
        "{events:?} {:?} {}",
        driver.transcript_page(0, 100),
        driver.stderr_tail()
    );
    assert!(!dump.contains(MCP_SECRET), "mcp secret leaked: {dump}");
    assert!(dump.contains("[REDACTED]"));
    let joined: String = driver
        .transcript_page(0, 100)
        .entries
        .iter()
        .map(|e| e.text.clone())
        .collect();
    assert!(joined.contains("The mcp key is") && joined.contains("done"));
    drop(driver.shutdown());

    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "mcp-reject");
    cfg.mcp_servers = vec![project_mcp_server(root.path())];
    let (driver, _manager) = start_with(cfg);
    let failure = failure_of(&driver);
    assert_eq!(failure.kind, FailureKind::SessionRejected);
    assert!(
        !format!("{failure:?} {failure}").contains(MCP_SECRET),
        "failure leaked: {failure:?}"
    );
    let dump = format!("{:?} {}", driver.drain_events(256), driver.stderr_tail());
    assert!(!dump.contains(MCP_SECRET), "events leaked: {dump}");
    drop(driver.shutdown());
}

#[test]
fn mcp_server_construction_rejects_relative_commands_and_unsafe_strings() {
    let abs = std::env::temp_dir().join("studio-mcp");
    let new = |name: &str, command: PathBuf, args: Vec<String>, env: Vec<(String, String)>| {
        McpStdioServer::new(name, command, args, env)
    };
    assert_eq!(
        new("srv", "relative/studio-mcp".into(), vec![], vec![]),
        Err(McpConfigError::RelativeCommand { name: "srv".into() })
    );
    assert_eq!(
        new("", abs.clone(), vec![], vec![]),
        Err(McpConfigError::EmptyName)
    );
    assert!(matches!(
        new("a\0b", abs.clone(), vec![], vec![]),
        Err(McpConfigError::Nul { .. })
    ));
    assert!(matches!(
        new("srv", abs.clone(), vec!["x\0".into()], vec![]),
        Err(McpConfigError::Nul { .. })
    ));
    assert!(matches!(
        new("srv", abs.clone(), vec![], vec![("K".into(), "v\0".into())]),
        Err(McpConfigError::Nul { .. })
    ));
    assert!(matches!(
        new("srv", abs.clone(), vec![], vec![("K=1".into(), "v".into())]),
        Err(McpConfigError::InvalidEnvName { .. })
    ));
    let error = new(
        "srv",
        abs.clone(),
        vec![],
        vec![("K\0".into(), MCP_SECRET.into())],
    )
    .unwrap_err();
    assert!(!error.to_string().contains(MCP_SECRET));

    // A server assembled through the public fields is re-validated before any spawn.
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "ok");
    cfg.mcp_servers = vec![McpStdioServer {
        name: "srv".into(),
        command: "relative/studio-mcp".into(),
        args: vec![],
        env: vec![("K".into(), MCP_SECRET.into())],
    }];
    let manager = ProcessTreeManager::new();
    let failure = AcpDriver::start(cfg, &manager).err().unwrap();
    assert_eq!(failure.kind, FailureKind::SpawnFailed);
    assert!(!format!("{failure:?}").contains(MCP_SECRET));
    assert_eq!(manager.active_count(), 0);
}
