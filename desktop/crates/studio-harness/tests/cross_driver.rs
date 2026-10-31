//! The same scenarios against every wire, through `start_driver` and the `AgentDriver`
//! trait only: ACP (`acp-peer.py`), Claude Code stream-json (`fake-claude.py`) and Codex
//! app-server (`fake-codex.py`).
//!
//! Each fake picks its behaviour differently (the ACP peer per process mode, the native
//! fakes per keyword in the prompt), so a [`Fixture`] maps every scenario to a mode and a
//! prompt. What a wire cannot do is asserted as such, through its capabilities.

use std::{
    path::Path,
    time::{Duration, Instant},
};
use studio_bootstrap::ProcessTreeManager;
use studio_harness::{
    AdapterConfig,
    discovery::{AdapterLaunch, ExecutableSearch},
    driver::{
        AgentDriver, AgentEvent, AgentEventKind, AgentWire, DriverConfig, DriverError,
        DriverLimits, DriverMode, McpStdioSupport, MessageRole, OwnershipSource, PermissionPrompt,
        PermissionReply, PromptImage, PromptOutcome, QuestionAnswer, SteeringMode, StopReasonKind,
        start_driver,
    },
};

const WAIT: Duration = Duration::from_secs(20);
/// A pause long enough for a fake to act on a reply it should not have acted on.
const SETTLE: Duration = Duration::from_millis(300);
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// One wire and how its fake is driven: `(mode, prompt)` per scenario.
struct Fixture {
    wire: AgentWire,
    script: &'static str,
    text: (&'static str, &'static str),
    edit: (&'static str, &'static str),
    /// A turn that runs until it is stopped.
    slow: (&'static str, &'static str),
    /// A turn that publishes a plan.
    plan: Option<(&'static str, &'static str)>,
    image: (&'static str, &'static str),
    /// The mode in which the fake can continue a saved session.
    resume: &'static str,
    /// The keyword prompt of a question, for wires that ask.
    ask: Option<&'static str>,
    /// The keyword prompt of a turn that waits for a steer.
    steer: Option<&'static str>,
    /// The keyword prompt of a turn that leaves background work.
    background: Option<&'static str>,
    /// Whether the fake applied the edit it asked permission for.
    edited: fn(&Path) -> bool,
}

fn native_edited(root: &Path) -> bool {
    std::fs::read_to_string(root.join("hello.txt")).is_ok_and(|text| text == "allowed\n")
}

const ACP: Fixture = Fixture {
    wire: AgentWire::Acp,
    script: "acp-peer.py",
    text: ("basic", "hello"),
    edit: ("permission", "edit"),
    slow: ("hang-prompt", "wait"),
    plan: Some(("plan", "go")),
    image: ("image-ok", "look"),
    resume: "load",
    ask: None,
    steer: None,
    background: None,
    edited: |root| root.join("main.rs").exists(),
};

const CLAUDE: Fixture = Fixture {
    wire: AgentWire::ClaudeStreamJson,
    script: "fake-claude.py",
    text: ("turns", "hello"),
    edit: ("turns", "EDIT it"),
    slow: ("turns", "SLOW work"),
    plan: Some(("turns", "TODO")),
    image: ("turns", "look"),
    resume: "turns",
    ask: Some("ASK me"),
    steer: Some("STEER me"),
    background: Some("BACKGROUND"),
    edited: native_edited,
};

const CODEX: Fixture = Fixture {
    wire: AgentWire::CodexAppServer,
    script: "fake-codex.py",
    text: ("turns", "hello"),
    edit: ("turns", "EDIT it"),
    slow: ("turns", "SLOW work"),
    plan: Some(("turns", "PLAN")),
    image: ("turns", "look"),
    resume: "turns",
    ask: Some("ASK me"),
    steer: Some("STEER me"),
    background: Some("BACKGROUND"),
    edited: native_edited,
};

const WIRES: [&Fixture; 3] = [&ACP, &CLAUDE, &CODEX];

fn python() -> String {
    studio_test_support::python()
        .to_str()
        .expect("the Python path is UTF-8")
        .to_owned()
}

