use std::{cell::Cell, fs, path::Path, time::Duration};
#[cfg(target_os = "linux")]
use std::{
    io::{BufRead, BufReader},
    time::Instant,
};
use studio_bootstrap::{SpawnOptions, TerminationReport, WriterOwnership};
use studio_engine::{
    AgentTaskContext, CaptureTicket, Controller, DraftState, EngineError, JobKind, JobState,
    QuiescenceBlock, QuiescenceEvidence, RepairDecision, TaskError, TaskIdentity, TaskState,
    TurnCompletion, WriterGeneration, WriterGoneEvidence, WriterObservation, app_paths::AppPaths,
    build_materialization::sdk_pin,
};
use studio_project::SourceRevision;
use studio_sdk::CompatibilityManifest;

#[path = "support/passing_probe.rs"]
mod passing_probe;

struct Fixture {
    _temp: tempfile::TempDir,
    root: std::path::PathBuf,
    paths: AppPaths,
}

fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    let compatibility = CompatibilityManifest::default_linux_x64();
    studio_project::create(
        &root,
        "Video",
        sdk_pin(&compatibility),
        &compatibility.fframes_version,
        "0.1.0",
    )
    .unwrap();
    Fixture {
        _temp: temp,
        root,
        paths,
    }
}

fn qualified() -> WriterOwnership {
    WriterOwnership::ProcessGroupContained {
        qualification: "test-adapter".into(),
    }
}

fn clean_termination() -> TerminationReport {
    TerminationReport {
        forced: false,
        direct_child_exited: true,
        group_empty: true,
        remaining: vec![],
    }
}

/// Clean end-turn evidence bound to the task's current provider session and writer
/// generation. Termination is deliberately not part of it: the controller derives that
/// from the task's own process scope.
fn evidence(controller: &Controller, id: &TaskIdentity) -> QuiescenceEvidence {
    let task = controller.agent_task().unwrap();
    QuiescenceEvidence {
        identity: id.clone(),
        provider_session: task.provider_session().map(str::to_owned),
        writer: task.writer_generation().expect("a writer was started"),
        completion: TurnCompletion::EndTurn,
        cancel_requested: false,
        unresolved_requests: 0,
        observed: WriterObservation {
            ownership: qualified(),
            escaped_pids: vec![],
        },
    }
}

fn sleeper() -> SpawnOptions {
    #[cfg(unix)]
    let mut options = SpawnOptions::new("sleep");
    #[cfg(unix)]
    options.arg("30");

    #[cfg(windows)]
    let options = {
        let mut options = SpawnOptions::new("powershell.exe");
        options.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Sleep -Seconds 30",
        ]);
        options
    };

    options
}

fn generation(value: u64) -> WriterGeneration {
    serde_json::from_str(&value.to_string()).unwrap()
}

fn quiesce(controller: &mut Controller, id: &TaskIdentity) -> Result<CaptureTicket, TaskError> {
    let evidence = evidence(controller, id);
    controller
        .agent_complete_quiescence(id, &evidence)
        .map_err(task_error)
}

/// A real, validated-for-this-task passing report is the only route to `CandidateReady`.
fn accept_candidate(controller: &mut Controller, id: &TaskIdentity) {
    let ticket = quiesce(controller, id).unwrap();
    let captured = controller.agent_capture_candidate(ticket).unwrap();
    let report = passing_probe::passing_report(&captured);
    assert_eq!(
        controller.agent_apply_validation(id, &report).unwrap(),
        studio_engine::candidate_validation::NextStep::Accept
    );
}

fn task_error(error: EngineError) -> TaskError {
    match error {
        EngineError::Task(task) => task,
        other => panic!("expected a task error, got {other}"),
    }
}

fn current_revision(root: &Path) -> SourceRevision {
    studio_project::open(root).unwrap().inventory.revision
}

fn run_to_quiescence(controller: &mut Controller, context: &AgentTaskContext) {
    let id = &context.identity;
    controller.agent_writer_started(id, qualified()).unwrap();
    controller
        .agent_task_transition(id, TaskState::Editing)
        .unwrap();
    controller
        .agent_task_transition(id, TaskState::Quiescing)
        .unwrap();
}

fn capture(controller: &mut Controller, id: &TaskIdentity) {
    let ticket = quiesce(controller, id).unwrap();
    controller.agent_capture_candidate(ticket).unwrap();
}

#[test]
fn dirty_imported_source_is_the_task_base_not_the_older_checkpoint() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let checkpoint = controller.state().accepted().clone();
    // Source changed outside the app after the last checkpoint (a dirty import).
    fs::write(f.root.join("src/lib.rs"), "// dirty imported bytes").unwrap();
    let dirty = current_revision(&f.root);
    assert_ne!(dirty, checkpoint);

    let context = controller
        .begin_agent_task("  Make the title blue  ")
        .unwrap();
    assert_eq!(context.brief, "Make the title blue");
    assert_eq!(context.source_base.revision(), &dirty);
    assert_eq!(context.prior_checkpoint, checkpoint);
    assert_eq!(
        fs::read_to_string(context.draft.join("src/lib.rs")).unwrap(),
        "// dirty imported bytes",
        "the draft is the current source, never reconciled from the checkpoint"
    );
    assert_eq!(
        context.draft,
        f.paths.agent_draft(&context.identity.project)
    );
    assert!(context.draft.starts_with(&f.paths.data));
    // M1 checkpoint semantics are untouched: no job ran and accepted did not move.
    assert_eq!(controller.state().accepted(), &checkpoint);
    assert!(matches!(controller.state().job(), JobState::Idle));
    assert_eq!(context.identity.session, *controller.state().session());
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::ContextReady
    );
    assert!(
        context
            .assets
            .iter()
            .all(|a| a.path.as_str().starts_with("media/"))
    );
}

