use fframes_studio_protocol::*;
use std::{fs, path::Path};
use studio_bootstrap::WriterOwnership;
use studio_engine::{
    AgentTaskContext, CaptureTicket, CompiledScope, Controller, PreviewFrame, QuiescenceEvidence,
    RepairDecision, ScopeSelection, ScopedScene, TaskScope, TaskState, TurnCompletion,
    WriterObservation, app_paths::AppPaths, build_materialization::sdk_pin,
    candidate_validation::*,
};
use studio_project::checkpoint::Checkpoints;
use studio_sdk::CompatibilityManifest;

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

fn start(controller: &mut Controller) -> AgentTaskContext {
    let context = controller.begin_agent_task("edit").unwrap();
    controller
        .agent_writer_started(&context.identity, qualified())
        .unwrap();
    controller
        .agent_task_transition(&context.identity, TaskState::Editing)
        .unwrap();
    controller
        .agent_task_transition(&context.identity, TaskState::Quiescing)
        .unwrap();
    context
}

fn ticket(controller: &mut Controller, context: &AgentTaskContext) -> CaptureTicket {
    let evidence = evidence(controller, context);
    controller
        .agent_complete_quiescence(&context.identity, &evidence)
        .unwrap()
}

fn checkpoints(f: &Fixture, controller: &Controller) -> Checkpoints {
    Checkpoints::new(&f.paths.project(&controller.project.manifest.project_id)).unwrap()
}

#[test]
fn capture_names_identities_hashes_and_changed_paths_and_registers_with_the_task() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = start(&mut controller);
    fs::write(context.draft.join("src/lib.rs"), "// edited by the agent").unwrap();
    fs::write(context.draft.join("media/new.txt"), "new asset").unwrap();
    let t = ticket(&mut controller, &context);
    let store = checkpoints(&f, &controller);
    let captured = capture_candidate(&t, &store).unwrap();
    assert_eq!(captured.identity(), &context.identity);
    assert_eq!(captured.source_base(), &context.source_base);
    assert_ne!(
        captured.candidate().revision(),
        context.source_base.revision()
    );
    let manifest_bytes = fs::read(store.manifest_path(captured.candidate().revision())).unwrap();
    assert_eq!(
        captured.manifest_sha256(),
        format!("{:x}", sha2_digest(&manifest_bytes))
    );
    assert!(captured.objects().iter().any(|o| o.path == "studio.json"));
    let changes = captured.changes();
    assert_eq!(changes.total, 2);
    assert!(changes.entries.iter().any(|c| c.path == "src/lib.rs"
        && c.change == ChangeKind::Modified
        && c.kind == studio_project::revision::FileKind::Rust));
    assert!(
        changes
            .entries
            .iter()
            .any(|c| c.path == "media/new.txt" && c.change == ChangeKind::Added)
    );
    // The candidate registers as a distinct type and starts validation.
    controller.agent_record_candidate(t, &captured).unwrap();
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Validating
    );
    // Source promotion is not part of capture: the live project is untouched.
    assert_ne!(
        fs::read_to_string(f.root.join("src/lib.rs")).unwrap(),
        "// edited by the agent"
    );
}

fn sha2_digest(bytes: &[u8]) -> impl std::fmt::LowerHex {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
}

#[test]
fn unchanged_candidate_has_the_base_revision_and_no_changes() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = start(&mut controller);
    let t = ticket(&mut controller, &context);
    let captured = capture_candidate(&t, &checkpoints(&f, &controller)).unwrap();
    assert_eq!(
        captured.candidate().revision(),
        captured.source_base().revision()
    );
    assert!(captured.changes().is_empty());
}

#[test]
fn a_draft_that_changes_during_capture_fails_at_either_verification() {
    for phase in [CapturePhase::Scanned, CapturePhase::Captured] {
        let f = fixture();
        let mut controller = Controller::open(&f.root, &f.paths).unwrap();
        let context = start(&mut controller);
        let t = ticket(&mut controller, &context);
        let store = checkpoints(&f, &controller);
        let draft = context.draft.clone();
        let error = capture_candidate_observed(&t, &store, &mut |p| {
            if p == phase {
                fs::write(draft.join("src/lib.rs"), format!("// moved during {p:?}")).unwrap();
            }
        })
        .unwrap_err();
        assert_eq!(error, CandidateError::DraftChanged, "{phase:?}");
        // Nothing was registered: the task still waits for a stable capture.
        assert_eq!(
            controller.agent_task().unwrap().state(),
            TaskState::Quiescing
        );
        assert!(controller.agent_task().unwrap().candidate().is_none());
    }
}

#[test]
fn invalid_drafts_and_foreign_project_ids_are_rejected_before_validation() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = start(&mut controller);
    let manifest = fs::read_to_string(context.draft.join("studio.json")).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    value["project_id"] = serde_json::json!("another-project-identity");
    fs::write(
        context.draft.join("studio.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
    let t = ticket(&mut controller, &context);
    let store = checkpoints(&f, &controller);
    assert!(matches!(
        capture_candidate(&t, &store).unwrap_err(),
        CandidateError::ProjectMismatch { .. }
    ));
    fs::remove_file(context.draft.join("studio.json")).unwrap();
    assert!(matches!(
        capture_candidate(&t, &store).unwrap_err(),
        CandidateError::InvalidDraft(_)
    ));
    #[cfg(unix)]
    {
        fs::write(context.draft.join("studio.json"), manifest).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", context.draft.join("media/link")).unwrap();
        assert!(matches!(
            capture_candidate(&t, &store).unwrap_err(),
            CandidateError::InvalidDraft(_)
        ));
    }
}

#[test]
fn restored_revision_is_a_private_immutable_tree_removed_on_drop() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = start(&mut controller);
    fs::write(context.draft.join("src/lib.rs"), "// restored").unwrap();
    let t = ticket(&mut controller, &context);
    let store = checkpoints(&f, &controller);
    let captured = capture_candidate(&t, &store).unwrap();
    let restored = restore_revision(&store, captured.candidate().revision()).unwrap();
    assert_eq!(
        &restored.project.inventory.revision,
        captured.candidate().revision()
    );
    assert_eq!(
        fs::read_to_string(restored.root().join("src/lib.rs")).unwrap(),
        "// restored"
    );
    // Editing the stable draft afterwards cannot affect the restored tree.
    fs::write(context.draft.join("src/lib.rs"), "// later").unwrap();
    assert_eq!(
        fs::read_to_string(restored.root().join("src/lib.rs")).unwrap(),
        "// restored"
    );
    let root = restored.root().to_owned();
    drop(restored);
    assert!(!root.exists());
}

// ---- validation with a deterministic probe ------------------------------------------------

fn identity(revision: &str) -> PreviewIdentity {
    PreviewIdentity {
        project_id: "p".into(),
        open_session: "s".into(),
        source_revision: revision.into(),
        worker_generation: 1,
    }
}

fn envelope(revision: &str) -> PreviewEnvelope {
    PreviewEnvelope {
        contract_version: PREVIEW_CONTRACT_VERSION,
        identity: identity(revision),
        request_id: 1,
    }
}

fn timeline(
    revision: &str,
    total: usize,
    scenes: &[(usize, usize)],
    tracks: &[(f64, f64)],
) -> PreviewTimelineResponse {
    PreviewTimelineResponse {
        envelope: envelope(revision),
        fps: 30,
        width: 4,
        height: 2,
        total_frames: total,
        duration_seconds: total as f32 / 30.,
        scenes: scenes
            .iter()
            .enumerate()
            .map(|(i, (s, e))| PreviewSceneInfo {
                instance_id: format!("scene-{i}"),
                editor_instance_key: None,
                index: i,
                name: format!("s{i}"),
                full_name: format!("s{i}"),
                start_frame: *s,
                end_frame: *e,
                start_seconds: *s as f32 / 30.,
                end_seconds: *e as f32 / 30.,
            })
            .collect(),
        audio_tracks: tracks
            .iter()
            .map(|(s, e)| PreviewAudioTrackInfo {
                file: "cue.wav".into(),
                start_seconds: *s,
                end_seconds: *e,
                mix: TrackMixInfo {
                    gain_db: 0.,
                    pan: 0.,
                    fade_in: 0.,
                    fade_out: 0.,
                    offset: 0.,
                    voice: false,
                    duck_under_voice: false,
                },
            })
            .collect(),
    }
}

