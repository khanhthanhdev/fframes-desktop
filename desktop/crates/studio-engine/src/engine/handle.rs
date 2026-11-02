//! The window's handle on the engine: commands go in on one thread, events come out on one
//! ordered channel.
//!
//! Every job (a command, a presentation refresh) runs on the engine's own thread, one at a
//! time and in the order it was sent, and reports back as one [`EngineEvent`]. The UI never
//! locks the backend or a controller: the one synchronous exception is
//! [`EngineHandle::with_install_fence`], which never blocks.
//!
//! A monitor thread watches what nobody sends a command about: the open project's source
//! changing on disk ([`EngineEvent::SourceChanged`]) and background cleanup failing
//! ([`EngineEvent::CleanupProblem`]).
//!
//! An open agent workflow says it published a snapshot with [`EngineEvent::WorkflowChanged`],
//! stamped with its session, so a window that left the session never hears it.

use super::backend::{Backend, Command, Presentation, StudioEnvironment};
use crate::agent::{AgentWorkflow, BuildSettings};
use crate::containment::OwnershipPolicy;
use crate::deck::{DeckRequest, DeckStop, export_deck};
use crate::presets::{Catalog, PresetReport};
use crate::providers::host::{HostInbox, WorkflowWake};
use crate::services::build_service::BuildService;
use crate::services::export_service::{
    ExportControl, ExportProgress, ExportRequest, RenderRequest, export_video,
    probe_export_capabilities,
};
use crate::services::preview_coordinator::BuildSpec;
use crate::services::teardown::{Teardown, TeardownReport};
use crate::{JobResult, OperationTag, ProjectState, ReadyPreview};
use fframes_studio_protocol::export::{ExportCapabilities, ExportSettings};
use parking_lot::Mutex;
use std::{
    path::PathBuf,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};
use studio_proto::{CommandId, SessionId};

/// What a finished command leaves for the window.
pub struct CommandFinished {
    pub id: CommandId,
    /// The project session open after the command, whether or not its presentation could
    /// be taken.
    pub session: Option<SessionId>,
    pub result: Result<(), String>,
    /// The presentation taken right after the command (`None` when that failed; the
    /// failure is then the result).
    pub presentation: Option<Presentation>,
    /// A preview build the command asked for.
    pub build: Option<BuildSpec>,
    /// The outcome of a preset command.
    pub preset: Option<PresetReport>,
}

/// What the preview coordinator reported that the open project must hear: a compiled
/// build, a failed one, a ready candidate, the displayed preview laid out again.
#[derive(Default)]
pub struct PreviewNews {
    pub compiled: Option<OperationTag>,
    pub error: Option<(OperationTag, String)>,
    pub ready: Option<Arc<ReadyPreview>>,
    /// The displayed preview after a timing edit, as the window installed it: its
    /// timeline is the one the person sees, which the slide stops resolve against.
    pub retimed: Option<Arc<ReadyPreview>>,
}

/// The open project after hearing [`PreviewNews`].
pub struct PreviewGated {
    pub session: Option<SessionId>,
    /// The ready candidate that was reported, to install if `state` allows it.
    pub ready: Option<Arc<ReadyPreview>>,
    pub state: Option<ProjectState>,
    /// The build that failed, or the news the project refused.
    pub error: Option<(OperationTag, String)>,
    pub presentation: Option<Presentation>,
}

pub enum EngineEvent {
    /// A command finished. Never filtered by session: it is what moves a window to the
    /// session its presentation shows.
    CommandFinished(Box<CommandFinished>),
    /// A fresh presentation taken by [`EngineHandle::refresh`].
    Refreshed {
        session: Option<SessionId>,
        presentation: Option<Box<Presentation>>,
    },
    /// The answer to [`EngineHandle::preview_news`].
    PreviewGated(Box<PreviewGated>),
    /// The open project's source changed on disk; a [`Command::Refresh`] reconciles it.
    /// Repeated while it stays unreconciled.
    SourceChanged { session: Option<SessionId> },
    /// Background cleanup could not finish (the newest problem). Never filtered.
    CleanupProblem(TeardownReport),
    /// The agent workflow opened with `serial` published a snapshot. Not repeated until its
    /// [`WorkflowWake`] (in [`crate::providers::host::Opened`]) is taken.
    WorkflowChanged { session: SessionId, serial: u64 },
}

