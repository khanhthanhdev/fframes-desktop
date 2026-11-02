//! The project backend the shell runs commands against: app storage, the open project's
//! controller, SDK compatibility and the preset catalog. Commands run off the UI thread
//! and leave a fresh [`Presentation`] for it.
use crate::presets::{Catalog, PresetCommand, PresetReport, PresetView};
use crate::project_presentation::ProjectPresentation;
use crate::services::{build_service::BuildService, preview_coordinator::BuildSpec};
use crate::{
    Controller,
    app_paths::AppPaths,
    build_materialization::{framework_for, sdk_pin},
    containment::OwnershipPolicy,
    store::Store,
};
use parking_lot::Mutex;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use studio_project::{ProjectId, manifest::CargoEntry};
use studio_proto::SessionId;
use studio_sdk::{CompatibilityManifest, Doctor};

#[derive(Clone)]
pub struct Recent {
    pub id: ProjectId,
    pub name: String,
    pub path: PathBuf,
    pub missing: bool,
    /// Encoded PNG of the last successful preview, read off the UI thread.
    pub preview: Option<Arc<[u8]>>,
}
#[derive(Clone, Default)]
pub struct Presentation {
    /// The session of the open project this presentation shows.
    pub session: Option<SessionId>,
    pub project: Option<ProjectPresentation>,
    pub recents: Vec<Recent>,
    pub sdk: String,
    pub state: Option<crate::ProjectState>,
    pub owner: Option<studio_bootstrap::ProcessTreeManager>,
    pub paths: Option<AppPaths>,
    /// The installed SDK agent tasks compile and preview against (set only while it
    /// verifies and supports the preview contract). Separate from agent readiness.
    pub agent_sdk: Option<AgentSdk>,
    /// See [`Backend::sdk_repin`].
    pub sdk_repin: Option<String>,
    /// Style-preset controls state of the open project (`None` when none is open).
    pub preset: Option<PresetView>,
    /// The open project's timing document, its clips and what can be edited now.
    pub timeline: Option<crate::timing::TimelineSnapshot>,
    /// The open project's props document and what can be edited now.
    pub props: Option<crate::props::PropsSnapshot>,
    /// The open project's slide stops, resolved against the timeline the person sees.
    pub slides: Option<crate::slides::SlidesSnapshot>,
}

#[derive(Clone)]
pub struct AgentSdk {
    pub dir: PathBuf,
    pub compatibility: CompatibilityManifest,
}

