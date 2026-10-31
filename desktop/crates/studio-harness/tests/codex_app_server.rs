//! Real-subprocess scenarios for the Codex app-server driver.
//!
//! The peer is `tests/fake-codex.py`, which speaks the message shapes recorded from
//! codex-cli 0.162.1. It proves framing, the handshake, approvals, turns, background
//! commands and stop only.

use std::{
    path::Path,
    time::{Duration, Instant},
};
use studio_bootstrap::ProcessTreeManager;
use studio_harness::{
    AdapterConfig,
    discovery::{AdapterLaunch, AdapterStatus, ExecutableSearch, ProbeOptions, probe_adapter_on},
    driver::{
        AgentDriver, AgentEvent, AgentEventKind, AgentWire, DriverConfig, DriverError,
        DriverLimits, DriverMode, FailureKind, McpHttpServer, McpStdioServer, McpStdioSupport,
        MessageRole, OwnershipSource, PermissionReply, PlanStatus, PromptImage, QuestionAnswer,
        StopReasonKind, UserMcpLaunch, start_driver,
    },
};

const WAIT: Duration = Duration::from_secs(20);
const MCP_SECRET: &str = "codex-mcp-sentinel-5d20";
/// Planted by the fake in `config/read` and `account/read`; never surfaced.
const USER_SECRET: &str = "codex-user-mcp-sentinel-77e1";
const EMAIL: &str = "person-sentinel@example.com";

fn python() -> String {
    studio_test_support::python()
        .to_str()
        .expect("the Python path is UTF-8")
        .to_owned()
}

fn launch(root: &Path, mode: &str) -> AdapterLaunch {
    let config = AdapterConfig {
        executable: python(),
        args: vec![
            format!("{}/tests/fake-codex.py", env!("CARGO_MANIFEST_DIR")),
            mode.into(),
            root.to_string_lossy().into(),
            "--model".into(),
            "gpt-test".into(),
            "--effort".into(),
            "low".into(),
        ],
        auth_env_names: vec![],
    };
    let search = ExecutableSearch {
        managed_dirs: vec![],
        gui_path: std::env::var_os("PATH"),
        node: None,
    };
    AdapterLaunch::resolve_with_env(&config, &search, |_| None).expect("Python resolves")
}

fn studio_server() -> McpStdioServer {
    McpStdioServer::new(
        "studio",
        std::env::current_exe().unwrap(),
        vec!["--serve".into()],
        vec![("STUDIO_TOKEN".into(), MCP_SECRET.into())],
    )
    .unwrap()
}

fn config(root: &Path, mode: &str) -> DriverConfig {
    DriverConfig {
        provider: "codex-fixture".into(),
        task: "task-1".into(),
        cwd: root.to_owned(),
        launch: launch(root, mode),
        limits: DriverLimits::default(),
        resume_session: None,
        ownership: OwnershipSource::Verified,
        mode: DriverMode::Full,
        mcp_servers: vec![studio_server()],
        user_mcp_servers: Vec::new(),
        mcp_stdio: McpStdioSupport::Baseline,
    }
}

fn start_with(config: DriverConfig) -> (Box<dyn AgentDriver>, ProcessTreeManager) {
    let manager = ProcessTreeManager::new();
    let driver =
        start_driver(AgentWire::CodexAppServer, config, &manager).expect("the fake CLI starts");
    driver.wait_ready(WAIT).expect("the session becomes ready");
    (driver, manager)
}

fn start(root: &Path, mode: &str) -> (Box<dyn AgentDriver>, ProcessTreeManager) {
    start_with(config(root, mode))
}

