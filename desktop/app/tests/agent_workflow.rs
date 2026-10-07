//! The native agent workflow against real subprocesses: the scripted ACP peer
//! (`tests/support/acp-agent.py`), the deterministic fake preview worker and the engine's
//! real controller. The peer proves transport and workflow only; it never qualifies a
//! provider, an adapter or authentication.

#![allow(unused_imports)]

use fframes_studio_protocol::{EditorFrameStatus, PreviewIdentity};

// The shared harness (World, Scripted compiler, script helpers) lives in one file so
// the scoped-editing suite runs against exactly the same subprocess setup.
include!("support/workflow_world.rs");

#[test]
fn opening_a_project_launches_no_agent_and_readiness_is_an_explicit_probe() {
    let w = World::new();
    let s = w.snap();
    assert_eq!(s.adapter.readiness, AdapterReadiness::Unchecked);
    assert_eq!(s.adapter.provider.as_deref(), Some("scripted"));
    assert!(s.task.is_none());
    assert!(
        !w.agent.join("calls.jsonl").exists() && !w.agent.join("starts").exists(),
        "project open must never start the adapter"
    );
    assert_eq!(w.processes(), 0);
    w.wf().check_adapter().unwrap();
    let s = w.wait("the probe result", |s| {
        matches!(s.adapter.readiness, AdapterReadiness::Checked { .. })
    });
    let AdapterReadiness::Checked { report, .. } = &s.adapter.readiness else {
        unreachable!()
    };
    assert!(
        matches!(&report.status, studio_agent_spike::AdapterStatus::Ready { agent, .. } if agent == "scripted-agent"),
        "{:?}",
        report.status
    );
    assert_eq!(w.processes(), 0, "the probe reaps its adapter");
    // The probe is not a task.
    assert!(w.snap().task.is_none());
}

#[test]
fn a_missing_adapter_is_reported_by_the_probe_and_by_submit_without_a_task() {
    let w = World::with(Options {
        executable: Some("definitely-not-an-adapter-xyz".into()),
        ..Options::default()
    });
    w.wf().check_adapter().unwrap();
    let s = w.wait("the probe result", |s| {
        matches!(s.adapter.readiness, AdapterReadiness::Checked { .. })
    });
    let AdapterReadiness::Checked { report, .. } = &s.adapter.readiness else {
        unreachable!()
    };
    assert!(matches!(
        report.status,
        studio_agent_spike::AdapterStatus::MissingExecutable { .. }
    ));
    w.wf().submit("do something").unwrap();
    w.wait("the missing-adapter error", |s| {
        has_error(s, "adapter_missing")
    });
    assert!(
        w.snap().task.is_none(),
        "no task is created for an adapter that cannot start"
    );
    assert_eq!(w.engine_state(), None);
}

#[test]
fn commands_are_refused_before_they_reach_the_actor() {
    let w = World::with(Options {
        adapter: false,
        ..Options::default()
    });
    assert_eq!(w.snap().adapter.readiness, AdapterReadiness::NotConfigured);
    assert_eq!(
        w.wf().submit("hello").unwrap_err(),
        WorkflowError::NotConfigured
    );
    assert_eq!(
        w.wf().check_adapter().unwrap_err(),
        WorkflowError::NotConfigured
    );
    let w = World::new();
    assert!(matches!(
        w.wf().submit("  ").unwrap_err(),
        WorkflowError::InvalidBrief(_)
    ));
    assert_eq!(
        w.wf().set_mode("code").unwrap_err(),
        WorkflowError::UnknownOption("code".into()),
        "options are only settable once the adapter advertised them"
    );
    w.wf().close();
    assert_eq!(w.wf().submit("hello").unwrap_err(), WorkflowError::Closed);
    assert!(w.snap().closed);
}

// ---- the happy path ---------------------------------------------------------------------------

#[test]
fn a_brief_becomes_a_validated_candidate_is_auto_applied_and_accepted() {
    let w = World::new();
    let original = w.source("src/lib.rs");
    w.submit(&good("first"));
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    assert_eq!(task.engine_state, TaskState::Accepted);
    assert_eq!(w.engine_state(), Some(TaskState::Accepted));
    assert_ne!(w.source("src/lib.rs"), original);
    assert_eq!(w.source("src/lib.rs"), "// first\n");
    assert_eq!(task.review_policy, studio_engine::ReviewPolicy::AutoApply);
    assert_eq!(
        task.repair.used, 0,
        "validation failed once: {:?} / {:?}",
        task.repair, task.validation
    );
    assert_eq!(task.turns, 1);
    assert_eq!(
        task.writer.as_ref().unwrap().ownership,
        "process-group-contained"
    );
    assert!(task.writer.as_ref().unwrap().provider_session.is_some());
    assert!(matches!(
        task.draft_state,
        Some(DraftState::Accepted { .. })
    ));

    let s = w.snap();
    // Ordered rows: strictly increasing ids, the brief first.
    let ids: Vec<u64> = s.rows.iter().map(|r| r.id.0).collect();
    assert!(ids.windows(2).all(|p| p[0] < p[1]), "{ids:?}");
    assert!(matches!(
        &s.rows[0].kind,
        RowKind::User { source: UserSource::Brief, text } if text.starts_with("SCRIPT")
    ));
    // Streamed text coalesced; tool deltas updated the SAME two cards in place.
    let agent_text: String = s
        .rows
        .iter()
        .filter_map(|r| match &r.kind {
            RowKind::Agent {
                text, streaming, ..
            } => {
                assert!(!streaming, "the stream closed with the turn");
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(agent_text, "Working on it. done");
    let tools: Vec<&ToolCard> = s
        .rows
        .iter()
        .filter_map(|r| match &r.kind {
            RowKind::Tool(card) => Some(card),
            _ => None,
        })
        .collect();
    assert_eq!(tools.len(), 2, "tool updates must not append rows");
    assert!(tools.iter().all(|t| {
        t.status == Some(studio_agent_spike::driver::ToolStatus::Completed) && t.updates == 2
    }));
    // Changed-file summary and validation coverage/diagnostics.
    let changes = task.changes.as_ref().expect("changed files");
    assert_eq!(changes.total, 2);
    assert!(changes.entries.iter().any(|e| e.path == "src/lib.rs"));
    let validation = task.validation.as_ref().expect("validation card");
    assert!(validation.passed, "{}", validation.summary);
    assert!(validation.coverage.as_ref().unwrap().rendered_frames > 0);
    assert!(validation.audio.as_ref().unwrap().placement_verified);
    assert!(
        s.rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Validation(c) if c.passed))
    );
    assert!(
        s.rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Outcome(o) if o.kind == OutcomeKind::Accepted))
    );
    // Advertised options and capabilities (limits to show); nothing else settable.
    assert_eq!(
        s.options
            .modes
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["code", "plan"]
    );
    assert!(s.capabilities.is_some());
    // History, Undo and the awaiting-preview state (no preview sink configured).
    assert_eq!(s.history.len(), 1);
    assert!(matches!(s.undo, UndoView::Available { .. }));
    let handoff = s.handoff.as_ref().unwrap();
    assert_eq!(handoff.kind, HandoffKind::Apply);
    assert_eq!(
        handoff.state,
        HandoffState::AwaitingPreview { reason: None }
    );
    w.assert_clean();
}

#[test]
fn project_tools_are_offered_over_mcp_only_when_policy_allows_and_die_with_the_task() {
    let w = World::new();
    w.submit(&json!({"hang": true}));
    w.wait_phase(TaskPhase::Editing);
    w.wait_prompts(1);
    let params: Value =
        serde_json::from_str(&fs::read_to_string(w.agent.join("session-params.json")).unwrap())
            .unwrap();
    let servers = params["mcpServers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["name"], "fframes-studio");
    let args = servers[0]["args"].as_array().unwrap();
    assert_eq!(args[0], "--capability");
    let capability = PathBuf::from(args[1].as_str().unwrap());
    assert!(
        capability.is_file(),
        "the capability exists while the task runs"
    );
    assert!(
        servers[0]["env"].as_array().unwrap().is_empty(),
        "no secret in the environment"
    );
    let s = w.snap();
    assert!(s.mcp.policy_enabled && s.mcp.binary_available && s.mcp.active);
    assert_eq!(s.resources.broker_grants, 1);
    w.wf().stop().unwrap();
    w.wait_task(None);
    assert!(!capability.exists(), "the capability died with the task");
    w.assert_clean();

    // Host policy disables MCP: no MCP server, but the CLI route has its own capability
    // (exercised end to end in `the_command_line_route_has_a_task_capability...`).
    let w = World::with(Options {
        mcp: McpStdioSupport::Unsupported,
        ..Options::default()
    });
    w.submit(&json!({"hang": true}));
    w.wait_phase(TaskPhase::Editing);
    w.wait_prompts(1);
    let params: Value =
        serde_json::from_str(&fs::read_to_string(w.agent.join("session-params.json")).unwrap())
            .unwrap();
    assert!(params["mcpServers"].as_array().unwrap().is_empty());
    let s = w.snap();
    assert!(!s.mcp.policy_enabled && !s.mcp.active && s.mcp.cli_active);
    assert_eq!(s.resources.broker_grants, 1);
    w.wf().stop().unwrap();
    w.wait_task(None);
}

#[test]
fn advertised_options_can_be_changed_and_unadvertised_ones_are_refused() {
    let w = World::new();
    w.submit(&json!({"hang": true}));
    w.wait("advertised options", |s| !s.options.modes.is_empty());
    w.wait_phase(TaskPhase::Editing);
    w.wf().set_mode("plan").unwrap();
    assert_eq!(
        w.wf().set_mode("bogus").unwrap_err(),
        WorkflowError::UnknownOption("bogus".into())
    );
    w.wf()
        .set_config_option("model", OptionValue::Value("deep".into()))
        .unwrap();
    assert_eq!(
        w.wf()
            .set_config_option("nope", OptionValue::Boolean(true))
            .unwrap_err(),
        WorkflowError::UnknownOption("nope".into())
    );
    let deadline = Instant::now() + WAIT;
    while !(w.agent.join("set-mode.json").exists() && w.agent.join("set-config.json").exists()) {
        assert!(
            Instant::now() < deadline,
            "the agent never saw the option changes"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Settings cannot change under a running task.
    assert!(matches!(
        w.wf()
            .set_review_policy(studio_engine::ReviewPolicy::ManualReview),
        Ok(())
    ));
    assert!(matches!(
        w.wf().set_adapter(None),
        Err(WorkflowError::Busy(_))
    ));
    w.wf().stop().unwrap();
    w.wait_task(None);
}

// ---- manual review, policy -----------------------------------------------------------------------

#[test]
fn manual_review_retains_the_candidate_and_apply_publishes_it() {
    let w = World::new();
    w.wf()
        .set_review_policy(studio_engine::ReviewPolicy::ManualReview)
        .unwrap();
    w.wait("the policy", |s| {
        s.review_policy == studio_engine::ReviewPolicy::ManualReview
    });
    let original = w.source("src/lib.rs");
    w.submit(&good("manual"));
    let task = w.wait_phase(TaskPhase::AwaitingReview);
    assert_eq!(task.engine_state, TaskState::CandidateReady);
    assert_eq!(
        w.source("src/lib.rs"),
        original,
        "nothing is published before review"
    );
    assert!(task.review.as_ref().unwrap().apply_blocked.is_none());
    assert!(task.validation.as_ref().unwrap().passed);
    assert!(task.changes.is_some());
    assert_eq!(w.snap().history.len(), 0);
    // Export gives an independent copy of the candidate without publishing it.
    let export = w._temp.path().join("exported");
    w.wf().export_candidate(&export).unwrap();
    w.wait("the export", |s| {
        s.rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Notice { text, .. } if text.starts_with("Exported the candidate")))
    });
    assert_eq!(
        fs::read_to_string(export.join("src/lib.rs")).unwrap(),
        "// manual\n"
    );
    assert_eq!(w.source("src/lib.rs"), original);
    // The candidate's staged playback worker is retained while the review waits.
    assert_eq!(w.engine_state(), Some(TaskState::CandidateReady));
    w.wf().apply().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    assert_eq!(w.source("src/lib.rs"), "// manual\n");
    assert_eq!(w.snap().history.len(), 1);
    w.assert_clean();
}

#[test]
fn discarding_a_reviewed_candidate_keeps_the_source_and_the_working_copy() {
    let w = World::new();
    w.wf()
        .set_review_policy(studio_engine::ReviewPolicy::ManualReview)
        .unwrap();
    w.wait("the policy", |s| {
        s.review_policy == studio_engine::ReviewPolicy::ManualReview
    });
    let original = w.source("src/lib.rs");
    w.submit(&good("discard me"));
    w.wait_phase(TaskPhase::AwaitingReview);
    // Stop is a notice, not a discard, while only a review is pending.
    w.wf().stop().unwrap();
    w.wait("the stop notice", |s| {
        s.rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Notice { text, .. } if text.starts_with("Nothing is running")))
    });
    assert_eq!(
        w.snap().task.as_ref().unwrap().phase,
        TaskPhase::AwaitingReview
    );
    w.wf().discard().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Cancelled);
    assert_eq!(w.source("src/lib.rs"), original);
    assert!(matches!(
        task.draft_state,
        Some(DraftState::Retained { .. })
    ));
    assert!(
        w.snap()
            .rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Outcome(o) if o.kind == OutcomeKind::Discarded))
    );
    // The retained working copy can be exported once no writer can remain.
    let export = w._temp.path().join("draft-export");
    w.wf().export_draft(&export).unwrap();
    w.wait("the draft export", |s| {
        s.rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Notice { text, .. } if text.starts_with("Exported the working copy")))
    });
    assert_eq!(
        fs::read_to_string(export.join("src/lib.rs")).unwrap(),
        "// discard me\n"
    );
    w.assert_clean();
}