impl Fixture {
    fn config(&self, root: &Path, mode: &str) -> DriverConfig {
        let adapter = AdapterConfig {
            executable: python(),
            args: vec![
                format!("{}/tests/{}", env!("CARGO_MANIFEST_DIR"), self.script),
                mode.into(),
                root.to_string_lossy().into(),
            ],
            auth_env_names: vec![],
        };
        let search = ExecutableSearch {
            managed_dirs: vec![],
            gui_path: std::env::var_os("PATH"),
            node: None,
        };
        DriverConfig {
            provider: format!("{}-fixture", self.wire.as_str()),
            task: "task-1".into(),
            cwd: root.to_owned(),
            launch: AdapterLaunch::resolve_with_env(&adapter, &search, |_| None)
                .expect("Python resolves"),
            limits: DriverLimits::default(),
            resume_session: None,
            ownership: OwnershipSource::Verified,
            mode: DriverMode::Full,
            mcp_servers: vec![],
            user_mcp_servers: Vec::new(),
            mcp_stdio: McpStdioSupport::Baseline,
        }
    }

    fn start_with(&self, config: DriverConfig) -> (Box<dyn AgentDriver>, ProcessTreeManager) {
        let manager = ProcessTreeManager::new();
        let driver = start_driver(self.wire, config, &manager)
            .unwrap_or_else(|failure| panic!("{}: {failure:?}", self.wire.as_str()));
        driver
            .wait_ready(WAIT)
            .unwrap_or_else(|failure| panic!("{}: {failure:?}", self.wire.as_str()));
        (driver, manager)
    }

