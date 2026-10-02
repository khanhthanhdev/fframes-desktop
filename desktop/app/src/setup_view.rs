use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div, green, red, rgb, white,
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
        let installer = SdkInstaller::new(sdk_home).with_process_manager(processes.clone());
        let active_sdk = installer.active_sdk_dir();

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
            let mut resolved = None;
            for dir in &candidate_artifact_dirs {
                let candidate = dir.join(filename);
                if candidate.exists() {
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

        // 2. Validate existing SDK or install cleanly from local artifacts
        let need_install = if active_sdk.exists() {
            let candidate_report =
                Doctor::verify_candidate_sdk_with_processes(&active_sdk, manifest, Some(processes));
            !candidate_report.is_ready()
        } else {
            true
        };

        if need_install {
            installer
                .install_from_local_artifacts(manifest, &artifact_files)
                .map_err(|e| format!("SDK installation failed: {e}"))?;
        }

        Ok(active_sdk)
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
                        .text_color(white())
                        .child("Checking Host Prerequisites..."),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xAAAAAA))
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
                            .text_color(rgb(0xF59E0B))
                            .child("System Prerequisites Needed"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xCCCCCC))
                            .child("The following host packages were detected as missing and require user review:"),
                    )
                    .child(
                        div()
                            .p_2()
                            .bg(rgb(0x0A0A0A))
                            .rounded_sm()
                            .border_1()
                            .border_color(rgb(0x333333))
                            .text_xs()
                            .text_color(green())
                            .child(pkg_cmd),
                    )
                    .child(
                        div()
                            .id("btn-recheck")
                            .px_3()
                            .py_1()
                            .bg(rgb(0x2563EB))
                            .text_color(white())
                            .text_xs()
                            .rounded_sm()
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.run_preflight(cx);
                            }))
                            .child("Re-run Preflight Check"),
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
                        .text_color(white())
                        .child(format!("Installing SDK... ({progress_percent}%)")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xAAAAAA))
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
                        .text_color(rgb(0x3B82F6))
                        .child("Host Preflight Passed — SDK Installation Required"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xCCCCCC))
                        .child("Host environment verified. Ready to install app-managed toolchain & FFmpeg dependencies."),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child(format!("Manifest: {:.16}...", manifest_digest)),
                )
                .child(
                    div()
                        .id("btn-install-sdk")
                        .px_3()
                        .py_1()
                        .bg(rgb(0x2563EB))
                        .text_color(white())
                        .text_xs()
                        .rounded_sm()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.start_install_and_build(cx);
                        }))
                        .child("Install Managed SDK & Build Template"),
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
                        .text_color(green())
                        .child("Managed SDK Ready"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xCCCCCC))
                        .child(format!("Active location: {:?}", active_path)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x888888))
                        .child(format!("Manifest Digest: {:.16}...", manifest_digest)),
                )
                .child(
                    div()
                        .id("btn-create-proj")
                        .px_3()
                        .py_1()
                        .bg(rgb(0x059669))
                        .text_color(white())
                        .text_xs()
                        .rounded_sm()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.start_install_and_build(cx);
                        }))
                        .child("Build and Render Template Project"),
                ),
            SetupState::CreatingProject { name } => div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(white())
                .child(format!("Generating project '{name}' outside workspace...")),

            SetupState::Building { name } => div()
                .text_sm()
                .font_weight(FontWeight::BOLD)
                .text_color(white())
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
                        .text_color(green())
                        .child("Project Rendered Successfully!"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(white())
                        .child(format!("Build duration: {:?}", build_time)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(white())
                        .child(format!("Frame render duration: {:?}", render_time)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(0x888888))
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
                        .text_color(red())
                        .child("Setup / Build Failed"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(white())
                        .child(error.clone()),
                )
                .child(
                    div()
                        .id("btn-retry")
                        .px_3()
                        .py_1()
                        .bg(rgb(0xDC2626))
                        .text_color(white())
                        .text_xs()
                        .rounded_sm()
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _event, _window, cx| {
                            this.run_preflight(cx);
                        }))
                        .child("Retry"),
                ),
        };

        div()
            .flex()
            .flex_col()
            .p_4()
            .bg(rgb(0x181818))
            .rounded_md()
            .border_1()
            .border_color(rgb(0x2A2A2A))
            .child(content)
    }
}
