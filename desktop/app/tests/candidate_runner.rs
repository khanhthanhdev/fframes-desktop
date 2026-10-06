//! Candidate validation and the six project tools against a real preview-worker process
//! (a deterministic python worker installed by an injected compiler) and the real shared
//! build service. No SDK is needed; real-SDK checks live in `real_sdk_candidates.rs`.
use fframes_studio::{
    agent_tools::{
        BoundRevision, ToolBinding, ToolDispatcher, ToolErrorCode,
        backend::{ArtifactStore, ProjectToolBackend, ToolBackendConfig, WriterGate},
    },
    build_service::{BuildKey, BuildLimits, BuildService},
    candidate_runner::{CandidateRunConfig, RunScopes, run_candidate_validation},
    preview_coordinator::{BuildSpec, PreviewCoordinator, SeekIntent},
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use studio_bootstrap::{ProcessTreeManager, WriterOwnership};
use studio_engine::{
    AgentTaskContext, CaptureTicket, Controller, JobKind, JobResult, PreviewState,
    QuiescenceEvidence, RepairDecision, TaskState, TurnCompletion, WriterObservation,
    app_paths::AppPaths,
    candidate_validation::{
        CandidateError, CapturePhase, CapturedCandidate, FailureKind, NextStep, ValidationStage,
        capture_candidate, capture_candidate_observed, next_step,
    },
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
    compiler: Arc<FakeCompiler>,
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
        compiler,
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
    fn run(
        &self,
        captured: &CapturedCandidate,
        repairs: u32,
    ) -> fframes_studio::candidate_runner::CandidateRun {
        run_candidate_validation(
            captured,
            &self.checkpoints(),
            &self.config(),
            &self.scopes(),
            10,
            repairs,
        )
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

#[test]
fn passing_candidate_is_validated_and_staged_while_the_old_preview_keeps_playing() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let displayed = state.displayed().unwrap().clone();
    let owned_before = coordinator.metrics().owned_processes;
    let artifact_before = w.service.stats().artifact_bytes;
    let context = w.start();
    worker_config(
        &context.draft,
        json!({"frames": 120, "tracks": [[0.0, 2.0]], "audio": "tone", "pixel": 40}),
    );
    fs::write(context.draft.join("src/lib.rs"), "// new scene\n").unwrap();
    let captured = w.capture(&context);
    let run = w.run(&captured, 0);
    assert!(run.report.passed(), "{:?}", run.report.failure());
    let staged = run.staged.as_ref().expect("a passing candidate is staged");
    // The staged playback matches the candidate, not the task base.
    let candidate = captured.candidate().revision().as_str();
    assert_eq!(staged.ready().identity().source_revision, candidate);
    assert_ne!(candidate, captured.source_base().revision().as_str());
    assert_eq!(staged.ready().timeline.total_frames, 120);
    assert!(staged.ready().frame.is_some());
    assert!(staged.ready().audio_source.is_some());
    assert_eq!(run.report.audio().unwrap().sample_rate, 48_000);
    assert!(run.report.build().is_some());
    // The candidate's prepared PCM is part of the shared build budget while the staged
    // source (an open file) exists...
    let pcm_bytes = run.report.audio().unwrap().sample_count * 8;
    assert!(pcm_bytes > 0);
    assert_eq!(
        w.service.stats().artifact_bytes,
        artifact_before + pcm_bytes
    );
    assert_eq!(run.report.candidate(), captured.candidate());
    // Nothing was published: the displayed preview is unchanged and still seekable.
    assert_eq!(state.displayed().unwrap(), &displayed);
    assert!(coordinator.events().ready.is_none());
    seek_and_wait(&coordinator, &mut state, 7);
    // The validation worker lives in its own scope, not in the coordinator's lanes.
    assert_eq!(coordinator.metrics().owned_processes, owned_before);
    drop(run);
    // ...and is returned to it once the staged candidate is dropped (its worker and lease
    // are released by the background teardown owner, never on the dropping thread).
    assert!(fframes_studio::teardown::Teardown::global().wait_idle(Duration::from_secs(30)));
    assert_eq!(w.service.stats().artifact_bytes, artifact_before);
    coordinator.close();
}

/// Run a passing candidate whose compiled timeline is `width`x`height`; returns the run.
fn run_sized(width: u32, height: u32) -> fframes_studio::candidate_runner::CandidateRun {
    let mut w = world();
    let context = w.start();
    worker_config(
        &context.draft,
        json!({"frames": 60, "width": width, "height": height, "vary_pixels": true}),
    );
    fs::write(context.draft.join("src/lib.rs"), "// sized scene\n").unwrap();
    let captured = w.capture(&context);
    w.run(&captured, 0)
}

#[test]
fn candidates_above_the_preview_cap_validate_and_stage_at_the_effective_scale() {
    // (compiled size, clamped preview size, effective scale): the real worker's clamp is
    // min(1280/w, 720/h, 1) and its response carries that scale.
    type Case = ((u32, u32), (u32, u32), f64);
    let cases: [Case; 4] = [
        ((640, 360), (640, 360), 1.),
        ((1920, 1080), (1280, 720), 1280. / 1920.),
        ((3840, 2160), (1280, 720), 1280. / 3840.),
        // Height binds, not width.
        ((1280, 1440), (640, 720), 0.5),
    ];
    for ((width, height), clamped, scale) in cases {
        let run = run_sized(width, height);
        assert!(
            run.report.passed(),
            "{width}x{height}: {:?}",
            run.report.failure()
        );
        assert!(!run.report.frames().is_empty());
        for frame in run.report.frames() {
            assert_eq!(
                (frame.width, frame.height),
                clamped,
                "{width}x{height} representative frame {}",
                frame.frame
            );
        }
        let staged = run.staged.as_ref().expect("a passing candidate is staged");
        let ready = staged.ready();
        assert_eq!(
            (ready.timeline.width, ready.timeline.height),
            (width as usize, height as usize)
        );
        let first = ready.frame.as_ref().expect("staged playhead frame");
        assert_eq!(first.response.scale, scale, "{width}x{height}");
        assert_eq!(
            (first.response.header.width, first.response.header.height),
            clamped
        );
    }
}

#[test]
fn compile_failure_reports_bounded_context_and_old_playback_survives() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let context = w.start();
    fs::write(context.draft.join("src/lib.rs"), "fn broken(").unwrap();
    let captured = w.capture(&context);
    w.compiler.fail.store(true, Ordering::SeqCst);
    let run = w.run(&captured, 0);
    assert!(run.staged.is_none());
    assert!(!run.report.passed());
    let failure = run.report.failure().unwrap();
    assert_eq!(failure.stage, ValidationStage::Compile);
    assert_eq!(failure.kind, FailureKind::Source);
    assert!(failure.output_tail.contains("E0425"));
    assert!(failure.changed_paths.contains(&"src/lib.rs".to_owned()));
    assert_eq!(failure.candidate, captured.candidate().revision().as_str());
    assert!(failure.build_key.is_some());
    // The failed validation is a repair candidate through the real Stage 1 budget.
    let budget = w.controller.agent_task().unwrap().repair().clone();
    assert!(matches!(
        next_step(&run.report, &budget),
        NextStep::Repair { attempt: 1, .. }
    ));
    seek_and_wait(&coordinator, &mut state, 11);
    coordinator.close();
}

#[test]
fn inspection_errors_worker_start_failures_and_cancellation_route_deterministically() {
    let mut w = world();
    let context = w.start();
    worker_config(
        &context.draft,
        json!({"inspect": [{"frame": 0, "severity": "error", "key": "missing_asset", "message": "media/absent.jpg"}]}),
    );
    let captured = w.capture(&context);
    let run = w.run(&captured, 0);
    let failure = run.report.failure().unwrap();
    assert_eq!(failure.stage, ValidationStage::Inspection);
    assert!(failure.summary.contains("absent.jpg"));
    assert!(run.staged.is_none());
    assert_eq!(
        w.controller.processes.active_count(),
        0,
        "worker reaped after failure"
    );

    // A worker that cannot start is the candidate's problem and may be repaired.
    let mut w = world();
    let context = w.start();
    worker_config(&context.draft, json!({"fail_start": true}));
    let captured = w.capture(&context);
    let run = w.run(&captured, 0);
    let failure = run.report.failure().unwrap();
    assert_eq!(
        (failure.stage, failure.kind),
        (ValidationStage::Worker, FailureKind::Source)
    );
    assert_eq!(w.controller.processes.active_count(), 0);

    // Cancelling the compile scope is environmental: it never spends the repair.
    let mut w = world();
    let context = w.start();
    fs::write(context.draft.join("src/lib.rs"), "// slow\n").unwrap();
    let captured = w.capture(&context);
    w.compiler.release.store(false, Ordering::SeqCst);
    let scopes = w.scopes();
    let compiler_scope = scopes.compiler.clone();
    let canceller = std::thread::spawn({
        let compiler = w.compiler.clone();
        move || {
            compiler.wait_started(1);
            compiler_scope.shutdown(Duration::ZERO);
        }
    });
    let run = run_candidate_validation(&captured, &w.checkpoints(), &w.config(), &scopes, 0, 0);
    canceller.join().unwrap();
    let failure = run.report.failure().unwrap();
    assert_eq!(failure.kind, FailureKind::Environment);
    let budget = w.controller.agent_task().unwrap().repair().clone();
    assert!(matches!(
        next_step(&run.report, &budget),
        NextStep::Retain { .. }
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    while w.compiler.killed.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < deadline,
            "the abandoned compile was not killed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn one_automatic_repair_then_terminal_with_the_second_failure_retained() {
    let mut w = world();
    let context = w.start();
    worker_config(&context.draft, json!({"render_fail_frame": 0}));
    let first = w.capture(&context);
    let run = w.run(&first, 0);
    assert_eq!(run.report.failure().unwrap().stage, ValidationStage::Render);
    assert!(matches!(
        w.controller
            .agent_validation_failed(&context.identity, "render failed")
            .unwrap(),
        RepairDecision::Repair { attempt: 1 }
    ));
    w.controller.agent_begin_repair(&context.identity).unwrap();
    // The repair reopens the same stable draft and captures a NEW immutable candidate.
    w.reopen(&context);
    worker_config(&context.draft, json!({"render_fail_frame": 89}));
    let second = w.capture(&context);
    assert_ne!(first.candidate(), second.candidate());
    let spent = w.controller.agent_task().unwrap().repair().clone();
    assert_eq!(spent.used(), 1);
    let run = w.run(&second, spent.used());
    assert_eq!(run.report.repair_count(), 1);
    match next_step(&run.report, &spent) {
        NextStep::Retain {
            context: Some(c), ..
        } => assert_eq!(c.repair_count, 1),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        w.controller
            .agent_validation_failed(&context.identity, "still failing")
            .unwrap(),
        RepairDecision::Exhausted
    );
    assert_eq!(
        w.controller.agent_task().unwrap().state(),
        TaskState::Failed
    );
}

#[test]
fn audio_only_edit_with_unchanged_video_passes_and_misplaced_audio_fails() {
    let mut w = world();
    let context = w.start();
    // Baseline: video only.
    worker_config(&context.draft, json!({"frames": 90, "pixel": 9}));
    let baseline = w.capture(&context);
    let baseline_run = w.run(&baseline, 0);
    assert!(baseline_run.report.passed());
    let frame_zero = baseline_run.report.frames()[0].sha256.clone();
    drop(baseline_run);
    w.controller
        .agent_validation_failed(&context.identity, "retry with audio")
        .unwrap();
    w.controller.agent_begin_repair(&context.identity).unwrap();
    w.reopen(&context);
    // Audio-only change: same pixels, a track now exists.
    worker_config(
        &context.draft,
        json!({"frames": 90, "pixel": 9, "tracks": [[0.5, 2.5]], "audio": "tone"}),
    );
    fs::write(context.draft.join("media/cue.wav"), b"RIFF new cue").unwrap();
    let audio_edit = w.capture(&context);
    let run = w.run(&audio_edit, 1);
    assert!(run.report.passed(), "{:?}", run.report.failure());
    assert_eq!(
        run.report.frames()[0].sha256,
        frame_zero,
        "frame zero is unchanged and that is fine"
    );
    let audio = run.report.audio().unwrap();
    assert!(audio.tracks[0].audible);
    assert!(audio.checks_passed.iter().any(|c| c == "track_placement"));
    drop(run);

    // Audible content outside every declared track window fails the placement check.
    let mut w = world();
    let context = w.start();
    worker_config(
        &context.draft,
        json!({"frames": 150, "tracks": [[0.0, 1.0]], "audio": "misplaced"}),
    );
    let captured = w.capture(&context);
    let run = w.run(&captured, 0);
    let failure = run.report.failure().unwrap();
    assert_eq!(failure.stage, ValidationStage::Audio);
    assert!(failure.summary.contains("outside"));
    assert!(run.staged.is_none());
}

#[test]
fn clipping_is_a_visible_warning_and_a_custom_main_import_needs_no_video_cli() {
    let mut w = world();
    let context = w.start();
    // The project's own main is not a video CLI: only the worker entry matters.
    fs::write(
        context.draft.join("src/main.rs"),
        "fn main() { println!(\"custom tool\"); }\n",
    )
    .unwrap();
    worker_config(
        &context.draft,
        json!({"tracks": [[0.0, 2.0]], "audio": "clip"}),
    );
    let captured = w.capture(&context);
    let run = w.run(&captured, 0);
    assert!(run.report.passed(), "{:?}", run.report.failure());
    assert!(
        run.report
            .diagnostics()
            .iter()
            .any(|d| d.key == "audio_clipping")
    );
    assert!(
        run.report
            .warnings()
            .iter()
            .any(|m| m.contains("full scale"))
    );
}

#[test]
fn an_unchanged_candidate_joins_the_ui_build_instead_of_recompiling() {
    let mut w = world();
    let (coordinator, _state) = display_base(&mut w);
    assert_eq!(w.compiler.started.load(Ordering::SeqCst), 1);
    let context = w.start();
    let captured = w.capture(&context);
    assert_eq!(
        captured.candidate().revision(),
        captured.source_base().revision()
    );
    let run = w.run(&captured, 0);
    assert!(run.report.passed(), "{:?}", run.report.failure());
    assert_eq!(
        w.compiler.started.load(Ordering::SeqCst),
        1,
        "equal key shared with the UI"
    );
    assert_eq!(w.service.stats().cache_hits, 1);
    coordinator.close();
}

#[test]
fn an_incompatible_worker_is_an_environment_failure_no_repair_can_fix() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let context = w.start();
    worker_config(&context.draft, json!({"backend": "gpu"}));
    let captured = w.capture(&context);
    let run = w.run(&captured, 0);
    let failure = run.report.failure().expect("negotiation must fail");
    assert_eq!(
        (failure.stage, failure.kind),
        (ValidationStage::Worker, FailureKind::Environment)
    );
    assert!(failure.output_tail.contains("Incompatible preview SDK"));
    let budget = w.controller.agent_task().unwrap().repair().clone();
    assert!(matches!(
        next_step(&run.report, &budget),
        NextStep::Retain { .. }
    ));
    assert!(run.staged.is_none());
    seek_and_wait(&coordinator, &mut state, 4);
    coordinator.close();
}

#[test]
fn the_workers_declared_shader_gap_reaches_the_report_as_a_visible_warning() {
    let mut w = world();
    let context = w.start();
    worker_config(
        &context.draft,
        json!({"capability_gaps": ["shader_preview"]}),
    );
    let captured = w.capture(&context);
    let run = w.run(&captured, 0);
    assert!(run.report.passed(), "{:?}", run.report.failure());
    assert_eq!(run.report.capability_gaps(), ["shader_preview".to_owned()]);
    assert!(
        run.report
            .warnings()
            .iter()
            .any(|m| m.contains("Unsupported shaders"))
    );
    // The preview still stages: the limitation is visible, not fatal.
    assert!(run.staged.is_some());
}

#[test]
fn stop_during_validation_is_environmental_and_leaves_the_old_preview_alone() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let displayed = state.displayed().unwrap().clone();
    let context = w.start();
    // Every worker request takes 40 ms, so the run is still rendering when Stop lands.
    worker_config(&context.draft, json!({"delay_ms": 40}));
    let captured = w.capture(&context);
    let scopes = w.scopes();
    let stop = scopes.worker.clone();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        stop.shutdown(Duration::ZERO);
    });
    let run = run_candidate_validation(&captured, &w.checkpoints(), &w.config(), &scopes, 0, 0);
    stopper.join().unwrap();
    let failure = run.report.failure().expect("stopped");
    assert_eq!(failure.kind, FailureKind::Environment);
    assert!(run.staged.is_none());
    let budget = w.controller.agent_task().unwrap().repair().clone();
    assert!(matches!(
        next_step(&run.report, &budget),
        NextStep::Retain { .. }
    ));
    assert_eq!(budget.used(), 0);
    assert_eq!(state.displayed().unwrap(), &displayed);
    seek_and_wait(&coordinator, &mut state, 9);
    coordinator.close();
}