pub enum Command {
    Initialize,
    /// A new project in the folder, from the template.
    Create(PathBuf, studio_project::ProjectTemplate),
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
    /// Pins the open project to the active installed SDK (its own is not installed).
    UseInstalledSdk,
    InstallSdk,
    ImportSdkBundle(PathBuf),
    Restore(PathBuf, studio_project::SourceRevision),
    Independent(PathBuf),
    /// A style-preset operation, fenced by the source revision the UI last showed.
    Preset(PresetCommand, Option<studio_project::SourceRevision>),
    /// A timeline edit of the open project (split, trim, move, a drag, undo or redo).
    Timeline(crate::timing::TimelineCommand),
    /// A props edit of the open project (a field, a drag, undo or redo).
    Props(crate::props::PropsCommand),
    /// A slides edit of the open project (a stop, its text, a drag). Undo and redo are
    /// [`Command::Timeline`]'s: one history holds both.
    Slides(crate::slides::SlidesCommand),
}
pub struct Backend {
    pub paths: Option<AppPaths>,
    /// Shared with the agent workflow, which locks it only for short engine calls and for
    /// capture/publication on its own job threads.
    pub controller: Option<Arc<Mutex<Controller>>>,
    /// The open project's session; reopening the open folder keeps it.
    pub session: Option<SessionId>,
    /// Sessions opened so far (the next one is `sessions + 1`).
    pub sessions: u64,
    pub compatibility: Option<CompatibilityManifest>,
    /// Why no manifest resolved for this host; shown while `compatibility` is `None`.
    pub compatibility_error: String,
    pub sdk_home: Option<PathBuf>,
    pub sdk: String,
    pub agent_sdk: Option<AgentSdk>,
    /// The installed SDK the open project can be moved to with
    /// [`Command::UseInstalledSdk`]; set while the SDK it is pinned to is not installed.
    pub sdk_repin: Option<String>,
    /// The manifest of the SDK the open project builds with (the project's selection or
    /// the active installation), whether or not it verified.
    pub selected_sdk: Option<CompatibilityManifest>,
    pub closed: Arc<AtomicBool>,
    pub processes: studio_bootstrap::ProcessTreeManager,
    pub build_spec: Option<BuildSpec>,
    /// Bundled presets plus the verified packages imported into the app's store.
    pub catalog: Catalog,
    /// The outcome of the last preset command, taken by the dispatch that ran it.
    pub preset_report: Option<PresetReport>,
    pub environment: StudioEnvironment,
    /// The running app's version; an SDK must be compatible with it.
    pub app_version: &'static str,
}
impl Backend {
    pub fn command(&mut self, command: Command) -> Result<(), String> {
        if matches!(command, Command::Initialize) {
            self.paths = Some(
                match &self.environment.app_data {
                    Some(data) => AppPaths::new(data),
                    None => AppPaths::system(),
                }
                .map_err(|e| e.to_string())?,
            );
            self.catalog = Catalog::load(self.paths.as_ref().expect("just set"));
            self.refresh_compatibility();
            return Ok(());
        }
        let paths = self
            .paths
            .as_ref()
            .ok_or("App storage unavailable; restart after fixing the reported directory")?
            .clone();
        match command {
            Command::Initialize => (),
            Command::Create(path, template) => {
                let compatibility = self
                    .compatibility
                    .as_ref()
                    .ok_or_else(|| self.compatibility_error.clone())?;
                template_supported(template, compatibility.kit_version.as_deref())?;
                let name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("New video");
                studio_project::create_from_template(
                    &path,
                    name,
                    sdk_pin(compatibility),
                    &compatibility.fframes_version,
                    &compatibility.runtime_version,
                    template,
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
                let controller = self.current()?;
                let sdk = self
                    .project_sdk()
                    .ok_or("Select an installed SDK before building preview")?;
                let mut controller = controller.lock();
                let manifest_path = sdk.join("compatibility.json");
                let compatibility = CompatibilityManifest::from_json_str(
                    &std::fs::read_to_string(&manifest_path).map_err(|e| {
                        format!("{}: SDK manifest unavailable: {e}", manifest_path.display())
                    })?,
                )
                .map_err(|e| {
                    format!(
                        "{}: this SDK cannot be read ({e}); install the current SDK in Setup",
                        manifest_path.display()
                    )
                })?;
                compatibility
                    .validate_for_current_app_version(self.app_version)
                    .map_err(|e| format!("{e}; install the current SDK in Setup"))?;
                if !compatibility
                    .preview_contract_versions
                    .contains(&fframes_studio_protocol::PREVIEW_CONTRACT_VERSION)
                {
                    return Err("Selected SDK has no preview worker. Select an SDK that supports the preview contract; imported source is not rewritten.".into());
                }
                if controller.state().active_tag().is_some() {
                    controller.cancel().map_err(|e| e.to_string())?;
                }
                let tag = controller
                    .begin_job(crate::JobKind::Build)
                    .map_err(|e| e.to_string())?;
                self.build_spec = Some(BuildSpec {
                    project: controller.project.clone(),
                    sdk,
                    compatibility,
                    builds,
                    tag,
                    compiler: controller.operation_processes(),
                    worker: controller.processes.sub_manager(),
                    service: self.environment.builds.clone(),
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
                    self.environment.builds.close_project(&String::from(
                        controller.project.manifest.project_id.clone(),
                    ));
                    controller.close().map_err(|e| e.to_string())?;
                }
                self.controller = None;
                self.session = None;
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
            Command::UseInstalledSdk => {
                let home = self.sdk_home.as_deref().ok_or("SDK home unavailable")?;
                let (_, active) = crate::sdk_setup::installed_sdks(home)
                    .into_iter()
                    .next()
                    .ok_or("No SDK is installed; install the managed SDK first")?;
                self.current()?
                    .lock()
                    .repin_sdk(sdk_pin(&active))
                    .map_err(|e| e.to_string())?;
                self.check_sdk();
            }
            Command::InstallSdk => {
                let sdk = match crate::sdk_setup::install_sdk(
                    self.compatibility
                        .as_ref()
                        .ok_or_else(|| self.compatibility_error.clone())?,
                    self.sdk_home.as_deref().ok_or("SDK home unavailable")?,
                    &self.processes,
                    self.app_version,
                    &paths.builds(),
                ) {
                    Ok(sdk) => sdk,
                    Err(error) => {
                        self.sdk =
                            "Failed · repair the reported installation error and retry".into();
                        return Err(error);
                    }
                };
                // Without the warm cache the first preview compiles the framework itself.
                let _ = crate::services::worker_project::warm_build_cache(
                    &sdk,
                    &paths.builds(),
                    &self.processes,
                );
                self.refresh_compatibility();
            }
            Command::ImportSdkBundle(bundle) => {
                let sdk = match crate::sdk_setup::install_sdk_from_bundle(
                    &bundle,
                    self.sdk_home.as_deref().ok_or("SDK home unavailable")?,
                    &self.processes,
                    self.app_version,
                    &paths.builds(),
                ) {
                    Ok(sdk) => sdk,
                    Err(error) => {
                        self.sdk =
                            "Failed · repair the reported installation error and retry".into();
                        return Err(error);
                    }
                };
                let _ = crate::services::worker_project::warm_build_cache(
                    &sdk,
                    &paths.builds(),
                    &self.processes,
                );
                // An imported SDK may be the first manifest this host has.
                self.refresh_compatibility();
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
                let report = crate::presets::execute(
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
            Command::Timeline(command) => self
                .current()?
                .lock()
                .timeline_command(command)
                .map_err(|e| e.to_string())?,
            Command::Props(command) => self
                .current()?
                .lock()
                .props_command(command)
                .map_err(|e| e.to_string())?,
            Command::Slides(command) => self
                .current()?
                .lock()
                .slides_command(command)
                .map_err(|e| e.to_string())?,
        }
        Ok(())
    }
    pub fn current(&self) -> Result<Arc<Mutex<Controller>>, String> {
        self.controller
            .clone()
            .ok_or_else(|| "Open a project first".into())
    }
    pub fn open(&mut self, path: &Path, paths: &AppPaths) -> Result<(), String> {
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
            self.environment
                .builds
                .close_project(&String::from(previous.project.manifest.project_id.clone()));
            previous.close().map_err(|e| e.to_string())?;
        }
        self.controller = Some(Arc::new(Mutex::new(candidate)));
        self.sessions += 1;
        self.session = Some(SessionId(self.sessions));
        self.check_sdk();
        Ok(())
    }
    /// Resolves the SDK manifest for this host; without one the reason becomes the SDK status.
    pub fn refresh_compatibility(&mut self) {
        match crate::sdk_setup::defaults_in(self.environment.sdk_home()) {
            Ok((compatibility, sdk_home)) => {
                self.compatibility = Some(compatibility);
                self.sdk_home = Some(sdk_home);
            }
            Err(error) => {
                self.compatibility = None;
                self.sdk_home = Some(self.environment.sdk_home());
                self.compatibility_error = error;
            }
        }
        self.check_sdk();
    }
    /// The SDK the open project builds with: the one selected for it while that folder
    /// exists, else the installed SDK it is pinned to, else the active installation.
    fn project_sdk(&self) -> Option<PathBuf> {
        let home = self.sdk_home.as_deref();
        let for_project = self.controller.as_ref().and_then(|c| {
            let c = c.lock();
            c.sdk_path()
                .filter(|path| path.exists())
                .map(PathBuf::from)
                .or_else(|| {
                    home.and_then(|h| crate::sdk_setup::installed_sdk_for(&c.project.manifest, h))
                })
        });
        for_project
            .or_else(|| home.map(|h| h.join("active")))
            .filter(|path| path.exists())
    }
    pub fn check_sdk(&mut self) {
        self.agent_sdk = None;
        self.selected_sdk = None;
        self.sdk_repin = None;
        let sdk = self.project_sdk();
        // Host prerequisites come from the SDK that is actually selected, which may be the
        // only manifest this host has; the app default stands in while none is installed.
        let selected = sdk.as_ref().map(|path| {
            std::fs::read_to_string(path.join("compatibility.json"))
                .map_err(|e| e.to_string())
                .and_then(|s| CompatibilityManifest::from_json_str(&s).map_err(|e| e.to_string()))
        });
        let preflight = match &selected {
            Some(Ok(manifest)) => manifest,
            Some(Err(_)) | None => match &self.compatibility {
                Some(compatibility) => compatibility,
                None => {
                    self.sdk = self.compatibility_error.clone();
                    return;
                }
            },
        };
        let report = Doctor::run_host_preflight(preflight);
        if !report.is_ready() {
            self.sdk = format!("Host prerequisites needed: {}", report.format_summary());
            return;
        }
        self.sdk = match (sdk, selected) {
            (Some(path), Some(selected)) => {
                let selected = match selected {
                    // An SDK installed by an older app stays active after an update; its
                    // app range is checked here because only the app knows its version.
                    Ok(manifest) => {
                        if let Err(error) =
                            manifest.validate_for_current_app_version(self.app_version)
                        {
                            self.sdk = format!("SDK failed: {error} · Install managed SDK");
                            return;
                        }
                        self.selected_sdk = Some(manifest.clone());
                        manifest
                    }
                    Err(error) => {
                        self.sdk = format!("SDK failed: {error}; select a complete installed SDK");
                        return;
                    }
                };
                let pin = self
                    .controller
                    .as_ref()
                    .map(|c| c.lock().project.manifest.clone())
                    .filter(|project| framework_for(project, &selected).is_none())
                    .map(|project| project.sdk);
                if let Some(pin) = pin {
                    // No installed SDK matches the project's pin (`project_sdk` looked).
                    self.sdk = if self
                        .compatibility
                        .as_ref()
                        .is_some_and(|available| sdk_pin(available) == pin)
                    {
                        format!(
                            "This project uses SDK {}, which is not installed yet · Install managed SDK",
                            pin.release
                        )
                    } else {
                        self.sdk_repin = Some(selected.sdk_id.clone());
                        format!(
                            "This project is pinned to SDK {}, which is not installed · Use installed SDK {} (rewrites studio.json's SDK pin) or import the project's SDK bundle",
                            pin.release, selected.sdk_id
                        )
                    };
                    return;
                }
                let report = Doctor::verify_candidate_sdk_with_processes(
                    &path,
                    &selected,
                    Some(&self.processes),
                );
                if report.is_ready() {
                    // Agent tasks compile and preview candidates, which needs the
                    // preview contract; an SDK without it stays usable for the rest.
                    if selected
                        .preview_contract_versions
                        .contains(&fframes_studio_protocol::PREVIEW_CONTRACT_VERSION)
                    {
                        self.agent_sdk = Some(AgentSdk {
                            dir: path.clone(),
                            compatibility: selected.clone(),
                        });
                    }
                    // The app ships a different SDK than the installed one (an app update
                    // brought a newer bundle): installing it keeps this one as `previous`,
                    // so projects pinned to it keep building.
                    let active = self.sdk_home.as_ref().map(|home| home.join("active"));
                    match &self.compatibility {
                        Some(available)
                            if active.as_ref() == Some(&path)
                                && available.digest() != selected.digest() =>
                        {
                            format!(
                                "Available · {} · SDK update {} available · Install managed SDK",
                                path.display(),
                                available.sdk_id
                            )
                        }
                        _ => format!("Available · {}", path.display()),
                    }
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
    pub fn presentation(&self) -> Result<Presentation, String> {
        let recents = match &self.paths {
            Some(paths) => Store::open(&paths.database())
                .and_then(|s| s.recents())
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|r| Recent {
                    missing: !r.location.is_dir(),
                    preview: std::fs::read(paths.project_preview(r.state.project()))
                        .ok()
                        .map(Arc::from),
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
            let project = ProjectPresentation::from_controller(&controller);
            let timeline = controller.timeline_snapshot();
            let props = controller.props_snapshot();
            let slides = controller.slides_snapshot();
            (
                project,
                controller.state().clone(),
                controller.processes.clone(),
                PresetView::capture(&mut controller, &self.catalog),
                timeline,
                props,
                slides,
            )
        });
        Ok(Presentation {
            session: self.session,
            project: open.as_ref().map(|(project, ..)| project.clone()),
            recents,
            sdk: self.sdk.clone(),
            state: open.as_ref().map(|(_, state, ..)| state.clone()),
            preset: open.as_ref().map(|(_, _, _, preset, ..)| preset.clone()),
            timeline: open.as_ref().map(|(.., timeline, _, _)| timeline.clone()),
            props: open.as_ref().map(|(.., props, _)| props.clone()),
            slides: open.as_ref().map(|(.., slides)| slides.clone()),
            owner: open.map(|(_, _, owner, ..)| owner),
            paths: self.paths.clone(),
            agent_sdk: self.agent_sdk.clone(),
            sdk_repin: self.sdk_repin.clone(),
        })
    }
}

/// Where the shell keeps its data, what compiles its workers and whom it trusts with
/// writes. [`StudioEnvironment::system`] is the installed app; a host that runs the shell
/// elsewhere (a harness, a sandbox) passes its own.
#[derive(Clone)]
pub struct StudioEnvironment {
    /// App data folder; `None` takes the platform's.
    pub app_data: Option<PathBuf>,
    /// Folder of installed SDKs (`active` is the default selection); `None` takes
    /// `~/.fframes/sdk`.
    pub sdk_home: Option<PathBuf>,
    /// Compiles preview, agent and export workers; equal build keys compile once.
    pub builds: BuildService,
    /// Who may say an agent adapter's writers are contained.
    pub ownership: OwnershipPolicy,
}

impl StudioEnvironment {
    pub fn system() -> Self {
        Self {
            app_data: None,
            sdk_home: None,
            builds: crate::services::worker_project::shared_build_service(),
            ownership: OwnershipPolicy::Validated,
        }
    }

    pub fn sdk_home(&self) -> PathBuf {
        self.sdk_home
            .clone()
            .unwrap_or_else(crate::sdk_setup::default_sdk_home)
    }
}

/// Whether the SDK can build `template`: a template on fframes-kit needs the SDK to
/// vendor exactly its kit release (`kit_version`).
fn template_supported(
    template: studio_project::ProjectTemplate,
    sdk_kit: Option<&str>,
) -> Result<(), String> {
    match template.kit_version() {
        Some(kit) if sdk_kit != Some(kit) => Err(format!(
            "The Slide deck template needs an SDK with fframes-kit {kit}; {}. Install the current managed SDK, or create a Blank project",
            match sdk_kit {
                Some(sdk) => format!("this SDK includes fframes-kit {sdk}"),
                None => "this SDK has no fframes-kit".to_owned(),
            }
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_slide_deck_template_needs_the_sdk_kit() {
        use studio_project::ProjectTemplate::{Blank, SlideDeck};
        assert!(template_supported(Blank, None).is_ok());
        assert!(template_supported(SlideDeck, Some("0.2.1")).is_ok());
        assert!(template_supported(SlideDeck, Some("0.2.0")).is_err());
        assert!(
            template_supported(SlideDeck, None)
                .unwrap_err()
                .contains("no fframes-kit")
        );
        assert!(
            template_supported(SlideDeck, Some("0.1.0"))
                .unwrap_err()
                .contains("includes fframes-kit 0.1.0")
        );
    }

    #[test]
    fn use_installed_sdk_repins_a_project_whose_sdk_is_not_installed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("video");
        let paths = AppPaths::new(temp.path().join("history")).unwrap();
        let mut old = CompatibilityManifest::default_linux_x64();
        old.sdk_id = "studio-sdk-old".into();
        studio_project::create(
            &root,
            "Video",
            sdk_pin(&old),
            &old.fframes_version,
            &old.runtime_version,
        )
        .unwrap();
        let sdk_home = temp.path().join("sdk");
        let mut installed = CompatibilityManifest::default_linux_x64();
        installed.target_triple = CompatibilityManifest::host_target().unwrap().into();
        fs::create_dir_all(sdk_home.join("active")).unwrap();
        fs::write(
            sdk_home.join("active/compatibility.json"),
            serde_json::to_string(&installed).unwrap(),
        )
        .unwrap();
        let controller = Controller::open(&root, &paths).unwrap();
        let mut backend = Backend {
            paths: Some(paths),
            controller: Some(Arc::new(Mutex::new(controller))),
            session: Some(SessionId(1)),
            sessions: 1,
            compatibility: None,
            compatibility_error: "SDK manifest unavailable".into(),
            sdk_home: Some(sdk_home),
            sdk: String::new(),
            agent_sdk: None,
            sdk_repin: None,
            selected_sdk: None,
            closed: Arc::new(AtomicBool::new(false)),
            processes: studio_bootstrap::ProcessTreeManager::new(),
            build_spec: None,
            catalog: Catalog::default(),
            preset_report: None,
            environment: StudioEnvironment::system(),
            app_version: "0.0.0",
        };
        let source = fs::read(root.join("src/lib.rs")).unwrap();

        backend.command(Command::UseInstalledSdk).unwrap();

        let reopened = studio_project::open(&root).unwrap().manifest;
        assert_eq!(reopened.sdk, sdk_pin(&installed));
        assert_eq!(fs::read(root.join("src/lib.rs")).unwrap(), source);
        assert_eq!(
            crate::sdk_setup::installed_sdk_for(&reopened, backend.sdk_home.as_ref().unwrap()),
            backend.sdk_home.as_ref().map(|home| home.join("active"))
        );
    }

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
            &compatibility.runtime_version,
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
            session: Some(SessionId(1)),
            sessions: 1,
            compatibility: None,
            compatibility_error: "SDK manifest unavailable".into(),
            sdk_home: None,
            sdk: String::new(),
            agent_sdk: None,
            sdk_repin: None,
            selected_sdk: None,
            closed: Arc::new(AtomicBool::new(false)),
            processes: studio_bootstrap::ProcessTreeManager::new(),
            build_spec: None,
            catalog: Catalog::default(),
            preset_report: None,
            environment: StudioEnvironment::system(),
            app_version: "0.0.0",
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
