//! The native panel's rules against REAL workflow snapshots: the scripted ACP peer
//! (`tests/support/acp-agent.py`), the deterministic fake preview worker and the engine's
//! real controller. No window is opened here: the controls, reply correlation, paging
//! window and shell hosting glue are pure functions of snapshots, so they are proven
//! against what the workflow actually publishes. (The GPUI process itself is exercised by
//! `x11_shell.rs`; neither the peer nor these tests qualify a provider.)
use fframes_studio::{
    agent_workflow::{log::RowLimits, *},
    build_service::{BuildLimits, BuildService},
    conversation_panel::{
        controls::{self, ReplyRefusal, ReplyTracker, SubmitKind, UndoControl, derive_controls},
        host::{self, AdapterFile, HostInbox, McpChoice},
        qualification::OwnershipPolicy,
        rows::{self, ListOp, ResidentLimits, RowKey},
    },
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::Arc, time::Duration};
use studio_agent_spike::{AdapterConfig, driver::OptionValue};
use studio_bootstrap::{ProcessTreeManager, WriterOwnership};
use studio_engine::{AgentTaskId, Controller, ReviewPolicy, app_paths::AppPaths};

#[path = "support/build_fixture.rs"]
mod fixture;
use fixture::*;

const WAIT: Duration = Duration::from_secs(90);
const GOOD_CONFIG: &str = r#"{"frames":120,"tracks":[[0.0,2.0]],"audio":"tone","pixel":40}"#;

fn good(marker: &str) -> Value {
    json!({
        "text": ["Working on it. ", "done"],
        "tool": 2,
        "write": {"src/lib.rs": format!("// {marker}\n"), FAKE_CONFIG: GOOD_CONFIG},
    })
}

fn qualified() -> WriterOwnership {
    WriterOwnership::ProcessGroupContained {
        qualification: "scripted-fixture".into(),
    }
}

/// The host never consults the global PATH: the description names the interpreter by its
/// absolute path, as a user's Setup would.
fn python3() -> String {
    std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|dir| dir.join("python3"))
        .find(|candidate| candidate.is_file())
        .expect("python3 on PATH")
        .to_string_lossy()
        .into_owned()
}

fn agent_script() -> String {
    format!("{}/tests/support/acp-agent.py", env!("CARGO_MANIFEST_DIR"))
}

struct World {
    _temp: tempfile::TempDir,
    paths: AppPaths,
    sdk: PathBuf,
    agent: PathBuf,
    controller: Arc<Mutex<Controller>>,
    service: BuildService,
    workflow: Option<Arc<AgentWorkflow>>,
}

impl World {
    fn build(&self) -> BuildSettings {
        BuildSettings {
            service: self.service.clone(),
            sdk: self.sdk.clone(),
            compatibility: manifest(),
        }
    }

    /// The scripted peer is contained by construction in these tests: an explicit, labelled
    /// test injection, never a qualification.
    fn policy(&self) -> OwnershipPolicy {
        OwnershipPolicy::TestInjected(WriterOwnership::ProcessGroupContained {
            qualification: "scripted-fixture".into(),
        })
    }

    fn adapter(&self, auth_env_names: Vec<String>) -> AdapterFile {
        AdapterFile {
            provider: "scripted".into(),
            executable: python3(),
            args: vec![agent_script(), self.agent.to_string_lossy().into_owned()],
            auth_env_names,
            auth_method: None,
            mcp: McpChoice::Unsupported,
        }
    }

    /// Opens the workflow exactly like the shell does: the saved description, the
    /// UI hand-off sink and the wake callback.
    fn open(&mut self, inbox: &Arc<HostInbox>) -> host::Opened {
        let opened = host::open_workflow(
            self.controller.clone(),
            self.paths.clone(),
            Some(self.build()),
            1,
            inbox.clone(),
            self.policy(),
        );
        assert!(
            opened.settings_error.is_none(),
            "{:?}",
            opened.settings_error
        );
        opened
    }

