// A GUI-subsystem executable, so Start menu and Explorer launches open no console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

use fframes_studio::StudioSpikeApp;
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use studio_sdk::{CompatibilityManifest, SdkInstaller};

struct PresentationQualification {
    sdk_home: PathBuf,
    project: PathBuf,
    output: PathBuf,
    manifest: CompatibilityManifest,
}
/// Reattaches the terminal a command was typed in, so subcommand output and errors stay
/// visible. Redirected handles (files, pipes) are already valid and are left alone.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE,
    };
    // SAFETY: plain Win32 calls without pointers; failure only means no parent console.
    unsafe {
        if GetStdHandle(STD_ERROR_HANDLE).is_null() {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

fn main() {
    #[cfg(windows)]
    attach_parent_console();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str).unwrap_or("studio") {
        "studio" => run_studio(None),
        "qualify-m3" => {
            let argument = |key: &str| {
                args.iter()
                    .position(|arg| arg == key)
                    .and_then(|index| args.get(index + 1))
                    .map(PathBuf::from)
            };
            // Test-only injection for fixtures (never reachable from the product entry or
            // the settings file): treats the adapter's writers as process-group
            // contained WITHOUT qualification evidence, and the panel says so.
            let test_ownership = argument("--test-writer-containment").map(|label| {
                studio_bootstrap::WriterOwnership::ProcessGroupContained {
                    qualification: format!("test-injection:{}", label.to_string_lossy()),
                }
            });
            match (argument("--project"), argument("--telemetry")) {
                (Some(project), Some(output)) if project.is_dir() && !output.exists() => {
                    run_studio(Some(Qualification::AgentPanel {
                        project,
                        output,
                        test_ownership,
                    }))
                }
                _ => {
                    eprintln!("qualify-m3 requires --project DIR and a fresh --telemetry FILE");
                    std::process::exit(1);
                }
            }
        }
        "qualify-m2" => {
            let argument = |key: &str| {
                args.iter()
                    .position(|arg| arg == key)
                    .and_then(|index| args.get(index + 1))
                    .map(PathBuf::from)
            };
            match (argument("--project"), argument("--telemetry")) {
                (Some(project), Some(output)) if project.is_dir() && !output.exists() => {
                    run_studio(Some(Qualification::Playback { project, output }))
                }
                _ => {
                    eprintln!("qualify-m2 requires --project DIR and a fresh --telemetry FILE");
                    std::process::exit(1);
                }
            }
        }
        "spike-ui" | "--spike-ui" => run_spike_ui(None),
        "qualify-presentation" => {
            let result = (|| -> Result<_, Box<dyn std::error::Error>> {
                let argument = |key: &str| -> Result<PathBuf, String> {
                    args.iter()
                        .position(|arg| arg == key)
                        .and_then(|index| args.get(index + 1))
                        .map(PathBuf::from)
                        .ok_or_else(|| format!("Missing {key}"))
                };
                let bundle = argument("--bundle")?.canonicalize()?;
                let current = std::env::current_dir()?;
                let sdk_home = current.join(argument("--sdk-home")?);
                let project = current.join(argument("--project")?);
                let output = current.join(argument("--output")?);
                if output.exists() {
                    return Err("Evidence output already exists".into());
                }
                let manifest = CompatibilityManifest::from_json_str(&std::fs::read_to_string(
                    bundle.join("compatibility.json"),
                )?)?;
                let artifacts: Vec<_> = manifest
                    .artifacts
                    .iter()
                    .map(|artifact| {
                        (
                            artifact.clone(),
                            bundle.join(artifact.url.trim_start_matches("file://")),
                        )
                    })
                    .collect();
                SdkInstaller::new(&sdk_home).install_from_local_artifacts_for_app(
                    &manifest,
                    &artifacts,
                    env!("CARGO_PKG_VERSION"),
                )?;
                Ok(PresentationQualification {
                    sdk_home,
                    project,
                    output,
                    manifest,
                })
            })();
            match result {
                Ok(qualification) => run_spike_ui(Some(qualification)),
                Err(error) => {
                    eprintln!("Qualification failed: {error}");
                    std::process::exit(1);
                }
            }
        }
        "help" | "--help" | "-h" => {
            println!("fframes-studio [studio] — native project workspace");
            println!("fframes-studio spike-ui");
            println!("fframes-studio qualify-m2 --project DIR --telemetry FILE");
            println!(
                "fframes-studio qualify-m3 --project DIR --telemetry FILE [--test-writer-containment LABEL]"
            );
            println!(
                "fframes-studio qualify-presentation --bundle DIR --sdk-home DIR --project DIR --output FILE"
            );
            println!("Use studio_setup for GPUI-free doctor and project scaffolding.");
        }
        command => {
            eprintln!("Unknown command: {command}");
            std::process::exit(1);
        }
    }
}

/// Observation entries of the production shell (telemetry files for the external
/// harnesses; they never change behavior).
enum Qualification {
    /// M2: build the preview and record playback telemetry.
    Playback { project: PathBuf, output: PathBuf },
    /// M3: open the project and record the agent panel's redacted state.
    AgentPanel {
        project: PathBuf,
        output: PathBuf,
        test_ownership: Option<studio_bootstrap::WriterOwnership>,
    },
}

fn run_studio(qualification: Option<Qualification>) {
    gpui_platform::application().run(move |cx: &mut App| {
        fframes_studio::text_input::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("fframes Studio".into()),
                        appears_transparent: false,
                        traffic_light_position: None,
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    cx.new(|cx| {
                        let mut shell = fframes_studio::studio_shell::StudioShell::new(window, cx);
                        match qualification {
                            Some(Qualification::Playback { project, output }) => {
                                shell.start_qualification(project, output, cx)
                            }
                            Some(Qualification::AgentPanel {
                                project,
                                output,
                                test_ownership,
                            }) => shell.start_agent_telemetry(project, output, test_ownership, cx),
                            None => (),
                        }
                        shell
                    })
                },
            )
            .expect("failed to open Studio");
        // Closing the last window must not release the shell before its awaited
        // quit hook can stop audio and drop the final materialization consumers.
        let shell = window.entity(cx).expect("Studio root entity");
        cx.on_window_closed(move |cx, _| {
            let _keep_alive = &shell;
            cx.quit();
        })
        .detach();
        cx.activate(true);
    });
}

