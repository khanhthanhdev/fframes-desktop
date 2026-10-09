use crate::design_system::colors::{ACCENT, BACKGROUND, BORDER, MUTED, PANEL, TEXT};
use crate::{
    agent_workflow::{AgentWorkflow, BuildSettings},
    conversation_panel::{
        Attachment, ConversationPanel, PanelEvent,
        host::{HostInbox, open_workflow},
        qualification::OwnershipPolicy,
    },
    project_view::ProjectPresentation,
    setup_view::SetupView,
};
use crate::{
    audio_service::{AudioEvent, AudioService, OutputDevice, OutputHandle},
    canvas_view::CanvasViewState,
    design_system::{self, ButtonStyle},
    export_service::{ExportControl, ExportProgress, export_mp4},
    frame_image::{ImagePresentationManager, create_render_image},
    preset_panel::{
        Catalog, PresetCommand, PresetEvent, PresetPanel, PresetPick, PresetReport, PresetView,
    },
    preview_coordinator::{BuildSpec, PreviewCoordinator, SeekIntent},
    timeline_view::{TimelineEvent, TimelineView},
};
use gpui::{
    AppContext, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, canvas, div, px, rgb,
};
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    time::{Duration, Instant},
};
use studio_engine::{
    Controller, TaskScope, app_paths::AppPaths, build_materialization::sdk_pin, store::Store,
};
use studio_project::{ProjectId, manifest::CargoEntry};
use studio_sdk::{CompatibilityManifest, Doctor};

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
    /// The open project's controller, shared with the agent workflow.
    controller: Option<Arc<Mutex<Controller>>>,
    paths: Option<AppPaths>,
    /// The installed SDK agent tasks compile and preview against (set only while it
    /// verifies and supports the preview contract). Separate from agent readiness.
    agent_sdk: Option<AgentSdk>,
    /// Style-preset controls state of the open project (`None` when none is open).
    preset: Option<PresetView>,
}

#[derive(Clone)]
struct AgentSdk {
    dir: PathBuf,
    compatibility: CompatibilityManifest,
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
    ImportSdkBundle(PathBuf),
    Restore(PathBuf, studio_project::SourceRevision),
    Independent(PathBuf),
    /// A style-preset operation, fenced by the source revision the UI last showed.
    Preset(PresetCommand, Option<studio_project::SourceRevision>),
}
enum Picker {
    Create,
    Restore,
    Open,
    Import,
    Asset,
    Sdk,
    SdkBundle,
    Locate(ProjectId),
    PresetImport,
    PresetCss,
    PresetExport(String),
    Export,
}
struct Backend {
    paths: Option<AppPaths>,
    /// Shared with the agent workflow, which locks it only for short engine calls and for
    /// capture/publication on its own job threads.
    controller: Option<Arc<Mutex<Controller>>>,
    compatibility: Option<CompatibilityManifest>,
    sdk_home: Option<PathBuf>,
    sdk: String,
    agent_sdk: Option<AgentSdk>,
    closed: Arc<AtomicBool>,
    processes: studio_bootstrap::ProcessTreeManager,
    build_spec: Option<BuildSpec>,
    /// Bundled presets plus the verified packages imported into the app's store.
    catalog: Catalog,
    /// The outcome of the last preset command, taken by the dispatch that ran it.
    preset_report: Option<PresetReport>,
}
impl Backend {
    fn command(&mut self, command: Command) -> Result<(), String> {
        if matches!(command, Command::Initialize) {
            self.paths = Some(AppPaths::system().map_err(|e| e.to_string())?);
            let (compatibility, sdk_home) = SetupView::defaults()?;
            self.compatibility = Some(compatibility);
            self.sdk_home = Some(sdk_home);
            self.catalog = Catalog::load(self.paths.as_ref().expect("just set"));
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
                    .lock()
                    .copy_asset_with_cancel(&path, || closed.load(Ordering::Acquire))
                    .map_err(|e| e.to_string())?;
            }
            Command::Refresh => {
                if let Some(controller) = &self.controller {
                    controller.lock().reconcile().map_err(|e| e.to_string())?;
                }
                self.check_sdk();
            }
            Command::Checkpoint => self
                .current()?
                .lock()
                .checkpoint()
                .map_err(|e| e.to_string())?,
            Command::Build => {
                let builds = paths.builds();
                let fallback = self.sdk_home.as_ref().map(|h| h.join("active"));
                let controller = self.current()?;
                let mut controller = controller.lock();
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
                    service: crate::worker_project::shared_build_service(),
                });
            }
            Command::CancelBuild => {
                let controller = self.current()?;
                let mut controller = controller.lock();
                if controller.state().active_tag().is_some() {
                    controller.cancel().map_err(|e| e.to_string())?;
                }
            }
            Command::Close => {
                if let Some(controller) = &self.controller {
                    let mut controller = controller.lock();
                    // Compiles for this project stop; leases held by live workers survive.
                    crate::worker_project::shared_build_service().close_project(&String::from(
                        controller.project.manifest.project_id.clone(),
                    ));
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
                    .lock()
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
            Command::ImportSdkBundle(bundle) => {
                if let Err(error) = SetupView::install_sdk_from_bundle(
                    &bundle,
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
                .lock()
                .export_checkpoint(&revision, &path)
                .map_err(|e| e.to_string())?,
            Command::Independent(path) => {
                studio_project::lifecycle::assign_independent_identity(&path)
                    .map_err(|e| e.to_string())?;
                self.open(&path, &paths)?;
            }
            Command::Preset(command, expected) => {
                let controller = self.current()?;
                let report = crate::preset_panel::execute(
                    command,
                    &controller,
                    &mut self.catalog,
                    &paths,
                    expected.as_ref(),
                );
                let refusal = report.refused().then(|| report.status_lines().join(" "));
                self.preset_report = Some(report);
                if let Some(refusal) = refusal {
                    return Err(refusal);
                }
            }
        }
        Ok(())
    }
    fn current(&self) -> Result<Arc<Mutex<Controller>>, String> {
        self.controller
            .clone()
            .ok_or_else(|| "Open a project first".into())
    }
    fn open(&mut self, path: &Path, paths: &AppPaths) -> Result<(), String> {
        if self
            .controller
            .as_ref()
            .is_some_and(|c| c.lock().project.root == path)
        {
            return self
                .current()?
                .lock()
                .reconcile()
                .map_err(|e| e.to_string());
        }
        let candidate = Controller::open(path, paths).map_err(|e| e.to_string())?;
        if let Some(previous) = &self.controller {
            let mut previous = previous.lock();
            crate::worker_project::shared_build_service()
                .close_project(&String::from(previous.project.manifest.project_id.clone()));
            previous.close().map_err(|e| e.to_string())?;
        }
        self.controller = Some(Arc::new(Mutex::new(candidate)));
        self.check_sdk();
        Ok(())
    }
    fn check_sdk(&mut self) {
        self.agent_sdk = None;
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
            .and_then(|c| c.lock().sdk_path().map(PathBuf::from))
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
                    .is_some_and(|c| c.lock().project.manifest.sdk != sdk_pin(&selected))
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
                    // Agent tasks compile and preview candidates, which needs the M2
                    // preview contract; a legacy-worker SDK stays usable for the rest.
                    if selected
                        .preview_contract_versions
                        .contains(&fframes_studio_protocol::PREVIEW_CONTRACT_VERSION)
                    {
                        self.agent_sdk = Some(AgentSdk {
                            dir: path.clone(),
                            compatibility: selected.clone(),
                        });
                    }
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
        // One short look at the controller; the workflow's job threads may hold it too.
        let open = self.controller.as_ref().map(|c| {
            let mut controller = c.lock();
            (
                ProjectPresentation::from_controller(&controller),
                controller.state().clone(),
                controller.processes.clone(),
                PresetView::capture(&mut controller, &self.catalog),
            )
        });
        Ok(Presentation {
            project: open.as_ref().map(|(project, _, _, _)| project.clone()),
            recents,
            sdk: self.sdk.clone(),
            state: open.as_ref().map(|(_, state, _, _)| state.clone()),
            preset: open.as_ref().map(|(_, _, _, preset)| preset.clone()),
            owner: open.map(|(_, _, owner, _)| owner),
            controller: self.controller.clone(),
            paths: self.paths.clone(),
            agent_sdk: self.agent_sdk.clone(),
        })
    }
}

struct PreparedInstall {
    ready: Arc<studio_engine::ReadyPreview>,
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
    canvas: CanvasViewState,
    canvas_origin: (f64, f64),
    canvas_extent: (f64, f64),
    canvas_pan_position: Option<(f64, f64)>,
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
    /// Set while the accepted source is ahead of what the preview shows: a handoff that
    /// failed (or has not finished) keeps the old preview playing under this label.
    awaiting_preview: Option<String>,
    panel: gpui::Entity<ConversationPanel>,
    _panel_subscription: gpui::Subscription,
    preset_panel: gpui::Entity<PresetPanel>,
    _preset_subscription: gpui::Subscription,
    agent: AgentHost,
    /// A promotion changed the source: take a fresh presentation (without cancelling the
    /// staged preview that was just adopted for exactly that source).
    refresh_wanted: bool,
    presentation_refresh_pending: bool,
    /// When the accepted source started waiting for a preview that nothing is preparing.
    awaiting_since: Option<Instant>,
    export_events: Option<Receiver<ExportProgress>>,
    export_status: Option<ExportProgress>,
    export_cancelled: Option<Arc<ExportControl>>,
    export_processes: Option<studio_bootstrap::ProcessTreeManager>,
}

/// The shell's side of the per-project agent workflow: which project it belongs to, how
/// it was opened and what must still be reported to it. Workflow threads reach the UI only
/// through [`HostInbox`], which the frame loop drains.
struct AgentHost {
    inbox: Arc<HostInbox>,
    workflow: Option<Arc<AgentWorkflow>>,
    session: Option<studio_engine::OpenSession>,
    /// Serial of the open in flight, if any; a result with another serial is closed.
    opening: Option<u64>,
    /// Serial of the workflow currently attached (hand-offs carry it).
    attached: Option<u64>,
    serial: u64,
    /// The SDK directory the workflow's build settings were last set to.
    applied_sdk: Option<PathBuf>,
    playhead: usize,
    /// A revision the preview now displays, still to be reported.
    displayed: Option<String>,
    last_sync: Instant,
    /// Who may say a writer is contained: validated qualification evidence only, except
    /// for the explicit test injection of the `qualify-m3` observation entry.
    ownership: OwnershipPolicy,
}

impl AgentHost {
    fn new() -> Self {
        Self {
            inbox: Arc::new(HostInbox::default()),
            workflow: None,
            session: None,
            opening: None,
            attached: None,
            serial: 0,
            applied_sdk: None,
            playhead: 0,
            displayed: None,
            last_sync: Instant::now(),
            ownership: OwnershipPolicy::Validated,
        }
    }
}

fn agent_build(sdk: &AgentSdk) -> BuildSettings {
    BuildSettings {
        service: crate::worker_project::shared_build_service(),
        sdk: sdk.dir.clone(),
        compatibility: sdk.compatibility.clone(),
    }
}

/// What a completed command's fresh presentation means for the preparation in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationVerdict {
    /// Take the presentation; preparation (if any) is still the right one.
    Replace,
    /// The source moved on without a matching promotion: preparation is obsolete.
    CancelPreparation,
    /// Captured before a publication the shell already knows: it replaces nothing and
    /// cancels nothing (a fresh presentation is taken instead).
    Older,
}