fn frame(revision: &str, index: usize, value: u8) -> PreviewFrame {
    let id = identity(revision);
    let header = FrameHeader::new_straight_rgba(revision, 1, 1, index, 4, 2).unwrap();
    PreviewFrame {
        response: ScaledFrameResponse {
            envelope: envelope(revision),
            frame_index: index,
            seek_serial: 0,
            scale: 1.,
            header,
            render_duration_micros: 1,
            record: BinaryRecordHeader {
                kind: BinaryRecordKind::FrameRgba8,
                identity: id,
                request_id: 1,
                offset: 0,
                payload_len: 32,
            },
            editor_metadata: None,
        },
        pixels: vec![value; 32],
    }
}

fn silent_audio(t: &PreviewTimelineResponse, _revision: &str) -> AudioProbe {
    let samples = (t.total_frames as u64 * 48_000).div_ceil(30);
    AudioProbe {
        descriptor: PreparedAudioDescriptor {
            envelope: t.envelope.clone(),
            artifact_id: "pcm-abc".into(),
            sample_rate: 48_000,
            channels: 2,
            sample_count: samples,
            byte_count: samples * 8,
            sha256: "a".repeat(64),
            silent: t.audio_tracks.is_empty(),
        },
        stats: PcmStats {
            sample_frames: samples,
            checked_windows: placement_windows(t, 48_000),
            ..Default::default()
        },
    }
}

struct Fake {
    revision: String,
    timeline: PreviewTimelineResponse,
    timeline_error: Option<ProbeError>,
    diagnostics: Vec<PreviewDiagnostic>,
    truncated: bool,
    render_error_at: Option<usize>,
    audio: Result<AudioProbe, ProbeError>,
    inspect_batches: Vec<usize>,
    inspected: Vec<usize>,
    rendered: Vec<usize>,
    pixel: u8,
    gaps: Vec<String>,
    /// Reports `cancelled()` once this many frames have been rendered (0: at once).
    cancel_after_renders: Option<usize>,
    /// Diagnostics returned by the nth inspection batch (overrides `diagnostics`).
    batch_diagnostics: Vec<(usize, Vec<PreviewDiagnostic>)>,
    /// Batch index whose response claims truncation.
    truncate_batch: Option<usize>,
    /// Frames carry this revision instead of the probed one.
    frame_revision: Option<String>,
}

impl Fake {
    fn new(revision: &str, total: usize) -> Self {
        let t = timeline(revision, total, &[(0, total / 2), (total / 2, total)], &[]);
        Self {
            revision: revision.into(),
            audio: Ok(silent_audio(&t, revision)),
            timeline: t,
            timeline_error: None,
            diagnostics: vec![],
            truncated: false,
            render_error_at: None,
            inspect_batches: vec![],
            inspected: vec![],
            rendered: vec![],
            pixel: 7,
            gaps: vec![],
            cancel_after_renders: None,
            batch_diagnostics: vec![],
            truncate_batch: None,
            frame_revision: None,
        }
    }
    fn with_timeline(mut self, t: PreviewTimelineResponse) -> Self {
        self.audio = Ok(silent_audio(&t, &self.revision));
        self.timeline = t;
        self
    }
}

impl CandidateProbe for Fake {
    fn timeline(&mut self) -> Result<PreviewTimelineResponse, ProbeError> {
        match &self.timeline_error {
            Some(e) => Err(e.clone()),
            None => Ok(self.timeline.clone()),
        }
    }
    fn inspect(&mut self, frames: &[usize]) -> Result<InspectResponse, ProbeError> {
        assert!(frames.len() <= MAX_INSPECT_FRAMES);
        let batch = self.inspect_batches.len();
        self.inspect_batches.push(frames.len());
        self.inspected.extend_from_slice(frames);
        let diagnostics = self
            .batch_diagnostics
            .iter()
            .find(|(n, _)| *n == batch)
            .map_or_else(|| self.diagnostics.clone(), |(_, d)| d.clone());
        Ok(InspectResponse {
            envelope: envelope(&self.revision),
            diagnostics,
            truncated: self.truncated || self.truncate_batch == Some(batch),
        })
    }
    fn render(&mut self, frame_index: usize) -> Result<PreviewFrame, ProbeError> {
        if self.render_error_at == Some(frame_index) {
            return Err(ProbeError::source(format!("panic rendering {frame_index}")));
        }
        self.rendered.push(frame_index);
        let revision = self.frame_revision.as_deref().unwrap_or(&self.revision);
        Ok(frame(revision, frame_index, self.pixel))
    }
    fn audio(&mut self) -> Result<AudioProbe, ProbeError> {
        self.audio.clone()
    }
    fn capability_gaps(&self) -> Vec<String> {
        self.gaps.clone()
    }
    fn cancelled(&self) -> bool {
        self.cancel_after_renders
            .is_some_and(|after| self.rendered.len() >= after)
    }
}

fn build_identity(captured: &CapturedCandidate) -> BuildIdentity {
    BuildIdentity {
        key_digest: "k".repeat(64),
        source_revision: captured.candidate().revision().as_str().into(),
        sdk_id: "sdk".into(),
        compatibility_digest: "d".repeat(64),
        toolchain: "1.98.1".into(),
        target_triple: "x86_64-unknown-linux-gnu".into(),
        package: "video".into(),
        worker_target: "worker".into(),
        profile: "debug".into(),
        backend: "cpu".into(),
    }
}

/// A captured candidate with the given draft edits, plus the live controller (kept for the
/// repair-budget tests).
fn captured_with(
    f: &Fixture,
    edits: impl FnOnce(&Path),
) -> (Controller, AgentTaskContext, CapturedCandidate) {
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = start(&mut controller);
    edits(&context.draft);
    let t = ticket(&mut controller, &context);
    let captured = capture_candidate(&t, &checkpoints(f, &controller)).unwrap();
    controller.agent_record_candidate(t, &captured).unwrap();
    (controller, context, captured)
}

#[test]
fn passing_report_names_every_identity_and_needs_no_pixel_or_title_change() {
    let f = fixture();
    // No source change at all: frame zero cannot differ from the base, and that is fine.
    let (_c, context, captured) = captured_with(&f, |_| {});
    let revision = captured.candidate().revision().as_str().to_owned();
    let mut probe = Fake::new(&revision, 300);
    let report = validate_candidate(
        &captured,
        BuildOutcome::Built(build_identity(&captured)),
        123,
        0,
        Some(&mut probe),
    );
    assert!(report.passed(), "{:?}", report.failure());
    assert_eq!(report.task(), &context.identity);
    assert_eq!(report.source_base(), &context.source_base);
    assert_eq!(report.candidate(), captured.candidate());
    assert_eq!(report.manifest_sha256(), captured.manifest_sha256());
    assert_eq!(report.build().unwrap().key_digest, "k".repeat(64));
    assert_eq!(report.audio().unwrap().sample_rate, 48_000);
    let coverage = report.coverage().unwrap();
    assert_eq!(coverage.broadened, None);
    assert!(coverage.complete);
    assert_eq!(coverage.playhead, 123);
    for required in [0usize, 299, 123, 150] {
        assert!(coverage.rendered_frames.contains(&required), "{required}");
    }
    assert!(coverage.sample_frames.len() <= MAX_SAMPLE_FRAMES);
    assert_eq!(report.frames().len(), coverage.rendered_frames.len());
    // Identical frames hash identically: no pixel-difference requirement.
    assert!(
        report
            .frames()
            .windows(2)
            .all(|w| w[0].sha256 == w[1].sha256)
    );
    assert!(report.changes().is_empty());
    // The report is serializable evidence.
    let json = serde_json::to_string(&report).unwrap();
    assert!(json.contains(&revision));
}