impl EngineEvent {
    /// The project session a session-scoped event belongs to; `None` for a finished
    /// command, which is never filtered.
    pub fn session(&self) -> Option<Option<SessionId>> {
        match self {
            EngineEvent::CommandFinished(_) => None,
            EngineEvent::Refreshed { session, .. } => Some(*session),
            EngineEvent::PreviewGated(gated) => Some(gated.session),
            EngineEvent::SourceChanged { session } => Some(*session),
            EngineEvent::CleanupProblem(_) => None,
            EngineEvent::WorkflowChanged { session, .. } => Some(Some(*session)),
        }
    }
}

/// What opening the agent workflow of a session needs from the window.
pub struct WorkflowOpen {
    pub session: SessionId,
    /// The window's open serial, carried by the result.
    pub serial: u64,
    pub build: Option<BuildSettings>,
    /// Receives the result and the workflow's hand-offs; its snapshots arrive as
    /// [`EngineEvent::WorkflowChanged`].
    pub inbox: Arc<HostInbox>,
    pub policy: OwnershipPolicy,
}

/// A running MP4 export.
pub struct ExportJob {
    pub control: Arc<ExportControl>,
    /// The export's processes; shutting them down stops it.
    pub processes: studio_bootstrap::ProcessTreeManager,
    /// Progress, ending with a terminal event (or closed if the export thread died).
    pub progress: async_channel::Receiver<ExportProgress>,
}

enum Job {
    Command {
        id: CommandId,
        command: Command,
        /// The workflow to close before the command runs (it shares the controller the
        /// command may close).
        closing: Option<Arc<AgentWorkflow>>,
    },
    Refresh,
    PreviewNews(PreviewNews),
    OpenWorkflow(Box<WorkflowOpen>),
}

struct Inner {
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    backend: Arc<Mutex<Backend>>,
    closed: Arc<AtomicBool>,
    processes: studio_bootstrap::ProcessTreeManager,
    next_command: AtomicU64,
}

/// Cheap to clone; every clone drives the same engine.
#[derive(Clone)]
pub struct EngineHandle {
    inner: Arc<Inner>,
}

impl EngineHandle {
    /// Starts the engine thread. `app_version` is the running app's version that SDKs must
    /// be compatible with.
    pub fn new(
        environment: StudioEnvironment,
        app_version: &'static str,
    ) -> (Self, async_channel::Receiver<EngineEvent>) {
        let closed = Arc::new(AtomicBool::new(false));
        let processes = studio_bootstrap::ProcessTreeManager::new();
        let backend = Arc::new(Mutex::new(Backend {
            paths: None,
            controller: None,
            session: None,
            sessions: 0,
            compatibility: None,
            compatibility_error: "SDK manifest unavailable".into(),
            sdk_home: None,
            sdk: "Checking SDK…".into(),
            agent_sdk: None,
            sdk_upgrade: None,
            sdk_update: None,
            upgrade_check: None,
            selected_sdk: None,
            closed: closed.clone(),
            processes: processes.clone(),
            build_spec: None,
            catalog: Catalog::default(),
            preset_report: None,
            environment,
            app_version,
        }));
        let (events, receiver) = async_channel::unbounded();
        let (jobs, queue) = mpsc::channel();
        let inner = Arc::new(Inner {
            jobs: Mutex::new(Some(jobs)),
            backend,
            closed,
            processes,
            next_command: AtomicU64::new(0),
        });
        let worker = Arc::downgrade(&inner);
        let monitored = Arc::downgrade(&inner);
        let monitor_events = events.clone();
        std::thread::Builder::new()
            .name("studio-engine".into())
            .spawn(move || run(worker, queue, events))
            .expect("start the engine thread");
        std::thread::Builder::new()
            .name("studio-engine-monitor".into())
            .spawn(move || monitor(monitored, monitor_events))
            .expect("start the engine monitor");
        (Self { inner }, receiver)
    }

    /// Queues `command`; its [`EngineEvent::CommandFinished`] carries the returned id.
    pub fn send(&self, command: Command, closing: Option<Arc<AgentWorkflow>>) -> CommandId {
        let id = CommandId(self.inner.next_command.fetch_add(1, Ordering::Relaxed) + 1);
        self.queue(Job::Command {
            id,
            command,
            closing,
        });
        id
    }