#[test]
fn context_lists_existing_assets_and_instruction_files() {
    let f = fixture();
    fs::create_dir_all(f.root.join("media")).unwrap();
    fs::write(f.root.join("media/clip.mp4"), b"not really a video").unwrap();
    fs::write(f.root.join("AGENTS.md"), "house rules").unwrap();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let assets: Vec<&str> = context.assets.iter().map(|a| a.path.as_str()).collect();
    assert!(assets.contains(&"media/clip.mp4"), "{assets:?}");
    assert!(assets.iter().all(|a| a.starts_with("media/")));
    assert_eq!(
        context
            .instructions
            .iter()
            .map(|a| a.path.as_str())
            .collect::<Vec<_>>(),
        ["AGENTS.md"]
    );
}

#[test]
fn repeat_tasks_reuse_the_stable_draft_after_archiving_the_retained_one() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let first = controller.begin_agent_task("first").unwrap();
    fs::write(first.draft.join("src/lib.rs"), "// failed attempt").unwrap();
    fs::write(first.draft.join("scratch.txt"), "notes").unwrap();
    controller
        .finish_agent_task(&first.identity, TaskState::Cancelled, "user stop")
        .unwrap();
    assert_eq!(
        controller.agent_draft_state().unwrap().unwrap(),
        DraftState::Retained {
            task: first.identity.task.0.clone(),
            reason: "Cancelled: user stop".into()
        }
    );

    let second = controller.begin_agent_task("second").unwrap();
    assert_eq!(second.draft, first.draft, "the cwd is stable across tasks");
    assert_ne!(second.identity.task, first.identity.task);
    assert_eq!(second.identity.generation, first.identity.generation + 1);
    let archived = second
        .archived_previous
        .clone()
        .expect("failed draft archived");
    assert_eq!(
        fs::read_to_string(archived.join("src/lib.rs")).unwrap(),
        "// failed attempt"
    );
    assert!(archived.join("scratch.txt").exists());
    assert!(
        !second.draft.join("scratch.txt").exists(),
        "refresh starts from the base again"
    );
    assert_eq!(second.source_base.revision(), first.source_base.revision());
    assert_ne!(
        fs::read_to_string(second.draft.join("src/lib.rs")).unwrap(),
        "// failed attempt"
    );
}

#[test]
fn only_one_task_holds_the_writer_lease_and_finishing_releases_it() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let first = controller.begin_agent_task("first").unwrap();
    let error = controller.begin_agent_task("second").err().unwrap();
    assert_eq!(
        task_error(error),
        TaskError::LeaseHeld(first.identity.task.clone())
    );
    controller
        .finish_agent_task(&first.identity, TaskState::Failed, "provider error")
        .unwrap();
    controller.begin_agent_task("second").unwrap();
}

#[test]
fn identities_from_an_earlier_generation_or_open_session_are_stale() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let first = controller.begin_agent_task("first").unwrap();
    controller
        .finish_agent_task(&first.identity, TaskState::Cancelled, "stop")
        .unwrap();
    let second = controller.begin_agent_task("second").unwrap();
    assert_eq!(
        task_error(
            controller
                .agent_task_transition(&first.identity, TaskState::Editing)
                .unwrap_err()
        ),
        TaskError::StaleIdentity
    );
    controller
        .agent_task_transition(&second.identity, TaskState::Editing)
        .unwrap();
    controller.close().unwrap();
    drop(controller);

    // Reopening yields a fresh open session: the old identity can never act again.
    let mut reopened = Controller::open(&f.root, &f.paths).unwrap();
    assert_eq!(
        task_error(
            reopened
                .agent_task_transition(&second.identity, TaskState::Quiescing)
                .unwrap_err()
        ),
        TaskError::NoActiveTask
    );
    let third = reopened.begin_agent_task("third").unwrap();
    assert_ne!(third.identity.session, second.identity.session);
    assert_eq!(
        third.identity.generation, 0,
        "generations restart per open session"
    );
    assert_eq!(
        task_error(
            reopened
                .agent_task_transition(&second.identity, TaskState::Quiescing)
                .unwrap_err()
        ),
        TaskError::StaleIdentity
    );
    assert_eq!(
        third.draft, second.draft,
        "the draft path is stable across reopen"
    );
}

