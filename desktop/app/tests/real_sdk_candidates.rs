//! Real SDK candidate validation (compiler + preview worker). Every test is `#[ignore]`
//! and PANICS (it does not skip) without a compatible SDK.
//!
//! SDK lookup (`fn sdk`): `SDK_BUNDLE` is an assembled bundle directory holding
//! `compatibility.json` and the `artifacts/*.tar.gz` it names (`file://artifacts/..` URLs
//! resolve relative to the bundle; see `desktop/scripts/assemble-phase-zero-sdk.py`). It is
//! installed into a temporary SDK home (or `CANDIDATE_SDK_HOME` when set). Without it,
//! `SDK_ACTIVE` names an already installed SDK directory.
//!
//!     SDK_BUNDLE=/path/to/bundle cargo test --locked -p fframes-studio \
//!         --test real_sdk_candidates -- --ignored --nocapture
//!
//! `real_sdk_candidate_validation_scenarios` walks one task through: baseline (validated,
//! staged, frame zero equal to the ordinary project CLI), comment-only edit (frame zero
//! unchanged is valid), audio-only edit, custom `main.rs` with no video CLI, compiler
//! failure (bounded context, changed paths), and an inspection failure at the playhead.
//! `real_sdk_candidates_above_the_preview_cap_validate_and_stage_at_the_effective_scale`
//! recomposes the fixture at 1920x1080 and 3840x2160 and requires validation and staging to
//! succeed with the worker's clamped 1280x720 frames.
//! `real_sdk_equal_key_requests_from_ui_tool_and_validation_run_cargo_once` counts real
//! Cargo compilations for equal and different keys.
//!
//! Skipped runs are not qualification.
use fframes_studio::{
    build_service::{
        BuildKey, BuildLimits, BuildService, CompileEnvironment, CompileRequest, Compiler,
        Subscriber, SubscriberKind,
    },
    candidate_runner::{CandidateRunConfig, RunScopes, run_candidate_validation},
    worker_project::{CargoCompiler, acquire_build_lock},
};
use sha2::Digest;
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use studio_bootstrap::{ProcessTreeManager, WriterOwnership};
use studio_engine::{
    AgentTaskContext, Controller, QuiescenceEvidence, TaskState, TurnCompletion, WriterObservation,
    app_paths::AppPaths,
    candidate_validation::{CapturedCandidate, FailureKind, ValidationStage, capture_candidate},
};
use studio_project::checkpoint::Checkpoints;
use studio_sdk::{CompatibilityManifest, ProjectManager};

#[path = "support/preview_fixture.rs"]
mod preview_fixture;

