//! Adversarial regressions for the Stage 1 review (races, bounds, redaction, shutdown).
//!
//! Real-subprocess peer: `tests/acp-peer.py`. It proves transport and lifecycle only.

use parking_lot::Mutex;
use std::{
    io,
    path::Path,
    sync::{Arc, Barrier, LazyLock},
    time::{Duration, Instant},
};
use studio_agent_spike::{
    AdapterConfig,
    discovery::{AdapterLaunch, ExecutableSearch},
    driver::{
        AcpDriver, AgentEvent, AgentEventKind, ConfigKind, DriverConfig, DriverError, DriverLimits,
        DriverMode, FailureKind, McpStdioServer, McpStdioSupport, OptionValue, PermissionId,
        PermissionReply, Phase, StopReasonKind,
    },
};
use studio_bootstrap::{ProcessTreeManager, WriterOwnership};

const SECRET: &str = "sentinel-credential-7f3a9c";
const WAIT: Duration = Duration::from_secs(20);

fn search() -> ExecutableSearch {
    ExecutableSearch {
        managed_dirs: vec![],
        gui_path: std::env::var_os("PATH"),
    }
}

fn config(root: &Path, mode: &str) -> DriverConfig {
    let adapter = AdapterConfig {
        executable: "python3".into(),
        args: vec![
            format!("{}/tests/acp-peer.py", env!("CARGO_MANIFEST_DIR")),
            mode.into(),
            root.to_string_lossy().into(),
        ],
        auth_env_names: vec!["ACP_SECRET".into()],
    };
    let launch = AdapterLaunch::resolve_with_env(&adapter, &search(), |name| {
        (name == "ACP_SECRET").then(|| SECRET.to_owned())
    })
    .expect("python3 resolves through the explicit search path");
    DriverConfig {
        provider: "fixture".into(),
        task: "task-1".into(),
        cwd: root.to_owned(),
        launch,
        limits: DriverLimits::default(),
        resume_session: None,
        writer_ownership: WriterOwnership::Unknown,
        mode: DriverMode::Full,
        mcp_servers: vec![],
        mcp_stdio: McpStdioSupport::Baseline,
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

fn wait_for<T>(mut poll: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(value) = poll() {
            return value;
        }
        assert!(Instant::now() < deadline, "condition never became true");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn collect_until(driver: &AcpDriver, mut done: impl FnMut(&AgentEvent) -> bool) -> Vec<AgentEvent> {
    let deadline = Instant::now() + WAIT;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        if let Some(event) = driver.next_event(Duration::from_millis(50)) {
            let stop = done(&event);
            events.push(event);
            if stop {
                return events;
            }
        }
    }
    panic!("expected event never arrived; saw {events:#?}");
}

fn read_json(root: &Path, name: &str) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join(name)).unwrap()).unwrap()
}

fn exists(root: &Path, name: &str) -> bool {
    root.join(name).exists()
}

fn calls(root: &Path) -> Vec<String> {
    std::fs::read(root.join("calls.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
        .unwrap_or_default()
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

// ---- H5: Stop is latched at the call boundary -------------------------------------------------

#[test]
fn stop_wins_over_an_end_turn_already_in_flight() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "race-end-turn");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    wait_for(|| exists(root.path(), "prompt.json").then_some(()));
    // The adapter holds a ready end_turn. Stop returns before it is released.
    driver.cancel_prompt().unwrap();
    for _ in 0..100 {
        // Repeats coalesce: one notification, never retargeting another turn.
        let _ = driver.cancel_prompt();
    }
    std::fs::write(root.path().join("go"), "1").unwrap();
    let outcome = driver.wait_turn(turn, WAIT).unwrap();
    assert_eq!(outcome.stop_reason, StopReasonKind::EndTurn);
    assert!(outcome.cancel_requested, "{outcome:?}");
    assert!(!outcome.eligible_for_quiescence());
    wait_for(|| {
        calls(root.path())
            .iter()
            .any(|c| c == "session/cancel")
            .then_some(())
    });
    assert_eq!(
        calls(root.path())
            .iter()
            .filter(|c| *c == "session/cancel")
            .count(),
        1,
        "coalesced into one notification"
    );
    assert!(matches!(
        driver.cancel_prompt(),
        Err(DriverError::NoActiveTurn)
    ));
    drop(driver.shutdown());
}

#[test]
fn a_permission_arriving_after_stop_is_cancelled_never_offered() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "perm-after-cancel");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    wait_for(|| exists(root.path(), "prompt.json").then_some(()));
    driver.cancel_prompt().unwrap();
    std::fs::write(root.path().join("go"), "1").unwrap();
    let outcome = driver.wait_turn(turn, WAIT).unwrap();
    assert_eq!(outcome.stop_reason, StopReasonKind::Cancelled);
    assert!(outcome.cancel_requested && !outcome.eligible_for_quiescence());
    // The peer answers the cancel notification (and so the prompt) as soon as it reads it,
    // which can be before it has read and recorded the driver's reply to the permission
    // request; the file appears (and is complete) shortly after the turn has ended.
    let reply = wait_for(|| {
        std::fs::read(root.path().join("permission-perm-1.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
    });
    assert_eq!(reply["outcome"], "cancelled");
    assert!(driver.late_events_rejected() >= 1);
    let events = driver.drain_events(256);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e.kind, AgentEventKind::PermissionRequested(_))),
        "no actionable permission may exist for a stopped turn: {events:#?}"
    );
    assert!(matches!(
        driver.reply_permission(PermissionId(0), PermissionReply::Select("allow".into())),
        Err(DriverError::UnknownPermission(_))
    ));
    drop(driver.shutdown());
}

