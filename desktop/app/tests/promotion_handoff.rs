//! Guarded handoff of a validated, published candidate to playback: adoption of the
//! staged preview under the engine's promotion authorization, re-priming at the latest
//! seek, matching video/audio identity, and a failed handoff keeping the old preview.
//! These publication and promotion integration cases run only on Linux, where Apply is
//! enabled after the no-clobber source publication primitives are qualified.
#![cfg(target_os = "linux")]
use fframes_studio::{
    build_service::{BuildLimits, BuildService},
    candidate_runner::{CandidateRunConfig, RunScopes, run_candidate_validation},
    preview_coordinator::{AdoptError, BuildSpec, PreviewCoordinator, SeekIntent},
    studio_shell::{PresentationVerdict, install_fence, presentation_verdict},
    teardown::Teardown,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use studio_bootstrap::{ProcessTreeManager, SpawnOptions, WriterOwnership};
use studio_engine::{
    AgentTaskContext, CaptureTicket, CompletionOutcome, Controller, JobKind, JobResult,
    PreviewState, QuiescenceEvidence, TaskState, TurnCompletion, WriterObservation,
    app_paths::AppPaths,
    candidate_validation::{CapturedCandidate, capture_candidate},
};
use studio_project::checkpoint::Checkpoints;

#[path = "support/build_fixture.rs"]
mod fixture;
use fixture::*;

struct World {
    _temp: tempfile::TempDir,
    sdk: PathBuf,
    paths: AppPaths,
    controller: Controller,
    service: BuildService,
}

fn world() -> World {
    let temp = tempfile::tempdir().unwrap();
    let sdk = fake_sdk(temp.path());
    let root = temp.path().join("video");
    create_project(&root);
    let paths = AppPaths::new(temp.path().join("data")).unwrap();
    let controller = Controller::open(&root, &paths).unwrap();
    let compiler = FakeCompiler::new(true);
    let service = BuildService::new(
        ProcessTreeManager::new(),
        compiler.clone(),
        BuildLimits::default(),
    );
    World {
        _temp: temp,
        sdk,
        paths,
        controller,
        service,
    }
}

fn qualified() -> WriterOwnership {
    WriterOwnership::ProcessGroupContained {
        qualification: "test-adapter".into(),
    }
}

/// Clean end-turn evidence bound to the task's current writer generation; termination
/// is derived by the controller from the task's own process scope.
fn evidence(controller: &Controller, context: &AgentTaskContext) -> QuiescenceEvidence {
    QuiescenceEvidence {
        identity: context.identity.clone(),
        provider_session: None,
        writer: controller
            .agent_task()
            .unwrap()
            .writer_generation()
            .expect("a writer was started"),
        completion: TurnCompletion::EndTurn,
        cancel_requested: false,
        unresolved_requests: 0,
        observed: WriterObservation {
            ownership: qualified(),
            escaped_pids: vec![],
        },
    }
}

impl World {
    fn start(&mut self) -> AgentTaskContext {
        let context = self.controller.begin_agent_task("edit").unwrap();
        self.reopen(&context);
        context
    }
    fn reopen(&mut self, context: &AgentTaskContext) {
        let id = &context.identity;
        self.controller
            .agent_writer_started(id, qualified())
            .unwrap();
        // A repair attempt already put the task back into `Editing`.
        if self.controller.agent_task().unwrap().state() != TaskState::Editing {
            self.controller
                .agent_task_transition(id, TaskState::Editing)
                .unwrap();
        }
        self.controller
            .agent_task_transition(id, TaskState::Quiescing)
            .unwrap();
    }
    fn checkpoints(&self) -> Checkpoints {
        Checkpoints::new(
            &self
                .paths
                .project(&self.controller.project.manifest.project_id),
        )
        .unwrap()
    }
    fn ticket(&mut self, context: &AgentTaskContext) -> CaptureTicket {
        let evidence = evidence(&self.controller, context);
        self.controller
            .agent_complete_quiescence(&context.identity, &evidence)
            .unwrap()
    }
    fn capture(&mut self, context: &AgentTaskContext) -> CapturedCandidate {
        let ticket = self.ticket(context);
        let captured = capture_candidate(&ticket, &self.checkpoints()).unwrap();
        self.controller
            .agent_record_candidate(ticket, &captured)
            .unwrap();
        captured
    }
    fn config(&self) -> CandidateRunConfig {
        CandidateRunConfig {
            service: self.service.clone(),
            sdk: self.sdk.clone(),
            compatibility: manifest(),
            builds: self.paths.builds(),
        }
    }
    fn scopes(&self) -> RunScopes {
        RunScopes {
            compiler: self.controller.processes.sub_manager(),
            worker: self.controller.processes.sub_manager(),
        }
    }
}

fn worker_config(draft: &Path, value: Value) {
    fs::write(draft.join(FAKE_CONFIG), serde_json::to_vec(&value).unwrap()).unwrap();
}

/// Display the current source through the UI coordinator so "old playback" exists.
fn display_base(w: &mut World) -> (PreviewCoordinator, PreviewState) {
    let coordinator = PreviewCoordinator::new(w.controller.processes.sub_manager());
    let mut state = PreviewState::default();
    let tag = w.controller.begin_job(JobKind::Build).unwrap();
    state.begin(tag.clone());
    coordinator.build(BuildSpec {
        project: w.controller.project.clone(),
        sdk: w.sdk.clone(),
        compatibility: manifest(),
        builds: w.paths.builds(),
        tag,
        compiler: w.controller.operation_processes(),
        worker: w.controller.processes.sub_manager(),
        service: w.service.clone(),
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let ready = loop {
        let e = coordinator.events();
        if let Some(tag) = e.compiled {
            w.controller
                .complete(&tag, JobResult::Built(tag.base_source.clone()))
                .unwrap();
        }
        if let Some((_, error)) = e.error {
            panic!("base preview failed: {error}");
        }
        if let Some(r) = e.ready {
            w.controller.reconcile().unwrap();
            break r;
        }
        assert!(Instant::now() < deadline, "base preview deadline");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(coordinator.commit(ready.identity().clone(), ready.seek_serial));
    state.install(&ready, w.controller.state()).unwrap();
    (coordinator, state)
}

fn seek_and_wait(p: &PreviewCoordinator, state: &mut PreviewState, position: usize) {
    let serial = state.seek(position, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: state.displayed().unwrap().clone(),
        serial,
        position,
        scale: state.scale(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(f) = p.events().frame
            && state.accepts_frame(&f)
        {
            assert_eq!(f.response.frame_index, position);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the displayed worker stopped answering"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn promote(
    w: &mut World,
    marker: &str,
) -> (
    CapturedCandidate,
    fframes_studio::candidate_runner::CandidateRun,
    studio_engine::Promotion,
    RunScopes,
) {
    let context = w.start();
    worker_config(
        &context.draft,
        json!({"frames": 120, "tracks": [[0.0, 2.0]], "audio": "tone", "pixel": 40}),
    );
    fs::write(context.draft.join("src/lib.rs"), format!("// {marker}\n")).unwrap();
    let captured = w.capture(&context);
    let scopes = w.scopes();
    let run = run_candidate_validation(&captured, &w.checkpoints(), &w.config(), &scopes, 10, 0);
    assert!(run.report.passed(), "{:?}", run.report.failure());
    let CompletionOutcome::Applied(promotion) = w
        .controller
        .complete_validated_task(&captured, &run.report)
        .unwrap()
    else {
        panic!("automatic policy applies");
    };
    assert_eq!(
        w.controller.agent_task().unwrap().state(),
        TaskState::Accepted
    );
    (captured, run, *promotion, scopes)
}

fn wait_ready(
    p: &PreviewCoordinator,
) -> Result<Arc<studio_engine::ReadyPreview>, (studio_engine::OperationTag, String)> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let e = p.events();
        if let Some(error) = e.error {
            return Err(error);
        }
        if let Some(ready) = e.ready {
            return Ok(ready);
        }
        assert!(Instant::now() < deadline, "no ready preview");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn the_published_candidate_is_reprimed_at_the_latest_seek_and_installs_with_matching_audio() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let old = state.displayed().unwrap().clone();
    seek_and_wait(&coordinator, &mut state, 25);
    let (captured, mut run, promotion, _scopes) = promote(&mut w, "new scene");
    let authorization = promotion.authorization.clone().unwrap();
    assert_eq!(
        &authorization.tag().base_source,
        &promotion.record.published
    );
    assert_eq!(promotion.record.published, *captured.candidate().revision());
    let staged = run.staged.take().unwrap();
    // The staged candidate was prepared at playhead 10; the shell is at 25 and moves on.
    assert_eq!(staged.ready().position, 10);
    let latest = SeekIntent {
        identity: old.clone(),
        serial: state.serial(),
        position: state.position(),
        scale: state.scale(),
    };
    coordinator.adopt(staged, &authorization, latest).unwrap();
    state.begin_promotion(&authorization);
    // A newer seek arrives while the candidate is being adopted: latest wins.
    let serial = state.seek(40, state.scale()).unwrap();
    coordinator.seek(SeekIntent {
        identity: old.clone(),
        serial,
        position: 40,
        scale: state.scale(),
    });
    let ready = loop {
        let ready = wait_ready(&coordinator).unwrap();
        if ready.seek_serial == serial {
            break ready;
        }
    };
    assert_eq!(ready.position, 40);
    assert_eq!(ready.tag(), authorization.tag());
    let published = promotion.record.published.as_str();
    assert_eq!(ready.identity().source_revision, published);
    assert_ne!(
        ready.identity().source_revision,
        captured.source_base().revision().as_str()
    );
    // Video, inspection and audio all carry the published identity and the latest position.
    assert_eq!(ready.timeline.envelope.identity, *ready.identity());
    assert_eq!(ready.audio.envelope.identity, *ready.identity());
    assert_eq!(ready.frame.as_ref().unwrap().response.frame_index, 40);
    assert_eq!(
        ready.pcm_start_sample,
        40 * ready.audio.sample_rate as u64 / ready.timeline.fps as u64
    );
    assert!(ready.audio_source.is_some());
    // Source is current, so the authorization still holds and the old preview was
    // never retagged: it is still the displayed identity until the commit.
    w.controller.reconcile().unwrap();
    assert_eq!(state.displayed(), Some(&old));
    assert!(state.can_install(&ready, w.controller.state()).is_ok());
    assert!(coordinator.commit(ready.identity().clone(), ready.seek_serial));
    state.install(&ready, w.controller.state()).unwrap();
    assert_eq!(state.displayed(), Some(ready.identity()));
    seek_and_wait(&coordinator, &mut state, 7);
    coordinator.close();
}

#[test]
fn a_failed_handoff_keeps_the_old_preview_playing_and_the_committed_source() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let old = state.displayed().unwrap().clone();
    let (_captured, mut run, promotion, scopes) = promote(&mut w, "failing handoff");
    let authorization = promotion.authorization.clone().unwrap();
    let staged = run.staged.take().unwrap();
    // The staged worker process dies (the scope itself is not sealed) before it can be
    // re-primed.
    scopes.worker.terminate_all(Duration::ZERO);
    let serial = state.seek(33, state.scale()).unwrap();
    let latest = SeekIntent {
        identity: old.clone(),
        serial,
        position: 33,
        scale: state.scale(),
    };
    coordinator.seek(latest.clone());
    coordinator.adopt(staged, &authorization, latest).unwrap();
    state.begin_promotion(&authorization);
    let (tag, message) = wait_ready(&coordinator).unwrap_err();
    assert_eq!(&tag, authorization.tag());
    assert!(!message.is_empty());
    state.fail(&tag, message);
    // Nothing was retagged: the old preview is displayed and still answers seeks.
    assert_eq!(state.displayed(), Some(&old));
    seek_and_wait(&coordinator, &mut state, 5);
    // The committed source is not undone implicitly and still awaits a preview.
    assert_eq!(w.controller.state().task_history().entries().len(), 1);
    assert_eq!(w.controller.state().source(), &promotion.record.published);
    assert!(authorization.is_current(w.controller.state()));
    coordinator.close();
}

/// Intent exactly equal to what a staged preview was prepared at.
fn staged_intent(
    staged: &fframes_studio::preview_coordinator::StagedPreview,
    identity: &fframes_studio_protocol::PreviewIdentity,
) -> SeekIntent {
    let ready = staged.ready();
    SeekIntent {
        identity: identity.clone(),
        serial: ready.seek_serial,
        position: ready.position,
        scale: ready.frame.as_ref().unwrap().response.scale,
    }
}

#[test]
fn a_dead_staged_worker_is_never_adopted_even_at_exactly_the_staged_serial_position_and_scale() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let old = state.displayed().unwrap().clone();
    let (_captured, mut run, promotion, scopes) = promote(&mut w, "dead worker, same intent");
    let authorization = promotion.authorization.clone().unwrap();
    let staged = run.staged.take().unwrap();
    // The intent is numerically the one the candidate was validated at: its cached frame
    // and PCM already "match", so only a live worker round trip can tell it is dead.
    let latest = staged_intent(&staged, &old);
    assert_eq!(staged.ready().position, 10);
    assert_eq!(latest.serial, state.serial());
    scopes.worker.terminate_all(Duration::ZERO);
    coordinator.adopt(staged, &authorization, latest).unwrap();
    state.begin_promotion(&authorization);
    // Readiness is never published from the cache: the adoption fails closed.
    let (tag, message) = wait_ready(&coordinator).unwrap_err();
    assert_eq!(&tag, authorization.tag());
    assert!(!message.is_empty());
    state.fail(&tag, message);
    assert!(!coordinator.commit(
        studio_engine::preview_identity(authorization.tag()),
        state.serial()
    ));
    // The old preview is still displayed and seekable; the committed source stays.
    assert_eq!(state.displayed(), Some(&old));
    seek_and_wait(&coordinator, &mut state, 5);
    assert_eq!(w.controller.state().source(), &promotion.record.published);
    assert!(authorization.is_current(w.controller.state()));
    coordinator.close();
}

#[test]
fn a_staged_preview_whose_worker_scope_is_already_shut_down_is_rejected_at_adoption() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let old = state.displayed().unwrap().clone();
    let (_captured, mut run, promotion, scopes) = promote(&mut w, "shut down scope");
    let authorization = promotion.authorization.clone().unwrap();
    let staged = run.staged.take().unwrap();
    let latest = staged_intent(&staged, &old);
    scopes.worker.shutdown(Duration::ZERO);
    assert_eq!(
        coordinator
            .adopt(staged, &authorization, latest)
            .unwrap_err(),
        AdoptError::WorkerGone
    );
    assert_eq!(state.displayed(), Some(&old));
    seek_and_wait(&coordinator, &mut state, 6);
    coordinator.close();
}

#[test]
fn a_staged_preview_carrying_another_promotions_tag_is_refused_at_adoption() {
    let mut w = world();
    let coordinator = PreviewCoordinator::new(w.controller.processes.sub_manager());
    let (_c1, mut run1, first, _s1) = promote(&mut w, "first revision");
    let (_c2, _run2, second, _s2) = promote(&mut w, "second revision");
    let second_auth = second.authorization.clone().unwrap();
    assert_ne!(first.authorization.unwrap().tag(), second_auth.tag());
    let staged = run1.staged.take().unwrap();
    let intent = staged_intent(&staged, staged.ready().identity());
    assert_eq!(
        coordinator.adopt(staged, &second_auth, intent).unwrap_err(),
        AdoptError::WrongTag
    );
    coordinator.close();
}

#[test]
fn a_closed_coordinator_never_adopts() {
    let mut w = world();
    let coordinator = PreviewCoordinator::new(w.controller.processes.sub_manager());
    let (_c, mut run, promotion, _s) = promote(&mut w, "closed coordinator");
    let authorization = promotion.authorization.clone().unwrap();
    let staged = run.staged.take().unwrap();
    let intent = staged_intent(&staged, staged.ready().identity());
    coordinator.close();
    assert_eq!(
        coordinator
            .adopt(staged, &authorization, intent)
            .unwrap_err(),
        AdoptError::Closed
    );
}

#[test]
fn an_expired_but_matching_authorization_adopts_but_the_state_install_fence_refuses_it() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let old = state.displayed().unwrap().clone();
    let (_c1, mut run1, first, _s1) = promote(&mut w, "first revision");
    let (_c2, _run2, second, _s2) = promote(&mut w, "second revision");
    let (first_auth, second_auth) = (
        first.authorization.clone().unwrap(),
        second.authorization.clone().unwrap(),
    );
    // The second publication revoked the first authorization, yet the first staged
    // preview still carries exactly the first authorization's tag.
    assert!(!first_auth.is_current(w.controller.state()));
    assert!(second_auth.is_current(w.controller.state()));
    let staged = run1.staged.take().unwrap();
    assert_eq!(staged.ready().tag(), first_auth.tag());
    let intent = staged_intent(&staged, &old);
    // The coordinator cannot see source state: tag equality is all it checks.
    coordinator.adopt(staged, &first_auth, intent).unwrap();
    state.begin_promotion(&first_auth);
    let ready = wait_ready(&coordinator).unwrap();
    assert_eq!(ready.tag(), first_auth.tag());
    // The final, state-level install fence is what refuses the expired authorization.
    assert!(state.can_install(&ready, w.controller.state()).is_err());
    assert!(state.install(&ready, w.controller.state()).is_err());
    assert_eq!(state.displayed(), Some(&old));
    coordinator.cancel_build();
    seek_and_wait(&coordinator, &mut state, 4);
    coordinator.close();
}

#[test]
fn an_edit_after_publication_revokes_the_install_but_not_the_old_playback() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let old = state.displayed().unwrap().clone();
    let (_captured, mut run, promotion, _scopes) = promote(&mut w, "late edit");
    let authorization = promotion.authorization.clone().unwrap();
    let latest = SeekIntent {
        identity: old.clone(),
        serial: state.serial(),
        position: state.position(),
        scale: state.scale(),
    };
    coordinator
        .adopt(run.staged.take().unwrap(), &authorization, latest)
        .unwrap();
    state.begin_promotion(&authorization);
    let ready = wait_ready(&coordinator).unwrap();
    // The user edits the source before the install commits.
    let root = w.controller.project.root.clone();
    fs::write(root.join("src/lib.rs"), "// typed by the user\n").unwrap();
    w.controller.reconcile().unwrap();
    assert!(state.can_install(&ready, w.controller.state()).is_err());
    coordinator.cancel_build();
    assert_eq!(state.displayed(), Some(&old));
    seek_and_wait(&coordinator, &mut state, 3);
    coordinator.close();
}

#[test]
fn an_undo_is_validated_through_the_candidate_pipeline_and_hands_off_like_an_apply() {
    use studio_engine::edit_transaction::TransactionKind;
    let mut w = world();
    let (captured, applied_run, applied, _scopes) = promote(&mut w, "to be undone");
    drop(applied_run);
    let predecessor = captured.source_base().revision().clone();
    assert_eq!(w.controller.state().source(), &applied.record.published);
    // Undo forms its candidate from the current full source and validates it through
    // the same build + probe + staging pipeline before anything is published.
    let undo = w.controller.prepare_undo(None).unwrap();
    assert_eq!(undo.target(), applied.record.id);
    let mut run = run_candidate_validation(
        undo.captured(),
        &w.checkpoints(),
        &w.config(),
        &w.scopes(),
        10,
        0,
    );
    assert!(run.report.passed(), "{:?}", run.report.failure());
    // Nothing is published until the validated report reaches `undo_task`.
    assert_eq!(w.controller.state().source(), &applied.record.published);
    let promotion = w.controller.undo_task(&undo, &run.report).unwrap();
    assert_eq!(promotion.record.kind, TransactionKind::Undo);
    assert_eq!(
        promotion.record.undoes.as_deref(),
        Some(applied.record.id.as_str())
    );
    // No unrelated edits: the resulting inventory is the historical predecessor.
    assert_eq!(promotion.record.published, predecessor);
    assert_eq!(w.controller.state().task_history().entries().len(), 2);
    // The Undo's staged preview adopts under its own authorization and the preview
    // installs at the restored revision, not at the Apply's.
    let authorization = promotion.authorization.clone().unwrap();
    assert_eq!(&authorization.tag().base_source, &predecessor);
    let coordinator = PreviewCoordinator::new(w.controller.processes.sub_manager());
    let staged = run.staged.take().unwrap();
    let identity = staged.ready().identity().clone();
    coordinator
        .adopt(
            staged,
            &authorization,
            SeekIntent {
                identity,
                serial: 0,
                position: 5,
                scale: 1.,
            },
        )
        .unwrap();
    let ready = wait_ready(&coordinator).unwrap();
    assert_eq!(ready.tag(), authorization.tag());
    assert_eq!(ready.identity().source_revision, predecessor.as_str());
    assert_eq!(
        ready.identity().source_revision,
        authorization.published().as_str()
    );
    assert!(ready.audio_source.is_some());
    // An Apply's authorization can never install the Undo's preview (or vice versa).
    assert_ne!(
        applied.authorization.as_ref().unwrap().tag(),
        authorization.tag()
    );
    coordinator.close();
}

/// A process in `scope` whose lock the caller holds: any teardown of the scope blocks on it
/// until the guard is dropped (a held process lock / slow termination).
fn held_child(
    scope: &ProcessTreeManager,
) -> std::sync::Arc<parking_lot::Mutex<studio_bootstrap::TrackedChild>> {
    let mut options = SpawnOptions::new("sleep");
    options.arg("60");
    scope.spawn(options).unwrap()
}

#[test]
fn rejecting_a_staged_preview_hands_its_teardown_off_instead_of_waiting_for_it() {
    let mut w = world();
    let coordinator = PreviewCoordinator::new(w.controller.processes.sub_manager());
    let (_c1, mut run1, _first, scopes1) = promote(&mut w, "first revision");
    let (_c2, _run2, second, _scopes2) = promote(&mut w, "second revision");
    let second_auth = second.authorization.clone().unwrap();
    let staged = run1.staged.take().unwrap();
    let intent = staged_intent(&staged, staged.ready().identity());
    let child = held_child(&scopes1.worker);
    let guard = child.lock();
    let started = Instant::now();
    // The wrong-tag refusal drops the staged preview on this thread. Reaping its worker
    // scope needs the child lock this test holds, so a synchronous drop would hang here.
    assert_eq!(
        coordinator.adopt(staged, &second_auth, intent).unwrap_err(),
        AdoptError::WrongTag
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the UI-facing refusal waited for the teardown: {:?}",
        started.elapsed()
    );
    assert!(
        Teardown::global().pending() >= 1,
        "the teardown was not handed off"
    );
    drop(guard);
    assert!(Teardown::global().wait_idle(Duration::from_secs(30)));
    assert!(scopes1.worker.is_shutdown());
    assert!(
        !child.lock().is_alive(),
        "the owner reaped the scope's processes"
    );
    coordinator.close();
}

#[test]
fn cancelling_an_adopted_handoff_fences_it_at_once_and_reaps_in_the_background() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let old = state.displayed().unwrap().clone();
    let (_captured, mut run, promotion, scopes) = promote(&mut w, "cancelled handoff");
    let authorization = promotion.authorization.clone().unwrap();
    let staged = run.staged.take().unwrap();
    let intent = staged_intent(&staged, &old);
    let child = held_child(&scopes.worker);
    let guard = child.lock();
    coordinator.adopt(staged, &authorization, intent).unwrap();
    state.begin_promotion(&authorization);
    let started = Instant::now();
    coordinator.cancel_build();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cancellation waited for the process teardown: {:?}",
        started.elapsed()
    );
    // Fenced immediately, even though the processes cannot be reaped yet.
    assert!(!coordinator.commit(old.clone(), state.serial()));
    assert!(coordinator.events().ready.is_none());
    drop(guard);
    assert!(Teardown::global().wait_idle(Duration::from_secs(30)));
    assert!(
        coordinator.events().ready.is_none(),
        "a cancelled handoff never becomes ready"
    );
    state.cancel_build();
    // The displayed preview keeps answering seeks.
    seek_and_wait(&coordinator, &mut state, 9);
    coordinator.close();
}