#[test]
fn coverage_has_boundaries_playhead_and_at_most_24_evenly_spaced_frames() {
    let t = timeline("r", 1000, &[(0, 300), (300, 700), (700, 1000)], &[]);
    let plan = plan_coverage(&t, 421, &ChangeSet::default());
    assert_eq!(plan.broadened, None);
    for boundary in [0usize, 299, 300, 699, 700, 999] {
        assert!(plan.boundary_frames.contains(&boundary));
        assert!(plan.rendered_frames.contains(&boundary));
        assert!(plan.inspect_frames.contains(&boundary));
    }
    assert!(plan.rendered_frames.contains(&421));
    assert_eq!(plan.sample_frames.len(), MAX_SAMPLE_FRAMES);
    assert_eq!(plan.sample_frames[0], 0);
    assert_eq!(*plan.sample_frames.last().unwrap(), 999);
    let gaps: Vec<usize> = plan.sample_frames.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps.iter().max().unwrap() - gaps.iter().min().unwrap() <= 1);
    assert!(plan.rendered_frames.len() <= MAX_RENDERED_FRAMES);
    assert!(plan.complete);
    // Short videos never ask for frames that do not exist.
    let short = timeline("r", 5, &[], &[]);
    let plan = plan_coverage(&short, 99, &ChangeSet::default());
    assert!(plan.rendered_frames.iter().all(|f| *f < 5));
    assert_eq!(evenly_spaced(5, 24), vec![0, 1, 2, 3, 4]);
    assert_eq!(evenly_spaced(0, 24), Vec::<usize>::new());
}

#[test]
fn scoped_coverage_requires_selected_samples_and_refuses_shrunken_candidates() {
    let scope = TaskScope {
        project_id: "p".into(),
        source_revision: "a".repeat(64),
        selection: ScopeSelection::FrameRange,
        compiled: Some(CompiledScope {
            preview: PreviewIdentity {
                project_id: "p".into(),
                open_session: "s".into(),
                source_revision: "a".repeat(64),
                worker_generation: 1,
            },
            fps: 30,
            total_frames: 1000,
            start_frame: 300,
            end_frame: 400,
            scenes: vec![ScopedScene {
                instance_id: "scene-1".into(),
                name: "Selected".into(),
                full_name: "Video::Selected".into(),
                start_frame: 250,
                end_frame: 450,
            }],
            boundary_frames: vec![249, 250, 299, 300, 399, 400, 449, 450],
            scene_context_truncated: false,
        }),
        scene_sources: vec![],
        scene_source_search_truncated: false,
        style_snapshot: None,
        canvas_selection: None,
    };
    scope.validate().unwrap();

    let candidate = timeline("x", 1000, &[(0, 1000)], &[]);
    let plan = plan_coverage_scoped(&candidate, 10, &ChangeSet::default(), Some(&scope));
    assert_eq!(plan.requested_scope, "Frames [300..400)");
    assert_eq!(plan.requested_interval, Some([300, 400]));
    assert!(plan.complete);
    for frame in &plan.requested_frames {
        assert!(
            plan.inspect_frames.contains(frame),
            "not inspected: {frame}"
        );
        assert!(
            plan.rendered_frames.contains(frame),
            "not rendered: {frame}"
        );
    }
    assert!(plan.requested_frames.contains(&300));
    assert!(plan.requested_frames.contains(&399));
    assert!(plan.requested_frames.contains(&249));
    assert!(plan.requested_frames.contains(&450));

    let shortened = timeline("x", 350, &[(0, 350)], &[]);
    let plan = plan_coverage_scoped(&shortened, 10, &ChangeSet::default(), Some(&scope));
    assert!(!plan.complete);
    assert!(plan.requested_frames.iter().all(|frame| *frame < 350));
}

fn change(path: &str, change: ChangeKind, kind: studio_project::revision::FileKind) -> ChangedPath {
    ChangedPath {
        path: path.into(),
        change,
        kind,
    }
}

#[test]
fn shared_source_deletions_and_truncated_changes_broaden_to_the_full_video_in_bounded_batches() {
    use studio_project::revision::FileKind::*;
    let t = timeline("r", 1000, &[(0, 500), (500, 1000)], &[]);
    let set = |entries: Vec<ChangedPath>, truncated| ChangeSet {
        total: entries.len(),
        entries,
        truncated,
    };
    assert_eq!(
        broadening(&set(
            vec![change("src/lib.rs", ChangeKind::Modified, Rust)],
            false
        )),
        Some(BroadenReason::SharedSourceChanged)
    );
    assert_eq!(
        broadening(&set(
            vec![change("Cargo.toml", ChangeKind::Modified, Cargo)],
            false
        )),
        Some(BroadenReason::SharedSourceChanged)
    );
    assert_eq!(
        broadening(&set(
            vec![change("media/a.png", ChangeKind::Deleted, Media)],
            false
        )),
        Some(BroadenReason::ExistingFilesChanged)
    );
    assert_eq!(
        broadening(&set(
            vec![change("media/a.png", ChangeKind::Modified, Media)],
            false
        )),
        Some(BroadenReason::ExistingFilesChanged)
    );
    assert_eq!(
        broadening(&set(vec![], true)),
        Some(BroadenReason::ChangeListTruncated)
    );
    // Added media or instruction-only edits keep the targeted coverage.
    assert_eq!(
        broadening(&set(
            vec![change("media/n.png", ChangeKind::Added, Media)],
            false
        )),
        None
    );
    assert_eq!(
        broadening(&set(
            vec![change("AGENTS.md", ChangeKind::Modified, Instructions)],
            false
        )),
        None
    );
    let plan = plan_coverage(
        &t,
        10,
        &set(
            vec![change("src/lib.rs", ChangeKind::Modified, Rust)],
            false,
        ),
    );
    assert_eq!(plan.inspect_frames.len(), 1000);
    assert!(plan.complete);
    // Rendering stays bounded even when inspection broadens.
    assert!(plan.rendered_frames.len() <= MAX_RENDERED_FRAMES);

    let f = fixture();
    let (_c, _ctx, captured) = captured_with(&f, |draft| {
        fs::write(draft.join("src/lib.rs"), "// shared function changed").unwrap();
    });
    let revision = captured.candidate().revision().as_str().to_owned();
    let mut probe = Fake::new(&revision, 1000);
    let report = validate_candidate(
        &captured,
        BuildOutcome::Built(build_identity(&captured)),
        0,
        0,
        Some(&mut probe),
    );
    assert!(report.passed());
    let coverage = report.coverage().unwrap();
    assert_eq!(coverage.broadened, Some(BroadenReason::SharedSourceChanged));
    assert_eq!(coverage.inspected_frames, 1000);
    assert_eq!(coverage.inspection_batches, 4);
    assert_eq!(probe.inspect_batches, vec![256, 256, 256, 232]);
    let mut seen = probe.inspected.clone();
    seen.sort_unstable();
    assert_eq!(seen, (0..1000).collect::<Vec<_>>());
}