    /// Queues a fresh presentation of the open project ([`EngineEvent::Refreshed`]).
    pub fn refresh(&self) {
        self.queue(Job::Refresh);
    }

    /// Tells the open project what its preview build reported: a compiled build and a
    /// failure complete its job, a ready candidate reconciles the source
    /// ([`EngineEvent::PreviewGated`]).
    pub fn preview_news(&self, news: PreviewNews) {
        self.queue(Job::PreviewNews(news));
    }

    /// Opens the agent workflow of `open.session` off the engine thread (replaying its
    /// conversation log) and pushes the result into `open.inbox`. Nothing is pushed when
    /// that session is no longer open: the window has left it.
    pub fn open_workflow(&self, open: WorkflowOpen) {
        self.queue(Job::OpenWorkflow(Box::new(open)));
    }

    /// Exports the accepted revision of `session` to `destination` on its own thread. The
    /// project is held only while its source is frozen, never while rendering.
    pub fn start_export(
        &self,
        session: SessionId,
        destination: PathBuf,
        settings: ExportSettings,
        sdk: PathBuf,
        service: BuildService,
    ) -> Result<ExportJob, String> {
        let control = Arc::new(ExportControl::default());
        let processes = studio_bootstrap::ProcessTreeManager::new();
        let (sender, progress) = async_channel::bounded(16);
        let backend = self.inner.backend.clone();
        let job = ExportJob {
            control: control.clone(),
            processes: processes.clone(),
            progress,
        };
        std::thread::Builder::new()
            .name("studio-mp4-export".into())
            .spawn(move || {
                let send = |event| {
                    let _ = sender.send_blocking(event);
                };
                let project = {
                    let backend = backend.lock();
                    (backend.session == Some(session))
                        .then(|| backend.controller.clone().zip(backend.paths.clone()))
                        .flatten()
                };
                let Some((controller, paths)) = project else {
                    send(ExportProgress::Failed(
                        "The project closed before the export started".into(),
                    ));
                    return;
                };
                let prepared = {
                    let controller = controller.lock();
                    controller
                        .freeze_export_source()
                        .map_err(|error| error.to_string())
                        .and_then(|source| {
                            let timing = controller.timing_for_export()?;
                            let props = controller.props_for_export()?;
                            Ok((source, timing, props))
                        })
                };
                drop(controller);
                let result = prepared.and_then(|(source, timing_json, props_json)| {
                    export_video(
                        ExportRequest {
                            source,
                            sdk,
                            builds: paths.builds(),
                            destination,
                            settings,
                            timing_json,
                            props_json,
                        },
                        service,
                        processes,
                        control.clone(),
                        send,
                    )
                });
                if let Err(error) = result {
                    send(if control.is_cancelled() {
                        ExportProgress::Cancelled
                    } else {
                        ExportProgress::Failed(error)
                    });
                }
            })
            .map_err(|error| format!("Could not start the export job: {error}"))?;
        Ok(job)
    }

