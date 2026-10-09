//! Scoped editing through the native workflow against the scripted ACP peer and the
//! deterministic fake worker: the frozen scope, the before-evidence paragraph of the first
//! prompt and refusal of a stale queued scope. Same harness as `agent_workflow.rs`. The
//! peer proves workflow behaviour only, never a provider.
//! These end-to-end cases require successful Apply, which is currently qualified only on Linux.
#![cfg(target_os = "linux")]
#![allow(dead_code, unused_imports)]
include!("support/workflow_world.rs");

fn identity(w: &World) -> fframes_studio_protocol::PreviewIdentity {
    let controller = w.ctl();
    let controller = controller.lock();
    fframes_studio_protocol::PreviewIdentity {
        project_id: String::from(controller.project.manifest.project_id.clone()),
        open_session: "session-1".into(),
        source_revision: controller.project.inventory.revision.as_str().into(),
        worker_generation: 1,
    }
}

fn scene_scope(initial: &fframes_studio_protocol::PreviewIdentity) -> TaskScope {
    let scene = studio_engine::ScopedScene {
        instance_id: "worker-1:intro".into(),
        name: "Intro".into(),
        full_name: "video::Intro".into(),
        start_frame: 0,
        end_frame: 30,
    };
    TaskScope {
        project_id: initial.project_id.clone(),
        source_revision: initial.source_revision.clone(),
        selection: ScopeSelection::Scene {
            instance_id: scene.instance_id.clone(),
            name: scene.name.clone(),
            full_name: scene.full_name.clone(),
        },
        compiled: Some(CompiledScope {
            preview: initial.clone(),
            fps: 30,
            total_frames: 60,
            start_frame: 0,
            end_frame: 30,
            scenes: vec![scene],
            boundary_frames: vec![0, 29, 30],
            scene_context_truncated: false,
        }),
        scene_sources: vec![],
        scene_source_search_truncated: false,
        style_snapshot: None,
        canvas_selection: None,
    }
}

#[test]
fn a_scene_scope_is_frozen_into_the_task_and_its_first_prompt_carries_references_not_pixels() {
    let w = World::new();
    let initial = identity(&w);
    w.wf()
        .set_displayed_preview_identity(Some(initial.clone()))
        .unwrap();
    w.wf()
        .submit_scoped(&World::brief(&good("scoped")), scene_scope(&initial))
        .unwrap();
    let task = w.wait_task(None);
    assert_eq!(task.scope.label(), "Scene Intro · [0..30)");
    assert!(
        task.image_limitation.is_some(),
        "the text-only limit is visible"
    );
    let before = task
        .before
        .expect("a scoped task has a before-evidence record");
    // Without a real SDK the tool backend cannot render: the record must say so, never
    // carry a placeholder, and the prompt must carry the same account.
    assert!(
        matches!(
            before.state,
            EvidenceState::Unavailable | EvidenceState::Ready | EvidenceState::Released
        ),
        "{before:?}"
    );
    if before.state == EvidenceState::Unavailable {
        assert!(before.artifacts.is_empty() && before.note.is_some());
    }
    let after = task
        .after
        .expect("AutoApply retains a candidate after-evidence result");
    assert_ne!(before.revision, after.revision);
    match after.state {
        EvidenceState::Released => assert!(!after.artifacts.is_empty()),
        EvidenceState::Unavailable => assert!(after.artifacts.is_empty() && after.note.is_some()),
        state => panic!("terminal AutoApply task retained unsettled evidence: {state:?}"),
    }
    if before.state != EvidenceState::Unavailable && after.state == EvidenceState::Released {
        assert_eq!(
            before
                .artifacts
                .iter()
                .map(|artifact| (&artifact.label, &artifact.frames))
                .collect::<Vec<_>>(),
            after
                .artifacts
                .iter()
                .map(|artifact| (&artifact.label, &artifact.frames))
                .collect::<Vec<_>>(),
            "before and after evidence render the same scope frames"
        );
    }
    let prompts = w.evidence("prompts.jsonl");
    let text = prompts[0]["text"].as_str().unwrap();
    assert!(text.contains("Requested scope: Scene Intro"), "{text}");
    assert!(
        text.contains("Before evidence (frozen source base"),
        "{text}"
    );
    assert!(
        text.contains("Text-only: this adapter did not negotiate ACP image prompts"),
        "text-only adapters receive a visible fallback explanation: {text}"
    );
    assert!(
        !text.contains("base64") && !text.contains("data:image"),
        "{text}"
    );
    w.assert_clean();
}