#[test]
fn videos_longer_than_the_broadened_bound_report_partial_coverage_visibly() {
    use studio_project::revision::FileKind::Rust;
    let t = timeline("r", MAX_BROADENED_FRAMES + 5000, &[], &[]);
    let changes = ChangeSet {
        total: 1,
        entries: vec![change("src/lib.rs", ChangeKind::Modified, Rust)],
        truncated: false,
    };
    let plan = plan_coverage(&t, 0, &changes);
    assert!(!plan.complete);
    assert!(plan.inspect_frames.len() <= MAX_BROADENED_FRAMES + 3);
    assert!(plan.inspect_frames.contains(&0));
    assert!(plan.inspect_frames.contains(&(MAX_BROADENED_FRAMES + 4999)));
}

#[test]
fn compile_failure_routes_to_one_repair_then_retains_with_bounded_context() {
    let f = fixture();
    let (mut controller, context, captured) = captured_with(&f, |draft| {
        fs::write(draft.join("src/lib.rs"), "fn broken(").unwrap();
        for i in 0..300 {
            fs::write(draft.join(format!("media/extra-{i:03}.txt")), "x").unwrap();
        }
    });
    let mut output = String::from("HEAD-MARKER ");
    output.push_str(&"error[E0001]: expected `)`\n".repeat(200_000));
    output.push_str("TAIL-MARKER");
    let failed = |used| {
        validate_candidate(
            &captured,
            BuildOutcome::Failed {
                kind: FailureKind::Source,
                stage: ValidationStage::Compile,
                output: output.clone(),
                build: Some(build_identity(&captured)),
            },
            0,
            used,
            None,
        )
    };
    let report = failed(0);
    assert!(!report.passed());
    let ctx = report.failure().unwrap();
    assert_eq!(ctx.stage, ValidationStage::Compile);
    assert!(ctx.output_tail.len() <= MAX_COMPILER_OUTPUT_BYTES);
    assert!(ctx.output_tail.ends_with("TAIL-MARKER"));
    assert!(!ctx.output_tail.contains("HEAD-MARKER"));
    assert!(ctx.changed_paths.len() <= MAX_FAILURE_PATHS);
    assert_eq!(ctx.candidate, captured.candidate().revision().as_str());
    assert_eq!(ctx.source_base, context.source_base.revision().as_str());
    assert_eq!(ctx.build_key.as_deref(), Some("k".repeat(64).as_str()));
    assert_eq!(ctx.repair_count, 0);
    assert!(report.changes().truncated || report.changes().total > MAX_FAILURE_PATHS);
    let prompt = ctx.repair_prompt();
    assert!(prompt.len() <= MAX_REPAIR_PROMPT_BYTES);
    assert!(prompt.contains("TAIL-MARKER") || prompt.contains("expected `)`"));

    // Real Stage 1 budget: first failure repairs, the second exhausts the single repair.
    let task_budget = controller.agent_task().unwrap().repair().clone();
    match next_step(&report, &task_budget) {
        NextStep::Repair { attempt: 1, .. } => {}
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        controller
            .agent_validation_failed(&context.identity, "compile")
            .unwrap(),
        RepairDecision::Repair { attempt: 1 }
    ));
    controller.agent_begin_repair(&context.identity).unwrap();
    let spent = controller.agent_task().unwrap().repair().clone();
    assert_eq!(spent.used(), 1);
    let second = failed(spent.used());
    assert_eq!(second.failure().unwrap().repair_count, 1);
    match next_step(&second, &spent) {
        NextStep::Retain { reason, .. } => assert!(reason.contains("after the automatic repair")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn environment_failures_never_spend_the_repair_and_a_missing_probe_never_passes() {
    let f = fixture();
    let (controller, _ctx, captured) = captured_with(&f, |_| {});
    let budget = controller.agent_task().unwrap().repair().clone();
    let cancelled = validate_candidate(
        &captured,
        BuildOutcome::Failed {
            kind: FailureKind::Environment,
            stage: ValidationStage::Compile,
            output: "Build cancelled".into(),
            build: None,
        },
        0,
        0,
        None,
    );
    assert!(matches!(
        next_step(&cancelled, &budget),
        NextStep::Retain { .. }
    ));
    // A cached compiler success without a worker probe is not acceptance.
    let unprobed = validate_candidate(
        &captured,
        BuildOutcome::Built(build_identity(&captured)),
        0,
        0,
        None,
    );
    assert!(!unprobed.passed());
    assert_eq!(unprobed.failure().unwrap().kind, FailureKind::Environment);
    assert!(matches!(
        next_step(&unprobed, &budget),
        NextStep::Retain { .. }
    ));
}

fn run(f: &Fixture, probe: &mut Fake, edits: impl FnOnce(&Path)) -> (ValidationReport, Controller) {
    let (controller, _ctx, captured) = captured_with(f, edits);
    let revision = captured.candidate().revision().as_str().to_owned();
    // The worker reports the identity of the revision it was asked to build.
    probe.revision = revision.clone();
    probe.timeline.envelope = envelope(&revision);
    if let Ok(audio) = &mut probe.audio {
        audio.descriptor.envelope = probe.timeline.envelope.clone();
    }
    let report = validate_candidate(
        &captured,
        BuildOutcome::Built(build_identity(&captured)),
        0,
        0,
        Some(probe),
    );
    (report, controller)
}

#[test]
fn inspection_errors_truncation_and_unsupported_shaders() {
    let f = fixture();
    // Critical error => failure routed to repair.
    let mut probe = Fake::new("x", 60);
    probe.diagnostics = vec![PreviewDiagnostic {
        frame: 12,
        severity: DiagnosticSeverity::Error,
        key: "missing_asset".into(),
        message: "media/absent.jpg".into(),
    }];
    let (report, controller) = run(&f, &mut probe, |_| {});
    assert!(!report.passed());
    let ctx = report.failure().unwrap();
    assert_eq!(ctx.stage, ValidationStage::Inspection);
    assert_eq!(ctx.kind, FailureKind::Source);
    assert!(ctx.summary.contains("absent.jpg"));
    assert!(
        probe.rendered.is_empty(),
        "no rendering after a critical error"
    );
    assert!(matches!(
        next_step(&report, controller.agent_task().unwrap().repair()),
        NextStep::Repair { .. }
    ));
    drop(controller);

    // Truncated inspection cannot rule out critical findings.
    let f2 = fixture();
    let mut probe = Fake::new("x", 60);
    probe.truncated = true;
    let (report, _c) = run(&f2, &mut probe, |_| {});
    assert!(!report.passed());
    assert_eq!(report.failure().unwrap().stage, ValidationStage::Inspection);

    // Unsupported shader: a visible warning, still a pass.
    let f3 = fixture();
    let mut probe = Fake::new("x", 60);
    probe.diagnostics = vec![PreviewDiagnostic {
        frame: 3,
        severity: DiagnosticSeverity::Warning,
        key: "shader_unsupported".into(),
        message: "custom shader falls back to CPU preview".into(),
    }];
    let (report, _c) = run(&f3, &mut probe, |_| {});
    assert!(report.passed());
    assert!(
        report
            .warnings()
            .iter()
            .any(|w| w.contains("Unsupported shader"))
    );
    assert!(report.diagnostics_total() >= 1);
}

#[test]
fn render_failures_timeline_problems_and_wrong_revision_fail_closed() {
    let f = fixture();
    let mut probe = Fake::new("x", 60);
    probe.render_error_at = Some(59);
    let (report, _c) = run(&f, &mut probe, |_| {});
    let ctx = report.failure().unwrap();
    assert_eq!(ctx.stage, ValidationStage::Render);
    assert!(ctx.summary.contains("59"));
    assert!(report.frames().iter().all(|a| a.frame != 59));

    let f = fixture();
    let mut probe = Fake::new("x", 0);
    let (report, _c) = run(&f, &mut probe, |_| {});
    assert_eq!(report.failure().unwrap().stage, ValidationStage::Timeline);
    assert_eq!(report.failure().unwrap().kind, FailureKind::Source);

    let f = fixture();
    let mut probe = Fake::new("x", 60);
    probe.timeline_error = Some(ProbeError::environment("worker crashed on startup"));
    let (report, _c) = run(&f, &mut probe, |_| {});
    assert_eq!(report.failure().unwrap().kind, FailureKind::Environment);

    // A timeline for another revision is never accepted for this candidate.
    let f = fixture();
    let (_c, _ctx, captured) = captured_with(&f, |_| {});
    let mut probe = Fake::new("someone-elses-revision", 60);
    let report = validate_candidate(
        &captured,
        BuildOutcome::Built(build_identity(&captured)),
        0,
        0,
        Some(&mut probe),
    );
    assert!(!report.passed());
    assert!(
        report
            .failure()
            .unwrap()
            .summary
            .contains("different revision")
    );
}

/// Prepared audio whose PCM is 0.5 inside `active` sample ranges and silent elsewhere,
/// scanned by the real streaming scan against the timeline's windows.
fn active_audio(
    t: &PreviewTimelineResponse,
    active: Vec<(u64, u64)>,
    mutate: impl FnOnce(&mut AudioProbe),
) -> AudioProbe {
    let mut probe = silent_audio(t, &t.envelope.identity.source_revision);
    let total = probe.descriptor.sample_count;
    let mut bytes = Vec::with_capacity(total as usize * 8);
    for frame in 0..total {
        let on = active.iter().any(|&(s, e)| frame >= s && frame < e);
        let v = if on { 0.5f32 } else { 0. };
        bytes.extend_from_slice(&v.to_le_bytes());
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    probe.stats = scan_pcm(
        bytes.as_slice(),
        48_000,
        &placement_windows(t, 48_000),
        &|| false,
    )
    .unwrap();
    mutate(&mut probe);
    probe
}

fn audio_failure(
    mutate: impl FnOnce(&mut AudioProbe),
    tracks: &[(f64, f64)],
    active: Vec<(u64, u64)>,
) -> Vec<String> {
    let t = timeline("r", 90, &[], tracks);
    let probe = active_audio(&t, active, mutate);
    let (_report, errors) = check_audio(&t, &probe, "r");
    errors.into_iter().map(|(k, _)| k).collect()
}

#[test]
fn audio_checks_cover_revision_geometry_duration_finiteness_and_placement() {
    let ok_tracks = [(0.5, 2.0)];
    let inside = vec![(24_000, 96_000)];
    assert!(audio_failure(|_| {}, &ok_tracks, inside.clone()).is_empty());
    assert_eq!(
        audio_failure(
            |a| a.descriptor.envelope.identity.source_revision = "other".into(),
            &ok_tracks,
            inside.clone()
        ),
        ["audio_revision"]
    );
    assert!(
        audio_failure(
            |a| a.descriptor.sample_rate = 7000,
            &ok_tracks,
            inside.clone()
        )
        .contains(&"audio_geometry".to_owned())
    );
    assert!(
        audio_failure(|a| a.descriptor.channels = 1, &ok_tracks, inside.clone())
            .contains(&"audio_geometry".to_owned())
    );
    assert!(
        audio_failure(
            |a| {
                a.descriptor.sample_count -= 480;
                a.descriptor.byte_count -= 480 * 8;
                a.stats.sample_frames -= 480;
            },
            &ok_tracks,
            inside.clone()
        )
        .contains(&"audio_duration".to_owned())
    );
    assert!(
        audio_failure(|a| a.stats.non_finite = 3, &ok_tracks, inside.clone())
            .contains(&"audio_finite".to_owned())
    );
    assert!(
        audio_failure(|a| a.stats.sample_frames -= 8, &ok_tracks, inside.clone())
            .contains(&"audio_pcm_length".to_owned())
    );
    assert!(
        audio_failure(|a| a.descriptor.silent = true, &ok_tracks, inside.clone())
            .contains(&"audio_silent_flag".to_owned())
    );
    // Audible content far outside the declared window is a placement failure...
    assert!(
        audio_failure(|_| {}, &ok_tracks, vec![(120_000, 140_000)])
            .contains(&"audio_track_placement".to_owned())
    );
    // ...and a track starting beyond the video is out of bounds.
    assert!(
        audio_failure(|_| {}, &[(30.0, 31.0)], vec![]).contains(&"audio_track_bounds".to_owned())
    );
    // A fragmented activity map never hides out-of-window audio: the streaming scan
    // already recorded it.
    assert!(
        audio_failure(
            |a| a.stats.activity_truncated = true,
            &ok_tracks,
            vec![(120_000, 140_000)]
        )
        .contains(&"audio_track_placement".to_owned())
    );
}

#[test]
fn clipping_and_silence_are_visible_diagnostics_not_failures() {
    let f = fixture();
    let mut probe = Fake::new("x", 90);
    let t = timeline("x", 90, &[], &[(0.0, 3.0)]);
    probe.audio = Ok(AudioProbe {
        stats: PcmStats {
            peak: 1.2,
            clipped: 44,
            active: vec![(0, 144_000)],
            ..silent_audio(&t, "x").stats
        },
        ..silent_audio(&t, "x")
    });
    probe.timeline = t;
    let (report, _c) = run(&f, &mut probe, |_| {});
    assert!(report.passed(), "{:?}", report.failure());
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.key == "audio_clipping")
    );
    assert!(report.warnings().iter().any(|w| w.contains("full scale")));

    let f = fixture();
    let mut probe = Fake::new("x", 90);
    let t = timeline("x", 90, &[], &[(0.0, 3.0)]);
    probe.audio = Ok(silent_audio(&t, "x"));
    probe.timeline = t;
    let (report, _c) = run(&f, &mut probe, |_| {});
    assert!(report.passed());
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.key == "audio_silent_mix")
    );
    assert!(!report.audio().unwrap().tracks[0].audible);
}

