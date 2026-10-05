//! Real SDK promotion handoff: a validated candidate is published, its staged preview is
//! adopted under the engine's promotion authorization, re-primed at the latest seek and
//! installed with matching video and audio; a failed handoff keeps the old preview and
//! the committed source, which is then rebuilt rather than retagged.
//!
//! Every test is `#[ignore]` and PANICS (it does not skip) without a compatible SDK.
//! Lookup is the same as `real_sdk_candidates`: `SDK_BUNDLE` (assembled bundle directory,
//! installed into a temporary SDK home) or `SDK_ACTIVE` (an installed SDK directory).
//!
//!     SDK_BUNDLE=/path/to/bundle cargo test --locked -p fframes-studio \
//!         --test real_sdk_promotion -- --ignored --nocapture
//!
//! Skipped runs are not qualification.
use fframes_studio::{
    build_service::{BuildLimits, BuildService},
    candidate_runner::{CandidateRun, CandidateRunConfig, RunScopes, run_candidate_validation},
    preview_coordinator::{BuildSpec, PreviewCoordinator, SeekIntent},
    worker_project::CargoCompiler,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use studio_bootstrap::{ProcessTreeManager, WriterOwnership};
use studio_engine::{
    AgentTaskContext, CompletionOutcome, Controller, JobKind, JobResult, PreviewState,
    QuiescenceEvidence, ReadyPreview, TaskState, TurnCompletion, WriterObservation,
    app_paths::AppPaths,
    candidate_validation::{CapturedCandidate, capture_candidate},
};
use studio_project::checkpoint::Checkpoints;
use studio_sdk::CompatibilityManifest;

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

struct Real {
    _temp: tempfile::TempDir,
    sdk: PathBuf,
    manifest: CompatibilityManifest,
    paths: AppPaths,
    controller: Controller,
    service: BuildService,
}

impl Real {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let (sdk, manifest) = sdk(&temp);
        let root = temp.path().join("portable-video");
        preview_fixture::create(&root, &manifest);
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let controller = Controller::open(&root, &paths).unwrap();
        let service = BuildService::new(
            ProcessTreeManager::new(),
            Arc::new(CargoCompiler),
            BuildLimits::default(),
        );
        Self {
            _temp: temp,
            sdk,
            manifest,
            paths,
            controller,
            service,
        }
    }

    fn checkpoints(&self) -> Checkpoints {
        Checkpoints::new(
            &self
                .paths
                .project(&self.controller.project.manifest.project_id),
        )
        .unwrap()
    }

    fn spec(&mut self, tag: studio_engine::OperationTag) -> BuildSpec {
        BuildSpec {
            project: self.controller.project.clone(),
            sdk: self.sdk.clone(),
            compatibility: self.manifest.clone(),
            builds: self.paths.builds(),
            tag,
            compiler: self.controller.operation_processes(),
            worker: self.controller.processes.sub_manager(),
            service: self.service.clone(),
        }
    }

    /// Builds and installs the current source through the ordinary UI route.
    fn display_current(
        &mut self,
        coordinator: &PreviewCoordinator,
        state: &mut PreviewState,
    ) -> Arc<ReadyPreview> {
        let tag = self.controller.begin_job(JobKind::Build).unwrap();
        state.begin(tag.clone());
        coordinator.build(self.spec(tag));
        let deadline = Instant::now() + Duration::from_secs(600);
        let ready = loop {
            let e = coordinator.events();
            if let Some(tag) = e.compiled {
                self.controller
                    .complete(&tag, JobResult::Built(tag.base_source.clone()))
                    .unwrap();
            }
            if let Some((_, error)) = e.error {
                panic!("preview failed: {error}");
            }
            if let Some(r) = e.ready {
                self.controller.reconcile().unwrap();
                break r;
            }
            assert!(Instant::now() < deadline, "preview build deadline");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(coordinator.commit(ready.identity().clone(), ready.seek_serial));
        state.install(&ready, self.controller.state()).unwrap();
        ready
    }

    /// One task cycle up to a validated, staged candidate (not yet applied).
    fn candidate(
        &mut self,
        edit: impl FnOnce(&Path),
        playhead: usize,
    ) -> (AgentTaskContext, CapturedCandidate, CandidateRun, RunScopes) {
        let context = self
            .controller
            .begin_agent_task("real sdk promotion")
            .unwrap();
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
        let evidence = QuiescenceEvidence {
            identity: id.clone(),
            provider_session: None,
            writer: self
                .controller
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
        };
        let ticket = self
            .controller
            .agent_complete_quiescence(&id, &evidence)
            .unwrap();
        let store = self.checkpoints();
        let captured = capture_candidate(&ticket, &store).unwrap();
        self.controller
            .agent_record_candidate(ticket, &captured)
            .unwrap();
        let scopes = RunScopes {
            compiler: self.controller.processes.sub_manager(),
            worker: self.controller.processes.sub_manager(),
        };
        let run = run_candidate_validation(
            &captured,
            &store,
            &CandidateRunConfig {
                service: self.service.clone(),
                sdk: self.sdk.clone(),
                compatibility: self.manifest.clone(),
                builds: self.paths.builds(),
            },
            &scopes,
            playhead,
            0,
        );
        assert!(run.report.passed(), "{:?}", run.report.failure());
        (context, captured, run, scopes)
    }
}

/// A different tone in the cue: the audio of the candidate differs from the base.
fn retone(draft: &Path, hz: f64) {
    let path = draft.join("media/cue.wav");
    let mut wav = fs::read(&path).unwrap();
    for (i, chunk) in wav[44..].as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let t = i as f64 / 48_000.;
        let left = ((t * hz * std::f64::consts::TAU).sin() * 3000.) as i16;
        chunk[..2].copy_from_slice(&left.to_le_bytes());
    }
    fs::write(path, wav).unwrap();
}