/// Decides what a presentation captured by a command means. A refresh that raced a
/// promotion's handoff must not cancel the preparation adopted for exactly that
/// promotion: only preparation that is obsolete under the *returned* state is cancelled,
/// and a presentation older than what the shell already knows is ignored.
pub fn presentation_verdict(
    preview: &studio_engine::PreviewState,
    known_generation: Option<u64>,
    incoming: Option<&studio_engine::ProjectState>,
) -> PresentationVerdict {
    let incoming_generation = incoming.map(studio_engine::ProjectState::generation);
    if let (Some(known), Some(new)) = (known_generation, incoming_generation)
        && new < known
    {
        return PresentationVerdict::Older;
    }
    if known_generation == incoming_generation {
        return PresentationVerdict::Replace;
    }
    if let Some(state) = incoming
        && let Some(promotion) = state.promotion()
        && preview.candidate() == Some(promotion.tag())
        && promotion.is_current(state)
    {
        return PresentationVerdict::Replace;
    }
    PresentationVerdict::CancelPreparation
}

/// The nonblocking source fence for committing a preview: the controller's guard, only
/// while it is free and no source scan is pending. Holding the guard excludes every
/// workflow publication and reconcile (they lock the same controller), so whatever is
/// validated against `guard.state()` is still true when the commit completes.
pub fn install_fence(
    controller: &Mutex<Controller>,
) -> Option<parking_lot::MutexGuard<'_, Controller>> {
    let guard = controller.try_lock()?;
    (!guard.changed_hint()).then_some(guard)
}

/// Closes a workflow away from the UI thread (cancels its task, reaps its scopes).
fn close_workflow_detached(workflow: Arc<AgentWorkflow>) {
    std::thread::spawn(move || workflow.close());
}

impl StudioShell {
    /// The text shown while the accepted source has no matching preview yet.
    pub fn awaiting_preview(&self) -> Option<&str> {
        self.awaiting_preview.as_deref()
    }