#[test]
fn audio_only_edit_with_unchanged_video_passes_and_checks_the_mix() {
    let f = fixture();
    let t = timeline("x", 90, &[(0, 90)], &[(0.0, 3.0)]);
    let mut probe = Fake::new("x", 90).with_timeline(t.clone());
    probe.audio = Ok(active_audio(&t, vec![(0, 144_000)], |_| {}));
    probe.timeline = t;
    let (report, _c) = run(&f, &mut probe, |draft| {
        fs::write(draft.join("media/cue.wav"), b"RIFF new audio").unwrap();
    });
    assert!(report.passed(), "{:?}", report.failure());
    let audio = report.audio().unwrap();
    assert!(audio.checks_passed.contains(&"track_placement".to_owned()));
    assert!(audio.tracks[0].audible);
    assert!(
        report
            .frames()
            .windows(2)
            .all(|w| w[0].sha256 == w[1].sha256)
    );
}

#[test]
fn scan_pcm_reports_activity_clipping_and_non_finite_samples() {
    let rate = 48_000u32;
    let mut bytes = Vec::new();
    let mut push = |l: f32, r: f32, n: usize| {
        for _ in 0..n {
            bytes.extend_from_slice(&l.to_le_bytes());
            bytes.extend_from_slice(&r.to_le_bytes());
        }
    };
    push(0., 0., 4800); // 100 ms silence
    push(0.5, -0.25, 4800); // 100 ms tone
    push(1.0, 0., 3); // clipped
    push(f32::NAN, 0., 2);
    push(0., 0., 4795);
    let stats = scan_pcm(bytes.as_slice(), rate, &[], &|| false).unwrap();
    assert_eq!(stats.sample_frames, 4800 + 4800 + 3 + 2 + 4795);
    assert_eq!(stats.non_finite, 2);
    assert_eq!(stats.clipped, 3);
    assert!((stats.peak - 1.0).abs() < f32::EPSILON);
    assert_eq!(stats.active, vec![(4800, 9600 + 480)]);
    assert!(!stats.activity_truncated);
    assert!(scan_pcm(bytes.as_slice(), rate, &[], &|| true).is_err());
}