    /// Exports the accepted revision of `session` as a slide deck folder `destination`
    /// (which must not exist) on its own thread. The stops resolve against the engine's
    /// latest laid-out timeline, which must belong to the frozen build and the exported
    /// timing; otherwise the export refuses with "Wait for the preview to update". One
    /// [`ExportControl`] covers rendering, posters and the folder's publication.
    pub fn start_deck_export(
        &self,
        session: SessionId,
        destination: PathBuf,
        settings: ExportSettings,
        sdk: PathBuf,
        service: BuildService,
    ) -> Result<ExportJob, String> {
        let control = Arc::new(ExportControl::default());
        let processes = studio_bootstrap::ProcessTreeManager::new();
        let (sender, progress) = async_channel::bounded(16);
        let backend = self.inner.backend.clone();
        let job = ExportJob {
            control: control.clone(),
            processes: processes.clone(),
            progress,
        };
        std::thread::Builder::new()
            .name("studio-deck-export".into())
            .spawn(move || {
                let send = |event| {
                    let _ = sender.send_blocking(event);
                };
                let project = {
                    let backend = backend.lock();
                    (backend.session == Some(session))
                        .then(|| backend.controller.clone().zip(backend.paths.clone()))
                        .flatten()
                };
                let Some((controller, paths)) = project else {
                    send(ExportProgress::Failed(
                        "The project closed before the export started".into(),
                    ));
                    return;
                };
                let prepared = freeze_deck(&controller.lock());
                drop(controller);
                let result = prepared.and_then(|frozen| {
                    export_deck(
                        DeckRequest {
                            render: RenderRequest {
                                source: frozen.source,
                                sdk,
                                builds: paths.builds(),
                                settings,
                                timing_json: frozen.timing_json,
                                props_json: frozen.props_json,
                            },
                            destination,
                            title: frozen.title,
                            stops: frozen.stops,
                        },
                        &service,
                        &processes,
                        &control,
                        &mut |event| send(event),
                    )
                });
                if let Err(error) = result {
                    send(if control.is_cancelled() {
                        ExportProgress::Cancelled
                    } else {
                        ExportProgress::Failed(error)
                    });
                }
            })
            .map_err(|error| format!("Could not start the export job: {error}"))?;
        Ok(job)
    }

    /// Asks the worker of the accepted revision of `session` what it can export, on its own
    /// thread: the build is the preview's when the project is unchanged. The answer arrives
    /// once; the channel closes without one when the project closed first.
    pub fn probe_export_capabilities(
        &self,
        session: SessionId,
        sdk: PathBuf,
        service: BuildService,
    ) -> Result<async_channel::Receiver<Result<ExportCapabilities, String>>, String> {
        let (sender, receiver) = async_channel::bounded(1);
        let backend = self.inner.backend.clone();
        std::thread::Builder::new()
            .name("studio-export-capabilities".into())
            .spawn(move || {
                let project = {
                    let backend = backend.lock();
                    (backend.session == Some(session))
                        .then(|| backend.controller.clone().zip(backend.paths.clone()))
                        .flatten()
                };
                let Some((controller, paths)) = project else {
                    return;
                };
                let source = controller.lock().freeze_export_source();
                drop(controller);
                let result = source
                    .map_err(|error| error.to_string())
                    .and_then(|source| {
                        probe_export_capabilities(
                            &source,
                            &sdk,
                            &paths.builds(),
                            &service,
                            &studio_bootstrap::ProcessTreeManager::new(),
                        )
                    });
                let _ = sender.send_blocking(result);
            })
            .map_err(|error| format!("Could not start the export probe: {error}"))?;
        Ok(receiver)
    }

    fn queue(&self, job: Job) {
        if let Some(jobs) = &*self.inner.jobs.lock() {
            let _ = jobs.send(job);
        }
    }

    /// Set once the window closes: queued commands then finish with an error and a running
    /// asset copy stops.
    pub fn closed(&self) -> &Arc<AtomicBool> {
        &self.inner.closed
    }

    /// The process scope of the open project's builds and tools.
    pub fn processes(&self) -> &studio_bootstrap::ProcessTreeManager {
        &self.inner.processes
    }

    /// The nonblocking source fence for committing a preview: runs `commit` against the
    /// open project's current state while holding its controller, so no publication or
    /// reconcile interleaves. `None` (deferred, not refused) while the engine or the
    /// controller is busy, a source scan is pending, or no project is open.
    pub fn with_install_fence<R>(&self, commit: impl FnOnce(&ProjectState) -> R) -> Option<R> {
        let backend = self.inner.backend.try_lock()?;
        let controller = backend.controller.clone()?;
        let fence = super::fence::install_fence(&controller)?;
        Some(commit(fence.state()))
    }

    /// Stops taking commands and closes the open project, waiting at most `patience` for a
    /// command or a job that holds it. A durable job that is still running recovers as
    /// interrupted at the next open.
    pub fn shutdown(&self, patience: Duration) {
        self.inner.closed.store(true, Ordering::Release);
        self.inner.jobs.lock().take();
        if let Some(mut backend) = self.inner.backend.try_lock_for(patience)
            && let Some(controller) = backend.controller.take()
            && let Some(mut controller) = controller.try_lock_for(patience)
        {
            let _ = controller.close();
        }
    }
}

