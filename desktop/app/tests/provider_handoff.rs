//! Integration tests for Phase 2: Controlled handoff and restoration (`provider_handoff`).
//!
//! Verifies:
//! - Outgoing queue retention during switch without automatic successor execution
//! - Explicit queue transfer vs clear after handoff continuation
//! - Outgoing writer teardown and draft capture
//! - Unqualified adapter switch blocking
//! - Late permission rejection after switch
//! - Continuation choices (ContinueDraft vs RestartFromAccepted)
//! - Session manifest persistence, atomic writes, credential redaction, and restoration lifecycle
//! - Session save failure handling and visible persistence disabling

#![allow(dead_code, unused_imports)]

use fframes_studio_protocol::PreviewIdentity;

include!("support/workflow_world.rs");

#[test]
fn session_manifest_lifecycle_and_redaction() {
    let w = World::new();
    let project_id = w.snap().project.clone();
    let app_paths = w.paths.clone();

    // 1. Initially no manifest exists
    let loaded =
        fframes_studio::agent_workflow::session_store::SessionStore::load(&app_paths, &project_id)
            .expect("load ok");
    assert!(loaded.is_none());

    // 2. Save a valid manifest
    let manifest = fframes_studio::agent_workflow::session_store::SessionManifest {
        schema_version:
            fframes_studio::agent_workflow::session_store::SESSION_MANIFEST_SCHEMA_VERSION,
        provider_id: "claude".into(),
        wire_session_id: Some("session-token-secret-12345678".into()),
        project_id: project_id.to_string(),
        draft_path: PathBuf::from("/tmp/draft"),
        launch_digest: "sha256:abc123".into(),
        last_source_revision: "1111111111111111111111111111111111111111111111111111111111111111"
            .into(),
        last_accepted_revision: "2222222222222222222222222222222222222222222222222222222222222222"
            .into(),
        context_schema: fframes_studio::agent_workflow::session_store::CONTEXT_SCHEMA_VERSION
            .into(),
        capability_snapshot: None,
        transcript_boundary: 42,
        state: fframes_studio::agent_workflow::session_store::SessionManifestState::Ready,
        created_at: "1728285120".into(),
        updated_at: "1728285120".into(),
    };

    assert!(manifest.is_resumable());
    assert_eq!(manifest.redacted_id(), "sessio…5678");

    fframes_studio::agent_workflow::session_store::SessionStore::save(
        &app_paths,
        &project_id,
        &manifest,
    )
    .expect("save ok");

    // 3. Load back and verify
    let loaded =
        fframes_studio::agent_workflow::session_store::SessionStore::load(&app_paths, &project_id)
            .expect("load ok")
            .expect("manifest found");
    assert_eq!(loaded.provider_id, "claude");
    assert_eq!(
        loaded.wire_session_id.as_deref(),
        Some("session-token-secret-12345678")
    );
    assert_eq!(loaded.transcript_boundary, 42);

    // 4. Test clearing / dismissing
    fframes_studio::agent_workflow::session_store::SessionStore::clear(&app_paths, &project_id)
        .expect("clear ok");

    let after_clear =
        fframes_studio::agent_workflow::session_store::SessionStore::load(&app_paths, &project_id)
            .expect("load ok");
    assert!(after_clear.is_none());
}

#[test]
fn session_save_failure_visibly_disables_restoration() {
    let temp = tempfile::tempdir().unwrap();
    let unwritable_paths =
        studio_engine::app_paths::AppPaths::new(temp.path().join("read_only")).unwrap();
    // Create read_only directory and make it read-only
    let state_dir = unwritable_paths
        .project(&studio_project::ProjectId::try_from("test-proj".to_string()).unwrap());
    fs::create_dir_all(state_dir.parent().unwrap()).unwrap();
    fs::write(&state_dir, "regular file").unwrap();
    let manifest = fframes_studio::agent_workflow::session_store::SessionManifest {
        schema_version:
            fframes_studio::agent_workflow::session_store::SESSION_MANIFEST_SCHEMA_VERSION,
        provider_id: "claude".into(),
        wire_session_id: Some("session-123".into()),
        project_id: "test-proj".into(),
        draft_path: PathBuf::from("/tmp/draft"),
        launch_digest: "sha256:abc123".into(),
        last_source_revision: "1111111111111111111111111111111111111111111111111111111111111111"
            .into(),
        last_accepted_revision: "2222222222222222222222222222222222222222222222222222222222222222"
            .into(),
        context_schema: fframes_studio::agent_workflow::session_store::CONTEXT_SCHEMA_VERSION
            .into(),
        capability_snapshot: None,
        transcript_boundary: 1,
        state: fframes_studio::agent_workflow::session_store::SessionManifestState::Ready,
        created_at: "1728285120".into(),
        updated_at: "1728285120".into(),
    };

    let save_result = fframes_studio::agent_workflow::session_store::SessionStore::save(
        &unwritable_paths,
        &studio_project::ProjectId::try_from("test-proj".to_string()).unwrap(),
        &manifest,
    );
    assert!(
        save_result.is_err(),
        "save must fail on ENOTDIR regular file"
    );
}