#[test]
fn old_playback_survives_every_kind_of_validation_failure() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let displayed = state.displayed().unwrap().clone();
    let cases: Vec<(Value, ValidationStage, FailureKind)> = vec![
        (
            json!({"inspect": [{"frame": 0, "severity": "error", "key": "missing_asset", "message": "media/absent.jpg"}]}),
            ValidationStage::Inspection,
            FailureKind::Source,
        ),
        (
            json!({"render_fail_frame": 0}),
            ValidationStage::Render,
            FailureKind::Source,
        ),
        (
            json!({"frames": 150, "tracks": [[0.0, 1.0]], "audio": "misplaced"}),
            ValidationStage::Audio,
            FailureKind::Source,
        ),
        (
            json!({"fail_start": true}),
            ValidationStage::Worker,
            FailureKind::Source,
        ),
        (
            json!({"backend": "gpu"}),
            ValidationStage::Worker,
            FailureKind::Environment,
        ),
    ];
    for (i, (config, stage, kind)) in cases.into_iter().enumerate() {
        let context = w.start();
        worker_config(&context.draft, config);
        fs::write(context.draft.join("src/lib.rs"), format!("// case {i}\n")).unwrap();
        let captured = w.capture(&context);
        let run = w.run(&captured, 0);
        let failure = run.report.failure().expect("the case must fail");
        assert_eq!((failure.stage, failure.kind), (stage, kind), "case {i}");
        assert!(run.staged.is_none());
        assert_eq!(state.displayed().unwrap(), &displayed, "case {i}");
        assert!(coordinator.events().ready.is_none(), "case {i}");
        seek_and_wait(&coordinator, &mut state, 3 + i);
        drop(run);
        w.controller
            .finish_agent_task(&context.identity, TaskState::Failed, "case done")
            .unwrap();
    }
    coordinator.close();
}