#[test]
fn broadening_needs_one_existing_non_instruction_file_to_change_and_unknown_kinds_widen() {
    use studio_project::revision::FileKind::{Instructions, Media, Other};
    let broaden = |entries: Vec<ChangedPath>| {
        broadening(&ChangeSet {
            total: entries.len(),
            entries,
            truncated: false,
        })
    };
    // A modified instruction file plus an *added* asset: no existing scene input changed.
    assert_eq!(
        broaden(vec![
            change("AGENTS.md", ChangeKind::Modified, Instructions),
            change("media/new.png", ChangeKind::Added, Media),
        ]),
        None
    );
    assert_eq!(
        broaden(vec![change(
            "AGENTS.md",
            ChangeKind::Modified,
            Instructions
        )]),
        None
    );
    for kind in [ChangeKind::Modified, ChangeKind::Deleted] {
        assert_eq!(
            broaden(vec![change("media/a.png", kind, Media)]),
            Some(BroadenReason::ExistingFilesChanged)
        );
    }
    assert_eq!(
        broaden(vec![change("fonts/x.ttf", ChangeKind::Added, Other)]),
        Some(BroadenReason::OtherFilesChanged)
    );
}

#[test]
fn targeted_coverage_widens_when_scenes_cannot_establish_it() {
    let widened = |t: PreviewTimelineResponse| {
        let plan = plan_coverage(&t, 10, &ChangeSet::default());
        (plan.broadened, plan.inspect_frames.len(), plan.complete)
    };
    // No scenes at all, or scenes with a hole: inspect every frame instead of guessing.
    assert_eq!(
        widened(timeline("r", 1000, &[], &[])),
        (Some(BroadenReason::ScenesUnavailable), 1000, true)
    );
    assert_eq!(
        widened(timeline("r", 1000, &[(0, 400), (500, 1000)], &[])),
        (Some(BroadenReason::ScenesUnavailable), 1000, true)
    );
    assert_eq!(
        widened(timeline("r", 1000, &[(0, 400)], &[])),
        (Some(BroadenReason::ScenesUnavailable), 1000, true)
    );
    // Overlapping scenes that still cover the whole video keep targeted coverage.
    let (broadened, count, _) = widened(timeline("r", 1000, &[(0, 600), (400, 1000)], &[]));
    assert_eq!(broadened, None);
    assert!(count < 1000);
}

#[test]
fn a_declared_shader_capability_gap_is_a_visible_warning_recorded_in_the_report() {
    let f = fixture();
    let mut probe = Fake::new("x", 60);
    probe.gaps = vec!["shader_preview".into()];
    let (report, _c) = run(&f, &mut probe, |_| {});
    assert!(report.passed(), "{:?}", report.failure());
    assert_eq!(report.capability_gaps(), ["shader_preview".to_owned()]);
    assert!(
        report
            .warnings()
            .iter()
            .any(|w| w.contains("Unsupported shaders") && w.contains("shader_preview"))
    );
    let diagnostic = report
        .diagnostics()
        .iter()
        .find(|d| d.key == "capability_gap")
        .expect("gap is a diagnostic");
    assert_eq!(diagnostic.severity, DiagnosticSeverity::Warning);
    assert_eq!(diagnostic.stage, ValidationStage::Worker);

    // Other gaps are recorded but are not shader warnings.
    let f = fixture();
    let mut probe = Fake::new("x", 60);
    probe.gaps = vec!["gpu_backend".into()];
    let (report, _c) = run(&f, &mut probe, |_| {});
    assert!(report.passed());
    assert_eq!(report.capability_gaps(), ["gpu_backend".to_owned()]);
    assert!(report.warnings().iter().all(|w| !w.contains("shader")));
}

#[test]
fn stop_between_worker_requests_is_an_environment_failure_that_spends_no_repair() {
    for (cancel_after, stage) in [
        (0usize, ValidationStage::Inspection),
        (2, ValidationStage::Render),
    ] {
        let f = fixture();
        let mut probe = Fake::new("x", 60);
        probe.cancel_after_renders = Some(cancel_after);
        let (report, controller) = run(&f, &mut probe, |_| {});
        let failure = report.failure().expect("cancelled");
        assert_eq!(failure.kind, FailureKind::Environment);
        assert_eq!(failure.stage, stage);
        assert!(failure.summary.contains("cancelled"));
        assert!(probe.rendered.len() <= cancel_after.max(1));
        assert!(matches!(
            next_step(&report, controller.agent_task().unwrap().repair()),
            NextStep::Retain { .. }
        ));
    }
}

