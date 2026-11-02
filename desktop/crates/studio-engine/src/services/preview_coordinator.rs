//! Background build/preparation and a single-flight latest-wins frame pump.
use super::{
    build_service::{BuildService, Subscriber, SubscriberKind},
    preview_worker_client::PreviewWorkerClient,
    teardown::Teardown,
    thumbnail_cache::ThumbnailKey,
    worker_project::{compile_portable_worker_via, launch_preview_worker},
};
use crate::{OperationTag, PreviewFrame, PromotionAuthorization, ReadyPreview, preview_identity};
use fframes_studio_protocol::*;
use parking_lot::{Condvar, Mutex};
use std::{collections::VecDeque, path::PathBuf, sync::Arc, time::Duration};
use studio_bootstrap::ProcessTreeManager;

#[derive(Debug, Clone, PartialEq)]
pub struct SeekIntent {
    pub identity: PreviewIdentity,
    pub serial: u64,
    pub position: usize,
    pub scale: f64,
}

/// The timing document the preview plays, with the generation that names its layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewTiming {
    /// `None` plays the code order.
    pub json: Option<String>,
    pub generation: u64,
}

/// The props document the preview draws, with the generation that names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewProps {
    /// `None` draws the code's values.
    pub json: Option<String>,
    pub generation: u64,
}

/// The props of the selected object, or why the preview cannot tell them.
pub type PropsInfoEvent = (EditorObjectIdentity, Result<Option<PropsInfo>, String>);

/// Read-only scan state for the Slides tab; absent for workers without the capability.
#[derive(Debug, Clone, PartialEq)]
pub struct StopScan {
    pub timeline: PreviewTimelineResponse,
    /// Props used while collecting these proposals; acceptance fences this generation too.
    pub props_generation: u64,
    pub running: bool,
    pub completed: usize,
    pub total: usize,
    pub sample_limit: bool,
    pub suggestions: StopSuggestions,
    pub notice: Option<String>,
    serial: u64,
    frames: Vec<usize>,
}