/// An adopted, re-primed, ready promotion with its state and an `Arc`-shared controller.
fn adopted_ready(
    w: &mut World,
    coordinator: &PreviewCoordinator,
    state: &mut PreviewState,
    marker: &str,
) -> Arc<studio_engine::ReadyPreview> {
    let old = state.displayed().unwrap().clone();
    let (_captured, mut run, promotion, _scopes) = promote(w, marker);
    let authorization = promotion.authorization.clone().unwrap();
    let latest = SeekIntent {
        identity: old,
        serial: state.serial(),
        position: state.position(),
        scale: state.scale(),
    };
    coordinator
        .adopt(run.staged.take().unwrap(), &authorization, latest)
        .unwrap();
    state.begin_promotion(&authorization);
    wait_ready(coordinator).unwrap()
}

/// Moves the world's controller where the shell holds it: shared with the workflow.
fn share(w: World) -> (Arc<parking_lot::Mutex<Controller>>, impl Drop) {
    let World {
        _temp, controller, ..
    } = w;
    (Arc::new(parking_lot::Mutex::new(controller)), _temp)
}

#[test]
fn the_install_fence_excludes_every_controller_mutation_until_the_install_completes() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let ready = adopted_ready(&mut w, &coordinator, &mut state, "fenced install");
    let (controller, _keep) = share(w);
    // A fence is only offered while the controller is free...
    let fence = install_fence(&controller).expect("a free controller offers the fence");
    assert!(
        install_fence(&controller).is_none(),
        "a second fence cannot exist"
    );
    // ...and while it is held no other thread (a workflow publication, a reconcile) can
    // mutate the controller, so the validation, the coordinator commit and the install
    // all see the same source.
    std::thread::scope(|scope| {
        let blocked = scope
            .spawn(|| {
                controller
                    .try_lock_for(Duration::from_millis(150))
                    .is_none()
            })
            .join()
            .unwrap();
        assert!(
            blocked,
            "the workflow could mutate the controller under the fence"
        );
    });
    assert!(state.can_install(&ready, fence.state()).is_ok());
    assert!(coordinator.commit(ready.identity().clone(), ready.seek_serial));
    state.install(&ready, fence.state()).unwrap();
    assert_eq!(state.displayed(), Some(ready.identity()));
    drop(fence);
    // Released: the controller is available to the workflow again.
    assert!(controller.try_lock_for(Duration::from_secs(2)).is_some());
    coordinator.close();
}