/// PCM that alternates 10 ms of signal and silence (more than `MAX_ACTIVITY_RANGES`
/// ranges), scanned by the real streaming scan against the timeline's windows.
fn fragmented_audio(t: &PreviewTimelineResponse) -> AudioProbe {
    let mut probe = silent_audio(t, "x");
    let total = probe.descriptor.sample_count;
    let mut bytes = Vec::with_capacity(total as usize * 8);
    for frame in 0..total {
        let v = if (frame / 480) % 2 == 0 { 0.5f32 } else { 0. };
        bytes.extend_from_slice(&v.to_le_bytes());
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    probe.stats = scan_pcm(
        bytes.as_slice(),
        48_000,
        &placement_windows(t, 48_000),
        &|| false,
    )
    .unwrap();
    assert!(
        probe.stats.activity_truncated,
        "the activity map overflowed"
    );
    probe
}

#[test]
fn fragmented_pcm_cannot_hide_audio_outside_the_track_windows() {
    // 83 s of video, audible content in every other 10 ms bucket; the single track only
    // covers the first 40 s. The bounded activity map overflows long before the
    // out-of-window content, yet the streaming scan has recorded it: not Passed, and a
    // source failure the agent may repair.
    let f = fixture();
    let t = timeline("x", 2500, &[(0, 2500)], &[(0.0, 40.0)]);
    let mut probe = Fake::new("x", 2500).with_timeline(t.clone());
    probe.audio = Ok(fragmented_audio(&t));
    probe.timeline = t;
    let (report, controller) = run(&f, &mut probe, |_| {});
    assert!(!report.passed());
    let failure = report.failure().unwrap();
    assert_eq!(failure.stage, ValidationStage::Audio);
    assert_eq!(failure.kind, FailureKind::Source);
    assert!(report.audio().unwrap().placement_verified);
    assert!(!report.audio().unwrap().audibility_complete);
    assert!(
        report
            .errors()
            .iter()
            .any(|d| d.key == "audio_track_placement")
    );
    assert!(matches!(
        next_step(&report, controller.agent_task().unwrap().repair()),
        NextStep::Repair { .. }
    ));

    // The same fragmented PCM wholly inside a track window is verified and passes;
    // incomplete audibility is a visible limitation, not an acceptance input.
    let f = fixture();
    let t = timeline("x", 2500, &[(0, 2500)], &[(0.0, 90.0)]);
    let mut probe = Fake::new("x", 2500).with_timeline(t.clone());
    probe.audio = Ok(fragmented_audio(&t));
    probe.timeline = t;
    let (report, _c) = run(&f, &mut probe, |_| {});
    assert!(report.passed(), "{:?}", report.failure());
    let audio = report.audio().unwrap();
    assert!(audio.placement_verified);
    assert!(!audio.audibility_complete);
    assert!(audio.checks_passed.contains(&"track_placement".to_owned()));
}

#[test]
fn unverifiable_track_placement_is_never_passed() {
    // A scan that was not given the compiled windows cannot vouch for placement, even
    // with audible content that happens to lie inside them.
    let f = fixture();
    let t = timeline("x", 90, &[(0, 90)], &[(0.5, 2.0)]);
    let mut probe = Fake::new("x", 90).with_timeline(t.clone());
    probe.audio = Ok(active_audio(&t, vec![(24_000, 96_000)], |a| {
        a.stats.checked_windows.clear();
    }));
    probe.timeline = t;
    let (report, controller) = run(&f, &mut probe, |_| {});
    assert!(!report.passed());
    let failure = report.failure().unwrap();
    assert_eq!(failure.stage, ValidationStage::Audio);
    assert_eq!(failure.kind, FailureKind::Environment);
    let audio = report.audio().unwrap();
    assert!(!audio.placement_verified);
    assert!(!audio.checks_passed.contains(&"track_placement".to_owned()));
    assert!(
        report
            .errors()
            .iter()
            .any(|d| d.key == "audio_placement_unverified")
    );
    assert!(matches!(
        next_step(&report, controller.agent_task().unwrap().repair()),
        NextStep::Retain { .. }
    ));
}

fn warnings_then(errors: Vec<PreviewDiagnostic>) -> Vec<PreviewDiagnostic> {
    let mut diagnostics: Vec<PreviewDiagnostic> = (0..200)
        .map(|i| PreviewDiagnostic {
            frame: i,
            severity: DiagnosticSeverity::Warning,
            key: format!("noise_{i}"),
            message: "harmless".into(),
        })
        .collect();
    diagnostics.extend(errors);
    diagnostics
}

#[test]
fn a_critical_inspection_fact_survives_the_bounded_display_buffer() {
    // Broadened (shared source change) => four inspection batches. Batch 0 fills the
    // 128-entry display buffer with warnings; an interior Error arrives in batch 3.
    let edit = |draft: &Path| fs::write(draft.join("src/lib.rs"), "// shared").unwrap();
    let f = fixture();
    let mut probe = Fake::new("x", 1000);
    probe.batch_diagnostics = vec![
        (0, warnings_then(vec![])),
        (
            3,
            vec![PreviewDiagnostic {
                frame: 777,
                severity: DiagnosticSeverity::Error,
                key: "missing_asset".into(),
                message: "media/late.jpg".into(),
            }],
        ),
    ];
    let (report, controller) = run(&f, &mut probe, edit);
    assert_eq!(probe.inspect_batches.len(), 4);
    assert!(!report.passed());
    let failure = report.failure().unwrap();
    assert_eq!(failure.stage, ValidationStage::Inspection);
    assert_eq!(failure.kind, FailureKind::Source);
    assert!(failure.summary.contains("late.jpg"));
    assert!(failure.diagnostics.iter().any(|d| d.frame == Some(777)));
    // The display buffer is full of warnings and does not contain the error...
    assert_eq!(report.diagnostics().len(), MAX_REPORT_DIAGNOSTICS);
    assert!(report.diagnostics_total() > MAX_REPORT_DIAGNOSTICS);
    assert!(
        report
            .diagnostics()
            .iter()
            .all(|d| d.severity != DiagnosticSeverity::Error)
    );
    // ...but the sticky error facts do.
    assert_eq!(report.errors_total(), 1);
    assert_eq!(report.errors()[0].frame, Some(777));
    assert!(
        probe.rendered.is_empty(),
        "no rendering after a critical error"
    );
    assert!(matches!(
        next_step(&report, controller.agent_task().unwrap().repair()),
        NextStep::Repair { .. }
    ));

    // A truncated later batch after the buffer filled up is just as fatal.
    let f = fixture();
    let mut probe = Fake::new("x", 1000);
    probe.batch_diagnostics = vec![(0, warnings_then(vec![]))];
    probe.truncate_batch = Some(3);
    let (report, _c) = run(&f, &mut probe, edit);
    assert!(!report.passed());
    let failure = report.failure().unwrap();
    assert_eq!(failure.stage, ValidationStage::Inspection);
    assert!(failure.summary.contains("truncated"));
    assert_eq!(report.errors_total(), 1);
    assert_eq!(report.diagnostics().len(), MAX_REPORT_DIAGNOSTICS);
}

#[test]
fn incomplete_required_coverage_is_retained_not_accepted() {
    // 40 scenes => 80 required boundary frames, more than the 64-frame render cap.
    let scenes: Vec<(usize, usize)> = (0..40).map(|i| (i * 25, (i + 1) * 25)).collect();
    let f = fixture();
    let t = timeline("x", 1000, &scenes, &[]);
    let mut probe = Fake::new("x", 1000).with_timeline(t);
    let (report, controller) = run(&f, &mut probe, |_| {});
    assert!(!report.passed());
    let failure = report.failure().unwrap();
    assert_eq!(failure.stage, ValidationStage::Coverage);
    assert_eq!(failure.kind, FailureKind::Environment);
    assert!(failure.summary.contains("bounded validation limitation"));
    assert!(!report.coverage().unwrap().complete);
    assert!(report.frames().len() <= MAX_RENDERED_FRAMES);
    match next_step(&report, controller.agent_task().unwrap().repair()) {
        NextStep::Retain { reason, context } => {
            assert!(reason.contains("bounded validation limitation"), "{reason}");
            assert!(context.is_some());
        }
        other => panic!("{other:?}"),
    }
    // Optional sampled frames beyond the cap do not count as missing coverage: 20 scenes
    // need 40 required frames and still complete.
    let scenes: Vec<(usize, usize)> = (0..20).map(|i| (i * 50, (i + 1) * 50)).collect();
    let plan = plan_coverage(
        &timeline("x", 1000, &scenes, &[]),
        10,
        &ChangeSet::default(),
    );
    assert!(plan.complete);

    // A broadened video beyond the inspection bound: the defect sits in a frame the
    // strided inspection never visits, so the only safe verdict is "not accepted".
    let f = fixture();
    let total = MAX_BROADENED_FRAMES + 5000;
    let mut probe = Fake::new("x", total);
    let (report, controller) = run(&f, &mut probe, |draft| {
        fs::write(draft.join("src/lib.rs"), "// shared").unwrap();
    });
    let omitted = (0..total)
        .find(|frame| !probe.inspected.contains(frame))
        .expect("a strided inspection omits frames");
    assert!(!probe.inspected.contains(&omitted));
    assert!(!report.passed());
    let failure = report.failure().unwrap();
    assert_eq!(failure.stage, ValidationStage::Coverage);
    assert!(!report.coverage().unwrap().complete);
    assert!(matches!(
        next_step(&report, controller.agent_task().unwrap().repair()),
        NextStep::Retain { .. }
    ));
}

#[test]
fn a_build_that_does_not_name_the_candidate_proves_nothing() {
    let f = fixture();
    let (_c, _ctx, captured) = captured_with(&f, |_| {});
    let revision = captured.candidate().revision().as_str().to_owned();
    for mutate in [
        (|b: &mut BuildIdentity| b.source_revision = "e".repeat(64)) as fn(&mut BuildIdentity),
        |b| b.backend = "gpu".into(),
    ] {
        let mut probe = Fake::new(&revision, 300);
        let mut identity = build_identity(&captured);
        mutate(&mut identity);
        let report = validate_candidate(
            &captured,
            BuildOutcome::Built(identity),
            0,
            0,
            Some(&mut probe),
        );
        assert!(!report.passed());
        let failure = report.failure().unwrap();
        assert_eq!(failure.stage, ValidationStage::Compile);
        assert_eq!(failure.kind, FailureKind::Environment);
        assert!(probe.inspected.is_empty() && probe.rendered.is_empty());
    }
}

#[test]
fn frames_must_carry_the_identity_of_the_candidate_timeline() {
    let f = fixture();
    let mut probe = Fake::new("x", 300);
    probe.frame_revision = Some("another-revision".into());
    let (report, _c) = run(&f, &mut probe, |_| {});
    assert!(!report.passed());
    let failure = report.failure().unwrap();
    assert_eq!(failure.stage, ValidationStage::Render);
    assert_eq!(failure.kind, FailureKind::Environment);
    assert!(report.errors().iter().any(|d| d.key == "render_identity"));
    assert!(report.frames().is_empty());
}

#[test]
fn a_report_is_never_an_acceptance_proof_when_read_back() {
    // The report can be written for display, but there is no way to read one back as
    // an acceptance proof: `ValidationReport` and `CapturedCandidate` are not
    // `Deserialize`.
    trait Not {
        const DESERIALIZE: bool = false;
    }
    impl<T> Not for Probe<T> {}
    struct Probe<T>(std::marker::PhantomData<T>);
    impl<T: serde::de::DeserializeOwned> Probe<T> {
        const DESERIALIZE: bool = true;
    }
    const { assert!(!<Probe<ValidationReport>>::DESERIALIZE) };
    const { assert!(!<Probe<CapturedCandidate>>::DESERIALIZE) };
    // Sanity: the detector does see ordinary Deserialize types.
    const { assert!(<Probe<Coverage>>::DESERIALIZE) };
}

#[test]
fn the_controller_captures_only_with_a_current_ticket_and_routes_validation_reports() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let context = start(&mut controller);
    let id = context.identity.clone();
    fs::write(context.draft.join("src/lib.rs"), "// first").unwrap();
    let first = ticket(&mut controller, &context);
    // A newer quiescence attempt retires the first ticket: it can no longer capture.
    let second = ticket(&mut controller, &context);
    assert!(controller.agent_capture_candidate(first).is_err());
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Quiescing
    );
    assert!(controller.agent_task().unwrap().candidate().is_none());

    let captured = controller.agent_capture_candidate(second).unwrap();
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Validating
    );
    assert_eq!(
        controller.agent_task().unwrap().candidate(),
        Some(captured.candidate())
    );

    let revision = captured.candidate().revision().as_str().to_owned();
    let validate = |probe: &mut Fake, repair_used| {
        probe.revision = revision.clone();
        probe.timeline.envelope = envelope(&revision);
        if let Ok(audio) = &mut probe.audio {
            audio.descriptor.envelope = probe.timeline.envelope.clone();
        }
        validate_candidate(
            &captured,
            BuildOutcome::Built(build_identity(&captured)),
            0,
            repair_used,
            Some(probe),
        )
    };

    // A report for another candidate (or task) never moves this task.
    let (foreign_controller, _foreign_ctx, foreign) = captured_with(&fixture(), |_| {});
    let foreign_report = validate_candidate(
        &foreign,
        BuildOutcome::Built(build_identity(&foreign)),
        0,
        0,
        None,
    );
    assert!(
        controller
            .agent_apply_validation(&id, &foreign_report)
            .is_err()
    );
    drop(foreign_controller);
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::Validating
    );

    // A source failure spends the single automatic repair.
    let mut bad = Fake::new("x", 60);
    bad.diagnostics = vec![PreviewDiagnostic {
        frame: 1,
        severity: DiagnosticSeverity::Error,
        key: "missing_asset".into(),
        message: "media/absent.jpg".into(),
    }];
    let failed = validate(&mut bad, 0);
    match controller.agent_apply_validation(&id, &failed).unwrap() {
        NextStep::Repair { attempt: 1, .. } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::RepairNeeded
    );
}