#[test]
fn a_draft_that_changes_during_capture_fails_capture_and_keeps_the_old_preview() {
    let mut w = world();
    let (coordinator, mut state) = display_base(&mut w);
    let displayed = state.displayed().unwrap().clone();
    let context = w.start();
    let ticket = w.ticket(&context);
    let draft = context.draft.clone();
    let outcome = capture_candidate_observed(&ticket, &w.checkpoints(), &mut |phase| {
        if phase == CapturePhase::Scanned {
            fs::write(draft.join("src/lib.rs"), "// changed under the capture\n").unwrap();
        }
    });
    assert_eq!(outcome.err(), Some(CandidateError::DraftChanged));
    // No candidate was registered and nothing about playback moved.
    assert!(w.controller.agent_task().unwrap().candidate().is_none());
    assert_eq!(state.displayed().unwrap(), &displayed);
    seek_and_wait(&coordinator, &mut state, 5);
    coordinator.close();
}

// ---- the six tools --------------------------------------------------------------------------

struct Tools {
    backend: Arc<ProjectToolBackend>,
    dispatcher: ToolDispatcher,
    artifacts: PathBuf,
}

fn tools(w: &World) -> Tools {
    let artifacts = w._temp.path().join("artifacts");
    let backend = Arc::new(
        ProjectToolBackend::new(ToolBackendConfig {
            project_id: w.controller.project.manifest.project_id.clone(),
            service: w.service.clone(),
            sdk: w.sdk.clone(),
            compatibility: manifest(),
            builds: w.paths.builds(),
            history: w.paths.project(&w.controller.project.manifest.project_id),
            artifacts: artifacts.clone(),
            processes: w.controller.processes.sub_manager(),
            gate: WriterGate::default(),
        })
        .unwrap(),
    );
    Tools {
        dispatcher: ToolDispatcher::new(backend.clone()),
        backend,
        artifacts,
    }
}