    fn start(&self, root: &Path, mode: &str) -> (Box<dyn AgentDriver>, ProcessTreeManager) {
        std::fs::write(root.join("hello.txt"), "hello\n").unwrap();
        self.start_with(self.config(root, mode))
    }
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

fn finished(event: &AgentEvent) -> Option<&PromptOutcome> {
    match &event.kind {
        AgentEventKind::PromptFinished(outcome) => Some(outcome),
        _ => None,
    }
}

/// Events until the turn's end, which must be the only end of `turn`.
fn turn_events(driver: &dyn AgentDriver, turn: u64) -> (Vec<AgentEvent>, PromptOutcome) {
    let events = collect_until(driver, |e| finished(e).is_some());
    let outcome = finished(events.last().unwrap()).unwrap().clone();
    assert_eq!(outcome.turn, turn);
    std::thread::sleep(SETTLE);
    let after = driver.drain_events(256);
    assert!(
        after.iter().all(|e| finished(e).is_none()),
        "{}: a second end of the turn: {after:#?}",
        driver.wire().as_str()
    );
    (events, outcome)
}

fn permission(driver: &dyn AgentDriver) -> PermissionPrompt {
    let events = collect_until(driver, |e| {
        matches!(e.kind, AgentEventKind::PermissionRequested(_))
    });
    match events.into_iter().last().unwrap().kind {
        AgentEventKind::PermissionRequested(prompt) => prompt,
        _ => unreachable!(),
    }
}

/// The option of the first kind starting with `prefix` (`allow` or `reject`).
fn option(prompt: &PermissionPrompt, prefix: &str) -> PermissionReply {
    let choice = prompt
        .options
        .iter()
        .find(|o| o.kind.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix} option in {:?}", prompt.options));
    PermissionReply::Select(choice.option_id.clone())
}

fn text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            AgentEventKind::MessageDelta {
                role: MessageRole::Agent,
                text,
            } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn text_streams_and_each_turn_ends_exactly_once() {
    for fixture in WIRES {
        let root = tempfile::tempdir().unwrap();
        let (driver, _manager) = fixture.start(root.path(), fixture.text.0);
        for _ in 0..2 {
            let turn = driver.prompt(fixture.text.1, Vec::new()).unwrap();
            let (events, outcome) = turn_events(&*driver, turn);
            assert!(
                !text(&events).is_empty(),
                "{}: no text",
                fixture.wire.as_str()
            );
            assert_eq!(outcome.stop_reason, StopReasonKind::EndTurn);
            assert!(outcome.eligible_for_quiescence(), "{outcome:?}");
        }
        let ended = driver.shutdown();
        assert!(ended.failure.is_none(), "{:?}", ended.failure);
        assert!(ended.termination.group_empty);
    }
}

#[test]
fn a_denied_permission_blocks_the_tool_and_an_allowed_one_runs_it() {
    for fixture in WIRES {
        let name = fixture.wire.as_str();
        let root = tempfile::tempdir().unwrap();
        let (driver, _manager) = fixture.start(root.path(), fixture.edit.0);
        let turn = driver.prompt(fixture.edit.1, Vec::new()).unwrap();
        let prompt = permission(&*driver);
        assert_eq!(prompt.turn, turn);
        driver
            .reply_permission(prompt.id, option(&prompt, "reject"))
            .unwrap();
        turn_events(&*driver, turn);
        assert!(!(fixture.edited)(root.path()), "{name}: a denied tool ran");

        let turn = driver.prompt(fixture.edit.1, Vec::new()).unwrap();
        let prompt = permission(&*driver);
        driver
            .reply_permission(prompt.id, option(&prompt, "allow"))
            .unwrap();
        let (_, outcome) = turn_events(&*driver, turn);
        assert!(outcome.eligible_for_quiescence(), "{name}: {outcome:?}");
        assert!(
            (fixture.edited)(root.path()),
            "{name}: the allowed tool did not run"
        );
    }
}

#[test]
fn an_unanswered_permission_blocks_the_tool_until_stop_closes_it() {
    for fixture in WIRES {
        let name = fixture.wire.as_str();
        let root = tempfile::tempdir().unwrap();
        let (driver, _manager) = fixture.start(root.path(), fixture.edit.0);
        let turn = driver.prompt(fixture.edit.1, Vec::new()).unwrap();
        let prompt = permission(&*driver);
        std::thread::sleep(SETTLE);
        assert!(
            !(fixture.edited)(root.path()),
            "{name}: ran without an answer"
        );
        driver.cancel_turn().unwrap();
        let (events, outcome) = turn_events(&*driver, turn);
        assert_eq!(outcome.stop_reason, StopReasonKind::Cancelled, "{name}");
        assert!(!outcome.eligible_for_quiescence());
        assert!(
            events.iter().any(|e| matches!(
                &e.kind,
                AgentEventKind::PermissionClosed { id, .. } if *id == prompt.id
            )),
            "{name}: the card stayed open"
        );
        assert!(!(fixture.edited)(root.path()), "{name}: ran after stop");
    }
}

#[test]
fn stop_ends_the_turn_and_keeps_the_agent_running() {
    for fixture in WIRES {
        let name = fixture.wire.as_str();
        let root = tempfile::tempdir().unwrap();
        let (driver, manager) = fixture.start(root.path(), fixture.slow.0);
        assert!(driver.capabilities().stops_turn_in_place, "{name}");
        let processes = manager.active_count();
        let turn = driver.prompt(fixture.slow.1, Vec::new()).unwrap();
        std::thread::sleep(SETTLE);
        driver.cancel_turn().unwrap();
        let (_, outcome) = turn_events(&*driver, turn);
        assert_eq!(outcome.stop_reason, StopReasonKind::Cancelled, "{name}");
        assert!(outcome.cancel_requested);
        assert!(matches!(
            driver.cancel_turn(),
            Err(DriverError::NoActiveTurn)
        ));
        assert_eq!(
            manager.active_count(),
            processes,
            "{name}: the agent exited"
        );
        assert!(driver.failure().is_none(), "{name}");
    }
}

#[test]
fn questions_are_answered_where_the_wire_asks_them() {
    for fixture in WIRES {
        let name = fixture.wire.as_str();
        let root = tempfile::tempdir().unwrap();
        let (driver, _manager) = fixture.start(root.path(), fixture.text.0);
        let Some(ask) = fixture.ask else {
            // ACP v1 never asks: any id is unknown to the question path.
            let turn = driver.prompt(fixture.text.1, Vec::new()).unwrap();
            let (events, _) = turn_events(&*driver, turn);
            assert!(
                events
                    .iter()
                    .all(|e| !matches!(e.kind, AgentEventKind::Question(_))),
                "{name}"
            );
            continue;
        };
        let turn = driver.prompt(ask, Vec::new()).unwrap();
        let events = collect_until(&*driver, |e| matches!(e.kind, AgentEventKind::Question(_)));
        let AgentEventKind::Question(question) = &events.last().unwrap().kind else {
            unreachable!()
        };
        assert_eq!(question.turn, turn);
        let answers = question
            .questions
            .iter()
            .map(|q| QuestionAnswer {
                selected: q
                    .options
                    .first()
                    .map(|o| o.label.clone())
                    .into_iter()
                    .collect(),
                text: q.options.is_empty().then(|| "free text".to_owned()),
            })
            .collect();
        driver.answer_question(question.id, answers).unwrap();
        let (_, outcome) = turn_events(&*driver, turn);
        assert!(outcome.eligible_for_quiescence(), "{name}: {outcome:?}");
    }
}

#[test]
fn a_steer_reaches_the_turn_at_the_declared_boundary_or_is_refused() {
    for fixture in WIRES {
        let name = fixture.wire.as_str();
        let root = tempfile::tempdir().unwrap();
        let (driver, _manager) = fixture.start(root.path(), fixture.text.0);
        let steering = driver.capabilities().steering;
        let Some(steer) = fixture.steer else {
            assert_eq!(steering, SteeringMode::None, "{name}");
            assert!(matches!(
                driver.steer("more"),
                Err(DriverError::Unsupported(_))
            ));
            continue;
        };
        assert_ne!(steering, SteeringMode::None, "{name}");
        assert!(
            matches!(driver.steer("too early"), Err(DriverError::NoActiveTurn)),
            "{name}"
        );
        let turn = driver.prompt(steer, Vec::new()).unwrap();
        collect_until(&*driver, |e| {
            matches!(e.kind, AgentEventKind::MessageDelta { .. })
        });
        driver.steer("also do this").unwrap();
        let (events, _) = turn_events(&*driver, turn);
        assert!(
            text(&events).ends_with("steered"),
            "{name}: {}",
            text(&events)
        );
    }
}

#[test]
fn a_saved_session_is_resumed() {
    for fixture in WIRES {
        let name = fixture.wire.as_str();
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("hello.txt"), "hello\n").unwrap();
        let saved = "0b7c6a2e-4f1d-4e8a-9a51-3c2d1e0f9b47";
        let mut config = fixture.config(root.path(), fixture.resume);
        config.resume_session = Some(saved.into());
        let (driver, _manager) = fixture.start_with(config);
        let info = driver.wait_ready(WAIT).unwrap();
        assert!(info.restored, "{name}");
        assert_eq!(driver.wire_session_id().as_deref(), Some(saved), "{name}");
        let turn = driver.prompt(fixture.text.1, Vec::new()).unwrap();
        let (_, outcome) = turn_events(&*driver, turn);
        assert!(outcome.eligible_for_quiescence(), "{name}: {outcome:?}");
    }
}