    /// Guarded handoff of a just-published task revision to playback.
    ///
    /// `staged` is the preview Stage 2's validation prepared for the candidate; it is
    /// adopted under the engine's fresh authorization and re-primed at the latest
    /// playhead before the usual matching video/audio commit (with a new audio epoch)
    /// installs it. If there is no staged preview or authorization, or adoption fails,
    /// the old preview keeps playing, the accepted source is labelled as awaiting its
    /// preview, and an ordinary build of the accepted source is queued. Committed
    /// source is never undone implicitly.
    ///
    /// Returns `Ok(())` when the coordinator adopted the staged preview, otherwise the
    /// reason the accepted revision is left awaiting an ordinary preview build.
    pub fn adopt_promotion(
        &mut self,
        staged: Option<crate::preview_coordinator::StagedPreview>,
        promotion: &studio_engine::Promotion,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let published = promotion.record.published.as_str();
        self.awaiting_preview = Some(format!(
            "Accepted source {} is awaiting its preview; the preview still shows the previous revision.",
            &published[..12]
        ));
        let handoff = match (
            staged,
            promotion.authorization.as_ref(),
            self.preview.as_ref(),
        ) {
            (Some(staged), Some(authorization), Some(preview)) => {
                let latest = SeekIntent {
                    identity: self
                        .preview_state
                        .displayed()
                        .cloned()
                        .unwrap_or_else(|| staged.ready().identity().clone()),
                    serial: self.preview_state.serial(),
                    position: self.preview_state.position(),
                    scale: self.preview_state.scale(),
                };
                preview
                    .adopt(staged, authorization, latest)
                    .map(|()| authorization.clone())
                    .map_err(|e| e.to_string())
            }
            (_, None, _) => {
                Err("the source changed again before the preview could be handed off".into())
            }
            _ => Err("no staged preview is available".into()),
        };
        // The published bytes changed the source: the presentation catches up once idle.
        self.refresh_wanted = true;
        let outcome = match handoff {
            Ok(authorization) => {
                // Preview-state bookkeeping follows the adoption so a rejected adoption
                // leaves nothing half-begun.
                self.preview_state.begin_promotion(&authorization);
                Ok(())
            }
            Err(reason) => {
                self.error = Some(format!(
                    "Preview handoff failed ({reason}); rebuilding the accepted source."
                ));
                self.dispatch(Command::Build, cx);
                Err(reason)
            }
        };
        cx.notify();
        outcome
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(false);
        window.focus(&focus, cx);
        let timeline = cx.new(TimelineView::new);
        let timeline_subscription =
            cx.subscribe(&timeline, |shell, _, event, cx| match event.clone() {
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
                TimelineEvent::ScopeChanged(scope) => {
                    let scope = scope.map(|scope| scope.map(|scope| *scope));
                    shell.install_task_scope(scope, cx);
                }
            });
        let panel = cx.new(ConversationPanel::new);
        let panel_subscription =
            cx.subscribe_in(&panel, window, |shell, _, event, window, cx| match event {
                PanelEvent::Leave => window.focus(&shell.focus, cx),
            });
        let preset_panel = cx.new(PresetPanel::new);
        let preset_subscription =
            cx.subscribe_in(&preset_panel, window, |shell, _, event, window, cx| {
                shell.preset_event(event.clone(), window, cx)
            });
        let closed = Arc::new(AtomicBool::new(false));
        let processes = studio_bootstrap::ProcessTreeManager::new();
        let backend = Arc::new(Mutex::new(Backend {
            paths: None,
            controller: None,
            compatibility: None,
            sdk_home: None,
            sdk: "Checking SDK…".into(),
            agent_sdk: None,
            closed: closed.clone(),
            processes: processes.clone(),
            build_spec: None,
            catalog: Catalog::default(),
            preset_report: None,
        }));
        let owner = backend.clone();
        let shutdown = closed.clone();
        let clock_start = Instant::now();
        let audio = Arc::new(AudioService::new(clock_start));
        let quit_audio = audio.clone();
        cx.on_app_quit(move |shell, cx| {
            shutdown.store(true, Ordering::Release);
            shell.cancel_export(cx);
            quit_audio.shutdown();
            let audio = quit_audio.clone();
            let preview = shell.preview.take();
            // The workflow reaps its task, broker, compilers and workers before the
            // controller they use is closed.
            let workflow = shell.agent.workflow.take();
            let displayed = shell.displayed.take();
            let pending = shell.pending_ready.take();
            shell.output = None;
            let owner = owner.clone();
            let processes = processes.clone();
            cx.background_executor().spawn(async move {
                if let Some(workflow) = workflow {
                    workflow.close();
                }
                processes.shutdown(Duration::ZERO);
                // Compiles are owned by the shared service, not the app process scope.
                crate::worker_project::shared_build_service().close();
                audio.join();
                if let Some(preview) = preview {
                    preview.close();
                    drop(preview);
                }
                drop((displayed, pending));
                // A hash/copy may still own the serialized queue. Do not wait
                // unboundedly on it during quit; durable jobs recover as interrupted.
                if let Some(mut backend) = owner.try_lock_for(Duration::from_millis(50))
                    && let Some(controller) = backend.controller.take()
                    && let Some(mut controller) = controller.try_lock_for(Duration::from_millis(50))
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
            canvas: CanvasViewState::default(),
            canvas_origin: (0.0, 0.0),
            canvas_extent: (0.0, 0.0),
            canvas_pan_position: None,
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
            awaiting_preview: None,
            panel,
            _panel_subscription: panel_subscription,
            preset_panel,
            _preset_subscription: preset_subscription,
            agent: AgentHost::new(),
            refresh_wanted: false,
            presentation_refresh_pending: false,
            awaiting_since: None,
            export_events: None,
            export_status: None,
            export_cancelled: None,
            export_processes: None,
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
                    shell.poll_agent(cx);
                    shell.poll_export(cx);
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
                            .is_some_and(|c| c.lock().changed_hint())
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
    /// M3 observation entry: opens `project` and writes the agent panel's redacted state
    /// (phase, counts, bounds; never prompts or message text) to `output` every 250 ms.
    /// Nothing is launched and no command is sent; an operator drives the real panel.
    ///
    /// `test_ownership` is the explicit test-only injection for fixtures: with it the
    /// adapter's writers count as that ownership WITHOUT any qualification evidence (the
    /// panel says so); without it only validated evidence can qualify an adapter, exactly
    /// as in the product.
    pub fn start_agent_telemetry(
        &mut self,
        project: PathBuf,
        output: PathBuf,
        test_ownership: Option<studio_bootstrap::WriterOwnership>,
        cx: &mut Context<Self>,
    ) {
        if let Some(ownership) = test_ownership {
            self.agent.ownership = OwnershipPolicy::TestInjected(ownership);
        }
        // Observation only: measured bounds of the named transport buttons go to the
        // telemetry so a driver clicks what the shell actually laid out.
        self.qualifying = true;
        cx.spawn(async move |this, cx| {
            let mut opened = false;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let Ok(value) = this.update(cx, |shell, cx| {
                    if !opened && !shell.busy {
                        shell.dispatch(Command::Open(project.clone()), cx);
                        opened = true;
                    }
                    serde_json::json!({
                        "project_open": shell.presentation.project.is_some(),
                        "sdk_ready_for_agent": shell.presentation.agent_sdk.is_some(),
                        "agent_workflow_open": shell.agent.workflow.is_some(),
                        "awaiting_preview": shell.awaiting_preview.is_some(),
                        "displayed_revision": shell
                            .displayed
                            .as_ref()
                            .map(|d| d.identity().source_revision.chars().take(12).collect::<String>()),
                        "displayed_identity": shell.displayed.as_ref().map(|d| serde_json::json!({
                            "open_session_len": d.identity().open_session.len(),
                            "worker_generation": d.identity().worker_generation,
                            "timeline_open_session_len": d.timeline.envelope.identity.open_session.len(),
                            "timeline_worker_generation": d.timeline.envelope.identity.worker_generation,
                        })),
                        "panel": shell.panel.read(cx).telemetry(),
                        "preview": shell.preview_telemetry(),
                        "canvas": shell.canvas_telemetry(),
                        "buttons": serde_json::to_value(&shell.button_bounds)
                            .unwrap_or(serde_json::Value::Null),
                        "ruler_bounds": shell.timeline.read(cx).qualification_metrics()["ruler_bounds"].clone(),
                        "timeline_selection": shell.timeline.read(cx).qualification_metrics()["selection"].clone(),
                    })
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
                        shell.error = Some(format!("Agent telemetry: {error}"));
                        cx.notify();
                    });
                    break;
                }
            }
        })
        .detach();
    }
    /// Redacted playback identity for the M3 observation: revisions are 12-hex prefixes,
    /// the audio digest is the prepared mix's hash prefix; nothing here is project text.
    fn preview_telemetry(&self) -> serde_json::Value {
        let prefix = |text: &str| text.chars().take(12).collect::<String>();
        serde_json::json!({
            "status": match &self.preview_state.status {
                studio_engine::PreviewStatus::Absent => "absent",
                studio_engine::PreviewStatus::Building => "building",
                studio_engine::PreviewStatus::Preparing => "preparing",
                studio_engine::PreviewStatus::Ready => "ready",
                studio_engine::PreviewStatus::Displayed => "displayed",
                studio_engine::PreviewStatus::Error(_) => "error",
                studio_engine::PreviewStatus::Closed => "closed",
            },
            "error": match &self.preview_state.status {
                studio_engine::PreviewStatus::Error(e) => Some(format!("{e:?}").chars().take(4000).collect::<String>()),
                _ => None,
            },
            "transport_epoch": self.transport.epoch(),
            "playing": self.transport.playing(),
            "seek_serial": self.preview_state.serial(),
            "position": self.preview_state.position(),
            "audio_output": self.output.is_some(),
            "painted_frame": self.painted_frame,
            "presented_serial": self.presented_serial,
            "displayed": self.displayed.as_ref().map(|d| serde_json::json!({
                "video_revision": prefix(&d.identity().source_revision),
                "audio_revision": prefix(&d.audio.envelope.identity.source_revision),
                "audio_sha256": prefix(&d.audio.sha256),
                "audio_samples": d.audio.sample_count,
                "sample_rate": d.audio.sample_rate,
                "fps": d.timeline.fps,
                "pcm_start_sample": d.pcm_start_sample,
                "pcm_samples": d.pcm.len() / 8,
                "frame_index": d.frame.as_ref().map(|f| f.response.frame_index),
                "frame_serial": d.frame.as_ref().map(|f| f.response.seek_serial),
                "total_frames": d.timeline.total_frames,
                "position": d.position,
                "seek_serial": d.seek_serial,
            })),
        })
    }
    /// Bounded native-selection observations for the opt-in qualification telemetry.
    /// Only semantic keys and geometry are recorded; authored source and image bytes are not.
    fn canvas_telemetry(&self) -> serde_json::Value {
        let metadata = self
            .canvas
            .displayed
            .as_ref()
            .and_then(|displayed| displayed.metadata.as_ref());
        let objects: Vec<_> = metadata
            .into_iter()
            .flat_map(|metadata| metadata.objects.iter())
            .take(64)
            .map(|object| {
                serde_json::json!({
                    "identity": object.identity,
                    "bounds": object.bounds,
                    "paint_order": object.paint_order,
                    "support": format!("{:?}", object.support),
                })
            })
            .collect();
        let selection = self.canvas.selection.as_ref().map(|selection| {
            serde_json::json!({
                "identity": selection.identity,
                "bounds": selection.bounds,
                "support": format!("{:?}", selection.support),
            })
        });
        let viewport = self.canvas.viewport.as_ref();
        let image_bounds = viewport.map(|viewport| {
            let bounds = viewport.image_bounds();
            serde_json::json!({
                "x": bounds.x,
                "y": bounds.y,
                "width": bounds.width,
                "height": bounds.height,
            })
        });
        let rectangle = self.canvas.rectangle_scope.map(|rectangle| {
            serde_json::json!({
                "x": rectangle.x,
                "y": rectangle.y,
                "width": rectangle.width,
                "height": rectangle.height,
            })
        });
        let rendered_image = self.images.current_image();
        let rendered_image_size = rendered_image.as_ref().map(|image| image.size(0));
        let rendered_bright_pixels = rendered_image
            .as_ref()
            .and_then(|image| image.as_bytes(0))
            .map(|pixels| {
                pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|pixel| pixel[..3].iter().all(|channel| *channel > 180) && pixel[3] > 0)
                    .count()
            });
        serde_json::json!({
            "frame_index": self.canvas.displayed.as_ref().map(|frame| frame.frame_index),
            "seek_serial": self.canvas.displayed.as_ref().map(|frame| frame.seek_serial),
            "metadata_status": metadata.map(|metadata| format!("{:?}", metadata.status)),
            "object_count": metadata.map_or(0, |metadata| metadata.objects.len()),
            "objects_truncated": metadata.is_some_and(|metadata| metadata.objects.len() > 64),
            "objects": objects,
            "selection": selection,
            "rectangle": rectangle,
            "drawing_rectangle": self.canvas.is_drawing_rectangle(),
            "video_dimensions": viewport.map(|viewport| viewport.video_dimensions()),
            "image_bounds": image_bounds,
            "canvas_origin": self.canvas_origin,
            "canvas_extent": self.canvas_extent,
            "rendered_image_size": rendered_image_size.map(|size| (size.width.0, size.height.0)),
            "rendered_bright_pixels": rendered_bright_pixels,
            "message": self.canvas.message,
        })
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
        if matches!(
            &command,
            Command::Close
                | Command::Create(_)
                | Command::Import(_)
                | Command::Open(_)
                | Command::Locate(_, _)
        ) {
            self.cancel_export(cx);
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
        // A command that replaces the open project closes the project's agent workflow
        // first (cancelling its task and reaping its scopes), on the background thread and
        // before the controller it shares is closed.
        let closing = self.take_workflow_for(&command, cx);
        let refresh = matches!(command, Command::Refresh);
        self.busy = true;
        self.epoch += 1;
        let epoch = self.epoch;
        let backend = self.backend.clone();
        let closed = self.closed.clone();
        let task = cx.background_executor().spawn(async move {
            if let Some(workflow) = closing {
                workflow.close();
            }
            let mut backend = backend.lock();
            if closed.load(Ordering::Acquire) {
                return (Err("Window closed".into()), None, None, None);
            }
            let result = backend.command(command);
            let presentation = backend.presentation();
            let spec = backend.build_spec.take();
            let report = backend.preset_report.take();
            match presentation {
                Ok(p) => (result, Some(p), spec, report),
                Err(e) => (Err(e), None, None, report),
            }
        });
        cx.spawn(async move |this, cx| {
            let (result, presentation, spec, report) = task.await;
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
                    let verdict = if changed_session {
                        PresentationVerdict::CancelPreparation
                    } else {
                        presentation_verdict(
                            &shell.preview_state,
                            shell.presentation.state.as_ref().map(|s| s.generation()),
                            presentation.state.as_ref(),
                        )
                    };
                    match verdict {
                        PresentationVerdict::Older => {
                            // Captured before a publication this shell already knows: it
                            // replaces nothing and cancels nothing; a fresh one is taken.
                            shell.refresh_wanted = true;
                        }
                        verdict => {
                            if verdict == PresentationVerdict::CancelPreparation {
                                if let Some(preview) = &shell.preview {
                                    preview.cancel_build();
                                }
                                shell.preview_state.cancel_build();
                                shell.discard_install(cx);
                            }
                            shell.presentation = presentation;
                            shell.sync_agent(cx);
                            shell.push_preset_view(cx);
                        }
                    }
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
                if let Some(report) = report {
                    shell.adopt_preset_report(report, cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows a preset command's outcome. Only a committed revision asks for a matching
    /// preview: the previous playable preview stays displayed under the awaiting label
    /// until it is ready (or for good if its build fails), and the source is never
    /// rolled back implicitly.
    fn adopt_preset_report(&mut self, report: PresetReport, cx: &mut Context<Self>) {
        let request = report.preview_request();
        let catalog_changed = report.catalog_changed;
        self.preset_panel
            .update(cx, |panel, cx| panel.report(report, cx));
        if catalog_changed {
            self.refresh_wanted = true;
        }
        if let Some(request) = request {
            self.awaiting_preview = Some(request.label);
            self.refresh_wanted = true;
            self.dispatch(Command::Build, cx);
        }
    }

    fn push_preset_view(&mut self, cx: &mut Context<Self>) {
        let view = self.presentation.preset.clone();
        self.preset_panel
            .update(cx, |panel, cx| panel.set_view(view, cx));
    }

    fn run_preset(&mut self, command: PresetCommand, cx: &mut Context<Self>) {
        // The fence is the revision this window last showed.
        let expected = self
            .presentation
            .state
            .as_ref()
            .map(|state| state.source().clone());
        self.dispatch(Command::Preset(command, expected), cx);
    }

    fn preset_event(&mut self, event: PresetEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PresetEvent::Leave => window.focus(&self.focus, cx),
            PresetEvent::Run(command) => self.run_preset(command, cx),
            PresetEvent::Pick(PresetPick::ImportFolder) => self.pick(Picker::PresetImport, cx),
            PresetEvent::Pick(PresetPick::ImportCss) => self.pick(Picker::PresetCss, cx),
            PresetEvent::Pick(PresetPick::Export { key }) => {
                self.pick(Picker::PresetExport(key), cx)
            }
        }
    }
    /// The workflow a command must close before it may run: every command that closes or
    /// replaces the open project. Re-selecting the same folder keeps the workflow.
    fn take_workflow_for(
        &mut self,
        command: &Command,
        cx: &mut Context<Self>,
    ) -> Option<Arc<AgentWorkflow>> {
        let current = self.presentation.project.as_ref().map(|p| p.root.clone());
        let replaces = match command {
            Command::Close | Command::Import(_) | Command::Create(_) => true,
            Command::Open(path) | Command::Locate(path, _) | Command::Independent(path) => {
                current.as_ref() != Some(path)
            }
            _ => false,
        };
        if !replaces {
            return None;
        }
        self.agent.opening = None;
        self.agent.session = None;
        self.agent.attached = None;
        self.agent.applied_sdk = None;
        self.panel.update(cx, |panel, cx| panel.detach(cx));
        self.agent.workflow.take()
    }

    /// Keeps the agent workflow in step with the open project: one workflow per open
    /// session, opened on its own thread (it replays the conversation log), never
    /// launching an agent. Also forwards SDK readiness, which stays separate from the
    /// agent's.
    fn sync_agent(&mut self, cx: &mut Context<Self>) {
        let session = self
            .presentation
            .state
            .as_ref()
            .map(|state| state.session().clone());
        let sdk_ready = self.presentation.agent_sdk.is_some();
        self.panel
            .update(cx, |panel, cx| panel.set_sdk_ready(sdk_ready, cx));
        let Some(session) = session else {
            if let Some(workflow) = self.agent.workflow.take() {
                close_workflow_detached(workflow);
            }
            self.agent.session = None;
            self.agent.opening = None;
            self.agent.attached = None;
            self.panel.update(cx, |panel, cx| panel.detach(cx));
            return;
        };
        if self.agent.session.as_ref() != Some(&session) {
            // A different open session (the project was replaced some other way).
            if let Some(workflow) = self.agent.workflow.take() {
                close_workflow_detached(workflow);
            }
            self.agent.attached = None;
            self.agent.opening = None;
            self.panel.update(cx, |panel, cx| panel.detach(cx));
            self.agent.session = Some(session);
        }
        if self.agent.workflow.is_none() && self.agent.opening.is_none() {
            self.start_agent_open();
        }
        self.sync_agent_build();
    }

    fn start_agent_open(&mut self) {
        let (Some(controller), Some(paths)) = (
            self.presentation.controller.clone(),
            self.presentation.paths.clone(),
        ) else {
            return;
        };
        self.agent.serial += 1;
        let serial = self.agent.serial;
        self.agent.opening = Some(serial);
        let build = self.presentation.agent_sdk.as_ref().map(agent_build);
        self.agent.applied_sdk = self.presentation.agent_sdk.as_ref().map(|s| s.dir.clone());
        let inbox = self.agent.inbox.clone();
        let policy = self.agent.ownership.clone();
        let spawned = std::thread::Builder::new()
            .name("studio-agent-open".into())
            .spawn({
                let inbox = inbox.clone();
                move || {
                    inbox.push_opened(open_workflow(
                        controller,
                        paths,
                        build,
                        serial,
                        inbox.clone(),
                        policy,
                    ))
                }
            });
        if let Err(error) = spawned {
            self.agent.opening = None;
            self.error = Some(format!("Cannot start the agent workflow: {error}"));
        }
    }

    /// Hands a changed SDK selection to the workflow (it refuses while a task runs; the
    /// frame loop retries).
    fn sync_agent_build(&mut self) {
        let Some(workflow) = &self.agent.workflow else {
            return;
        };
        let desired = self.presentation.agent_sdk.as_ref().map(|s| s.dir.clone());
        if desired == self.agent.applied_sdk {
            return;
        }
        if workflow
            .set_build(self.presentation.agent_sdk.as_ref().map(agent_build))
            .is_ok()
        {
            self.agent.applied_sdk = desired;
        }
    }

    /// Frame-loop half of the agent workflow: adopt a finished open, adopt committed
    /// promotions on this thread (the only place the preview coordinator is touched),
    /// report what the preview displays, forward the playhead and refresh the panel.
    fn poll_agent(&mut self, cx: &mut Context<Self>) {
        let inbox = self.agent.inbox.clone();
        if inbox.take_wake() || self.agent.workflow.is_some() {
            for opened in inbox.take_opened() {
                self.adopt_opened(opened, cx);
            }
        }
        for queued in inbox.take_handoffs() {
            if self.agent.attached != Some(queued.serial) {
                // Not this project's workflow any more: the staged worker is reaped by
                // dropping it; nothing is adopted.
                continue;
            }
            let published = queued
                .handoff
                .promotion
                .record
                .published
                .as_str()
                .to_owned();
            let outcome =
                self.adopt_promotion(queued.handoff.staged, &queued.handoff.promotion, cx);
            if let Some(workflow) = &self.agent.workflow {
                let _ = workflow.report_handoff(&published, outcome);
            }
        }
        if let Some(workflow) = &self.agent.workflow {
            if let Some(revision) = self.agent.displayed.take() {
                let _ = workflow.preview_displayed(&revision);
            }
            let position = self.preview_state.position();
            if position != self.agent.playhead {
                self.agent.playhead = position;
                workflow.set_playhead(position);
            }
        }
        if self.agent.workflow.is_some()
            && self.agent.last_sync.elapsed() > Duration::from_millis(500)
        {
            self.agent.last_sync = Instant::now();
            self.sync_agent_build();
        }
        if self.refresh_wanted {
            self.refresh_wanted = false;
            self.refresh_presentation(cx);
        }
        self.rebuild_stranded_acceptance(cx);
        // Background teardown only ever reports problems; surface the newest one.
        if let Some(report) = crate::teardown::Teardown::global().take_reports().pop() {
            self.error = Some(format!(
                "Background cleanup ({}): {}",
                report.label, report.problem
            ));
            cx.notify();
        }
        self.panel.update(cx, |panel, cx| {
            panel.poll(cx);
        });
    }

    /// Replaces the presentation with the controller's current one. Unlike a command's
    /// completion this never cancels preparation: a promotion advances the generation on
    /// purpose and the staged preview adopted for it must survive.
    fn refresh_presentation(&mut self, cx: &mut Context<Self>) {
        if self.presentation_refresh_pending || self.closed.load(Ordering::Acquire) {
            return;
        }
        self.presentation_refresh_pending = true;
        let backend = self.backend.clone();
        let session = self.preview_session.clone();
        let task = cx
            .background_executor()
            .spawn(async move { backend.lock().presentation().ok() });
        cx.spawn(async move |this, cx| {
            let presentation = task.await;
            let _ = this.update(cx, |shell, cx| {
                shell.presentation_refresh_pending = false;
                if shell.preview_session != session {
                    return;
                }
                if let Some(presentation) = presentation {
                    shell.presentation = presentation;
                    shell.push_preset_view(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Safety net for an accepted source whose handoff never produced a preview (a
    /// refresh or a failed staging cancelled it): after a grace period with nothing being
    /// prepared, build the accepted source the ordinary way. Never undoes anything.
    fn rebuild_stranded_acceptance(&mut self, cx: &mut Context<Self>) {
        if self.awaiting_preview.is_none() {
            self.awaiting_since = None;
            return;
        }
        let since = *self.awaiting_since.get_or_insert_with(Instant::now);
        let preparing = matches!(
            self.preview_state.status,
            studio_engine::PreviewStatus::Building | studio_engine::PreviewStatus::Preparing
        ) || self.pending_ready.is_some();
        if preparing {
            self.awaiting_since = Some(Instant::now());
        } else if since.elapsed() > Duration::from_secs(8)
            && !self.busy
            && self.preview.is_some()
            && self
                .presentation
                .project
                .as_ref()
                .is_some_and(|p| p.worker_available)
        {
            self.awaiting_since = Some(Instant::now());
            self.dispatch(Command::Build, cx);
        }
    }

    fn adopt_opened(
        &mut self,
        opened: crate::conversation_panel::host::Opened,
        cx: &mut Context<Self>,
    ) {
        let wanted = self.agent.opening == Some(opened.serial);
        let workflow = match opened.result {
            Ok(workflow) => workflow,
            Err(error) => {
                if wanted {
                    self.agent.opening = None;
                    self.error = Some(format!("Agent workflow unavailable: {error}"));
                    cx.notify();
                }
                return;
            }
        };
        if !wanted {
            // Opened for a project the shell has left.
            std::thread::spawn(move || workflow.close());
            return;
        }
        self.agent.opening = None;
        let workflow = Arc::new(workflow);
        let preview_identity = self
            .displayed
            .as_ref()
            .map(|preview| preview.identity().clone());
        let _ = workflow.set_displayed_preview_identity(preview_identity);
        self.agent.workflow = Some(workflow.clone());
        self.agent.attached = Some(opened.serial);
        self.agent.playhead = usize::MAX;
        if let Some(paths) = self.presentation.paths.clone() {
            let scope = self.timeline.read(cx).selected_scope();
            self.panel.update(cx, |panel, cx| {
                panel.attach(
                    Attachment {
                        workflow,
                        paths,
                        serial: opened.serial,
                        adapter: opened.adapter,
                        registry: opened.registry,
                        qualification: opened.qualification,
                        settings_error: opened.settings_error,
                        resolution: opened.resolution,
                        policy: opened.policy,
                    },
                    cx,
                );
            });
            self.install_task_scope(scope, cx);
        }
        self.sync_agent_build();
        cx.notify();
    }

    fn detach_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(workflow) = &self.agent.workflow {
            let _ = workflow.set_displayed_preview_identity(None);
        }
        if let Some(preview) = self.preview.take() {
            // Teardown stays off the UI thread; the root token rejects any late spawn.
            std::thread::spawn(move || preview.close());
        }
        self.preview_session = None;
        self.awaiting_preview = None;
        self.preview_state.close();
        self.displayed = None;
        self.pending_ready = None;
        self.pending_frame = None;
        self.canvas.clear();
        self.canvas_pan_position = None;
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
            let b = backend.lock();
            let mut error = None;
            let controller = b.controller.clone();
            let state = if let Some(c) = &controller {
                let mut c = c.lock();
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
                    shell.push_preset_view(cx);
                }
                if let Some((tag, e)) = error {
                    shell.preview_state.fail(&tag, e.clone());
                    shell.discard_install(cx);
                    shell.error = Some(e);
                } else if let (Some(ready), Some(state)) = (ready, state) {
                    // A staged preview of just-published bytes carries the promotion
                    // tag (base source = the published revision, the task's own
                    // generation); it is installable only under the engine's current
                    // authorization, never as a build of the task base.
                    let promoted = state.promotion().is_some_and(|p| p.tag() == ready.tag());
                    if !promoted
                        && (ready.tag().generation != state.generation()
                            || ready.tag().base_source != *state.source())
                    {
                        if let Some(preview) = &shell.preview {
                            preview.cancel_build();
                        }
                        shell.preview_state.cancel_build();
                    } else if shell.transport.playing()
                        && (promoted
                            || matches!(state.job(), studio_engine::JobState::Succeeded(tag) if tag == ready.tag())) {
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
                                shell.pending_ready = Some(PreparedInstall { ready, clock, audio: None, audio_ready: false });
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

    fn pause_at_painted_frame(&mut self, cx: &mut Context<Self>) {
        if !self.transport.playing() {
            return;
        }
        let frame = self
            .painted_frame
            .unwrap_or_else(|| self.transport.position());
        self.pause_playback(cx);
        self.seek_preview(frame, cx);
    }

    fn install_task_scope(
        &mut self,
        scope: Result<Option<TaskScope>, String>,
        cx: &mut Context<Self>,
    ) {
        let scope = scope.and_then(|scope| match scope {
            Some(mut scope) => {
                scope.canvas_selection = self.canvas.task_scope_selection();
                scope.validate().map_err(|error| error.to_string())?;
                Ok(Some(scope))
            }
            None if self.canvas.task_scope_selection().is_some() => {
                Err("canvas selection has no matching compiled timeline".into())
            }
            None => Ok(None),
        });
        self.panel
            .update(cx, |panel, cx| panel.set_task_scope(scope, cx));
    }

    fn canvas_point(&self, position: gpui::Point<gpui::Pixels>) -> (f64, f64) {
        (
            f64::from(position.x) - self.canvas_origin.0,
            f64::from(position.y) - self.canvas_origin.1,
        )
    }

    fn canvas_pointer_down(&mut self, event: &gpui::MouseDownEvent, cx: &mut Context<Self>) {
        let point = self.canvas_point(event.position);
        if event.button != MouseButton::Middle {
            self.canvas_pan_position = None;
        }
        if event.button == MouseButton::Left {
            self.pause_at_painted_frame(cx);
        }
        match event.button {
            MouseButton::Middle => self.canvas_pan_position = Some(point),
            MouseButton::Left if event.modifiers.shift => {
                if !self.canvas.begin_rectangle(point.0, point.1) {
                    self.canvas.message =
                        Some("Start a rectangle drag inside the painted video image.".into());
                }
            }
            MouseButton::Left if event.modifiers.alt => {
                if let Err(error) = self.canvas.select_at(point.0, point.1, true) {
                    self.canvas.message = Some(error.to_string());
                }
            }
            MouseButton::Left => {
                if let Err(error) = self.canvas.select_at(point.0, point.1, false) {
                    self.canvas.message = Some(error.to_string());
                }
            }
            _ => return,
        }
        self.install_task_scope(self.timeline.read(cx).selected_scope(), cx);
        cx.notify();
    }

    fn canvas_pointer_move(&mut self, event: &gpui::MouseMoveEvent, cx: &mut Context<Self>) {
        let point = self.canvas_point(event.position);
        if self.canvas.is_drawing_rectangle() {
            self.canvas.update_rectangle(point.0, point.1);
            cx.notify();
        } else if event.pressed_button == Some(MouseButton::Middle)
            && let Some(previous) = self.canvas_pan_position.replace(point)
            && let Some(viewport) = &mut self.canvas.viewport
        {
            let _ = viewport.pan_by(point.0 - previous.0, point.1 - previous.1);
            cx.notify();
        }
    }

    fn canvas_pointer_up(&mut self, button: MouseButton, cx: &mut Context<Self>) {
        if button == MouseButton::Middle {
            self.canvas_pan_position = None;
        } else if button == MouseButton::Left && self.canvas.is_drawing_rectangle() {
            if let Some(rect) = self.canvas.end_rectangle() {
                self.canvas.message = Some(format!(
                    "Rectangle scope · {:.0}×{:.0} video pixels · source identity is not inferred.",
                    rect.width, rect.height
                ));
            }
            self.install_task_scope(self.timeline.read(cx).selected_scope(), cx);
            cx.notify();
        }
    }

    fn canvas_scroll(&mut self, event: &gpui::ScrollWheelEvent, cx: &mut Context<Self>) {
        let point = self.canvas_point(event.position);
        let delta = match event.delta {
            gpui::ScrollDelta::Pixels(pixels) => f64::from(pixels.y),
            gpui::ScrollDelta::Lines(lines) => f64::from(lines.y * 32.0),
        };
        if event.modifiers.control || event.modifiers.platform {
            let factor = if delta > 0.0 {
                1.0 + delta.abs() * 0.01
            } else {
                1.0 / (1.0 + delta.abs() * 0.01)
            };
            if let Some(viewport) = &mut self.canvas.viewport {
                let _ = viewport.zoom_at(point.0, point.1, factor);
            }
        } else if let Some(viewport) = &mut self.canvas.viewport {
            let _ = viewport.pan_by(-delta, 0.0);
        }
        cx.notify();
    }

    fn cycle_canvas_selection(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.canvas.cycle_last_hit() {
            self.canvas.message = Some(error.to_string());
        }
        cx.notify();
    }

    fn reset_canvas_view(&mut self, cx: &mut Context<Self>) {
        if let Some(viewport) = &mut self.canvas.viewport {
            viewport.reset_to_fit();
        }
        cx.notify();
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
        if matches!(
            kind,
            Picker::Create | Picker::Restore | Picker::PresetExport(_) | Picker::Export
        ) {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let picker = cx.prompt_for_new_path(
                &home,
                Some(match kind {
                    Picker::Create => "New video",
                    Picker::PresetExport(_) => "Exported preset",
                    Picker::Export => "Exported video.mp4",
                    _ => "Recovered video",
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
                            if let Picker::PresetExport(key) = kind {
                                shell.run_preset(PresetCommand::Export { key, dest: path }, cx);
                            } else if matches!(kind, Picker::Create) {
                                shell.dispatch(Command::Create(path), cx);
                            } else if matches!(kind, Picker::Export) {
                                shell.start_export(path, cx);
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
                Picker::SdkBundle => "Offline SDK bundle folder",
                Picker::Locate(_) => "Locate",
                Picker::PresetImport => "Import preset folder",
                Picker::PresetCss => "CSS report",
                _ => "Open",
            };
            let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: matches!(kind, Picker::Asset | Picker::Import | Picker::PresetCss),
                directories: !matches!(kind, Picker::Asset | Picker::Import | Picker::PresetCss),
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
                                Picker::SdkBundle => Command::ImportSdkBundle(path),
                                Picker::Locate(id) => Command::Locate(path, id),
                                Picker::PresetImport => {
                                    return shell
                                        .run_preset(PresetCommand::ImportFolder(path), cx);
                                }
                                Picker::PresetCss => {
                                    return shell.run_preset(PresetCommand::ImportCss(path), cx);
                                }
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

    fn start_export(&mut self, destination: PathBuf, cx: &mut Context<Self>) {
        if self.export_cancelled.is_some() {
            return;
        }
        let (Some(controller), Some(paths), Some(sdk)) = (
            self.presentation.controller.clone(),
            self.presentation.paths.clone(),
            self.presentation
                .agent_sdk
                .as_ref()
                .map(|sdk| sdk.dir.clone()),
        ) else {
            self.export_status = Some(ExportProgress::Failed(
                "Select a compatible installed SDK and open a project before exporting".into(),
            ));
            cx.notify();
            return;
        };
        let control = Arc::new(ExportControl::default());
        let processes = studio_bootstrap::ProcessTreeManager::new();
        let (sender, receiver) = mpsc::sync_channel(16);
        self.export_cancelled = Some(control.clone());
        self.export_processes = Some(processes.clone());
        self.export_events = Some(receiver);
        self.export_status = Some(ExportProgress::Started {
            revision: "capturing validated revision".into(),
            label: "Preparing immutable project snapshot".into(),
        });
        let service = crate::worker_project::shared_build_service();
        let spawn = std::thread::Builder::new()
            .name("studio-mp4-export".into())
            .spawn(move || {
                let result = controller
                    .lock()
                    .freeze_export_source()
                    .map_err(|error| error.to_string())
                    .and_then(|source| {
                        export_mp4(
                            crate::export_service::ExportRequest {
                                source,
                                sdk,
                                builds: paths.builds(),
                                destination,
                            },
                            service,
                            processes,
                            control.clone(),
                            |event| {
                                let _ = sender.send(event);
                            },
                        )
                    });
                if let Err(error) = result {
                    let event = if control.is_cancelled() {
                        ExportProgress::Cancelled
                    } else {
                        ExportProgress::Failed(error)
                    };
                    let _ = sender.send(event);
                }
            });
        if let Err(error) = spawn {
            self.export_events = None;
            self.export_cancelled = None;
            self.export_processes = None;
            self.export_status = Some(ExportProgress::Failed(format!(
                "Could not start the export job: {error}"
            )));
        }
        cx.notify();
    }

    fn poll_export(&mut self, cx: &mut Context<Self>) {
        let Some(receiver) = &self.export_events else {
            return;
        };
        let mut terminal = false;
        loop {
            match receiver.try_recv() {
                Ok(event) => {
                    terminal = matches!(
                        event,
                        ExportProgress::Complete { .. }
                            | ExportProgress::Failed(_)
                            | ExportProgress::Cancelled
                    );
                    self.export_status = Some(event);
                    cx.notify();
                    if terminal {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    terminal = true;
                    if self
                        .export_cancelled
                        .as_ref()
                        .is_some_and(|token| token.is_cancelled())
                    {
                        self.export_status = Some(ExportProgress::Cancelled);
                    } else {
                        self.export_status = Some(ExportProgress::Failed(
                            "Export worker stopped without a completion result".into(),
                        ));
                    }
                    cx.notify();
                    break;
                }
            }
        }
        if terminal {
            self.export_events = None;
            self.export_cancelled = None;
            self.export_processes = None;
        }
    }

    fn cancel_export(&mut self, cx: &mut Context<Self>) {
        let Some(cancelled) = &self.export_cancelled else {
            return;
        };
        if !cancelled.request_cancel() {
            if cancelled.is_publishing() {
                self.export_status = Some(ExportProgress::Verifying);
                cx.notify();
            }
            return;
        }
        if let Some(processes) = self.export_processes.clone() {
            cx.background_executor()
                .spawn(async move {
                    processes.shutdown(Duration::ZERO);
                })
                .detach();
        }
        self.export_status = Some(ExportProgress::Cancelled);
        cx.notify();
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
        let style = match &id {
            gpui::ElementId::Name(name)
                if matches!(name.as_ref(), "Project" | "Assets" | "Style")
                    && name.as_ref() == self.navigation =>
            {
                ButtonStyle::Selected
            }
            gpui::ElementId::Name(name) if matches!(name.as_ref(), "build-preview" | "create") => {
                ButtonStyle::Primary
            }
            gpui::ElementId::Name(name) if name.as_ref() == "close" => ButtonStyle::Destructive,
            _ => ButtonStyle::Secondary,
        };
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
        design_system::button(
            div()
                .id(id)
                .relative()
                .role(gpui::Role::Button)
                .aria_label(label.clone())
                .tab_index(0)
                .tab_stop(enabled)
                .text_sm(),
            style,
            enabled,
        )
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
            // No filesystem work here. The controller's nonblocking guard is the source
            // fence and it is held from the final `can_install` check through the
            // coordinator commit and the preview-state install: a workflow publication
            // or a source reconcile cannot interleave, and every check reads the guard's
            // current state, never a copy taken when preparation finished. Unavailable
            // (a job holds the controller, or a source scan is pending) means deferred.
            let backend = self.backend.clone();
            let locked = backend.try_lock();
            let controller = locked.as_ref().and_then(|b| b.controller.clone());
            let fence = controller.as_deref().and_then(install_fence);
            let current = fence.as_ref().map(|c| c.state());
            if current.is_none() || !install.audio_ready {
                committed = true; // Deferred, not rejected.
                self.pending_ready = Some(install);
            } else if let Some(source) = current
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
                            if let Some(frame) = ready.frame.as_ref() {
                                if let Err(error) =
                                    self.canvas.install_frame(ready.identity().clone(), frame)
                                {
                                    self.canvas.clear();
                                    self.canvas.message =
                                        Some(format!("Canvas metadata unavailable: {error}"));
                                }
                            } else {
                                self.canvas.clear();
                            }
                            self.timeline
                                .update(cx, |v, cx| v.install(model.unwrap(), ready.position, cx));
                            self.displayed = Some(ready.clone());
                            if let Some(workflow) = &self.agent.workflow {
                                let _ = workflow
                                    .set_displayed_preview_identity(Some(ready.identity().clone()));
                            }
                            if ready.tag().base_source == *source.source() {
                                self.awaiting_preview = None;
                            }
                            // Tell the workflow what is on screen so its accepted-awaiting
                            // label clears exactly when the matching preview shows.
                            self.agent.displayed = Some(ready.identity().source_revision.clone());
                            self.error = None;
                            committed = true;
                        }
                    }
                    Err(e) => self.error = Some(e.to_string()),
                }
            }
            drop(fence);
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
                    if let Some(preview) = self
                        .displayed
                        .as_ref()
                        .map(|ready| ready.identity().clone())
                    {
                        if let Err(error) = self.canvas.install_frame(preview, &f) {
                            self.canvas.clear();
                            self.canvas.message =
                                Some(format!("Canvas metadata unavailable: {error}"));
                        }
                    } else {
                        self.canvas.clear();
                    }
                    self.presented_serial = f.response.seek_serial;
                    self.painted_frame = Some(f.response.frame_index);
                    self.install_task_scope(self.timeline.read(cx).selected_scope(), cx);
                }
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        self.activate_audio(cx);
        let enabled = !self.busy;
        let tokens = design_system::tokens();
        let project = self.presentation.project.clone();
        let mut header = div()
            .flex()
            .items_center()
            .justify_between()
            .p(px(tokens.metrics.space_4))
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(tokens.palette.surface))
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
                    "export-mp4",
                    "Export MP4…",
                    enabled
                        && project.is_some()
                        && self.presentation.agent_sdk.is_some()
                        && self.export_cancelled.is_none(),
                    cx,
                    |s, cx| s.pick(Picker::Export, cx),
                ))
                .child(self.button(
                    "close",
                    "Close project",
                    enabled && project.is_some(),
                    cx,
                    |s, cx| s.dispatch(Command::Close, cx),
                )),
        );
        let mut navigation = div()
            .w(px(tokens.metrics.sidebar_width))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(tokens.metrics.space_3))
            .p(px(tokens.metrics.space_4))
            .bg(rgb(tokens.palette.sidebar))
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
            .p(px(tokens.metrics.space_4))
            .gap(px(tokens.metrics.space_2));
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
            if let Some(status) = &self.export_status {
                let message = match status {
                    ExportProgress::Started { revision, label } => {
                        format!("Export snapshot · {label} · {revision}")
                    }
                    ExportProgress::Rendering { done, total } => {
                        format!("Rendering MP4 · {done}/{total} frames")
                    }
                    ExportProgress::Audio => "Writing and verifying audio".into(),
                    ExportProgress::Warning(message) => format!("Export warning · {message}"),
                    ExportProgress::Verifying => "Verifying MP4 before publication".into(),
                    ExportProgress::Complete {
                        destination,
                        revision,
                    } => format!("Exported {} · revision {}", destination.display(), revision),
                    ExportProgress::Failed(message) => format!("Export failed · {message}"),
                    ExportProgress::Cancelled => "Export cancelled · partial output removed".into(),
                };
                let mut status_row = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(PANEL))
                    .text_xs()
                    .child(message);
                if self.export_cancelled.is_some() {
                    status_row = status_row.child(self.button(
                        "cancel-export",
                        "Cancel export",
                        true,
                        cx,
                        |shell, cx| shell.cancel_export(cx),
                    ));
                }
                center = center.child(status_row);
            }
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
                if event.keystroke.modifiers.alt
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.shift
                {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "space" => shell.toggle_playback(cx),
                    "left" => shell.step_preview(-1, cx),
                    "right" => shell.step_preview(1, cx),
                    "home" => shell.seek_preview(0, cx),
                    "c" => shell.cycle_canvas_selection(cx),
                    "0" => shell.reset_canvas_view(cx),
                    "escape" => {
                        shell.canvas.clear_selection();
                        shell.install_task_scope(shell.timeline.read(cx).selected_scope(), cx);
                        cx.notify();
                    }
                    "end" => {
                        if let Some(d) = &shell.displayed {
                            shell.seek_preview(d.timeline.total_frames, cx);
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|shell, event: &gpui::MouseDownEvent, _, cx| {
                    shell.canvas_pointer_down(event, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|shell, event: &gpui::MouseDownEvent, _, cx| {
                    shell.canvas_pointer_down(event, cx)
                }),
            )
            .on_mouse_move(cx.listener(|shell, event: &gpui::MouseMoveEvent, _, cx| {
                shell.canvas_pointer_move(event, cx)
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|shell, _, _, cx| shell.canvas_pointer_up(MouseButton::Left, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|shell, _, _, cx| shell.canvas_pointer_up(MouseButton::Left, cx)),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|shell, _, _, cx| shell.canvas_pointer_up(MouseButton::Middle, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Middle,
                cx.listener(|shell, _, _, cx| shell.canvas_pointer_up(MouseButton::Middle, cx)),
            )
            .on_scroll_wheel(cx.listener(|shell, event: &gpui::ScrollWheelEvent, _, cx| {
                shell.canvas_scroll(event, cx)
            }))
            .flex_1()
            .min_h(px(120.))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded_lg()
            .bg(rgb(tokens.palette.canvas))
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
                        shell.canvas_origin =
                            (f64::from(bounds.origin.x), f64::from(bounds.origin.y));
                        shell.canvas_extent =
                            (f64::from(bounds.size.width), f64::from(bounds.size.height));
                        shell.preview_extent = extent;
                        if let Some(displayed) = &shell.displayed {
                            let before = shell.canvas.viewport.clone();
                            let _ = shell.canvas.resize(
                                f64::from(bounds.size.width),
                                f64::from(bounds.size.height),
                                displayed.timeline.width as u32,
                                displayed.timeline.height as u32,
                            );
                            if before != shell.canvas.viewport {
                                cx.notify();
                            }
                        }
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
            let image = gpui::img(image);
            preview_surface = if let Some(bounds) = self
                .canvas
                .viewport
                .as_ref()
                .map(studio_engine::CanvasViewport::image_bounds)
            {
                preview_surface.child(
                    image
                        .absolute()
                        .left(px(bounds.x as f32))
                        .top(px(bounds.y as f32))
                        .w(px(bounds.width as f32))
                        .h(px(bounds.height as f32)),
                )
            } else {
                preview_surface.child(image.size_full())
            };
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
        if let Some(bounds) = self.canvas.selected_bounds() {
            preview_surface = preview_surface.child(
                div()
                    .absolute()
                    .left(px(bounds.x as f32))
                    .top(px(bounds.y as f32))
                    .w(px(bounds.width as f32))
                    .h(px(bounds.height as f32))
                    .border_2()
                    .border_color(rgb(ACCENT)),
            );
        }
        if let (Some(viewport), Some(rect)) = (&self.canvas.viewport, &self.canvas.rectangle_scope)
        {
            let bounds = viewport.video_scope_bounds(rect);
            preview_surface = preview_surface.child(
                div()
                    .absolute()
                    .left(px(bounds.x as f32))
                    .top(px(bounds.y as f32))
                    .w(px(bounds.width as f32))
                    .h(px(bounds.height as f32))
                    .border_2()
                    .border_color(rgb(design_system::colors::WARNING_TEXT)),
            );
        }
        center = center.child(preview_surface);
        center = center.child(
            div()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("Select: click · Rectangle: Shift-drag · Cycle: Alt-click/C · Zoom: Ctrl-wheel · Pan: middle-drag · Fit: 0"),
        );
        if let Some(message) = &self.canvas.message {
            center = center.child(
                div()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(message.clone()),
            );
        }
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
        if self.navigation == "Style" {
            center = center.child(self.preset_panel.clone());
        }
        // SDK readiness and recent projects live under the navigation; the agent has its
        // own panel (and its own readiness) on the right.
        let mut library = div()
            .flex_shrink_0()
            .max_h(px(360.))
            .flex()
            .flex_col()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(rgb(BORDER))
            .child("Managed SDK")
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
                "import-sdk-bundle",
                "Import offline SDK bundle…",
                enabled,
                cx,
                |s, cx| s.pick(Picker::SdkBundle, cx),
            ))
            .child(self.button(
                "sdk",
                "Select installed SDK…",
                enabled && project.is_some(),
                cx,
                |s, cx| s.pick(Picker::Sdk, cx),
            ))
            .child(div().mt_2().child("Recent projects"));
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
        library = library.child(recents);
        navigation = navigation.child(library);
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
        if let Some(label) = &self.awaiting_preview {
            root = root.child(
                div()
                    .id("awaiting-preview")
                    .flex_shrink_0()
                    .p(px(tokens.metrics.space_3))
                    .bg(rgb(design_system::colors::WARNING))
                    .text_color(rgb(design_system::colors::WARNING_TEXT))
                    .text_sm()
                    .child(label.clone()),
            );
        }
        if let Some(error) = &self.error {
            root = root.child(
                div()
                    .id("error")
                    .max_h(px(120.))
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .p(px(tokens.metrics.space_3))
                    .bg(rgb(design_system::colors::DANGER))
                    .text_color(rgb(design_system::colors::DANGER_TEXT))
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
                .child(
                    div()
                        .w(px(tokens.metrics.agent_panel_width))
                        .flex_shrink_0()
                        .min_h_0()
                        .child(self.panel.clone()),
                ),
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
        let compatibility = CompatibilityManifest::default_linux_x64();
        studio_project::create(
            &root,
            "Video",
            sdk_pin(&compatibility),
            &compatibility.fframes_version,
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
            controller: Some(Arc::new(Mutex::new(controller))),
            compatibility: None,
            sdk_home: None,
            sdk: String::new(),
            agent_sdk: None,
            closed: Arc::new(AtomicBool::new(false)),
            processes: studio_bootstrap::ProcessTreeManager::new(),
            build_spec: None,
            catalog: Catalog::default(),
            preset_report: None,
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
