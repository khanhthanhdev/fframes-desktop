//! Real-subprocess scenarios for the Claude Code stream-json driver.
//!
//! The peer is `tests/fake-claude.py`, which speaks the frame shapes recorded from Claude
//! Code 2.1.296. It proves framing, turns, permissions, background work and stop only.

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
        StopReasonKind, TurnOrigin, UserMcpLaunch, start_driver,
    },
};

const WAIT: Duration = Duration::from_secs(20);
const MCP_SECRET: &str = "claude-mcp-sentinel-41c9";

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
            format!("{}/tests/fake-claude.py", env!("CARGO_MANIFEST_DIR")),
            mode.into(),
            root.to_string_lossy().into(),
            "--model".into(),
            "haiku".into(),
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
        provider: "claude-fixture".into(),
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

fn start(root: &Path, mode: &str) -> (Box<dyn AgentDriver>, ProcessTreeManager) {
    let manager = ProcessTreeManager::new();
    let driver = start_driver(AgentWire::ClaudeStreamJson, config(root, mode), &manager)
        .expect("the fake CLI starts");
    driver.wait_ready(WAIT).expect("the session becomes ready");
    (driver, manager)
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
fn a_new_session_has_our_id_and_loads_only_the_studio_settings_and_server() {
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
    let manager = ProcessTreeManager::new();
    let driver = start_driver(AgentWire::ClaudeStreamJson, cfg, &manager).unwrap();
    driver.wait_ready(WAIT).unwrap();
    let session = driver.wire_session_id().expect("a session id");
    let launch = read_json(root.path(), "launch.json");
    let args: Vec<String> = serde_json::from_value(launch["args"].clone()).unwrap();
    let joined = args.join(" ");
    for expected in [
        format!("--session-id {session}"),
        "--setting-sources project,local".to_owned(),
        "--strict-mcp-config".to_owned(),
        "--permission-prompt-tool stdio".to_owned(),
        "--permission-mode default".to_owned(),
    ] {
        assert!(joined.contains(&expected), "{expected}: {joined}");
    }
    assert!(joined.ends_with("--model haiku"), "{joined}");
    assert!(
        !joined.contains(MCP_SECRET),
        "secrets never go on the command line"
    );
    let mcp = &launch["--mcp-config"]["mcpServers"]["studio"];
    assert_eq!(mcp["env"]["STUDIO_TOKEN"], MCP_SECRET);
    // The user's own server launches too, but only the studio's tools skip the question.
    let github = &launch["--mcp-config"]["mcpServers"]["github"];
    assert_eq!(github["env"]["GITHUB_TOKEN"], "user-mcp-secret-77");
    assert_eq!(
        launch["--settings"]["permissions"]["allow"],
        serde_json::json!(["mcp__studio"])
    );
    assert!(!joined.contains("user-mcp-secret-77"));
    // An HTTP server: its header values come from the private file, never the command
    // line, and the tool the user turned off is denied.
    assert_eq!(
        launch["--mcp-config"]["mcpServers"]["docs"],
        serde_json::json!({
            "type": "http",
            "url": "https://docs.test/mcp",
            "headers": {"Authorization": "Bearer user-http-secret-88"},
        })
    );
    assert!(!joined.contains("user-http-secret-88"));
    assert_eq!(
        launch["--settings"]["permissions"]["deny"],
        serde_json::json!(["mcp__docs__delete_page"])
    );
    let mcp_path = args[args.iter().position(|a| a == "--mcp-config").unwrap() + 1].clone();
    assert!(Path::new(&mcp_path).exists());

    let turn = driver.prompt("say hi", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    let outcome = finished(events.last().unwrap()).unwrap();
    assert_eq!(outcome.turn, turn);
    assert!(outcome.eligible_for_quiescence());
    assert_eq!(outcome.origin, TurnOrigin::Prompted);
    assert_eq!(agent_text(&events, MessageRole::Agent), "on it");
    assert_eq!(
        agent_text(&events, MessageRole::Thought),
        "thinking about it"
    );

    let ended = driver.shutdown();
    assert!(ended.failure.is_none(), "{:?}", ended.failure);
    assert!(ended.termination.group_empty);
    assert!(
        !Path::new(&mcp_path).exists(),
        "the private launch files are removed"
    );
}

#[test]
fn deny_blocks_the_edit_and_allow_applies_it() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("hello.txt"), "hello\n").unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    for (choice, expected) in [("deny", "hello\n"), ("allow", "allowed\n")] {
        let _ = std::fs::remove_file(root.path().join("permission.json"));
        driver.prompt("EDIT the file", Vec::new()).unwrap();
        let events = collect_until(&*driver, |e| {
            matches!(e.kind, AgentEventKind::PermissionRequested(_))
        });
        let AgentEventKind::PermissionRequested(prompt) = &events.last().unwrap().kind else {
            unreachable!()
        };
        assert_eq!(prompt.tool_call_id, "toolu_edit");
        assert_eq!(prompt.title, "Edit: fixture");
        let tool = events.iter().find_map(|e| match &e.kind {
            AgentEventKind::ToolCall(tool) => Some(tool),
            _ => None,
        });
        assert_eq!(tool.unwrap().kind.as_deref(), Some("edit"));
        driver
            .reply_permission(prompt.id, PermissionReply::Select(choice.into()))
            .unwrap();
        let events = collect_until(&*driver, |e| finished(e).is_some());
        assert!(
            finished(events.last().unwrap())
                .unwrap()
                .eligible_for_quiescence()
        );
        assert_eq!(
            read_json(root.path(), "permission.json")["behavior"],
            choice
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("hello.txt")).unwrap(),
            expected
        );
    }
}