    fn new() -> (Self, Arc<HostInbox>) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("video");
        create_project(&root);
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let agent = temp.path().join("agent-evidence");
        fs::create_dir_all(&agent).unwrap();
        let controller = Arc::new(Mutex::new(Controller::open(&root, &paths).unwrap()));
        let service = BuildService::new(
            ProcessTreeManager::new(),
            FakeCompiler::new(true),
            BuildLimits::default(),
        );
        let mut world = Self {
            sdk: fake_sdk(temp.path()),
            _temp: temp,
            paths,
            agent,
            controller,
            service,
            workflow: None,
        };
        // The adapter is configured the way the Setup tab saves it.
        host::save_settings(&world.paths, &world.adapter(vec![])).unwrap();
        let inbox = Arc::new(HostInbox::default());
        let opened = world.open(&inbox);
        world.workflow = Some(Arc::new(opened.result.unwrap()));
        (world, inbox)
    }

    fn wf(&self) -> &Arc<AgentWorkflow> {
        self.workflow.as_ref().unwrap()
    }

    fn snap(&self) -> Arc<WorkflowSnapshot> {
        self.wf().snapshot()
    }

    fn wait(
        &self,
        what: &str,
        predicate: impl Fn(&WorkflowSnapshot) -> bool,
    ) -> Arc<WorkflowSnapshot> {
        self.wf().wait_for(WAIT, predicate).unwrap_or_else(|| {
            let s = self.snap();
            panic!(
                "timed out waiting for {what}; phase {:?}, error {:?}",
                s.task.as_ref().map(|t| t.phase),
                s.task.as_ref().and_then(|t| t.error.clone())
            )
        })
    }

    fn wait_phase(&self, phase: TaskPhase) -> Arc<WorkflowSnapshot> {
        self.wait(&format!("phase {phase:?}"), |s| {
            s.task.as_ref().is_some_and(|t| t.phase == phase)
        })
    }

    fn brief(script: &Value) -> String {
        format!("SCRIPT {script}\nplease make the edit")
    }

    fn submit(&self, script: &Value) {
        self.wf().submit(&Self::brief(script)).unwrap();
    }

    fn set_plan(&self, turns: &[Value]) {
        fs::write(
            self.agent.join("plan.json"),
            json!({"turns": turns, "default": good("default")}).to_string(),
        )
        .unwrap();
        let _ = fs::remove_file(self.agent.join("prompt-counter"));
    }

    fn processes(&self) -> usize {
        self.controller.lock().processes.active_count()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        if let Some(workflow) = self.workflow.take() {
            workflow.close();
        }
    }
}

fn controls_of(world: &World, input_empty: bool) -> controls::Controls {
    derive_controls(&world.snap(), input_empty, true)
}

// ---- hosting -------------------------------------------------------------------------------------

#[test]
fn opening_through_the_host_reads_the_saved_description_and_launches_no_agent() {
    let (world, _inbox) = World::new();
    let snapshot = world.snap();
    assert_eq!(snapshot.adapter.provider.as_deref(), Some("scripted"));
    assert_eq!(snapshot.adapter.readiness, AdapterReadiness::Unchecked);
    assert_eq!(
        snapshot.adapter.writer_ownership.as_deref(),
        Some("process-group-contained")
    );
    assert_eq!(world.processes(), 0, "opening a project starts nothing");
    assert!(
        !world.agent.join("starts").exists(),
        "the adapter was never started"
    );
    let guidance = controls::guidance(&snapshot.adapter.readiness, true);
    assert!(guidance.usable);
    let controls = controls_of(&world, false);
    assert_eq!(controls.submit.kind, SubmitKind::Brief);
    assert!(controls.submit.blocked.is_none());
    assert!(controls.check_adapter);
    assert!(controls.stop.is_none());
}

#[test]
fn a_missing_description_leaves_the_workflow_unconfigured_and_the_panel_says_how_to_start() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    create_project(&root);
    let paths = AppPaths::new(temp.path().join("data")).unwrap();
    let controller = Arc::new(Mutex::new(Controller::open(&root, &paths).unwrap()));
    let inbox = Arc::new(HostInbox::default());
    let opened = host::open_workflow(
        controller,
        paths.clone(),
        None,
        3,
        inbox,
        OwnershipPolicy::Validated,
    );
    assert!(opened.adapter.is_none() && opened.settings_error.is_none());
    let workflow = opened.result.unwrap();
    let snapshot = workflow.snapshot();
    assert_eq!(snapshot.adapter.readiness, AdapterReadiness::NotConfigured);
    let controls = derive_controls(&snapshot, false, true);
    assert!(controls.submit.blocked.unwrap().contains("Setup"));
    assert!(!controls.check_adapter);
    assert!(!controls::guidance(&snapshot.adapter.readiness, false).usable);
    // A damaged description is reported, never silently ignored: the workflow still opens,
    // unconfigured, and the panel shows why.
    workflow.close();
    drop(workflow);
    fs::write(host::settings_path(&paths), "{broken").unwrap();
    let controller = Arc::new(Mutex::new(Controller::open(&root, &paths).unwrap()));
    let opened = host::open_workflow(
        controller,
        paths,
        None,
        4,
        Arc::new(HostInbox::default()),
        OwnershipPolicy::Validated,
    );
    assert!(opened.adapter.is_none());
    assert!(
        opened
            .settings_error
            .as_deref()
            .unwrap()
            .contains("Invalid adapter JSON")
    );
    assert_eq!(
        opened.result.unwrap().snapshot().adapter.readiness,
        AdapterReadiness::NotConfigured
    );
}