/// Why a deck cannot use the slides the engine resolved.
const WAIT_FOR_PREVIEW: &str = "Wait for the preview to update";

/// What a deck export takes from the open project, all under one lock.
struct FrozenDeck {
    source: crate::FrozenExportSource,
    timing_json: Option<String>,
    props_json: Option<String>,
    title: String,
    stops: Vec<DeckStop>,
}

/// Freezes the export source with its timing and props, and resolves the stops against
/// the latest laid-out timeline, which must be the frozen build's (the preview of
/// another build lays out other scenes) and the exported timing's (checked by
/// [`crate::Controller::resolved_slides`]).
fn freeze_deck(controller: &crate::Controller) -> Result<FrozenDeck, String> {
    let source = controller
        .freeze_export_source()
        .map_err(|error| error.to_string())?;
    let timing_json = controller.timing_for_export()?;
    let props_json = controller.props_for_export()?;
    let state = controller.state();
    let frozen_build = source.project().inventory.build_revision();
    if state.built() != Some(state.build_source()) || frozen_build != *state.build_source() {
        return Err(format!(
            "{WAIT_FOR_PREVIEW}: the slides are laid out on another build than the exported one"
        ));
    }
    let (stops, _skipped) = controller
        .resolved_slides()
        .map_err(|reason| format!("{WAIT_FOR_PREVIEW}: {reason}"))?;
    if stops.is_empty() {
        return Err(format!("{WAIT_FOR_PREVIEW}: no slide stop resolved"));
    }
    Ok(FrozenDeck {
        title: source.project().manifest.display.name.clone(),
        source,
        timing_json,
        props_json,
        stops: stops
            .into_iter()
            .map(|stop| DeckStop {
                video_frame: stop.video_frame,
                title: stop.title,
                notes: stop.notes,
            })
            .collect(),
    })
}

fn run(
    engine: Weak<Inner>,
    queue: mpsc::Receiver<Job>,
    events: async_channel::Sender<EngineEvent>,
) {
    while let Ok(job) = queue.recv() {
        let Some(engine) = engine.upgrade() else {
            return;
        };
        let event = Some(match job {
            Job::Command {
                id,
                command,
                closing,
            } => {
                if let Some(workflow) = closing {
                    workflow.close();
                }
                EngineEvent::CommandFinished(Box::new(execute(&engine, id, command)))
            }
            Job::Refresh => {
                let backend = engine.backend.lock();
                EngineEvent::Refreshed {
                    session: backend.session,
                    presentation: backend.presentation().ok().map(Box::new),
                }
            }
            Job::PreviewNews(news) => EngineEvent::PreviewGated(Box::new(gate(&engine, news))),
            Job::OpenWorkflow(open) => {
                open_workflow(&engine, *open, &events);
                drop(engine);
                continue;
            }
        });
        drop(engine);
        if let Some(event) = event
            && events.send_blocking(event).is_err()
        {
            return;
        }
    }
}

fn open_workflow(engine: &Inner, open: WorkflowOpen, events: &async_channel::Sender<EngineEvent>) {
    let project = {
        let backend = engine.backend.lock();
        (backend.session == Some(open.session))
            .then(|| backend.controller.clone().zip(backend.paths.clone()))
            .flatten()
    };
    let Some((controller, paths)) = project else {
        return;
    };
    let WorkflowOpen {
        session,
        serial,
        build,
        inbox,
        policy,
    } = open;
    let events = events.clone();
    let wake = WorkflowWake::new(move || {
        let _ = events.try_send(EngineEvent::WorkflowChanged { session, serial });
    });
    let replay = move || {
        let opened = crate::providers::host::open_workflow(
            controller,
            paths,
            build,
            serial,
            inbox.clone(),
            wake,
            policy,
        );
        inbox.push_opened(opened);
    };
    // The replay never runs on the engine thread unless no thread can be started.
    type Replay = Box<dyn FnOnce() + Send>;
    let replay: Arc<Mutex<Option<Replay>>> = Arc::new(Mutex::new(Some(Box::new(replay))));
    let spawned = std::thread::Builder::new()
        .name("studio-agent-open".into())
        .spawn({
            let replay = replay.clone();
            move || {
                if let Some(replay) = replay.lock().take() {
                    replay();
                }
            }
        });
    if spawned.is_err()
        && let Some(replay) = replay.lock().take()
    {
        replay();
    }
}