#[test]
fn build_and_checkpoint_jobs_keep_their_semantics_while_a_task_is_active() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    fs::write(f.root.join("src/lib.rs"), "// edited before task").unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    controller.checkpoint().unwrap();
    assert!(matches!(controller.state().job(), JobState::Succeeded(_)));
    assert_eq!(
        controller.state().accepted(),
        context.source_base.revision()
    );
    let tag = controller.begin_job(JobKind::Build).unwrap();
    assert!(
        controller.draft().is_some(),
        "job drafts stay separate from the agent draft"
    );
    assert_ne!(controller.draft().unwrap(), context.draft);
    controller.cancel().unwrap();
    assert!(matches!(controller.state().job(), JobState::Interrupted(t) if *t == tag));
    // The task, its identity and its lease are unaffected by job churn.
    let task = controller.agent_task().unwrap();
    assert_eq!(task.state(), TaskState::ContextReady);
    assert_eq!(task.identity(), &context.identity);
    assert!(controller.begin_agent_task("again").is_err());
}

#[test]
fn unqualified_or_unverified_writers_keep_the_draft_unrefreshable() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let first = controller.begin_agent_task("first").unwrap();
    controller
        .agent_writer_started(&first.identity, WriterOwnership::Unknown)
        .unwrap();
    fs::write(
        first.draft.join("src/lib.rs"),
        "// maybe still being written",
    )
    .unwrap();
    controller
        .finish_agent_task(&first.identity, TaskState::Failed, "adapter crashed")
        .unwrap();
    assert!(matches!(
        controller.agent_draft_state().unwrap().unwrap(),
        DraftState::UnsafeWriter { .. }
    ));

    // Refresh is refused and the draft bytes are untouched.
    let error = controller.begin_agent_task("second").err().unwrap();
    assert!(matches!(task_error(error), TaskError::DraftUnsafe(_)));
    assert_eq!(
        fs::read_to_string(first.draft.join("src/lib.rs")).unwrap(),
        "// maybe still being written"
    );
    // A verified group termination alone cannot clear an unqualified ownership model.
    let weak = WriterGoneEvidence::QualifiedAndVerified {
        ownership: WriterOwnership::Unknown,
        termination: clean_termination(),
    };
    assert!(controller.acknowledge_agent_writer_gone(&weak).is_err());
    assert!(controller.begin_agent_task("second").is_err());

    controller
        .acknowledge_agent_writer_gone(&WriterGoneEvidence::UserConfirmed)
        .unwrap();
    let second = controller.begin_agent_task("second").unwrap();
    let archived = second.archived_previous.unwrap();
    assert_eq!(
        fs::read_to_string(archived.join("src/lib.rs")).unwrap(),
        "// maybe still being written",
        "the unsafe draft is archived, not overwritten"
    );
}

#[test]
fn a_qualified_verified_acknowledgement_clears_an_unsafe_marker() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let first = controller.begin_agent_task("first").unwrap();
    controller
        .agent_writer_started(&first.identity, qualified())
        .unwrap();
    // The driver saw a helper leave the task process group: the writer is detached.
    controller
        .agent_observe_writer(
            &first.identity,
            &WriterObservation {
                ownership: WriterOwnership::Detached,
                escaped_pids: vec![4242],
            },
        )
        .unwrap();
    controller
        .finish_agent_task(&first.identity, TaskState::Failed, "escaped helper")
        .unwrap();
    assert!(matches!(
        controller.agent_draft_state().unwrap().unwrap(),
        DraftState::UnsafeWriter { .. }
    ));
    assert!(controller.begin_agent_task("second").is_err());
    let still_detached = WriterGoneEvidence::QualifiedAndVerified {
        ownership: WriterOwnership::Detached,
        termination: clean_termination(),
    };
    assert!(
        controller
            .acknowledge_agent_writer_gone(&still_detached)
            .is_err()
    );
    controller
        .acknowledge_agent_writer_gone(&WriterGoneEvidence::QualifiedAndVerified {
            ownership: qualified(),
            termination: clean_termination(),
        })
        .unwrap();
    controller.begin_agent_task("second").unwrap();
}

#[test]
fn an_active_marker_left_by_a_crashed_session_is_unsafe_after_restart() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("first").unwrap();
    // Simulate an abnormal end: the marker still says active when the process is gone.
    let store = controller.agent_draft_store().clone();
    drop(controller);
    store
        .set_state(DraftState::Active {
            task: context.identity.task.0.clone(),
        })
        .unwrap();
    let mut reopened = Controller::open(&f.root, &f.paths).unwrap();
    assert!(matches!(
        reopened.agent_draft_state().unwrap().unwrap(),
        DraftState::UnsafeWriter { .. }
    ));
    assert!(matches!(
        task_error(reopened.begin_agent_task("again").err().unwrap()),
        TaskError::DraftUnsafe(_)
    ));
    assert!(context.draft.exists(), "the draft is retained");
}

#[test]
fn a_draft_refresh_is_refused_while_a_provider_session_is_live() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let first = controller.begin_agent_task("first").unwrap();
    controller
        .finish_agent_task(&first.identity, TaskState::Cancelled, "stop")
        .unwrap();
    // A finished task's scope is sealed and reaped, so a live session can only be
    // reported by the refresh guard itself.
    let store = controller.agent_draft_store().clone();
    let error = store
        .prepare(
            &controller.checkpoints,
            &first.source_base,
            &first.identity.task,
            true,
        )
        .unwrap_err();
    assert_eq!(error, TaskError::DraftBusy);
    assert!(
        first.draft.exists(),
        "the cwd is never changed under a live session"
    );
    controller.begin_agent_task("second").unwrap();
}