// ---- H6: grant vs Stop vs completion are totally ordered ----------------------------------------

#[test]
fn a_grant_racing_stop_is_either_applied_before_it_or_cancelled_never_both() {
    for round in 0..30 {
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
        let id = prompt.id;
        let driver = Arc::new(driver);
        let barrier = Arc::new(Barrier::new(2));
        let reply = {
            let (driver, barrier) = (driver.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                driver.reply_permission(id, PermissionReply::Select("allow".into()))
            })
        };
        barrier.wait();
        let stop = driver.cancel_prompt();
        let reply = reply.join().unwrap();
        let outcome = driver.wait_turn(turn, WAIT).unwrap();
        wait_for(|| exists(root.path(), "permission-perm-1.json").then_some(()));
        let peer = read_json(root.path(), "permission-perm-1.json");
        let granted = peer["outcome"] == "selected";
        assert_eq!(
            reply.is_ok(),
            granted,
            "round {round}: the host reply result must match what the adapter saw ({reply:?} vs {peer})"
        );
        if !granted {
            assert_eq!(peer["outcome"], "cancelled");
            assert!(matches!(reply, Err(DriverError::UnknownPermission(_))));
        }
        if stop.is_ok() {
            assert!(outcome.cancel_requested, "round {round}: {outcome:?}");
            assert!(!outcome.eligible_for_quiescence(), "round {round}");
        }
        if granted && stop.is_ok() {
            // A grant that landed before Stop cannot make the stopped turn eligible.
            assert!(!outcome.eligible_for_quiescence());
        }
        let driver = Arc::try_unwrap(driver).ok().expect("no other owner");
        drop(driver.shutdown());
    }
}

// ---- H7: failure dominates completion --------------------------------------------------------------

#[test]
fn a_completion_that_cannot_be_queued_is_never_published_as_success() {
    // Capacity 4: Initialized, SessionReady, two tool calls fill it exactly, so the
    // next event, PromptFinished, is the one that overflows.
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "tools-n");
    cfg.limits.max_queued_events = 4;
    let (driver, manager) = start_with(cfg);
    ready(&driver);
    let turn = driver.prompt("2").unwrap();
    let failure = driver.wait_turn(turn, WAIT).unwrap_err();
    assert_eq!(failure.kind, FailureKind::EventQueueOverflow, "{failure:?}");
    assert!(driver.last_prompt().is_none(), "no success after failure");
    assert!(matches!(
        driver.wait_turn(turn, Duration::from_millis(50)),
        Err(f) if f.kind == FailureKind::EventQueueOverflow
    ));
    let outcome = driver.shutdown();
    assert!(outcome.last_prompt.is_none());
    assert_eq!(
        outcome.failure.map(|f| f.kind),
        Some(FailureKind::EventQueueOverflow)
    );
    assert!(outcome.termination.verified());
    assert_eq!(manager.active_count(), 0);

    // Control: one more slot and the same turn completes.
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "tools-n");
    cfg.limits.max_queued_events = 5;
    let (driver, _manager) = start_with(cfg);
    ready(&driver);
    let turn = driver.prompt("2").unwrap();
    assert!(
        driver
            .wait_turn(turn, WAIT)
            .unwrap()
            .eligible_for_quiescence()
    );
    drop(driver.shutdown());
}

