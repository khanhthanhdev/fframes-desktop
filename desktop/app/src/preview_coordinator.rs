//! Background build/preparation and a single-flight latest-wins frame pump.
use crate::{
    build_service::{BuildService, Subscriber, SubscriberKind},
    preview_worker_client::PreviewWorkerClient,
    teardown::Teardown,
    thumbnail_cache::ThumbnailKey,
    worker_project::{compile_portable_worker_via, launch_preview_worker},
};
use fframes_studio_protocol::*;
use parking_lot::Mutex;
use std::{collections::VecDeque, path::PathBuf, sync::Arc, time::Duration};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{
    OperationTag, PreviewFrame, PromotionAuthorization, ReadyPreview, preview_identity,
};

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
    /// Cancelling this scope detaches this build's subscription; the shared compile only
    /// stops when no subscriber remains.
    pub compiler: ProcessTreeManager,
    pub worker: ProcessTreeManager,
    /// Shared compile service; equal build keys (UI, agent tools) compile once.
    pub service: BuildService,
}
/// A prepared, matching candidate (worker, timeline, inspection, first frame, PCM). It is
/// owned by whoever holds it and is never visible to playback until it is explicitly
/// committed.
///
/// Dropping it never reaps anything on the dropping thread: the worker, its scope and
/// the materialization lease are handed to the process-wide [`crate::teardown`] owner
/// (bounded, non-blocking), so a drop is safe on the UI thread even while a worker is
/// slow to die or a process lock is held. [`StagedPreview::teardown_now`] is the explicit
/// synchronous alternative for threads that may block.
pub struct StagedPreview {
    parts: Option<StagedParts>,
}
/// The process-owning half of a [`StagedPreview`]; it moves to the teardown owner as a
/// whole.
#[doc(hidden)]
pub struct StagedParts {
    worker: PreviewWorkerClient,
    scope: ProcessTreeManager,
    boundary_inspection: InspectResponse,
    ready: Arc<ReadyPreview>,
}
impl StagedParts {
    /// Seals and terminates the worker scope, verifies it, then reaps the worker client
    /// (which reaps before dropping its materialization lease). `Some(problem)` when the
    /// exit could not be verified.
    fn finish(self) -> Option<String> {
        let Self { worker, scope, .. } = self;
        let termination = scope.shutdown_verified(Duration::ZERO);
        drop(worker);
        (!termination.verified())
            .then(|| "a staged preview worker's process tree was not verified gone".to_owned())
    }
}
impl std::ops::Deref for StagedPreview {
    type Target = StagedParts;
    fn deref(&self) -> &StagedParts {
        self.parts
            .as_ref()
            .expect("staged preview parts are present until it is dropped")
    }
}
impl std::ops::DerefMut for StagedPreview {
    fn deref_mut(&mut self) -> &mut StagedParts {
        self.parts
            .as_mut()
            .expect("staged preview parts are present until it is dropped")
    }
}
impl StagedPreview {
    fn new(parts: StagedParts) -> Self {
        Self { parts: Some(parts) }
    }
    pub fn ready(&self) -> &Arc<ReadyPreview> {
        &self.ready
    }
    /// Reaps the worker on the calling thread and waits for the verified exit. Only for
    /// threads that may block (never the UI thread).
    pub fn teardown_now(mut self) -> Option<String> {
        self.parts.take().and_then(StagedParts::finish)
    }
    fn matches_intent(&self, intent: &SeekIntent) -> bool {
        intent.serial == self.ready.seek_serial
            && intent.position.min(self.ready.timeline.total_frames) == self.ready.position
            && self.ready.frame.as_ref().is_none_or(|f| {
                f.response.scale == effective_scale(&self.ready.timeline, intent.scale)
            })
    }
}
impl Drop for StagedPreview {
    fn drop(&mut self) {
        if let Some(parts) = self.parts.take() {
            let tag = parts.ready.tag().clone();
            Teardown::global().submit("staged preview", Some(tag), move || parts.finish());
        }
    }
}