#[test]
fn validation_failure_spends_the_single_repair_then_fails_and_frees_the_draft() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    run_to_quiescence(&mut controller, &context);
    capture(&mut controller, &id);
    assert_eq!(
        controller
            .agent_validation_failed(&id, "compile error")
            .unwrap(),
        RepairDecision::Repair { attempt: 1 }
    );
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::RepairNeeded
    );
    assert_eq!(controller.agent_begin_repair(&id).unwrap(), 1);
    controller.agent_writer_started(&id, qualified()).unwrap();
    controller
        .agent_task_transition(&id, TaskState::Quiescing)
        .unwrap();
    capture(&mut controller, &id);
    assert_eq!(
        controller
            .agent_validation_failed(&id, "still failing")
            .unwrap(),
        RepairDecision::Exhausted
    );
    let task = controller.agent_task().unwrap();
    assert_eq!(task.state(), TaskState::Failed);
    assert_eq!(task.repair().used(), 1);
    assert!(matches!(
        controller.agent_draft_state().unwrap().unwrap(),
        DraftState::Retained { .. }
    ));
    let next = controller.begin_agent_task("again").unwrap();
    assert!(next.archived_previous.is_some());
}

#[test]
fn an_accepted_task_draft_is_replaced_without_archiving() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    run_to_quiescence(&mut controller, &context);
    capture(&mut controller, &id);
    // The public transition API cannot skip validation, and nothing changed.
    assert_eq!(
        task_error(
            controller
                .agent_task_transition(&id, TaskState::CandidateReady)
                .unwrap_err()
        ),
        TaskError::GatedTransition(TaskState::CandidateReady)
    );
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Validating
    );
    // A task that validates normally gets to CandidateReady only through a sound
    // passing report.
    controller
        .finish_agent_task(&id, TaskState::Cancelled, "restart")
        .unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    run_to_quiescence(&mut controller, &context);
    accept_candidate(&mut controller, &id);
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::CandidateReady
    );
    controller
        .agent_task_transition(&id, TaskState::Promoting)
        .unwrap();
    controller
        .finish_agent_task(&id, TaskState::Accepted, "published")
        .unwrap();
    assert!(matches!(
        controller.agent_draft_state().unwrap().unwrap(),
        DraftState::Accepted { .. }
    ));
    let next = controller.begin_agent_task("next").unwrap();
    assert!(next.archived_previous.is_none());
}

#[test]
fn a_live_writer_blocks_capture_whatever_the_caller_claims() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    // A previous task's perfectly clean evidence, kept around.
    let old = controller.begin_agent_task("old").unwrap();
    controller
        .agent_writer_started(&old.identity, qualified())
        .unwrap();
    let old_clean = evidence(&controller, &old.identity);
    controller
        .finish_agent_task(&old.identity, TaskState::Cancelled, "done")
        .unwrap();

    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    run_to_quiescence(&mut controller, &context);
    let first_writer = evidence(&controller, &id).writer;

    // A real writer process is still alive inside the task's own scope.
    let scope = controller.agent_processes(&id).unwrap();
    let writer = scope.spawn(sleeper()).unwrap();
    let blocked = |controller: &mut Controller, evidence: &QuiescenceEvidence| {
        task_error(
            controller
                .agent_complete_quiescence(&id, evidence)
                .expect_err("the gate must stay shut"),
        )
    };

    // Clean evidence that belongs to another task.
    assert_eq!(
        blocked(&mut controller, &old_clean),
        TaskError::QuiescenceBlocked(QuiescenceBlock::ForeignEvidence)
    );
    // Foreign evidence re-labelled with this task's identity still names the wrong
    // provider session.
    let mut foreign_session = evidence(&controller, &id);
    foreign_session.provider_session = Some("session-of-another-task".into());
    assert_eq!(
        blocked(&mut controller, &foreign_session),
        TaskError::QuiescenceBlocked(QuiescenceBlock::ProviderSessionMismatch)
    );
    // Evidence gathered for an earlier writer of this very task.
    controller.agent_writer_started(&id, qualified()).unwrap();
    let mut stale = evidence(&controller, &id);
    stale.writer = first_writer;
    assert!(matches!(
        blocked(&mut controller, &stale),
        TaskError::QuiescenceBlocked(QuiescenceBlock::StaleWriter { .. })
    ));
    // Correctly bound and claiming a clean end_turn, but the writer is really alive.
    let current = evidence(&controller, &id);
    assert!(matches!(
        blocked(&mut controller, &current),
        TaskError::QuiescenceBlocked(
            QuiescenceBlock::GroupSurvivors(_) | QuiescenceBlock::WriterNotTerminated
        )
    ));
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Quiescing
    );
    assert!(controller.agent_task().unwrap().candidate().is_none());
    assert!(
        scope.spawn(sleeper()).is_err(),
        "the scope is sealed against further spawns once capture is attempted"
    );

    // Reaping the writer is what opens the gate, not any assertion about it.
    let report = writer
        .lock()
        .terminate_verified(Duration::from_millis(200))
        .unwrap();
    assert!(report.verified());
    let ticket = quiesce(&mut controller, &id).unwrap();
    assert_eq!(ticket.identity(), &id);
    assert_eq!(ticket.source_base(), &context.source_base);
    assert_eq!(ticket.draft(), context.draft);
    controller.agent_capture_candidate(ticket).unwrap();
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Validating
    );
}