// ---- H8: wedged connection falls back to a bounded, correct shutdown ---------------------------------

#[test]
#[cfg(unix)]
fn shutdown_of_a_wedged_connection_uses_the_bounded_fallback_and_demotes_ownership() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "wedge-init");
    cfg.writer_ownership = WriterOwnership::ProcessGroupContained {
        qualification: "claimed".into(),
    };
    cfg.limits.shutdown_grace = Duration::from_millis(200);
    let (driver, manager) = start_with(cfg);
    let helper: i32 = wait_for(|| {
        std::fs::read_to_string(root.path().join("helper.pid"))
            .ok()
            .and_then(|t| t.trim().parse().ok())
    });
    // The escaped helper holds the adapter's stdout, and initialize is never answered:
    // the connection can never unwind by itself.
    let began = Instant::now();
    let outcome = driver.shutdown();
    let took = began.elapsed();
    let survived = pid_running(helper);
    // Clean up only the known, test-owned helper before asserting.
    // SAFETY: helper is the pid the fixture wrote for its own child.
    unsafe {
        libc::kill(helper, libc::SIGKILL);
    }
    assert!(survived, "the escaped helper outlives group cleanup");
    assert!(
        took >= Duration::from_millis(1200),
        "the normal path would have finished at once; this must be the fallback: {took:?}"
    );
    assert!(
        took < Duration::from_secs(8),
        "fallback is bounded: {took:?}"
    );
    assert!(outcome.termination.verified(), "{:?}", outcome.termination);
    assert_eq!(outcome.escaped_pids, vec![helper as u32]);
    assert_eq!(
        outcome.ownership,
        WriterOwnership::Detached,
        "the fallback applies the same sticky demotion as the normal path"
    );
    assert_eq!(manager.active_count(), 0);
}

// ---- M1: duplicate wire ids --------------------------------------------------------------------------

#[test]
fn two_in_flight_permission_requests_sharing_one_wire_id_are_a_protocol_violation() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "dup-perm-id");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    let failure = driver.wait_turn(turn, WAIT).unwrap_err();
    assert_eq!(failure.kind, FailureKind::ProtocolViolation, "{failure:?}");
    assert!(failure.message.contains("duplicate"), "{failure:?}");
    let events = driver.drain_events(256);
    let prompts: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.kind {
            AgentEventKind::PermissionRequested(p) => Some(p.title.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        prompts,
        vec!["First"],
        "the ambiguous second prompt is never surfaced"
    );
    assert!(driver.shutdown().termination.verified());
}

// ---- H11: bounded admission, requests and ingress -----------------------------------------------------

#[test]
fn unanswered_option_requests_are_bounded_and_expire_visibly() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "options-silent");
    cfg.limits.request_timeout = Duration::from_millis(500);
    let (driver, manager) = start_with(cfg);
    ready(&driver);
    for _ in 0..4 {
        driver
            .set_config_option("model", OptionValue::Value("deep".into()))
            .unwrap();
    }
    assert_eq!(driver.outstanding_requests(), 4);
    assert!(matches!(
        driver.set_config_option("model", OptionValue::Value("deep".into())),
        Err(DriverError::Busy(_))
    ));
    assert!(matches!(driver.set_mode("plan"), Err(DriverError::Busy(_))));
    let failure = wait_for(|| driver.failure());
    assert_eq!(failure.kind, FailureKind::DeadlineExceeded, "{failure:?}");
    assert!(failure.message.contains("mode/config"), "{failure:?}");
    let outcome = driver.shutdown();
    assert!(outcome.termination.verified());
    assert_eq!(manager.active_count(), 0);
}