#[test]
fn a_policy_change_mid_task_only_affects_the_next_task() {
    let w = World::new();
    let mut script = good("running");
    script["wait_file"] = json!("go");
    w.submit(&script);
    w.wait_phase(TaskPhase::Editing);
    w.wait_prompts(1);
    w.wf()
        .set_review_policy(studio_engine::ReviewPolicy::ManualReview)
        .unwrap();
    let s = w.wait("the policy", |s| {
        s.review_policy == studio_engine::ReviewPolicy::ManualReview
    });
    assert_eq!(
        s.task.as_ref().unwrap().review_policy,
        studio_engine::ReviewPolicy::AutoApply,
        "the running task froze auto-Apply when it started"
    );
    w.release("go");
    let first = w.wait_task(None);
    assert_eq!(
        first.phase,
        TaskPhase::Accepted,
        "the running task still auto-applied"
    );
    w.submit(&good("next"));
    let second = w.wait_phase(TaskPhase::AwaitingReview);
    assert_ne!(second.id, first.id);
    assert_eq!(
        second.review_policy,
        studio_engine::ReviewPolicy::ManualReview
    );
}

// ---- queueing --------------------------------------------------------------------------------------

#[test]
fn a_second_edit_waits_for_the_first_writer_and_starts_from_the_published_source() {
    let w = World::new();
    let mut first = good("one");
    first["wait_file"] = json!("go");
    w.submit(&first);
    w.wait_phase(TaskPhase::Editing);
    w.wait_prompts(1);
    w.submit(&good("two"));
    let s = w.wait("the queued brief", |s| s.queue.len() == 1);
    assert_eq!(
        s.task.as_ref().unwrap().phase,
        TaskPhase::Editing,
        "the first task still runs"
    );
    assert!(s.queue[0].summary.contains("SCRIPT"));
    assert_eq!(
        w.evidence("prompts.jsonl").len(),
        1,
        "the second task did not start yet"
    );
    w.release("go");
    let s = w.wait("both edits accepted", |s| {
        s.history.len() == 2
            && s.task
                .as_ref()
                .is_some_and(|t| t.phase == TaskPhase::Accepted)
            && s.queue.is_empty()
    });
    assert_eq!(w.source("src/lib.rs"), "// two\n");
    // The second task was frozen against the first one's published revision.
    assert_eq!(s.task.as_ref().unwrap().source_base, s.history[0].published);
    let prompts = w.evidence("prompts.jsonl");
    assert_eq!(prompts.len(), 2);
    assert_eq!(
        prompts[0]["cwd"], prompts[1]["cwd"],
        "the stable draft cwd is reused"
    );
    w.assert_clean();
}

#[test]
fn the_queue_is_bounded_cancellable_and_cleared_by_stop() {
    let w = World::new();
    w.submit(&json!({"hang": true}));
    w.wait_phase(TaskPhase::Editing);
    let mut ids = Vec::new();
    for i in 0..MAX_QUEUED_BRIEFS {
        ids.push(w.wf().submit(&format!("queued {i}")).unwrap());
    }
    assert_eq!(
        w.wf().submit("one too many").unwrap_err(),
        WorkflowError::QueueFull
    );
    w.wait("the full queue", |s| s.queue.len() == MAX_QUEUED_BRIEFS);
    w.wf().cancel_queued(ids[0]).unwrap();
    w.wait("a cancelled queue entry", |s| {
        s.queue.len() == MAX_QUEUED_BRIEFS - 1
    });
    w.wf().submit("fits again").unwrap();
    w.wf().stop().unwrap();
    let s = w.wait("the cleared queue", |s| {
        s.queue.is_empty()
            && s.task
                .as_ref()
                .is_some_and(|t| t.phase == TaskPhase::Cancelled)
    });
    assert_eq!(s.task.as_ref().unwrap().phase, TaskPhase::Cancelled);
    // Nothing queued behind it ever started: an actor round trip (a refresh dispatched
    // after Stop and whatever Stop finished) is the barrier, not a sleep.
    let revision = s.revision;
    w.wf().refresh().unwrap();
    let after = w.wait("an actor round trip", |s| s.revision > revision);
    assert!(after.queue.is_empty());
    assert_eq!(
        after.task.as_ref().map(|t| &t.id),
        s.task.as_ref().map(|t| &t.id),
        "no new task replaced the stopped one"
    );
    assert_eq!(w.evidence("prompts.jsonl").len(), 1);
    w.assert_clean();
}

// ---- Undo ------------------------------------------------------------------------------------------------

#[test]
fn undo_is_validated_published_and_handed_off_like_an_apply() {
    let w = World::new();
    let original = w.source("src/lib.rs");
    w.submit(&good("to undo"));
    w.wait_task(None);
    assert_eq!(w.source("src/lib.rs"), "// to undo\n");
    w.wf().undo(None).unwrap();
    let s = w.wait("the Undo", |s| {
        s.history.len() == 2 && matches!(s.undo, UndoView::Unavailable { .. })
    });
    assert_eq!(w.source("src/lib.rs"), original);
    assert_eq!(s.history[1].kind, "Undo");
    assert_eq!(
        s.history[1].undoes.as_deref(),
        Some(s.history[0].id.as_str())
    );
    assert_eq!(s.handoff.as_ref().unwrap().kind, HandoffKind::Undo);
    w.assert_clean();
}

#[test]
fn undo_without_an_accepted_edit_explains_itself() {
    let w = World::new();
    assert!(
        matches!(&w.snap().undo, UndoView::Unavailable { reason } if reason.contains("No agent edit"))
    );
    w.wf().undo(None).unwrap();
    w.wait("the refusal", |s| has_error(s, "undo_unavailable"));
}

// ---- validation failure and repair -----------------------------------------------------------------------------

#[test]
fn a_compiler_failure_gets_structured_repair_context_and_exactly_one_repair_turn() {
    let w = World::new();
    w.submit_plan(&[
        broken(),
        json!({"text": "fixed", "write": {"src/lib.rs": "// fixed\n"}}),
    ]);
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    assert_eq!(task.repair.used, 1);
    assert_eq!(task.repair.max, 1);
    assert!(task.repair.context_summary.is_some());
    assert_eq!(w.source("src/lib.rs"), "// fixed\n");
    let prompts = w.evidence("prompts.jsonl");
    assert_eq!(
        prompts.len(),
        2,
        "one first turn and exactly one repair turn"
    );
    let repair_text = prompts[1]["text"].as_str().unwrap();
    assert!(repair_text.contains("failed validation"), "{repair_text}");
    assert!(
        repair_text.contains("BROKEN"),
        "the compiler output is in the context"
    );
    // A fresh adapter session/process served the repair (the old writer was reaped).
    assert_ne!(prompts[0]["pid"], prompts[1]["pid"]);
    let s = w.snap();
    assert!(s.rows.iter().any(|r| matches!(
        &r.kind,
        RowKind::User {
            source: UserSource::Repair,
            ..
        }
    )));
    let cards: Vec<&ValidationCard> = s
        .rows
        .iter()
        .filter_map(|r| match &r.kind {
            RowKind::Validation(c) => Some(&**c),
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 2);
    assert!(!cards[0].passed && cards[0].failure.is_some());
    assert!(cards[1].passed);
    assert_eq!(cards[1].repair_count, 1);
    w.assert_clean();
}

#[test]
fn a_second_failure_is_terminal_with_the_draft_and_candidate_retained() {
    let w = World::new();
    let original = w.source("src/lib.rs");
    w.submit_plan(&[broken(), broken()]);
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Failed);
    assert_eq!(task.repair.used, 1);
    assert_eq!(task.error.as_ref().unwrap().code, "validation_failed");
    assert!(task.error.as_ref().unwrap().retained);
    assert_eq!(
        w.evidence("prompts.jsonl").len(),
        2,
        "no third turn is ever started"
    );
    assert_eq!(w.source("src/lib.rs"), original);
    assert!(matches!(
        task.draft_state,
        Some(DraftState::Retained { .. })
    ));
    assert_eq!(w.snap().history.len(), 0);
    w.assert_clean();
}

// ---- Stop in every state ------------------------------------------------------------------------------------------

#[test]
fn stop_during_editing_reaps_the_adapter_and_retains_the_draft() {
    let w = World::new();
    w.submit(&json!({"text": "starting", "write": {"partial.txt": "half"}, "hang": true}));
    w.wait_phase(TaskPhase::Editing);
    w.poll("the partial write", || {
        w.ctl()
            .lock()
            .agent_draft_store()
            .path()
            .join("partial.txt")
            .exists()
    });
    w.wf().stop().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Cancelled);
    assert!(task.stop_requested);
    assert_eq!(w.engine_state(), Some(TaskState::Cancelled));
    assert!(matches!(w.draft_state(), Some(DraftState::Retained { .. })));
    assert!(
        w.agent.join("cancel.json").exists(),
        "the prompt was cancelled before reaping"
    );
    assert!(
        w.ctl()
            .lock()
            .agent_draft_store()
            .path()
            .join("partial.txt")
            .exists()
    );
    assert_eq!(w.snap().history.len(), 0);
    w.assert_clean();
}

#[test]
fn stop_while_a_permission_is_open_closes_it_as_cancelled() {
    let w = World::new();
    w.submit(&json!({"permission": {"title": "Edit title"}, "write": {"x.txt": "x"}}));
    let s = w.wait_phase_snapshot(TaskPhase::WaitingPermission);
    assert_eq!(s.open_permissions().len(), 1);
    assert_eq!(s.task.as_ref().unwrap().engine_state, TaskState::Waiting);
    let reference = s.open_permissions()[0].reference.clone();
    w.wf().stop().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Cancelled);
    let s = w.snap();
    assert!(s.open_permissions().is_empty());
    assert!(s.rows.iter().any(
        |r| matches!(&r.kind, RowKind::Permission(c) if c.state == PermissionState::Cancelled)
    ));
    // The agent saw the cancelled outcome; a late click is stale, never delivered.
    let outcome = fs::read_to_string(w.agent.join("permission-perm-1.json")).unwrap();
    assert!(outcome.contains("cancelled"), "{outcome}");
    assert_eq!(
        w.wf()
            .reply_permission(&reference, PermissionAnswer::Select("allow".into()))
            .unwrap_err(),
        WorkflowError::StalePermission
    );
    w.assert_clean();
}

#[test]
fn stop_while_waiting_for_an_answer_ends_the_task() {
    let w = World::new();
    w.submit(&json!({"text": "Which title should I use?"}));
    let task = w.wait_phase(TaskPhase::WaitingClarification);
    assert_eq!(task.engine_state, TaskState::Waiting);
    w.wf().stop().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Cancelled);
    assert_eq!(
        w.wf()
            .reply_clarification(&task.id, "too late")
            .unwrap_err(),
        WorkflowError::NotWaiting
    );
    w.assert_clean();
}

