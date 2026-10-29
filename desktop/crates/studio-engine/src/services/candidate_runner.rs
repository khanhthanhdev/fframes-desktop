//! Runs the engine's acceptance validation for one captured candidate against a real
//! preview worker, and stages matching playback without publishing it.
//!
//! The candidate is restored from its immutable checkpoint objects into a private tree,
//! compiled through the shared [`BuildService`] (a cached or in-flight equal key is
//! joined, never recompiled), and probed with an owned preview worker that has its own
//! process scope: it never touches the displayed or thumbnail lanes. On success the
//! candidate's timeline, inspection, playhead frame and PCM are prepared into a
//! [`StagedPreview`] that nothing displays: the existing preview keeps playing, and on any
//! failure it simply remains. Installing a staged candidate is Stage 3's guarded
//! promotion; a shared build lease grants no install authority.
use super::{
    build_service::{
        BuildError, BuildKey, BuildService, CompileEnvironment, CompileRequest, Subscriber,
        SubscriberKind,
    },
    preview_coordinator::{
        SeekIntent, StagedPreview, account_prepared_audio, effective_scale, prepare_preview,
    },
    preview_worker_client::PreviewWorkerClient,
    worker_project::{PreviewLaunchError, launch_preview_worker_checked},
};
use crate::{
    OperationId, OperationTag, PreparedAudioSource, PreviewFrame,
    candidate_validation::{
        AudioProbe, BuildIdentity, BuildOutcome, CandidateError, CandidateProbe, CapturedCandidate,
        FailureKind, ProbeError, ValidationReport, ValidationStage, placement_windows,
        restore_revision, scan_pcm, validate_candidate,
    },
    preview_identity,
};
use fframes_studio_protocol::{InspectResponse, PreparedAudioDescriptor, PreviewTimelineResponse};
use std::{any::Any, fs::File, io::Read, path::PathBuf, sync::Arc};
use studio_bootstrap::ProcessTreeManager;
use studio_project::{
    checkpoint::Checkpoints,
    revision::{SLIDES_FILE, TIMING_FILE},
};
use studio_sdk::CompatibilityManifest;

/// The timing generation a candidate's worker plays its own `timeline.json` under; no
/// session reaches it, so a cached frame of this layout never matches a session retime.
const CANDIDATE_TIMING_GENERATION: u64 = u64::MAX;

pub struct CandidateRunConfig {
    pub service: BuildService,
    pub sdk: PathBuf,
    pub compatibility: CompatibilityManifest,
    pub builds: PathBuf,
}

/// Cancellation scopes of one run. Shutting `compiler` down detaches this run from the
/// shared compile; shutting `worker` down reaps its preview worker.
pub struct RunScopes {
    pub compiler: ProcessTreeManager,
    pub worker: ProcessTreeManager,
}

pub struct CandidateRun {
    pub report: ValidationReport,
    /// Present only for a passing report; not visible to playback until promoted.
    pub staged: Option<StagedPreview>,
}

pub fn build_identity(key: &BuildKey) -> BuildIdentity {
    BuildIdentity {
        key_digest: key.digest(),
        source_revision: key.source_revision.clone(),
        sdk_id: key.sdk_id.clone(),
        compatibility_digest: key.compatibility_digest.clone(),
        toolchain: key.toolchain.clone(),
        target_triple: key.target_triple.clone(),
        package: key.package.clone(),
        worker_target: key.worker_target.clone(),
        profile: key.profile.as_str().into(),
        backend: key.backend.clone(),
    }
}

fn failed(
    captured: &CapturedCandidate,
    kind: FailureKind,
    stage: ValidationStage,
    output: String,
    build: Option<BuildIdentity>,
    playhead: usize,
    repair_used: u32,
) -> CandidateRun {
    CandidateRun {
        report: validate_candidate(
            captured,
            BuildOutcome::Failed {
                kind,
                stage,
                output,
                build,
            },
            playhead,
            repair_used,
            None,
        ),
        staged: None,
    }
}

/// Positioned reads of the prepared PCM; never moves a shared cursor.
struct PositionedReader<'a> {
    file: &'a File,
    offset: u64,
}

impl Read for PositionedReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(unix)]
        let read = std::os::unix::fs::FileExt::read_at(self.file, buffer, self.offset)?;
        #[cfg(windows)]
        let read = std::os::windows::fs::FileExt::seek_read(self.file, buffer, self.offset)?;
        self.offset += read as u64;
        Ok(read)
    }
}