#[test]
fn answered_requests_return_their_ingress_credit_and_sessions_never_stall() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "options");
    ready(&driver);
    for round in 0..60 {
        let (value, expect_event): (&str, fn(&AgentEvent) -> bool) = if round % 2 == 0 {
            ("deep", |e| matches!(&e.kind, AgentEventKind::Options(_)))
        } else {
            ("fast", |e| {
                matches!(&e.kind, AgentEventKind::OptionRejected { .. })
            })
        };
        driver
            .set_config_option("model", OptionValue::Value(value.into()))
            .unwrap();
        collect_until(&driver, expect_event);
    }
    wait_for(|| (driver.ingress_in_flight() == 0).then_some(()));
    assert_eq!(driver.outstanding_requests(), 0);
    assert!(driver.failure().is_none(), "{:?}", driver.failure());
    drop(driver.shutdown());
}

#[test]
fn a_sustained_notification_flood_never_exceeds_the_ingress_window() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "flood-plans");
    ready(&driver);
    let turn = driver.prompt("flood").unwrap();
    let mut peak = 0;
    let outcome = loop {
        peak = peak.max(driver.ingress_in_flight());
        match driver.wait_turn(turn, Duration::from_millis(1)) {
            Ok(outcome) => break outcome,
            Err(failure) if failure.kind == FailureKind::DeadlineExceeded => {
                assert!(driver.failure().is_none(), "{:?}", driver.failure());
            }
            Err(failure) => panic!("{failure:?}"),
        }
    };
    assert!(outcome.eligible_for_quiescence());
    assert!(peak <= 32, "ingress window exceeded: {peak}");
    assert!(driver.failure().is_none());
    wait_for(|| (driver.ingress_in_flight() == 0).then_some(()));
    drop(driver.shutdown());
}

#[test]
fn a_non_reading_adapter_flooding_requests_fails_the_driver_instead_of_blocking() {
    let root = tempfile::tempdir().unwrap();
    let (driver, manager) = start(root.path(), "flood-requests");
    ready(&driver);
    driver.prompt("go").unwrap();
    let began = Instant::now();
    let failure = wait_for(|| driver.failure());
    assert_eq!(failure.kind, FailureKind::WriteBlocked, "{failure:?}");
    assert!(began.elapsed() < Duration::from_secs(10));
    assert!(driver.shutdown().termination.verified());
    assert_eq!(manager.active_count(), 0);
}

// ---- M2: bounds on events, handle calls never run cleanup -----------------------------------------------

#[test]
fn one_near_limit_delta_is_split_into_capped_events() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "big-delta");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    assert!(
        driver
            .wait_turn(turn, WAIT)
            .unwrap()
            .eligible_for_quiescence()
    );
    let events = driver.drain_events(256);
    let lengths: Vec<usize> = events
        .iter()
        .filter_map(|e| match &e.kind {
            AgentEventKind::MessageDelta { text, .. } => Some(text.len()),
            _ => None,
        })
        .collect();
    assert!(lengths.iter().all(|len| *len <= 16 * 1024), "{lengths:?}");
    assert!(lengths.len() > 40, "{} events", lengths.len());
    assert!(lengths.iter().sum::<usize>() >= 899_000);
    drop(driver.shutdown());
}

#[test]
fn a_permission_reply_on_a_full_queue_returns_promptly_and_fails_asynchronously() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "perm-full");
    cfg.limits.max_queued_events = 4;
    let (driver, manager) = start_with(cfg);
    ready(&driver);
    driver.prompt("go").unwrap();
    // Initialized, SessionReady, ToolCall and PermissionRequested fill the queue exactly.
    wait_for(|| (driver.queued_events() >= 4).then_some(()));
    assert!(driver.failure().is_none());
    let began = Instant::now();
    let result = driver.reply_permission(PermissionId(0), PermissionReply::Select("allow".into()));
    assert!(
        began.elapsed() < Duration::from_millis(250),
        "handle calls never wait on process cleanup: {:?}",
        began.elapsed()
    );
    // The grant itself was delivered; the overflow it caused is a latched failure.
    assert!(result.is_ok(), "{result:?}");
    let failure = wait_for(|| driver.failure());
    assert_eq!(failure.kind, FailureKind::EventQueueOverflow);
    let outcome = driver.shutdown();
    assert!(outcome.termination.verified());
    assert_eq!(manager.active_count(), 0);
}

// ---- H12/H13: redaction -----------------------------------------------------------------------------------