fn call(
    t: &Tools,
    binding: &ToolBinding,
    method: &str,
    params: Value,
) -> Result<Value, ToolErrorCode> {
    t.dispatcher
        .dispatch(binding, method, &params, &|| false)
        .map_err(|e| e.code)
}

fn draft_binding(t: &Tools, ctx: &AgentTaskContext) -> ToolBinding {
    t.backend.register_task(
        &ctx.identity,
        ctx.draft.clone(),
        ctx.source_base.revision().clone(),
    );
    ToolBinding {
        task: ctx.identity.clone(),
        revision: BoundRevision::Draft,
    }
}

#[test]
fn six_tools_answer_for_labelled_immutable_draft_revisions() {
    let mut w = world();
    let context = w.start();
    worker_config(
        &context.draft,
        json!({"frames": 60, "scenes": [[0, 30], [30, 60]], "tracks": [[0.0, 1.0]], "pixel": 33,
               "inspect": [{"frame": 3, "severity": "warning", "key": "shader_unsupported", "message": "falls back"}]}),
    );
    let t = tools(&w);
    let binding = draft_binding(&t, &context);

    // build_status observes: no capture, no compile.
    let status = call(&t, &binding, "build_status", json!({})).unwrap();
    assert_eq!(w.compiler.started.load(Ordering::SeqCst), 0);
    assert_eq!(status["revision"]["label"], "task_base");
    assert_eq!(status["revision"]["validated"], false);
    assert_eq!(status["result"]["service"]["compiles_started"], 0);

    let ctx_reply = call(&t, &binding, "project_context", json!({})).unwrap();
    let first_revision = ctx_reply["revision"]["id"].as_str().unwrap().to_owned();
    assert_eq!(ctx_reply["revision"]["label"], "draft_snapshot");
    assert_eq!(ctx_reply["revision"]["validated"], false);
    assert_ne!(first_revision, context.source_base.revision().as_str());
    let files = ctx_reply["result"]["files"].as_array().unwrap();
    assert!(files.iter().any(|f| f["path"] == "src/lib.rs"));
    assert_eq!(
        ctx_reply["result"]["task"]["base_revision"],
        context.source_base.revision().as_str()
    );
    assert_eq!(
        w.compiler.started.load(Ordering::SeqCst),
        0,
        "project_context never compiles"
    );

    let timeline = call(&t, &binding, "timeline", json!({})).unwrap();
    assert_eq!(timeline["result"]["total_frames"], 60);
    assert_eq!(timeline["result"]["scenes"].as_array().unwrap().len(), 2);
    assert_eq!(timeline["revision"]["id"], first_revision);
    assert_eq!(w.compiler.started.load(Ordering::SeqCst), 1);

    let frame = call(
        &t,
        &binding,
        "render_frame",
        json!({"frame": 4, "scale": 1.0}),
    )
    .unwrap();
    let artifact = &frame["artifacts"][0];
    assert_eq!(artifact["media_type"], "image/png");
    let path = PathBuf::from(artifact["path"].as_str().unwrap());
    assert!(path.starts_with(t.backend.artifacts().root()));
    let bytes = fs::read(&path).unwrap();
    assert_eq!(artifact["bytes"], bytes.len());
    assert!(bytes.len() <= 8 * 1024 * 1024);
    assert_eq!(artifact["sha256"], format!("{:x}", sha2_hex(&bytes)));
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (8, 4));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    assert!(!serde_json::to_string(&frame).unwrap().contains("base64"));

    let strip = call(
        &t,
        &binding,
        "render_strip",
        json!({"start": 0, "end": 59, "count": 5, "scale": 1.0}),
    )
    .unwrap();
    assert_eq!(strip["result"]["frames"].as_array().unwrap().len(), 5);
    assert_eq!(strip["result"]["width"], 8 * 5);
    assert_eq!(strip["artifacts"].as_array().unwrap().len(), 1);

    let inspect = call(&t, &binding, "inspect", json!({"frames": [3, 10]})).unwrap();
    assert_eq!(
        inspect["result"]["diagnostics"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        inspect["result"]["diagnostics"][0]["key"],
        "shader_unsupported"
    );
    // Out-of-range frames and ranges are rejected before any worker I/O.
    assert_eq!(
        call(&t, &binding, "inspect", json!({"frames": [60]})).err(),
        Some(ToolErrorCode::InvalidParams)
    );
    assert_eq!(
        call(&t, &binding, "render_frame", json!({"frame": 60})).err(),
        Some(ToolErrorCode::InvalidParams)
    );
    assert_eq!(
        call(
            &t,
            &binding,
            "render_strip",
            json!({"start": 0, "end": 500, "count": 4})
        )
        .err(),
        Some(ToolErrorCode::InvalidParams)
    );
    assert_eq!(
        call(
            &t,
            &binding,
            "render_strip",
            json!({"start": 0, "end": 5, "count": 25})
        )
        .err(),
        Some(ToolErrorCode::InvalidParams)
    );

    // The same draft revision reused one compile and one worker.
    assert_eq!(w.compiler.started.load(Ordering::SeqCst), 1);
    assert_eq!(t.backend.worker_count(), 1);

    // An edit produces a different labelled revision (and a different build key).
    worker_config(&context.draft, json!({"frames": 90}));
    let second = call(&t, &binding, "timeline", json!({})).unwrap();
    assert_ne!(second["revision"]["id"], first_revision);
    assert_eq!(second["result"]["total_frames"], 90);
    assert_eq!(w.compiler.started.load(Ordering::SeqCst), 2);
    // An earlier snapshot of this task can still be asked about; a stranger cannot.
    let again = call(
        &t,
        &binding,
        "timeline",
        json!({"revision": first_revision}),
    )
    .unwrap();
    assert_eq!(again["result"]["total_frames"], 60);
    assert_eq!(
        call(
            &t,
            &binding,
            "timeline",
            json!({"revision": "f".repeat(64)})
        )
        .err(),
        Some(ToolErrorCode::StaleRevision)
    );
    // Tool workers are bounded; each uses its own scope, never the UI lanes.
    assert!(t.backend.worker_count() <= 2);
}