#[test]
fn captured_candidate_retains_the_scope_frozen_on_the_task_context() {
    let f = fixture();
    let mut controller = Controller::open(&f.root, &f.paths).unwrap();
    let project = studio_project::open(&f.root).unwrap();
    let inventory = studio_project::revision::SourceInventory::scan(&f.root).unwrap();
    let scope = TaskScope::whole_project(
        String::from(project.manifest.project_id.clone()),
        inventory.revision.as_str(),
    );
    let context = controller
        .begin_agent_task_scoped("edit", scope.clone())
        .unwrap();
    controller
        .agent_writer_started(&context.identity, qualified())
        .unwrap();
    controller
        .agent_task_transition(&context.identity, TaskState::Editing)
        .unwrap();
    controller
        .agent_task_transition(&context.identity, TaskState::Quiescing)
        .unwrap();
    let ticket = ticket(&mut controller, &context);
    let captured = controller.agent_capture_candidate(ticket).unwrap();
    assert_eq!(captured.scope(), &scope);
}

#[test]
fn accepted_environment_and_exhausted_reports_end_in_the_right_states() {
    // Pass: the task becomes CandidateReady and the report is the evidence.
    let f = fixture();
    let (mut controller, context, captured) = captured_with(&f, |_| {});
    let revision = captured.candidate().revision().as_str().to_owned();
    let mut probe = Fake::new(&revision, 60);
    let report = validate_candidate(
        &captured,
        BuildOutcome::Built(build_identity(&captured)),
        0,
        0,
        Some(&mut probe),
    );
    assert!(report.passed());
    assert_eq!(
        controller
            .agent_apply_validation(&context.identity, &report)
            .unwrap(),
        NextStep::Accept
    );
    assert_eq!(
        controller.agent_task().unwrap().state(),
        TaskState::CandidateReady
    );
    // The report cannot be applied twice.
    assert!(
        controller
            .agent_apply_validation(&context.identity, &report)
            .is_err()
    );
    drop(controller);

    // Environment failure: terminal, draft retained, repair budget untouched.
    let f = fixture();
    let (mut controller, context, captured) = captured_with(&f, |_| {});
    let report = validate_candidate(
        &captured,
        BuildOutcome::Failed {
            kind: FailureKind::Environment,
            stage: ValidationStage::Worker,
            output: "incompatible preview bridge".into(),
            build: None,
        },
        0,
        0,
        None,
    );
    assert!(matches!(
        controller
            .agent_apply_validation(&context.identity, &report)
            .unwrap(),
        NextStep::Retain { .. }
    ));
    let task = controller.agent_task().unwrap();
    assert_eq!(task.state(), TaskState::Failed);
    assert_eq!(task.repair().used(), 0);
    assert!(matches!(
        controller.agent_draft_state().unwrap().unwrap(),
        studio_engine::DraftState::Retained { .. }
    ));
    drop(controller);

    // Source failure with the single repair already spent is terminal.
    let f = fixture();
    let (mut controller, context, captured) = captured_with(&f, |_| {});
    let source_failure = |used| {
        validate_candidate(
            &captured,
            BuildOutcome::Failed {
                kind: FailureKind::Source,
                stage: ValidationStage::Compile,
                output: "error[E0425]".into(),
                build: None,
            },
            0,
            used,
            None,
        )
    };
    assert!(matches!(
        controller
            .agent_apply_validation(&context.identity, &source_failure(0))
            .unwrap(),
        NextStep::Repair { attempt: 1, .. }
    ));
    assert_eq!(controller.agent_begin_repair(&context.identity).unwrap(), 1);
    let task = controller.agent_task().unwrap();
    assert_eq!(task.repair().remaining(), 0);
    // Second candidate in the repair epoch.
    controller
        .agent_writer_started(&context.identity, qualified())
        .unwrap();
    controller
        .agent_task_transition(&context.identity, TaskState::Quiescing)
        .unwrap();
    let t = ticket(&mut controller, &context);
    let second = controller.agent_capture_candidate(t).unwrap();
    let second_report = validate_candidate(
        &second,
        BuildOutcome::Failed {
            kind: FailureKind::Source,
            stage: ValidationStage::Compile,
            output: "error[E0425]".into(),
            build: None,
        },
        0,
        1,
        None,
    );
    assert!(matches!(
        controller
            .agent_apply_validation(&context.identity, &second_report)
            .unwrap(),
        NextStep::Retain { .. }
    ));
    assert_eq!(controller.agent_task().unwrap().state(), TaskState::Failed);
    assert_eq!(controller.agent_task().unwrap().repair().used(), 1);
}