/// What a finished probe hands to staging.
pub struct ProbeParts {
    pub worker: Option<PreviewWorkerClient>,
    pub audio: Option<(PreparedAudioDescriptor, Arc<PreparedAudioSource>)>,
    /// The compiled timeline the probe validated; staging derives its scale from it.
    pub timeline: Option<PreviewTimelineResponse>,
}

/// [`CandidateProbe`] over an owned preview worker.
pub struct WorkerProbe<'a> {
    worker: Option<PreviewWorkerClient>,
    cancelled: &'a (dyn Fn() -> bool + Sync),
    service: &'a BuildService,
    audio: Option<(PreparedAudioDescriptor, Arc<PreparedAudioSource>)>,
    timeline: Option<PreviewTimelineResponse>,
}

impl<'a> WorkerProbe<'a> {
    pub fn new(
        worker: PreviewWorkerClient,
        cancelled: &'a (dyn Fn() -> bool + Sync),
        service: &'a BuildService,
    ) -> Self {
        Self {
            worker: Some(worker),
            cancelled,
            service,
            audio: None,
            timeline: None,
        }
    }

    fn client(&mut self) -> Result<&mut PreviewWorkerClient, ProbeError> {
        self.worker
            .as_mut()
            .ok_or_else(|| ProbeError::environment("preview worker was released"))
    }

    fn fail(&self, message: impl ToString) -> ProbeError {
        if (self.cancelled)() {
            ProbeError::environment("validation cancelled")
        } else {
            // A worker that crashes or answers badly for these bytes is the candidate's
            // problem: the agent's code may be what broke it.
            ProbeError::source(message.to_string())
        }
    }

    pub fn into_parts(self) -> ProbeParts {
        ProbeParts {
            worker: self.worker,
            audio: self.audio,
            timeline: self.timeline,
        }
    }
}

impl CandidateProbe for WorkerProbe<'_> {
    fn timeline(&mut self) -> Result<PreviewTimelineResponse, ProbeError> {
        let result = self.client()?.timeline();
        let timeline = result.map_err(|e| self.fail(e))?;
        self.timeline = Some(timeline.clone());
        Ok(timeline)
    }

    fn inspect(&mut self, frames: &[usize]) -> Result<InspectResponse, ProbeError> {
        let result = self.client()?.inspect(frames.to_vec());
        result.map_err(|e| self.fail(e))
    }

    fn render(&mut self, frame: usize) -> Result<PreviewFrame, ProbeError> {
        // The worker never renders above the preview cap and answers with the clamped
        // scale, which the client only accepts when it was the scale requested: ask for
        // the effective scale of this candidate's compiled dimensions.
        let scale = effective_scale(
            self.timeline
                .as_ref()
                .ok_or_else(|| ProbeError::environment("compiled timeline was not probed"))?,
            1.,
        );
        let result = self.client()?.frame(frame, 0, scale);
        result.map_err(|e| self.fail(e))
    }

    fn audio(&mut self) -> Result<AudioProbe, ProbeError> {
        let cancelled = self.cancelled;
        let descriptor = {
            let result = self.client()?.prepare_audio(48_000);
            result.map_err(|e| self.fail(e))?
        };
        let source = {
            let result = self.client()?.retain_audio_source(&descriptor, cancelled);
            result.map_err(|e| self.fail(e))?
        };
        // The PCM stays allocated for as long as this source (an open file) exists:
        // account it in the shared build budget now, released when the source drops.
        account_prepared_audio(self.service, &descriptor, &source)
            .map_err(ProbeError::environment)?;
        let windows = placement_windows(
            self.timeline
                .as_ref()
                .ok_or_else(|| ProbeError::environment("compiled timeline was not probed"))?,
            descriptor.sample_rate,
        );
        let stats = scan_pcm(
            PositionedReader {
                file: &source.file,
                offset: 0,
            },
            descriptor.sample_rate,
            &windows,
            cancelled,
        )
        .map_err(|e| self.fail(e))?;
        self.audio = Some((descriptor.clone(), source));
        Ok(AudioProbe { descriptor, stats })
    }

    fn capability_gaps(&self) -> Vec<String> {
        self.worker
            .as_ref()
            .map(PreviewWorkerClient::capability_gaps)
            .unwrap_or_default()
    }

    fn backend(&self) -> String {
        self.worker
            .as_ref()
            .map_or_else(String::new, |worker| worker.backend().to_owned())
    }

    fn cancelled(&self) -> bool {
        (self.cancelled)()
    }
}