#[test]
fn stop_while_validating_cancels_the_compile_and_keeps_the_draft() {
    let w = World::with(Options {
        block_compiler: true,
        ..Options::default()
    });
    w.submit(&good("never compiled"));
    w.compiler.wait_started(1);
    w.wait_phase(TaskPhase::Validating);
    w.wf().stop().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Cancelled);
    assert_eq!(w.engine_state(), Some(TaskState::Cancelled));
    assert!(matches!(w.draft_state(), Some(DraftState::Retained { .. })));
    assert!(
        w.compiler.killed.load(Ordering::SeqCst) >= 1,
        "the shared compile saw the cancel"
    );
    assert_eq!(w.snap().history.len(), 0);
    w.compiler.release.store(true, Ordering::SeqCst);
    w.assert_clean();
}

#[test]
fn stop_during_the_repair_turn_ends_the_task() {
    let w = World::new();
    w.submit_plan(&[broken(), json!({"hang": true})]);
    let task = w.wait_phase(TaskPhase::Repairing);
    assert_eq!(task.repair.used, 1);
    assert!(task.repair.in_progress);
    w.wait_prompts(2);
    w.wf().stop().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Cancelled);
    assert_eq!(task.repair.used, 1);
    assert!(matches!(w.draft_state(), Some(DraftState::Retained { .. })));
    w.assert_clean();
}

// ---- permissions and questions -----------------------------------------------------------------------------------------

#[test]
fn permission_replies_are_correlated_and_stale_or_duplicate_ones_are_rejected() {
    let w = World::new();
    let mut script = good("permitted");
    script["permission"] = json!({"title": "Edit title"});
    w.submit(&script);
    let s = w.wait_phase_snapshot(TaskPhase::WaitingPermission);
    let card = s.open_permissions()[0].clone();
    assert_eq!(
        card.options
            .iter()
            .map(|o| o.option_id.as_str())
            .collect::<Vec<_>>(),
        ["allow", "reject"]
    );
    // An option the request never offered.
    assert_eq!(
        w.wf()
            .reply_permission(&card.reference, PermissionAnswer::Select("nope".into()))
            .unwrap_err(),
        WorkflowError::UnknownChoice("nope".into())
    );
    // A request of another task / agent session.
    let mut foreign = card.reference.clone();
    foreign.task = AgentTaskId::new();
    assert_eq!(
        w.wf()
            .reply_permission(&foreign, PermissionAnswer::Select("allow".into()))
            .unwrap_err(),
        WorkflowError::StalePermission
    );
    let mut other_writer = card.reference.clone();
    other_writer.writer += 1;
    assert_eq!(
        w.wf()
            .reply_permission(&other_writer, PermissionAnswer::Select("allow".into()))
            .unwrap_err(),
        WorkflowError::StalePermission
    );
    w.wf()
        .reply_permission(&card.reference, PermissionAnswer::Select("allow".into()))
        .unwrap();
    assert_eq!(
        w.wf()
            .reply_permission(&card.reference, PermissionAnswer::Select("allow".into()))
            .unwrap_err(),
        WorkflowError::DuplicatePermission
    );
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    let s = w.snap();
    assert!(
        s.rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Permission(c)
        if c.state == PermissionState::Selected { option_id: "allow".into() }))
    );
    assert_eq!(
        w.wf()
            .reply_permission(&card.reference, PermissionAnswer::Select("allow".into()))
            .unwrap_err(),
        WorkflowError::DuplicatePermission
    );
    w.assert_clean();
}

#[test]
fn a_rejected_permission_lets_the_agent_finish_without_changes_and_the_user_answers() {
    let w = World::new();
    w.submit_plan(&[
        json!({"permission": {"title": "Edit title"}, "write": {"src/lib.rs": "// nope\n"}, "text": "I could not edit"}),
        good("after question"),
    ]);
    let s = w.wait_phase_snapshot(TaskPhase::WaitingPermission);
    let card = s.open_permissions()[0].clone();
    w.wf()
        .reply_permission(&card.reference, PermissionAnswer::Select("reject".into()))
        .unwrap();
    // No file changed: the turn is a question the user must answer.
    let task = w.wait_phase(TaskPhase::WaitingClarification);
    assert_ne!(w.source("src/lib.rs"), "// nope\n");
    w.wf()
        .reply_clarification(&task.id, "Go ahead and edit it")
        .unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    assert_eq!(w.source("src/lib.rs"), "// after question\n");
}

#[test]
fn a_clarification_is_a_subsequent_prompt_in_the_same_provider_session() {
    let w = World::new();
    w.submit_plan(&[json!({"text": "Which title should I use?"}), good("titled")]);
    let task = w.wait_phase(TaskPhase::WaitingClarification);
    assert_eq!(
        w.wf()
            .reply_clarification(&AgentTaskId::new(), "wrong task")
            .unwrap_err(),
        WorkflowError::NotWaiting
    );
    w.wf().reply_clarification(&task.id, "Use Title A").unwrap();
    assert_eq!(
        w.wf().reply_clarification(&task.id, "again").unwrap_err(),
        WorkflowError::NotWaiting,
        "the answer was claimed"
    );
    let done = w.wait_task(None);
    assert_eq!(done.phase, TaskPhase::Accepted, "{:?}", done.error);
    assert_eq!(done.turns, 2);
    let prompts = w.evidence("prompts.jsonl");
    assert_eq!(prompts.len(), 2);
    assert_eq!(
        prompts[0]["session"], prompts[1]["session"],
        "one provider session"
    );
    assert_eq!(prompts[0]["pid"], prompts[1]["pid"], "one adapter process");
    assert!(
        prompts[1]["text"]
            .as_str()
            .unwrap()
            .starts_with("Use Title A")
    );
    assert_eq!(w.evidence("session-params.json").len().max(1), 1);
    assert!(w.snap().rows.iter().any(|r| matches!(&r.kind, RowKind::User { source: UserSource::Clarification, text } if text == "Use Title A")));
    w.assert_clean();
}

// ---- failures, containment, conflicts --------------------------------------------------------------------------------------------

#[test]
fn a_provider_crash_fails_the_task_with_the_draft_retained_and_no_writer() {
    let w = World::new();
    let original = w.source("src/lib.rs");
    w.submit(
        &json!({"text": "about to crash", "write": {"src/lib.rs": "// crashed\n"}, "crash": 7}),
    );
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Failed);
    let error = task.error.as_ref().unwrap();
    assert!(error.code.starts_with("provider_"), "{}", error.code);
    assert!(error.retained);
    assert!(error.action.is_some());
    assert_eq!(
        w.source("src/lib.rs"),
        original,
        "a crash never reaches the project"
    );
    assert_eq!(w.engine_state(), Some(TaskState::Failed));
    assert!(matches!(
        task.draft_state,
        Some(DraftState::Retained { .. })
    ));
    assert!(
        w.ctl()
            .lock()
            .agent_draft_store()
            .path()
            .join("src/lib.rs")
            .exists()
    );
    assert_eq!(w.snap().history.len(), 0);
    w.assert_clean();
}

#[test]
fn an_adapter_that_dies_during_startup_fails_the_task_cleanly() {
    let w = World::with(Options {
        wrapper: vec!["env".into(), "SCRIPTED_AGENT_INIT=exit".into()],
        ..Options::default()
    });
    w.submit(&good("never"));
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Failed);
    assert!(task.error.as_ref().unwrap().code.starts_with("provider_"));
    assert!(w.evidence("prompts.jsonl").is_empty());
    w.assert_clean();
}

#[test]
fn an_unqualified_adapter_blocks_capture_and_retains_the_draft_locked() {
    let w = World::with(Options {
        ownership: WriterOwnership::Unknown,
        ..Options::default()
    });
    let original = w.source("src/lib.rs");
    w.submit(&good("unqualified"));
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Failed);
    assert_eq!(task.error.as_ref().unwrap().code, "quiescence_blocked");
    assert_eq!(w.source("src/lib.rs"), original);
    assert!(matches!(
        task.draft_state,
        Some(DraftState::UnsafeWriter { .. })
    ));
    w.assert_clean();
}

#[test]
fn an_escaped_writer_blocks_apply_retains_the_draft_and_locks_it_until_acknowledged() {
    let w = World::new();
    let original = w.source("src/lib.rs");
    let mut script = good("escaped");
    script["escape"] = json!(true);
    w.submit(&script);
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Failed);
    assert_eq!(
        task.error.as_ref().unwrap().code,
        "quiescence_blocked",
        "{:?}",
        task.error
    );
    assert_eq!(w.source("src/lib.rs"), original, "nothing was applied");
    assert_eq!(w.snap().history.len(), 0);
    assert!(matches!(
        task.draft_state,
        Some(DraftState::UnsafeWriter { .. })
    ));
    let pids: Vec<i32> = fs::read_to_string(w.agent.join("escape.pid"))
        .unwrap()
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect();
    assert!(
        !pids.is_empty() && pids.iter().all(|p| pid_alive(*p)),
        "the escaped helper is still running"
    );
    // The next task is refused while the draft may still be written.
    w.wf().submit(&World::brief(&good("blocked"))).unwrap();
    w.wait("the draft lock", |s| has_error(s, "draft_unsafe"));
    assert!(w.snap().recovery.draft.as_ref().unwrap().can_acknowledge);
    assert_eq!(
        w.snap().task.as_ref().unwrap().id,
        task.id,
        "no new task was created"
    );
    // The user confirms after dealing with the helper; then work continues.
    for pid in &pids {
        unsafe { libc::kill(*pid, libc::SIGKILL) };
    }
    w.wf().acknowledge_writer_gone().unwrap();
    w.wait("the unlocked draft", |s| {
        matches!(
            s.recovery.draft.as_ref().map(|d| &d.state),
            Some(DraftState::Retained { .. })
        )
    });
    w.submit(&good("after unlock"));
    let next = w.wait_task(Some(&task.id));
    assert_eq!(next.phase, TaskPhase::Accepted, "{:?}", next.error);
}