#[test]
fn secrets_in_wire_metadata_ids_and_options_never_reach_any_exported_surface() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "meta-secret");
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(!format!("{info:?}").contains(SECRET), "{info:?}");
    assert!(!info.initialized.agent_name.contains(SECRET));
    assert!(driver.session_id().is_some_and(|s| !s.contains(SECRET)));
    assert_eq!(
        driver.wire_session_id(),
        None,
        "a session id that carries a secret is never exposed for persistence"
    );

    // Options: exported ids are distinct, redacted, and map back to the wire ids.
    let options = driver.options();
    assert!(!format!("{options:?}").contains(SECRET), "{options:?}");
    assert_eq!(options.modes.len(), 2);
    assert_ne!(options.modes[0].id, options.modes[1].id);
    driver.set_mode(&options.modes[1].id).unwrap();
    wait_for(|| exists(root.path(), "set-mode.json").then_some(()));
    assert_eq!(
        read_json(root.path(), "set-mode.json")["modeId"],
        format!("mode2-{SECRET}"),
        "the wire id, not the redacted export, reaches the adapter"
    );
    let config_option = &options.config[0];
    let ConfigKind::Select { values, .. } = &config_option.kind else {
        panic!("select expected")
    };
    driver
        .set_config_option(
            &config_option.id,
            OptionValue::Value(values[1].value.clone()),
        )
        .unwrap();
    wait_for(|| exists(root.path(), "set-config.json").then_some(()));
    let sent = read_json(root.path(), "set-config.json");
    assert_eq!(sent["configId"], format!("cfg-{SECRET}"));
    assert_eq!(sent["value"], format!("val2-{SECRET}"));

    // Permission: tool id and option ids are exported redacted but selectable.
    let turn = driver.prompt("go").unwrap();
    let events = collect_until(&driver, |e| {
        matches!(e.kind, AgentEventKind::PermissionRequested(_))
    });
    let AgentEventKind::PermissionRequested(prompt) = &events.last().unwrap().kind else {
        unreachable!()
    };
    assert_ne!(prompt.options[0].option_id, prompt.options[1].option_id);
    driver
        .reply_permission(
            prompt.id,
            PermissionReply::Select(prompt.options[0].option_id.clone()),
        )
        .unwrap();
    let outcome = driver.wait_turn(turn, WAIT).unwrap();
    assert!(outcome.eligible_for_quiescence());
    assert_eq!(
        read_json(root.path(), "permission-perm-1.json")["optionId"],
        format!("allow-{SECRET}")
    );
    let mut dump = format!("{events:?} {:?} {outcome:?}", driver.options());
    dump.push_str(&format!("{:?}", driver.drain_events(256)));
    dump.push_str(&format!("{:?}", driver.transcript_page(0, 100)));
    dump.push_str(&driver.stderr_tail());
    assert!(!dump.contains(SECRET), "secret leaked: {dump}");
    let closed = driver.shutdown();
    assert!(!format!("{closed:?}").contains(SECRET), "{closed:?}");
}

#[test]
fn stderr_is_sanitized_incrementally_so_polling_between_fragments_never_sees_a_prefix() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "stderr-fragments");
    ready(&driver);
    let turn = driver.prompt("go").unwrap();
    let half = SECRET.len() / 2;
    let (first, second) = (&SECRET[..half], &SECRET[half..]);
    wait_for(|| exists(root.path(), "frag1").then_some(()));
    std::thread::sleep(Duration::from_millis(100));
    let poll1 = driver.stderr_tail();
    assert!(
        !poll1.contains(first),
        "partial secret exposed between fragments: {poll1:?}"
    );
    std::fs::write(root.path().join("ack1"), "1").unwrap();
    wait_for(|| exists(root.path(), "frag2").then_some(()));
    std::thread::sleep(Duration::from_millis(100));
    let poll2 = driver.stderr_tail();
    assert!(!poll2.contains(SECRET), "{poll2:?}");
    assert!(
        !poll2.contains(first) && !poll2.contains(second),
        "{poll2:?}"
    );
    std::fs::write(root.path().join("ack2"), "1").unwrap();
    assert!(
        driver
            .wait_turn(turn, WAIT)
            .unwrap()
            .eligible_for_quiescence()
    );
    let outcome = driver.shutdown();
    drop(outcome);
}

// ---- H12: SDK diagnostics boundary -----------------------------------------------------------------------------