fn run_spike_ui(qualification: Option<PresentationQualification>) {
    let evidence_output = qualification
        .as_ref()
        .map(|qualification| qualification.output.clone());
    gpui_platform::application().run(move |cx: &mut App| {
        fframes_studio::text_input::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(1000.), px(1000.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("fframes studio — Native GPUI Spike".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            move |window, cx| {
                cx.new(|cx| {
                    let mut app = StudioSpikeApp::new(cx);
                    app.load_initial_frame(window, cx);
                    if let Some(qualification) = qualification {
                        app.setup_view.update(cx, |view, _| {
                            view.sdk_home = qualification.sdk_home;
                            view.manifest = qualification.manifest;
                        });
                        app.worker_project_root = Some(qualification.project);
                        app.qualification_output = Some(qualification.output);
                        app.launch_real_worker(cx);
                        cx.spawn(async move |this, cx| {
                            let deadline = Instant::now() + Duration::from_secs(120);
                            loop {
                                cx.background_executor()
                                    .timer(Duration::from_millis(100))
                                    .await;
                                let waiting = this
                                    .update(cx, |app, cx| {
                                        if !app.worker_busy {
                                            if app.worker_client.is_some() {
                                                app.start_stress_test(cx);
                                            } else {
                                                app.abort_stress(
                                                    format!(
                                                        "Worker startup failed: {}",
                                                        app.worker_status
                                                    ),
                                                    cx,
                                                );
                                            }
                                            return false;
                                        }
                                        if Instant::now() >= deadline {
                                            app.abort_stress(
                                                "Worker startup exceeded deadline".into(),
                                                cx,
                                            );
                                            return false;
                                        }
                                        true
                                    })
                                    .unwrap_or(false);
                                if !waiting {
                                    break;
                                }
                            }
                        })
                        .detach();
                    }
                    app
                })
            },
        )
        .expect("failed to open window");
        cx.activate(true);
    });
    if let Some(output) = evidence_output {
        let verified = std::fs::read(&output)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|record| {
                record["completed"] == true
                    && record["confirmed_presentations"] == 1000
                    && record["paint_submissions"] == 1000
                    && record["verified_render_requests"] == 2000
                    && record["failure"].is_null()
            });
        if !verified {
            let details = std::fs::read_to_string(&output).unwrap_or_else(|e| {
                format!(
                    "Could not read evidence output from {}: {e}",
                    output.display()
                )
            });
            eprintln!("Native presentation qualification did not complete: {details}");
            std::process::exit(1);
        }
    }
}