#[test]
fn a_plain_helper_inside_the_process_group_is_reaped_and_does_not_block_capture() {
    let w = World::new();
    let mut script = good("with helper");
    script["helper"] = json!(true);
    w.submit(&script);
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    let pid: i32 = fs::read_to_string(w.agent.join("helper.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_alive(pid) && Instant::now() < deadline {
        // Zombies of a killed non-child still answer kill(0) until reaped by init.
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        if stat.is_empty() || stat.contains(") Z ") {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    assert!(
        stat.is_empty() || stat.contains(") Z "),
        "the helper outlived the task: {stat}"
    );
    w.assert_clean();
}

#[test]
fn a_source_edit_during_review_ends_in_conflict_with_paths_and_the_candidate_kept() {
    let w = World::new();
    w.wf()
        .set_review_policy(studio_engine::ReviewPolicy::ManualReview)
        .unwrap();
    w.wait("the policy", |s| {
        s.review_policy == studio_engine::ReviewPolicy::ManualReview
    });
    w.submit(&good("candidate"));
    w.wait_phase(TaskPhase::AwaitingReview);
    fs::write(w.root.join("src/lib.rs"), "// an editor changed this\n").unwrap();
    w.wf().apply().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Conflict, "{:?}", task.error);
    let conflict = task.conflict.as_ref().expect("conflict paths");
    assert!(
        conflict.external_paths.iter().any(|p| p == "src/lib.rs"),
        "{conflict:?}"
    );
    assert!(conflict.candidate_paths.iter().any(|p| p == "src/lib.rs"));
    assert!(conflict.overlapping.iter().any(|p| p == "src/lib.rs"));
    assert_eq!(task.error.as_ref().unwrap().code, "source_conflict");
    assert_eq!(
        w.source("src/lib.rs"),
        "// an editor changed this\n",
        "the editor's bytes stay"
    );
    assert_eq!(w.snap().history.len(), 0);
    assert!(matches!(
        task.draft_state,
        Some(DraftState::Retained { .. })
    ));
    w.assert_clean();
}

// ---- close -----------------------------------------------------------------------------------------------------------------

#[test]
fn closing_the_workflow_mid_task_reaps_everything() {
    let w = World::new();
    w.submit(&json!({"hang": true}));
    w.wait_phase(TaskPhase::Editing);
    w.wait_prompts(1);
    assert_eq!(w.snap().resources.broker_grants, 1);
    w.wf().close();
    assert_eq!(w.processes(), 0);
    assert_eq!(w.engine_state(), Some(TaskState::Interrupted));
    assert!(matches!(w.draft_state(), Some(DraftState::Retained { .. })));
    let s = w.snap();
    assert!(s.closed);
    assert_eq!(s.task.as_ref().unwrap().phase, TaskPhase::Interrupted);
    assert_eq!(s.resources.broker_grants, 0);
    assert!(
        fs::read_dir(w.runtime.path().join("rt"))
            .map(|d| d.count())
            .unwrap_or(0)
            == 0,
        "no socket or capability file remains"
    );
    assert_eq!(w.service.stats().leased_entries, 0);
    w.wf().close();
}

#[test]
fn closing_during_validation_and_during_review_leaves_nothing_behind() {
    let w = World::with(Options {
        block_compiler: true,
        ..Options::default()
    });
    w.submit(&good("closing"));
    w.compiler.wait_started(1);
    w.wait_phase(TaskPhase::Validating);
    w.wf().close();
    assert_eq!(w.processes(), 0);
    assert_eq!(w.engine_state(), Some(TaskState::Interrupted));
    assert_eq!(w.service.stats().in_flight, 0);

    let w = World::new();
    w.wf()
        .set_review_policy(studio_engine::ReviewPolicy::ManualReview)
        .unwrap();
    w.wait("the policy", |s| {
        s.review_policy == studio_engine::ReviewPolicy::ManualReview
    });
    w.submit(&good("review then close"));
    w.wait_phase(TaskPhase::AwaitingReview);
    assert!(
        w.processes() > 0,
        "the staged preview worker is alive during review"
    );
    w.wf().close();
    assert_eq!(w.processes(), 0);
    assert_eq!(w.engine_state(), Some(TaskState::Interrupted));
    w.assert_clean();
}

// ---- restart and recovery ---------------------------------------------------------------------------------------------------------

#[test]
fn an_interrupted_task_is_recovered_on_open_with_its_draft_locked_until_confirmed() {
    let mut w = World::new();
    w.submit(&json!({"text": "partial", "write": {"scratch.txt": "partial"}, "hang": true}));
    w.wait_phase(TaskPhase::Editing);
    w.poll("the partial write", || {
        w.ctl()
            .lock()
            .agent_draft_store()
            .path()
            .join("scratch.txt")
            .exists()
    });
    let task = w.snap().task.clone().unwrap();
    w.wf().stop().unwrap();
    w.wait_task(None);
    // The app died mid-task: the marker still says Active.
    let project = w.ctl().lock().project.manifest.project_id.clone();
    let marker = w.paths.agent_draft_state(&project);
    let mut value: Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    value["state"] = json!({"state": "active", "task": task.id.0});
    fs::write(&marker, serde_json::to_vec(&value).unwrap()).unwrap();
    w.restart();
    let s = w.snap();
    let notice = s.recovery.draft.as_ref().expect("recovery draft notice");
    assert!(notice.interrupted_by_restart && notice.can_acknowledge);
    assert!(matches!(notice.state, DraftState::UnsafeWriter { .. }));
    assert!(s.rows.iter().any(
        |r| matches!(&r.kind, RowKind::Outcome(o) if o.kind == OutcomeKind::Interrupted)
            && r.task.as_ref() == Some(&task.id)
    ));
    // The conversation of the earlier session is still there (persisted rows).
    assert!(s.rows.iter().any(|r| matches!(
        &r.kind,
        RowKind::User {
            source: UserSource::Brief,
            ..
        }
    )));
    // The retained draft is not refreshed while it may still be written.
    w.submit(&good("blocked"));
    w.wait("the draft lock", |s| has_error(s, "draft_unsafe"));
    w.wf().acknowledge_writer_gone().unwrap();
    w.wait("unlocked", |s| {
        matches!(
            s.recovery.draft.as_ref().map(|d| &d.state),
            Some(DraftState::Retained { .. })
        )
    });
    w.submit(&good("recovered"));
    let next = w.wait_task(None);
    assert_eq!(next.phase, TaskPhase::Accepted, "{:?}", next.error);
    // Opening twice does not duplicate the interrupted outcome.
    let interrupted = w
        .snap()
        .rows
        .iter()
        .filter(|r| matches!(&r.kind, RowKind::Outcome(o) if o.kind == OutcomeKind::Interrupted))
        .count();
    assert_eq!(interrupted, 1);
}

struct CrashAtPublish {
    armed: AtomicBool,
}

impl TransactionHooks for CrashAtPublish {
    fn at(&self, boundary: &Boundary) -> Result<(), Fault> {
        if self.armed.load(Ordering::SeqCst) && matches!(boundary, Boundary::AfterPublish(_)) {
            return Err(Fault::Crash);
        }
        Ok(())
    }
}

#[test]
fn an_interrupted_publication_is_rolled_back_by_recovery_and_work_continues() {
    let hooks = Arc::new(CrashAtPublish {
        armed: AtomicBool::new(true),
    });
    let mut w = World::with(Options {
        hooks: Some(hooks.clone()),
        ..Options::default()
    });
    let original = w.source("src/lib.rs");
    w.submit(&good("half published"));
    let task = w.wait_task(None);
    assert_ne!(
        task.phase,
        TaskPhase::Accepted,
        "the process 'died' mid-publication"
    );
    assert!(
        matches!(
            task.error.as_ref().map(|e| e.code.as_str()),
            Some("promotion_failed")
        ),
        "{:?}",
        task.error
    );
    assert_eq!(w.snap().history.len(), 0);
    hooks.armed.store(false, Ordering::SeqCst);
    w.restart();
    let s = w.snap();
    assert_eq!(
        s.recovery.rolled_back.len(),
        1,
        "recovery rolled the transaction back"
    );
    assert_eq!(
        w.source("src/lib.rs"),
        original,
        "the half-published file was restored"
    );
    assert!(s.history.is_empty());
    w.submit(&good("after recovery"));
    let next = w.wait_task(None);
    assert_eq!(next.phase, TaskPhase::Accepted, "{:?}", next.error);
}

#[test]
fn an_interrupted_undo_is_rolled_back_by_recovery_and_stays_available() {
    let hooks = Arc::new(CrashAtPublish {
        armed: AtomicBool::new(false),
    });
    let mut w = World::with(Options {
        hooks: Some(hooks.clone()),
        ..Options::default()
    });
    w.submit(&good("kept"));
    w.wait_task(None);
    assert_eq!(w.snap().history.len(), 1);
    hooks.armed.store(true, Ordering::SeqCst);
    w.wf().undo(None).unwrap();
    w.wait("the failed Undo", |s| has_error(s, "promotion_failed"));
    hooks.armed.store(false, Ordering::SeqCst);
    w.restart();
    let s = w.snap();
    assert_eq!(s.recovery.rolled_back.len(), 1);
    assert_eq!(
        w.source("src/lib.rs"),
        "// kept\n",
        "the accepted edit survived the interrupted Undo"
    );
    assert_eq!(s.history.len(), 1);
    assert!(matches!(s.undo, UndoView::Available { .. }));
}

struct BlockAtPublish {
    reached: AtomicBool,
    release: AtomicBool,
}

impl TransactionHooks for BlockAtPublish {
    fn at(&self, boundary: &Boundary) -> Result<(), Fault> {
        if matches!(boundary, Boundary::BeforePublish(_)) && !self.release.load(Ordering::SeqCst) {
            self.reached.store(true, Ordering::SeqCst);
            while !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        Ok(())
    }
}

#[test]
fn stop_during_publication_cannot_cancel_the_commit_and_reports_it() {
    let hooks = Arc::new(BlockAtPublish {
        reached: AtomicBool::new(false),
        release: AtomicBool::new(false),
    });
    let w = World::with(Options {
        hooks: Some(hooks.clone()),
        ..Options::default()
    });
    w.submit(&good("boundary"));
    let deadline = Instant::now() + WAIT;
    while !hooks.reached.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "publication never started");
        std::thread::sleep(Duration::from_millis(5));
    }
    let task = w.wait_phase(TaskPhase::Promoting);
    assert_eq!(task.engine_state, TaskState::Promoting);
    w.wf().stop().unwrap();
    w.wait("the explanation", |s| {
        s.rows.iter().any(|r| {
            matches!(&r.kind, RowKind::Notice { text, .. } if text.starts_with("The edit is being published"))
        })
    });
    assert_eq!(w.snap().task.as_ref().unwrap().phase, TaskPhase::Promoting);
    hooks.release.store(true, Ordering::SeqCst);
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    assert_eq!(
        w.source("src/lib.rs"),
        "// boundary\n",
        "the commit was never cancelled"
    );
    let s = w.snap();
    assert!(s.rows.iter().any(
        |r| matches!(&r.kind, RowKind::Notice { level: NoticeLevel::Warning, text }
        if text.starts_with("Stop arrived while the edit was being published"))
    ));
    assert_eq!(s.history.len(), 1);
    assert!(
        matches!(s.undo, UndoView::Available { .. }),
        "Undo is the recovery"
    );
    w.assert_clean();
}

#[test]
fn a_blocked_publication_gate_retains_the_candidate_and_apply_can_be_retried() {
    let w = World::new();
    w.ctl()
        .lock()
        .override_apply_gate(Some(studio_engine::ApplyGate::Blocked(
            "test filesystem".into(),
        )));
    let original = w.source("src/lib.rs");
    w.submit(&good("blocked gate"));
    let task = w.wait_phase(TaskPhase::AwaitingReview);
    let review = task.review.as_ref().unwrap();
    assert!(
        review
            .apply_blocked
            .as_deref()
            .is_some_and(|r| r.contains("test filesystem")),
        "{review:?}"
    );
    assert!(
        has_error(&w.snap(), "apply_blocked"),
        "{:?}",
        w.snap().task.as_ref().and_then(|t| t.error.clone())
    );
    assert_eq!(w.source("src/lib.rs"), original);
    assert_eq!(w.engine_state(), Some(TaskState::CandidateReady));
    // Export still works while Apply is blocked.
    let export = w._temp.path().join("blocked-export");
    w.wf().export_candidate(&export).unwrap();
    w.wait("the export", |s| {
        s.rows.iter().any(|r| matches!(&r.kind, RowKind::Notice { text, .. } if text.starts_with("Exported the candidate")))
    });
    w.ctl().lock().override_apply_gate(None);
    w.wf().apply().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    assert_eq!(w.source("src/lib.rs"), "// blocked gate\n");
    w.assert_clean();
}

// ---- preview handoff -----------------------------------------------------------------------------------------------------------------------

struct Sink {
    coordinator: Mutex<Option<Arc<PreviewCoordinator>>>,
    fail: AtomicBool,
    adopted: Mutex<Vec<String>>,
}

impl PreviewHandoff for Sink {
    fn handoff(&self, handoff: PromotionHandoff) -> Result<(), String> {
        if self.fail.load(Ordering::SeqCst) {
            return Err("preview worker refused".into());
        }
        let staged = handoff.staged.ok_or("no staged preview")?;
        let authorization = handoff
            .promotion
            .authorization
            .as_ref()
            .ok_or("the source changed again")?;
        let identity = staged.ready().identity().clone();
        let coordinator = self
            .coordinator
            .lock()
            .clone()
            .ok_or("the coordinator is gone")?;
        coordinator
            .adopt(
                staged,
                authorization,
                SeekIntent {
                    identity,
                    serial: 0,
                    position: 5,
                    scale: 1.,
                },
            )
            .map_err(|e| e.to_string())?;
        self.adopted
            .lock()
            .push(handoff.promotion.record.published.as_str().to_owned());
        Ok(())
    }
}

fn wait_ready(p: &PreviewCoordinator) -> Arc<studio_engine::ReadyPreview> {
    let deadline = Instant::now() + WAIT;
    loop {
        let e = p.events();
        if let Some((_, error)) = e.error {
            panic!("preview failed: {error}");
        }
        if let Some(ready) = e.ready {
            return ready;
        }
        assert!(Instant::now() < deadline, "no ready preview");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_committed_revision_is_handed_to_the_preview_and_a_failed_handoff_is_exposed_honestly() {
    let owner = ProcessTreeManager::new();
    let sink = Arc::new(Sink {
        coordinator: Mutex::new(Some(Arc::new(PreviewCoordinator::new(owner.sub_manager())))),
        fail: AtomicBool::new(false),
        adopted: Mutex::new(Vec::new()),
    });
    let w = World::with(Options {
        handoff: Some(sink.clone()),
        ..Options::default()
    });
    w.submit(&good("handed off"));
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    let published = w.ctl().lock().state().source().as_str().to_owned();
    assert_eq!(
        w.snap().handoff.as_ref().unwrap().state,
        HandoffState::Adopted
    );
    let coordinator = sink.coordinator.lock().clone().unwrap();
    let ready = wait_ready(&coordinator);
    assert_eq!(ready.identity().source_revision, published);
    assert_eq!(
        sink.adopted.lock().as_slice(),
        std::slice::from_ref(&published)
    );
    // The UI reports what it displays; the awaiting label clears.
    w.wf()
        .preview_displayed("not-the-published-revision")
        .unwrap();
    w.wf().preview_displayed(&published).unwrap();
    w.wait("the displayed preview", |s| {
        s.handoff
            .as_ref()
            .is_some_and(|h| h.state == HandoffState::Displayed)
    });
    // A failing handoff keeps the old preview and accepts the source anyway.
    sink.fail.store(true, Ordering::SeqCst);
    w.submit(&good("not adopted"));
    let second = w.wait_task(Some(&task.id));
    assert_eq!(second.phase, TaskPhase::Accepted);
    assert_eq!(w.source("src/lib.rs"), "// not adopted\n");
    assert_eq!(
        w.snap().handoff.as_ref().unwrap().state,
        HandoffState::AwaitingPreview {
            reason: Some("preview worker refused".into())
        },
        "accepted-awaiting-preview is exposed with the reason"
    );
    assert_eq!(sink.adopted.lock().len(), 1);
    // A sink that adopts later (on another thread) reports the outcome afterwards.
    let published_two = w.ctl().lock().state().source().as_str().to_owned();
    w.wf()
        .report_handoff("some-other-revision", Ok(()))
        .unwrap();
    w.wf().report_handoff(&published_two, Ok(())).unwrap();
    w.wait("the reported adoption", |s| {
        s.handoff
            .as_ref()
            .is_some_and(|h| h.state == HandoffState::Adopted)
    });
    // Dropping the coordinator reaps the adopted worker it owns.
    drop(ready);
    let owned = sink.coordinator.lock().take().unwrap();
    drop(coordinator);
    owned.close();
    drop(owned);
    w.assert_clean();
}

// ---- resource bounds ----------------------------------------------------------------------------------------------------------------------------

#[test]
fn the_resident_transcript_is_bounded_and_older_rows_are_paged_from_the_log() {
    use fframes_studio::agent_workflow::log::RowLimits;
    let w = World::with(Options {
        row_limits: Some(RowLimits {
            max_rows: 20,
            ..RowLimits::default()
        }),
        ..Options::default()
    });
    let mut script = good("flooded");
    script["flood_tools"] = json!(150);
    w.submit(&script);
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    let s = w.snap();
    assert!(
        s.rows.len() <= 20 && s.resources.resident_rows <= 20,
        "{}",
        s.rows.len()
    );
    assert!(s.older_rows);
    assert!(s.resources.resident_bytes <= s.resources.max_resident_bytes);
    let first = s.rows[0].id;
    let page = w.wf().history_page(first, 1000).unwrap();
    assert_eq!(page.len(), MAX_HISTORY_PAGE, "pages are clamped");
    assert!(page.iter().all(|r| r.id < first));
    assert!(page.windows(2).all(|p| p[0].id < p[1].id));
    assert!(page.iter().any(|r| matches!(&r.kind, RowKind::Tool(_))));
    // Paging back reaches the persisted brief, the oldest row of all.
    let mut before = page[0].id;
    let mut oldest = page;
    loop {
        let older = w.wf().history_page(before, MAX_HISTORY_PAGE).unwrap();
        if older.is_empty() {
            break;
        }
        before = older[0].id;
        oldest = older;
    }
    assert!(matches!(
        &oldest[0].kind,
        RowKind::User {
            source: UserSource::Brief,
            ..
        }
    ));
}

#[test]
fn streaming_text_is_bounded_by_bytes_and_never_one_unbounded_row() {
    use fframes_studio::agent_workflow::log::RowLimits;
    let w = World::with(Options {
        row_limits: Some(RowLimits {
            max_bytes: 48 * 1024,
            ..RowLimits::default()
        }),
        ..Options::default()
    });
    let mut script = good("long");
    script["flood"] = json!(20000);
    w.submit(&script);
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    let s = w.snap();
    let biggest = s.rows.iter().map(|r| r.estimated_bytes()).max().unwrap();
    assert!(
        biggest <= MAX_ROW_TEXT_BYTES + ROW_OVERHEAD_BYTES + 64,
        "{biggest}"
    );
    // The newest row is never evicted; the rest fits the budget (plus that one row).
    assert!(s.resources.resident_bytes <= 48 * 1024 + MAX_ROW_TEXT_BYTES + ROW_OVERHEAD_BYTES);
    assert!(s.older_rows);
}

// ---- repeated cycles ----------------------------------------------------------------------------------------------------------------------------------

#[test]
fn twenty_four_edit_and_failed_task_cycles_leak_no_process_capability_or_lease() {
    let w = World::new();
    let mut last: Option<AgentTaskId> = None;
    let mut accepted = 0;
    for cycle in 0..24 {
        match cycle % 6 {
            0 | 1 => {
                w.submit(&good(&format!("cycle {cycle}")));
                let task = w.wait_task(last.as_ref());
                assert_eq!(
                    task.phase,
                    TaskPhase::Accepted,
                    "cycle {cycle}: {:?}",
                    task.error
                );
                accepted += 1;
                last = Some(task.id);
            }
            2 => {
                w.submit_plan(&[broken(), broken()]);
                let task = w.wait_task(last.as_ref());
                assert_eq!(task.phase, TaskPhase::Failed, "cycle {cycle}");
                last = Some(task.id);
            }
            3 => {
                w.submit(&json!({"write": {"c.txt": "x"}, "crash": 3}));
                let task = w.wait_task(last.as_ref());
                assert_eq!(task.phase, TaskPhase::Failed, "cycle {cycle}");
                last = Some(task.id);
            }
            4 => {
                let before = w.evidence("prompts.jsonl").len();
                w.submit(&json!({"hang": true}));
                w.wait_prompts(before + 1);
                w.wait_phase(TaskPhase::Editing);
                w.wf().stop().unwrap();
                let task = w.wait_task(last.as_ref());
                assert_eq!(task.phase, TaskPhase::Cancelled, "cycle {cycle}");
                last = Some(task.id);
            }
            _ => {
                w.submit(&json!({"permission": {"title": "Edit"}, "write": {"y.txt": "y"}}));
                w.wait_phase(TaskPhase::WaitingPermission);
                w.wf().stop().unwrap();
                let task = w.wait_task(last.as_ref());
                assert_eq!(task.phase, TaskPhase::Cancelled, "cycle {cycle}");
                last = Some(task.id);
            }
        }
        assert_eq!(w.processes(), 0, "cycle {cycle} left a process");
        let s = w.snap();
        assert_eq!(
            s.resources.broker_grants, 0,
            "cycle {cycle} left a capability"
        );
        assert_eq!(
            w.service.stats().leased_entries,
            0,
            "cycle {cycle} left a build lease"
        );
        assert!(s.resources.resident_rows <= MAX_RESIDENT_ROWS);
    }
    assert_eq!(w.snap().history.len(), accepted);
    w.wf().close();
    w.assert_clean();
    assert!(
        fs::read_dir(w.runtime.path().join("rt"))
            .map(|d| d.count())
            .unwrap_or(0)
            == 0
    );
}

// ---- Stage 4a review regressions ---------------------------------------------------------------------------------------------------------------------

fn escaped_pids(w: &World) -> Vec<i32> {
    fs::read_to_string(w.agent.join("escape.pid"))
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect()
}

fn kill_escaped(w: &World) {
    for pid in escaped_pids(w) {
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

/// Every row of the conversation, whether resident or paged back from the log.
fn every_row(w: &World) -> std::collections::BTreeMap<u64, Arc<Row>> {
    let mut all = std::collections::BTreeMap::new();
    let mut before = RowId(u64::MAX);
    loop {
        let page = w.wf().history_page(before, 100).unwrap();
        let Some(oldest) = page.first().map(|r| r.id) else {
            break;
        };
        before = oldest;
        for row in page {
            all.insert(row.id.0, row);
        }
    }
    for row in &w.snap().rows {
        all.insert(row.id.0, row.clone());
    }
    all
}

/// A successor is refused while the draft may still be written, until the user confirms.
fn assert_successor_refused_then_unlocked(w: &World, previous: &AgentTaskId) {
    assert!(
        matches!(w.draft_state(), Some(DraftState::UnsafeWriter { .. })),
        "{:?}",
        w.draft_state()
    );
    w.wf().submit(&World::brief(&good("blocked"))).unwrap();
    w.wait("the draft lock", |s| has_error(s, "draft_unsafe"));
    assert_eq!(
        w.snap().task.as_ref().unwrap().id,
        *previous,
        "no new task was created"
    );
    kill_escaped(w);
    w.wf().acknowledge_writer_gone().unwrap();
    w.wait("the unlocked draft", |s| {
        matches!(
            s.recovery.draft.as_ref().map(|d| &d.state),
            Some(DraftState::Retained { .. })
        )
    });
    w.submit(&good("after unlock"));
    let next = w.wait_task(Some(previous));
    assert_eq!(next.phase, TaskPhase::Accepted, "{:?}", next.error);
}

#[test]
fn an_escaped_writer_locks_the_draft_when_the_user_stops_the_task() {
    let w = World::new();
    w.submit(&json!({"escape": true, "hang": true}));
    w.wait_phase(TaskPhase::Editing);
    w.poll("the escaped helper", || w.agent.join("escape.pid").exists());
    w.wf().stop().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Cancelled);
    assert!(
        matches!(task.draft_state, Some(DraftState::UnsafeWriter { .. })),
        "{:?}",
        task.draft_state
    );
    assert_successor_refused_then_unlocked(&w, &task.id);
}

#[test]
fn an_escaped_writer_locks_the_draft_when_the_provider_fails() {
    let w = World::new();
    w.submit(&json!({"escape": true, "linger": true, "malformed": true}));
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Failed);
    assert!(
        task.error
            .as_ref()
            .is_some_and(|e| e.code.starts_with("provider_")),
        "{:?}",
        task.error
    );
    assert_successor_refused_then_unlocked(&w, &task.id);
}

#[test]
fn an_escaped_writer_locks_the_draft_when_the_project_closes() {
    let mut w = World::new();
    w.submit(&json!({"escape": true, "hang": true}));
    w.wait_phase(TaskPhase::Editing);
    w.poll("the escaped helper", || w.agent.join("escape.pid").exists());
    w.wf().close();
    assert!(
        matches!(w.draft_state(), Some(DraftState::UnsafeWriter { .. })),
        "{:?}",
        w.draft_state()
    );
    // A reopened project refuses a successor until the user confirms.
    w.restart();
    w.wf().submit(&World::brief(&good("blocked"))).unwrap();
    w.wait("the draft lock", |s| has_error(s, "draft_unsafe"));
    kill_escaped(&w);
    w.wf().acknowledge_writer_gone().unwrap();
    w.wait("the unlocked draft", |s| {
        matches!(
            s.recovery.draft.as_ref().map(|d| &d.state),
            Some(DraftState::Retained { .. })
        )
    });
    w.submit(&good("after unlock"));
    let next = w.wait_task(None);
    assert_eq!(next.phase, TaskPhase::Accepted, "{:?}", next.error);
}

#[test]
fn an_escaped_writer_locks_the_draft_when_stop_cancels_the_quiescence() {
    let w = World::new();
    // The adapter ignores SIGTERM and stdin EOF: the reap inside the pipeline takes its
    // whole grace period, which is when Stop arrives.
    let mut script = good("quiescing");
    script["escape"] = json!(true);
    script["linger"] = json!(true);
    w.submit(&script);
    w.wait_phase(TaskPhase::Quiescing);
    w.wf().stop().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Cancelled, "{:?}", task.error);
    assert!(
        matches!(task.draft_state, Some(DraftState::UnsafeWriter { .. })),
        "{:?}",
        task.draft_state
    );
    assert_successor_refused_then_unlocked(&w, &task.id);
}

#[test]
fn a_crashing_first_task_never_fails_its_queued_successors() {
    let w = World::new();
    w.set_plan(&[
        json!({"text": ["before the crash ", "and more"], "tool": 3, "crash": 7}),
        good("second"),
        good("third"),
    ]);
    w.wf().submit("plan brief one").unwrap();
    w.wf().submit("plan brief two").unwrap();
    w.wf().submit("plan brief three").unwrap();
    let first = w.wait_task(None);
    assert_eq!(first.phase, TaskPhase::Failed);
    let second = w.wait_task(Some(&first.id));
    assert_eq!(second.phase, TaskPhase::Accepted, "{:?}", second.error);
    let third = w.wait_task(Some(&second.id));
    assert_eq!(third.phase, TaskPhase::Accepted, "{:?}", third.error);
    assert_eq!(w.source("src/lib.rs"), "// third\n");
    let errors = w
        .snap()
        .rows
        .iter()
        .filter(|r| matches!(r.kind, RowKind::Error(_)))
        .count();
    assert_eq!(errors, 1, "only the crash is an error");
    // The crashed task's own text stays attached to it, and nothing of it leaks forward.
    for row in &w.snap().rows {
        if let RowKind::Agent { text, .. } = &row.kind
            && text.contains("before the crash")
        {
            assert_eq!(row.task.as_ref(), Some(&first.id));
        }
    }
    w.assert_clean();
}

#[test]
fn an_engine_refusal_before_the_session_is_registered_fails_boundedly_and_close_completes() {
    let w = World::new();
    fs::write(w.agent.join("hold-session"), "held").unwrap();
    w.submit(&good("never runs"));
    w.poll("the engine task", || {
        w.ctl()
            .lock()
            .agent_task()
            .is_some_and(|t| t.state() == TaskState::ContextReady)
    });
    // Another owner of the shared controller ends the task while the agent is still
    // answering `session/new`.
    let identity = w.ctl().lock().agent_task().unwrap().identity().clone();
    w.ctl()
        .lock()
        .finish_agent_task(&identity, TaskState::Interrupted, "ended by another owner")
        .unwrap();
    w.release("release-session");
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Failed);
    assert!(task.error.is_some());
    let workflow = w.workflow.clone().unwrap();
    let (done, closed) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        workflow.close();
        let _ = done.send(());
    });
    closed
        .recv_timeout(Duration::from_secs(30))
        .expect("close must not wedge behind a controller guard");
    w.assert_clean();
}

