use crate::design_system::colors::{
    ACCENT, BORDER, CANVAS, DANGER_TEXT, MUTED, PANEL, SUCCESS_TEXT, TEXT, WARNING_TEXT,
};
use crate::design_system::{self, ButtonStyle};
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div, rgb,
};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use studio_bootstrap::ProcessTreeManager;
use studio_sdk::{
    Doctor, ProbeStatus, environment::SdkEnvironment, install::SdkInstaller,
    manifest::CompatibilityManifest, project::ProjectManager,
};

fn default_sdk_home() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".fframes").join("sdk")
    } else if let Ok(profile) = std::env::var("USERPROFILE") {
        PathBuf::from(profile).join(".fframes").join("sdk")
    } else {
        std::env::temp_dir().join("fframes_sdk")
    }
}

fn default_projects_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".fframes").join("projects")
    } else if let Ok(profile) = std::env::var("USERPROFILE") {
        PathBuf::from(profile).join(".fframes").join("projects")
    } else {
        std::env::temp_dir().join("fframes_projects")
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupState {
    Checking,
    PrerequisitesNeeded {
        missing_packages: Vec<String>,
        summary: String,
    },
    SdkInstallNeeded {
        manifest_digest: String,
    },
    Installing {
        progress_percent: u8,
        step: String,
    },
    SdkReady {
        active_path: PathBuf,
        manifest_digest: String,
    },
    CreatingProject {
        name: String,
    },
    Building {
        name: String,
    },
    Rendered {
        frame_path: PathBuf,
        build_time: Duration,
        render_time: Duration,
    },
    Failed {
        error: String,
    },
}

pub struct SetupView {
    pub state: SetupState,
    pub manifest: CompatibilityManifest,
    pub sdk_home: PathBuf,
    pub projects_root: PathBuf,
    pub process_tree: ProcessTreeManager,
}

impl SetupView {
    /// Shared discovery for the spike and product shell; callers run this off the UI thread.
    pub fn defaults() -> Result<(CompatibilityManifest, PathBuf), String> {
        let packaged_manifest = std::env::var_os("FFRAMES_SDK_BUNDLE")
            .map(PathBuf::from)
            .map(|p| p.join("compatibility.json"))
            .or_else(|| {
                std::env::current_exe().ok().and_then(|p| {
                    p.parent()?
                        .parent()
                        .map(|p| p.join("sdk/compatibility.json"))
                })
            });
        let manifest_result = packaged_manifest.filter(|p| p.is_file()).map(|p| {
            std::fs::read_to_string(p)
                .map_err(|e| e.to_string())
                .and_then(|json| {
                    CompatibilityManifest::from_json_str(&json).map_err(|e| e.to_string())
                })
        });
        Ok((
            manifest_result
                .transpose()?
                .unwrap_or_else(CompatibilityManifest::default_linux_x64),
            default_sdk_home(),
        ))
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let defaults = Self::defaults();
        let (manifest, sdk_home) = defaults.as_ref().cloned().unwrap_or_else(|_| {
            (
                CompatibilityManifest::default_linux_x64(),
                default_sdk_home(),
            )
        });
        let mut view = Self {
            state: SetupState::Checking,
            manifest,
            sdk_home,
            projects_root: default_projects_root(),
            process_tree: ProcessTreeManager::new(),
        };
        if let Err(error) = defaults {
            view.state = SetupState::Failed { error };
        } else {
            view.run_preflight(cx);
        }
        view
    }

    pub fn run_preflight(&mut self, cx: &mut Context<Self>) {
        self.state = SetupState::Checking;
        let report = Doctor::run_host_preflight(&self.manifest);

        if report.overall_status == ProbeStatus::Fail {
            if !report.missing_system_packages.is_empty() {
                self.state = SetupState::PrerequisitesNeeded {
                    missing_packages: report.missing_system_packages.clone(),
                    summary: report.format_summary(),
                };
            } else {
                self.state = SetupState::Failed {
                    error: report.format_summary(),
                };
            }
            cx.notify();
            return;
        }

        let active_path = self.sdk_home.join("active");
        if active_path.exists() {
            let candidate_report = Doctor::verify_candidate_sdk_with_processes(
                &active_path,
                &self.manifest,
                Some(&self.process_tree),
            );
            if candidate_report.is_ready() {
                self.state = SetupState::SdkReady {
                    active_path,
                    manifest_digest: self.manifest.digest(),
                };
                cx.notify();
                return;
            }
        }

        self.state = SetupState::SdkInstallNeeded {
            manifest_digest: self.manifest.digest(),
        };
        cx.notify();
    }

    pub fn start_install_and_build(&mut self, cx: &mut Context<Self>) {
        self.state = SetupState::Installing {
            progress_percent: 20,
            step: "Checking / preparing SDK artifacts and dependencies...".to_string(),
        };
        cx.notify();

        let manifest = self.manifest.clone();
        let sdk_home = self.sdk_home.clone();
        let projects_root = self.projects_root.clone();
        let process_tree = self.process_tree.clone();

        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    Self::execute_install_and_build_pipeline(
                        manifest,
                        sdk_home,
                        projects_root,
                        process_tree,
                    )
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                match res {
                    Ok((frame_path, build_time, render_time)) => {
                        view.state = SetupState::Rendered {
                            frame_path,
                            build_time,
                            render_time,
                        };
                    }
                    Err(err) => {
                        view.state = SetupState::Failed { error: err };
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Install only the SDK. Product setup must not regenerate the spike fixture.
    pub fn install_sdk(
        manifest: &CompatibilityManifest,
        sdk_home: &std::path::Path,
        processes: &ProcessTreeManager,
    ) -> Result<PathBuf, String> {
        manifest
            .validate_for_current_app_version(env!("CARGO_PKG_VERSION"))
            .map_err(|error| format!("SDK compatibility rejected before setup: {error}"))?;
        let installer = SdkInstaller::new(sdk_home).with_process_manager(processes.clone());
        installer
            .recover_interrupted_install()
            .map_err(|error| format!("Interrupted SDK install recovery failed: {error}"))?;
        if let Some(active_sdk) = installer
            .reusable_active_for_app(manifest, env!("CARGO_PKG_VERSION"), Some(processes))
            .map_err(|error| format!("Installed SDK receipt check failed: {error}"))?
        {
            return Ok(active_sdk);
        }

        // 1. Resolve artifacts strictly from manifest across candidate directories
        let mut candidate_artifact_dirs = vec![
            PathBuf::from("desktop/packaging/sdk/artifacts"),
            PathBuf::from("packaging/sdk/artifacts"),
            PathBuf::from("target/phase-zero-sdk/artifacts"),
            PathBuf::from("desktop/target/phase-zero-sdk/artifacts"),
        ];
        if let Some(bundle) = std::env::var_os("FFRAMES_SDK_BUNDLE") {
            candidate_artifact_dirs.insert(0, PathBuf::from(bundle).join("artifacts"));
        }
        if let Ok(executable) = std::env::current_exe()
            && let Some(package) = executable.parent().and_then(|p| p.parent())
        {
            candidate_artifact_dirs.insert(0, package.join("sdk/artifacts"));
        }

        let mut artifact_files = Vec::new();
        for artifact in &manifest.artifacts {
            let filename = artifact
                .url
                .strip_prefix("file://artifacts/")
                .or_else(|| artifact.url.strip_prefix("file://"))
                .unwrap_or(&artifact.name);
            let relative = PathBuf::from(filename);
            if relative.as_os_str().is_empty()
                || relative
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err(format!(
                    "SDK artifact '{}' has an unsafe local bundle path",
                    artifact.name
                ));
            }
            let mut resolved = None;
            for dir in &candidate_artifact_dirs {
                let root = dir.canonicalize().ok();
                let candidate = dir.join(&relative).canonicalize().ok();
                if let (Some(root), Some(candidate)) = (root, candidate)
                    && candidate.starts_with(root)
                    && candidate.is_file()
                {
                    resolved = Some(candidate);
                    break;
                }
            }
            let file_path = resolved.ok_or_else(|| {
                format!(
                    "required artifact '{}' ({}) not found in candidate artifact directories",
                    artifact.name, filename
                )
            })?;
            artifact_files.push((artifact.clone(), file_path));
        }

        // 2. A non-receipted or incompatible active SDK is replaced only after the
        // local bundle has been resolved and the new candidate passes build/render probes.
        installer
            .install_from_local_artifacts_for_app(
                manifest,
                &artifact_files,
                env!("CARGO_PKG_VERSION"),
            )
            .map_err(|e| format!("SDK installation failed: {e}"))?;

        Ok(installer.active_sdk_dir())
    }

    /// Explicit import path for a user-selected offline SDK bundle. No network lookup
    /// occurs and a receipt-backed compatible active SDK can still be reused archive-free.
    pub fn install_sdk_from_bundle(
        bundle: &std::path::Path,
        sdk_home: &std::path::Path,
        processes: &ProcessTreeManager,
    ) -> Result<PathBuf, String> {
        let manifest_path = bundle.join("compatibility.json");
        let json = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("SDK bundle manifest {}: {error}", manifest_path.display()))?;
        let manifest = CompatibilityManifest::from_json_str(&json)
            .map_err(|error| format!("SDK bundle manifest is invalid: {error}"))?;
        manifest
            .validate_for_current_app_version(env!("CARGO_PKG_VERSION"))
            .map_err(|error| format!("SDK bundle is incompatible: {error}"))?;

        let installer = SdkInstaller::new(sdk_home).with_process_manager(processes.clone());
        installer
            .recover_interrupted_install()
            .map_err(|error| format!("Interrupted SDK install recovery failed: {error}"))?;
        if let Some(active_sdk) = installer
            .reusable_active_for_app(&manifest, env!("CARGO_PKG_VERSION"), Some(processes))
            .map_err(|error| format!("Installed SDK receipt check failed: {error}"))?
        {
            return Ok(active_sdk);
        }

        let bundle_root = bundle
            .canonicalize()
            .map_err(|error| format!("SDK bundle folder is unavailable: {error}"))?;
        let artifacts_dir = bundle_root
            .join("artifacts")
            .canonicalize()
            .map_err(|error| format!("SDK bundle artifacts folder is unavailable: {error}"))?;
        let mut artifacts = Vec::with_capacity(manifest.artifacts.len());
        for artifact in &manifest.artifacts {
            let filename = artifact
                .url
                .strip_prefix("file://artifacts/")
                .ok_or_else(|| {
                    format!(
                        "SDK bundle artifact '{}' has a non-local URL",
                        artifact.name
                    )
                })?;
            let relative = PathBuf::from(filename);
            if relative.as_os_str().is_empty()
                || relative
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err(format!(
                    "SDK bundle artifact '{}' has an unsafe path",
                    artifact.name
                ));
            }
            let path = artifacts_dir.join(relative);
            let canonical = path.canonicalize().map_err(|error| {
                format!(
                    "SDK bundle artifact '{}' is unavailable: {error}",
                    artifact.name
                )
            })?;
            if !canonical.starts_with(&artifacts_dir) || !canonical.is_file() {
                return Err(format!(
                    "SDK bundle artifact '{}' resolves outside its artifacts directory",
                    artifact.name,
                ));
            }
            artifacts.push((artifact.clone(), canonical));
        }
        installer
            .install_from_local_artifacts_for_app(&manifest, &artifacts, env!("CARGO_PKG_VERSION"))
            .map_err(|error| format!("SDK bundle installation failed: {error}"))?;
        Ok(installer.active_sdk_dir())
    }

    fn execute_install_and_build_pipeline(
        manifest: CompatibilityManifest,
        sdk_home: PathBuf,
        projects_root: PathBuf,
        process_tree: ProcessTreeManager,
    ) -> Result<(PathBuf, Duration, Duration), String> {
        let active_sdk = Self::install_sdk(&manifest, &sdk_home, &process_tree)?;
        // 3. Generate template project using the managed SDK's standalone framework
        fs::create_dir_all(&projects_root).map_err(|e| e.to_string())?;
        let project_dir = projects_root.join("phase-zero-template-video");
        if project_dir.exists() {
            let _ = fs::remove_dir_all(&project_dir);
        }

        ProjectManager::generate_cpu_project_with_sdk(
            "phase-zero-template-video",
            &project_dir,
            &manifest.fframes_version,
            Some(&active_sdk),
        )
        .map_err(|e| format!("Project generation failed: {e}"))?;

        // 4. Build project with isolated SDK environment in offline mode
        let sdk_env = SdkEnvironment::new(&active_sdk, &project_dir, manifest.clone(), true);
        let build_time = ProjectManager::build_project(&project_dir, &sdk_env, &process_tree)
            .map_err(|e| format!("Project build failed: {e}"))?;

        // 5. Render frame 0
        let frames_dir = project_dir.join("frames");
        let (render_time, _bytes) =
            ProjectManager::render_frame(&project_dir, &sdk_env, 0, &frames_dir, &process_tree)
                .map_err(|e| format!("Frame render failed: {e}"))?;

        let expected_frame = frames_dir.join("0.png");
        let final_frame = if expected_frame.exists() {
            expected_frame
        } else {
            frames_dir
        };

        Ok((final_frame, build_time, render_time))
    }

    fn action_button(
        &self,
        id: &'static str,
        label: &'static str,
        style: ButtonStyle,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let action = std::rc::Rc::new(action);
        let on_click = action.clone();
        design_system::button(
            div()
                .id(id)
                .relative()
                .role(gpui::Role::Button)
                .aria_label(label)
                .tab_index(0),
            style,
            true,
        )
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
        .on_key_down(
            cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    action(this, window, cx);
                    cx.stop_propagation();
                }
            }),
        )
        .child(label)
    }
}