#[test]
fn saving_a_description_reconfigures_an_idle_workflow_and_is_refused_while_a_task_runs() {
    let (world, _inbox) = World::new();
    let changed = AdapterFile {
        provider: "other".into(),
        ..world.adapter(vec!["SOME_TOKEN_NAME".into()])
    };
    let (settings, _) = changed.resolve(&world.paths, &world.policy());
    world.wf().set_adapter(Some(settings)).unwrap();
    world.wait("the new adapter", |s| {
        s.adapter.provider.as_deref() == Some("other")
    });
    let (original, _) = world.adapter(vec![]).resolve(&world.paths, &world.policy());
    world.wf().set_adapter(Some(original)).unwrap();
    world.wait("the original adapter", |s| {
        s.adapter.provider.as_deref() == Some("scripted")
    });

    world.submit(&json!({"hang": true}));
    world.wait_phase(TaskPhase::Editing);
    assert!(!controls_of(&world, false).settings_editable);
    assert!(matches!(
        world
            .wf()
            .set_adapter(Some(changed.resolve(&world.paths, &world.policy()).0)),
        Err(WorkflowError::Busy(_))
    ));
    world.wf().stop().unwrap();
    world.wait("the stopped task", |s| {
        s.task
            .as_ref()
            .is_some_and(|t| t.phase == TaskPhase::Cancelled)
    });
    assert!(controls_of(&world, false).settings_editable);
}

// ---- controls follow the real task ---------------------------------------------------------------------

