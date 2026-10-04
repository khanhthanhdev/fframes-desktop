use crate::{
    audio_service::{AudioEvent, AudioService, OutputDevice, OutputHandle},
    frame_image::{ImagePresentationManager, create_render_image},
    preview_coordinator::{BuildSpec, PreviewCoordinator, SeekIntent},
    timeline_view::{TimelineEvent, TimelineView},
};
use crate::{project_view::ProjectPresentation, setup_view::SetupView};
use gpui::{
    AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, canvas, div, px, rgb,
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use studio_engine::{
    Controller, app_paths::AppPaths, build_materialization::sdk_pin, store::Store,
};
use studio_project::{ProjectId, manifest::CargoEntry};
use studio_sdk::{CompatibilityManifest, Doctor};

const BACKGROUND: u32 = 0x10151e;
const PANEL: u32 = 0x19212e;
const BORDER: u32 = 0x303c4d;
const TEXT: u32 = 0xe3eaf3;
const MUTED: u32 = 0x9aaabd;
const ACCENT: u32 = 0x78b7fa;

#[derive(Clone)]
struct Recent {
    id: ProjectId,
    name: String,
    path: PathBuf,
    missing: bool,
}
#[derive(Clone, Default)]
struct Presentation {
    project: Option<ProjectPresentation>,
    recents: Vec<Recent>,
    sdk: String,
    state: Option<studio_engine::ProjectState>,
    owner: Option<studio_bootstrap::ProcessTreeManager>,
}

enum Command {
    Initialize,
    Create(PathBuf),
    Open(PathBuf),
    Locate(PathBuf, ProjectId),
    Import(PathBuf),
    Asset(PathBuf),
    Refresh,
    Checkpoint,
    Build,
    CancelBuild,
    Close,
    RemoveRecent(ProjectId),
    SelectSdk(PathBuf),
    InstallSdk,
    Restore(PathBuf, studio_project::SourceRevision),
    Independent(PathBuf),
}
enum Picker {
    Create,
    Restore,
    Open,
    Import,
    Asset,
    Sdk,
    Locate(ProjectId),
}
struct Backend {
    paths: Option<AppPaths>,
    controller: Option<Controller>,
    compatibility: Option<CompatibilityManifest>,
    sdk_home: Option<PathBuf>,
    sdk: String,
    closed: Arc<AtomicBool>,
    processes: studio_bootstrap::ProcessTreeManager,
    build_spec: Option<BuildSpec>,
}
impl Backend {
    fn command(&mut self, command: Command) -> Result<(), String> {
        if matches!(command, Command::Initialize) {
            self.paths = Some(AppPaths::system().map_err(|e| e.to_string())?);
            let (compatibility, sdk_home) = SetupView::defaults()?;
            self.compatibility = Some(compatibility);
            self.sdk_home = Some(sdk_home);
            self.check_sdk();
            return Ok(());
        }
        let paths = self
            .paths
            .as_ref()
            .ok_or("App storage unavailable; restart after fixing the reported directory")?
            .clone();
        match command {
            Command::Initialize => (),
            Command::Create(path) => {
                let compatibility = self
                    .compatibility
                    .as_ref()
                    .ok_or("Compatible SDK manifest unavailable")?;
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("New video");
                studio_project::create(
                    &path,
                    name,
                    sdk_pin(compatibility),
                    &compatibility.fframes_version,
                    "0.1.0",
                )
                .map_err(|e| e.to_string())?;
                self.open(&path, &paths)?;
            }
            Command::Open(path) => self.open(&path, &paths)?,
            Command::Locate(path, id) => {
                if studio_project::open(&path)
                    .map_err(|e| e.to_string())?
                    .manifest
                    .project_id
                    != id
                {
                    return Err("Selected folder belongs to a different project; locate this recent project's folder or use Open for another project".into());
                }
                self.open(&path, &paths)?;
            }
            Command::Import(path) => {
                // The selected package is explicit. Walk only to its containing workspace root.
                if path.file_name().and_then(|n| n.to_str()) != Some("Cargo.toml") {
                    return Err("Choose the selected package's Cargo.toml".into());
                }
                let package_root = path.parent().ok_or("Choose a package Cargo.toml")?;
                let cargo = studio_project::lifecycle::read_cargo(
                    package_root,
                    &"Cargo.toml"
                        .to_owned()
                        .try_into()
                        .map_err(|e: studio_project::ProjectError| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                let package = cargo
                    .get("package")
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .ok_or(
                        "Choose a package Cargo.toml inside the workspace, not its virtual root",
                    )?
                    .to_owned();
                let mut root = package_root.to_path_buf();
                for ancestor in package_root.ancestors() {
                    if ancestor.join("Cargo.toml").is_file() {
                        let candidate = studio_project::lifecycle::read_cargo(
                            ancestor,
                            &"Cargo.toml".to_owned().try_into().unwrap(),
                        )
                        .map_err(|e| e.to_string())?;
                        if candidate.get("workspace").is_some() {
                            root = ancestor.to_path_buf();
                            break;
                        }
                    }
                }
                let manifest = path
                    .strip_prefix(&root)
                    .map_err(|e| e.to_string())?
                    .to_str()
                    .ok_or("Cargo path must be UTF-8")?
                    .replace('\\', "/")
                    .try_into()
                    .map_err(|e: studio_project::ProjectError| e.to_string())?;
                studio_project::import(
                    &root,
                    CargoEntry {
                        manifest,
                        package,
                        worker_target: "studio_worker".into(),
                    },
                    sdk_pin(
                        self.compatibility
                            .as_ref()
                            .ok_or("SDK manifest unavailable")?,
                    ),
                )
                .map_err(|e| e.to_string())?;
                self.open(&root, &paths)?;
            }
            Command::Asset(path) => {
                let closed = self.closed.clone();
                self.current()?
                    .copy_asset_with_cancel(&path, || closed.load(Ordering::Acquire))
                    .map_err(|e| e.to_string())?;
            }
            Command::Refresh => {
                if let Some(controller) = &mut self.controller {
                    controller.reconcile().map_err(|e| e.to_string())?;
                }
                self.check_sdk();
            }
            Command::Checkpoint => self.current()?.checkpoint().map_err(|e| e.to_string())?,
            Command::Build => {
                let builds = paths.builds();
                let fallback = self.sdk_home.as_ref().map(|h| h.join("active"));
                let controller = self.current()?;
                let sdk = controller
                    .sdk_path()
                    .map(PathBuf::from)
                    .or(fallback)
                    .ok_or("Select an installed SDK before building preview")?;
                let compatibility = CompatibilityManifest::from_json_str(
                    &std::fs::read_to_string(sdk.join("compatibility.json"))
                        .map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
                if !compatibility
                    .preview_contract_versions
                    .contains(&fframes_studio_protocol::PREVIEW_CONTRACT_VERSION)
                {
                    return Err("Selected SDK supports only the legacy worker. Select an M2 preview SDK; imported source is not rewritten.".into());
                }
                if controller.state().active_tag().is_some() {
                    controller.cancel().map_err(|e| e.to_string())?;
                }
                let tag = controller
                    .begin_job(studio_engine::JobKind::Build)
                    .map_err(|e| e.to_string())?;
                self.build_spec = Some(BuildSpec {
                    project: controller.project.clone(),
                    sdk,
                    compatibility,
                    builds,
                    tag,
                    compiler: controller.operation_processes(),
                    worker: controller.processes.sub_manager(),
                });
            }
            Command::CancelBuild => {
                if self.current()?.state().active_tag().is_some() {
                    self.current()?.cancel().map_err(|e| e.to_string())?;
                }
            }
            Command::Close => {
                if let Some(controller) = &mut self.controller {
                    controller.close().map_err(|e| e.to_string())?;
                }
                self.controller = None;
                self.check_sdk();
            }
            Command::RemoveRecent(id) => Store::open(&paths.database())
                .and_then(|mut s| s.remove_recent(&id))
                .map_err(|e| e.to_string())?,
            Command::SelectSdk(path) => {
                self.current()?
                    .select_sdk(path)
                    .map_err(|e| e.to_string())?;
                self.check_sdk();
            }
            Command::InstallSdk => {
                if let Err(error) = SetupView::install_sdk(
                    self.compatibility
                        .as_ref()
                        .ok_or("SDK manifest unavailable")?,
                    self.sdk_home.as_deref().ok_or("SDK home unavailable")?,
                    &self.processes,
                ) {
                    self.sdk = "Failed · repair the reported installation error and retry".into();
                    return Err(error);
                }
                self.check_sdk();
            }
            Command::Restore(path, revision) => self
                .current()?
                .export_checkpoint(&revision, &path)
                .map_err(|e| e.to_string())?,
            Command::Independent(path) => {
                studio_project::lifecycle::assign_independent_identity(&path)
                    .map_err(|e| e.to_string())?;
                self.open(&path, &paths)?;
            }
        }
        Ok(())
    }
    fn current(&mut self) -> Result<&mut Controller, String> {
        self.controller
            .as_mut()
            .ok_or_else(|| "Open a project first".into())
    }
    fn open(&mut self, path: &Path, paths: &AppPaths) -> Result<(), String> {
        if self
            .controller
            .as_ref()
            .is_some_and(|c| c.project.root == path)
        {
            return self.current()?.reconcile().map_err(|e| e.to_string());
        }
        let candidate = Controller::open(path, paths).map_err(|e| e.to_string())?;
        if let Some(previous) = &mut self.controller {
            previous.close().map_err(|e| e.to_string())?;
        }
        self.controller = Some(candidate);
        self.check_sdk();
        Ok(())
    }
    fn check_sdk(&mut self) {
        let Some(compatibility) = &self.compatibility else {
            self.sdk = "SDK manifest unavailable".into();
            return;
        };
        let report = Doctor::run_host_preflight(compatibility);
        if !report.is_ready() {
            self.sdk = format!("Host prerequisites needed: {}", report.format_summary());
            return;
        }
        let sdk = self
            .controller
            .as_ref()
            .and_then(|c| c.sdk_path().map(PathBuf::from))
            .or_else(|| self.sdk_home.as_ref().map(|h| h.join("active")));
        self.sdk = match sdk {
            Some(path) if path.exists() => {
                let selected = std::fs::read_to_string(path.join("compatibility.json"))
                    .map_err(|e| e.to_string())
                    .and_then(|s| {
                        CompatibilityManifest::from_json_str(&s).map_err(|e| e.to_string())
                    });
                let selected = match selected {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        self.sdk = format!("SDK failed: {error}; select a complete installed SDK");
                        return;
                    }
                };
                if self
                    .controller
                    .as_ref()
                    .is_some_and(|c| c.project.manifest.sdk != sdk_pin(&selected))
                {
                    self.sdk = "Incompatible SDK pin; select the project's compatible SDK".into();
                    return;
                }
                let report = Doctor::verify_candidate_sdk_with_processes(
                    &path,
                    &selected,
                    Some(&self.processes),
                );
                if report.is_ready() {
                    format!("Available · {}", path.display())
                } else {
                    format!(
                        "SDK failed verification: {}; select/reinstall compatible SDK",
                        report.format_summary()
                    )
                }
            }
            _ => "Missing · install the managed SDK or select an installed compatible SDK".into(),
        };
    }
    fn presentation(&self) -> Result<Presentation, String> {
        let recents = match &self.paths {
            Some(paths) => Store::open(&paths.database())
                .and_then(|s| s.recents())
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|r| Recent {
                    missing: !r.location.is_dir(),
                    id: r.state.project().clone(),
                    name: r.name,
                    path: r.location,
                })
                .collect(),
            None => vec![],
        };
        Ok(Presentation {
            project: self
                .controller
                .as_ref()
                .map(ProjectPresentation::from_controller),
            recents,
            sdk: self.sdk.clone(),
            state: self.controller.as_ref().map(|c| c.state().clone()),
            owner: self.controller.as_ref().map(|c| c.processes.clone()),
        })
    }
}

struct PreparedInstall {
    ready: Arc<studio_engine::ReadyPreview>,
    source: studio_engine::ProjectState,
    clock: studio_engine::PlaybackClock,
    audio: Option<OutputHandle>,
    audio_ready: bool,
}

pub struct StudioShell {
    backend: Arc<Mutex<Backend>>,
    closed: Arc<AtomicBool>,
    focus: gpui::FocusHandle,
    presentation: Presentation,
    busy: bool,
    epoch: u64,
    error: Option<String>,
    failed_open: Option<PathBuf>,
    navigation: &'static str,
    preview: Option<Arc<PreviewCoordinator>>,
    preview_session: Option<studio_engine::OpenSession>,
    preview_state: studio_engine::PreviewState,
    displayed: Option<Arc<studio_engine::ReadyPreview>>,
    pending_ready: Option<PreparedInstall>,
    pending_frame: Option<studio_engine::PreviewFrame>,
    images: ImagePresentationManager,
    preview_extent: (u32, u32),
    clear_images: bool,
    preview_gate_pending: bool,
    timeline: gpui::Entity<TimelineView>,
    _timeline_subscription: gpui::Subscription,
    transport: studio_engine::PlaybackClock,
    audio: Arc<AudioService>,
    output: Option<OutputHandle>,
    audio_ready_epoch: Option<u64>,
    audio_note: String,
    output_device: OutputDevice,
    muted: bool,
    resume_install: bool,
    requested_clock_epoch: u64,
    presented_serial: u64,
    clock_start: Instant,
    resume_scrub: bool,
    qualifying: bool,
    button_bounds: HashMap<String, [f32; 4]>,
    painted_frame: Option<usize>,
}
impl StudioShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(false);
        window.focus(&focus, cx);
        let timeline = cx.new(TimelineView::new);
        let timeline_subscription = cx.subscribe(&timeline, |shell, _, event, cx| match *event {
            TimelineEvent::Seek(frame) => shell.seek_preview(frame, cx),
            TimelineEvent::Step(delta) => shell.step_preview(delta, cx),
            TimelineEvent::TogglePlayback => shell.toggle_playback(cx),
            TimelineEvent::ToggleMute => shell.toggle_mute(cx),
            TimelineEvent::BeginScrub => {
                shell.resume_scrub = shell.transport.playing() || shell.resume_install;
                shell.resume_install = false;
                shell.pause_playback(cx);
            }
            TimelineEvent::EndScrub => {
                if shell.resume_scrub
                    && !shell.transport.playing()
                    && shell
                        .displayed
                        .as_ref()
                        .is_some_and(|d| shell.transport.position() < d.timeline.total_frames)
                {
                    shell.toggle_playback(cx);
                }
                shell.resume_scrub = false;
            }
        });
        let closed = Arc::new(AtomicBool::new(false));
        let processes = studio_bootstrap::ProcessTreeManager::new();
        let backend = Arc::new(Mutex::new(Backend {
            paths: None,
            controller: None,
            compatibility: None,
            sdk_home: None,
            sdk: "Checking SDK…".into(),
            closed: closed.clone(),
            processes: processes.clone(),
            build_spec: None,
        }));
        let owner = backend.clone();
        let shutdown = closed.clone();
        let clock_start = Instant::now();
        let audio = Arc::new(AudioService::new(clock_start));
        let quit_audio = audio.clone();
        cx.on_app_quit(move |shell, cx| {
            shutdown.store(true, Ordering::Release);
            quit_audio.shutdown();
            let audio = quit_audio.clone();
            let preview = shell.preview.take();
            let displayed = shell.displayed.take();
            let pending = shell.pending_ready.take();
            shell.output = None;
            let owner = owner.clone();
            let processes = processes.clone();
            cx.background_executor().spawn(async move {
                processes.shutdown(Duration::ZERO);
                audio.join();
                if let Some(preview) = preview {
                    preview.close();
                    drop(preview);
                }
                drop((displayed, pending));
                // A hash/copy may still own the serialized queue. Do not wait
                // unboundedly on it during quit; durable jobs recover as interrupted.
                if let Some(mut backend) = owner.try_lock_for(Duration::from_millis(50))
                    && let Some(mut controller) = backend.controller.take()
                {
                    let _ = controller.close();
                }
            })
        })
        .detach();
        let mut shell = Self {
            backend,
            closed,
            focus,
            presentation: Presentation::default(),
            busy: false,
            epoch: 0,
            error: None,
            failed_open: None,
            navigation: "Project",
            preview: None,
            preview_session: None,
            preview_state: studio_engine::PreviewState::default(),
            displayed: None,
            pending_ready: None,
            pending_frame: None,
            images: ImagePresentationManager::new(),
            preview_extent: (1280, 720),
            clear_images: false,
            preview_gate_pending: false,
            timeline,
            _timeline_subscription: timeline_subscription,
            transport: studio_engine::PlaybackClock::install(30, 0, 0, 0.)
                .expect("valid empty clock"),
            audio,
            output: None,
            audio_ready_epoch: None,
            audio_note: "Audio output not prepared".into(),
            output_device: OutputDevice::Default,
            muted: false,
            resume_install: false,
            requested_clock_epoch: 0,
            presented_serial: 0,
            clock_start,
            resume_scrub: false,
            qualifying: false,
            button_bounds: HashMap::new(),
            painted_frame: None,
        };
        shell.dispatch(Command::Initialize, cx);
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let Ok(keep_running) = this.update(cx, |shell, cx| {
                    if shell.closed.load(Ordering::Acquire) {
                        return false;
                    }
                    shell.tick_playback(cx);
                    shell.poll_preview(cx);
                    true
                }) else {
                    break;
                };
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
        // Coalesced notifications; never hash source in render or on the UI thread.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(750))
                    .await;
                let Ok(Some((backend, closed))) = this.update(cx, |shell, _| {
                    if shell.busy {
                        None
                    } else {
                        Some((shell.backend.clone(), shell.closed.clone()))
                    }
                }) else {
                    if this.upgrade().is_none() {
                        break;
                    }
                    continue;
                };
                if closed.load(Ordering::Acquire) {
                    break;
                }
                let changed = cx
                    .background_executor()
                    .spawn(async move {
                        backend
                            .lock()
                            .controller
                            .as_ref()
                            .is_some_and(Controller::changed_hint)
                    })
                    .await;
                if changed {
                    let _ = this.update(cx, |shell, cx| shell.dispatch(Command::Refresh, cx));
                }
            }
        })
        .detach();
        shell
    }
    /// Observe the production shell while the external harness sends native input.
    /// File serialization/writes stay off the UI thread and are awaited (no queue).
    pub fn start_qualification(
        &mut self,
        project: PathBuf,
        output: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.qualifying = true;
        cx.spawn(async move |this, cx| {
            let mut stage = 0;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(10))
                    .await;
                let Ok(value) = this.update(cx, |shell, cx| {
                    if !shell.busy && stage == 0 {
                        shell.dispatch(Command::Open(project.clone()), cx);
                        stage = 1;
                    } else if !shell.busy && stage == 1 && shell.presentation.project.is_some() {
                        shell.dispatch(Command::Build, cx);
                        stage = 2;
                    }
                    shell.qualification_snapshot(cx)
                }) else {
                    break;
                };
                let path = output.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let temporary = path.with_extension("tmp");
                        std::fs::write(
                            &temporary,
                            serde_json::to_vec(&value).map_err(std::io::Error::other)?,
                        )?;
                        std::fs::rename(temporary, path)
                    })
                    .await;
                if let Err(error) = result {
                    let _ = this.update(cx, |shell, cx| {
                        shell.error = Some(format!("Qualification telemetry: {error}"));
                        cx.notify();
                    });
                    break;
                }
            }
        })
        .detach();
    }
    fn qualification_snapshot(&self, cx: &Context<Self>) -> serde_json::Value {
        let thumbnails = self.timeline.read(cx).qualification_metrics();
        let mut buttons = serde_json::to_value(&self.button_bounds).expect("finite geometry");
        buttons["timeline-fit"] = thumbnails["fit_bounds"].clone();
        serde_json::json!({
            "pid": std::process::id(), "busy": self.busy,
            "project_path": self.presentation.project.as_ref().map(|p| &p.root), "error": self.error,
            "current_source_revision": self.presentation.project.as_ref().map(|p| p.source.as_str()),
            "preview_status": format!("{:?}", self.preview_state.status),
            "displayed_generation": self.displayed.as_ref().map(|d| d.identity().worker_generation),
            "displayed_revision": self.displayed.as_ref().map(|d| &d.identity().source_revision),
            "position": self.transport.position(), "playing": self.transport.playing() || self.resume_install,
            "total_frames": self.displayed.as_ref().map(|d| d.timeline.total_frames),
            "fps": self.displayed.as_ref().map(|d| d.timeline.fps),
            "clock_epoch": self.transport.epoch(), "seek_serial": self.preview_state.serial(),
            "paint_serial": self.presented_serial, "paint_frame": self.painted_frame,
            "preview_extent": self.preview_extent, "preview_scale": self.preview_state.scale(),
            "audio_ready": self.output.is_some(),
            "audio_identity_matches": self.output.as_ref().is_none_or(|o| Some(&o.identity) == self.preview_state.displayed() && o.epoch == self.transport.epoch()),
            "audio_note": self.audio_note, "output_device": format!("{:?}", self.output_device), "muted": self.muted,
            "audio_metrics": self.output.as_ref().map(OutputHandle::metrics),
            "thumbnail_metrics": thumbnails,
            "image_metrics": { "resident": self.images.resident_count(), "queued": self.images.queue_depth(), "dropped": self.images.dropped_count(), "release_failures": self.images.release_failures() },
            "pump_metrics": self.preview.as_ref().map(|p| p.metrics()), "buttons": buttons,
            "ruler_bounds": thumbnails["ruler_bounds"],
        })
    }
    fn discard_install(&mut self, cx: &mut Context<Self>) {
        if self.pending_ready.take().is_some() {
            self.audio.stop(self.transport.epoch());
            self.output = None;
            self.audio_ready_epoch = None;
        }
        if self.resume_install {
            self.resume_install = false;
            self.toggle_playback(cx);
        }
    }
    fn dispatch(&mut self, command: Command, cx: &mut Context<Self>) {
        if self.busy || self.closed.load(Ordering::Acquire) {
            return;
        }
        if let Command::Open(path) | Command::Locate(path, _) = &command {
            self.failed_open = Some(path.clone());
        }
        if matches!(command, Command::CancelBuild | Command::Build) {
            if let Some(preview) = &self.preview {
                preview.cancel_build();
            }
            self.preview_state.cancel_build();
            self.discard_install(cx);
        }
        if matches!(command, Command::Close) {
            self.detach_preview(cx);
        }
        let refresh = matches!(command, Command::Refresh);
        self.busy = true;
        self.epoch += 1;
        let epoch = self.epoch;
        let backend = self.backend.clone();
        let closed = self.closed.clone();
        let task = cx.background_executor().spawn(async move {
            let mut backend = backend.lock();
            if closed.load(Ordering::Acquire) {
                return (Err("Window closed".into()), None, None);
            }
            let result = backend.command(command);
            let presentation = backend.presentation();
            let spec = backend.build_spec.take();
            match presentation {
                Ok(p) => (result, Some(p), spec),
                Err(e) => (Err(e), None, None),
            }
        });
        cx.spawn(async move |this, cx| {
            let (result, presentation, spec) = task.await;
            let _ = this.update(cx, |shell, cx| {
                if shell.epoch != epoch || shell.closed.load(Ordering::Acquire) {
                    return;
                }
                shell.busy = false;
                if let Some(presentation) = presentation {
                    let changed_session = shell.preview_session.as_ref()
                        != presentation.state.as_ref().map(|s| s.session());
                    if changed_session {
                        shell.detach_preview(cx);
                        if let (Some(state), Some(owner)) =
                            (&presentation.state, &presentation.owner)
                        {
                            shell.preview =
                                Some(Arc::new(PreviewCoordinator::new(owner.sub_manager())));
                            shell.preview_session = Some(state.session().clone());
                            shell.preview_state = studio_engine::PreviewState::default();
                        }
                    }
                    if shell.presentation.state.as_ref().map(|s| s.generation())
                        != presentation.state.as_ref().map(|s| s.generation())
                    {
                        if let Some(preview) = &shell.preview {
                            preview.cancel_build();
                        }
                        shell.preview_state.cancel_build();
                        shell.discard_install(cx);
                    }
                    shell.presentation = presentation;
                }
                if let Some(spec) = spec {
                    shell.preview_state.begin(spec.tag.clone());
                    if let Some(preview) = &shell.preview {
                        preview.build(spec);
                    }
                }
                match result {
                    Ok(()) => {
                        if !refresh {
                            shell.error = None;
                            shell.failed_open = None;
                        }
                    }
                    Err(error) => shell.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn detach_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(preview) = self.preview.take() {
            // Teardown stays off the UI thread; the root token rejects any late spawn.
            std::thread::spawn(move || preview.close());
        }
        self.preview_session = None;
        self.preview_state.close();
        self.displayed = None;
        self.pending_ready = None;
        self.pending_frame = None;
        self.clear_images = true;
        let _ = self
            .transport
            .reinstall(30, 0, 0, self.clock_start.elapsed().as_secs_f64());
        self.audio.stop(self.transport.epoch());
        self.output = None;
        self.audio_ready_epoch = None;
        self.resume_install = false;
        self.audio_note = "Audio output not prepared".into();
        self.resume_scrub = false;
        self.timeline.update(cx, |v, cx| v.clear(cx));
    }
    fn poll_preview(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.preview_gate_pending {
            return;
        }
        let Some(preview) = &self.preview else {
            return;
        };
        let events = preview.events();
        if let Some((key, frame)) = events.thumbnail {
            self.timeline
                .update(cx, |v, cx| v.thumbnail(key, frame, cx));
        }
        preview.thumbnails(if self.transport.playing() {
            vec![]
        } else {
            self.timeline.read(cx).thumbnail_requests()
        });
        if let Some(frame) = events.frame
            && self.preview_state.accepts_frame(&frame)
            && self.requested_clock_epoch == self.transport.epoch()
        {
            self.pending_frame = Some(frame);
            cx.notify();
        }
        if events.compiled.is_none() && events.ready.is_none() && events.error.is_none() {
            return;
        }
        self.preview_gate_pending = true;
        let backend = self.backend.clone();
        let session = self.preview_session.clone();
        let task = cx.background_executor().spawn(async move {
            let mut b = backend.lock();
            let mut error = None;
            let state = if let Some(c) = &mut b.controller {
                if let Some(tag) = events.compiled
                    && let Err(e) = c.complete(
                        &tag,
                        studio_engine::JobResult::Built(tag.base_source.clone()),
                    )
                {
                    error = Some((tag, e.to_string()));
                }
                if let Some((tag, e)) = events.error {
                    if c.state().active_tag() == Some(&tag) {
                        let _ = c.complete(&tag, studio_engine::JobResult::Failed(e.clone()));
                    }
                    error = Some((tag, e));
                }
                if events.ready.is_some()
                    && let Err(e) = c.reconcile()
                {
                    error = events
                        .ready
                        .as_ref()
                        .map(|r| (r.tag().clone(), e.to_string()));
                }
                Some(c.state().clone())
            } else {
                None
            };
            (events.ready, state, error, b.presentation().ok())
        });
        cx.spawn(async move |this, cx| {
            let (ready, state, error, presentation) = task.await;
            let _ = this.update(cx, |shell, cx| {
                shell.preview_gate_pending = false;
                if shell.preview_session != session {
                    return;
                }
                if let Some(p) = presentation {
                    shell.presentation = p;
                }
                if let Some((tag, e)) = error {
                    shell.preview_state.fail(&tag, e.clone());
                    shell.discard_install(cx);
                    shell.error = Some(e);
                } else if let (Some(ready), Some(state)) = (ready, state) {
                    if ready.tag().generation != state.generation()
                        || ready.tag().base_source != *state.source()
                    {
                        if let Some(preview) = &shell.preview {
                            preview.cancel_build();
                        }
                        shell.preview_state.cancel_build();
                    } else if shell.transport.playing()
                        && matches!(state.job(), studio_engine::JobState::Succeeded(tag) if tag == ready.tag()) {
                            // Re-evaluate the audible position, then re-prime the candidate
                            // under the newly frozen seek serial. Old playback served the
                            // entire compilation/preparation, not the compiler's cursor.
                            shell.resume_install = true;
                            shell.pause_playback(cx);
                    } else if shell.preview_state.can_install(&ready, &state).is_ok() {
                            let mut clock = shell.transport.clone();
                            if let Err(e) = clock.reinstall(ready.timeline.fps, ready.timeline.total_frames,
                                ready.position, shell.clock_start.elapsed().as_secs_f64()) {
                                shell.error = Some(e.to_string());
                            } else {
                                // Reserve the staged epoch in the active clock as well.
                                // A seek during preparation must advance past this epoch,
                                // never collide with a callback for another revision.
                                let _ = shell.transport.wait_for_output();
                                shell.audio.stage(ready.clone(), clock.epoch(), ready.position, shell.output_device.clone());
                                shell.output = None;
                                shell.audio_ready_epoch = None;
                                shell.pending_ready = Some(PreparedInstall { ready, source: state, clock, audio: None, audio_ready: false });
                                shell.audio_note = "Priming matching audio and first frame".into();
                            }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn seek_preview(&mut self, position: usize, cx: &mut Context<Self>) {
        let resume = std::mem::take(&mut self.resume_install);
        self.pending_ready = None;
        if let Err(e) = self
            .transport
            .seek(position, self.clock_start.elapsed().as_secs_f64())
        {
            self.error = Some(e.to_string());
            return;
        }
        self.audio.stop(self.transport.epoch());
        self.output = None;
        self.audio_ready_epoch = None;
        self.request_frame(self.transport.position(), cx);
        if resume && !self.transport.playing() {
            self.toggle_playback(cx);
        }
        if self.transport.playing() {
            self.prime_playback(cx);
        }
    }
    fn request_frame(&mut self, position: usize, cx: &mut Context<Self>) {
        let Some(id) = self.preview_state.displayed().cloned() else {
            return;
        };
        let scale = self.displayed.as_ref().map_or(1., |d| {
            crate::frame_image::preview_scale(
                d.timeline.width,
                d.timeline.height,
                self.preview_extent,
            )
        });
        let Ok(serial) = self.preview_state.seek(position, scale) else {
            return;
        };
        self.requested_clock_epoch = self.transport.epoch();
        if let Some(preview) = &self.preview {
            preview.seek(SeekIntent {
                identity: id,
                serial,
                position: self.preview_state.position(),
                scale: self.preview_state.scale(),
            });
        }
        self.timeline.update(cx, |v, cx| {
            v.set_position(self.preview_state.position(), cx)
        });
        cx.notify();
    }
    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        if self.transport.playing() || self.resume_install {
            self.resume_install = false;
            self.pause_playback(cx);
        } else {
            self.pending_ready = None;
            if let Err(e) = self
                .transport
                .toggle(self.clock_start.elapsed().as_secs_f64())
            {
                self.error = Some(e.to_string());
            } else if self.transport.playing() {
                // Every start passes through a matching first-frame/PCM epoch.
                let _ = self.transport.wait_for_output();
                self.request_frame(self.transport.position(), cx);
                self.prime_playback(cx);
            }
        }
        cx.notify();
    }
    fn pause_playback(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self
            .transport
            .pause(self.clock_start.elapsed().as_secs_f64())
        {
            self.error = Some(e.to_string());
        }
        self.pending_ready = None;
        self.audio.stop(self.transport.epoch());
        self.output = None;
        self.audio_ready_epoch = None;
        self.request_frame(self.transport.position(), cx);
    }
    fn prime_playback(&mut self, cx: &mut Context<Self>) {
        if let Some(ready) = &self.displayed {
            self.output = None;
            self.audio_ready_epoch = None;
            self.audio.stage(
                ready.clone(),
                self.transport.epoch(),
                self.transport.position(),
                self.output_device.clone(),
            );
            self.audio_note = "Buffering · waiting for matching first frame and PCM".into();
            cx.notify();
        }
    }
    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        self.audio.mute(self.muted);
        cx.notify();
    }
    fn change_output(&mut self, next: OutputDevice, cx: &mut Context<Self>) {
        self.output_device = next;
        self.seek_preview(self.transport.position(), cx);
        if !self.transport.playing() {
            self.prime_playback(cx);
        }
    }
    fn cycle_output(&mut self, cx: &mut Context<Self>) {
        let devices = self.audio.devices();
        let next = match &self.output_device {
            OutputDevice::Default => devices
                .first()
                .cloned()
                .map_or(OutputDevice::Unavailable, OutputDevice::Named),
            OutputDevice::Named(name) => devices
                .iter()
                .position(|d| d == name)
                .and_then(|i| devices.get(i + 1))
                .cloned()
                .map_or(OutputDevice::Unavailable, OutputDevice::Named),
            OutputDevice::Unavailable => OutputDevice::Default,
        };
        self.change_output(next, cx);
    }
    fn poll_audio(&mut self, cx: &mut Context<Self>) {
        match self.audio.event() {
            Some(AudioEvent::Staged {
                epoch,
                identity,
                output,
                unavailable,
            }) => {
                if let Some(pending) = &mut self.pending_ready
                    && pending.clock.epoch() == epoch
                    && pending.ready.identity() == &identity
                {
                    pending.audio = output;
                    pending.audio_ready = true;
                    self.audio_note =
                        unavailable.unwrap_or_else(|| "Matching audio prepared".into());
                } else if epoch == self.transport.epoch()
                    && self.preview_state.displayed() == Some(&identity)
                {
                    self.output = output;
                    self.audio_ready_epoch = Some(epoch);
                    self.audio_note = unavailable.unwrap_or_else(|| "Audio output prepared".into());
                }
                cx.notify();
            }
            Some(AudioEvent::Failed { epoch, error }) => {
                if self
                    .pending_ready
                    .as_ref()
                    .is_some_and(|p| p.clock.epoch() == epoch)
                {
                    let pending = self.pending_ready.take().unwrap();
                    self.preview_state.fail(pending.ready.tag(), error.clone());
                    if let Some(preview) = &self.preview {
                        preview.cancel_build();
                    }
                    self.error = Some(format!(
                        "Audio preparation failed; prior preview retained: {error}"
                    ));
                    if self.resume_install {
                        self.resume_install = false;
                        self.toggle_playback(cx);
                    }
                } else if epoch == self.transport.epoch() {
                    self.output = None;
                    self.audio_ready_epoch = Some(epoch);
                    self.audio_note = format!("Audio unavailable · {error}");
                }
                cx.notify();
            }
            Some(AudioEvent::Lost { epoch }) if epoch == self.transport.epoch() => {
                self.output = None;
                self.audio_ready_epoch = None;
                let _ = self
                    .transport
                    .fallback(self.clock_start.elapsed().as_secs_f64());
                self.audio.stop(self.transport.epoch());
                self.request_frame(self.transport.position(), cx);
                self.audio_note =
                    "Audio device lost · monotonic fallback · Retry audio to reconnect".into();
            }
            _ => {}
        }
    }
    fn activate_audio(&mut self, cx: &mut Context<Self>) {
        let epoch = self.transport.epoch();
        if self.audio_ready_epoch == Some(epoch)
            && self.presented_serial == self.preview_state.serial()
        {
            self.audio_ready_epoch = None;
            if self.output.is_some() {
                self.audio.commit(epoch, self.transport.playing());
            } else {
                let _ = self
                    .transport
                    .fallback(self.clock_start.elapsed().as_secs_f64());
                self.audio.stop(self.transport.epoch());
                self.requested_clock_epoch = self.transport.epoch();
                self.audio_note = "Audio unavailable · monotonic fallback".into();
            }
            cx.notify();
        }
    }
    fn step_preview(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.resume_install = false;
        self.pause_playback(cx);
        self.seek_preview(self.transport.position().saturating_add_signed(delta), cx);
    }
    fn tick_playback(&mut self, cx: &mut Context<Self>) {
        self.poll_audio(cx);
        if let Some(output) = &self.output
            && let Some(snapshot) = output.snapshot()
        {
            self.transport.accept_output(snapshot);
        }
        let was_playing = self.transport.playing();
        let position = self
            .transport
            .tick(self.clock_start.elapsed().as_secs_f64());
        if position != self.preview_state.position() {
            self.request_frame(position, cx);
        }
        if was_playing != self.transport.playing() {
            if !self.transport.playing() {
                self.audio.stop(self.transport.epoch());
                self.output = None;
                self.audio_ready_epoch = None;
            }
            cx.notify();
        }
    }
    fn pick(&mut self, kind: Picker, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        // A watcher refresh must not discard the result of an active native dialog.
        self.busy = true;
        cx.notify();
        let epoch = self.epoch;
        if matches!(kind, Picker::Create | Picker::Restore) {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let picker = cx.prompt_for_new_path(
                &home,
                Some(if matches!(kind, Picker::Create) {
                    "New video"
                } else {
                    "Recovered video"
                }),
            );
            // Export the checkpoint selected before the picker opened, even if
            // current source is degraded. No source installation occurs here.
            let accepted = self
                .presentation
                .project
                .as_ref()
                .map(|p| p.accepted.clone());
            cx.spawn(async move |this, cx| {
                let result = picker.await;
                let _ = this.update(cx, |shell, cx| {
                    if shell.epoch != epoch || shell.closed.load(Ordering::Acquire) {
                        return;
                    }
                    shell.busy = false;
                    match result {
                        Ok(Ok(Some(path))) => {
                            if matches!(kind, Picker::Create) {
                                shell.dispatch(Command::Create(path), cx);
                            } else if let Some(accepted) = accepted {
                                shell.dispatch(Command::Restore(path, accepted), cx);
                            }
                        }
                        Ok(Ok(None)) => (),
                        error => shell.error = Some(format!("Native picker failed: {error:?}; check the desktop portal/file chooser and retry")),
                    }
                    cx.notify();
                });
            }).detach();
        } else {
            let label = match &kind {
                Picker::Asset => "Asset",
                Picker::Import => "Import",
                Picker::Sdk => "SDK",
                Picker::Locate(_) => "Locate",
                _ => "Open",
            };
            let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: matches!(kind, Picker::Asset | Picker::Import),
                directories: !matches!(kind, Picker::Asset | Picker::Import),
                multiple: false,
                prompt: Some(format!("{label}…").into()),
            });
            cx.spawn(async move |this, cx| {
                let result = picker.await;
                let _ = this.update(cx, |shell, cx| {
                    if shell.epoch != epoch || shell.closed.load(Ordering::Acquire) {
                        return;
                    }
                    shell.busy = false;
                    match result {
                        Ok(Ok(Some(paths))) => if let Some(path) = paths.into_iter().next() {
                            let command = match kind {
                                Picker::Asset => Command::Asset(path),
                                Picker::Import => Command::Import(path),
                                Picker::Sdk => Command::SelectSdk(path),
                                Picker::Locate(id) => Command::Locate(path, id),
                                _ => Command::Open(path),
                            };
                            shell.dispatch(command, cx);
                        },
                        Ok(Ok(None)) => (),
                        error => shell.error = Some(format!("Native picker failed: {error:?}; check the desktop portal/file chooser and retry")),
                    }
                    cx.notify();
                });
            }).detach();
        }
    }
    fn button(
        &self,
        id: impl Into<gpui::ElementId>,
        label: impl Into<gpui::SharedString>,
        enabled: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut gpui::Context<Self>) + 'static,
    ) -> impl IntoElement {
        let label = label.into();
        let action = std::rc::Rc::new(action);
        let click = action.clone();
        let id = id.into();
        let name = match &id {
            gpui::ElementId::Name(name)
                if self.qualifying
                    && matches!(
                        name.as_ref(),
                        "build-preview"
                            | "cancel-preview"
                            | "play-pause"
                            | "mute-audio"
                            | "next-audio-output"
                            | "retry-audio-output"
                            | "no-audio-output"
                            | "previous-frame"
                            | "next-frame"
                            | "close"
                    ) =>
            {
                Some(name.to_string())
            }
            _ => None,
        };
        let entity = cx.entity();
        div()
            .id(id)
            .relative()
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .tab_index(0)
            .tab_stop(enabled)
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(PANEL))
            .text_sm()
            .text_color(rgb(if enabled { TEXT } else { MUTED }))
            .opacity(if enabled { 1.0 } else { 0.45 })
            .focus_visible(|s| s.border_color(rgb(ACCENT)))
            .on_click(cx.listener(move |shell, _, _, cx| {
                if enabled {
                    click(shell, cx);
                }
            }))
            .on_key_down(
                cx.listener(move |shell, event: &gpui::KeyDownEvent, _, cx| {
                    if enabled && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        action(shell, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(label)
            .children(name.map(|name| {
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |shell, _| {
                            shell.button_bounds.insert(
                                name,
                                [
                                    f32::from(bounds.left()),
                                    f32::from(bounds.top()),
                                    f32::from(bounds.size.width),
                                    f32::from(bounds.size.height),
                                ],
                            );
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            }))
    }
}

impl Render for StudioShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.clear_images {
            self.images.clear(window);
            self.clear_images = false;
        }
        if let Some(install) = self.pending_ready.take() {
            let mut committed = false;
            let ready = &install.ready;
            let source = &install.source;
            // No filesystem work here. Hold a nonblocking state fence across the
            // UI commit; defer if a source scan is pending/in progress.
            let backend = self.backend.clone();
            let locked = backend.try_lock();
            let source_current = locked
                .as_ref()
                .and_then(|b| b.controller.as_ref())
                .and_then(|c| {
                    (!c.changed_hint())
                        .then(|| self.preview_state.can_install(ready, c.state()).is_ok())
                });
            if source_current.is_none() || !install.audio_ready {
                committed = true; // Deferred, not rejected.
                self.pending_ready = Some(install);
            } else if source_current == Some(true)
                && self.presentation.state.as_ref().is_some_and(|s| {
                    s.session() == source.session()
                        && s.source() == source.source()
                        && s.generation() == source.generation()
                })
                && self.preview_state.can_install(ready, source).is_ok()
            {
                let image = ready
                    .frame
                    .as_ref()
                    .map(|f| create_render_image(&f.response.header, &f.pixels))
                    .transpose();
                match image {
                    Ok(image) => {
                        let model = studio_engine::TimelineModel::new(ready.timeline.clone());
                        if let Err(e) = &model {
                            self.error = Some(e.clone());
                        }
                        if self.preview.as_ref().is_some_and(|p| {
                            model.is_ok() && p.commit(ready.identity().clone(), ready.seek_serial)
                        }) && self.preview_state.install(ready, source).is_ok()
                        {
                            if let Some(image) = image {
                                self.images.replace_image(image, window);
                            } else {
                                self.images.clear(window);
                            }
                            self.transport = install.clock;
                            if self.resume_install && ready.position < ready.timeline.total_frames {
                                let _ = self
                                    .transport
                                    .toggle(self.clock_start.elapsed().as_secs_f64());
                            }
                            self.resume_install = false;
                            self.output = install.audio;
                            self.audio_ready_epoch = Some(self.transport.epoch());
                            self.requested_clock_epoch = self.transport.epoch();
                            self.presented_serial = ready.seek_serial;
                            self.painted_frame =
                                ready.frame.as_ref().map(|f| f.response.frame_index);
                            self.timeline
                                .update(cx, |v, cx| v.install(model.unwrap(), ready.position, cx));
                            self.displayed = Some(ready.clone());
                            self.error = None;
                            committed = true;
                        }
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            drop(locked);
            if !committed {
                self.audio.stop(self.transport.epoch());
                self.discard_install(cx);
            }
        }
        if let Some(f) = self.pending_frame.take()
            && self.preview_state.accepts_frame(&f)
            && self.requested_clock_epoch == self.transport.epoch()
        {
            match create_render_image(&f.response.header, &f.pixels) {
                Ok(image) => {
                    self.images.replace_image(image, window);
                    self.presented_serial = f.response.seek_serial;
                    self.painted_frame = Some(f.response.frame_index);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        self.activate_audio(cx);
        let enabled = !self.busy;
        let project = self.presentation.project.clone();
        let mut header = div()
            .flex()
            .items_center()
            .justify_between()
            .p_4()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div().text_lg().child(
                            project
                                .as_ref()
                                .map(|p| p.name.clone())
                                .unwrap_or_else(|| "fframes Studio".into()),
                        ),
                    )
                    .child(
                        div().text_xs().text_color(rgb(MUTED)).child(
                            project
                                .as_ref()
                                .map(|p| p.root.display().to_string())
                                .unwrap_or_else(|| {
                                    "A native workspace for your video projects".into()
                                }),
                        ),
                    ),
            );
        header = header.child(
            div()
                .flex()
                .gap_2()
                .child(self.button("create", "Create…", enabled, cx, |s, cx| {
                    s.pick(Picker::Create, cx)
                }))
                .child(self.button("open", "Open…", enabled, cx, |s, cx| {
                    s.pick(Picker::Open, cx)
                }))
                .child(self.button("import", "Import Rust…", enabled, cx, |s, cx| {
                    s.pick(Picker::Import, cx)
                }))
                .child(self.button(
                    "close",
                    "Close project",
                    enabled && project.is_some(),
                    cx,
                    |s, cx| s.dispatch(Command::Close, cx),
                )),
        );
        let mut navigation = div()
            .w(px(248.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER));
        for name in ["Project", "Assets", "Style"] {
            navigation = navigation.child(self.button(name, name, true, cx, move |s, cx| {
                s.navigation = name;
                cx.notify();
            }));
        }
        let mut inventory = div()
            .id("inventory")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .text_xs()
            .text_color(rgb(MUTED));
        if let Some(p) = &project {
            let files = match self.navigation {
                "Assets" => &p.assets,
                "Style" => &p.styles,
                _ => &p.files,
            };
            if files.is_empty() {
                inventory = inventory.child(if self.navigation == "Style" {
                    "No style preset configured"
                } else {
                    "No assets imported"
                });
            }
            for file in files {
                let path = p.root.join(file);
                inventory = inventory.child(self.button(
                    format!("file-{file}"),
                    file.clone(),
                    true,
                    cx,
                    move |_, cx| cx.reveal_path(&path),
                ));
            }
            if self.navigation == "Project" && p.file_count > p.files.len() {
                inventory = inventory.child(format!(
                    "Showing {} of {} files",
                    p.files.len(),
                    p.file_count
                ));
            }
        } else {
            inventory = inventory.child("Create or open a project to browse its files.");
        }
        navigation = navigation.child(inventory).child(self.button(
            "asset",
            "Copy asset…",
            enabled && project.is_some(),
            cx,
            |s, cx| s.pick(Picker::Asset, cx),
        ));
        let mut center = div()
            .id("center")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p_4()
            .gap_1();
        if let Some(p) = &project {
            center = center.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(format!(
                        "Source {} · Checkpoint {} · Job {}",
                        &p.source.as_str()[..12],
                        &p.accepted.as_str()[..12],
                        p.job
                    ))
                    .child(if p.source == p.accepted {
                        "Current source matches the saved checkpoint"
                    } else {
                        "Source changed · saved checkpoint remains available"
                    })
                    .child(if p.history_available {
                        "Local history available"
                    } else {
                        "Previous machine history unavailable · source baseline saved locally"
                    })
                    .child(if p.worker_available {
                        "Worker bridge available · build an immutable preview"
                    } else {
                        "Worker bridge missing · source remains navigable"
                    }),
            );
            if p.interrupted {
                center = center.child(div().text_color(rgb(ACCENT)).child(if p.draft.is_some() {
                    "Interrupted job · retained draft available"
                } else {
                    "Interrupted before draft publication · source and saved checkpoint are safe"
                }));
            }
            if let Some(notice) = &p.recovery_notice {
                center = center.child(
                    div()
                        .text_xs()
                        .text_color(rgb(ACCENT))
                        .child(notice.clone()),
                );
            }
        }
        let mut preview_surface = div()
            .id("preview-surface")
            .relative()
            .tab_index(0)
            .role(gpui::Role::Group)
            .aria_label("Compiled preview")
            .focus_visible(|s| s.border_color(rgb(ACCENT)))
            .on_key_down(cx.listener(|shell, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "space" => shell.toggle_playback(cx),
                    "left" => shell.step_preview(-1, cx),
                    "right" => shell.step_preview(1, cx),
                    "home" => shell.seek_preview(0, cx),
                    "end" => {
                        if let Some(d) = &shell.displayed {
                            shell.seek_preview(d.timeline.total_frames, cx);
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .flex_1()
            .min_h(px(120.))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded_lg()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2();
        let entity = cx.entity();
        preview_surface = preview_surface.child(
            canvas(
                move |bounds, window, cx| {
                    let dpi = window.scale_factor();
                    let extent = (
                        (f32::from(bounds.size.width) * dpi).round().max(1.) as u32,
                        (f32::from(bounds.size.height) * dpi).round().max(1.) as u32,
                    );
                    entity.update(cx, |shell, cx| {
                        shell.preview_extent = extent;
                        if shell.displayed.as_ref().is_some_and(|d| {
                            crate::frame_image::preview_scale(
                                d.timeline.width,
                                d.timeline.height,
                                extent,
                            ) != shell.preview_state.scale()
                        }) {
                            shell.request_frame(shell.transport.position(), cx);
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
        if let Some(image) = self.images.current_image() {
            preview_surface = preview_surface.child(gpui::img(image).size_full());
        } else {
            preview_surface = preview_surface
                .child(div().text_lg().child(if self.displayed.is_some() {
                    "Empty video"
                } else {
                    "No preview available"
                }))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child("Build preview to compile the project's current source."),
                );
        }
        center = center.child(preview_surface);
        let label = match &self.preview_state.status {
            studio_engine::PreviewStatus::Building => "Building/preparing · prior preview retained",
            studio_engine::PreviewStatus::Preparing => "Preparing matching frame and audio",
            studio_engine::PreviewStatus::Error(_) => "Preview failed · prior preview retained",
            _ => "CPU preview · shaders unsupported",
        };
        center = center.child(div().text_xs().text_color(rgb(MUTED)).child(label));
        let audio_label = self.output.as_ref().map_or_else(
            || self.audio_note.clone(),
            |output| {
                let metrics = output.metrics();
                format!(
                    "{} · {} Hz · predicted output clock{}{} · latency {:.1}ms · underruns {}",
                    output.device,
                    output.sample_rate,
                    if self.muted { " · muted" } else { "" },
                    if metrics.invalid_timestamps > 0 {
                        " (latency estimate)"
                    } else {
                        ""
                    },
                    metrics.predicted_latency_ms,
                    metrics.underrun_frames
                )
            },
        );
        center = center.child(div().text_xs().text_color(rgb(MUTED)).child(audio_label));
        if let Some(d) = &self.displayed {
            center = center.child(div().text_xs().child(format!(
                    "Displayed {} · frame {} / {} · {:.3}s{}",
                    &d.identity().source_revision[..12.min(d.identity().source_revision.len())],
                    self.preview_state.position(),
                    d.timeline.total_frames,
                    self.preview_state.position() as f64 / d.timeline.fps as f64,
                    if project
                        .as_ref()
                        .is_some_and(|p| p.source.as_str() != d.identity().source_revision)
                    {
                        " · prior revision"
                    } else {
                        ""
                    }
                )));
        }
        center = center.child(
            div()
                .flex()
                .gap_2()
                .flex_wrap()
                .child(self.button(
                    "build-preview",
                    "Build preview",
                    enabled && project.as_ref().is_some_and(|p| p.worker_available),
                    cx,
                    |s, cx| s.dispatch(Command::Build, cx),
                ))
                .child(self.button(
                    "cancel-preview",
                    "Cancel build",
                    self.preview.is_some(),
                    cx,
                    |s, cx| s.dispatch(Command::CancelBuild, cx),
                ))
                .child(
                    self.button(
                        "play-pause",
                        if self.transport.playing() || self.resume_install {
                            "Pause"
                        } else {
                            "Play"
                        },
                        self.displayed
                            .as_ref()
                            .is_some_and(|d| d.timeline.total_frames > 0),
                        cx,
                        |s, cx| s.toggle_playback(cx),
                    ),
                )
                .child(self.button(
                    "mute-audio",
                    if self.muted { "Unmute (M)" } else { "Mute (M)" },
                    self.displayed.is_some(),
                    cx,
                    |s, cx| s.toggle_mute(cx),
                ))
                .child(self.button(
                    "next-audio-output",
                    "Next output",
                    self.displayed.is_some(),
                    cx,
                    |s, cx| s.cycle_output(cx),
                ))
                .child(self.button(
                    "retry-audio-output",
                    "Retry audio",
                    self.displayed.is_some(),
                    cx,
                    |s, cx| s.change_output(OutputDevice::Default, cx),
                ))
                .child(self.button(
                    "no-audio-output",
                    "No audio",
                    self.displayed.is_some(),
                    cx,
                    |s, cx| s.change_output(OutputDevice::Unavailable, cx),
                ))
                .child(
                    self.button(
                        "previous-frame",
                        "Previous frame",
                        self.displayed
                            .as_ref()
                            .is_some_and(|d| d.timeline.total_frames > 0),
                        cx,
                        |s, cx| s.step_preview(-1, cx),
                    ),
                )
                .child(
                    self.button(
                        "next-frame",
                        "Next frame",
                        self.displayed
                            .as_ref()
                            .is_some_and(|d| d.timeline.total_frames > 0),
                        cx,
                        |s, cx| s.step_preview(1, cx),
                    ),
                )
                .child(self.button("refresh", "Refresh", enabled, cx, |s, cx| {
                    s.dispatch(Command::Refresh, cx)
                }))
                .child(self.button(
                    "checkpoint",
                    "Save checkpoint",
                    enabled && project.is_some(),
                    cx,
                    |s, cx| s.dispatch(Command::Checkpoint, cx),
                ))
                .child(self.button(
                    "restore",
                    "Restore as copy…",
                    enabled && project.is_some(),
                    cx,
                    |s, cx| s.pick(Picker::Restore, cx),
                )),
        );
        if let Some(p) = &project {
            let mut history = div().flex().gap_2().flex_wrap();
            if let Some(draft) = p.draft.clone() {
                history = history.child(self.button(
                    "draft",
                    "Reveal retained draft",
                    true,
                    cx,
                    move |_, cx| cx.reveal_path(&draft),
                ));
            }
            let checkpoint = p.checkpoint.clone();
            history = history.child(self.button(
                "reveal-checkpoint",
                "Reveal checkpoint",
                true,
                cx,
                move |_, cx| cx.reveal_path(&checkpoint),
            ));
            center = center.child(history);
        }
        center = center.child(self.timeline.clone());
        let mut agent = div()
            .w(px(280.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .p_4()
            .gap_4()
            .border_l_1()
            .border_color(rgb(BORDER))
            .bg(rgb(PANEL))
            .child("Agent")
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("Agent editing is not available yet."),
            )
            .child(div().mt_4().child("Managed SDK"))
            .child(div().text_xs().text_color(rgb(MUTED)).child(if self.busy {
                "Operation in progress…".into()
            } else {
                self.presentation.sdk.clone()
            }))
            .child(
                self.button("install", "Install managed SDK", enabled, cx, |s, cx| {
                    s.dispatch(Command::InstallSdk, cx)
                }),
            )
            .child(self.button(
                "sdk",
                "Select installed SDK…",
                enabled && project.is_some(),
                cx,
                |s, cx| s.pick(Picker::Sdk, cx),
            ))
            .child(div().mt_4().child("Recent projects"));
        let mut recents = div()
            .id("recents")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2();
        if self.presentation.recents.is_empty() {
            recents = recents.child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child("No recent projects"),
            );
        }
        for recent in self.presentation.recents.clone() {
            let id = String::from(recent.id.clone());
            let path = recent.path.clone();
            let project_id = recent.id;
            recents = recents
                .child(self.button(
                    format!("recent-{id}"),
                    format!(
                        "{}{}",
                        recent.name,
                        if recent.missing { " · Missing" } else { "" }
                    ),
                    enabled && !recent.missing,
                    cx,
                    move |s, cx| s.dispatch(Command::Open(path.clone()), cx),
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(recent.path.display().to_string()),
                );
            if recent.missing {
                let locate_id = project_id.clone();
                recents = recents.child(self.button(
                    format!("locate-{id}"),
                    "Locate…",
                    enabled,
                    cx,
                    move |s, cx| s.pick(Picker::Locate(locate_id.clone()), cx),
                ));
            }
            recents = recents.child(self.button(
                format!("remove-{id}"),
                "Remove recent",
                enabled,
                cx,
                move |s, cx| s.dispatch(Command::RemoveRecent(project_id.clone()), cx),
            ));
        }
        agent = agent.child(recents);
        let mut root = div()
            .size_full()
            .track_focus(&self.focus)
            .tab_group()
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab" {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .bg(rgb(BACKGROUND))
            .text_color(rgb(TEXT))
            .flex()
            .flex_col()
            .child(header);
        if let Some(error) = &self.error {
            root = root.child(
                div()
                    .id("error")
                    .max_h(px(120.))
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .p_3()
                    .bg(rgb(0x442c27))
                    .text_sm()
                    .child(error.clone()),
            );
        }
        if let Some(path) = self.failed_open.clone().filter(|_| {
            self.error.as_ref().is_some_and(|e| {
                e.contains("Duplicate project ID")
                    || e.contains("Relocated project content differs")
            })
        }) {
            root = root.child(self.button(
                "independent",
                "Use this folder as an independent project",
                enabled,
                cx,
                move |s, cx| s.dispatch(Command::Independent(path.clone()), cx),
            ));
        }
        root.child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(navigation)
                .child(center)
                .child(agent),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn restore_command_exports_selected_checkpoint_without_requiring_healthy_source() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("video");
        let paths = AppPaths::new(temp.path().join("history")).unwrap();
        studio_project::create(
            &root,
            "Video",
            sdk_pin(&CompatibilityManifest::default_linux_x64()),
            "1.1.0",
            "0.1.0",
        )
        .unwrap();
        let mut controller = Controller::open(&root, &paths).unwrap();
        let selected = controller.state().accepted().clone();
        let saved = fs::read(root.join("src/lib.rs")).unwrap();
        fs::write(root.join("src/lib.rs"), b"// newer checkpoint").unwrap();
        controller.checkpoint().unwrap();
        assert_ne!(controller.state().accepted(), &selected);
        fs::write(root.join("studio.json"), b"invalid source manifest").unwrap();
        let mut backend = Backend {
            paths: Some(paths),
            controller: Some(controller),
            compatibility: None,
            sdk_home: None,
            sdk: String::new(),
            closed: Arc::new(AtomicBool::new(false)),
            processes: studio_bootstrap::ProcessTreeManager::new(),
            build_spec: None,
        };
        let destination = temp.path().join("restored");
        backend
            .command(Command::Restore(destination.clone(), selected))
            .unwrap();
        assert_eq!(fs::read(destination.join("src/lib.rs")).unwrap(), saved);
        assert!(studio_project::open(&destination).is_ok());
        assert_eq!(
            fs::read(root.join("src/lib.rs")).unwrap(),
            b"// newer checkpoint"
        );
        assert_eq!(
            fs::read(root.join("studio.json")).unwrap(),
            b"invalid source manifest"
        );
    }
}