#[test]
fn a_question_is_answered_with_every_answer_inside_the_original_input() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("ASK me", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| matches!(e.kind, AgentEventKind::Question(_)));
    let AgentEventKind::Question(question) = &events.last().unwrap().kind else {
        unreachable!()
    };
    assert_eq!(question.questions.len(), 2);
    assert_eq!(question.questions[0].options[0].description, "warm");
    assert!(question.questions[1].multi_select);
    driver
        .answer_question(
            question.id,
            vec![
                QuestionAnswer {
                    selected: vec!["Blue".into()],
                    text: None,
                },
                QuestionAnswer {
                    selected: vec!["S".into(), "M".into()],
                    text: None,
                },
            ],
        )
        .unwrap();
    collect_until(&*driver, |e| finished(e).is_some());
    let answer = read_json(root.path(), "question.json");
    assert_eq!(answer["behavior"], "allow");
    assert_eq!(answer["updatedInput"]["answers"]["Which color?"], "Blue");
    assert_eq!(
        answer["updatedInput"]["answers"]["Which sizes?"],
        serde_json::json!(["S", "M"])
    );
    assert_eq!(
        answer["updatedInput"]["questions"][0]["header"], "Color",
        "the original input is kept"
    );
}

#[test]
fn background_work_is_reported_and_the_agents_own_turn_ends_on_its_own() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    let first = driver.prompt("BACKGROUND GATED", Vec::new()).unwrap();
    let mut seen = collect_until(&*driver, |e| finished(e).is_some());
    let outcome = finished(seen.last().unwrap()).unwrap();
    assert_eq!(outcome.turn, first);
    assert!(
        seen.iter().any(|e| matches!(
            e.kind,
            AgentEventKind::Background {
                tasks: 1,
                continuing: false
            }
        )),
        "the running task is reported before the turn ends"
    );
    // Nothing new can start while background work is live.
    assert!(matches!(
        driver.prompt("again", Vec::new()),
        Err(DriverError::Busy(_)) | Err(DriverError::TurnInProgress)
    ));
    let events = collect_until(&*driver, |e| {
        matches!(e.kind, AgentEventKind::PermissionRequested(_))
    });
    let started = events.iter().find_map(|e| match e.kind {
        AgentEventKind::TurnStarted { turn, origin } => Some((turn, origin)),
        _ => None,
    });
    let (autonomous, origin) = started.expect("the agent's own turn is announced");
    assert_eq!(origin, TurnOrigin::Autonomous);
    assert!(autonomous > first);
    let AgentEventKind::PermissionRequested(prompt) = &events.last().unwrap().kind else {
        unreachable!()
    };
    assert_eq!(
        prompt.turn, autonomous,
        "a gated call in the agent's own turn asks"
    );
    driver
        .reply_permission(prompt.id, PermissionReply::Select("allow".into()))
        .unwrap();
    let rest = collect_until(&*driver, |e| {
        matches!(
            e.kind,
            AgentEventKind::Background {
                tasks: 0,
                continuing: false
            }
        )
    });
    let outcome = rest.iter().find_map(finished).expect("the own turn ends");
    assert_eq!(outcome.turn, autonomous);
    assert_eq!(outcome.origin, TurnOrigin::Autonomous);
    assert!(outcome.eligible_for_quiescence());
    seen.extend(events);
    seen.extend(rest);
    let finishes = seen.iter().filter(|e| finished(e).is_some()).count();
    assert_eq!(finishes, 2, "one end per turn");
    // Quiet again: the next prompt is accepted.
    driver.prompt("hi", Vec::new()).unwrap();
    collect_until(&*driver, |e| finished(e).is_some());
}