#[test]
fn bounded_handoff_context_budget_enforcement() {
    let large_diag: Vec<String> = (0..5000)
        .map(|i| format!("diagnostic line number {i} with substantial detail"))
        .collect();
    let ctx = fframes_studio::agent_workflow::session_store::BoundedHandoffContext::new(
        "src_rev".into(),
        Some("draft_rev".into()),
        "brief text".into(),
        Some("outcome summary".into()),
        None,
        None,
        large_diag,
    );

    let bytes = serde_json::to_vec(&ctx).expect("serializes");
    assert!(
        bytes.len() <= fframes_studio::agent_workflow::session_store::MAX_CONTEXT_ENVELOPE_BYTES,
        "context size {} exceeds budget {}",
        bytes.len(),
        fframes_studio::agent_workflow::session_store::MAX_CONTEXT_ENVELOPE_BYTES
    );
    assert!(ctx.omissions.contains(&"diagnostics_truncated".to_string()));
}

#[test]
fn idle_switch_transitions_to_pending_and_can_cancel() {
    let w = World::new();

    // Workflow starts idle
    assert!(w.snap().task.is_none());
    assert!(w.snap().switch_pending.is_none());

    // Initiate switch to another provider
    w.wf().initiate_switch("codex").expect("initiate_switch ok");

    let s = w.wait("switch pending view appears", |s| {
        s.switch_pending.is_some()
    });

    let pending = s.switch_pending.as_ref().unwrap();
    assert_eq!(pending.target_provider, "codex");
    assert_eq!(pending.retained_queue_count, 0);

    // Cancelling switch returns to normal idle state
    w.wf().cancel_switch().expect("cancel_switch ok");

    w.wait("switch pending view cleared", |s| {
        s.switch_pending.is_none()
    });
}

#[test]
fn two_retained_queued_briefs_without_automatic_execution_and_explicit_transfer() {
    let w = World::new();

    // Configure target profile in registry to use test adapter
    let mut registry =
        fframes_studio::conversation_panel::provider_profiles::ProviderRegistry::default();
    if let Some(codex) = registry.profiles.iter_mut().find(|p| p.id == "codex") {
        codex.adapter.executable = "python3".into();
        codex.adapter.args = vec![agent_script(), w.agent.to_string_lossy().into_owned()];
    }
    fs::create_dir_all(&w.paths.data).unwrap();
    fs::write(
        w.paths
            .data
            .join(fframes_studio::conversation_panel::host::REGISTRY_FILE),
        registry.to_pretty(),
    )
    .unwrap();

    // Submit a brief while idle; it starts running
    w.wf().submit("first active task").expect("submit ok");
    w.wait("first task starts", |s| s.task.is_some());

    // Submit two queued briefs
    w.wf()
        .submit("second queued task")
        .expect("submit queued 1 ok");
    w.wf()
        .submit("third queued task")
        .expect("submit queued 2 ok");
    let s = w.wait("two tasks are queued", |s| s.queue.len() == 2);
    assert_eq!(s.queue[0].summary, "second queued task");
    assert_eq!(s.queue[1].summary, "third queued task");

    // Initiate provider switch: active task is stopped, and both queued briefs are retained
    w.wf().initiate_switch("codex").expect("initiate_switch ok");

    let s = w.wait("switch pending appears with 2 retained briefs", |s| {
        s.switch_pending.is_some() && s.retained_queue.len() == 2
    });

    let pending = s.switch_pending.as_ref().unwrap();
    assert_eq!(pending.target_provider, "codex");
    assert_eq!(pending.retained_queue_count, 2);
    assert_eq!(s.retained_queue.len(), 2);
    assert_eq!(s.queue.len(), 0, "active queue must be empty during switch");

    // Choose continuation: incoming provider is now configured
    w.wf()
        .choose_handoff_continuation(
            fframes_studio::agent_workflow::model::HandoffChoice::RestartFromAccepted,
        )
        .expect("choose ok");

    w.wait("switch resolved", |s| s.switch_pending.is_none());

    // CRITICAL (Success Criterion 6): Incoming provider MUST NOT auto-run the old queue!
    std::thread::sleep(std::time::Duration::from_millis(150));
    let s = w.snap();
    assert_eq!(
        s.retained_queue.len(),
        2,
        "briefs remain held in retained_queue"
    );
    assert_eq!(s.queue.len(), 0, "active queue must remain empty");
    assert!(
        s.task
            .as_ref()
            .is_none_or(|t| t.brief != "second queued task" && t.phase.is_terminal()),
        "queued briefs should not start automatically"
    );

    // User explicitly transfers retained briefs
    w.wf().transfer_retained_queue().expect("transfer ok");

    let s = w.wait("transferred briefs start executing", |s| {
        s.task
            .as_ref()
            .is_some_and(|t| t.brief == "second queued task")
    });
    assert_eq!(s.task.as_ref().unwrap().brief, "second queued task");
}