fn sdk(temp: &tempfile::TempDir) -> (PathBuf, CompatibilityManifest) {
    let path = if let Some(bundle) = std::env::var_os("SDK_BUNDLE") {
        let bundle = PathBuf::from(bundle);
        let manifest = CompatibilityManifest::from_json_str(
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
    let manifest = CompatibilityManifest::from_json_str(
        &fs::read_to_string(path.join("compatibility.json")).unwrap(),
    )
    .unwrap();
    (path, manifest)
}

fn qualified() -> WriterOwnership {
    WriterOwnership::ProcessGroupContained {
        qualification: "real-sdk-test".into(),
    }
}

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

struct Real {
    _temp: tempfile::TempDir,
    sdk: PathBuf,
    manifest: CompatibilityManifest,
    paths: AppPaths,
    controller: Controller,
    service: BuildService,
    /// Counts real Cargo compilations (the shared service keeps this at one per key).
    cargo: Arc<CountingCargo>,
}

struct CountingCargo(AtomicUsize);

impl Compiler for CountingCargo {
    fn compile(
        &self,
        request: &CompileRequest,
        scope: &ProcessTreeManager,
    ) -> Result<Arc<studio_engine::build_materialization::MaterializedBuild>, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        CargoCompiler.compile(request, scope)
    }
    fn accepts(&self, key: &BuildKey) -> Result<(), String> {
        CargoCompiler.accepts(key)
    }
    fn verify(&self, build: &studio_engine::build_materialization::MaterializedBuild) -> bool {
        CargoCompiler.verify(build)
    }
}

impl Real {
    fn new() -> Self {
        let cargo = Arc::new(CountingCargo(AtomicUsize::new(0)));
        let temp = tempfile::tempdir().unwrap();
        let (sdk, manifest) = sdk(&temp);
        let root = temp.path().join("portable-video");
        preview_fixture::create(&root, &manifest);
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let controller = Controller::open(&root, &paths).unwrap();
        Self {
            _temp: temp,
            sdk,
            manifest,
            paths,
            controller,
            service: BuildService::new(
                ProcessTreeManager::new(),
                cargo.clone(),
                BuildLimits::default(),
            ),
            cargo,
        }
    }

    /// One task cycle: edit the stable draft, quiesce, capture, validate.
    fn candidate(
        &mut self,
        edit: impl FnOnce(&std::path::Path),
        playhead: usize,
    ) -> (
        CapturedCandidate,
        fframes_studio::candidate_runner::CandidateRun,
    ) {
        let context: AgentTaskContext = self.controller.begin_agent_task("real sdk edit").unwrap();
        let id = context.identity.clone();
        self.controller
            .agent_writer_started(&id, qualified())
            .unwrap();
        self.controller
            .agent_task_transition(&id, TaskState::Editing)
            .unwrap();
        edit(&context.draft);
        self.controller
            .agent_task_transition(&id, TaskState::Quiescing)
            .unwrap();
        let evidence = evidence(&self.controller, &context);
        let ticket = self
            .controller
            .agent_complete_quiescence(&id, &evidence)
            .unwrap();
        let store = Checkpoints::new(
            &self
                .paths
                .project(&self.controller.project.manifest.project_id),
        )
        .unwrap();
        let captured = capture_candidate(&ticket, &store).unwrap();
        self.controller
            .agent_record_candidate(ticket, &captured)
            .unwrap();
        let run = run_candidate_validation(
            &captured,
            &store,
            &CandidateRunConfig {
                service: self.service.clone(),
                sdk: self.sdk.clone(),
                compatibility: self.manifest.clone(),
                builds: self.paths.builds(),
            },
            &RunScopes {
                compiler: self.controller.processes.sub_manager(),
                worker: self.controller.processes.sub_manager(),
            },
            playhead,
            0,
        );
        // Release the lease for the next task cycle; the draft is retained, not accepted.
        self.controller
            .finish_agent_task(&id, TaskState::Failed, "scenario complete")
            .unwrap();
        (captured, run)
    }
}

#[test]
#[ignore = "requires SDK_BUNDLE or SDK_ACTIVE; real compiler + preview worker candidate validation"]
fn real_sdk_candidate_validation_scenarios() {
    let mut real = Real::new();

    // Baseline: the unchanged fixture validates and stages matching playback.
    let (baseline, run) = real.candidate(|_| {}, 43);
    assert!(run.report.passed(), "{:?}", run.report.failure());
    assert_eq!(
        baseline.candidate().revision(),
        baseline.source_base().revision()
    );
    let staged = run.staged.as_ref().expect("staged");
    assert_eq!(staged.ready().timeline.total_frames, 165);
    let frame_zero = run.report.frames()[0].sha256.clone();
    assert_eq!(run.report.frames()[0].frame, 0);
    // The ordinary project CLI renders the same candidate bytes to the same pixels as the
    // validator's real preview frame (identical assets, fonts and CPU backend).
    let source = staged.ready().audio_source.as_ref().expect("retained PCM");
    let build = source.build().clone();
    let cli_scope = real.controller.processes.sub_manager();
    let lock = acquire_build_lock(
        &build.environment.target_dir.join(".build_lock"),
        &cli_scope,
        std::time::Duration::from_secs(120),
    )
    .unwrap();
    ProjectManager::build_project(&build.root, &build.environment, &cli_scope).unwrap();
    let png_dir = build.isolated_bin_dir.join("cli-frame");
    let (_, png) =
        ProjectManager::render_frame(&build.root, &build.environment, 0, &png_dir, &cli_scope)
            .unwrap();
    drop(lock);
    let cli_rgba = image::load_from_memory(&png).unwrap().into_rgba8();
    let first = &run.report.frames()[0];
    assert_eq!(cli_rgba.dimensions(), (first.width, first.height));
    assert_eq!(
        format!("{:x}", sha2::Sha256::digest(cli_rgba.as_raw())),
        first.sha256,
        "project CLI frame zero and validator frame zero are the same pixels"
    );
    let audio = run.report.audio().unwrap();
    assert_eq!(audio.sample_rate, 48_000);
    assert!(audio.tracks.iter().all(|t| t.audible), "{audio:?}");
    drop(run);

    // Comment-only edit: nothing visible changes (frame zero included) and that is valid.
    let (_, run) = real.candidate(
        |draft| {
            let path = draft.join("src/lib.rs");
            let mut source = fs::read_to_string(&path).unwrap();
            source.push_str("\n// touched by the agent\n");
            fs::write(path, source).unwrap();
        },
        0,
    );
    assert!(run.report.passed(), "{:?}", run.report.failure());
    assert_eq!(run.report.frames()[0].sha256, frame_zero);
    drop(run);

    // Audio-only edit: a different tone in the same cue; video is unchanged.
    let (_, run) = real.candidate(
        |draft| {
            let path = draft.join("media/cue.wav");
            let mut wav = fs::read(&path).unwrap();
            for (i, chunk) in wav[44..].as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let t = i as f64 / 48_000.;
                let left = ((t * 880. * std::f64::consts::TAU).sin() * 3000.) as i16;
                chunk[..2].copy_from_slice(&left.to_le_bytes());
            }
            fs::write(path, wav).unwrap();
        },
        0,
    );
    assert!(run.report.passed(), "{:?}", run.report.failure());
    assert_eq!(run.report.frames()[0].sha256, frame_zero);
    assert!(
        run.report
            .audio()
            .unwrap()
            .checks_passed
            .iter()
            .any(|c| c == "track_placement")
    );
    drop(run);

    // Custom main with no video CLI: only the worker entry is built and validated.
    let (_, run) = real.candidate(
        |draft| {
            fs::write(
                draft.join("src/main.rs"),
                "fn main() { println!(\"not a video cli\"); }\n",
            )
            .unwrap();
        },
        0,
    );
    assert!(run.report.passed(), "{:?}", run.report.failure());
    drop(run);

    // Compiler failure: bounded context naming the error and the changed path.
    let (_, run) = real.candidate(
        |draft| {
            fs::write(
                draft.join("src/lib.rs"),
                "compile_error!(\"deliberate broken candidate\");",
            )
            .unwrap();
        },
        0,
    );
    let failure = run.report.failure().expect("compile failure");
    assert_eq!(
        (failure.stage, failure.kind),
        (ValidationStage::Compile, FailureKind::Source)
    );
    assert!(failure.output_tail.contains("deliberate broken candidate"));
    assert!(failure.changed_paths.contains(&"src/lib.rs".to_owned()));
    assert!(run.staged.is_none());
    drop(run);

    // Inspection failure: media used only at the playhead is missing.
    let (_, run) = real.candidate(
        |draft| {
            let path = draft.join("src/lib.rs");
            let original = fs::read_to_string(&path).unwrap();
            // Only the main composition's own render_frame names `frame`/`ctx`.
            let missing = original.replace(
                "        let x = (frame.index * 7 % 1100) + 80;",
                "        if frame.index == 43 { let _ = ctx.get_image(\"absent.jpg\"); }\n        let x = (frame.index * 7 % 1100) + 80;",
            );
            assert_ne!(missing, original);
            fs::write(path, missing).unwrap();
        },
        43,
    );
    let failure = run.report.failure().expect("inspection failure");
    assert_eq!(failure.stage, ValidationStage::Inspection);
    assert!(
        failure.summary.contains("absent.jpg"),
        "{}",
        failure.summary
    );
    assert!(run.staged.is_none());
}