#[test]
fn cached_revision_labels_follow_the_binding_not_the_open_order() {
    for draft_first in [false, true] {
        let mut w = world();
        let context = w.start();
        let t = tools(&w);
        let draft = draft_binding(&t, &context);
        let base_revision = context.source_base.revision().clone();
        let base = ToolBinding {
            task: context.identity.clone(),
            revision: BoundRevision::Fixed(base_revision.clone()),
        };

        if draft_first {
            let draft_reply = call(&t, &draft, "project_context", json!({})).unwrap();
            assert_eq!(draft_reply["revision"]["id"], base_revision.as_str());
            assert_eq!(draft_reply["revision"]["label"], "draft_snapshot");

            let base_reply = call(&t, &base, "project_context", json!({})).unwrap();
            assert_eq!(base_reply["revision"]["id"], base_revision.as_str());
            assert_eq!(base_reply["revision"]["label"], "task_base");
        } else {
            let base_reply = call(&t, &base, "project_context", json!({})).unwrap();
            assert_eq!(base_reply["revision"]["label"], "task_base");

            let draft_reply = call(
                &t,
                &draft,
                "project_context",
                json!({"revision": base_revision.as_str()}),
            )
            .unwrap();
            assert_eq!(draft_reply["revision"]["id"], base_revision.as_str());
            assert_eq!(draft_reply["revision"]["label"], "draft_snapshot");
        }

        t.backend.close();
    }
}

fn sha2_hex(bytes: &[u8]) -> impl std::fmt::LowerHex {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
}

#[test]
fn draft_requests_return_busy_while_the_writer_gate_is_held_and_never_compile() {
    let mut w = world();
    let context = w.start();
    let t = tools(&w);
    let binding = draft_binding(&t, &context);
    let guard = t.backend.writer_gate().try_acquire().unwrap();
    assert_eq!(
        call(&t, &binding, "timeline", json!({})).err(),
        Some(ToolErrorCode::Busy)
    );
    assert_eq!(
        call(&t, &binding, "project_context", json!({})).err(),
        Some(ToolErrorCode::Busy)
    );
    assert_eq!(w.compiler.started.load(Ordering::SeqCst), 0);
    drop(guard);
    assert!(call(&t, &binding, "project_context", json!({})).is_ok());
}

#[test]
fn fixed_bindings_answer_for_the_captured_candidate_and_share_the_validation_build() {
    let mut w = world();
    let context = w.start();
    worker_config(&context.draft, json!({"frames": 45}));
    let captured = w.capture(&context);
    let run = w.run(&captured, 0);
    assert!(run.report.passed());
    drop(run);
    assert_eq!(w.compiler.started.load(Ordering::SeqCst), 1);
    let t = tools(&w);
    t.backend.register_task(
        &context.identity,
        context.draft.clone(),
        context.source_base.revision().clone(),
    );
    let binding = ToolBinding {
        task: context.identity.clone(),
        revision: BoundRevision::Fixed(captured.candidate().revision().clone()),
    };
    let timeline = call(&t, &binding, "timeline", json!({})).unwrap();
    assert_eq!(timeline["revision"]["label"], "candidate");
    assert_eq!(
        timeline["revision"]["id"],
        captured.candidate().revision().as_str()
    );
    assert_eq!(timeline["result"]["total_frames"], 45);
    assert_eq!(
        w.compiler.started.load(Ordering::SeqCst),
        1,
        "tools joined the validation build"
    );
    // An assertion naming another revision is rejected, as is an unknown fixed revision.
    assert_eq!(
        call(
            &t,
            &binding,
            "timeline",
            json!({"revision": "a".repeat(64)})
        )
        .err(),
        Some(ToolErrorCode::StaleRevision)
    );
    let missing = ToolBinding {
        task: context.identity.clone(),
        revision: BoundRevision::Fixed("e".repeat(64).try_into().unwrap()),
    };
    assert_eq!(
        call(&t, &missing, "timeline", json!({})).err(),
        Some(ToolErrorCode::Unavailable)
    );
    let environment = fframes_studio::build_service::CompileEnvironment::resolve(
        &w.sdk,
        &manifest(),
        &w.paths.builds(),
    )
    .unwrap();
    let key = BuildKey::worker(&studio_project::open(&context.draft).unwrap(), &environment);
    assert!(matches!(
        w.service.status(&key),
        fframes_studio::build_service::BuildState::Ready { .. }
    ));
}