#[test]
fn a_conversation_log_that_cannot_be_written_stops_the_task_and_keeps_memory_bounded() {
    use fframes_studio::agent_workflow::log::RowLimits;
    let limits = RowLimits {
        max_rows: 8,
        max_bytes: 64 * 1024,
        ..RowLimits::default()
    };
    let w = World::with(Options {
        row_limits: Some(limits),
        ..Options::default()
    });
    // The log breaks after the workflow opened: a directory where the file belongs.
    let log = w
        .paths
        .project(&w.snap().project)
        .join("conversation.jsonl");
    let _ = fs::remove_file(&log);
    fs::create_dir(&log).unwrap();
    w.submit(&json!({"flood_tools": 3000, "hang": true}));
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Failed);
    assert_eq!(
        task.error.as_ref().map(|e| e.code.as_str()),
        Some("conversation_unsaved"),
        "{:?}",
        task.error
    );
    let s = w.snap();
    assert!(s.rows.len() <= limits.max_rows, "{} rows", s.rows.len());
    assert!(s.resources.resident_bytes <= limits.max_bytes);
    w.wf().close();
    w.assert_clean();
}

#[test]
fn stop_is_dispatched_while_the_controller_is_held_and_a_queued_refresh_waits_behind_it() {
    let w = World::new();
    let mut script = good("held");
    script["wait_file_late"] = json!("go");
    w.submit(&script);
    w.wait_phase(TaskPhase::Editing);
    // Whoever holds the controller for a long time (a slow capture or publication).
    let controller = w.ctl();
    let guard = controller.lock();
    w.release("go");
    w.wait_phase(TaskPhase::Quiescing);
    w.wf().refresh().unwrap();
    w.wf().stop().unwrap();
    // The actor dispatched Stop although a Refresh was queued ahead of it and the
    // controller is still held: neither command blocked it.
    w.wait("Stop dispatched under the barrier", |s| {
        s.task.as_ref().is_some_and(|t| t.stop_requested)
    });
    drop(guard);
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Cancelled, "{:?}", task.error);
    w.assert_clean();
}