#[test]
fn a_failed_before_evidence_render_does_not_settle_candidate_evidence() {
    let failed = Arc::new(AtomicBool::new(false));
    let fail_before = failed.clone();
    let job_faults: JobFaults = Arc::new(move |job| {
        if job == "evidence" && !fail_before.swap(true, Ordering::AcqRel) {
            JobFault::Panic
        } else {
            JobFault::None
        }
    });
    let w = World::with(Options {
        job_faults: Some(job_faults),
        ..Options::default()
    });
    let initial = identity(&w);
    w.wf()
        .set_displayed_preview_identity(Some(initial.clone()))
        .unwrap();
    w.wf()
        .submit_scoped(
            &World::brief(&good("evidence worker panic")),
            scene_scope(&initial),
        )
        .unwrap();

    let task = w.wait_task(None);
    let before = task.before.expect("before evidence record");
    assert_eq!(before.state, EvidenceState::Unavailable);
    assert!(
        before
            .note
            .as_deref()
            .is_some_and(|note| note.contains("injected worker failure"))
    );

    let after = task.after.expect("candidate evidence record");
    assert!(
        matches!(
            after.state,
            EvidenceState::Released | EvidenceState::Unavailable
        ),
        "the candidate evidence render must settle independently of the before render: {after:?}"
    );
    if after.state == EvidenceState::Unavailable {
        assert!(after.note.is_some());
    }
    assert_ne!(before.revision, after.revision);
    let prompts = w.evidence("prompts.jsonl");
    assert_eq!(prompts.len(), 1);
    assert!(
        prompts[0]["text"]
            .as_str()
            .unwrap()
            .contains("Before evidence")
    );
    w.assert_clean();
}

#[test]
fn a_whole_project_task_has_no_before_evidence_and_says_it_was_not_scoped() {
    let w = World::new();
    w.submit(&good("whole"));
    let task = w.wait_task(None);
    assert!(task.before.is_none() && task.image_limitation.is_none());
    let prompts = w.evidence("prompts.jsonl");
    let text = prompts[0]["text"].as_str().unwrap();
    assert!(text.contains("Requested scope: whole project"), "{text}");
    assert!(!text.contains("Before evidence"), "{text}");
    w.assert_clean();
}

#[test]
fn a_queued_scope_is_visible_and_refused_when_the_displayed_preview_changes() {
    let w = World::new();
    let initial = {
        let controller = w.ctl();
        let controller = controller.lock();
        fframes_studio_protocol::PreviewIdentity {
            project_id: String::from(controller.project.manifest.project_id.clone()),
            open_session: "session-1".into(),
            source_revision: controller.project.inventory.revision.as_str().into(),
            worker_generation: 1,
        }
    };
    let scope = TaskScope {
        project_id: initial.project_id.clone(),
        source_revision: initial.source_revision.clone(),
        selection: ScopeSelection::FrameRange,
        compiled: Some(CompiledScope {
            preview: initial.clone(),
            fps: 30,
            total_frames: 60,
            start_frame: 10,
            end_frame: 20,
            scenes: vec![],
            boundary_frames: vec![9, 10, 19, 20],
            scene_context_truncated: false,
        }),
        scene_sources: vec![],
        scene_source_search_truncated: false,
        style_snapshot: None,
        canvas_selection: None,
    };
    w.wf()
        .set_displayed_preview_identity(Some(initial.clone()))
        .unwrap();

    let mut first = good("one");
    first["wait_file"] = json!("go");
    w.submit(&first);
    w.wait_phase(TaskPhase::Editing);
    w.wf().submit_scoped("edit this range", scope).unwrap();
    let queued = w.wait("the scoped queued brief", |s| s.queue.len() == 1);
    assert_eq!(queued.queue[0].scope.as_deref(), Some("Frames [10..20)"));
    assert!(!queued.queue[0].stale_scope);

    let mut changed = initial;
    changed.worker_generation += 1;
    w.wf()
        .set_displayed_preview_identity(Some(changed))
        .unwrap();
    let stale = w.wait("the queue stale marker", |s| {
        s.queue.len() == 1 && s.queue[0].stale_scope
    });
    assert!(stale.queue[0].stale_scope);

    w.release("go");
    let refused = w.wait("the stale scope refusal", |s| {
        s.queue.is_empty() && has_error(s, "stale_task_scope")
    });
    assert!(has_error(&refused, "stale_task_scope"));
    assert_eq!(w.evidence("prompts.jsonl").len(), 1);
    w.assert_clean();
}