/// Validate `captured`: restore, compile (shared), probe, report, and stage playback.
/// Blocking: run on a background thread.
pub fn run_candidate_validation(
    captured: &CapturedCandidate,
    checkpoints: &Checkpoints,
    config: &CandidateRunConfig,
    scopes: &RunScopes,
    playhead: usize,
    repair_used: u32,
) -> CandidateRun {
    let fail = |kind, stage, output: String, build| {
        scopes.worker.shutdown(std::time::Duration::ZERO);
        failed(captured, kind, stage, output, build, playhead, repair_used)
    };
    let restored = match restore_revision(checkpoints, captured.candidate().revision()) {
        Ok(restored) => Arc::new(restored),
        Err(CandidateError::InvalidDraft(message)) => {
            return fail(FailureKind::Source, ValidationStage::Capture, message, None);
        }
        Err(other) => {
            return fail(
                FailureKind::Environment,
                ValidationStage::Capture,
                other.to_string(),
                None,
            );
        }
    };
    // The tree handed to the compiler must be exactly the captured candidate.
    if restored.project.inventory.revision != *captured.candidate().revision() {
        return fail(
            FailureKind::Environment,
            ValidationStage::Capture,
            "the restored tree does not match the captured candidate revision".into(),
            None,
        );
    }
    let candidate_timing = match std::fs::read_to_string(restored.project.root.join(TIMING_FILE)) {
        Ok(json) => Some(json),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return fail(
                FailureKind::Source,
                ValidationStage::Capture,
                format!("{TIMING_FILE} could not be read: {error}"),
                None,
            );
        }
    };
    // The candidate's slides document is refused whole, like the session refuses it,
    // before anything is compiled.
    let candidate_slides = match std::fs::read_to_string(restored.project.root.join(SLIDES_FILE))
        .map_err(|error| error.to_string())
        .and_then(|json| crate::slides::SlidesDoc::parse(&json))
    {
        Ok(doc) => Some(doc),
        Err(_) if !restored.project.root.join(SLIDES_FILE).exists() => None,
        Err(reason) => {
            return fail(
                FailureKind::Source,
                ValidationStage::Capture,
                format!("{SLIDES_FILE} is invalid: {reason}"),
                None,
            );
        }
    };
    let slides_changed = captured
        .changes()
        .entries
        .iter()
        .any(|change| change.path.as_str() == SLIDES_FILE);
    let environment =
        match CompileEnvironment::resolve(&config.sdk, &config.compatibility, &config.builds) {
            Ok(environment) => environment,
            Err(message) => {
                return fail(
                    FailureKind::Environment,
                    ValidationStage::Compile,
                    message,
                    None,
                );
            }
        };
    let key = BuildKey::worker(&restored.project, &environment);
    let identity = build_identity(&key);
    let task = captured.identity();
    let request = CompileRequest {
        project: restored.project.clone(),
        environment,
        retained: Some(restored.clone() as Arc<dyn Any + Send + Sync>),
    };
    let subscriber = Subscriber::new(
        SubscriberKind::Validation,
        format!("task {} generation {}", task.task.0, task.generation),
    );
    let lease = match config
        .service
        .subscribe(key, request, subscriber)
        .and_then(|subscription| subscription.wait(&|| scopes.compiler.is_shutdown()))
    {
        Ok(lease) => lease,
        Err(BuildError::Failed(message)) => {
            return fail(
                FailureKind::Source,
                ValidationStage::Compile,
                message,
                Some(identity),
            );
        }
        Err(other) => {
            return fail(
                FailureKind::Environment,
                ValidationStage::Compile,
                other.to_string(),
                Some(identity),
            );
        }
    };
    // Candidate bytes are not the task base: the preview identity names the candidate.
    let tag = OperationTag {
        project: task.project.clone(),
        session: task.session.clone(),
        base_source: captured.candidate().revision().clone(),
        operation: OperationId(task.generation),
        generation: task.generation,
    };
    let worker = match launch_preview_worker_checked(
        lease.into_build(),
        preview_identity(&tag),
        &scopes.worker,
    ) {
        Ok(worker) => worker,
        Err(error) => {
            // A worker that is not a compatible CPU preview worker is the SDK's problem:
            // no repair turn can fix it. Anything else fails for these bytes unless the
            // run was stopped.
            let kind = if matches!(error, PreviewLaunchError::Incompatible(_))
                || scopes.worker.is_shutdown()
                || scopes.compiler.is_shutdown()
            {
                FailureKind::Environment
            } else {
                FailureKind::Source
            };
            return fail(
                kind,
                ValidationStage::Worker,
                error.to_string(),
                Some(identity),
            );
        }
    };
    let cancelled = || scopes.compiler.is_shutdown() || scopes.worker.is_shutdown();
    // The candidate's own timing document lays out everything probed and staged below,
    // as the project will play once it is published.
    let mut worker = worker;
    if let Some(json) = &candidate_timing {
        let applied = if worker.supports_timing() {
            worker
                .set_timing(Some(json.clone()), CANDIDATE_TIMING_GENERATION)
                .map(drop)
                .map_err(|error| error.to_string())
        } else {
            Err("this project's preview worker cannot play a timing document".into())
        };
        if let Err(reason) = applied {
            let kind = if cancelled() {
                FailureKind::Environment
            } else {
                FailureKind::Source
            };
            return fail(
                kind,
                ValidationStage::Worker,
                format!("{TIMING_FILE} was rejected: {reason}"),
                Some(identity),
            );
        }
    }
    let mut probe = WorkerProbe::new(worker, &cancelled, &config.service);
    let report = validate_candidate(
        captured,
        BuildOutcome::Built(identity.clone()),
        playhead,
        repair_used,
        Some(&mut probe),
    );
    let ProbeParts {
        worker,
        audio,
        timeline,
    } = probe.into_parts();
    if !report.passed() {
        drop(worker);
        scopes.worker.shutdown(std::time::Duration::ZERO);
        return CandidateRun {
            report,
            staged: None,
        };
    }
    // Stops the agent wrote must land on frames the candidate plays. A code edit that
    // leaves the person's own stops unresolved is not refused: the Slides tab shows them.
    if slides_changed && let (Some(doc), Some(timeline)) = (&candidate_slides, &timeline) {
        let (_, problems) = crate::slides::resolve::resolve(doc, timeline);
        let unresolved: Vec<String> = problems
            .iter()
            // An old worker is the SDK's problem, not the edit's.
            .filter(|problem| matches!(problem, crate::slides::SlidesProblem::Unresolved { .. }))
            .map(ToString::to_string)
            .collect();
        if !unresolved.is_empty() {
            drop(worker);
            return fail(
                FailureKind::Source,
                ValidationStage::Timeline,
                format!(
                    "{SLIDES_FILE} has stops the video does not play: {}",
                    unresolved.join("; ")
                ),
                Some(identity),
            );
        }
    }
    let staged = timeline
        .ok_or_else(|| "compiled timeline was not probed".to_owned())
        .map(|timeline| SeekIntent {
            identity: preview_identity(&tag),
            serial: 0,
            position: playhead,
            // Initial preparation shows the candidate at the largest preview scale its
            // dimensions allow, never the unsupported full size.
            scale: effective_scale(&timeline, 1.),
        })
        .and_then(|desired| {
            worker
                .ok_or_else(|| "preview worker was released".to_owned())
                .and_then(|worker| {
                    prepare_preview(
                        worker,
                        &tag,
                        &scopes.compiler,
                        &scopes.worker,
                        &|| Some(desired.clone()),
                        audio,
                        // The worker already plays the candidate's timing (the probed
                        // audio is laid out with it); an adopted candidate is retimed to
                        // the session's through the displayed lane.
                        None,
                        &config.service,
                    )
                    .map(|staged| staged.with_played_timing(candidate_timing))
                })
        });
    match staged {
        Ok(staged) => CandidateRun {
            report,
            staged: Some(staged),
        },
        // Validated bytes that cannot be prepared for matching playback are not acceptable.
        Err(message) => fail(
            FailureKind::Environment,
            ValidationStage::Worker,
            format!("staging matching playback failed: {message}"),
            Some(identity),
        ),
    }
}