fn collect_until(
    driver: &dyn AgentDriver,
    mut done: impl FnMut(&AgentEvent) -> bool,
) -> Vec<AgentEvent> {
    let deadline = Instant::now() + WAIT;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        for event in driver.drain_events(64) {
            let stop = done(&event);
            events.push(event);
            if stop {
                return events;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("expected event never arrived; saw {events:#?}");
}

fn finished(event: &AgentEvent) -> Option<&studio_harness::driver::PromptOutcome> {
    match &event.kind {
        AgentEventKind::PromptFinished(outcome) => Some(outcome),
        _ => None,
    }
}

fn permission(events: &[AgentEvent]) -> &studio_harness::driver::PermissionPrompt {
    match &events.last().unwrap().kind {
        AgentEventKind::PermissionRequested(prompt) => prompt,
        other => panic!("not a permission request: {other:?}"),
    }
}

fn read_json(root: &Path, name: &str) -> serde_json::Value {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(bytes) = std::fs::read(root.join(name))
            && let Ok(value) = serde_json::from_slice(&bytes)
        {
            return value;
        }
        assert!(Instant::now() < deadline, "{name} was never written");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn agent_text(events: &[AgentEvent], role: MessageRole) -> String {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            AgentEventKind::MessageDelta { role: r, text } if *r == role => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_user_s_servers_run_beside_the_studio_s_with_http_headers_off_the_command_line() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "turns");
    let mut docs: UserMcpLaunch = McpHttpServer::new(
        "docs",
        "https://docs.test/mcp",
        vec![("Authorization".into(), "Bearer user-http-secret-88".into())],
    )
    .unwrap()
    .into();
    docs.disabled_tools = vec!["delete_page".into()];
    cfg.user_mcp_servers = vec![
        McpStdioServer::new(
            "github",
            std::env::current_exe().unwrap(),
            vec![],
            vec![("GITHUB_TOKEN".into(), "user-mcp-secret-77".into())],
        )
        .unwrap()
        .into(),
        docs,
    ];
    // The fake's config.toml has a stdio `docs`: sent under its own name, Studio's HTTP
    // `docs` would merge into it and Codex would refuse the thread.
    let (driver, _manager) = start_with(cfg);
    let ready = collect_until(&*driver, |event| {
        matches!(event.kind, AgentEventKind::SessionReady { .. })
    });
    let Some(AgentEventKind::SessionReady {
        user_mcp_servers, ..
    }) = ready.last().map(|event| &event.kind)
    else {
        unreachable!()
    };
    assert_eq!(user_mcp_servers, &["github", "docs"]);
    let launch = read_json(root.path(), "launch.json");
    assert_eq!(launch["args"], serde_json::json!(["app-server"]));
    let config = &read_json(root.path(), "thread.json")["params"]["config"];
    assert_eq!(config["mcp_servers.studio.enabled"], true);
    assert_eq!(config["mcp_servers.github.enabled"], true);
    assert_eq!(
        config["mcp_servers.github.env"]["GITHUB_TOKEN"],
        "user-mcp-secret-77"
    );
    assert_eq!(config["mcp_servers.docs.enabled"], false);
    assert!(config.get("mcp_servers.docs.url").is_none());
    assert_eq!(
        config["mcp_servers.studio-docs.url"],
        "https://docs.test/mcp"
    );
    assert_eq!(
        config["mcp_servers.studio-docs.http_headers"]["Authorization"],
        "Bearer user-http-secret-88"
    );
    assert_eq!(
        config["mcp_servers.studio-docs.disabled_tools"],
        serde_json::json!(["delete_page"])
    );
    assert_eq!(config["mcp_servers.rea.enabled"], false);
    assert!(!launch.to_string().contains("user-http-secret-88"));
    let ended = driver.shutdown();
    assert!(ended.failure.is_none(), "{:?}", ended.failure);
}

#[test]
fn a_thread_runs_in_the_draft_with_only_the_studio_server_and_user_answered_approvals() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    let launch = read_json(root.path(), "launch.json");
    assert_eq!(launch["args"], serde_json::json!(["app-server"]));
    let thread = read_json(root.path(), "thread.json");
    assert_eq!(thread["method"], "thread/start");
    let params = &thread["params"];
    assert_eq!(
        Path::new(params["cwd"].as_str().unwrap()),
        root.path(),
        "the thread runs in the draft"
    );
    assert_eq!(params["approvalPolicy"], "untrusted");
    assert_eq!(params["sandbox"], "workspace-write");
    assert_eq!(params["model"], "gpt-test");
    let config = &params["config"];
    assert_eq!(config["features.apps"], false);
    assert_eq!(
        config["mcp_servers.rea.enabled"], false,
        "the user's server is off"
    );
    assert_eq!(config["mcp_servers.studio.env"]["STUDIO_TOKEN"], MCP_SECRET);
    assert_eq!(
        Path::new(
            read_json(root.path(), "config-read.json")["cwd"]
                .as_str()
                .unwrap()
        ),
        root.path(),
        "project servers of the draft are read too"
    );
    let session = driver.wire_session_id().expect("the thread id");
    assert!(session.starts_with("thr-"));

    let turn = driver.prompt("say hi", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    let outcome = finished(events.last().unwrap()).unwrap();
    assert_eq!(outcome.turn, turn);
    assert!(outcome.eligible_for_quiescence());
    assert_eq!(agent_text(&events, MessageRole::Agent), "on it");
    assert_eq!(agent_text(&events, MessageRole::Thought), "thinking");
    let turn_params = read_json(root.path(), "turn.json");
    assert_eq!(turn_params["approvalPolicy"], "untrusted");
    assert_eq!(turn_params["effort"], "low");
    assert_eq!(turn_params["sandboxPolicy"]["type"], "workspaceWrite");
    assert_eq!(turn_params["sandboxPolicy"]["networkAccess"], false);
    assert_eq!(turn_params["sandboxPolicy"]["excludeSlashTmp"], true);

    let everything = format!("{events:?}");
    for secret in [USER_SECRET, EMAIL, MCP_SECRET] {
        assert!(!everything.contains(secret), "{secret} reached an event");
    }
    let ended = driver.shutdown();
    assert!(ended.failure.is_none(), "{:?}", ended.failure);
    assert!(ended.termination.group_empty);
    let ended = format!("{ended:?}");
    for secret in [USER_SECRET, EMAIL, MCP_SECRET] {
        assert!(!ended.contains(secret), "{secret} reached the outcome");
    }
}

#[test]
fn decline_blocks_a_command_and_accept_applies_a_file_change() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hello.txt"), "hello\n").unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("RUN it", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| {
        matches!(e.kind, AgentEventKind::PermissionRequested(_))
    });
    let prompt = permission(&events);
    assert_eq!(prompt.title, "Run: echo ran > ran.txt");
    assert_eq!(prompt.tool_call_id, "exec-1");
    let ids: Vec<&str> = prompt
        .options
        .iter()
        .map(|o| o.option_id.as_str())
        .collect();
    assert_eq!(ids, ["accept", "decline"]);
    let tool = events.iter().find_map(|e| match &e.kind {
        AgentEventKind::ToolCall(tool) => Some(tool),
        _ => None,
    });
    assert_eq!(tool.unwrap().kind.as_deref(), Some("execute"));
    driver
        .reply_permission(prompt.id, PermissionReply::Select("decline".into()))
        .unwrap();
    collect_until(&*driver, |e| finished(e).is_some());
    assert_eq!(
        read_json(root.path(), "decision.json"),
        serde_json::json!({"decision": "decline"})
    );
    assert!(!root.path().join("ran.txt").exists());

    std::fs::remove_file(root.path().join("decision.json")).unwrap();
    driver.prompt("EDIT it", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| {
        matches!(e.kind, AgentEventKind::PermissionRequested(_))
    });
    let prompt = permission(&events);
    assert!(
        prompt.title.starts_with("Edit: ") && prompt.title.ends_with("hello.txt"),
        "{}",
        prompt.title
    );
    driver
        .reply_permission(prompt.id, PermissionReply::Select("accept".into()))
        .unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert!(
        finished(events.last().unwrap())
            .unwrap()
            .eligible_for_quiescence()
    );
    assert_eq!(
        read_json(root.path(), "decision.json")["decision"],
        "accept"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("hello.txt")).unwrap(),
        "allowed\n"
    );
}