#[test]
fn captured_snapshot_history_is_bounded_and_evicted_identities_are_stale() {
    use fframes_studio::agent_tools::{
        MAX_TEXT_REPLY_BYTES,
        backend::{MAX_KNOWN_REVISIONS, STATUS_KNOWN_REVISIONS},
    };
    let mut w = world();
    let context = w.start();
    let t = tools(&w);
    let binding = draft_binding(&t, &context);
    let captured = MAX_KNOWN_REVISIONS + 8;
    let mut ids = Vec::new();
    for n in 0..captured {
        worker_config(&context.draft, json!({"frames": 10 + n}));
        let reply = call(&t, &binding, "project_context", json!({})).unwrap();
        let id = reply["revision"]["id"].as_str().unwrap().to_owned();
        assert!(!ids.contains(&id), "every edit is a distinct snapshot");
        ids.push(id);
    }

    let status = call(&t, &binding, "build_status", json!({})).unwrap();
    let listed = status["result"]["known_revisions"].as_array().unwrap();
    assert_eq!(listed.len(), STATUS_KNOWN_REVISIONS);
    assert_eq!(listed.last().unwrap(), ids.last().unwrap().as_str());
    assert_eq!(status["result"]["known_revisions_total"], captured);
    assert_eq!(status["result"]["known_revisions_truncated"], true);
    assert!(serde_json::to_vec(&status).unwrap().len() < MAX_TEXT_REPLY_BYTES);

    // The oldest identities were evicted: the existing stale error, never a restore.
    let evicted = captured - MAX_KNOWN_REVISIONS;
    for id in &ids[..evicted] {
        assert_eq!(
            call(&t, &binding, "project_context", json!({"revision": id})).err(),
            Some(ToolErrorCode::StaleRevision)
        );
    }
    // The oldest retained and the newest identities are still valid.
    for id in [&ids[evicted], ids.last().unwrap()] {
        let again = call(&t, &binding, "project_context", json!({"revision": id})).unwrap();
        assert_eq!(again["revision"]["id"], id.as_str());
    }
}