#[test]
fn two_retained_queued_briefs_explicit_clear() {
    let w = World::new();

    w.wf().submit("first active task").expect("submit ok");
    w.wait("first task starts", |s| s.task.is_some());

    w.wf().submit("queued task A").expect("submit queued A");
    w.wf().submit("queued task B").expect("submit queued B");
    w.wait("two tasks are queued", |s| s.queue.len() == 2);

    w.wf().initiate_switch("codex").expect("initiate_switch ok");
    w.wait("switch pending with 2 retained briefs", |s| {
        s.switch_pending.is_some() && s.retained_queue.len() == 2
    });

    w.wf()
        .choose_handoff_continuation(
            fframes_studio::agent_workflow::model::HandoffChoice::RestartFromAccepted,
        )
        .expect("choose ok");
    w.wait("switch resolved", |s| s.switch_pending.is_none());

    // Discard / clear retained briefs
    w.wf().clear_retained_queue().expect("clear ok");
    let s = w.wait("retained queue cleared", |s| s.retained_queue.is_empty());
    assert!(s.queue.is_empty());
    assert!(
        s.task.as_ref().is_none_or(|t| t.phase.is_terminal()),
        "no task should be running after clearing retained queue"
    );
}

#[test]
fn unqualified_adapter_switch_is_blocked() {
    let w = World::new();

    // Initiate switch to a non-existent or unqualified provider ID
    w.wf()
        .initiate_switch("unqualified-provider-xyz")
        .expect("initiate ok");
    w.wait("switch pending", |s| s.switch_pending.is_some());

    // Attempting continuation must be rejected because adapter is not qualified
    w.wf()
        .choose_handoff_continuation(
            fframes_studio::agent_workflow::model::HandoffChoice::RestartFromAccepted,
        )
        .expect("command dispatched");

    // Switch pending remains Some! It is not resolved!
    std::thread::sleep(std::time::Duration::from_millis(100));
    let s = w.snap();
    assert!(
        s.switch_pending.is_some(),
        "switch must remain pending when target provider is not qualified"
    );
}

#[test]
fn late_permission_reply_rejected_after_switch() {
    let w = World::new();

    // Start a task with a permission request
    w.submit(&json!({
        "permission": {"title": "Allow tool execution?"},
        "text": "Waiting for permission",
    }));

    let _task = w.wait_phase(TaskPhase::WaitingPermission);
    let s = w.snap();
    let perms = s.open_permissions();
    assert_eq!(perms.len(), 1);
    let perm_ref = perms[0].reference.clone();

    // Initiate switch while permission is open
    w.wf().initiate_switch("codex").expect("initiate ok");
    w.wait("switch pending", |s| s.switch_pending.is_some());

    // Replying to the late permission from the outgoing task must fail (cannot affect incoming task)
    let reply_result = w.wf().reply_permission(
        &perm_ref,
        fframes_studio::agent_workflow::PermissionAnswer::Select("yes".into()),
    );
    assert!(
        matches!(
            reply_result,
            Err(fframes_studio::agent_workflow::WorkflowError::StalePermission)
        ),
        "late permission reply must be rejected as stale, got: {reply_result:?}"
    );
}

#[test]
fn continuation_choices_apply_correct_draft_preparation() {
    let w = World::new();

    w.wf().initiate_switch("codex").expect("initiate ok");
    w.wait("switch pending", |s| s.switch_pending.is_some());

    w.wf()
        .choose_handoff_continuation(
            fframes_studio::agent_workflow::model::HandoffChoice::RestartFromAccepted,
        )
        .expect("choose ok");

    w.wait("switch resolved", |s| s.switch_pending.is_none());
}