#[test]
fn stop_while_a_promotion_waits_for_the_controller_cancels_it_and_nothing_is_published() {
    let w = World::new();
    w.wf()
        .set_review_policy(studio_engine::ReviewPolicy::ManualReview)
        .unwrap();
    w.submit(&good("never published"));
    w.wait_phase(TaskPhase::AwaitingReview);
    let original = w.source("src/lib.rs");
    let controller = w.ctl();
    let guard = controller.lock();
    w.wf().apply().unwrap();
    w.wait_phase(TaskPhase::Promoting);
    w.wf().stop().unwrap();
    drop(guard);
    let task = w.wait_task(None);
    assert_eq!(
        task.phase,
        TaskPhase::Cancelled,
        "Stop wins while the controller lock is still awaited: {:?}",
        task.error
    );
    assert_eq!(w.source("src/lib.rs"), original, "nothing was published");
    assert_eq!(w.snap().history.len(), 0);
    w.assert_clean();
}

#[test]
fn a_configured_credential_never_reaches_a_persisted_row_card_or_repair_prompt() {
    let secret = "zQ9-plain-credential-0042";
    let w = World::with(Options {
        secret: Some(secret.into()),
        ..Options::default()
    });
    w.set_plan(&[
        json!({"text": "oops", "write": {"src/lib.rs": format!("// BROKEN {secret}\n"), FAKE_CONFIG: GOOD_CONFIG}}),
        good("fixed"),
    ]);
    w.wf()
        .submit(&format!("plan brief that mentions {secret} verbatim"))
        .unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    assert!(task.repair.used == 1, "the compile failure was repaired");
    let rows = every_row(&w);
    let all = format!("{rows:?}");
    assert!(
        !all.contains(secret),
        "a persisted row carries the credential"
    );
    assert!(
        rows.values().any(|r| matches!(&r.kind, RowKind::User { source: UserSource::Brief, text } if text.contains("[REDACTED]"))),
        "the brief row shows the redaction"
    );
    assert!(
        rows.values().any(|r| matches!(
            &r.kind,
            RowKind::User {
                source: UserSource::Repair,
                ..
            }
        )),
        "the repair prompt is a row"
    );
    let s = w.snap();
    assert!(!format!("{:?}", s.task).contains(secret));
    let log = w.paths.project(&s.project).join("conversation.jsonl");
    assert!(
        !fs::read_to_string(log).unwrap().contains(secret),
        "the conversation log carries the credential"
    );
    w.assert_clean();
}

fn fault_once(job: &'static str, fault: JobFault) -> JobFaults {
    let fired = Arc::new(AtomicBool::new(false));
    Arc::new(move |name| {
        if name == job && !fired.swap(true, Ordering::SeqCst) {
            fault
        } else {
            JobFault::None
        }
    })
}

#[test]
fn a_pipeline_worker_that_cannot_start_or_panics_settles_the_task_and_the_queue_continues() {
    for (fault, code) in [
        (JobFault::Refuse, "thread_failed"),
        (JobFault::Panic, "worker_failed"),
    ] {
        let w = World::with(Options {
            job_faults: Some(fault_once("pipeline", fault)),
            ..Options::default()
        });
        w.submit(&good("first"));
        w.submit(&good("second"));
        let first = w.wait_task(None);
        assert_eq!(first.phase, TaskPhase::Failed, "{fault:?}");
        assert_eq!(first.error.as_ref().map(|e| e.code.as_str()), Some(code));
        let second = w.wait_task(Some(&first.id));
        assert_eq!(second.phase, TaskPhase::Accepted, "{:?}", second.error);
        assert_eq!(w.source("src/lib.rs"), "// second\n");
        w.assert_clean();
    }
}

#[test]
fn a_promotion_worker_that_cannot_start_leaves_the_candidate_applicable() {
    let w = World::with(Options {
        job_faults: Some(fault_once("promote", JobFault::Refuse)),
        ..Options::default()
    });
    w.wf()
        .set_review_policy(studio_engine::ReviewPolicy::ManualReview)
        .unwrap();
    w.submit(&good("applied later"));
    w.wait_phase(TaskPhase::AwaitingReview);
    w.wf().apply().unwrap();
    w.wait("the refused promotion", |s| {
        has_error(s, "thread_failed")
            && s.task.as_ref().is_some_and(|t| {
                t.phase == TaskPhase::AwaitingReview
                    && t.review.as_ref().is_some_and(|r| r.apply_blocked.is_some())
            })
    });
    assert_eq!(w.engine_state(), Some(TaskState::CandidateReady));
    w.wf().apply().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    assert_eq!(w.source("src/lib.rs"), "// applied later\n");
    w.assert_clean();
}

#[test]
fn a_promotion_worker_that_panics_fails_the_task_and_publishes_nothing() {
    let w = World::with(Options {
        job_faults: Some(fault_once("promote", JobFault::Panic)),
        ..Options::default()
    });
    w.wf()
        .set_review_policy(studio_engine::ReviewPolicy::ManualReview)
        .unwrap();
    let original = w.source("src/lib.rs");
    w.submit(&good("never published"));
    w.wait_phase(TaskPhase::AwaitingReview);
    w.wf().apply().unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Failed, "{:?}", task.error);
    assert_eq!(
        task.error.as_ref().map(|e| e.code.as_str()),
        Some("worker_failed")
    );
    assert_eq!(w.source("src/lib.rs"), original);
    assert_eq!(w.engine_state(), Some(TaskState::Failed));
    w.assert_clean();
}

#[test]
fn an_undo_worker_that_cannot_start_or_panics_never_leaves_the_undo_in_progress() {
    for (fault, code) in [
        (JobFault::Refuse, "thread_failed"),
        (JobFault::Panic, "worker_failed"),
    ] {
        let w = World::with(Options {
            job_faults: Some(fault_once("undo", fault)),
            ..Options::default()
        });
        let original = w.source("src/lib.rs");
        w.submit(&good("to undo"));
        assert_eq!(w.wait_task(None).phase, TaskPhase::Accepted);
        w.wf().undo(None).unwrap();
        w.wait("the failed undo", |s| {
            has_error(s, code) && !matches!(s.undo, UndoView::InProgress { .. })
        });
        assert_eq!(w.source("src/lib.rs"), "// to undo\n", "{fault:?}");
        // The Undo is still available and now runs.
        w.wf().undo(None).unwrap();
        w.wait("the undo", |s| s.history.len() == 2);
        assert_eq!(w.source("src/lib.rs"), original);
        w.assert_clean();
    }
}

#[test]
fn a_probe_worker_that_cannot_start_reports_it_and_readiness_is_not_stuck_checking() {
    let w = World::with(Options {
        job_faults: Some(fault_once("probe", JobFault::Refuse)),
        ..Options::default()
    });
    w.wf().check_adapter().unwrap();
    w.wait("the refused probe", |s| has_error(s, "thread_failed"));
    assert_eq!(w.snap().adapter.readiness, AdapterReadiness::Unchecked);
    w.wf().check_adapter().unwrap();
    w.wait("the probe", |s| {
        matches!(s.adapter.readiness, AdapterReadiness::Checked { .. })
    });
}