#[test]
fn an_mcp_tool_call_is_asked_as_claude_names_it_and_allowed_only_once() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "turns");
    // config.toml has a `docs`: Codex knows this one as `studio-docs`.
    cfg.user_mcp_servers = vec![
        McpHttpServer::new("docs", "https://docs.test/mcp", vec![])
            .unwrap()
            .into(),
    ];
    let (driver, _manager) = start_with(cfg);
    for (reply, expected) in [
        (
            "accept",
            serde_json::json!({"action": "accept", "content": {}}),
        ),
        (
            "decline",
            serde_json::json!({"action": "decline", "content": null}),
        ),
    ] {
        let _ = std::fs::remove_file(root.path().join("mcp-approval.json"));
        driver.prompt("MCPTOOL now", Vec::new()).unwrap();
        let events = collect_until(&*driver, |e| {
            matches!(e.kind, AgentEventKind::PermissionRequested(_))
        });
        // Studio's own server was answered without asking: once, nothing remembered.
        assert_eq!(
            read_json(root.path(), "studio-approval.json")["result"],
            serde_json::json!({"action": "accept", "content": {}})
        );
        let prompt = permission(&events);
        assert_eq!(prompt.title, "mcp__docs__search");
        assert_eq!(prompt.kind.as_deref(), Some("other"));
        let kinds: Vec<(&str, &str)> = prompt
            .options
            .iter()
            .map(|o| (o.option_id.as_str(), o.kind.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [("accept", "allow_once"), ("decline", "reject_once")]
        );
        driver
            .reply_permission(prompt.id, PermissionReply::Select(reply.into()))
            .unwrap();
        collect_until(&*driver, |e| finished(e).is_some());
        let answer = read_json(root.path(), "mcp-approval.json");
        assert_eq!(answer["result"], expected, "{answer}");
        assert!(answer["result"].get("_meta").is_none(), "never persisted");
    }
    let ended = driver.shutdown();
    assert!(ended.failure.is_none(), "{:?}", ended.failure);
}