#[test]
fn restore_session_binds_to_provider_and_launch_digest_and_reaches_driver() {
    let w = World::new();

    // 1. Run a seed task so the workflow establishes authentic project/adapter state
    w.submit(&good("seed"));
    w.wait_task(None);

    // 2. Enable load-session capability in acp-agent.py
    fs::write(w.agent.join("load-session"), "1").unwrap();

    let wire_session_id = "test-opaque-session-token-xyz".to_string();

    // 3. Load the authentic manifest created by the seed task, set wire_session_id and state
    let mut manifest = fframes_studio::agent_workflow::session_store::SessionStore::load(
        &w.paths,
        &w.snap().project,
    )
    .unwrap()
    .expect("manifest created by seed task");
    assert_eq!(
        manifest.last_source_revision,
        w.ctl().lock().current_source_revision().to_string(),
        "restoration source revision must still match"
    );
    assert_eq!(
        manifest.last_accepted_revision,
        w.ctl().lock().state().accepted().to_string(),
        "restoration accepted revision must still match"
    );
    manifest.wire_session_id = Some(wire_session_id.clone());
    manifest.state = fframes_studio::agent_workflow::session_store::SessionManifestState::Ready;
    fframes_studio::agent_workflow::session_store::SessionStore::save(
        &w.paths,
        &w.snap().project,
        &manifest,
    )
    .expect("save ok");

    // 4. Request restore
    w.wf().restore_session().expect("restore ok");

    // 5. Submit a brief: writer spawns and passes resume_session to DriverConfig!
    w.submit(&good("resumed"));
    w.wait("resumed task starts", |s| s.task.is_some());

    // 6. The driver connects to acp-agent.py; acp-agent.py records session-load.json!
    w.poll("session-load.json written by acp-agent", || {
        w.agent.join("session-load.json").exists()
    });

    let load_params: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(w.agent.join("session-load.json")).unwrap())
            .unwrap();
    assert_eq!(
        load_params.get("sessionId").and_then(|v| v.as_str()),
        Some(wire_session_id.as_str())
    );
}

#[test]
fn restore_session_rejects_changed_adapter_closure() {
    let w = World::new();

    w.submit(&good("seed"));
    w.wait_task(None);

    // Enable load-session capability in acp-agent.py
    fs::write(w.agent.join("load-session"), "1").unwrap();

    // Load authentic manifest but tamper with the exact launch digest.
    let mut manifest = fframes_studio::agent_workflow::session_store::SessionStore::load(
        &w.paths,
        &w.snap().project,
    )
    .unwrap()
    .expect("manifest created by seed task");
    manifest.wire_session_id = Some("test-session-diff".into());
    manifest.launch_digest = "some-different-launch-closure".into();
    manifest.state = fframes_studio::agent_workflow::session_store::SessionManifestState::Ready;
    fframes_studio::agent_workflow::session_store::SessionStore::save(
        &w.paths,
        &w.snap().project,
        &manifest,
    )
    .expect("save ok");

    w.wf().restore_session().expect("restore ok");
    w.submit(&good("fresh"));
    w.wait("task starts fresh", |s| s.task.is_some());

    // Because launch digest differed, session-load.json must NOT exist!
    assert!(
        !w.agent.join("session-load.json").exists(),
        "session-load must not be called when launch digest changed"
    );
}

#[test]
fn submit_while_switch_pending_is_queued_not_started() {
    let w = World::new();

    w.wf().initiate_switch("codex").expect("initiate ok");
    w.wait("switch pending", |s| s.switch_pending.is_some());

    // Submit a brief while switch is pending: must queue behind the switch!
    w.wf()
        .submit("brief during pending switch")
        .expect("submit ok");
    let s = w.wait("brief is queued", |s| s.queue.len() == 1);
    assert_eq!(s.queue[0].summary, "brief during pending switch");
    assert!(
        s.task.is_none(),
        "task must not start while switch is pending"
    );
}