/// Why a staged preview was not adopted (it is dropped, reaping its worker).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdoptError {
    #[error("the staged preview does not carry the promotion authorization's tag")]
    WrongTag,
    #[error("the staged preview's worker has already been shut down")]
    WorkerGone,
    #[error("the latest seek intent has no valid preview scale")]
    InvalidIntent,
    #[error("the preview coordinator is closed")]
    Closed,
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
/// What a cancelled build owned: handed to the teardown owner (or reaped inline by
/// [`PreviewCoordinator::close`]), never dropped where the fence happened.
struct Discarded {
    scopes: Option<(ProcessTreeManager, ProcessTreeManager)>,
    staged: Option<StagedPreview>,
    result: Option<Result<StagedPreview, (OperationTag, String)>>,
    adopting: Option<StagedPreview>,
}
impl Discarded {
    fn is_empty(&self) -> bool {
        self.scopes.is_none()
            && self.staged.is_none()
            && self.result.is_none()
            && self.adopting.is_none()
    }
    fn staged_previews(
        self,
    ) -> (
        Option<(ProcessTreeManager, ProcessTreeManager)>,
        Vec<StagedPreview>,
    ) {
        let Self {
            scopes,
            staged,
            result,
            adopting,
        } = self;
        let previews = staged
            .into_iter()
            .chain(result.and_then(Result::ok))
            .chain(adopting)
            .collect();
        (scopes, previews)
    }
    fn submit(self) {
        if self.is_empty() {
            return;
        }
        let (scopes, previews) = self.staged_previews();
        Teardown::global().submit("preview build", None, move || {
            let mut problem = None;
            if let Some((compiler, worker)) = scopes {
                let compiler = compiler.shutdown_verified(Duration::ZERO);
                let worker = worker.shutdown_verified(Duration::ZERO);
                if !(compiler.merged().verified() && worker.merged().verified()) {
                    problem =
                        Some("a cancelled build's process tree was not verified gone".to_owned());
                }
            }
            // The previews' own destructors would only hand themselves to the owner
            // again; this closure already runs on it.
            for preview in previews {
                if let Some(error) = preview.teardown_now() {
                    problem = Some(error);
                }
            }
            problem
        });
    }
    fn teardown_now(self) {
        let (scopes, previews) = self.staged_previews();
        if let Some((compiler, worker)) = scopes {
            compiler.shutdown(Duration::ZERO);
            worker.shutdown(Duration::ZERO);
        }
        for preview in previews {
            let _ = preview.teardown_now();
        }
    }
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
    result: Option<Result<StagedPreview, (OperationTag, String)>>,
    staged: Option<StagedPreview>,
    /// An adopted preview waiting for its mandatory live worker round trip; it never
    /// reaches `result`/`staged` (and so never `ready`) without completing it.
    adopting: Option<StagedPreview>,
    install: Option<StagedPreview>,
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
    /// Cancels the current build and fences its results: after this returns (a bounded
    /// mailbox update, never a wait) nothing of it can be committed, adopted or published.
    /// The processes it owned (compile and worker scopes, staged workers) are reaped by
    /// the background [`crate::teardown`] owner, so this is safe on the UI thread even
    /// while a process lock is held or a worker is slow to die.
    pub fn cancel_build(&self) {
        let discarded = self.fence_build();
        discarded.submit();
    }
    /// Invalidates every build-owned slot under the mailbox lock and returns what owned
    /// processes; nothing is reaped here.
    fn fence_build(&self) -> Discarded {
        let mut s = self.shared.lock();
        s.pending_build = None;
        s.active_build = None;
        s.compiled = None;
        s.error = None;
        s.ready = None;
        Discarded {
            scopes: s.scopes.take(),
            staged: s.staged.take(),
            result: s.result.take(),
            adopting: s.adopting.take(),
        }
    }
    /// Hands a staged preview of just-published bytes to the playback lane.
    ///
    /// The staged candidate was prepared (by candidate validation) against the candidate
    /// revision at whatever playhead was current then; it is keyed to the promotion tag
    /// of `authorization` (base source = the published revision), never to the task
    /// base. Adoption always runs its own live worker round trip: the pump re-primes the
    /// staged worker (inspection, frame and PCM window) at the *latest* seek intent
    /// even when that intent numerically equals the one the candidate was validated at,
    /// and `ready` is published only after that round trip succeeds. The shell then
    /// reserves a new audio epoch and commits matching video and audio exactly as for
    /// any other candidate. A staged preview whose worker scope is already shut down is
    /// rejected. On any later failure (a dead worker) the staged worker is reaped, an
    /// `error` event is published and the displayed preview is untouched: nothing is
    /// retagged.
    ///
    /// A pending ordinary build is superseded (its source is obsolete once the
    /// published revision is the source).
    ///
    /// `latest` is the newest seek intent the shell knows (serial, playhead, scale). It
    /// is recorded as the desired intent unless a newer one already arrived, so the
    /// staged candidate is always re-primed to the latest position rather than the
    /// playhead it was validated at.
    pub fn adopt(
        &self,
        staged: StagedPreview,
        authorization: &PromotionAuthorization,
        latest: SeekIntent,
    ) -> Result<(), AdoptError> {
        if staged.ready.tag() != authorization.tag() {
            return Err(AdoptError::WrongTag);
        }
        if staged.scope.is_shutdown() {
            return Err(AdoptError::WorkerGone);
        }
        if !(latest.scale.is_finite() && latest.scale > 0. && latest.scale <= 1.) {
            return Err(AdoptError::InvalidIntent);
        }
        self.cancel_build();
        let mut s = self.shared.lock();
        if s.closed {
            return Err(AdoptError::Closed);
        }
        if s.desired.as_ref().is_none_or(|d| latest.serial >= d.serial) {
            s.desired = Some(latest);
        }
        s.active_build = Some(authorization.tag().clone());
        s.adopting = Some(staged);
        Ok(())
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
    /// Terminal: fences everything and reaps every owned process on the calling thread
    /// (waiting for the verified exits). Never call this on the UI thread.
    pub fn close(&self) {
        let displayed = {
            let mut s = self.shared.lock();
            s.closed = true;
            s.thumbnails.clear();
            s.thumbnail = None;
            s.displayed_scope.take()
        };
        self.fence_build().teardown_now();
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
) -> Result<StagedPreview, (OperationTag, String)> {
    let tag = spec.tag.clone();
    let result = (|| -> Result<StagedPreview, String> {
        let build = compile_portable_worker_via(
            &spec.service,
            Subscriber::new(SubscriberKind::Ui, format!("{:?}", tag)),
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
        let worker = launch_preview_worker(build, preview_identity(&tag), &spec.worker)?;
        prepare_preview(
            worker,
            &tag,
            &spec.compiler,
            &spec.worker,
            &|| shared.lock().desired.clone(),
            None,
            &spec.service,
        )
    })();
    if result.is_err() {
        spec.worker.shutdown(Duration::ZERO);
    }
    result.map_err(|e| (tag, e))
}

/// Registers the PCM bytes of `source` in the shared build service's budget and ties the
/// accounting to the source (and so to the open file). An entry the cache no longer
/// holds is outside the budget already, so nothing is charged for it.
pub(crate) fn account_prepared_audio(
    service: &BuildService,
    descriptor: &PreparedAudioDescriptor,
    source: &Arc<studio_engine::PreparedAudioSource>,
) -> Result<(), String> {
    match service.account_artifact_bytes(source.build(), descriptor.byte_count) {
        Ok(guard) => {
            source.hold_accounting(Box::new(guard));
            Ok(())
        }
        Err(crate::build_service::BuildError::NotCached) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Prepare a negotiated worker into a complete matching candidate: timeline, boundary and
/// playhead inspection, first frame and PCM window. `prepared_audio` lets a caller that
/// already prepared, verified and accounted the mix (candidate validation) reuse it
/// instead of asking the worker for a second artifact; otherwise the PCM prepared here
/// is accounted in `service`'s budget. Nothing here publishes anything.
pub(crate) fn prepare_preview(
    mut worker: PreviewWorkerClient,
    tag: &OperationTag,
    compiler: &ProcessTreeManager,
    worker_scope: &ProcessTreeManager,
    desired: &dyn Fn() -> Option<SeekIntent>,
    prepared_audio: Option<(
        PreparedAudioDescriptor,
        Arc<studio_engine::PreparedAudioSource>,
    )>,
    service: &BuildService,
) -> Result<StagedPreview, String> {
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
    let (audio, audio_source) = match prepared_audio {
        Some(prepared) => prepared,
        None => {
            let audio = worker.prepare_audio(48000).map_err(|e| e.to_string())?;
            let source = worker.retain_audio_source(&audio, || {
                compiler.is_shutdown() || worker_scope.is_shutdown()
            })?;
            account_prepared_audio(service, &audio, &source)?;
            (audio, source)
        }
    };
    let desired = desired();
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
    if compiler.is_shutdown() || worker_scope.is_shutdown() {
        return Err("Preview preparation cancelled".into());
    }
    Ok(StagedPreview::new(StagedParts {
        worker,
        scope: worker_scope.clone(),
        boundary_inspection,
        ready: Arc::new(ready),
    }))
}

fn publish_result(
    shared: &Arc<Mutex<Mailbox>>,
    result: Result<StagedPreview, (OperationTag, String)>,
) {
    let tag = match &result {
        Ok(c) => c.ready.tag(),
        Err((tag, _)) => tag,
    };
    let mut s = shared.lock();
    if s.active_build.as_ref() == Some(tag) && !s.closed {
        s.result = Some(result);
        return;
    }
    drop(s);
    // Superseded while it was being prepared: this is a build thread, so the worker is
    // reaped (and verified) right here.
    if let Ok(staged) = result {
        let _ = staged.teardown_now();
    }
}

fn pump(shared: Arc<Mutex<Mailbox>>) {
    let mut displayed: Option<StagedPreview> = None;
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
        let (spec, result, install, desired, adoption, candidate) = {
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
                if preparation.is_none() && s.desired.is_some() {
                    s.adopting.take()
                } else {
                    None
                },
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
        if let (Some(mut c), Some(d)) = (adoption, desired.clone()) {
            // Mandatory, even when `d` equals the intent `c` was prepared at: readiness
            // is only ever published by a worker round trip made for this adoption.
            let mailbox = shared.clone();
            preparation = Some(std::thread::spawn(move || {
                let result = verify_adopted(&mut c, &d)
                    .map_err(|e| (c.ready.tag().clone(), e))
                    .map(|()| c);
                publish_result(&mailbox, result);
            }));
        } else if let (Some(mut c), Some(d)) = (candidate, desired.clone()) {
            let mailbox = shared.clone();
            preparation = Some(std::thread::spawn(move || {
                let result = reprime(&mut c, &d)
                    .map_err(|e| (c.ready.tag().clone(), e))
                    .map(|()| c);
                publish_result(&mailbox, result);
            }));
        }
        let parts: Option<&mut StagedParts> = displayed.as_deref_mut();
        if let (Some(c), Some(d)) = (parts, &desired)
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
        if let Some(c) = displayed.as_deref_mut() {
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
    // The pump is its own background thread: reap the displayed worker here.
    if let Some(displayed) = displayed {
        let _ = displayed.teardown_now();
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

/// The adoption-specific live check: the staged worker's scope must still be live, the
/// worker must answer a full re-prime at `d` (inspection, frame, PCM window), and the
/// scope must still be live afterwards.
fn verify_adopted(c: &mut StagedParts, d: &SeekIntent) -> Result<(), String> {
    let gone = || "the staged preview's worker was shut down before it could be adopted".to_owned();
    if c.scope.is_shutdown() {
        return Err(gone());
    }
    reprime(c, d)?;
    if c.scope.is_shutdown() {
        return Err(gone());
    }
    Ok(())
}

fn reprime(c: &mut StagedParts, d: &SeekIntent) -> Result<(), String> {
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