#[test]
fn a_subagents_approval_is_asked_and_unsupported_requests_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("CHILD TOOLCALL", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| {
        matches!(e.kind, AgentEventKind::PermissionRequested(_))
    });
    let prompt = permission(&events);
    assert_eq!(prompt.title, "Run: ls");
    driver
        .reply_permission(prompt.id, PermissionReply::Select("accept".into()))
        .unwrap();
    collect_until(&*driver, |e| finished(e).is_some());
    assert_eq!(
        read_json(root.path(), "child-decision.json")["decision"],
        "accept"
    );
    let refused = read_json(root.path(), "toolcall.json");
    assert_eq!(refused["error"]["code"], -32601);
}

#[test]
fn a_question_is_answered_under_each_question_id() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("ASK me", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| matches!(e.kind, AgentEventKind::Question(_)));
    let AgentEventKind::Question(question) = &events.last().unwrap().kind else {
        unreachable!()
    };
    assert_eq!(question.questions.len(), 2);
    assert_eq!(question.questions[0].options[0].description, "warm");
    assert!(question.questions[1].options.is_empty());
    driver
        .answer_question(
            question.id,
            vec![
                QuestionAnswer {
                    selected: vec!["Blue".into()],
                    text: None,
                },
                QuestionAnswer {
                    selected: vec![],
                    text: Some("Ada".into()),
                },
            ],
        )
        .unwrap();
    collect_until(&*driver, |e| finished(e).is_some());
    assert_eq!(
        read_json(root.path(), "answers.json"),
        serde_json::json!({"answers": {"color": {"answers": ["Blue"]}, "name": {"answers": ["Ada"]}}})
    );
}

#[test]
fn stop_before_the_turn_is_announced_waits_for_it_and_keeps_the_server() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("SLOW work", Vec::new()).unwrap();
    // The fake announces the turn 0.3 s after accepting it.
    driver.cancel_turn().unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    let outcome = finished(events.last().unwrap()).unwrap();
    assert_eq!(outcome.stop_reason, StopReasonKind::Cancelled);
    assert!(outcome.cancel_requested);
    assert!(
        !root.path().join("early-interrupt.json").exists(),
        "the interrupt went out before Codex announced the turn"
    );
    assert!(matches!(
        driver.cancel_turn(),
        Err(DriverError::NoActiveTurn)
    ));
    driver.prompt("hi", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert!(
        finished(events.last().unwrap())
            .unwrap()
            .eligible_for_quiescence()
    );
}

#[test]
fn a_command_that_outlives_its_turn_is_background_work_reported_before_the_end() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("BACKGROUND", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    let background = events.iter().position(|e| {
        matches!(
            e.kind,
            AgentEventKind::Background {
                tasks: 1,
                continuing: false
            }
        )
    });
    assert!(
        background.is_some_and(|at| at + 1 < events.len()),
        "the running command is reported before the turn ends: {events:#?}"
    );
    assert!(
        finished(events.last().unwrap())
            .unwrap()
            .eligible_for_quiescence()
    );
    let rest = collect_until(&*driver, |e| {
        matches!(
            e.kind,
            AgentEventKind::Background {
                tasks: 0,
                continuing: false
            }
        )
    });
    assert!(rest.iter().all(|e| finished(e).is_none()), "no second end");
}