#[test]
fn capture_needs_a_writer_and_caller_ownership_can_only_demote() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    controller
        .agent_task_transition(&id, TaskState::Editing)
        .unwrap();
    controller
        .agent_task_transition(&id, TaskState::Quiescing)
        .unwrap();
    // No writer record exists: a hand-built generation cannot stand in for one.
    let forged = QuiescenceEvidence {
        identity: id.clone(),
        provider_session: None,
        writer: generation(1),
        completion: TurnCompletion::EndTurn,
        cancel_requested: false,
        unresolved_requests: 0,
        observed: WriterObservation {
            ownership: qualified(),
            escaped_pids: vec![],
        },
    };
    assert_eq!(
        task_error(
            controller
                .agent_complete_quiescence(&id, &forged)
                .err()
                .unwrap()
        ),
        TaskError::QuiescenceBlocked(QuiescenceBlock::NoWriter)
    );

    // An unqualified writer stays unqualified however the evidence describes it, and
    // starting a "qualified" writer later does not launder the demotion.
    controller
        .agent_writer_started(&id, WriterOwnership::Unknown)
        .unwrap();
    let claims_qualified = evidence(&controller, &id);
    assert_eq!(
        task_error(
            controller
                .agent_complete_quiescence(&id, &claims_qualified)
                .err()
                .unwrap()
        ),
        TaskError::QuiescenceBlocked(QuiescenceBlock::UnqualifiedOwnership("unknown"))
    );
    controller.agent_writer_started(&id, qualified()).unwrap();
    assert_eq!(
        controller.agent_task().unwrap().writer_ownership(),
        Some(&WriterOwnership::Unknown)
    );
    assert_eq!(
        quiesce(&mut controller, &id).unwrap_err(),
        TaskError::QuiescenceBlocked(QuiescenceBlock::UnqualifiedOwnership("unknown"))
    );
}

#[test]
fn capture_tickets_are_single_use_and_die_with_every_gate_event() {
    // Fails to compile (ambiguous impl) if the ticket ever becomes `Clone`.
    trait AmbiguousIfClone<A> {
        fn check() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<u8> for T {}
    <CaptureTicket as AmbiguousIfClone<_>>::check();

    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    run_to_quiescence(&mut controller, &context);

    // Two tickets issued back to back: only the newest works.
    let older = quiesce(&mut controller, &id).unwrap();
    let newer = quiesce(&mut controller, &id).unwrap();
    assert_eq!(
        task_error(controller.agent_capture_candidate(older).unwrap_err()),
        TaskError::StaleTicket
    );
    // A failed quiescence retires the outstanding ticket.
    let mut cancelled = evidence(&controller, &id);
    cancelled.cancel_requested = true;
    assert!(
        controller
            .agent_complete_quiescence(&id, &cancelled)
            .is_err()
    );
    assert_eq!(
        task_error(controller.agent_capture_candidate(newer).unwrap_err()),
        TaskError::StaleTicket
    );
    // So does starting a new writer.
    let before_writer = quiesce(&mut controller, &id).unwrap();
    controller.agent_writer_started(&id, qualified()).unwrap();
    assert_eq!(
        task_error(
            controller
                .agent_capture_candidate(before_writer)
                .unwrap_err()
        ),
        TaskError::StaleTicket
    );
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Quiescing
    );
    assert!(controller.agent_task().unwrap().candidate().is_none());
    // A fresh ticket still works, exactly once.
    let ticket = quiesce(&mut controller, &id).unwrap();
    controller.agent_capture_candidate(ticket).unwrap();
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Validating
    );
}

#[test]
fn repair_starts_a_fresh_scope_and_a_fresh_gate() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    run_to_quiescence(&mut controller, &context);
    let first_scope = controller.agent_processes(&id).unwrap();
    capture(&mut controller, &id);
    controller
        .agent_validation_failed(&id, "compile error")
        .unwrap();
    controller.agent_begin_repair(&id).unwrap();

    assert!(
        first_scope.spawn(sleeper()).is_err(),
        "the previous writer's scope is sealed for good"
    );
    let repair_scope = controller.agent_processes(&id).unwrap();
    let repair_writer = repair_scope.spawn(sleeper()).unwrap();
    controller.agent_writer_started(&id, qualified()).unwrap();
    controller
        .agent_task_transition(&id, TaskState::Quiescing)
        .unwrap();
    // The new writer is still running: the old evidence cannot cover it.
    assert!(matches!(
        quiesce(&mut controller, &id).unwrap_err(),
        TaskError::QuiescenceBlocked(
            QuiescenceBlock::GroupSurvivors(_) | QuiescenceBlock::WriterNotTerminated
        )
    ));
    repair_writer
        .lock()
        .terminate_verified(Duration::from_millis(200))
        .unwrap();
    capture(&mut controller, &id);
}