impl StopScan {
    /// Empty scan state for a capable preview, also usable by UI fixtures.
    pub fn new(timeline: PreviewTimelineResponse, props_generation: u64) -> Self {
        Self {
            timeline,
            props_generation,
            running: false,
            completed: 0,
            total: 0,
            sample_limit: false,
            suggestions: StopSuggestions::default(),
            notice: None,
            serial: 0,
            frames: Vec::new(),
        }
    }
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
/// the materialization lease are handed to the process-wide [`crate::services::teardown`] owner
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
    /// The timing generation this worker was last asked to play; `None` until one was
    /// applied.
    timing_attempt: Option<u64>,
    /// The document the worker plays (`None`: code order), to go back to when a retime
    /// fails halfway.
    timing_json: Option<String>,
    /// The props generation this worker was last asked to draw; `None` until one was.
    props_attempt: Option<u64>,
    /// Accounts the PCM a retimed layout prepares.
    service: BuildService,
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
    /// Records the timing document the worker was laid out with before preparation, so
    /// a failed retime restores it.
    pub(crate) fn with_played_timing(mut self, timing_json: Option<String>) -> Self {
        if let Some(parts) = self.parts.as_mut() {
            parts.timing_json = timing_json;
        }
        self
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
    /// The displayed preview laid out with the latest [`PreviewCoordinator::set_timing`],
    /// or why it could not be. It replaces the displayed preview's timeline, frame and
    /// audio; the worker stays the same.
    pub retimed: Option<Result<Arc<ReadyPreview>, (OperationTag, String)>>,
    /// The displayed worker applied the props of this generation, or why it did not.
    pub props_applied: Option<Result<u64, String>>,
    /// The props of the object [`PreviewCoordinator::want_props_info`] asked for.
    pub props_info: Option<PropsInfoEvent>,
    /// Scan availability or progress changed; query [`PreviewCoordinator::stop_scan`].
    pub stop_scan_changed: bool,
}
#[derive(Default, Clone, serde::Serialize)]
pub struct PumpMetrics {
    pub renders: usize,
    pub late_frames: usize,
    pub render_max_ms: f64,
    pub queue_high_water: usize,
    pub thumbnail_queue_high_water: usize,
    /// Thumbnails delivered to the UI.
    pub thumbnails: usize,
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
    /// The timing the displayed preview and later builds should play.
    timing: Option<PreviewTiming>,
    retimed: Option<Result<Arc<ReadyPreview>, (OperationTag, String)>>,
    /// The props the displayed preview and later builds should draw.
    props: Option<PreviewProps>,
    props_applied: Option<Result<u64, String>>,
    /// The object whose props the shell shows, with a serial that changes with every ask.
    props_wanted: Option<(EditorObjectIdentity, u64)>,
    props_info: Option<PropsInfoEvent>,
    stop_scan: Option<StopScan>,
    stop_scan_changed: bool,
    closed: bool,
    /// The listener was told about the news now waiting; it is told again only after
    /// [`PreviewCoordinator::events`] took it.
    notified: bool,
}

impl Mailbox {
    fn has_news(&self) -> bool {
        self.compiled.is_some()
            || self.ready.is_some()
            || self.frame.is_some()
            || self.thumbnail.is_some()
            || self.error.is_some()
            || self.retimed.is_some()
            || self.props_applied.is_some()
            || self.props_info.is_some()
            || self.stop_scan_changed
    }
}

type Listener = Arc<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>>;

/// The mailbox retains one replaceable intent, one frame and one candidate event.
/// No mutex is held while hashing, compiling or talking to a worker.
pub struct PreviewCoordinator {
    shared: Arc<Mutex<Mailbox>>,
    /// Wakes the idle pump when a seek arrives, so a frame request never waits out its
    /// idle tick; other news is picked up on the next tick.
    seeked: Arc<Condvar>,
    listener: Listener,
    owner: ProcessTreeManager,
    task: Option<std::thread::JoinHandle<()>>,
}
impl PreviewCoordinator {
    pub fn new(owner: ProcessTreeManager) -> Self {
        let shared = Arc::new(Mutex::new(Mailbox::default()));
        let seeked = Arc::new(Condvar::new());
        let listener = Listener::default();
        let mailbox = shared.clone();
        let pump_seeked = seeked.clone();
        let wake = listener.clone();
        let task = std::thread::spawn(move || pump(mailbox, pump_seeked, wake));
        Self {
            shared,
            seeked,
            listener,
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
    /// the background [`crate::services::teardown`] owner, so this is safe on the UI thread even
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
            self.seeked.notify_one();
        }
    }
    /// Plays `timing` from now on: the pump applies it to the displayed worker (preparing
    /// its audio again) and reports the result as [`PreviewEvents::retimed`]; builds apply
    /// it before they read their timeline.
    pub fn set_timing(&self, timing: PreviewTiming) {
        let mut s = self.shared.lock();
        if !s.closed {
            if s.timing.as_ref() != Some(&timing) {
                s.stop_scan = None;
                s.stop_scan_changed = true;
            }
            s.timing = Some(timing);
        }
    }
    /// Draws `props` from now on: the pump applies them to the displayed worker and
    /// renders the current frame again, reporting [`PreviewEvents::props_applied`]; builds
    /// apply them before their first frame.
    pub fn set_props(&self, props: PreviewProps) {
        let mut s = self.shared.lock();
        if !s.closed {
            if let Some(scan) = &mut s.stop_scan {
                scan.running = false;
                scan.serial += 1;
                scan.suggestions = StopSuggestions::default();
                scan.notice = Some("Props changed; scan again".into());
                s.stop_scan_changed = true;
            }
            s.props = Some(props);
        }
    }
    /// Asks for the props of `identity` (`None`: of nothing) as the displayed preview
    /// draws it; [`PreviewEvents::props_info`] answers, and answers again after the props
    /// change or another build is displayed.
    pub fn want_props_info(&self, identity: Option<EditorObjectIdentity>) {
        let mut s = self.shared.lock();
        if s.closed {
            return;
        }
        let serial = s.props_wanted.as_ref().map_or(0, |(_, serial)| serial + 1);
        s.props_wanted = identity.map(|identity| (identity, serial));
        s.props_info = None;
    }
    /// Called on the coordinator's thread, never under its lock, when news (a compiled or
    /// ready candidate, a frame, a thumbnail, an error) is waiting for [`Self::events`];
    /// once until it is taken.
    pub fn set_listener(&self, listener: impl Fn() + Send + Sync + 'static) {
        *self.listener.lock() = Some(Arc::new(listener));
        self.shared.lock().notified = false;
    }
    pub fn events(&self) -> PreviewEvents {
        let mut s = self.shared.lock();
        s.notified = false;
        PreviewEvents {
            compiled: s.compiled.take(),
            ready: s.ready.take(),
            frame: s.frame.take(),
            thumbnail: s.thumbnail.take(),
            error: s.error.take(),
            retimed: s.retimed.take(),
            props_applied: s.props_applied.take(),
            props_info: s.props_info.take(),
            stop_scan_changed: std::mem::take(&mut s.stop_scan_changed),
        }
    }