#[test]
fn the_controls_follow_a_real_task_from_start_to_acceptance() {
    let (world, inbox) = World::new();
    let mut script = good("controls");
    script["wait_file"] = json!("go");
    world.submit(&script);
    let running = world.wait_phase(TaskPhase::Editing);
    let controls = derive_controls(&running, false, true);
    assert_eq!(controls.submit.kind, SubmitKind::Queue);
    assert!(controls.stop.is_some(), "Stop while the agent edits");
    assert!(!controls.settings_editable);
    assert!(
        matches!(controls.undo, UndoControl::Disabled(_)),
        "no Undo while a task runs"
    );
    let task = running.task.as_ref().unwrap();
    let lines = controls::identity_lines(task);
    assert!(lines.iter().any(|(k, v)| *k == "Base" && !v.is_empty()));
    assert!(
        lines
            .iter()
            .any(|(k, v)| *k == "Working copy" && v.ends_with("agent/draft")),
        "{lines:?}"
    );
    // A follow-up queues behind the running task.
    world.wf().submit("a follow-up").unwrap();
    let queued = world.wait("the queued follow-up", |s| s.queue.len() == 1);
    let controls = derive_controls(&queued, false, true);
    assert_eq!(controls.submit.kind, SubmitKind::Queue);
    assert!(
        controls
            .stop
            .as_deref()
            .unwrap()
            .contains("Cancels the agent")
    );
    fs::write(world.agent.join("go"), "go").unwrap();
    world.wait("both tasks to finish", |s| {
        s.queue.is_empty()
            && s.task
                .as_ref()
                .is_some_and(|t| t.phase == TaskPhase::Accepted)
            && s.history.len() >= 2
    });
    let done = controls_of(&world, false);
    assert_eq!(done.submit.kind, SubmitKind::Brief);
    assert!(done.stop.is_none());
    assert!(matches!(done.undo, UndoControl::Enabled { .. }));
    assert!(done.settings_editable);
    // The staged previews wait in the shell's inbox; dropping them (the shell adopts or
    // drops every one) reaps their workers.
    drop(inbox.take_handoffs());
    let deadline = std::time::Instant::now() + WAIT;
    while world.processes() != 0 {
        assert!(std::time::Instant::now() < deadline, "a worker survived");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn manual_review_exposes_apply_discard_and_the_frozen_policy() {
    let (world, _inbox) = World::new();
    world
        .wf()
        .set_review_policy(ReviewPolicy::ManualReview)
        .unwrap();
    world.wait("the policy", |s| {
        s.review_policy == ReviewPolicy::ManualReview
    });
    world.submit(&good("review me"));
    let reviewing = world.wait_phase(TaskPhase::AwaitingReview);
    let controls = derive_controls(&reviewing, false, true);
    assert!(
        controls.review.is_some(),
        "Apply / Discard / Export are offered"
    );
    assert!(controls.stop.as_deref().unwrap().contains("Discard"));
    // A later policy change never touches the running task's frozen policy.
    world
        .wf()
        .set_review_policy(ReviewPolicy::AutoApply)
        .unwrap();
    let after = world.wait("the policy flip", |s| {
        s.review_policy == ReviewPolicy::AutoApply
    });
    assert_eq!(
        after.task.as_ref().unwrap().review_policy,
        ReviewPolicy::ManualReview
    );
    assert!(derive_controls(&after, false, true).review.is_some());
    let lines = controls::identity_lines(after.task.as_ref().unwrap());
    assert!(
        lines
            .iter()
            .any(|(k, v)| *k == "Review" && v.contains("Manual"))
    );
    world.wf().discard().unwrap();
    let discarded = world.wait("the discard", |s| {
        s.task.as_ref().is_some_and(|t| t.phase.is_terminal())
    });
    assert!(derive_controls(&discarded, false, true).review.is_none());
}

#[test]
fn a_clarification_turns_the_prompt_into_an_answer_for_that_task() {
    let (world, _inbox) = World::new();
    world.set_plan(&[json!({"text": "Which title should I use?"}), good("titled")]);
    world.wf().submit("plan brief").unwrap();
    let waiting = world.wait_phase(TaskPhase::WaitingClarification);
    let task = waiting.task.clone().unwrap();
    let controls = derive_controls(&waiting, false, true);
    assert_eq!(controls.submit.kind, SubmitKind::Clarify(task.id.clone()));
    assert_eq!(controls.submit.label(), "Answer");
    assert!(controls.stop.is_some());
    world
        .wf()
        .reply_clarification(&task.id, "Use Title A")
        .unwrap();
    world.wait("acceptance", |s| {
        s.task
            .as_ref()
            .is_some_and(|t| t.phase == TaskPhase::Accepted)
    });
    assert!(matches!(
        world.wf().reply_clarification(&task.id, "again"),
        Err(WorkflowError::NotWaiting)
    ));
}

#[test]
fn advertised_options_are_offered_only_while_the_agent_advertises_them() {
    let (world, _inbox) = World::new();
    let idle = controls_of(&world, false);
    assert!(!idle.modes && !idle.config_options && idle.options_note.is_none());
    world.submit(&json!({"hang": true}));
    let live = world.wait("advertised options", |s| !s.options.modes.is_empty());
    let controls = derive_controls(&live, false, true);
    assert!(controls.modes && controls.config_options);
    assert!(controls.options_note.is_none());
    world.wf().set_mode("plan").unwrap();
    world
        .wf()
        .set_config_option("model", OptionValue::Value("deep".into()))
        .unwrap();
    assert!(matches!(
        world.wf().set_mode("not-advertised"),
        Err(WorkflowError::UnknownOption(_))
    ));
    world.wf().stop().unwrap();
    world.wait("the stop", |s| {
        s.task
            .as_ref()
            .is_some_and(|t| t.phase == TaskPhase::Cancelled)
    });
}

#[test]
fn a_real_permission_card_is_answered_once_and_closed_cards_refuse_clicks() {
    let (world, _inbox) = World::new();
    let mut script = good("permitted");
    script["permission"] = json!({"title": "Edit title"});
    world.submit(&script);
    let waiting = world.wait_phase(TaskPhase::WaitingPermission);
    let open = waiting.open_permissions();
    assert_eq!(open.len(), 1);
    let card = open[0].clone();
    let mut tracker = ReplyTracker::default();
    // The UI claims the answer, the workflow takes it, a second click is a no-op.
    assert_eq!(tracker.begin(&card.reference, &open), Ok(()));
    world
        .wf()
        .reply_permission(&card.reference, PermissionAnswer::Select("allow".into()))
        .unwrap();
    assert_eq!(
        tracker.begin(&card.reference, &open),
        Err(ReplyRefusal::AlreadySent)
    );
    let done = world.wait("acceptance", |s| {
        s.task
            .as_ref()
            .is_some_and(|t| t.phase == TaskPhase::Accepted)
    });
    // A click on the (now closed) card of a LATER snapshot is refused by the UI as stale,
    // and the workflow agrees.
    let closed = done
        .rows
        .iter()
        .find_map(|r| match &r.kind {
            RowKind::Permission(c) if c.reference == card.reference => Some(c.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        closed.state,
        PermissionState::Selected {
            option_id: "allow".into()
        }
    );
    let mut fresh = ReplyTracker::default();
    assert_eq!(
        fresh.begin(&closed.reference, &done.open_permissions()),
        Err(ReplyRefusal::NotOpen)
    );
    assert_eq!(
        world
            .wf()
            .reply_permission(&closed.reference, PermissionAnswer::Cancel)
            .unwrap_err(),
        WorkflowError::DuplicatePermission
    );
    // Another task's / another writer's request is stale for both.
    let mut foreign = closed.reference.clone();
    foreign.task = AgentTaskId::new();
    assert_eq!(
        fresh.begin(&foreign, &done.open_permissions()),
        Err(ReplyRefusal::NotOpen)
    );
    assert_eq!(
        world
            .wf()
            .reply_permission(&foreign, PermissionAnswer::Cancel)
            .unwrap_err(),
        WorkflowError::StalePermission
    );
}

#[test]
fn the_production_policy_never_lets_a_saved_adapter_claim_containment() {
    use fframes_studio::conversation_panel::qualification::Containment;
    let (mut world, inbox) = World::new();
    world.workflow.take().unwrap().close();
    // The same saved description, opened under the production policy: no ledger is
    // installed, so the writer stays unqualified however the file is written.
    let opened = host::open_workflow(
        world.controller.clone(),
        world.paths.clone(),
        Some(world.build()),
        9,
        inbox.clone(),
        OwnershipPolicy::Validated,
    );
    let resolution = opened.resolution.clone().expect("an adapter is saved");
    assert!(matches!(
        resolution.containment,
        Containment::Unknown { .. }
    ));
    assert_eq!(resolution.ownership, WriterOwnership::Unknown);
    let workflow = opened.result.unwrap();
    let snapshot = workflow.snapshot();
    assert_eq!(
        snapshot.adapter.writer_ownership.as_deref(),
        Some(WriterOwnership::Unknown.label())
    );
    assert!(
        resolution
            .summary()
            .contains("Candidates from this adapter stay blocked")
    );
    workflow.close();
}

// ---- the virtualized window over a real log ----------------------------------------------------------

#[test]
fn a_long_real_conversation_pages_back_inside_the_resident_limits() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    create_project(&root);
    let paths = AppPaths::new(temp.path().join("data")).unwrap();
    let agent = temp.path().join("agent-evidence");
    fs::create_dir_all(&agent).unwrap();
    let controller = Arc::new(Mutex::new(Controller::open(&root, &paths).unwrap()));
    let service = BuildService::new(
        ProcessTreeManager::new(),
        FakeCompiler::new(true),
        BuildLimits::default(),
    );
    let mut config = WorkflowConfig::new(paths);
    config.row_limits = RowLimits {
        max_rows: 30,
        ..RowLimits::default()
    };
    config.build = Some(BuildSettings {
        service,
        sdk: fake_sdk(temp.path()),
        compatibility: manifest(),
    });
    config.adapter = Some(AdapterSettings {
        provider: "scripted".into(),
        adapter: AdapterConfig {
            executable: python3(),
            args: vec![agent_script(), agent.to_string_lossy().into_owned()],
            auth_env_names: vec![],
        },
        writer_ownership: qualified(),
        ownership_probe: None,
        mcp: studio_agent_spike::McpStdioSupport::Unsupported,
        auth_method: None,
    });
    config.limits.shutdown_grace = Duration::from_secs(1);
    let workflow = AgentWorkflow::open(config, controller).unwrap();
    let mut script = good("flood");
    script["flood_tools"] = json!(150);
    workflow
        .submit(&format!("SCRIPT {script}\nplease make the edit"))
        .unwrap();
    let snapshot = workflow
        .wait_for(WAIT, |s| {
            s.task
                .as_ref()
                .is_some_and(|t| t.phase == TaskPhase::Accepted)
        })
        .expect("the flooded task is accepted");
    assert!(snapshot.older_rows);

    let limits = ResidentLimits {
        rows: 60,
        bytes: 64 * 1024,
    };
    // The list starts with the live window; every page it asks for lands in front.
    let mut older: Vec<Arc<Row>> = Vec::new();
    let mut view = rows::compose_view(&older, &snapshot.rows, limits);
    let mut keys: Vec<RowKey> = view.rows.iter().map(RowKey::of).collect();
    assert_eq!(view.rows.len(), snapshot.rows.len());
    let mut list_len = keys.len();
    let mut loaded_pages = 0;
    loop {
        let first = view.rows[0].id;
        let Some((before, limit)) =
            rows::page_request(&view.rows, rows::Viewport { first_visible: 0 }, true, false)
        else {
            unreachable!("the viewport is at the top and nothing is loading")
        };
        assert_eq!(before, first.0);
        let page = workflow
            .history_page(RowId(before), limit)
            .expect("the log is readable");
        if page.is_empty() {
            break;
        }
        rows::prepend_page(&mut older, page, limits);
        let next = rows::compose_view(&older, &snapshot.rows, limits);
        let next_keys: Vec<RowKey> = next.rows.iter().map(RowKey::of).collect();
        // The mutations the list receives keep what the reader looks at in place.
        let ops = rows::diff_rows(&keys, &next_keys);
        for op in &ops {
            match op {
                ListOp::Splice { at, old, new } => {
                    assert!(*at + *old <= list_len, "{op:?} stays inside the list");
                    list_len = list_len - old + new;
                }
                ListOp::Remeasure { at, len } => assert!(*at + *len <= list_len),
                ListOp::Reset { .. } => panic!("an ordered log never resets the list"),
            }
        }
        assert_eq!(list_len, next_keys.len(), "the ops replay to the new list");
        // Resident memory never exceeds the limits, however far back the user pages.
        let bytes: usize = next.rows.iter().map(|r| r.estimated_bytes()).sum();
        assert!(next.rows.len() <= limits.rows && bytes <= limits.bytes);
        assert!(next.rows.windows(2).all(|p| p[0].id < p[1].id));
        view = next;
        keys = next_keys;
        loaded_pages += 1;
        if view.rows.len() >= limits.rows || loaded_pages > 12 {
            break;
        }
    }
    assert!(loaded_pages >= 1, "at least one older page was paged in");
    assert!(view.rows.len() <= limits.rows);
    workflow.close();
}

// ---- the preview hand-off the shell completes --------------------------------------------------------

#[test]
fn the_ui_sink_queues_the_promotion_and_the_shell_acknowledgement_reaches_the_snapshot() {
    let (mut world, inbox) = World::new();
    // Re-open with the sink the shell installs (World::new used the same host path).
    world.workflow.take().unwrap().close();
    let opened = world.open(&inbox);
    let workflow = Arc::new(opened.result.unwrap());
    world.workflow = Some(workflow.clone());
    world.submit(&good("handed off"));
    world.wait("acceptance", |s| {
        s.task
            .as_ref()
            .is_some_and(|t| t.phase == TaskPhase::Accepted)
    });
    let queued = {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            let mut taken = inbox.take_handoffs();
            if let Some(first) = taken.pop() {
                break first;
            }
            assert!(std::time::Instant::now() < deadline, "no hand-off arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    assert_eq!(
        queued.serial, 1,
        "tagged with the workflow that produced it"
    );
    assert_eq!(queued.handoff.kind, HandoffKind::Apply);
    let published = queued
        .handoff
        .promotion
        .record
        .published
        .as_str()
        .to_owned();
    // The shell adopts on its thread: first an Err (nothing to adopt) is shown honestly.
    workflow
        .report_handoff(&published, Err("the preview worker refused".into()))
        .unwrap();
    let waiting = world.wait("the awaiting label", |s| {
        matches!(
            s.handoff.as_ref().map(|h| &h.state),
            Some(HandoffState::AwaitingPreview { reason: Some(_) })
        )
    });
    assert_eq!(waiting.handoff.as_ref().unwrap().kind, HandoffKind::Apply);
    // An ordinary rebuild later displays exactly the accepted revision.
    workflow.preview_displayed(&published).unwrap();
    world.wait("the displayed state", |s| {
        s.handoff
            .as_ref()
            .is_some_and(|h| h.state == HandoffState::Displayed)
    });
    // A hand-off from another workflow serial is the shell's to drop; the inbox keeps no residue.
    assert_eq!(inbox.pending_handoffs(), 0);
    drop(queued);
}