/// How often the monitor looks at the source hint and the cleanup reports.
const MONITOR_PERIOD: Duration = Duration::from_millis(750);

fn monitor(engine: Weak<Inner>, events: async_channel::Sender<EngineEvent>) {
    loop {
        std::thread::sleep(MONITOR_PERIOD);
        let Some(engine) = engine.upgrade() else {
            return;
        };
        if engine.closed.load(Ordering::Acquire) {
            return;
        }
        let mut news = Vec::new();
        // Never waits: a running command or job holds them, and looks again next time.
        if let Some(backend) = engine.backend.try_lock()
            && let Some(controller) = &backend.controller
            && controller.try_lock().is_some_and(|c| c.changed_hint())
        {
            news.push(EngineEvent::SourceChanged {
                session: backend.session,
            });
        }
        if let Some(report) = Teardown::global().take_reports().pop() {
            news.push(EngineEvent::CleanupProblem(report));
        }
        drop(engine);
        for event in news {
            if events.send_blocking(event).is_err() {
                return;
            }
        }
    }
}

fn gate(engine: &Inner, news: PreviewNews) -> PreviewGated {
    let backend = engine.backend.lock();
    let mut error = None;
    let state = backend.controller.as_ref().map(|controller| {
        let mut c = controller.lock();
        if let Some(tag) = news.compiled
            && let Err(e) = c.complete(&tag, JobResult::Built(tag.base_source.clone()))
        {
            error = Some((tag, e.to_string()));
        }
        if let Some((tag, e)) = news.error {
            if c.state().active_tag() == Some(&tag) {
                let _ = c.complete(&tag, JobResult::Failed(e.clone()));
            }
            error = Some((tag, e));
        }
        if let Some(ready) = &news.ready {
            if let Err(e) = c.reconcile() {
                error = Some((ready.tag().clone(), e.to_string()));
            }
            // The scenes the build reports are what the timeline edits.
            c.learn_timeline(ready.tag(), &ready.timeline);
        }
        if let Some(retimed) = &news.retimed {
            // Only builds reach the engine otherwise: without this the slide stops
            // would resolve against the layout before the timing edit.
            c.learn_timeline(retimed.tag(), &retimed.timeline);
        }
        c.state().clone()
    });
    if error.is_none()
        && let (Some(paths), Some(state), Some(ready)) = (&backend.paths, &state, &news.ready)
        && (state.is_current_build(ready.tag())
            || state.promotion().is_some_and(|p| p.tag() == ready.tag()))
        && let Some(frame) = &ready.frame
    {
        // A poster is disposable app data: a cache failure must not fail a good build.
        let _ = paths.save_project_preview(state.project(), frame);
    }
    PreviewGated {
        session: backend.session,
        ready: news.ready,
        state,
        error,
        presentation: backend.presentation().ok(),
    }
}