#[test]
fn a_steer_reaches_the_running_turn_and_a_refused_one_is_an_error() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    assert!(matches!(
        driver.steer("too early"),
        Err(DriverError::NoActiveTurn)
    ));
    let turn = driver.prompt("STEER me", Vec::new()).unwrap();
    collect_until(&*driver, |e| {
        matches!(e.kind, AgentEventKind::MessageDelta { .. })
    });
    driver.steer("also do this").unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert_eq!(finished(events.last().unwrap()).unwrap().turn, turn);
    assert!(agent_text(&events, MessageRole::Agent).ends_with("steered"));
    assert_eq!(steer_results(&events), [("also do this".to_owned(), true)]);
    let steer = read_json(root.path(), "steer.json");
    assert!(
        steer["expectedTurnId"]
            .as_str()
            .unwrap()
            .starts_with("turn-")
    );
    assert_eq!(steer["input"][0]["text"], "also do this");
    assert!(matches!(
        driver.steer("late"),
        Err(DriverError::NoActiveTurn)
    ));

    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "steer-reject");
    driver.prompt("STEER me", Vec::new()).unwrap();
    collect_until(&*driver, |e| {
        matches!(e.kind, AgentEventKind::MessageDelta { .. })
    });
    // The call only hands the message over; Codex's refusal arrives as an event.
    driver.steer("refused").unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert_eq!(steer_results(&events), [("refused".to_owned(), false)]);
}