#[test]
fn stop_interrupts_the_turn_and_keeps_the_process_for_the_next_prompt() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("SLOW work", Vec::new()).unwrap();
    collect_until(&*driver, |e| {
        matches!(e.kind, AgentEventKind::MessageDelta { .. })
    });
    driver.cancel_turn().unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    let outcome = finished(events.last().unwrap()).unwrap();
    assert_eq!(outcome.stop_reason, StopReasonKind::Cancelled);
    assert!(outcome.cancel_requested);
    let log = read_json(root.path(), "log.json");
    assert!(log.as_array().unwrap().iter().any(|frame| {
        frame["request"]["subtype"] == "interrupt" && frame["request"]["cancel_queued"] == true
    }));
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
fn a_steer_taken_up_after_the_reported_end_continues_the_same_turn() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    let turn = driver.prompt("STEER me", Vec::new()).unwrap();
    collect_until(&*driver, |e| {
        matches!(e.kind, AgentEventKind::MessageDelta { .. })
    });
    driver.steer("also do this").unwrap();
    let events = collect_until(&*driver, |e| finished(e).is_some());
    assert_eq!(finished(events.last().unwrap()).unwrap().turn, turn);
    assert!(agent_text(&events, MessageRole::Agent).ends_with("steered"));
    assert!(events.iter().any(|e| matches!(
        &e.kind,
        AgentEventKind::SteerResult { text, accepted: true, .. } if text == "also do this"
    )));
    let steer = read_json(root.path(), "steer.json");
    assert_eq!(steer["priority"], "now");
    assert_eq!(steer["message"]["content"], "also do this");
    // Nothing else ends: the held first end was superseded.
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        driver
            .drain_events(64)
            .iter()
            .all(|e| finished(e).is_none())
    );
    assert!(matches!(
        driver.steer("late"),
        Err(DriverError::NoActiveTurn)
    ));
}

#[test]
fn todo_lists_become_plans_and_subagent_text_stays_out_of_the_transcript() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("TODO list", Vec::new()).unwrap();
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
    assert_eq!(plan[1].text, "Second");
    assert!(!agent_text(&events, MessageRole::Agent).contains("subagent"));
    assert!(
        !events.iter().any(
            |e| matches!(&e.kind, AgentEventKind::ToolCall(t) if t.tool_call_id == "toolu_todo")
        ),
        "a plan is not a tool call"
    );
}

#[test]
fn images_go_before_the_text_and_the_budget_bounds_them() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    let budget = driver.prompt_image_budget("look", 1);
    assert!(budget > 0 && budget <= 5 * 1024 * 1024, "{budget}");
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend([0u8; 64]);
    driver.prompt("look", vec![PromptImage::png(png)]).unwrap();
    collect_until(&*driver, |e| finished(e).is_some());
    assert_eq!(
        read_json(root.path(), "images.json"),
        serde_json::json!(["image/png"])
    );
}

#[test]
fn a_signed_out_cli_fails_as_auth_required() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "turns");
    driver.prompt("NOLOGIN", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| matches!(e.kind, AgentEventKind::Failure(_)));
    let AgentEventKind::Failure(failure) = &events.last().unwrap().kind else {
        unreachable!()
    };
    assert_eq!(failure.kind, FailureKind::AuthRequired);
}

#[test]
fn another_session_id_is_a_protocol_violation() {
    let root = tempfile::tempdir().unwrap();
    let (driver, _manager) = start(root.path(), "wrong-session");
    driver.prompt("hi", Vec::new()).unwrap();
    let events = collect_until(&*driver, |e| matches!(e.kind, AgentEventKind::Failure(_)));
    let AgentEventKind::Failure(failure) = &events.last().unwrap().kind else {
        unreachable!()
    };
    assert_eq!(failure.kind, FailureKind::ProtocolViolation);
}

#[test]
fn an_old_cli_is_refused_before_it_runs_and_the_probe_says_why() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessTreeManager::new();
    let Err(failure) = start_driver(
        AgentWire::ClaudeStreamJson,
        config(root.path(), "old"),
        &manager,
    ) else {
        panic!("an old release must be refused");
    };
    assert_eq!(failure.kind, FailureKind::SpawnFailed);
    assert!(failure.message.contains("2.1.296"), "{}", failure.message);
    assert!(!root.path().join("launch.json").exists(), "nothing ran");

    let adapter = AdapterConfig {
        executable: python(),
        args: launch(root.path(), "turns").args,
        auth_env_names: vec![],
    };
    let search = ExecutableSearch {
        managed_dirs: vec![],
        gui_path: std::env::var_os("PATH"),
        node: None,
    };
    let scratch = tempfile::tempdir().unwrap();
    let report = probe_adapter_on(
        AgentWire::ClaudeStreamJson,
        &adapter,
        &search,
        &ProbeOptions::new(scratch.path().to_owned()),
        &manager,
    );
    assert!(
        matches!(&report.status, AdapterStatus::Ready { agent, version } if agent == "Claude Code" && version == "2.1.296"),
        "{:?}",
        report.status
    );
}

#[test]
fn arguments_that_change_permissions_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let manager = ProcessTreeManager::new();
    let mut config = config(root.path(), "turns");
    config
        .launch
        .args
        .push("--dangerously-skip-permissions".into());
    let Err(failure) = start_driver(AgentWire::ClaudeStreamJson, config, &manager) else {
        panic!("a permission bypass must be refused");
    };
    assert!(failure.message.contains("--dangerously-skip-permissions"));
}
