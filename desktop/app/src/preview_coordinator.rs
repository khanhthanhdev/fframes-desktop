//! Background build/preparation and a single-flight latest-wins frame pump.
use crate::{
    preview_worker_client::PreviewWorkerClient,
    thumbnail_cache::ThumbnailKey,
    worker_project::{compile_portable_worker, launch_preview_worker},
};
use fframes_studio_protocol::*;
use parking_lot::Mutex;
use std::{collections::VecDeque, path::PathBuf, sync::Arc, time::Duration};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{OperationTag, PreviewFrame, ReadyPreview, preview_identity};

#[derive(Debug, Clone, PartialEq)]
pub struct SeekIntent {
    pub identity: PreviewIdentity,
    pub serial: u64,
    pub position: usize,
    pub scale: f64,
}

pub struct BuildSpec {
    pub project: studio_project::OpenProject,
    pub sdk: PathBuf,
    pub compatibility: studio_sdk::CompatibilityManifest,
    pub builds: PathBuf,
    pub tag: OperationTag,
    pub compiler: ProcessTreeManager,
    pub worker: ProcessTreeManager,
}
struct Candidate {
    worker: PreviewWorkerClient,
    scope: ProcessTreeManager,
    boundary_inspection: InspectResponse,
    ready: Arc<ReadyPreview>,
}
impl Candidate {
    fn matches_intent(&self, intent: &SeekIntent) -> bool {
        intent.serial == self.ready.seek_serial
            && intent.position.min(self.ready.timeline.total_frames) == self.ready.position
            && self.ready.frame.as_ref().is_none_or(|f| {
                f.response.scale == effective_scale(&self.ready.timeline, intent.scale)
            })
    }
}
impl Drop for Candidate {
    fn drop(&mut self) {
        self.scope.shutdown(Duration::ZERO);
        // WorkerClient reaps before dropping its materialization lease.
    }
}

#[derive(Default)]
pub struct PreviewEvents {
    pub compiled: Option<OperationTag>,
    pub ready: Option<Arc<ReadyPreview>>,
    pub frame: Option<PreviewFrame>,
    pub thumbnail: Option<(ThumbnailKey, PreviewFrame)>,
    pub error: Option<(OperationTag, String)>,
}
#[derive(Default, Clone, serde::Serialize)]
pub struct PumpMetrics {
    pub renders: usize,
    pub late_frames: usize,
    pub render_max_ms: f64,
    pub queue_high_water: usize,
    pub thumbnail_queue_high_water: usize,
    pub owned_processes: usize,
}
#[derive(Default)]
struct Mailbox {
    metrics: PumpMetrics,
    desired: Option<SeekIntent>,
    thumbnails: VecDeque<ThumbnailKey>,
    active_thumbnail: Option<ThumbnailKey>,
    thumbnail: Option<(ThumbnailKey, PreviewFrame)>,
    pending_build: Option<BuildSpec>,
    active_build: Option<OperationTag>,
    scopes: Option<(ProcessTreeManager, ProcessTreeManager)>,
    displayed_scope: Option<ProcessTreeManager>,
    result: Option<Result<Candidate, (OperationTag, String)>>,
    staged: Option<Candidate>,
    install: Option<Candidate>,
    compiled: Option<OperationTag>,
    ready: Option<Arc<ReadyPreview>>,
    frame: Option<PreviewFrame>,
    error: Option<(OperationTag, String)>,
    closed: bool,
}