/// `(text, accepted)` of every steer answer in `events`.
fn steer_results(events: &[AgentEvent]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|e| match &e.kind {
            AgentEventKind::SteerResult { text, accepted, .. } => Some((text.clone(), *accepted)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_parked_steer_never_blocks_the_caller_and_is_dropped_when_the_turn_does_not_start() {
    let root = tempfile::tempdir().unwrap();
    let mut cfg = config(root.path(), "turns");
    cfg.limits.request_timeout = Duration::from_millis(400);
    let (driver, _manager) = start_with(cfg);
    let turn = driver.prompt("NOSTART", Vec::new()).unwrap();
    let asked = Instant::now();
    driver.steer("too slow").unwrap();
    assert!(
        asked.elapsed() < Duration::from_millis(300),
        "steer waited for the agent: {:?}",
        asked.elapsed()
    );
    let events = collect_until(&*driver, |e| {
        matches!(e.kind, AgentEventKind::SteerResult { .. })
    });
    assert_eq!(steer_results(&events), [("too slow".to_owned(), false)]);
    // The turn starts afterwards: the dropped message is not delivered late.
    std::fs::write(root.path().join("release"), "go").unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert_eq!(finished(events.last().unwrap()).unwrap().turn, turn);
    assert!(steer_results(&events).is_empty());
    assert!(!root.path().join("steer.json").exists());
}

#[test]
fn a_steer_asked_before_the_turn_is_announced_is_delivered_once_it_is() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    let turn = driver.prompt("STEER me", Vec::new()).unwrap();
    // The fixture announces the turn a moment after it answers turn/start.
    driver.steer("right away").unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert_eq!(finished(events.last().unwrap()).unwrap().turn, turn);
    assert!(agent_text(&events, MessageRole::Agent).ends_with("steered"));
    assert_eq!(steer_results(&events), [("right away".to_owned(), true)]);
    assert_eq!(
        read_json(root.path(), "steer.json")["input"][0]["text"],
        "right away"
    );
}

#[test]
fn compact_and_review_run_as_turns_and_a_review_stops_by_its_announced_id() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    let turn = driver.prompt("/compact", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    let outcome = finished(events.last().unwrap()).unwrap();
    assert_eq!(outcome.turn, turn);
    assert!(outcome.eligible_for_quiescence());
    assert!(read_json(root.path(), "compact.json")["threadId"].is_string());

    driver.prompt("/review", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert!(
        finished(events.last().unwrap())
            .unwrap()
            .eligible_for_quiescence(),
        "the review ends under its reply's id"
    );
    assert_eq!(
        read_json(root.path(), "review.json")["target"],
        serde_json::json!({"type": "uncommittedChanges"})
    );

    // The interrupt is held until the review's turn is announced.
    driver.prompt("/review SLOW parts", Vec::new()).unwrap();
    driver.cancel_turn().unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert_eq!(
        finished(events.last().unwrap()).unwrap().stop_reason,
        StopReasonKind::Cancelled
    );
    assert!(
        !root.path().join("wrong-interrupt.json").exists(),
        "the interrupt named the announced turn"
    );
    assert_eq!(
        read_json(root.path(), "review.json")["target"]["instructions"],
        "SLOW parts"
    );

    driver.prompt("/review REFUSE", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert!(
        finished(events.last().unwrap())
            .unwrap()
            .reported_incomplete
    );
    assert!(
        driver.failure().is_none(),
        "a refused command keeps the session"
    );
    driver.prompt("hi", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert!(
        finished(events.last().unwrap())
            .unwrap()
            .eligible_for_quiescence()
    );
}

#[test]
fn plans_whole_messages_and_images_are_carried() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("PLAN QUIET", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    let plan = events
        .iter()
        .find_map(|e| match &e.kind {
            AgentEventKind::Plan(entries) => Some(entries.clone()),
            _ => None,
        })
        .expect("a plan");
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].status, PlanStatus::Completed);
    assert_eq!(plan[1].status, PlanStatus::InProgress);
    assert_eq!(agent_text(&events, MessageRole::Agent), "whole message");

    let budget = driver.prompt_image_budget("look", 1);
    assert_eq!(budget, 5 * 1024 * 1024);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([7u8; 64]);
    driver
        .prompt("look", vec![PromptImage::png(png.clone())])
        .unwrap();
    collect_until(&*driver, |e| finished(e).is_some());
    let images = read_json(root.path(), "images.json");
    let path = images[0].as_str().expect("one image path");
    assert!(path.ends_with(".png"));
    assert_eq!(std::fs::read(path).unwrap(), png);
}

#[test]
fn failed_turns_are_incomplete_and_a_rejected_login_fails_the_session() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("FAIL", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    let outcome = finished(events.last().unwrap()).unwrap();
    assert!(outcome.reported_incomplete);
    assert!(!outcome.eligible_for_quiescence());

    driver.prompt("UNAUTH", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| matches!(e.kind, AgentEventKind::Failure(_)));
    let AgentEventKind::Failure(failure) = &events.last().unwrap().kind else {
        unreachable!()
    };
    assert_eq!(failure.kind, FailureKind::AuthRequired);
}

#[test]
fn resume_continues_the_thread_or_starts_a_new_one_when_it_is_gone() {
    let root = tempfile::tempdir().unwrap();
    let mut kept = config(root.path(), "turns");
    kept.resume_session = Some("thr-kept".into());
    let (driver, _manager) = start_with(kept);
    assert_eq!(
        read_json(root.path(), "thread.json")["method"],
        "thread/resume"
    );
    assert_eq!(driver.wire_session_id().as_deref(), Some("thr-kept"));
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(info.restored);
    drop(driver);

    let root = tempfile::tempdir().unwrap();
    let mut gone = config(root.path(), "turns");
    gone.resume_session = Some("gone".into());
    let (driver, _manager) = start_with(gone);
    assert_eq!(
        read_json(root.path(), "thread.json")["method"],
        "thread/start"
    );
    let info = driver.wait_ready(WAIT).unwrap();
    assert!(!info.restored, "a new thread is not reported as restored");
}

#[test]
fn the_probe_reports_the_login_and_old_or_bypassing_launches_are_refused() {
    let manager = ProcessTreeManager::new();
    let search = ExecutableSearch {
        managed_dirs: vec![],
        gui_path: std::env::var_os("PATH"),
        node: None,
    };
    let probe = |mode: &str| {
        let root = tempfile::tempdir().unwrap();
        let adapter = AdapterConfig {
            executable: python(),
            args: launch(root.path(), mode).args,
            auth_env_names: vec![],
        };
        let report = probe_adapter_on(
            AgentWire::CodexAppServer,
            &adapter,
            &search,
            &ProbeOptions::new(root.path().to_owned()),
            &manager,
        );
        assert!(
            !root.path().join("thread.json").exists(),
            "a probe starts no thread"
        );
        report.status
    };
    assert!(
        matches!(probe("turns"), AdapterStatus::Ready { agent, version } if agent == "Codex" && version == "0.162.1")
    );
    assert!(matches!(
        probe("signed-out"),
        AdapterStatus::AuthRequired { .. }
    ));

    let root = tempfile::tempdir().unwrap();
    let Err(failure) = start_driver(
        AgentWire::CodexAppServer,
        config(root.path(), "old"),
        &manager,
    ) else {
        panic!("an old release must be refused");
    };
    assert_eq!(failure.kind, FailureKind::SpawnFailed);
    assert!(failure.message.contains("0.162.1"), "{}", failure.message);
    assert!(!root.path().join("launch.json").exists(), "nothing ran");

    let mut bypass = config(root.path(), "turns");
    bypass
        .launch
        .args
        .extend(["-c".into(), "approval_policy=never".into()]);
    let Err(failure) = start_driver(AgentWire::CodexAppServer, bypass, &manager) else {
        panic!("a config override must be refused");
    };
    assert!(failure.message.contains("-c"), "{}", failure.message);
}