#[test]
fn a_stale_finish_cannot_disturb_the_successor_or_its_scope() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let first = controller.begin_agent_task("first").unwrap();
    controller
        .finish_agent_task(&first.identity, TaskState::Cancelled, "stop")
        .unwrap();
    let second = controller.begin_agent_task("second").unwrap();
    let id = second.identity.clone();
    controller.agent_writer_started(&id, qualified()).unwrap();
    controller
        .agent_task_transition(&id, TaskState::Editing)
        .unwrap();
    let scope = controller.agent_processes(&id).unwrap();
    let provider = scope.spawn(sleeper()).unwrap();

    // A late result for the first task arrives while the second one is running.
    let error = controller
        .finish_agent_task(&first.identity, TaskState::Failed, "late async result")
        .unwrap_err();
    assert_eq!(task_error(error), TaskError::StaleIdentity);
    assert!(
        controller.agent_processes(&first.identity).is_err(),
        "a stale identity is handed no scope"
    );
    // An invalid terminal request for the *current* task is refused just as early.
    for bad in [
        TaskState::Accepted,
        TaskState::Editing,
        TaskState::Promoting,
    ] {
        let error = controller.finish_agent_task(&id, bad, "bad").unwrap_err();
        assert!(
            matches!(task_error(error), TaskError::InvalidTransition { .. }),
            "{bad:?}"
        );
    }

    assert!(
        provider.lock().try_wait().unwrap().is_none(),
        "the successor's provider is still running"
    );
    assert!(!scope.is_shutdown(), "the successor's scope was not sealed");
    let extra = scope
        .spawn(sleeper())
        .expect("its scope still admits spawns");
    assert_eq!(scope.active_count(), 2);
    let task = controller.agent_task().unwrap();
    assert_eq!(task.identity(), &second.identity);
    assert_eq!(task.state(), TaskState::Editing);
    assert_eq!(
        task_error(controller.begin_agent_task("third").err().unwrap()),
        TaskError::LeaseHeld(second.identity.task.clone()),
        "the successor still holds the lease"
    );

    // The real finish for the real task reaps everything it owns.
    controller
        .finish_agent_task(&id, TaskState::Cancelled, "stop")
        .unwrap();
    assert!(provider.lock().try_wait().unwrap().is_some());
    assert!(extra.lock().try_wait().unwrap().is_some());
    assert_eq!(scope.active_count(), 0);
}

#[test]
fn the_previous_scope_cannot_spawn_between_the_emptiness_check_and_the_refresh() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let first = controller.begin_agent_task("first").unwrap();
    // A background holder keeps a clone of the first task's scope.
    let background = controller.agent_processes(&first.identity).unwrap();
    controller
        .agent_writer_started(&first.identity, qualified())
        .unwrap();
    background.spawn(sleeper()).expect("admitted while active");
    controller
        .finish_agent_task(&first.identity, TaskState::Cancelled, "stop")
        .unwrap();

    let barrier_hits = Cell::new(0);
    let second = controller
        .begin_agent_task_observed("second", |step| {
            if step == "agent_scope_checked" {
                barrier_hits.set(barrier_hits.get() + 1);
                // The old scope was observed empty; nothing may start in it now.
                assert_eq!(background.active_count(), 0);
                assert!(
                    background.spawn(sleeper()).is_err(),
                    "no spawn between the emptiness observation and the refresh"
                );
            }
        })
        .unwrap();
    assert_eq!(barrier_hits.get(), 1);
    assert_eq!(background.active_count(), 0);
    assert!(background.spawn(sleeper()).is_err());

    // Only the successor's own scope, handed out after lease and draft exist, spawns.
    let own = controller.agent_processes(&second.identity).unwrap();
    assert!(!own.is_shutdown());
    let provider = own.spawn(sleeper()).unwrap();
    assert_eq!(own.active_count(), 1);
    assert_eq!(background.active_count(), 0);
    provider
        .lock()
        .terminate_verified(Duration::from_millis(200))
        .unwrap();
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug)]
enum Ending {
    Finish,
    Close,
    SourceInvalidated,
}

#[cfg(target_os = "linux")]
struct KillOnDrop(u32);

#[cfg(target_os = "linux")]
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill")
            .arg("-KILL")
            .arg(self.0.to_string())
            .status();
    }
}

/// A zombie still answers signal 0 but cannot write; /proc tells them apart.
#[cfg(target_os = "linux")]
fn pid_alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| !stat.rsplit(") ").next().unwrap_or("").starts_with('Z'))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Starts, inside the task's own scope, a writer whose helper leaves the process group
/// with `setsid` and keeps appending to `out`. Returns the helper's pid.
#[cfg(target_os = "linux")]
fn spawn_escaping_writer(controller: &Controller, id: &TaskIdentity, out: &Path) -> u32 {
    let script = format!(
        "setsid sh -c 'while :; do echo w >> \"$1\"; sleep 0.05; done' sh {} & echo $!; wait",
        out.display()
    );
    let mut options = SpawnOptions::new("sh");
    options.arg("-c").arg(script);
    let leader = controller
        .agent_processes(id)
        .unwrap()
        .spawn(options)
        .unwrap();
    let stdout = leader
        .lock()
        .child_mut()
        .stdout
        .take()
        .expect("piped stdout");
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    let helper: u32 = line.trim().parse().expect("helper pid line");
    wait_until("the escaped helper to start writing", || out.exists());
    helper
}