/// Installed on first use as this binary's only global subscriber; records every event.
static CAPTURED: LazyLock<Arc<Mutex<Vec<String>>>> = LazyLock::new(|| {
    let sink = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::set_global_default(Capture(sink.clone()))
        .expect("this test binary installs the only global subscriber");
    sink
});

struct Capture(Arc<Mutex<Vec<String>>>);

struct Collect<'a>(&'a mut String);

impl tracing::field::Visit for Collect<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = String::new();
        event.record(&mut Collect(&mut line));
        self.0.lock().push(line);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

fn capture() -> Arc<Mutex<Vec<String>>> {
    CAPTURED.clone()
}

struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.0 {
            std::task::Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    }
}

/// Drives the raw SDK over an in-memory line transport with no application guard and
/// returns what a host subscriber saw while one secret-bearing line was written.
fn unguarded_sdk_trace(secret: &str) -> Vec<String> {
    use agent_client_protocol::{
        Agent, Client, ConnectionTo, Lines, schema::v1::CancelNotification,
    };
    let sink_lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = sink_lines.clone();
    let sink = futures::sink::unfold(seen, async |seen, line: String| {
        seen.lock().push(line);
        Ok::<_, io::Error>(seen)
    });
    let stream = futures::stream::pending::<io::Result<String>>();
    let observed = sink_lines.clone();
    let secret_session = format!("sess-{secret}");
    let result = futures::executor::block_on(Client.builder().connect_with(
        Lines::new(Box::pin(sink), Box::pin(stream)),
        async move |cx: ConnectionTo<Agent>| {
            cx.send_notification(CancelNotification::new(secret_session))?;
            // Let the outgoing transport actor write and trace the line.
            for _ in 0..500 {
                YieldOnce(false).await;
                if !observed.lock().is_empty() {
                    break;
                }
            }
            for _ in 0..500 {
                YieldOnce(false).await;
            }
            Ok(())
        },
    ));
    result.expect("sdk connection");
    assert!(
        sink_lines.lock().iter().any(|l| l.contains(secret)),
        "the line really carried the secret"
    );
    capture().lock().clone()
}

#[test]
fn sdk_wire_diagnostics_never_reach_a_host_subscriber_from_a_driver_connection() {
    let captured = capture();
    // Control: the same SDK on an unguarded thread DOES log the secret-bearing line, so
    // the capture below is meaningful.
    let control_secret = "control-secret-0b6e41";
    let control = unguarded_sdk_trace(control_secret);
    assert!(
        control.iter().any(|line| line.contains(control_secret)),
        "control run must prove that SDK traces carry raw wire lines: {control:?}"
    );

    let outbound_secret = "mcp-outbound-secret-5d2c88";
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "secret-whole");
    cfg.mcp_servers = vec![
        McpStdioServer::new(
            "srv",
            "/bin/true",
            vec![],
            vec![("TOKEN".into(), outbound_secret.into())],
        )
        .unwrap(),
    ];
    let (driver, _manager) = start_with(cfg);
    ready(&driver);
    let turn = driver.prompt(&format!("prompt carries {SECRET}")).unwrap();
    driver.wait_turn(turn, WAIT).unwrap();
    drop(driver.shutdown());

    let lines = captured.lock().clone();
    for secret in [SECRET, outbound_secret] {
        assert!(
            !lines.iter().any(|line| line.contains(secret)),
            "SDK diagnostics leaked {secret}: {:?}",
            lines
                .iter()
                .filter(|l| l.contains(secret))
                .collect::<Vec<_>>()
        );
    }
    // The inbound text carrying the secret was delivered to the host redacted.
    assert!(
        std::fs::read_to_string(root.path().join("session-params.json"))
            .unwrap()
            .contains(outbound_secret),
        "the adapter really received the outbound secret"
    );
}

// ---- misc contracts --------------------------------------------------------------------------------------------------

#[test]
fn host_visible_phases_for_unanswered_requests_are_stable() {
    // Failure from an expired option request is attributed to the idle phase.
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "options-silent");
    cfg.limits.request_timeout = Duration::from_millis(200);
    let (driver, _manager) = start_with(cfg);
    ready(&driver);
    driver.set_mode("plan").unwrap();
    let failure = wait_for(|| driver.failure());
    assert_eq!(failure.phase, Phase::Idle);
    drop(driver.shutdown());
}