    /// Current bounded scan state, never the worker itself.
    pub fn stop_scan(&self) -> Option<StopScan> {
        self.shared.lock().stop_scan.clone()
    }

    /// Starts a fresh explicit scan; calling again supersedes any in-flight batch.
    pub fn scan_stops(&self) {
        let mut s = self.shared.lock();
        if s.closed {
            return;
        }
        if let Some(scan) = &mut s.stop_scan {
            let (frames, limited) = crate::slides::suggest::sample_frames(&scan.timeline);
            scan.serial += 1;
            scan.total = frames.len();
            scan.frames = frames;
            scan.sample_limit = limited;
            scan.completed = 0;
            scan.running = scan.total > 0;
            scan.suggestions = StopSuggestions::default();
            scan.notice = None;
            s.stop_scan_changed = true;
        }
    }

    /// Cancels immediately and discards partial results, including an in-flight reply.
    pub fn cancel_stop_scan(&self) {
        let mut s = self.shared.lock();
        if let Some(scan) = &mut s.stop_scan {
            scan.serial += 1;
            scan.running = false;
            scan.suggestions = StopSuggestions::default();
            scan.notice = Some("Scan cancelled; no slides changed".into());
            s.stop_scan_changed = true;
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
        s.stop_scan = None;
        s.stop_scan_changed = true;
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
            s.stop_scan = None;
            s.thumbnails.clear();
            s.thumbnail = None;
            s.retimed = None;
            s.props_applied = None;
            s.props_info = None;
            s.displayed_scope.take()
        };
        self.seeked.notify_one();
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
        let mut worker = launch_preview_worker(build, preview_identity(&tag), &spec.worker)?;
        let (timing, props) = {
            let s = shared.lock();
            (s.timing.clone(), s.props.clone())
        };
        // Props before the first frame; one the worker refuses is reported by the pump,
        // which tries the latest generation again once the preview is displayed.
        let props_attempt = props
            .filter(|props| apply_props(&mut worker, props).is_ok())
            .map(|props| props.generation);
        let mut staged = prepare_preview(
            worker,
            &tag,
            &spec.compiler,
            &spec.worker,
            &|| shared.lock().desired.clone(),
            None,
            timing,
            &spec.service,
        )?;
        staged.props_attempt = props_attempt;
        Ok(staged)
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
    source: &Arc<crate::PreparedAudioSource>,
) -> Result<(), String> {
    match service.account_artifact_bytes(source.build(), descriptor.byte_count) {
        Ok(guard) => {
            source.hold_accounting(Box::new(guard));
            Ok(())
        }
        Err(crate::services::build_service::BuildError::NotCached) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Prepare a negotiated worker into a complete matching candidate: timeline, boundary and
/// playhead inspection, first frame and PCM window. `prepared_audio` lets a caller that
/// already prepared, verified and accounted the mix (candidate validation) reuse it
/// instead of asking the worker for a second artifact; otherwise the PCM prepared here
/// is accounted in `service`'s budget. `timing` is applied before the timeline is read
/// (it must be `None` with `prepared_audio`, which describes the code order); a worker
/// that cannot play it starts in code order and the pump retries it there, reporting why.
/// Nothing here publishes anything.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_preview(
    mut worker: PreviewWorkerClient,
    tag: &OperationTag,
    compiler: &ProcessTreeManager,
    worker_scope: &ProcessTreeManager,
    desired: &dyn Fn() -> Option<SeekIntent>,
    prepared_audio: Option<(PreparedAudioDescriptor, Arc<crate::PreparedAudioSource>)>,
    timing: Option<PreviewTiming>,
    service: &BuildService,
) -> Result<StagedPreview, String> {
    let mut timing_attempt = None;
    let mut timing_json = None;
    let mut timeline = None;
    if let Some(timing) = timing {
        if worker.supports_timing() {
            if let Ok(applied) = worker.set_timing(timing.json.clone(), timing.generation) {
                timing_attempt = Some(timing.generation);
                timing_json = timing.json;
                timeline = Some(applied);
            }
        } else if timing.json.is_none() {
            timing_attempt = Some(timing.generation);
        }
    }
    let timeline = match timeline {
        Some(timeline) => timeline,
        None => worker.timeline().map_err(|e| e.to_string())?,
    };
    let boundary_inspection = inspect_boundaries(&mut worker, &timeline)?;
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
    let ready = prime(
        &mut worker,
        tag,
        timeline,
        &boundary_inspection,
        audio,
        Some(audio_source),
        position,
        desired.as_ref().map_or(0, |d| d.serial),
        desired.as_ref().map_or(1., |d| d.scale),
    )?;
    if compiler.is_shutdown() || worker_scope.is_shutdown() {
        return Err("Preview preparation cancelled".into());
    }
    Ok(StagedPreview::new(StagedParts {
        worker,
        scope: worker_scope.clone(),
        boundary_inspection,
        ready: Arc::new(ready),
        timing_attempt,
        timing_json,
        props_attempt: None,
        service: service.clone(),
    }))
}

/// Inspects the first and last frame and both edges of every scene; any error refuses
/// the preview.
fn inspect_boundaries(
    worker: &mut PreviewWorkerClient,
    timeline: &PreviewTimelineResponse,
) -> Result<InspectResponse, String> {
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
    Ok(boundary_inspection)
}

/// The parts of a ready preview that depend on the playhead: its inspection, its frame
/// and the PCM window there. `position` is at most the timeline's length.
#[allow(clippy::too_many_arguments)]
fn prime(
    worker: &mut PreviewWorkerClient,
    tag: &OperationTag,
    timeline: PreviewTimelineResponse,
    boundary_inspection: &InspectResponse,
    audio: PreparedAudioDescriptor,
    audio_source: Option<Arc<crate::PreparedAudioSource>>,
    position: usize,
    serial: u64,
    scale: f64,
) -> Result<ReadyPreview, String> {
    let inspection = if timeline.total_frames == 0 {
        boundary_inspection.clone()
    } else {
        inspect_installation_frame(
            worker,
            boundary_inspection,
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
    )?;
    match audio_source {
        Some(source) => ready.with_audio_source(source),
        None => Ok(ready),
    }
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

fn pump(shared: Arc<Mutex<Mailbox>>, seeked: Arc<Condvar>, listener: Listener) {
    let mut displayed: Option<StagedPreview> = None;
    // Compilation, initial preparation and re-priming share one candidate lane.
    // Its blocking worker requests never run on the displayed-worker pump.
    let mut preparation: Option<std::thread::JoinHandle<()>> = None;
    let mut rendered: Option<SeekIntent> = None;
    // The props info last asked of the displayed worker: the ask's serial and the props
    // generation the worker drew then.
    let mut info_asked: Option<(u64, Option<u64>)> = None;
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
            info_asked = None;
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
        let timing = shared.lock().timing.clone();
        if let (Some(c), Some(timing), Some(d)) = (displayed.as_deref_mut(), timing, &desired)
            && c.timing_attempt != Some(timing.generation)
            && c.ready.identity() == &d.identity
        {
            let tag = c.ready.tag().clone();
            let result = retime(c, timing, d)
                .map(|()| c.ready.clone())
                .map_err(|error| (tag, error));
            if result.is_ok() {
                // The retimed preview carries the frame at `d`.
                rendered = Some(d.clone());
            }
            let mut s = shared.lock();
            if !s.closed {
                s.retimed = Some(result);
            }
        }
        let props = shared.lock().props.clone();
        if let (Some(c), Some(props)) = (displayed.as_deref_mut(), props)
            && c.props_attempt != Some(props.generation)
        {
            // Tried once per generation, like a retime.
            c.props_attempt = Some(props.generation);
            let result = apply_props(&mut c.worker, &props).map(|()| props.generation);
            if result.is_ok() {
                // The frame on screen shows the previous props: render it again.
                rendered = None;
            }
            let mut s = shared.lock();
            if !s.closed {
                s.props_applied = Some(result);
            }
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
                    if s.desired.as_ref() != Some(d) {
                        s.metrics.late_frames += 1;
                    }
                    // A late frame of the same preview is still published: the shell shows
                    // it during playback when it is newer than the frame on screen.
                    if s.desired
                        .as_ref()
                        .is_some_and(|now| now.identity == d.identity)
                    {
                        s.frame = Some(f);
                    }
                }
                Err(e) => {
                    let error = displayed_frame_failed(
                        c.worker.backend(),
                        c.worker.has_stopped(),
                        e.to_string(),
                    );
                    shared.lock().error = Some((c.ready.tag().clone(), error));
                }
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
        let wanted = shared.lock().props_wanted.clone();
        if let (Some(c), Some((identity, serial))) = (displayed.as_deref_mut(), wanted)
            && rendered.as_ref() == desired.as_ref()
            && info_asked != Some((serial, c.props_attempt))
        {
            // Asked after the frame rendered, so the answer describes the frame on screen.
            info_asked = Some((serial, c.props_attempt));
            let info = if c.worker.supports_props() {
                c.worker
                    .props_info(identity.clone())
                    .map(|response| response.info)
                    .map_err(|error| error.to_string())
            } else {
                Err(NO_PROPS_WORKER.to_owned())
            };
            let mut s = shared.lock();
            if !s.closed
                && s.props_wanted
                    .as_ref()
                    .is_some_and(|(_, now)| *now == serial)
            {
                s.props_info = Some((identity, info));
            }
        }
        if let Some(c) = displayed.as_deref_mut() {
            scan_batch(c, &shared, rendered.as_ref());
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
                // A key of another timing generation describes another layout; the worker
                // would draw this one's frame under it.
                if key.identity == *c.ready.identity()
                    && key.timing_generation == c.ready.timeline.timing_generation
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
                        s.metrics.thumbnails += 1;
                    }
                }
                shared.lock().active_thumbnail = None;
            }
        }
        let wake = {
            let mut s = shared.lock();
            let wake = s.has_news() && !s.notified && !s.closed;
            s.notified |= wake;
            wake
        };
        if wake && let Some(listener) = listener.lock().clone() {
            listener();
        }
        // A playhead that moved on while the frame rendered is rendered at once; an idle
        // pump waits for the next seek or tick. Checked under the lock `seek` notifies
        // under, so a seek between the check and the wait is not missed.
        let mut s = shared.lock();
        let behind = displayed.as_deref().is_some_and(|c| {
            c.ready.timeline.total_frames > 0
                && s.desired.as_ref().is_some_and(|d| {
                    d.identity == *c.ready.identity() && Some(d) != rendered.as_ref()
                })
        });
        if !behind && !s.closed {
            seeked.wait_for(&mut s, Duration::from_millis(5));
        }
    }
    if let Some(t) = preparation {
        let _ = t.join();
    }
    // The pump is its own background thread: reap the displayed worker here.
    if let Some(displayed) = displayed {
        let _ = displayed.teardown_now();
    }
}

/// One synchronous bounded batch per pump turn, after seeks and before background thumbnails.
fn scan_batch(c: &mut StagedParts, shared: &Arc<Mutex<Mailbox>>, rendered: Option<&SeekIntent>) {
    let batch = {
        let mut s = shared.lock();
        if s.closed
            || s.install.is_some()
            || c.worker.has_stopped()
            || s.timing
                .as_ref()
                .is_some_and(|timing| timing.generation != c.ready.timeline.timing_generation)
            || s.props
                .as_ref()
                .is_some_and(|props| c.props_attempt != Some(props.generation))
        {
            return;
        }
        if !c.worker.supports_suggested_stops() {
            return;
        }
        if s.stop_scan.as_ref().is_none_or(|scan| {
            scan.timeline.envelope.identity != *c.ready.identity()
                || scan.timeline.timing_generation != c.ready.timeline.timing_generation
                || scan.props_generation != c.props_attempt.unwrap_or(0)
        }) {
            s.stop_scan = Some(StopScan::new(
                c.ready.timeline.clone(),
                c.props_attempt.unwrap_or(0),
            ));
            s.stop_scan_changed = true;
        }
        if s.desired.as_ref() != rendered {
            return;
        }
        let scan = s
            .stop_scan
            .as_ref()
            .expect("capable displayed worker has scan state");
        if !scan.running {
            return;
        }
        let end = (scan.completed + MAX_SUGGEST_FRAMES).min(scan.total);
        (
            scan.serial,
            scan.completed,
            end,
            scan.frames[scan.completed..end].to_vec(),
        )
    };
    let (serial, completed, end, frames) = batch;
    let response =
        c.worker
            .suggest_stops(frames, c.ready.timeline.timing_generation, completed == 0);
    let mut s = shared.lock();
    if s.closed || s.install.is_some() {
        return;
    }
    let Some(scan) = &mut s.stop_scan else {
        return;
    };
    if scan.serial != serial
        || !scan.running
        || scan.timeline.envelope.identity != *c.ready.identity()
        || scan.timeline.timing_generation != c.ready.timeline.timing_generation
        || scan.props_generation != c.props_attempt.unwrap_or(0)
    {
        return;
    }
    match response {
        Ok(suggestions) => {
            scan.suggestions = suggestions;
            scan.completed = end;
            scan.running = end < scan.total;
        }
        Err(error) => {
            scan.running = false;
            scan.suggestions = StopSuggestions::default();
            scan.notice = Some(format!("Scan failed: {error}"));
        }
    }
    s.stop_scan_changed = true;
}

/// The error of a frame the displayed worker did not deliver. A worker that rendered on the
/// GPU and was stopped (it missed its deadline or exited) may have hung the GPU: the rest of
/// the session renders with the built-in CPU renderer, from the next preview build on. A
/// frame the worker refused (an error in the video) changes nothing.
fn displayed_frame_failed(backend: &str, stopped: bool, error: String) -> String {
    if stopped && super::renderer_preference::gpu_worker_failed(backend, &error) {
        format!(
            "{error}. The GPU renderer stopped responding; previews of this session render on the CPU from the next build."
        )
    } else {
        error
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
    let ready = prime(
        &mut c.worker,
        c.ready.tag(),
        c.ready.timeline.clone(),
        &c.boundary_inspection,
        c.ready.audio.clone(),
        c.ready.audio_source.clone(),
        d.position.min(c.ready.timeline.total_frames),
        d.serial,
        d.scale,
    )?;
    c.ready = Arc::new(ready);
    Ok(())
}

/// Why a preview cannot show or draw props edits.
const NO_PROPS_WORKER: &str = "This preview's worker cannot edit props: the project does not wire props hooks into its preview worker";

/// Hands `props` to `worker`. A worker without props support draws the code's values,
/// so only an empty document is fine for it.
fn apply_props(worker: &mut PreviewWorkerClient, props: &PreviewProps) -> Result<(), String> {
    if !worker.supports_props() {
        return match props.json {
            None => Ok(()),
            Some(_) => Err(NO_PROPS_WORKER.into()),
        };
    }
    worker
        .set_props(props.json.clone(), props.generation)
        .map_err(|error| error.to_string())
}

/// Lays the displayed worker out with `timing` and primes it at `d`: the new layout's
/// boundaries are inspected and its audio prepared and accounted, then the previous
/// artifact is released. A worker without timing support refuses any document but the
/// code order. On failure the worker keeps (or is told to go back to) the layout of
/// `c.ready`.
fn retime(c: &mut StagedParts, timing: PreviewTiming, d: &SeekIntent) -> Result<(), String> {
    c.timing_attempt = Some(timing.generation);
    if !c.worker.supports_timing() {
        return match timing.json {
            None => Ok(()),
            Some(_) => Err(
                "This preview's worker cannot play timeline edits; build the preview with the current SDK"
                    .into(),
            ),
        };
    }
    let timing_json = timing.json.clone();
    let timeline = c
        .worker
        .set_timing(timing.json, timing.generation)
        .map_err(|e| e.to_string())?;
    let position = d.position.min(timeline.total_frames);
    let mut prepared = None;
    let laid_out = (|| {
        let boundary_inspection = inspect_boundaries(&mut c.worker, &timeline)?;
        let audio = c.worker.prepare_audio(48000).map_err(|e| e.to_string())?;
        prepared = Some(audio.artifact_id.clone());
        let source = c
            .worker
            .retain_audio_source(&audio, || c.scope.is_shutdown())?;
        account_prepared_audio(&c.service, &audio, &source)?;
        let ready = prime(
            &mut c.worker,
            c.ready.tag(),
            timeline,
            &boundary_inspection,
            audio,
            Some(source),
            position,
            d.serial,
            d.scale,
        )?;
        Ok::<_, String>((boundary_inspection, ready))
    })();
    let (boundary_inspection, ready) = match laid_out {
        Ok(laid_out) => laid_out,
        Err(error) => {
            // The worker keeps at most two mixes; the displayed one stays.
            if let Some(artifact_id) = prepared {
                let _ = c.worker.release_audio(artifact_id);
            }
            let _ = c
                .worker
                .set_timing(c.timing_json.clone(), c.ready.timeline.timing_generation);
            return Err(error);
        }
    };
    c.timing_json = timing_json;
    if ready.audio.artifact_id != c.ready.audio.artifact_id {
        // The displayed audio keeps its open file; the worker no longer needs to serve it.
        let _ = c.worker.release_audio(c.ready.audio.artifact_id.clone());
    }
    c.boundary_inspection = boundary_inspection;
    c.ready = Arc::new(ready);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::worker_client::WorkerClient;
    use crate::{OpenSession, OperationId};
    use std::io::{Cursor, empty, sink};

    #[test]
    fn only_a_stopped_gpu_worker_moves_the_session_to_the_cpu_renderer() {
        use crate::services::renderer_preference as preference;
        let _serial = preference::tests_lock();
        preference::reset_for_tests();
        // an error in the video, answered by a running worker
        assert_eq!(
            displayed_frame_failed("skia-gpu", false, "render".into()),
            "render"
        );
        // a slow Skia CPU or tiny-skia worker is not a GPU hang
        assert_eq!(
            displayed_frame_failed("skia-cpu", true, "deadline".into()),
            "deadline"
        );
        assert_eq!(
            displayed_frame_failed("cpu", true, "deadline".into()),
            "deadline"
        );
        assert!(preference::demotion().is_none());
        let error = displayed_frame_failed("skia-gpu", true, "deadline".into());
        assert!(
            error.starts_with("deadline. The GPU renderer stopped responding"),
            "{error}"
        );
        assert_eq!(preference::current(), preference::RendererChoice::Cpu);
        assert_eq!(preference::demotion().as_deref(), Some("deadline"));
        preference::reset_for_tests();
    }

    #[test]
    fn a_gpu_worker_that_stops_answering_a_frame_is_killed_and_the_next_launch_uses_the_cpu() {
        use crate::services::preview_worker_client::PreviewWorkerClient;
        use crate::services::renderer_preference as preference;
        let _serial = preference::tests_lock();
        preference::reset_for_tests();
        preference::set_choice(preference::RendererChoice::Gpu);
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(
            cwd.path().join("fframes-fake-worker.json"),
            br#"{"hang_frame": 3}"#,
        )
        .unwrap();
        let script = std::path::Path::new(studio_test_support::ENGINE_TESTS_DIR)
            .join("support/fake-preview-worker.py");
        let script = script.to_str().unwrap();
        let identity = PreviewIdentity {
            project_id: "p".into(),
            open_session: "s".into(),
            source_revision: "r".into(),
            worker_generation: 1,
        };
        let manager = studio_bootstrap::ProcessTreeManager::new();
        let [flag, renderer] = preference::worker_args();
        let mut transport = WorkerClient::new("r", 1);
        transport.set_request_timeout(std::time::Duration::from_secs(2));
        transport
            .spawn_worker(
                studio_test_support::python(),
                &[
                    script,
                    flag,
                    renderer,
                    "--project-id",
                    "p",
                    "--open-session",
                    "s",
                    "--audio-cache",
                    "audio",
                ],
                Some(cwd.path()),
                studio_bootstrap::ChildEnvironment::default_allowlist(),
                &manager,
            )
            .unwrap();
        let mut worker = PreviewWorkerClient::new(transport, identity);
        worker.negotiate().unwrap();
        assert_eq!(worker.backend(), "skia-gpu");
        worker.timeline().unwrap();
        worker.frame(0, 1, 1.0).expect("frames before the hang");
        let error = worker.frame(3, 2, 1.0).unwrap_err().to_string();
        assert!(
            worker.has_stopped(),
            "the hung worker is killed at the deadline"
        );
        assert_eq!(manager.active_count(), 0);
        let error = displayed_frame_failed(worker.backend(), worker.has_stopped(), error);
        assert!(
            error.contains("The GPU renderer stopped responding"),
            "{error}"
        );
        assert_eq!(preference::worker_args(), ["--renderer", "cpu"]);
        preference::reset_for_tests();
    }

    #[test]
    fn applied_props_change_the_next_frame_and_the_selected_objects_values() {
        use crate::services::preview_worker_client::PreviewWorkerClient;
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(
            cwd.path().join("fframes-fake-worker.json"),
            br#"{"editor_objects": [{"component": "Title", "object": "headline", "bounds": [0, 0, 4, 2]}],
                "props": {"fields": [{"name": "pixel", "kind": "integer", "min": 0, "max": 255}],
                          "values": {"pixel": 7}}}"#,
        )
        .unwrap();
        let script = std::path::Path::new(studio_test_support::ENGINE_TESTS_DIR)
            .join("support/fake-preview-worker.py");
        let manager = studio_bootstrap::ProcessTreeManager::new();
        let mut transport = WorkerClient::new("r", 1);
        transport.set_request_timeout(std::time::Duration::from_secs(5));
        transport
            .spawn_worker(
                studio_test_support::python(),
                &[
                    script.to_str().unwrap(),
                    "--renderer",
                    "cpu",
                    "--project-id",
                    "p",
                    "--open-session",
                    "s",
                    "--audio-cache",
                    "audio",
                ],
                Some(cwd.path()),
                studio_bootstrap::ChildEnvironment::default_allowlist(),
                &manager,
            )
            .unwrap();
        let identity = PreviewIdentity {
            project_id: "p".into(),
            open_session: "s".into(),
            source_revision: "r".into(),
            worker_generation: 1,
        };
        let mut worker = PreviewWorkerClient::new(transport, identity);
        worker.negotiate().unwrap();
        assert!(worker.supports_props());
        worker.timeline().unwrap();
        assert_eq!(worker.frame(0, 1, 1.0).unwrap().pixels[0], 7);

        let headline = EditorObjectIdentity {
            scene_instance_key: "scene-0".into(),
            component_key: "Title".into(),
            object_key: "headline".into(),
            repeat_key: "primary".into(),
        };
        let mut doc = fframes_studio_protocol::PropsDocument::default();
        doc.set(&headline, "pixel", Some(serde_json::json!(40)));
        apply_props(
            &mut worker,
            &PreviewProps {
                json: Some(doc.to_json()),
                generation: 2,
            },
        )
        .unwrap();
        assert_eq!(worker.frame(0, 2, 1.0).unwrap().pixels[0], 40);
        let answer = worker.props_info(headline.clone()).unwrap();
        assert_eq!(answer.props_generation, 2);
        let info = answer.info.expect("the headline is editable");
        assert_eq!(info.schema.component, "Title");
        assert_eq!(info.code_values["pixel"], serde_json::json!(7));
        assert_eq!(info.values["pixel"], serde_json::json!(40));
        let unknown = EditorObjectIdentity {
            object_key: "caption".into(),
            ..headline
        };
        assert_eq!(worker.props_info(unknown).unwrap().info, None);

        // `None` restores the code's values.
        apply_props(
            &mut worker,
            &PreviewProps {
                json: None,
                generation: 3,
            },
        )
        .unwrap();
        assert_eq!(worker.frame(0, 3, 1.0).unwrap().pixels[0], 7);
        worker.shutdown();
    }

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
    fn at_most_twelve_thumbnails_wait_however_many_are_visible() {
        let identity = inspection(vec![], false).envelope.identity;
        let coordinator = PreviewCoordinator::new(ProcessTreeManager::new());
        coordinator.shared.lock().desired = Some(SeekIntent {
            identity: identity.clone(),
            serial: 1,
            position: 0,
            scale: 0.5,
        });
        let visible = (0..40)
            .map(|frame| ThumbnailKey::new(identity.clone(), 0, 0.25, frame))
            .collect();
        coordinator.thumbnails(visible);
        assert_eq!(coordinator.shared.lock().thumbnails.len(), 12);
        assert_eq!(coordinator.metrics().thumbnail_queue_high_water, 12);
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