fn execute(engine: &Inner, id: CommandId, command: Command) -> CommandFinished {
    let mut backend = engine.backend.lock();
    if engine.closed.load(Ordering::Acquire) {
        return CommandFinished {
            id,
            session: backend.session,
            result: Err("Window closed".into()),
            presentation: None,
            build: None,
            preset: None,
        };
    }
    let result = backend.command(command);
    let presentation = backend.presentation();
    let build = backend.build_spec.take();
    let preset = backend.preset_report.take();
    match presentation {
        Ok(presentation) => CommandFinished {
            id,
            session: backend.session,
            result,
            presentation: Some(presentation),
            build,
            preset,
        },
        Err(error) => CommandFinished {
            id,
            session: backend.session,
            result: Err(error),
            presentation: None,
            build: None,
            preset,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_materialization::sdk_pin;
    use studio_sdk::CompatibilityManifest;

    fn next(events: &async_channel::Receiver<EngineEvent>) -> EngineEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(event) = events.try_recv() {
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "engine event deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn preview_news_is_answered_with_the_open_session_and_its_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("video");
        let compatibility = CompatibilityManifest::default_linux_x64();
        studio_project::create(
            &root,
            "Video",
            sdk_pin(&compatibility),
            &compatibility.fframes_version,
            &compatibility.runtime_version,
        )
        .unwrap();
        let mut environment = StudioEnvironment::system();
        environment.app_data = Some(temp.path().join("data"));
        environment.sdk_home = Some(temp.path().join("sdk"));
        let (engine, events) = EngineHandle::new(environment, "0.0.0");

        engine.preview_news(PreviewNews::default());
        let EngineEvent::PreviewGated(closed) = next(&events) else {
            panic!("a gated preview");
        };
        assert_eq!(closed.session, None);
        assert!(closed.state.is_none() && closed.error.is_none());

        engine.send(Command::Initialize, None);
        let opened = engine.send(Command::Open(root), None);
        let finished = loop {
            if let EngineEvent::CommandFinished(finished) = next(&events)
                && finished.id == opened
            {
                break finished;
            }
        };
        assert_eq!(finished.result, Ok(()));
        let session = finished.presentation.unwrap().session;
        assert!(session.is_some());

        engine.preview_news(PreviewNews::default());
        // Opening also queues status and presentation events; the answer may follow them.
        let open = loop {
            if let EngineEvent::PreviewGated(open) = next(&events) {
                break open;
            }
        };
        assert_eq!(open.session, session);
        assert!(open.state.is_some() && open.error.is_none() && open.presentation.is_some());
        engine.shutdown(Duration::from_secs(5));
    }

    #[test]
    fn an_external_source_change_is_reported_for_the_open_session() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("video");
        let compatibility = CompatibilityManifest::default_linux_x64();
        studio_project::create(
            &root,
            "Video",
            sdk_pin(&compatibility),
            &compatibility.fframes_version,
            &compatibility.runtime_version,
        )
        .unwrap();
        let mut environment = StudioEnvironment::system();
        environment.app_data = Some(temp.path().join("data"));
        environment.sdk_home = Some(temp.path().join("sdk"));
        let (engine, events) = EngineHandle::new(environment, "0.0.0");
        engine.send(Command::Initialize, None);
        let opened = engine.send(Command::Open(root.clone()), None);
        let session = loop {
            if let EngineEvent::CommandFinished(finished) = next(&events)
                && finished.id == opened
            {
                break finished.presentation.unwrap().session;
            }
        };
        std::fs::write(root.join("src/lib.rs"), "// external write").unwrap();
        loop {
            if let EngineEvent::SourceChanged { session: changed } = next(&events) {
                assert_eq!(changed, session);
                break;
            }
        }
        engine.shutdown(Duration::from_secs(5));
    }
}

#[cfg(test)]
mod workflow_and_export_tests {
    use super::*;
    use crate::build_materialization::sdk_pin;
    use studio_sdk::CompatibilityManifest;

    fn opened_engine(
        temp: &tempfile::TempDir,
    ) -> (
        EngineHandle,
        async_channel::Receiver<EngineEvent>,
        Option<SessionId>,
    ) {
        let root = temp.path().join("video");
        let compatibility = CompatibilityManifest::default_linux_x64();
        studio_project::create(
            &root,
            "Video",
            sdk_pin(&compatibility),
            &compatibility.fframes_version,
            &compatibility.runtime_version,
        )
        .unwrap();
        let mut environment = StudioEnvironment::system();
        environment.app_data = Some(temp.path().join("data"));
        environment.sdk_home = Some(temp.path().join("sdk"));
        let (engine, events) = EngineHandle::new(environment, "0.0.0");
        engine.send(Command::Initialize, None);
        let opened = engine.send(Command::Open(root), None);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let session = loop {
            assert!(std::time::Instant::now() < deadline, "open deadline");
            if let Ok(EngineEvent::CommandFinished(finished)) = events.try_recv()
                && finished.id == opened
            {
                break finished.presentation.unwrap().session;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        (engine, events, session)
    }

    #[test]
    fn a_workflow_opens_only_for_the_session_still_open() {
        let temp = tempfile::tempdir().unwrap();
        let (engine, _events, session) = opened_engine(&temp);
        let inbox = Arc::new(HostInbox::default());
        let open = |session, serial| WorkflowOpen {
            session,
            serial,
            build: None,
            inbox: inbox.clone(),
            policy: OwnershipPolicy::Validated,
        };
        engine.open_workflow(open(SessionId(999), 1));
        engine.open_workflow(open(session.unwrap(), 2));
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let opened = loop {
            let opened = inbox.take_opened();
            if !opened.is_empty() {
                break opened;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "workflow open deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(opened.iter().map(|o| o.serial).collect::<Vec<_>>(), [2]);
        for opened in opened {
            if let Ok(workflow) = opened.result {
                workflow.close();
            }
        }
        engine.shutdown(Duration::from_secs(5));
    }

    #[test]
    fn a_workflow_change_is_one_event_of_its_session_until_it_is_taken() {
        let temp = tempfile::tempdir().unwrap();
        let (engine, events, session) = opened_engine(&temp);
        let inbox = Arc::new(HostInbox::default());
        engine.open_workflow(WorkflowOpen {
            session: session.unwrap(),
            serial: 4,
            build: None,
            inbox: inbox.clone(),
            policy: OwnershipPolicy::Validated,
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let opened = loop {
            if let Some(opened) = inbox.take_opened().pop() {
                break opened;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "workflow open deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        let changes = || {
            let mut changes = Vec::new();
            while let Ok(event) = events.try_recv() {
                if let EngineEvent::WorkflowChanged { session, serial } = event {
                    changes.push((session, serial));
                }
            }
            changes
        };
        // The wake outlives its workflow; closing it first leaves no publish racing the
        // assertions. Then drop whatever the workflow published.
        if let Ok(workflow) = opened.result {
            workflow.close();
        }
        changes();
        opened.wake.take();
        for _ in 0..5 {
            opened.wake.wake();
        }
        assert_eq!(changes(), [(session.unwrap(), 4)]);
        opened.wake.wake();
        assert!(changes().is_empty(), "nothing more until it is taken");
        assert!(opened.wake.take());
        opened.wake.wake();
        assert_eq!(changes(), [(session.unwrap(), 4)]);
        engine.shutdown(Duration::from_secs(5));
    }

    #[test]
    fn an_export_of_a_session_no_longer_open_fails_with_a_reason() {
        let temp = tempfile::tempdir().unwrap();
        let (engine, _events, session) = opened_engine(&temp);
        let job = engine
            .start_export(
                SessionId(session.unwrap().0 + 1),
                temp.path().join("out.mp4"),
                ExportSettings::default(),
                temp.path().join("sdk"),
                crate::services::worker_project::shared_build_service(),
            )
            .unwrap();
        match job.progress.recv_blocking() {
            Ok(ExportProgress::Failed(reason)) => assert!(reason.contains("closed")),
            _ => panic!("a failure"),
        }
        assert!(job.progress.recv_blocking().is_err(), "nothing after it");
        engine.shutdown(Duration::from_secs(5));
    }

    #[test]
    fn a_deck_export_refuses_without_a_built_preview_and_leaves_no_folder() {
        let temp = tempfile::tempdir().unwrap();
        let (engine, _events, session) = opened_engine(&temp);
        let destination = temp.path().join("Slides");
        let settings = ExportSettings {
            codec: fframes_studio_protocol::export::ExportCodec::H264,
            ..ExportSettings::default()
        };
        let closed = engine
            .start_deck_export(
                SessionId(session.unwrap().0 + 1),
                destination.clone(),
                settings,
                temp.path().join("sdk"),
                crate::services::worker_project::shared_build_service(),
            )
            .unwrap();
        match closed.progress.recv_blocking() {
            Ok(ExportProgress::Failed(reason)) => assert!(reason.contains("closed")),
            _ => panic!("a failure"),
        }
        let unbuilt = engine
            .start_deck_export(
                session.unwrap(),
                destination.clone(),
                settings,
                temp.path().join("sdk"),
                crate::services::worker_project::shared_build_service(),
            )
            .unwrap();
        match unbuilt.progress.recv_blocking() {
            Ok(ExportProgress::Failed(reason)) => assert!(reason.contains("build"), "{reason}"),
            _ => panic!("a failure"),
        }
        assert!(
            unbuilt.progress.recv_blocking().is_err(),
            "nothing after it"
        );
        assert!(!destination.exists());
        engine.shutdown(Duration::from_secs(5));
    }
}