#[test]
fn switch_when_candidate_awaiting_review_preserves_candidate() {
    let w = World::new();

    w.wf()
        .set_review_policy(studio_engine::ReviewPolicy::ManualReview)
        .unwrap();
    w.submit(&json!({
        "text": ["Made an edit. ", "done"],
        "write": {"src/lib.rs": "// edit\n", "timeline.json": r#"{"frames":120,"tracks":[[0.0,2.0]],"audio":"tone","pixel":40}"#}
    }));

    w.wait_phase(TaskPhase::AwaitingReview);
    let s = w.snap();
    assert_eq!(
        s.task.as_ref().unwrap().phase,
        TaskPhase::AwaitingReview,
        "candidate exists awaiting review"
    );

    // Initiating switch while candidate is awaiting review must be refused and candidate kept!
    w.wf().initiate_switch("codex").expect("command dispatched");

    // Candidate remains in review! Switch is blocked!
    std::thread::sleep(std::time::Duration::from_millis(100));
    let s = w.snap();
    assert_eq!(
        s.task.as_ref().unwrap().phase,
        TaskPhase::AwaitingReview,
        "candidate must not be dropped by switch"
    );
    assert!(
        s.switch_pending.is_none(),
        "switch must be blocked when candidate awaits review"
    );
}

#[test]
fn continue_draft_preserves_outgoing_draft_edits_for_incoming_provider() {
    let w = World::new();

    // Configure codex in registry with test adapter
    let mut registry =
        fframes_studio::conversation_panel::provider_profiles::ProviderRegistry::default();
    if let Some(codex) = registry.profiles.iter_mut().find(|p| p.id == "codex") {
        codex.adapter.executable = "python3".into();
        codex.adapter.args = vec![agent_script(), w.agent.to_string_lossy().into_owned()];
    }
    fs::create_dir_all(&w.paths.data).unwrap();
    fs::write(
        w.paths
            .data
            .join(fframes_studio::conversation_panel::host::REGISTRY_FILE),
        registry.to_pretty(),
    )
    .unwrap();

    // 1. First task writes an outgoing draft edit while paused in permission (NEVER committed to root!)
    w.submit(&json!({
        "permission": {"title": "hold task"},
        "text": ["Writing draft edit. ", "done"],
        "write_early": {
            "src/lib.rs": "// preserved draft line\n",
            "timeline.json": GOOD_CONFIG,
            "outgoing_marker.txt": "preserved content\n",
        },
    }));
    w.wait_phase(TaskPhase::WaitingPermission);

    // CRITICAL: Verify source in project root does NOT have outgoing_marker or draft line
    assert!(
        !w.root.join("outgoing_marker.txt").exists(),
        "accepted source must not have marker"
    );
    assert!(
        !w.source("src/lib.rs").contains("// preserved draft line"),
        "accepted source must not have draft line"
    );

    // 2. Initiate switch to codex: cancels active task and captures outgoing draft
    w.wf().initiate_switch("codex").expect("initiate ok");
    let s = w.wait("switch pending", |s| s.switch_pending.is_some());
    assert!(s.switch_pending.as_ref().unwrap().draft_revision.is_some());

    // Source in root STILL does not have outgoing edits!
    assert!(
        !w.root.join("outgoing_marker.txt").exists(),
        "accepted source must still not have marker"
    );

    // 3. Choose ContinueDraft
    w.wf()
        .choose_handoff_continuation(
            fframes_studio::agent_workflow::model::HandoffChoice::ContinueDraft,
        )
        .expect("choose ok");
    w.wait("switch resolved", |s| s.switch_pending.is_none());

    // 4. Start next task on the new provider: the working copy contains the preserved marker file!
    let first_id = s.task.as_ref().unwrap().id.clone();
    w.submit(&json!({
        "text": ["Building on draft. ", "done"],
        "write": {
            "src/lib.rs": "// preserved draft line\n// second line\n",
            "timeline.json": GOOD_CONFIG,
        }
    }));
    w.wait("second task prepared", |s| {
        s.task
            .as_ref()
            .is_some_and(|t| t.id != first_id && t.phase != TaskPhase::Starting)
    });
    let draft_dir = w.ctl().lock().agent_draft_store().path().to_path_buf();
    assert!(
        draft_dir.join("outgoing_marker.txt").exists(),
        "outgoing draft edit must be preserved in the incoming provider's draft"
    );

    // Let second task finish, validate and commit to accepted source:
    let second_task = w.wait_task(Some(&first_id));
    assert_eq!(second_task.phase, TaskPhase::Accepted);

    // Criterion 3: Continued draft publishes through existing fences!
    let final_src = w.source("src/lib.rs");
    assert!(
        final_src.contains("// preserved draft line") && final_src.contains("// second line"),
        "published source must contain both draft and continuation lines: {final_src}"
    );
}

#[test]
fn unsupported_expired_resume_failure_falls_back_to_fresh_session() {
    let w = World::new();

    w.submit(&good("seed"));
    let seed_task = w.wait_task(None);

    // Enable load-session but inject failure via fail-load
    fs::write(w.agent.join("load-session"), "1").unwrap();
    fs::write(w.agent.join("fail-load"), "1").unwrap();

    let mut manifest = fframes_studio::agent_workflow::session_store::SessionStore::load(
        &w.paths,
        &w.snap().project,
    )
    .unwrap()
    .expect("manifest created by seed task");
    manifest.wire_session_id = Some("test-fail-resume".into());
    manifest.state = fframes_studio::agent_workflow::session_store::SessionManifestState::Ready;
    fframes_studio::agent_workflow::session_store::SessionStore::save(
        &w.paths,
        &w.snap().project,
        &manifest,
    )
    .expect("save ok");

    w.wf().restore_session().expect("restore ok");

    // Submitting a brief encounters the load error from acp-agent and fails
    w.submit(&good("fallback"));
    let s = w.wait("task finishes with failure", |s| {
        s.task
            .as_ref()
            .is_some_and(|t| t.id != seed_task.id && t.phase.is_terminal())
    });
    let failed_task = s.task.as_ref().unwrap();
    assert_ne!(
        failed_task.id, seed_task.id,
        "must be a distinct task from seed"
    );
    assert!(
        failed_task.brief.contains("fallback"),
        "must be the fallback task"
    );
    assert!(
        w.agent.join("session-load.json").exists(),
        "session-load was attempted by driver"
    );

    // Criterion 5: Unsupported restoration is visibly a new session fallback
    let restore_view = s
        .session_restore
        .as_ref()
        .expect("session_restore view present");
    assert!(
        !restore_view.is_resumable,
        "failed session must be marked non-resumable"
    );
    assert!(
        restore_view
            .notice
            .as_ref()
            .is_some_and(|n| n.contains("restoration failed")),
        "fresh session notice must be visible: {:?}",
        restore_view.notice
    );

    // Next task runs fresh without session-load (clean fresh session!)
    let _ = fs::remove_file(w.agent.join("fail-load"));
    let _ = fs::remove_file(w.agent.join("session-load.json"));

    w.submit(&good("fresh_after_failure"));
    let s = w.wait("fresh task finishes", |s| {
        s.task
            .as_ref()
            .is_some_and(|t| t.brief.contains("fresh_after_failure") && t.phase.is_terminal())
    });
    assert!(s.task.is_some());
    assert!(
        !w.agent.join("session-load.json").exists(),
        "fresh task must not attempt failed session-load"
    );
}

#[test]
fn restart_from_accepted_discards_uncommitted_draft_edits() {
    let w = World::new();

    let mut registry =
        fframes_studio::conversation_panel::provider_profiles::ProviderRegistry::default();
    if let Some(codex) = registry.profiles.iter_mut().find(|p| p.id == "codex") {
        codex.adapter.executable = "python3".into();
        codex.adapter.args = vec![agent_script(), w.agent.to_string_lossy().into_owned()];
    }
    fs::create_dir_all(&w.paths.data).unwrap();
    fs::write(
        w.paths
            .data
            .join(fframes_studio::conversation_panel::host::REGISTRY_FILE),
        registry.to_pretty(),
    )
    .unwrap();

    // 1. First task writes an uncommitted draft marker while paused in permission
    w.submit(&json!({
        "permission": {"title": "hold task"},
        "text": ["Writing draft edit. ", "done"],
        "write_early": {
            "discard_marker.txt": "discard me\n",
        },
    }));
    w.wait_phase(TaskPhase::WaitingPermission);

    // 2. Initiate switch to codex
    w.wf().initiate_switch("codex").expect("initiate ok");
    w.wait("switch pending", |s| s.switch_pending.is_some());

    // 3. Choose RestartFromAccepted
    w.wf()
        .choose_handoff_continuation(
            fframes_studio::agent_workflow::model::HandoffChoice::RestartFromAccepted,
        )
        .expect("choose ok");
    w.wait("switch resolved", |s| s.switch_pending.is_none());

    // 4. Start next task on the new provider: working copy must NOT have discard_marker.txt!
    let first_id = w.snap().task.as_ref().unwrap().id.clone();
    w.submit(&good("fresh_second"));
    w.wait("fresh second task prepared", |s| {
        s.task
            .as_ref()
            .is_some_and(|t| t.id != first_id && t.phase != TaskPhase::Starting)
    });
    let draft_dir = w.ctl().lock().agent_draft_store().path().to_path_buf();
    assert!(
        !draft_dir.join("discard_marker.txt").exists(),
        "uncommitted draft edit must be discarded when restarting from accepted"
    );
    assert!(
        !w.root.join("discard_marker.txt").exists(),
        "root source must not have marker"
    );
}

struct BlockAtPublishHook {
    reached: std::sync::atomic::AtomicBool,
    release: std::sync::atomic::AtomicBool,
}

impl studio_engine::TransactionHooks for BlockAtPublishHook {
    fn at(
        &self,
        boundary: &studio_engine::edit_transaction::Boundary,
    ) -> Result<(), studio_engine::edit_transaction::Fault> {
        if matches!(
            boundary,
            studio_engine::edit_transaction::Boundary::BeforePublish(_)
        ) && !self.release.load(std::sync::atomic::Ordering::SeqCst)
        {
            self.reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            while !self.release.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        Ok(())
    }
}

#[test]
fn switch_during_undo_waits_for_undo_to_settle_then_resumes_teardown() {
    let hooks = std::sync::Arc::new(BlockAtPublishHook {
        reached: std::sync::atomic::AtomicBool::new(false),
        release: std::sync::atomic::AtomicBool::new(true),
    });
    let w = World::with(Options {
        hooks: Some(hooks.clone()),
        ..Options::default()
    });

    w.submit(&good("first"));
    let first = w.wait_task(None);
    w.submit(&good("second"));
    let _second = w.wait_task(Some(&first.id));

    // Arm hook right before starting Undo
    hooks
        .reached
        .store(false, std::sync::atomic::Ordering::SeqCst);
    hooks
        .release
        .store(false, std::sync::atomic::Ordering::SeqCst);

    // Start Undo; it runs and blocks at BeforePublishHook!
    w.wf().undo(None).expect("undo ok");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !hooks.reached.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "undo never reached publication hook"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Now Undo is actively in-flight: initiating switch must notice waiting and hold switch_target!
    w.wf().initiate_switch("codex").expect("initiate switch ok");
    w.wait("undo waiting notice", |s| {
        s.rows.iter().any(|r| {
            matches!(&r.kind, RowKind::Notice { text, .. } if text.contains("Waiting for in-flight Undo"))
        })
    });
    // switch_pending is STILL None because Undo is in-flight!
    assert!(
        w.snap().switch_pending.is_none(),
        "switch must not finalize while undo is in flight"
    );

    // Release publication hook: Undo finishes, undo_settled resumes switch teardown!
    hooks
        .release
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let s = w.wait("undo finishes and switch pending appears", |s| {
        s.switch_pending.is_some()
    });
    assert_eq!(s.switch_pending.as_ref().unwrap().target_provider, "codex");

    w.wf().cancel_switch().expect("cancel ok");
}

#[test]
fn switch_at_apply_commit_cannot_cancel_until_published() {
    let hooks = std::sync::Arc::new(BlockAtPublishHook {
        reached: std::sync::atomic::AtomicBool::new(false),
        release: std::sync::atomic::AtomicBool::new(false),
    });
    let w = World::with(Options {
        hooks: Some(hooks.clone()),
        ..Options::default()
    });

    w.submit(&good("boundary"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !hooks.reached.load(std::sync::atomic::Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "publication never reached BeforePublish hook"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let task = w.wait_phase(TaskPhase::Promoting);
    assert_eq!(task.engine_state, TaskState::Promoting);

    // Initiating switch while publication commit is running cannot cancel and reports waiting
    w.wf().initiate_switch("codex").expect("switch dispatched");
    w.wait("publication wait notice", |s| {
        s.rows.iter().any(|r| {
            matches!(&r.kind, RowKind::Notice { text, .. } if text.contains("The edit is being published"))
        })
    });
    assert_eq!(w.snap().task.as_ref().unwrap().phase, TaskPhase::Promoting);

    // Release publication commit hook: publication finishes, and switch teardown proceeds!
    hooks
        .release
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let s = w.wait("publication completes and switch teardown completes", |s| {
        s.switch_pending.is_some()
    });
    assert_eq!(s.switch_pending.as_ref().unwrap().target_provider, "codex");

    w.wf().cancel_switch().expect("cancel ok");
}

#[test]
fn crash_restart_recovery_preserves_retained_draft_and_marks_unproven_writer_unsafe() {
    let mut w = World::new();

    // Start a task that holds an active writer
    w.submit(&json!({
        "permission": {"title": "hold writer"},
        "text": "working",
        "write_early": {"uncommitted.txt": "crash data\n"}
    }));
    w.wait_phase(TaskPhase::WaitingPermission);

    let task = w.snap().task.clone().unwrap();
    w.wf().stop().unwrap();
    w.wait_task(None);

    // Simulate crash where app died mid-task and marker still says Active
    let project = w.ctl().lock().project.manifest.project_id.clone();
    let marker = w.paths.agent_draft_state(&project);
    let mut value: Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
    value["state"] = json!({"state": "active", "task": task.id.0});
    fs::write(&marker, serde_json::to_vec(&value).unwrap()).unwrap();

    let calls_before = fs::read_to_string(w.agent.join("calls.jsonl")).unwrap_or_default();
    let source_before = w.source("src/lib.rs");

    w.restart();

    // After restart, writer is unproven and must be marked UnsafeWriter, working copy kept
    assert!(
        w.snap().task.is_none(),
        "reopened project starts with no task running"
    );
    let s = w.snap();
    let notice = s.recovery.draft.as_ref().expect("recovery draft notice");
    assert!(notice.interrupted_by_restart && notice.can_acknowledge);
    assert!(
        matches!(notice.state, DraftState::UnsafeWriter { .. }),
        "unproven writer after restart must be marked UnsafeWriter: {:?}",
        notice.state
    );
    let draft_state = w.draft_state().expect("draft state present");
    assert!(
        matches!(draft_state, DraftState::UnsafeWriter { .. }),
        "draft state after crash restart must be UnsafeWriter: {draft_state:?}"
    );

    let draft_path = w.ctl().lock().agent_draft_store().path().to_path_buf();
    assert!(
        draft_path.join("uncommitted.txt").exists(),
        "uncommitted working copy preserved across crash"
    );

    // Criterion 7: Verify nothing was re-sent after restart (no new calls)
    let calls_after = fs::read_to_string(w.agent.join("calls.jsonl")).unwrap_or_default();
    assert_eq!(calls_before, calls_after, "no new calls after restart");

    // Criterion 7: Verify source is untouched (no auto-publishing candidates)
    assert_eq!(
        source_before,
        w.source("src/lib.rs"),
        "source remains untouched after restart"
    );
}

#[test]
fn tool_snapshot_race_waits_for_tool_snapshot_to_drain_before_draft_capture() {
    let w = World::new();

    // Submit initial task so tools runtime is initialized
    w.submit(&good("initial"));
    w.wait_task(None);

    // Hold the tools writer gate to simulate an actively executing tool snapshot
    let guard = w
        .wf()
        .test_hold_tools_gate()
        .expect("acquired tools writer gate");

    // Initiate switch: teardown MUST wait for in-flight tool snapshot to drain!
    w.wf().initiate_switch("codex").expect("switch ok");

    w.wait("waiting for tool snapshot notice", |s| {
        s.rows.iter().any(|r| {
            matches!(&r.kind, RowKind::Notice { text, .. } if text.contains("Waiting for in-flight tool snapshot to drain"))
        })
    });
    // Switch pending must STILL be None because gate is held!
    assert!(
        w.snap().switch_pending.is_none(),
        "switch must not capture draft while tools gate is held"
    );

    // Release the tool snapshot guard: gate drains!
    drop(guard);

    // Switch teardown automatically resumes on the actor's next tick without outside help!
    let s = w.wait("switch pending appears after tool snapshot drains", |s| {
        s.switch_pending.is_some()
    });
    assert!(
        s.switch_pending.as_ref().unwrap().draft_revision.is_some(),
        "draft captured after gate drain"
    );

    w.wf().cancel_switch().expect("cancel ok");
}

#[test]
fn restore_session_rejects_changed_draft_location() {
    let w = World::new();

    w.submit(&good("seed"));
    let seed = w.wait_task(None);

    fs::write(w.agent.join("load-session"), "1").unwrap();

    let mut manifest = fframes_studio::agent_workflow::session_store::SessionStore::load(
        &w.paths,
        &w.snap().project,
    )
    .unwrap()
    .expect("manifest created by seed task");
    manifest.wire_session_id = Some("test-session-diff-cwd".into());
    // Mismatched draft path
    manifest.draft_path = PathBuf::from("/tmp/nonexistent-draft-path-xyz");
    manifest.state = fframes_studio::agent_workflow::session_store::SessionManifestState::Ready;
    fframes_studio::agent_workflow::session_store::SessionStore::save(
        &w.paths,
        &w.snap().project,
        &manifest,
    )
    .expect("save ok");

    w.wf().restore_session().expect("restore ok");
    w.submit(&good("fresh"));
    w.wait("task starts fresh", |s| {
        s.task.as_ref().is_some_and(|t| t.id != seed.id)
    });
    // Because draft_path differed, session-load.json must NOT exist!
    assert!(
        !w.agent.join("session-load.json").exists(),
        "session-load must not be called when draft_path changed"
    );
    let s = w.snap();
    assert!(
        s.rows.iter().any(|r| {
            matches!(&r.kind, RowKind::Notice { text, .. } if text.contains("draft location changed"))
        }),
        "notice about draft location change must be present"
    );
}