/// The mailbox retains one replaceable intent, one frame and one candidate event.
/// No mutex is held while hashing, compiling or talking to a worker.
pub struct PreviewCoordinator {
    shared: Arc<Mutex<Mailbox>>,
    owner: ProcessTreeManager,
    task: Option<std::thread::JoinHandle<()>>,
}
impl PreviewCoordinator {
    pub fn new(owner: ProcessTreeManager) -> Self {
        let shared = Arc::new(Mutex::new(Mailbox::default()));
        let mailbox = shared.clone();
        let task = std::thread::spawn(move || pump(mailbox));
        Self {
            shared,
            owner,
            task: Some(task),
        }
    }
    pub fn build(&self, spec: BuildSpec) {
        self.cancel_build();
        let mut s = self.shared.lock();
        if s.closed {
            return;
        }
        s.scopes = Some((spec.compiler.clone(), spec.worker.clone()));
        s.active_build = Some(spec.tag.clone());
        s.pending_build = Some(spec);
    }
    pub fn cancel_build(&self) {
        let (scopes, staged, result) = {
            let mut s = self.shared.lock();
            s.pending_build = None;
            s.active_build = None;
            s.compiled = None;
            s.error = None;
            s.ready = None;
            (s.scopes.take(), s.staged.take(), s.result.take())
        };
        if let Some((compiler, worker)) = scopes {
            compiler.shutdown(Duration::ZERO);
            worker.shutdown(Duration::ZERO);
        }
        drop((staged, result));
    }
    pub fn seek(&self, intent: SeekIntent) {
        let mut s = self.shared.lock();
        if !s.closed
            && intent.scale.is_finite()
            && intent.scale > 0.
            && intent.scale <= 1.
            && s.desired.as_ref().is_none_or(|d| intent.serial > d.serial)
        {
            s.desired = Some(intent);
            s.metrics.queue_high_water = 1;
        }
    }
    pub fn events(&self) -> PreviewEvents {
        let mut s = self.shared.lock();
        PreviewEvents {
            compiled: s.compiled.take(),
            ready: s.ready.take(),
            frame: s.frame.take(),
            thumbnail: s.thumbnail.take(),
            error: s.error.take(),
        }
    }
    pub fn metrics(&self) -> PumpMetrics {
        let mut metrics = self.shared.lock().metrics.clone();
        metrics.owned_processes = self.owner.active_count();
        metrics
    }
    /// Replace obsolete visible samples. Main seeks always dispatch first.
    pub fn thumbnails(&self, keys: Vec<ThumbnailKey>) {
        let mut s = self.shared.lock();
        if s.closed {
            return;
        }
        let mut next = VecDeque::new();
        for key in keys.into_iter().take(12) {
            let scale = f64::from_bits(key.scale_bits);
            if scale.is_finite()
                && scale > 0.
                && scale <= 1.
                && s.desired
                    .as_ref()
                    .is_some_and(|d| d.identity == key.identity)
                && s.active_thumbnail.as_ref() != Some(&key)
                && s.thumbnail.as_ref().is_none_or(|(k, _)| k != &key)
                && !next.contains(&key)
            {
                next.push_back(key);
            }
        }
        s.metrics.thumbnail_queue_high_water = s.metrics.thumbnail_queue_high_water.max(next.len());
        s.thumbnails = next;
    }
    pub fn commit(&self, identity: PreviewIdentity, serial: u64) -> bool {
        let mut s = self.shared.lock();
        if !s.staged.as_ref().is_some_and(|c| {
            c.ready.identity() == &identity
                && c.ready.seek_serial == serial
                && s.active_build.as_ref() == Some(c.ready.tag())
                && s.desired.as_ref().is_none_or(|d| c.matches_intent(d))
        }) || s.closed
        {
            return false;
        }
        s.install = s.staged.take();
        s.thumbnails.clear();
        s.thumbnail = None;
        if let Some(c) = &s.install {
            let ready = c.ready.clone();
            let scope = c.scope.clone();
            s.desired = Some(SeekIntent {
                identity,
                serial,
                position: ready.position,
                scale: ready.frame.as_ref().map_or(1., |f| f.response.scale),
            });
            s.displayed_scope = Some(scope);
        }
        s.active_build = None;
        s.ready = None;
        // The accepted candidate is now owned by the displayed-worker lane.
        s.scopes = None;
        true
    }
    pub fn close(&self) {
        let displayed = {
            let mut s = self.shared.lock();
            s.closed = true;
            s.thumbnails.clear();
            s.thumbnail = None;
            s.displayed_scope.take()
        };
        self.cancel_build();
        if let Some(displayed) = displayed {
            displayed.shutdown(Duration::ZERO);
        }
        self.owner.shutdown(Duration::ZERO);
    }
}
impl Drop for PreviewCoordinator {
    fn drop(&mut self) {
        self.close();
        if let Some(t) = self.task.take() {
            let _ = t.join();
        }
    }
}