#[test]
fn images_and_plans_are_carried() {
    for fixture in WIRES {
        let name = fixture.wire.as_str();
        let root = tempfile::tempdir().unwrap();
        let (driver, _manager) = fixture.start(root.path(), fixture.image.0);
        assert!(driver.capabilities().images, "{name}");
        let turn = driver
            .prompt(fixture.image.1, vec![PromptImage::png(PNG.to_vec())])
            .unwrap();
        let (_, outcome) = turn_events(&*driver, turn);
        assert!(outcome.eligible_for_quiescence(), "{name}: {outcome:?}");
        drop(driver);

        let Some((mode, prompt)) = fixture.plan else {
            continue;
        };
        let root = tempfile::tempdir().unwrap();
        let (driver, _manager) = fixture.start(root.path(), mode);
        let turn = driver.prompt(prompt, Vec::new()).unwrap();
        let (events, _) = turn_events(&*driver, turn);
        assert!(
            events
                .iter()
                .any(|e| matches!(&e.kind, AgentEventKind::Plan(entries) if !entries.is_empty())),
            "{name}: no plan"
        );
        // Every wire lists the commands it understands, without a leading slash.
        let commands: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.kind {
                AgentEventKind::AvailableCommands(commands) => Some(commands),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(!commands.is_empty(), "{name}: no commands");
        assert!(
            commands.iter().all(|c| !c.name.starts_with('/')),
            "{name}: {commands:?}"
        );
        assert!(commands.iter().any(|c| c.name == "compact"), "{name}");
    }
}

/// The case ACP adapters cannot express: the agent leaves background work and ends its
/// reply. The turn ends on the wire's own end frame, and the driver says the work is
/// still running, before that end and again when it clears; the engine's capture waits
/// on it.
#[test]
fn background_work_is_reported_around_the_turn_end_where_the_wire_has_it() {
    for fixture in WIRES {
        let name = fixture.wire.as_str();
        let root = tempfile::tempdir().unwrap();
        let (driver, _manager) = fixture.start(root.path(), fixture.text.0);
        let Some(background) = fixture.background else {
            assert!(!driver.capabilities().deterministic_turn_end, "{name}");
            continue;
        };
        assert!(driver.capabilities().deterministic_turn_end, "{name}");
        driver.prompt(background, Vec::new()).unwrap();
        let events = collect_until(&*driver, |e| finished(e).is_some());
        let running = events
            .iter()
            .position(|e| matches!(e.kind, AgentEventKind::Background { tasks, .. } if tasks > 0));
        assert!(
            running.is_some_and(|at| at + 1 < events.len()),
            "{name}: the work was not reported before the end: {events:#?}"
        );
        // Claude may continue in a turn of its own once the work clears.
        collect_until(&*driver, |e| {
            matches!(e.kind, AgentEventKind::Background { tasks: 0, .. })
        });
    }
}
