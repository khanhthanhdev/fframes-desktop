use crate::{project_view::ProjectPresentation, setup_view::SetupView};
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement,
    Styled, Window, div, px, rgb,
};
use parking_lot::Mutex;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
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
        })
    }
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
}
impl StudioShell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(false);
        window.focus(&focus, cx);
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
        }));
        let owner = backend.clone();
        let shutdown = closed.clone();
        cx.on_app_quit(move |_, cx| {
            shutdown.store(true, Ordering::Release);
            let owner = owner.clone();
            let processes = processes.clone();
            cx.background_executor().spawn(async move {
                processes.shutdown(Duration::ZERO);
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
        };
        shell.dispatch(Command::Initialize, cx);
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
    fn dispatch(&mut self, command: Command, cx: &mut Context<Self>) {
        if self.busy || self.closed.load(Ordering::Acquire) {
            return;
        }
        if let Command::Open(path) | Command::Locate(path, _) = &command {
            self.failed_open = Some(path.clone());
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
                return (Err("Window closed".into()), None);
            }
            let result = backend.command(command);
            let presentation = backend.presentation();
            match presentation {
                Ok(p) => (result, Some(p)),
                Err(e) => (Err(e), None),
            }
        });
        cx.spawn(async move |this, cx| {
            let (result, presentation) = task.await;
            let _ = this.update(cx, |shell, cx| {
                if shell.epoch != epoch || shell.closed.load(Ordering::Acquire) {
                    return;
                }
                shell.busy = false;
                if let Some(presentation) = presentation {
                    shell.presentation = presentation;
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
        div()
            .id(id)
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
    }
}

impl Render for StudioShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .gap_4();
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
                        "Worker bridge available · preview is not connected yet"
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
        center = center.child(
            div()
                .flex_1()
                .min_h(px(180.))
                .border_1()
                .border_color(rgb(BORDER))
                .rounded_lg()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(div().text_lg().child("No preview available"))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child("Preview and playback are not available yet."),
                ),
        );
        center = center.child(
            div()
                .flex()
                .gap_2()
                .flex_wrap()
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
            if let Some(draft) = p.draft.clone() {
                center = center.child(self.button(
                    "draft",
                    "Reveal retained draft",
                    true,
                    cx,
                    move |_, cx| cx.reveal_path(&draft),
                ));
            }
            let checkpoint = p.checkpoint.clone();
            center = center.child(self.button(
                "reveal-checkpoint",
                "Reveal checkpoint",
                true,
                cx,
                move |_, cx| cx.reveal_path(&checkpoint),
            ));
        }
        center = center.child(
            div()
                .h(px(125.))
                .flex_shrink_0()
                .border_t_1()
                .border_color(rgb(BORDER))
                .pt_4()
                .child("Timeline")
                .child(
                    div()
                        .pt_3()
                        .text_sm()
                        .text_color(rgb(MUTED))
                        .child("No compiled timeline available."),
                ),
        );
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