fn prepare(
    spec: BuildSpec,
    shared: &Arc<Mutex<Mailbox>>,
) -> Result<Candidate, (OperationTag, String)> {
    let tag = spec.tag.clone();
    let result = (|| -> Result<Candidate, String> {
        let build = compile_portable_worker(
            &spec.project,
            &spec.sdk,
            spec.compatibility,
            &spec.builds,
            &spec.compiler,
        )?;
        if spec.compiler.is_shutdown() {
            return Err("Preview build cancelled".into());
        }
        {
            let mut s = shared.lock();
            if s.active_build.as_ref() != Some(&tag) || s.closed {
                return Err("Preview build superseded".into());
            }
            s.compiled = Some(tag.clone());
        }
        let mut worker = launch_preview_worker(build, preview_identity(&tag), &spec.worker)?;
        let timeline = worker.timeline().map_err(|e| e.to_string())?;
        let mut boundaries = vec![];
        if timeline.total_frames > 0 {
            boundaries.extend([0, timeline.total_frames - 1]);
        }
        for scene in &timeline.scenes {
            if scene.start_frame < timeline.total_frames {
                boundaries.push(scene.start_frame);
            }
            if scene.end_frame > 0 {
                boundaries.push(scene.end_frame - 1);
            }
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        if boundaries.len() > MAX_INSPECT_FRAMES {
            return Err("Too many scene boundaries for complete bounded preview inspection".into());
        }
        let boundary_inspection = worker.inspect(boundaries).map_err(|e| e.to_string())?;
        if boundary_inspection.truncated
            || boundary_inspection
                .diagnostics
                .iter()
                .any(|d| d.severity == DiagnosticSeverity::Error)
        {
            return Err(format!(
                "Preview inspection failed: {:?}",
                boundary_inspection.diagnostics
            ));
        }
        let audio = worker.prepare_audio(48000).map_err(|e| e.to_string())?;
        let audio_source = worker.retain_audio_source(&audio, || {
            spec.compiler.is_shutdown() || spec.worker.is_shutdown()
        })?;
        let desired = shared.lock().desired.clone();
        let position = desired
            .as_ref()
            .map_or(0, |d| d.position)
            .min(timeline.total_frames);
        let serial = desired.as_ref().map_or(0, |d| d.serial);
        let scale = desired.as_ref().map_or(1., |d| d.scale);
        let inspection = if timeline.total_frames == 0 {
            boundary_inspection.clone()
        } else {
            inspect_installation_frame(
                &mut worker,
                &boundary_inspection,
                position.min(timeline.total_frames - 1),
            )?
        };
        let frame = if timeline.total_frames == 0 {
            None
        } else {
            Some(
                worker
                    .frame(position.min(timeline.total_frames - 1), serial, scale)
                    .map_err(|e| e.to_string())?,
            )
        };
        let sample = (position as u128 * audio.sample_rate as u128 / timeline.fps as u128)
            .min(audio.sample_count as u128) as u64;
        let length = ((audio.byte_count - sample * 8).min(MAX_AUDIO_READ_BYTES as u64)) as usize;
        let pcm = worker
            .read_audio(&audio, sample * 8, length)
            .map_err(|e| e.to_string())?;
        let ready = ReadyPreview::new(
            tag.clone(),
            timeline,
            inspection,
            frame,
            audio,
            sample,
            pcm,
            position,
            serial,
        )?
        .with_audio_source(audio_source)?;
        if spec.compiler.is_shutdown() || spec.worker.is_shutdown() {
            return Err("Preview preparation cancelled".into());
        }
        Ok(Candidate {
            worker,
            scope: spec.worker.clone(),
            boundary_inspection,
            ready: Arc::new(ready),
        })
    })();
    if result.is_err() {
        spec.worker.shutdown(Duration::ZERO);
    }
    result.map_err(|e| (tag, e))
}

fn publish_result(shared: &Arc<Mutex<Mailbox>>, result: Result<Candidate, (OperationTag, String)>) {
    let tag = match &result {
        Ok(c) => c.ready.tag(),
        Err((tag, _)) => tag,
    };
    let mut s = shared.lock();
    if s.active_build.as_ref() == Some(tag) && !s.closed {
        s.result = Some(result);
    }
}

fn pump(shared: Arc<Mutex<Mailbox>>) {
    let mut displayed: Option<Candidate> = None;
    // Compilation, initial preparation and re-priming share one candidate lane.
    // Its blocking worker requests never run on the displayed-worker pump.
    let mut preparation: Option<std::thread::JoinHandle<()>> = None;
    let mut rendered: Option<SeekIntent> = None;
    loop {
        if shared.lock().closed {
            break;
        }
        if preparation.as_ref().is_some_and(|t| t.is_finished()) {
            let _ = preparation.take().unwrap().join();
        }
        let (spec, result, install, desired, candidate) = {
            let mut s = shared.lock();
            (
                if preparation.is_none() {
                    s.pending_build.take()
                } else {
                    None
                },
                s.result.take(),
                s.install.take(),
                s.desired.clone(),
                if preparation.is_none()
                    && s.staged
                        .as_ref()
                        .is_some_and(|c| s.desired.as_ref().is_some_and(|d| !c.matches_intent(d)))
                {
                    // A previously published ready event is no longer installable.
                    s.ready = None;
                    s.staged.take()
                } else {
                    None
                },
            )
        };
        if let Some(new) = install {
            rendered = Some(SeekIntent {
                identity: new.ready.identity().clone(),
                serial: new.ready.seek_serial,
                position: new.ready.position,
                scale: new.ready.frame.as_ref().map_or(1., |f| f.response.scale),
            });
            displayed = Some(new);
        }
        if let Some(spec) = spec {
            let mailbox = shared.clone();
            preparation = Some(std::thread::spawn(move || {
                let result = prepare(spec, &mailbox);
                publish_result(&mailbox, result);
            }));
        }
        if let Some(result) = result {
            let mut s = shared.lock();
            match result {
                Ok(c) if s.active_build.as_ref() == Some(c.ready.tag()) => {
                    // A seek may have arrived while the candidate lane was blocked.
                    // Keep the worker for re-priming, but never publish stale readiness.
                    s.ready = s
                        .desired
                        .as_ref()
                        .is_none_or(|d| c.matches_intent(d))
                        .then(|| c.ready.clone());
                    s.staged = Some(c);
                }
                Ok(_) => (),
                Err(e) if s.active_build.as_ref() == Some(&e.0) => s.error = Some(e),
                Err(_) => (),
            }
        }
        if let (Some(mut c), Some(d)) = (candidate, desired.clone()) {
            let mailbox = shared.clone();
            preparation = Some(std::thread::spawn(move || {
                let result = reprime(&mut c, &d)
                    .map_err(|e| (c.ready.tag().clone(), e))
                    .map(|()| c);
                publish_result(&mailbox, result);
            }));
        }
        if let (Some(c), Some(d)) = (&mut displayed, &desired)
            && c.ready.identity() == &d.identity
            && rendered.as_ref() != Some(d)
            && c.ready.timeline.total_frames > 0
        {
            let started = std::time::Instant::now();
            match c.worker.frame(
                d.position.min(c.ready.timeline.total_frames - 1),
                d.serial,
                d.scale,
            ) {
                Ok(f) => {
                    let mut s = shared.lock();
                    if s.desired.as_ref() == Some(d) {
                        s.frame = Some(f);
                    } else {
                        s.metrics.late_frames += 1;
                    }
                }
                Err(e) => shared.lock().error = Some((c.ready.tag().clone(), e.to_string())),
            }
            {
                let mut s = shared.lock();
                s.metrics.renders += 1;
                s.metrics.render_max_ms = s
                    .metrics
                    .render_max_ms
                    .max(started.elapsed().as_secs_f64() * 1000.);
            }
            rendered = Some(d.clone());
        }
        if let Some(c) = &mut displayed {
            let key = {
                let mut s = shared.lock();
                if s.install.is_none()
                    && s.thumbnail.is_none()
                    && s.frame.is_none()
                    && s.desired.as_ref() == rendered.as_ref()
                {
                    let key = s.thumbnails.pop_front();
                    s.active_thumbnail = key.clone();
                    key
                } else {
                    None
                }
            };
            if let Some(key) = key {
                if key.identity == *c.ready.identity()
                    && key.frame_index < c.ready.timeline.total_frames
                {
                    let frame = c
                        .worker
                        .frame(key.frame_index, 0, f64::from_bits(key.scale_bits));
                    let mut s = shared.lock();
                    if !s.closed
                        && s.install.is_none()
                        && s.desired
                            .as_ref()
                            .is_some_and(|d| d.identity == key.identity)
                        && let Ok(frame) = frame
                    {
                        // Separate completion destination: never touches s.frame or desired.
                        s.thumbnail = Some((key, frame));
                    }
                }
                shared.lock().active_thumbnail = None;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Some(t) = preparation {
        let _ = t.join();
    }
}

pub fn effective_scale(t: &PreviewTimelineResponse, scale: f64) -> f64 {
    scale.min(
        (MAX_PREVIEW_WIDTH as f64 / t.width as f64)
            .min(MAX_PREVIEW_HEIGHT as f64 / t.height as f64)
            .min(1.),
    )
}

fn inspect_installation_frame(
    worker: &mut PreviewWorkerClient,
    boundaries: &InspectResponse,
    frame: usize,
) -> Result<InspectResponse, String> {
    let mut inspection = worker.inspect(vec![frame]).map_err(|e| e.to_string())?;
    // Retain boundary findings, but replace findings for this frame rather than
    // accumulating old playheads or duplicate diagnostics on every re-prime.
    inspection.diagnostics.extend(
        boundaries
            .diagnostics
            .iter()
            .filter(|d| d.frame != frame)
            .cloned(),
    );
    inspection.truncated |= boundaries.truncated || inspection.diagnostics.len() > MAX_DIAGNOSTICS;
    inspection.diagnostics.truncate(MAX_DIAGNOSTICS);
    if inspection.truncated
        || inspection
            .diagnostics
            .iter()
            .any(|d| d.severity == DiagnosticSeverity::Error)
    {
        return Err(format!(
            "Preview inspection failed: {:?}",
            inspection.diagnostics
        ));
    }
    Ok(inspection)
}

fn reprime(c: &mut Candidate, d: &SeekIntent) -> Result<(), String> {
    let position = d.position.min(c.ready.timeline.total_frames);
    let inspection = if c.ready.timeline.total_frames == 0 {
        c.boundary_inspection.clone()
    } else {
        inspect_installation_frame(
            &mut c.worker,
            &c.boundary_inspection,
            position.min(c.ready.timeline.total_frames - 1),
        )?
    };
    let frame = if c.ready.timeline.total_frames == 0 {
        None
    } else {
        Some(
            c.worker
                .frame(
                    position.min(c.ready.timeline.total_frames - 1),
                    d.serial,
                    d.scale,
                )
                .map_err(|e| e.to_string())?,
        )
    };
    let sample = (position as u128 * c.ready.audio.sample_rate as u128
        / c.ready.timeline.fps as u128)
        .min(c.ready.audio.sample_count as u128) as u64;
    let length = (c.ready.audio.byte_count - sample * 8).min(MAX_AUDIO_READ_BYTES as u64) as usize;
    let pcm = c
        .worker
        .read_audio(&c.ready.audio, sample * 8, length)
        .map_err(|e| e.to_string())?;
    let mut ready = ReadyPreview::new(
        c.ready.tag().clone(),
        c.ready.timeline.clone(),
        inspection,
        frame,
        c.ready.audio.clone(),
        sample,
        pcm,
        position,
        d.serial,
    )?;
    if let Some(source) = &c.ready.audio_source {
        ready = ready.with_audio_source(source.clone())?;
    }
    c.ready = Arc::new(ready);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker_client::WorkerClient;
    use std::io::{Cursor, empty, sink};
    use studio_engine::{OpenSession, OperationId};

    fn inspection(diagnostics: Vec<PreviewDiagnostic>, truncated: bool) -> InspectResponse {
        InspectResponse {
            envelope: PreviewEnvelope {
                contract_version: PREVIEW_CONTRACT_VERSION,
                identity: PreviewIdentity {
                    project_id: "p".into(),
                    open_session: "s".into(),
                    source_revision: "r".into(),
                    worker_generation: 1,
                },
                request_id: 1,
            },
            diagnostics,
            truncated,
        }
    }

    fn inspection_worker(response: InspectResponse) -> PreviewWorkerClient {
        let identity = response.envelope.identity.clone();
        let bytes = serde_json::to_vec(&PreviewResponse::Inspect(response)).unwrap();
        let mut framed = (bytes.len() as u32).to_be_bytes().to_vec();
        framed.extend(bytes);
        let mut transport = WorkerClient::new("r", 1);
        transport.attach_pipes(sink(), Cursor::new(framed), empty());
        PreviewWorkerClient::new(transport, identity)
    }

    fn warning(frame: usize) -> PreviewDiagnostic {
        PreviewDiagnostic {
            frame,
            severity: DiagnosticSeverity::Warning,
            key: "warning".into(),
            message: "test warning".into(),
        }
    }

    #[test]
    fn install_frame_inspection_rejects_errors_truncation_and_combined_overflow() {
        let mut missing = warning(19);
        missing.severity = DiagnosticSeverity::Error;
        missing.message = "absent.jpg".into();
        for (response, boundaries) in [
            (inspection(vec![missing], false), inspection(vec![], false)),
            (inspection(vec![], true), inspection(vec![], false)),
            (
                inspection(vec![warning(19)], false),
                inspection(vec![warning(0); MAX_DIAGNOSTICS], false),
            ),
        ] {
            let mut worker = inspection_worker(response);
            assert!(inspect_installation_frame(&mut worker, &boundaries, 19).is_err());
        }
    }

    #[test]
    fn install_frame_inspection_retains_boundaries_without_duplicate_playheads() {
        let boundaries = inspection(vec![warning(0), warning(19)], false);
        for frame in [19, 37] {
            let mut worker = inspection_worker(inspection(vec![warning(frame)], false));
            let actual = inspect_installation_frame(&mut worker, &boundaries, frame).unwrap();
            let expected = if frame == 19 {
                vec![warning(19), warning(0)]
            } else {
                vec![warning(37), warning(0), warning(19)]
            };
            assert_eq!(actual.diagnostics, expected);
            assert!(!actual.truncated);
        }
    }

    #[test]
    fn cancelled_build_result_cannot_become_current_after_a_new_build_starts() {
        let old = OperationTag {
            project: "p".to_owned().try_into().unwrap(),
            session: OpenSession::new(),
            base_source: "a".repeat(64).try_into().unwrap(),
            operation: OperationId(1),
            generation: 1,
        };
        let mut newer = old.clone();
        newer.operation = OperationId(2);
        newer.generation = 2;
        let coordinator = PreviewCoordinator::new(ProcessTreeManager::new());
        {
            let mut s = coordinator.shared.lock();
            s.active_build = Some(newer);
            s.result = Some(Err((old, "obsolete compiler failure".into())));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while coordinator.shared.lock().result.is_some() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(coordinator.events().error.is_none());
        coordinator.close();
    }
}