#[cfg(target_os = "linux")]
fn escaped_writer_keeps_the_draft_unsafe(ending: Ending) {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    // The adapter was qualified as keeping its writers inside the process group.
    controller.agent_writer_started(&id, qualified()).unwrap();
    controller
        .agent_task_transition(&id, TaskState::Editing)
        .unwrap();
    let out = context.draft.join("agent-output.txt");
    let helper = spawn_escaping_writer(&controller, &id, &out);
    let _cleanup = KillOnDrop(helper);
    assert!(pid_alive(helper));
    let manifest = fs::read(f.root.join("studio.json")).unwrap();

    match ending {
        Ending::Finish => {
            controller
                .finish_agent_task(&id, TaskState::Failed, "adapter crashed")
                .unwrap();
        }
        Ending::Close => controller.close().unwrap(),
        Ending::SourceInvalidated => {
            fs::write(f.root.join("studio.json"), b"{").unwrap();
            assert!(controller.reconcile().is_err());
        }
    }

    // Teardown killed the tracked tree but cannot claim the escaped helper is gone.
    let task = controller.agent_task().unwrap();
    assert!(task.state().is_terminal(), "{ending:?}");
    assert_eq!(
        task.writer_ownership(),
        Some(&WriterOwnership::Detached),
        "{ending:?}: an observed escape demotes the qualified writer for good"
    );
    assert!(!task.writer_safe(), "{ending:?}");
    assert!(
        matches!(
            controller.agent_draft_state().unwrap().unwrap(),
            DraftState::UnsafeWriter { .. }
        ),
        "{ending:?}"
    );

    // The helper keeps writing into the draft after the task ended.
    let len_a = fs::metadata(&out).unwrap().len();
    wait_until("the escaped helper to keep writing", || {
        fs::metadata(&out).unwrap().len() > len_a
    });
    assert!(pid_alive(helper), "{ending:?}");

    if matches!(ending, Ending::Finish) {
        assert!(matches!(
            task_error(controller.begin_agent_task("second").err().unwrap()),
            TaskError::DraftUnsafe(_)
        ));
    }
    fs::write(f.root.join("studio.json"), manifest).unwrap();
    drop(controller);
    let mut reopened = Controller::open(&f.root, &f.paths).unwrap();
    assert!(
        matches!(
            task_error(reopened.begin_agent_task("again").err().unwrap()),
            TaskError::DraftUnsafe(_)
        ),
        "{ending:?}: refresh stays blocked"
    );
    let len_b = fs::metadata(&out).unwrap().len();
    wait_until(
        "the helper to keep writing into the untouched draft",
        || fs::metadata(&out).unwrap().len() > len_b,
    );
    assert!(context.draft.exists());
}

#[test]
#[cfg(target_os = "linux")]
fn an_escaped_writer_keeps_the_draft_unsafe_after_finish() {
    escaped_writer_keeps_the_draft_unsafe(Ending::Finish);
}

#[test]
#[cfg(target_os = "linux")]
fn an_escaped_writer_keeps_the_draft_unsafe_after_close() {
    escaped_writer_keeps_the_draft_unsafe(Ending::Close);
}

#[test]
#[cfg(target_os = "linux")]
fn an_escaped_writer_keeps_the_draft_unsafe_after_source_invalidation() {
    escaped_writer_keeps_the_draft_unsafe(Ending::SourceInvalidated);
}

#[test]
#[cfg(target_os = "linux")]
fn an_escape_seen_before_quiescence_blocks_capture_and_stays_demoted() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    run_to_quiescence(&mut controller, &context);
    let out = context.draft.join("agent-output.txt");
    let helper = spawn_escaping_writer(&controller, &id, &out);
    let _cleanup = KillOnDrop(helper);
    let error = quiesce(&mut controller, &id).unwrap_err();
    assert!(
        matches!(
            error,
            TaskError::QuiescenceBlocked(
                QuiescenceBlock::GroupSurvivors(_) | QuiescenceBlock::UnqualifiedOwnership(_)
            )
        ),
        "{error:?}"
    );
    assert_eq!(
        controller.agent_task().unwrap().writer_ownership(),
        Some(&WriterOwnership::Detached)
    );
}

#[test]
fn driver_reported_escapes_demote_even_when_the_scope_looks_clean() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    run_to_quiescence(&mut controller, &context);
    // The driver's DriverOutcome: it saw a helper leave the group, then shut down.
    controller
        .agent_observe_writer(
            &id,
            &WriterObservation {
                ownership: WriterOwnership::Detached,
                escaped_pids: vec![31337],
            },
        )
        .unwrap();
    assert_eq!(
        quiesce(&mut controller, &id).unwrap_err(),
        TaskError::QuiescenceBlocked(QuiescenceBlock::UnqualifiedOwnership("detached"))
    );
    controller
        .finish_agent_task(&id, TaskState::Failed, "escaped")
        .unwrap();
    assert!(matches!(
        controller.agent_draft_state().unwrap().unwrap(),
        DraftState::UnsafeWriter { .. }
    ));
}