#[test]
#[ignore = "requires SDK_BUNDLE or SDK_ACTIVE; real compiler + preview worker above the 1280x720 preview cap"]
fn real_sdk_candidates_above_the_preview_cap_validate_and_stage_at_the_effective_scale() {
    let mut real = Real::new();
    // The fixture composes 1280x720; change only its declared composition size. The real
    // worker clamps every frame to min(1280/w, 720/h, 1) and answers with that scale.
    for ((width, height), scale) in [((1920, 1080), 1280. / 1920.), ((3840, 2160), 1280. / 3840.)] {
        let (_, run) = real.candidate(
            |draft| {
                let path = draft.join("src/lib.rs");
                let original = fs::read_to_string(&path).unwrap();
                let resized = original
                    .replace(
                        "const WIDTH: usize = 1280;",
                        &format!("const WIDTH: usize = {width};"),
                    )
                    .replace(
                        "const HEIGHT: usize = 720;",
                        &format!("const HEIGHT: usize = {height};"),
                    );
                assert_ne!(resized, original, "fixture no longer declares its size");
                fs::write(path, resized).unwrap();
            },
            43,
        );
        assert!(
            run.report.passed(),
            "{width}x{height}: {:?}",
            run.report.failure()
        );
        assert!(!run.report.frames().is_empty());
        for frame in run.report.frames() {
            assert_eq!(
                (frame.width, frame.height),
                (1280, 720),
                "{width}x{height} representative frame {}",
                frame.frame
            );
        }
        let staged = run
            .staged
            .as_ref()
            .expect("the validated candidate is staged");
        let ready = staged.ready();
        assert_eq!(
            (ready.timeline.width, ready.timeline.height),
            (width, height)
        );
        let shown = ready.frame.as_ref().expect("staged playhead frame");
        assert_eq!(shown.response.scale, scale);
        assert_eq!(
            (shown.response.header.width, shown.response.header.height),
            (1280, 720)
        );
        assert_eq!(ready.position, 43);
    }
}