#[test]
fn a_source_that_moved_before_the_fence_never_installs_the_prepared_preview() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let old = state.displayed().unwrap().clone();
    let ready = adopted_ready(&mut w, &coordinator, &mut state, "moved before the fence");
    let (controller, _keep) = share(w);
    // The source advances after preparation finished and before the commit.
    let root = controller.lock().project.root.clone();
    fs::write(root.join("src/lib.rs"), "// moved on\n").unwrap();
    // The watcher's hint may arrive after the first scan: reconcile until it is quiet.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut c = controller.lock();
        c.reconcile().unwrap();
        if !c.changed_hint() {
            break;
        }
        drop(c);
        assert!(Instant::now() < deadline, "the change hint never settled");
        std::thread::sleep(Duration::from_millis(50));
    }
    let fence = install_fence(&controller).expect("free controller");
    // Validated against the guard's current state, not a copy taken earlier: refused.
    assert!(state.can_install(&ready, fence.state()).is_err());
    assert!(state.install(&ready, fence.state()).is_err());
    assert_eq!(state.displayed(), Some(&old));
    drop(fence);
    coordinator.cancel_build();
    coordinator.close();
}

#[test]
fn a_refresh_never_cancels_the_preparation_adopted_for_the_matching_promotion() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let known = w.controller.state().generation();
    let ready = adopted_ready(&mut w, &coordinator, &mut state, "refresh races handoff");
    let current = w.controller.state().clone();
    assert!(current.generation() > known);
    // The refresh's presentation carries the matching, current promotion: keep.
    assert_eq!(
        presentation_verdict(&state, Some(known), Some(&current)),
        PresentationVerdict::Replace
    );
    // The shell already knows this generation: nothing to do either way.
    assert_eq!(
        presentation_verdict(&state, Some(current.generation()), Some(&current)),
        PresentationVerdict::Replace
    );
    // A presentation captured before the publication (older than what the shell knows)
    // replaces nothing and cancels nothing.
    assert_eq!(
        presentation_verdict(&state, Some(current.generation() + 1), Some(&current)),
        PresentationVerdict::Older
    );
    // Preparation that is obsolete under the returned state is cancelled: another
    // publication revoked the adopted authorization...
    let (_c2, _run2, _second, _s2) = promote(&mut w, "a later publication");
    let later = w.controller.state().clone();
    assert_eq!(
        presentation_verdict(&state, Some(known), Some(&later)),
        PresentationVerdict::CancelPreparation
    );
    // ...or nothing matching is being prepared at all.
    state.cancel_build();
    assert_eq!(
        presentation_verdict(&state, Some(known), Some(&current)),
        PresentationVerdict::CancelPreparation
    );
    let _ = ready;
    coordinator.close();
}