impl Render for SetupView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match &self.state {
            SetupState::Checking => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(TEXT))
                        .child("Checking Host Prerequisites..."),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child("Running read-only preflight probes on compiler, CMake, and graphics libraries..."),
                ),

            SetupState::PrerequisitesNeeded {
                missing_packages,
                summary: _summary,
            } => {
                let pkg_cmd = format!("sudo apt-get install -y {}", missing_packages.join(" "));
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(WARNING_TEXT))
                            .child("System Prerequisites Needed"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(TEXT))
                            .child("The following host packages were detected as missing and require user review:"),
                    )
                    .child(
                        div()
                            .p_2()
                            .bg(rgb(CANVAS))
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(BORDER))
                            .text_xs()
                            .text_color(rgb(ACCENT))
                            .child(pkg_cmd),
                    )
                    .child(
                        self.action_button(
                            "btn-recheck",
                            "Re-run Preflight Check",
                            ButtonStyle::Primary,
                            cx,
                            |this, _, cx| this.run_preflight(cx),
                        ),
                    )
            }

            SetupState::Installing {
                progress_percent,
                step,
            } => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(TEXT))
                        .child(format!("Installing SDK... ({progress_percent}%)")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(step.clone()),
                ),

            SetupState::SdkInstallNeeded { manifest_digest } => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(ACCENT))
                        .child("Host Preflight Passed — SDK Installation Required"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(TEXT))
                        .child("Host environment verified. Ready to install app-managed toolchain & FFmpeg dependencies."),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(format!("Manifest: {:.16}...", manifest_digest)),
                )
                .child(
                    self.action_button(
                        "btn-install-sdk",
                        "Install Managed SDK & Build Template",
                        ButtonStyle::Primary,
                        cx,
                        |this, _, cx| this.start_install_and_build(cx),
                    ),
                ),

            SetupState::SdkReady {
                active_path,
                manifest_digest,
            } => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(SUCCESS_TEXT))
                        .child("Managed SDK Ready"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(TEXT))
                        .child(format!("Active location: {:?}", active_path)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(format!("Manifest Digest: {:.16}...", manifest_digest)),
                )
                .child(
                    self.action_button(
                        "btn-create-proj",
                        "Build and Render Template Project",
                        ButtonStyle::Primary,
                        cx,
                        |this, _, cx| this.start_install_and_build(cx),
                    ),
                ),
            SetupState::CreatingProject { name } => div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(TEXT))
                .child(format!("Generating project '{name}' outside workspace...")),

            SetupState::Building { name } => div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(TEXT))
                .child(format!("Building '{name}' with isolated SDK toolchain...")),

            SetupState::Rendered {
                frame_path,
                build_time,
                render_time,
            } => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(SUCCESS_TEXT))
                        .child("Project Rendered Successfully!"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(TEXT))
                        .child(format!("Build duration: {:?}", build_time)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(TEXT))
                        .child(format!("Frame render duration: {:?}", render_time)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(format!("Output PNG: {:?}", frame_path)),
                ),

            SetupState::Failed { error } => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(rgb(DANGER_TEXT))
                        .child("Setup / Build Failed"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(TEXT))
                        .child(error.clone()),
                )
                .child(
                    self.action_button(
                        "btn-retry",
                        "Retry",
                        ButtonStyle::Destructive,
                        cx,
                        |this, _, cx| this.run_preflight(cx),
                    ),
                ),
        };

        div()
            .flex()
            .flex_col()
            .p_4()
            .bg(rgb(PANEL))
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .child(content)
    }
}
