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
fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str).unwrap_or("spike-ui") {
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
                SdkInstaller::new(&sdk_home).install_from_local_artifacts(&manifest, &artifacts)?;
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
            println!("fframes-studio spike-ui");
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

fn run_spike_ui(qualification: Option<PresentationQualification>) {
    let evidence_output = qualification
        .as_ref()
        .map(|qualification| qualification.output.clone());
    gpui_platform::application().run(move |cx: &mut App| {
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