#[test]
fn settings_are_refused_for_a_just_submitted_task_and_for_an_undo_before_any_snapshot_shows_them() {
    let w = World::new();
    w.wf()
        .submit(&World::brief(&json!({"hang": true})))
        .unwrap();
    assert!(matches!(
        w.wf().set_build(None),
        Err(WorkflowError::Busy(_))
    ));
    assert!(matches!(
        w.wf().set_tools(None),
        Err(WorkflowError::Busy(_))
    ));
    assert!(matches!(
        w.wf().set_adapter(None),
        Err(WorkflowError::Busy(_))
    ));
    w.wait_phase(TaskPhase::Editing);
    w.wf().stop().unwrap();
    let first = w.wait_task(None);
    w.wf().set_adapter(Some(w.adapter_settings())).unwrap();
    w.submit(&good("one edit"));
    assert_eq!(w.wait_task(Some(&first.id)).phase, TaskPhase::Accepted);
    w.wf().undo(None).unwrap();
    assert!(
        matches!(w.wf().set_adapter(None), Err(WorkflowError::Busy(_))),
        "an Undo is busy from the moment it is requested"
    );
    w.wait("the undo", |s| s.history.len() == 2);
    w.wf().set_adapter(Some(w.adapter_settings())).unwrap();
}

#[test]
fn an_open_permission_stays_answerable_after_its_card_leaves_the_transcript_window() {
    use fframes_studio::agent_workflow::log::RowLimits;
    let w = World::with(Options {
        row_limits: Some(RowLimits {
            max_rows: 5,
            ..RowLimits::default()
        }),
        ..Options::default()
    });
    let mut script = good("evicted request");
    script["permission"] = json!({"title": "Edit title"});
    w.submit(&script);
    let s = w.wait_phase_snapshot(TaskPhase::WaitingPermission);
    let card = s.open_permissions()[0].clone();
    // Enough transcript traffic (one notice per queued brief) to push the card out.
    for _ in 0..8 {
        w.wf()
            .submit(&World::brief(&json!({"hang": true})))
            .unwrap();
    }
    let s = w.wait("the card to leave the window", |s| {
        s.queue.len() == 8 && s.open_permissions().is_empty()
    });
    assert_eq!(s.resources.open_permissions, 1, "the request is still open");
    assert_eq!(
        w.wf()
            .reply_permission(&card.reference, PermissionAnswer::Select("nope".into()))
            .unwrap_err(),
        WorkflowError::UnknownChoice("nope".into()),
        "a bad choice is refused without consuming the request"
    );
    w.wf()
        .reply_permission(&card.reference, PermissionAnswer::Select("allow".into()))
        .unwrap();
    assert_eq!(
        w.wf()
            .reply_permission(&card.reference, PermissionAnswer::Select("allow".into()))
            .unwrap_err(),
        WorkflowError::DuplicatePermission
    );
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    w.wf().stop().unwrap();
    w.wait("the queue to be cleared", |s| s.queue.is_empty());
    let rows = every_row(&w);
    let persisted = rows
        .values()
        .find_map(|r| match &r.kind {
            RowKind::Permission(c) if c.reference == card.reference => Some(c.state.clone()),
            _ => None,
        })
        .expect("the card is in the log");
    assert_eq!(
        persisted,
        PermissionState::Selected {
            option_id: "allow".into()
        },
        "the closure reaches the persisted card, not a second one"
    );
    assert_eq!(
        rows.values()
            .filter(|r| matches!(&r.kind, RowKind::Permission(c) if c.reference == card.reference))
            .count(),
        1
    );
    assert_eq!(w.snap().resources.open_permissions, 0);
}

/// Reports the handoff outcome from inside `handoff()`, before the job's result message.
struct EagerSink {
    workflow: Mutex<std::sync::Weak<AgentWorkflow>>,
    display: bool,
}

impl PreviewHandoff for EagerSink {
    fn handoff(&self, handoff: PromotionHandoff) -> Result<(), String> {
        let published = handoff.promotion.record.published.as_str().to_owned();
        let workflow = self
            .workflow
            .lock()
            .upgrade()
            .ok_or("the workflow is gone")?;
        if self.display {
            workflow.preview_displayed(&published).unwrap();
        } else {
            workflow
                .report_handoff(&published, Err("failed while handing off".into()))
                .unwrap();
        }
        Ok(())
    }
}

#[test]
fn an_acknowledgement_sent_from_inside_the_handoff_is_not_lost() {
    for display in [false, true] {
        let sink = Arc::new(EagerSink {
            workflow: Mutex::new(std::sync::Weak::new()),
            display,
        });
        let w = World::with(Options {
            handoff: Some(sink.clone()),
            ..Options::default()
        });
        *sink.workflow.lock() = Arc::downgrade(w.workflow.as_ref().unwrap());
        w.submit(&good("eager"));
        let task = w.wait_task(None);
        assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
        let state = w.snap().handoff.as_ref().unwrap().state.clone();
        if display {
            assert_eq!(state, HandoffState::Displayed);
        } else {
            assert_eq!(
                state,
                HandoffState::AwaitingPreview {
                    reason: Some("failed while handing off".into())
                },
                "the early failure is not overwritten by the sink's Ok"
            );
        }
    }
}

#[test]
fn a_follow_up_task_keeps_the_pending_preview_of_the_accepted_revision() {
    let sink = Arc::new(Sink {
        coordinator: Mutex::new(None),
        fail: AtomicBool::new(true),
        adopted: Mutex::new(Vec::new()),
    });
    let w = World::with(Options {
        handoff: Some(sink),
        ..Options::default()
    });
    w.submit(&good("accepted first"));
    let first = w.wait_task(None);
    assert_eq!(first.phase, TaskPhase::Accepted, "{:?}", first.error);
    let published = w.ctl().lock().state().source().as_str().to_owned();
    let awaiting = HandoffState::AwaitingPreview {
        reason: Some("preview worker refused".into()),
    };
    assert_eq!(w.snap().handoff.as_ref().unwrap().state, awaiting);
    // A follow-up that waits and is stopped never touches the accepted revision's state.
    w.submit(&json!({"hang": true}));
    w.wait_phase(TaskPhase::Editing);
    assert_eq!(w.snap().handoff.as_ref().unwrap().state, awaiting);
    // The delayed preview recovers while the follow-up runs.
    w.wf().report_handoff(&published, Ok(())).unwrap();
    w.wait("the late adoption", |s| {
        s.handoff
            .as_ref()
            .is_some_and(|h| h.state == HandoffState::Adopted)
    });
    w.wf().stop().unwrap();
    w.wait_task(Some(&first.id));
    assert_eq!(
        w.snap().handoff.as_ref().unwrap().state,
        HandoffState::Adopted
    );
}

#[test]
fn a_tool_call_updated_after_its_card_left_the_window_is_one_persisted_card() {
    use fframes_studio::agent_workflow::log::RowLimits;
    use studio_agent_spike::driver::ToolStatus;
    let w = World::with(Options {
        row_limits: Some(RowLimits {
            max_rows: 5,
            ..RowLimits::default()
        }),
        ..Options::default()
    });
    let mut script = good("interleaved tools");
    script["tool"] = Value::Array(
        (0..12)
            .map(|i| json!({"id": format!("call-{i}"), "title": format!("Tool {i}")}))
            .collect(),
    );
    script["tool_interleave"] = json!(true);
    w.submit(&script);
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    let rows = every_row(&w);
    let cards: Vec<&ToolCard> = rows
        .values()
        .filter_map(|r| match &r.kind {
            RowKind::Tool(card) if card.call_id.starts_with("call-") => Some(card),
            _ => None,
        })
        .collect();
    assert_eq!(cards.len(), 12, "one persisted card per tool call");
    assert!(
        cards
            .iter()
            .all(|c| c.status == Some(ToolStatus::Completed) && c.updates >= 2),
        "every card carries its final state"
    );
}

fn run_studio_tools(capability: &std::path::Path, tool: &str) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_studio-tools"))
        .arg("--capability")
        .arg(capability)
        .arg(tool)
        .output()
        .unwrap()
}