#[test]
fn tool_access_is_bound_to_the_task_and_project_and_closes_cleanly() {
    let mut w = world();
    let context = w.start();
    worker_config(&context.draft, json!({"frames": 30}));
    let t = tools(&w);
    let binding = draft_binding(&t, &context);
    call(&t, &binding, "render_frame", json!({"frame": 1})).unwrap();
    assert_eq!(t.backend.worker_count(), 1);
    assert!(!t.backend.artifacts().is_empty());
    // Another generation of "the same" task is stale; an unknown task is stale.
    let mut stale = binding.clone();
    stale.task.generation += 1;
    assert_eq!(
        call(&t, &stale, "timeline", json!({})).err(),
        Some(ToolErrorCode::StaleTask)
    );
    // Cross-project: an explicit assertion and a foreign identity are both refused.
    assert_eq!(
        call(&t, &binding, "timeline", json!({"project": "someone-else"})).err(),
        Some(ToolErrorCode::CrossProject)
    );
    let mut foreign = binding.clone();
    foreign.task.project = "foreign-project".to_owned().try_into().unwrap();
    assert_eq!(
        call(&t, &foreign, "timeline", json!({})).err(),
        Some(ToolErrorCode::CrossProject)
    );
    // Unknown methods and fields never reach the backend.
    assert_eq!(
        call(&t, &binding, "shell", json!({})).err(),
        Some(ToolErrorCode::MethodNotFound)
    );
    assert_eq!(
        call(&t, &binding, "timeline", json!({"path": "../x"})).err(),
        Some(ToolErrorCode::InvalidParams)
    );
    // Task end releases the workers, restored trees and artifacts it owned.
    assert!(fframes_studio::agent_tools::TaskLiveness::is_live(
        &*t.backend,
        &context.identity
    ));
    t.backend.unregister_task(&context.identity.task);
    assert_eq!(t.backend.worker_count(), 0);
    assert!(t.backend.artifacts().is_empty());
    assert_eq!(
        call(&t, &binding, "timeline", json!({})).err(),
        Some(ToolErrorCode::StaleTask)
    );
    assert!(!fframes_studio::agent_tools::TaskLiveness::is_live(
        &*t.backend,
        &context.identity
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    while w.controller.processes.active_count() > 0 {
        assert!(Instant::now() < deadline, "tool worker survived task end");
        std::thread::sleep(Duration::from_millis(10));
    }
    t.backend.close();
    assert!(t.artifacts.exists());
}

#[test]
fn at_most_two_tool_workers_live_and_close_releases_all_of_them() {
    let mut w = world();
    let context = w.start();
    let t = tools(&w);
    let binding = draft_binding(&t, &context);
    for frames in [30, 31, 32, 33] {
        worker_config(&context.draft, json!({"frames": frames}));
        let reply = call(&t, &binding, "timeline", json!({})).unwrap();
        assert_eq!(reply["result"]["total_frames"], frames);
        assert!(t.backend.worker_count() <= 2, "worker bound");
    }
    assert_eq!(t.backend.worker_count(), 2);
    t.backend.close();
    assert_eq!(t.backend.worker_count(), 0);
    assert_eq!(
        call(&t, &binding, "timeline", json!({})).err(),
        Some(ToolErrorCode::StaleTask)
    );
}

#[test]
fn artifact_store_enforces_ids_expiry_size_and_symlink_containment() {
    let temp = tempfile::tempdir().unwrap();
    let store =
        ArtifactStore::with_limit(&temp.path().join("art"), Duration::from_millis(400), 4096)
            .unwrap();
    let task = studio_engine::AgentTaskId::new();
    let png = vec![0u8; 100];
    let artifact = store.put_png(&task, &png, 1, 1).unwrap();
    assert!(artifact.id.starts_with("art-") && artifact.id.len() == 36);
    assert_eq!(
        store.read_png_for_task(&task, &artifact.id, 100).unwrap(),
        png
    );
    assert_eq!(
        store
            .read_png_for_task(&studio_engine::AgentTaskId::new(), &artifact.id, 100)
            .err()
            .map(|error| error.code),
        Some(ToolErrorCode::Unauthorized)
    );
    assert_eq!(
        store
            .read_png_for_task(&task, &artifact.id, 99)
            .err()
            .map(|error| error.code),
        Some(ToolErrorCode::TooLarge)
    );
    assert_eq!(
        store.resolve(&artifact.id).unwrap(),
        PathBuf::from(&artifact.path)
    );
    for bad in [
        "../x",
        "art-../../etc/passwd",
        "art-xyz",
        "",
        "ART-0123456789abcdef0123456789abcdef",
    ] {
        assert_eq!(
            store.resolve(bad).err().map(|e| e.code),
            Some(ToolErrorCode::InvalidParams),
            "{bad}"
        );
    }
    assert_eq!(
        store
            .resolve(&format!("art-{}", "0".repeat(32)))
            .err()
            .map(|e| e.code),
        Some(ToolErrorCode::NotFound)
    );
    // Oversized images are refused; the total store size is bounded by eviction.
    assert_eq!(
        store
            .put_png(&task, &vec![0u8; 8 * 1024 * 1024 + 1], 1, 1)
            .err()
            .map(|e| e.code),
        Some(ToolErrorCode::TooLarge)
    );
    for _ in 0..60 {
        store.put_png(&task, &[1u8; 100], 1, 1).unwrap();
    }
    assert!(store.len() <= 40, "the store stays within its byte limit");
    // A symlink swapped in for an artifact file is refused.
    #[cfg(unix)]
    {
        let victim = store.put_png(&task, &png, 1, 1).unwrap();
        let outside = temp.path().join("outside.png");
        fs::write(&outside, b"secret").unwrap();
        fs::remove_file(&victim.path).unwrap();
        std::os::unix::fs::symlink(&outside, &victim.path).unwrap();
        assert_eq!(
            store.resolve(&victim.id).err().map(|e| e.code),
            Some(ToolErrorCode::Unauthorized)
        );
    }
    // Expiry removes the file and the id.
    let short = store.put_png(&task, &png, 1, 1).unwrap();
    std::thread::sleep(Duration::from_millis(450));
    assert_eq!(
        store.resolve(&short.id).err().map(|e| e.code),
        Some(ToolErrorCode::NotFound)
    );
    assert!(!Path::new(&short.path).exists());
    // Task removal drops everything the task owned.
    let owned = store.put_png(&task, &png, 1, 1).unwrap();
    store.remove_task(&task);
    assert!(!Path::new(&owned.path).exists());
    assert!(store.is_empty());
}

#[test]
fn mcp_session_config_passes_only_the_capability_path() {
    let temp = tempfile::tempdir().unwrap();
    let capability = temp.path().join("cap-test.json");
    let secret = "s".repeat(64);
    fs::write(&capability, format!("{{\"secret\":\"{secret}\"}}")).unwrap();
    let grant = fframes_studio::agent_tools::broker::ToolGrant {
        capability_file: capability.clone(),
        expires_at: std::time::SystemTime::now(),
    };
    let command = temp.path().join("studio-mcp");
    let server = fframes_studio::agent_tools::mcp_server_for(&grant, &command).unwrap();
    assert_eq!(server.command, command);
    assert_eq!(server.args, ["--capability", capability.to_str().unwrap()]);
    assert!(server.env.is_empty());
    assert!(!format!("{server:?}").contains(&secret));
    assert!(!server.args.iter().any(|a| a.contains(&secret)));
    // A relative command is refused before any adapter sees it.
    assert!(fframes_studio::agent_tools::mcp_server_for(&grant, Path::new("studio-mcp")).is_err());
}

#[test]
fn cli_through_the_broker_reaches_the_real_backend_and_stops_after_the_task_ends() {
    use fframes_studio::agent_tools::broker::{BrokerConfig, ToolBroker};
    let mut w = world();
    let context = w.start();
    worker_config(
        &context.draft,
        json!({"frames": 40, "tracks": [[0.0, 1.0]], "audio": "tone"}),
    );
    let t = tools(&w);
    let binding = draft_binding(&t, &context);
    // Unix socket paths are short: use a short runtime directory.
    let runtime = tempfile::Builder::new()
        .prefix("fft")
        .tempdir_in("/tmp")
        .unwrap();
    let broker = ToolBroker::start(BrokerConfig {
        runtime_dir: runtime.path().join("rt"),
        dispatcher: ToolDispatcher::new(t.backend.clone()),
        liveness: t.backend.clone(),
    })
    .unwrap();
    let grant = broker
        .grant(binding.clone(), Duration::from_secs(300))
        .unwrap();
    let cli = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_studio-tools"))
            .arg("--capability")
            .arg(&grant.capability_file)
            .args(args)
            .output()
            .unwrap()
    };
    let out = cli(&["timeline"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let via_cli: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(via_cli["result"]["total_frames"], 40);
    assert_eq!(via_cli["revision"]["validated"], false);
    // The facade started no compiler of its own: exactly the backend's one shared compile.
    assert_eq!(w.compiler.started.load(Ordering::SeqCst), 1);
    let frame = cli(&["render_frame", "--json", "{\"frame\":3}"]);
    assert!(frame.status.success());
    let frame: Value = serde_json::from_slice(&frame.stdout).unwrap();
    assert!(Path::new(frame["artifacts"][0]["path"].as_str().unwrap()).is_file());
    let status = cli(&["build_status"]);
    assert!(status.status.success());
    assert_eq!(
        w.compiler.started.load(Ordering::SeqCst),
        1,
        "build_status never compiles"
    );
    // The same call straight through the dispatcher answers identically.
    let direct = call(&t, &binding, "timeline", json!({})).unwrap();
    assert_eq!(direct, via_cli);
    // Ending the task (liveness) refuses the next facade call.
    t.backend.unregister_task(&context.identity.task);
    let stale = cli(&["timeline"]);
    // The capability is refused at connect time: "capability unusable" (exit 3).
    assert_eq!(stale.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("stale_task"));
    drop(broker);
    assert!(
        !grant.capability_file.exists(),
        "the capability file is removed with the broker"
    );
}

#[test]
fn a_draft_that_names_another_project_is_never_served() {
    let mut w = world();
    let context = w.start();
    let manifest_path = context.draft.join("studio.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    value["project_id"] = json!("another-project");
    fs::write(&manifest_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let t = tools(&w);
    let binding = draft_binding(&t, &context);
    for method in ["project_context", "timeline"] {
        assert_eq!(
            call(&t, &binding, method, json!({})).err(),
            Some(ToolErrorCode::CrossProject),
            "{method}"
        );
    }
    assert_eq!(
        w.compiler.started.load(Ordering::SeqCst),
        0,
        "nothing was built"
    );
}

#[test]
fn a_task_that_ends_mid_call_leaves_no_worker_or_artifact_behind() {
    let mut w = world();
    let context = w.start();
    worker_config(&context.draft, json!({"delay_ms": 250}));
    let t = tools(&w);
    let binding = draft_binding(&t, &context);
    let task = context.identity.task.clone();
    // The task ends while its worker is still starting.
    let during_launch = std::thread::scope(|s| {
        let running = s.spawn(|| call(&t, &binding, "render_frame", json!({"frame": 1})));
        std::thread::sleep(Duration::from_millis(300));
        t.backend.unregister_task(&task);
        running.join().unwrap()
    });
    assert_eq!(during_launch.err(), Some(ToolErrorCode::StaleTask));
    assert_eq!(t.backend.worker_count(), 0);
    assert!(t.backend.artifacts().is_empty());

    // The task ends while a frame is rendering: the image is dropped, not stored.
    let binding = draft_binding(&t, &context);
    call(&t, &binding, "timeline", json!({})).unwrap();
    assert_eq!(t.backend.worker_count(), 1);
    let during_render = std::thread::scope(|s| {
        let running = s.spawn(|| call(&t, &binding, "render_frame", json!({"frame": 2})));
        std::thread::sleep(Duration::from_millis(100));
        t.backend.unregister_task(&task);
        running.join().unwrap()
    });
    assert_eq!(during_render.err(), Some(ToolErrorCode::StaleTask));
    assert_eq!(t.backend.worker_count(), 0);
    assert!(t.backend.artifacts().is_empty());
}

#[test]
fn concurrent_calls_on_distinct_revisions_never_exceed_the_worker_bound() {
    use studio_engine::candidate_validation::capture_draft_revision;
    let mut w = world();
    let context = w.start();
    worker_config(&context.draft, json!({"delay_ms": 60}));
    let store = w.checkpoints();
    let revisions: Vec<_> = (0..4)
        .map(|i| {
            fs::write(
                context.draft.join("src/lib.rs"),
                format!("// revision {i}\n"),
            )
            .unwrap();
            capture_draft_revision(&store, &context.draft).unwrap()
        })
        .collect();
    let t = tools(&w);
    t.backend.register_task(
        &context.identity,
        context.draft.clone(),
        context.source_base.revision().clone(),
    );
    let done = std::sync::atomic::AtomicBool::new(false);
    let (results, high_water) = std::thread::scope(|s| {
        let monitor = s.spawn(|| {
            let mut high = 0;
            while !done.load(Ordering::SeqCst) {
                high = high.max(t.backend.worker_count());
                std::thread::sleep(Duration::from_millis(1));
            }
            high
        });
        let callers: Vec<_> = revisions
            .iter()
            .map(|revision| {
                let binding = ToolBinding {
                    task: context.identity.clone(),
                    revision: BoundRevision::Fixed(revision.clone()),
                };
                let t = &t;
                s.spawn(move || call(t, &binding, "timeline", json!({})))
            })
            .collect();
        let results: Vec<_> = callers.into_iter().map(|c| c.join().unwrap()).collect();
        done.store(true, Ordering::SeqCst);
        (results, monitor.join().unwrap())
    });
    assert!(high_water <= 2, "worker bound exceeded: {high_water}");
    assert!(results.iter().any(Result::is_ok), "{results:?}");
    for result in &results {
        assert!(
            matches!(result, Ok(_) | Err(ToolErrorCode::Busy)),
            "{result:?}"
        );
    }
    assert!(t.backend.worker_count() <= 2);
}

/// Runs `studio-mcp` against `capability`, feeding `requests`, and returns every stdout
/// line parsed as JSON (stdout must hold nothing else).
fn mcp_session(capability: &Path, requests: &[Value]) -> Vec<Value> {
    use std::io::Write;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_studio-mcp"))
        .arg("--capability")
        .arg(capability)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout is JSON-RPC only"))
        .collect()
}

#[test]
fn cli_and_mcp_agree_on_revision_artifact_hashes_and_errors_over_the_real_backend() {
    use fframes_studio::agent_tools::broker::{BrokerConfig, ToolBroker};
    let mut w = world();
    let context = w.start();
    worker_config(
        &context.draft,
        json!({"frames": 40, "tracks": [[0.0, 1.0]], "audio": "tone", "pixel": 21}),
    );
    let t = tools(&w);
    let binding = draft_binding(&t, &context);
    let runtime = tempfile::Builder::new()
        .prefix("fft")
        .tempdir_in("/tmp")
        .unwrap();
    let broker = ToolBroker::start(BrokerConfig {
        runtime_dir: runtime.path().join("rt"),
        dispatcher: ToolDispatcher::new(t.backend.clone()),
        liveness: t.backend.clone(),
    })
    .unwrap();
    let grant = broker.grant(binding, Duration::from_secs(300)).unwrap();
    let cli = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_studio-tools"))
            .arg("--capability")
            .arg(&grant.capability_file)
            .args(args)
            .output()
            .unwrap()
    };
    let call_mcp = |id: u64, name: &str, arguments: Value| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}});
    let replies = mcp_session(
        &grant.capability_file,
        &[
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}),
            call_mcp(2, "timeline", json!({})),
            call_mcp(3, "render_frame", json!({"frame": 5})),
            call_mcp(4, "render_frame", json!({"frame": 9999})),
            call_mcp(5, "inspect", json!({"count": 4, "start": 0, "end": 39})),
            call_mcp(6, "project_context", json!({})),
        ],
    );
    assert_eq!(replies.len(), 6);

    // timeline: identical result, revision and label.
    let out = cli(&["timeline"]);
    assert!(out.status.success());
    let cli_timeline: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(replies[1]["result"]["structuredContent"], cli_timeline);

    // render_frame: same revision, same pixels (hash), distinct app-owned artifact files.
    let out = cli(&["render_frame", "--json", "{\"frame\":5}"]);
    assert!(out.status.success());
    let cli_frame: Value = serde_json::from_slice(&out.stdout).unwrap();
    let mcp_frame = &replies[2]["result"]["structuredContent"];
    assert_eq!(mcp_frame["revision"], cli_frame["revision"]);
    assert_eq!(mcp_frame["result"], cli_frame["result"]);
    assert_eq!(
        mcp_frame["artifacts"][0]["sha256"],
        cli_frame["artifacts"][0]["sha256"]
    );
    assert!(Path::new(mcp_frame["artifacts"][0]["path"].as_str().unwrap()).is_file());

    // The same out-of-range request fails with the same tool error code on both.
    let out = cli(&["render_frame", "--json", "{\"frame\":9999}"]);
    assert_eq!(out.status.code(), Some(1));
    let cli_error: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(replies[3]["result"]["isError"], true);
    let mcp_error: Value =
        serde_json::from_str(replies[3]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(mcp_error, cli_error["error"]);
    assert_eq!(mcp_error["code"], "invalid_params");

    // inspect and project_context agree too.
    let out = cli(&["inspect", "--json", "{\"count\":4,\"start\":0,\"end\":39}"]);
    let cli_inspect: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(replies[4]["result"]["structuredContent"], cli_inspect);
    let out = cli(&["project_context"]);
    let cli_context: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(replies[5]["result"]["structuredContent"], cli_context);
    // One compile served every facade call.
    assert_eq!(w.compiler.started.load(Ordering::SeqCst), 1);
    drop(broker);
}