#[test]
fn processes_in_the_scope_without_a_registered_writer_keep_the_draft_unsafe() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let provider = controller
        .agent_processes(&context.identity)
        .unwrap()
        .spawn(sleeper())
        .unwrap();
    controller
        .finish_agent_task(&context.identity, TaskState::Cancelled, "stop")
        .unwrap();
    assert!(
        provider.lock().try_wait().unwrap().is_some(),
        "the scope is reaped"
    );
    assert!(matches!(
        controller.agent_draft_state().unwrap().unwrap(),
        DraftState::UnsafeWriter { .. }
    ));
}

#[test]
fn protocol_completion_alone_never_opens_the_gate() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    run_to_quiescence(&mut controller, &context);
    for (mutate, expected) in [
        (
            Box::new(|e: &mut QuiescenceEvidence| e.completion = TurnCompletion::Refusal)
                as Box<dyn Fn(&mut QuiescenceEvidence)>,
            QuiescenceBlock::NotEndTurn(TurnCompletion::Refusal),
        ),
        (
            Box::new(|e| e.completion = TurnCompletion::None),
            QuiescenceBlock::NoAuthoritativeCompletion,
        ),
        (
            Box::new(|e| e.cancel_requested = true),
            QuiescenceBlock::CancelRequested,
        ),
        (
            Box::new(|e| e.unresolved_requests = 1),
            QuiescenceBlock::UnresolvedRequests(1),
        ),
        (
            Box::new(|e| e.observed.ownership = WriterOwnership::Detached),
            QuiescenceBlock::UnqualifiedOwnership("detached"),
        ),
    ] {
        let mut e = evidence(&controller, &id);
        mutate(&mut e);
        assert_eq!(
            task_error(controller.agent_complete_quiescence(&id, &e).err().unwrap()),
            TaskError::QuiescenceBlocked(expected)
        );
    }
}

#[test]
fn invalid_source_retains_draft_and_checkpoint_and_edit_back_never_resurrects_the_task() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let checkpoint = controller.state().accepted().clone();
    let context = controller.begin_agent_task("go").unwrap();
    let id = context.identity.clone();
    controller.agent_writer_started(&id, qualified()).unwrap();
    controller
        .agent_task_transition(&id, TaskState::Editing)
        .unwrap();
    fs::write(context.draft.join("src/lib.rs"), "// agent progress").unwrap();

    let manifest = fs::read(f.root.join("studio.json")).unwrap();
    fs::write(f.root.join("studio.json"), b"{").unwrap();
    assert!(controller.reconcile().is_err());

    let task = controller.agent_task().unwrap();
    assert_eq!(task.state(), TaskState::Interrupted);
    assert!(task.source_invalidated());
    assert_eq!(
        controller.state().accepted(),
        &checkpoint,
        "checkpoint retained"
    );
    assert_eq!(
        fs::read_to_string(context.draft.join("src/lib.rs")).unwrap(),
        "// agent progress",
        "draft retained"
    );
    assert!(matches!(
        controller.agent_draft_state().unwrap().unwrap(),
        DraftState::Retained { .. }
    ));

    // Edit back to the exact base bytes: the interrupted task stays dead.
    fs::write(f.root.join("studio.json"), manifest).unwrap();
    controller.reconcile().unwrap();
    assert!(matches!(
        task_error(
            controller
                .agent_task_transition(&id, TaskState::Quiescing)
                .unwrap_err()
        ),
        TaskError::NoActiveTask
    ));
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Interrupted
    );
    let next = controller.begin_agent_task("retry").unwrap();
    assert_ne!(next.identity.task, id.task);
    assert!(next.archived_previous.is_some());
}

#[test]
fn closing_or_dropping_interrupts_the_active_task_and_reaps_its_scope() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = controller.begin_agent_task("go").unwrap();
    let provider = controller
        .agent_processes(&context.identity)
        .unwrap()
        .spawn(sleeper())
        .unwrap();
    controller
        .agent_writer_started(&context.identity, WriterOwnership::Unknown)
        .unwrap();
    controller.close().unwrap();
    assert!(
        provider.lock().try_wait().unwrap().is_some(),
        "provider reaped"
    );
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Interrupted
    );
    assert!(matches!(
        controller.agent_draft_state().unwrap().unwrap(),
        DraftState::UnsafeWriter { .. }
    ));
    drop(controller);

    let mut reopened = Controller::open(&f.root, &f.paths).unwrap();
    assert!(matches!(
        task_error(reopened.begin_agent_task("again").err().unwrap()),
        TaskError::DraftUnsafe(_)
    ));
}

#[test]
fn empty_and_oversized_briefs_are_rejected_before_any_state_changes() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    assert!(matches!(
        task_error(controller.begin_agent_task("  \n").err().unwrap()),
        TaskError::InvalidBrief(_)
    ));
    assert!(matches!(
        task_error(
            controller
                .begin_agent_task(&"x".repeat(70 * 1024))
                .err()
                .unwrap()
        ),
        TaskError::InvalidBrief(_)
    ));
    assert!(controller.agent_task().is_none());
    assert!(
        !f.paths
            .agent_draft(&controller.state().project().clone())
            .exists()
    );
}