#[test]
fn the_command_line_route_has_a_task_capability_when_mcp_is_disabled_or_its_helper_is_missing() {
    for (options, why) in [
        (
            Options {
                mcp: McpStdioSupport::Unsupported,
                ..Options::default()
            },
            "disabled by host policy",
        ),
        (
            Options {
                mcp_helper: false,
                ..Options::default()
            },
            "helper was not found",
        ),
    ] {
        let w = World::with(options);
        w.submit(&json!({"hang": true}));
        w.wait_phase(TaskPhase::Editing);
        w.wait_prompts(1);
        let params: Value =
            serde_json::from_str(&fs::read_to_string(w.agent.join("session-params.json")).unwrap())
                .unwrap();
        assert!(params["mcpServers"].as_array().unwrap().is_empty());
        let s = w.snap();
        assert!(!s.mcp.active && s.mcp.cli_active);
        assert!(
            s.mcp.note.as_deref().is_some_and(|n| n.contains(why)),
            "{:?}",
            s.mcp.note
        );
        assert_eq!(s.resources.broker_grants, 1);
        let capability = s.mcp.capability_file.clone().unwrap();
        assert!(capability.is_file());
        let out = run_studio_tools(&capability, "project_context");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(reply.is_object(), "{reply}");
        let prompt = w.evidence("prompts.jsonl")[0]["text"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(prompt.contains("studio-tools") && prompt.contains(&*capability.to_string_lossy()));
        w.wf().stop().unwrap();
        w.wait_task(None);
        assert!(!capability.exists(), "the capability died with the task");
        w.assert_clean();
    }
}

#[test]
fn without_any_tool_route_no_capability_exists_and_the_snapshot_says_so() {
    let w = World::with(Options {
        mcp_helper: false,
        cli_helper: false,
        ..Options::default()
    });
    w.submit(&json!({"hang": true}));
    w.wait_phase(TaskPhase::Editing);
    let s = w.snap();
    assert!(!s.mcp.active && !s.mcp.cli_active && s.mcp.capability_file.is_none());
    assert_eq!(s.resources.broker_grants, 0);
    assert!(
        s.mcp
            .note
            .as_deref()
            .is_some_and(|n| n.contains("no project tools")),
        "{:?}",
        s.mcp.note
    );
    w.wf().stop().unwrap();
    w.wait_task(None);
    w.assert_clean();
}

#[test]
fn a_history_page_never_blocks_the_caller_or_the_actor_when_the_log_read_blocks() {
    use std::os::unix::ffi::OsStrExt;
    let w = World::new();
    // A FIFO where the log belongs: opening it for reading blocks until a writer comes.
    let log = w
        .paths
        .project(&w.snap().project)
        .join("conversation.jsonl");
    let _ = fs::remove_file(&log);
    let path = std::ffi::CString::new(log.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let (tx, rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    w.wf()
        .history_page_with(RowId(u64::MAX), 10, move |page| {
            let _ = tx.send(page);
        })
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the caller was not blocked by the read"
    );
    let before = w.snap().revision;
    w.wf()
        .set_review_policy(studio_engine::ReviewPolicy::ManualReview)
        .unwrap();
    let after = w.wf().wait_changed(before, Duration::from_secs(10));
    assert!(
        after.revision > before,
        "the actor kept publishing while the read was blocked"
    );
    assert!(
        rx.try_recv().is_err(),
        "the page is still waiting on the disk"
    );
    // A writer arrives and leaves: the read completes.
    drop(fs::OpenOptions::new().write(true).open(&log).unwrap());
    let page = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the page completes once the disk answers");
    assert!(page.is_ok(), "{page:?}");
}

#[test]
fn open_detached_delivers_the_workflow_on_its_own_thread_and_the_replay_never_runs_on_the_caller() {
    let w = World::new();
    w.wf().close();
    let (tx, rx) = std::sync::mpsc::channel();
    let caller = std::thread::current().id();
    AgentWorkflow::open_detached(w.config(), w.ctl(), move |opened| {
        let _ = tx.send((std::thread::current().id(), opened));
    });
    let (thread, opened) = rx.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_ne!(thread, caller, "the open ran on its own thread");
    let opened = opened.expect("the project opens");
    assert_eq!(opened.snapshot().project, w.snap().project);
    opened.close_detached();
}

// ---- helpers used above ---------------------------------------------------------------------------------------------------------------------------------------

impl World {
    fn wait_phase_snapshot(&self, phase: TaskPhase) -> Arc<WorkflowSnapshot> {
        self.wait(&format!("phase {phase:?}"), |s| {
            s.task.as_ref().is_some_and(|t| t.phase == phase)
        })
    }
}

// ---- the real SDK ----------------------------------------------------------------------------------------------------------------------------------------

/// The SDK the real tests compile against: `SDK_BUNDLE` (an assembled bundle, installed
/// into a temporary SDK home) or `SDK_ACTIVE` (an installed SDK directory).
fn real_sdk(temp: &tempfile::TempDir) -> (PathBuf, studio_sdk::CompatibilityManifest) {
    let path = if let Some(bundle) = std::env::var_os("SDK_BUNDLE") {
        let bundle = PathBuf::from(bundle);
        let manifest = studio_sdk::CompatibilityManifest::from_json_str(
            &fs::read_to_string(bundle.join("compatibility.json")).unwrap(),
        )
        .unwrap();
        let artifacts: Vec<_> = manifest
            .artifacts
            .iter()
            .map(|a| (a.clone(), bundle.join(a.url.trim_start_matches("file://"))))
            .collect();
        let home = std::env::var_os("CANDIDATE_SDK_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| temp.path().join("sdk"));
        studio_sdk::SdkInstaller::new(home)
            .install_from_local_artifacts(&manifest, &artifacts)
            .unwrap()
    } else {
        PathBuf::from(std::env::var_os("SDK_ACTIVE").expect("SDK_ACTIVE or SDK_BUNDLE"))
    };
    let manifest = studio_sdk::CompatibilityManifest::from_json_str(
        &fs::read_to_string(path.join("compatibility.json")).unwrap(),
    )
    .unwrap();
    (path, manifest)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The cue with another tone: the candidate's audio differs from the base's.
fn retoned(cue: &[u8], hz: f64) -> Vec<u8> {
    let mut wav = cue.to_vec();
    for (i, chunk) in wav[44..].as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let t = i as f64 / 48_000.;
        let left = ((t * hz * std::f64::consts::TAU).sin() * 3000.) as i16;
        chunk[..2].copy_from_slice(&left.to_le_bytes());
    }
    wav
}

/// Real Cargo compiles, the real preview worker, real Apply/Undo through the project
/// folder, and the scripted peer as the agent editing the working copy. Executed
/// explicitly; a skipped run is not evidence:
///
///     SDK_BUNDLE=/tmp/studio-m2-sdk-stage3 cargo test --locked -p fframes-studio \
///         --test agent_workflow -- --ignored --nocapture
#[test]
#[ignore = "requires SDK_BUNDLE or SDK_ACTIVE; real compiler + preview worker; scripted agent"]
fn real_sdk_workflow_edits_applies_hands_off_undoes_and_closes_clean() {
    let sdk_temp = tempfile::tempdir().unwrap();
    let (sdk, manifest) = real_sdk(&sdk_temp);
    let owner = ProcessTreeManager::new();
    let sink = Arc::new(Sink {
        coordinator: Mutex::new(Some(Arc::new(PreviewCoordinator::new(owner.sub_manager())))),
        fail: AtomicBool::new(false),
        adopted: Mutex::new(Vec::new()),
    });
    let w = World::with(Options {
        real: Some((sdk, manifest)),
        handoff: Some(sink.clone()),
        ..Options::default()
    });
    let original_lib = w.source("src/lib.rs");
    let original_cue = fs::read(w.root.join("media/cue.wav")).unwrap();
    let original_revision = w.ctl().lock().state().source().as_str().to_owned();
    let new_cue = retoned(&original_cue, 880.);
    w.submit_plan(&[json!({
        "text": "Retoned the cue.",
        "write": {"src/lib.rs": format!("{original_lib}\n// edited by the scripted agent\n")},
        "write_b64": {"media/cue.wav": base64(&new_cue)},
    })]);
    let task = w.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    let validation = task.validation.as_ref().expect("validation");
    assert!(validation.passed, "{}", validation.summary);
    assert!(validation.coverage.as_ref().unwrap().rendered_frames > 0);
    assert!(
        validation
            .audio
            .as_ref()
            .is_some_and(|a| !a.silent && a.placement_verified)
    );
    let changes = task.changes.as_ref().unwrap();
    assert!(changes.entries.iter().any(|e| e.path == "media/cue.wav"));
    assert!(changes.entries.iter().any(|e| e.path == "src/lib.rs"));
    assert_eq!(fs::read(w.root.join("media/cue.wav")).unwrap(), new_cue);
    let published = w.ctl().lock().state().source().as_str().to_owned();
    assert_ne!(published, original_revision);
    let coordinator = sink.coordinator.lock().clone().unwrap();
    let ready = wait_ready(&coordinator);
    assert_eq!(
        ready.identity().source_revision,
        published,
        "the handed-off preview is the published revision"
    );
    assert!(ready.audio_source.is_some());
    assert_eq!(
        w.snap().handoff.as_ref().unwrap().state,
        HandoffState::Adopted
    );
    drop(ready);

    // Undo returns to the original bytes through the same validation + handoff.
    w.wf().undo(None).unwrap();
    w.wait("the Undo", |s| s.history.len() == 2);
    assert_eq!(w.source("src/lib.rs"), original_lib);
    assert_eq!(
        fs::read(w.root.join("media/cue.wav")).unwrap(),
        original_cue
    );
    assert_eq!(w.ctl().lock().state().source().as_str(), original_revision);
    let ready = wait_ready(&coordinator);
    assert_eq!(ready.identity().source_revision, original_revision);
    drop(ready);
    drop(coordinator);
    let owned = sink.coordinator.lock().take().unwrap();
    owned.close();
    drop(owned);
    w.wf().close();
    w.assert_clean();
}

/// A real generated starter title is selected from managed-worker frame metadata, frozen in
/// the task, edited by the workflow, validated and applied, then undone and reopened. The ACP
/// peer is intentionally scripted: this is M5 development evidence, never provider evidence.
#[test]
#[ignore = "requires SDK_BUNDLE; real managed compiler/worker and scripted development agent"]
fn selected_title_prompt_apply_undo_and_reopen_round_trips_identity() {
    let sdk_temp = tempfile::tempdir().unwrap();
    let (sdk, manifest) = real_sdk(&sdk_temp);
    let mut world = World::with(Options {
        real: Some((sdk, manifest)),
        starter_project: true,
        ..Options::default()
    });

    let (scope, initial_preview, initial_title) = measured_starter_title_scope(&world);
    world
        .wf()
        .set_displayed_preview_identity(Some(initial_preview.clone()))
        .unwrap();
    let original_source = world.source("src/lib.rs");
    assert_eq!(original_source.matches("Your video starts here").count(), 1);
    let candidate_source =
        original_source.replace("Your video starts here", "A selected title change");
    scope.validate().unwrap();
    world.submit_scoped_plan(
        "Change only the selected title to ‘A selected title change’.",
        scope.clone(),
        &[json!({
            "text": "Updated the selected starter title.",
            "write": {"src/lib.rs": candidate_source},
        })],
    );

    let task = world.wait_task(None);
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    assert_eq!(task.scope.canvas_selection, scope.canvas_selection);
    assert!(
        task.scope
            .prompt_context()
            .contains("Explicit source anchor")
    );
    assert!(task.scope.prompt_context().contains("starter-title"));
    let validation = task.validation.as_ref().expect("real candidate validation");
    assert!(validation.passed, "{}", validation.summary);
    assert!(validation.coverage.as_ref().unwrap().rendered_frames > 0);
    assert_eq!(world.source("src/lib.rs"), candidate_source);
    let applied_revision = world.ctl().lock().state().source().as_str().to_owned();
    assert_ne!(applied_revision, initial_preview.source_revision);

    let prompts = world.evidence("prompts.jsonl");
    assert_eq!(prompts.len(), 1);
    let prompt = prompts[0]["text"].as_str().unwrap();
    assert!(
        prompt.contains("Requested scope: Element headline"),
        "{prompt}"
    );
    assert!(prompt.contains("scene=starter-video"), "{prompt}");
    assert!(prompt.contains("component=starter-title"), "{prompt}");
    assert!(
        prompt.contains("marker=Some(\"studio-title-source-anchor\")"),
        "{prompt}"
    );

    world.wf().undo(None).unwrap();
    world.wait("selected title undo", |snapshot| {
        snapshot.history.len() == 2
    });
    assert_eq!(world.source("src/lib.rs"), original_source);
    assert_eq!(
        world.ctl().lock().state().source().as_str(),
        initial_preview.source_revision
    );
    world.restart();
    assert_eq!(world.source("src/lib.rs"), original_source);
    assert_eq!(
        world.ctl().lock().project.inventory.revision.as_str(),
        initial_preview.source_revision
    );
    let (reopened_scope, reopened_preview, reopened_title) = measured_starter_title_scope(&world);
    assert_eq!(
        reopened_preview.source_revision,
        initial_preview.source_revision
    );
    assert_eq!(reopened_title.identity, initial_title.identity);
    assert_eq!(reopened_title.source_anchor, initial_title.source_anchor);
    assert_eq!(reopened_title.style_tokens, initial_title.style_tokens);
    assert_eq!(
        reopened_scope.canvas_selection.unwrap().editor_index_digest,
        scope.canvas_selection.unwrap().editor_index_digest
    );
    assert_ne!(applied_revision, initial_preview.source_revision);
    world.assert_clean();
}

fn measured_starter_title_scope(
    world: &World,
) -> (
    TaskScope,
    PreviewIdentity,
    fframes_studio_protocol::EditorObjectGeometry,
) {
    let project = world.ctl().lock().project.clone();
    let preview = PreviewIdentity {
        project_id: String::from(project.manifest.project_id.clone()),
        open_session: "m5-selected-title-session".into(),
        source_revision: project.inventory.revision.as_str().into(),
        worker_generation: 1,
    };
    let manager = ProcessTreeManager::new();
    let build = fframes_studio::worker_project::compile_portable_worker(
        &project,
        &world.sdk,
        world.manifest.clone(),
        &world._temp.path().join("m5-selected-title-builds"),
        &manager,
    )
    .expect("compile the generated starter using the managed SDK");
    let mut worker =
        fframes_studio::worker_project::launch_preview_worker(build, preview.clone(), &manager)
            .expect("launch the generated starter preview worker");
    let timeline = worker.timeline().expect("managed starter timeline");
    let frame = worker.frame(0, 17, 1.0).expect("managed title frame");
    let metadata = frame
        .response
        .editor_metadata
        .expect("frame-coupled starter title metadata");
    assert_eq!(metadata.status, EditorFrameStatus::Supported);
    assert_eq!((metadata.frame_index, metadata.seek_serial), (0, 17));
    let title = metadata
        .objects
        .into_iter()
        .find(|object| {
            object.identity.scene_instance_key == "starter-video"
                && object.identity.component_key == "starter-title"
                && object.identity.object_key == "headline"
                && object.identity.repeat_key == "primary"
        })
        .expect("the registered starter title is present in the actual managed frame");
    let left = title.bounds.x.floor() as u32;
    let top = title.bounds.y.floor() as u32;
    let right = (title.bounds.x + title.bounds.width).ceil() as u32;
    let bottom = (title.bounds.y + title.bounds.height).ceil() as u32;
    let mut scope = TaskScope::from_timeline(&timeline, &TimelineSelection::default()).unwrap();
    scope.canvas_selection = Some(CanvasTaskSelection {
        preview: preview.clone(),
        frame_index: metadata.frame_index,
        seek_serial: metadata.seek_serial,
        editor_index_digest: Some(metadata.editor_index_digest),
        frame_geometry_digest: Some(metadata.frame_geometry_digest),
        video_width: metadata.video_width,
        video_height: metadata.video_height,
        selection: CanvasTaskSelectionKind::Element {
            identity: title.identity.clone(),
            bounds: VideoPixelRect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            },
            support: EditorGeometrySupport::ApproximateBounds,
            source_anchor: title.source_anchor.clone(),
            style_tokens: title.style_tokens.clone(),
        },
    });
    scope.validate().unwrap();
    drop(worker);
    manager.terminate_all(Duration::from_millis(300));
    assert_eq!(manager.active_count(), 0);
    (scope, preview, title)
}