#[test]
#[ignore = "requires SDK_BUNDLE or SDK_ACTIVE; real Cargo: equal-key UI/tool/validation requests compile once"]
fn real_sdk_equal_key_requests_from_ui_tool_and_validation_run_cargo_once() {
    let real = Real::new();
    let project = real.controller.project.clone();
    let environment =
        CompileEnvironment::resolve(&real.sdk, &real.manifest, &real.paths.builds()).unwrap();
    let key = BuildKey::worker(&project, &environment);
    let request = || CompileRequest {
        project: project.clone(),
        environment: environment.clone(),
        retained: None,
    };
    // Subscribe all three before any can finish: Cargo takes far longer than this loop.
    let subscriptions: Vec<_> = [
        (SubscriberKind::Ui, "ui"),
        (SubscriberKind::Tool, "tool"),
        (SubscriberKind::Validation, "validation"),
    ]
    .into_iter()
    .map(|(kind, name)| {
        real.service
            .subscribe(key.clone(), request(), Subscriber::new(kind, name))
            .unwrap()
    })
    .collect();
    let leases: Vec<_> = subscriptions
        .into_iter()
        .map(|s| std::thread::spawn(move || s.wait(&|| false).unwrap()))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();
    assert_eq!(
        real.cargo.0.load(Ordering::SeqCst),
        1,
        "one Cargo compile for three subscribers"
    );
    assert!(
        leases
            .windows(2)
            .all(|w| Arc::ptr_eq(w[0].build(), w[1].build()))
    );
    assert!(fframes_studio::worker_project::worker_binary(leases[0].build()).is_file());
    // A later equal request is served from the cache, still without a second compile.
    let later = real
        .service
        .subscribe(
            key.clone(),
            request(),
            Subscriber::new(SubscriberKind::Tool, "later"),
        )
        .unwrap();
    assert!(later.is_ready());
    assert_eq!(real.cargo.0.load(Ordering::SeqCst), 1);
    // A different revision is a different key and compiles separately.
    fs::write(
        project.root.join("src/lib.rs"),
        format!("{}\n// edit\n", preview_fixture::SOURCE),
    )
    .unwrap();
    let edited = studio_project::open(&project.root).unwrap();
    let other = real
        .service
        .subscribe(
            BuildKey::worker(&edited, &environment),
            CompileRequest {
                project: edited.clone(),
                environment: environment.clone(),
                retained: None,
            },
            Subscriber::new(SubscriberKind::Tool, "other"),
        )
        .unwrap()
        .wait(&|| false)
        .unwrap();
    assert_eq!(real.cargo.0.load(Ordering::SeqCst), 2);
    assert!(!Arc::ptr_eq(leases[0].build(), other.build()));
}