fn pcm_hash(ready: &ReadyPreview) -> String {
    let source = ready.audio_source.as_ref().expect("retained PCM");
    let mut bytes = vec![0_u8; source.descriptor.byte_count as usize];
    source.file.read_exact_at(&mut bytes, 0).unwrap();
    format!("{:x}", Sha256::digest(&bytes))
}

fn seek_and_wait(
    p: &PreviewCoordinator,
    state: &mut PreviewState,
    position: usize,
    answered_by: &fframes_studio_protocol::PreviewIdentity,
) {
    let serial = state.seek(position, state.scale()).unwrap();
    p.seek(SeekIntent {
        identity: answered_by.clone(),
        serial,
        position,
        scale: state.scale(),
    });
    let deadline = Instant::now() + Duration::from_secs(10);
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

fn wait_event(
    p: &PreviewCoordinator,
    mut want: impl FnMut(&Arc<ReadyPreview>) -> bool,
) -> Result<Arc<ReadyPreview>, (studio_engine::OperationTag, String)> {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let e = p.events();
        if let Some(error) = e.error {
            return Err(error);
        }
        if let Some(ready) = e.ready
            && want(&ready)
        {
            return Ok(ready);
        }
        assert!(Instant::now() < deadline, "no ready preview");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "requires SDK_BUNDLE or SDK_ACTIVE; real compiler + preview worker + audio handoff"]
fn real_sdk_promotion_installs_matching_video_and_audio_and_a_failed_handoff_keeps_the_old_preview()
{
    let mut real = Real::new();
    let coordinator = PreviewCoordinator::new(real.controller.processes.sub_manager());
    let mut state = PreviewState::default();
    let base = real.display_current(&coordinator, &mut state);
    let base_identity = base.identity().clone();
    let base_pcm = pcm_hash(&base);
    seek_and_wait(&coordinator, &mut state, 25, &base_identity);

    // ---- a successful handoff -----------------------------------------------------------
    let (_context, captured, mut run, _scopes) = real.candidate(|d| retone(d, 880.), 10);
    let staged_at = run.staged.as_ref().expect("staged").ready().position;
    assert_eq!(staged_at, 10);
    let CompletionOutcome::Applied(promotion) = real
        .controller
        .complete_validated_task(&captured, &run.report)
        .unwrap()
    else {
        panic!("automatic policy applies a validated candidate");
    };
    let published = promotion.record.published.clone();
    assert_eq!(&published, captured.candidate().revision());
    assert_ne!(&published, captured.source_base().revision());
    assert_eq!(real.controller.state().source(), &published);
    // The saved checkpoint is not the accepted task revision.
    assert_ne!(real.controller.state().accepted(), &published);
    let authorization = promotion.authorization.clone().expect("authorization");
    assert_eq!(&authorization.tag().base_source, &published);
    coordinator
        .adopt(
            run.staged.take().unwrap(),
            &authorization,
            SeekIntent {
                identity: base_identity.clone(),
                serial: state.serial(),
                position: state.position(),
                scale: state.scale(),
            },
        )
        .unwrap();
    state.begin_promotion(&authorization);
    // The newest seek wins over the playhead the candidate was validated at.
    let serial = state.seek(40, state.scale()).unwrap();
    coordinator.seek(SeekIntent {
        identity: base_identity.clone(),
        serial,
        position: 40,
        scale: state.scale(),
    });
    let ready = wait_event(&coordinator, |r| r.seek_serial == serial).unwrap();
    assert_eq!(ready.tag(), authorization.tag());
    assert_eq!(ready.identity().source_revision, published.as_str());
    assert_ne!(
        ready.identity().source_revision,
        base_identity.source_revision
    );
    assert_eq!(ready.position, 40);
    assert_eq!(ready.frame.as_ref().unwrap().response.frame_index, 40);
    assert_eq!(ready.timeline.envelope.identity, *ready.identity());
    assert_eq!(ready.audio.envelope.identity, *ready.identity());
    assert_eq!(
        ready.pcm_start_sample,
        40 * u64::from(ready.audio.sample_rate) / ready.timeline.fps as u64
    );
    // Audio is the candidate's, not the previous revision's.
    assert_ne!(
        pcm_hash(&ready),
        base_pcm,
        "audio must come from the candidate"
    );
    assert_eq!(
        state.displayed(),
        Some(&base_identity),
        "nothing retagged yet"
    );
    real.controller.reconcile().unwrap();
    state.can_install(&ready, real.controller.state()).unwrap();
    assert!(coordinator.commit(ready.identity().clone(), ready.seek_serial));
    state.install(&ready, real.controller.state()).unwrap();
    let promoted_identity = ready.identity().clone();
    assert_eq!(state.displayed(), Some(&promoted_identity));
    seek_and_wait(&coordinator, &mut state, 7, &promoted_identity);

    // ---- a failed handoff ---------------------------------------------------------------
    let (_context, captured, mut run, scopes) = real.candidate(|d| retone(d, 660.), 3);
    let CompletionOutcome::Applied(second) = real
        .controller
        .complete_validated_task(&captured, &run.report)
        .unwrap()
    else {
        panic!("automatic policy applies a validated candidate");
    };
    let authorization = second.authorization.clone().expect("authorization");
    let staged = run.staged.take().unwrap();
    // The staged worker process dies (its scope is not sealed) before it can be re-primed.
    scopes.worker.terminate_all(Duration::ZERO);
    let serial = state.seek(33, state.scale()).unwrap();
    let latest = SeekIntent {
        identity: promoted_identity.clone(),
        serial,
        position: 33,
        scale: state.scale(),
    };
    coordinator.seek(latest.clone());
    coordinator.adopt(staged, &authorization, latest).unwrap();
    state.begin_promotion(&authorization);
    let (tag, message) = wait_event(&coordinator, |_| true).unwrap_err();
    assert_eq!(&tag, authorization.tag());
    state.fail(&tag, message);
    // The old preview is still displayed and still answers seeks; the second revision is
    // committed in the source and is not undone implicitly.
    assert_eq!(state.displayed(), Some(&promoted_identity));
    seek_and_wait(&coordinator, &mut state, 9, &promoted_identity);
    assert_eq!(real.controller.state().task_history().entries().len(), 2);
    assert_eq!(real.controller.state().source(), &second.record.published);
    // The accepted source is rebuilt as an ordinary preview, never retagged from the
    // old pixels.
    let rebuilt = real.display_current(&coordinator, &mut state);
    assert_eq!(
        rebuilt.identity().source_revision,
        second.record.published.as_str()
    );
    assert_ne!(rebuilt.identity(), &promoted_identity);
    assert_ne!(pcm_hash(&rebuilt), pcm_hash(&ready));
    coordinator.close();
}
