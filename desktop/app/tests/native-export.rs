use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use studio_engine::{Controller, JobKind, JobResult, app_paths::AppPaths};
use studio_sdk::{CompatibilityManifest, SdkInstaller};

fn create_test_project(root: &std::path::Path) {
    studio_project::create(
        root,
        "Export source",
        studio_engine::build_materialization::sdk_pin(&CompatibilityManifest::default_linux_x64()),
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
}

fn install_sdk(temp: &tempfile::TempDir) -> (PathBuf, CompatibilityManifest) {
    let bundle = PathBuf::from(std::env::var_os("SDK_BUNDLE").expect("SDK_BUNDLE"));
    let manifest = CompatibilityManifest::from_json_str(
        &fs::read_to_string(bundle.join("compatibility.json")).unwrap(),
    )
    .unwrap();
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
    let sdk = SdkInstaller::new(temp.path().join("sdk"))
        .install_from_local_artifacts(&manifest, &artifacts)
        .unwrap();
    (sdk, manifest)
}

fn audio_wave() -> Vec<u8> {
    const RATE: u32 = 48_000;
    const FRAMES: u32 = RATE;
    let data_bytes = FRAMES * 2;
    let mut wave = Vec::with_capacity(data_bytes as usize + 44);
    wave.extend(b"RIFF");
    wave.extend((36 + data_bytes).to_le_bytes());
    wave.extend(b"WAVEfmt ");
    wave.extend(16_u32.to_le_bytes());
    wave.extend(1_u16.to_le_bytes());
    wave.extend(1_u16.to_le_bytes());
    wave.extend(RATE.to_le_bytes());
    wave.extend((RATE * 2).to_le_bytes());
    wave.extend(2_u16.to_le_bytes());
    wave.extend(16_u16.to_le_bytes());
    wave.extend(b"data");
    wave.extend(data_bytes.to_le_bytes());
    for frame in 0..FRAMES {
        let sample = ((f64::from(frame) * 440. * std::f64::consts::TAU / f64::from(RATE)).sin()
            * 8000.) as i16;
        wave.extend(sample.to_le_bytes());
    }
    wave
}

fn run_cli(
    project: &Path,
    environment: &fframes_studio::build_service::CompileEnvironment,
    args: &[&str],
) -> Output {
    let mut command = Command::new("cargo");
    command
        .current_dir(project)
        .args(["run", "--locked", "--offline", "--quiet", "--manifest-path"])
        .arg(project.join("Cargo.toml"))
        .arg("--")
        .args(args);
    environment
        .sdk_environment()
        .build_child_environment()
        .apply_to_command(&mut command);
    command.output().unwrap()
}

#[test]
fn saved_revision_must_have_a_successful_preview_build_before_freezing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    create_test_project(&root);
    let paths = AppPaths::new(temp.path().join("app")).unwrap();
    let controller = Controller::open(&root, &paths).unwrap();

    let error = controller
        .freeze_export_source()
        .err()
        .expect("unbuilt project must be refused")
        .to_string();
    assert!(error.contains("build the current saved revision"));
}

#[test]
fn export_source_is_an_immutable_checkpoint_copy_not_the_live_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    create_test_project(&root);
    let paths = AppPaths::new(temp.path().join("app")).unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let source = controller.state().source().clone();
    let build = controller.begin_job(JobKind::Build).unwrap();
    controller
        .complete(&build, JobResult::Built(source.clone()))
        .unwrap();

    let frozen = controller.freeze_export_source().unwrap();
    assert_eq!(frozen.revision(), &source);
    assert_eq!(frozen.project().inventory.revision, source);
    assert_eq!(frozen.label(), "Saved project · preview build verified");

    fs::write(
        root.join("src/lib.rs"),
        "// mutable checkout changed after capture",
    )
    .unwrap();
    assert_ne!(
        fs::read(root.join("src/lib.rs")).unwrap(),
        fs::read(frozen.root().join("src/lib.rs")).unwrap()
    );
    assert_eq!(frozen.project().inventory.revision, *frozen.revision());
}

#[test]
fn export_refuses_a_newer_unvalidated_live_source() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    create_test_project(&root);
    let paths = AppPaths::new(temp.path().join("app")).unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let source = controller.state().source().clone();
    let build = controller.begin_job(JobKind::Build).unwrap();
    controller
        .complete(&build, JobResult::Built(source))
        .unwrap();
    fs::write(root.join("src/lib.rs"), "// a newer unvalidated source").unwrap();

    let error = controller
        .freeze_export_source()
        .err()
        .expect("changed live source must be refused")
        .to_string();
    assert!(error.contains("differs from the selected export revision"));
}

#[test]
#[ignore = "requires assembled Linux SDK_BUNDLE and compiles/renders the actual generated worker"]
fn linux_export_renders_the_frozen_saved_revision_to_decodable_video_and_audio() {
    use fframes_studio::{
        build_service::{BuildLimits, BuildService, Subscriber, SubscriberKind},
        export_service::{ExportControl, ExportProgress, ExportRequest, export_mp4},
        worker_project::{CargoCompiler, compile_portable_worker_via},
    };
    use studio_bootstrap::ProcessTreeManager;
    use studio_engine::build_materialization::sdk_pin;

    let temporary = tempfile::tempdir().unwrap();
    let (sdk, manifest) = install_sdk(&temporary);
    let root = temporary.path().join("project");
    studio_project::create(
        &root,
        "Native export qualification",
        sdk_pin(&manifest),
        &manifest.fframes_version,
        "0.1.0",
    )
    .unwrap();
    let source_path = root.join("src/lib.rs");
    let template = fs::read_to_string(&source_path).unwrap();
    let audio_map = r#"fframes::AudioMap::from([(
            "tone.wav",
            fframes::AudioTimestamp::Second(0.)..fframes::AudioTimestamp::Eof,
        )])"#;
    assert!(template.contains("fframes::AudioMap::none()"));
    fs::write(
        &source_path,
        template.replace("fframes::AudioMap::none()", audio_map),
    )
    .unwrap();
    fs::write(root.join("media/tone.wav"), audio_wave()).unwrap();

    let paths = AppPaths::new(temporary.path().join("app-data")).unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let project = controller.project.clone();
    let service = BuildService::new(
        ProcessTreeManager::new(),
        Arc::new(CargoCompiler),
        BuildLimits::default(),
    );
    let processes = ProcessTreeManager::new();
    let builds = paths.builds();
    let preview_build = compile_portable_worker_via(
        &service,
        Subscriber::new(SubscriberKind::Validation, "native export baseline"),
        &project,
        &sdk,
        manifest.clone(),
        &builds,
        &processes,
    )
    .unwrap();
    assert!(preview_build.manifest.is_file());
    let revision = controller.state().source().clone();
    let build = controller.begin_job(JobKind::Build).unwrap();
    controller
        .complete(&build, JobResult::Built(revision.clone()))
        .unwrap();

    let cancelled_destination = temporary.path().join("cancelled.mp4");
    let cancelled_source = controller.freeze_export_source().unwrap();
    let cancelled_control = Arc::new(ExportControl::default());
    let callback_control = cancelled_control.clone();
    let cancelled = export_mp4(
        ExportRequest {
            source: cancelled_source,
            sdk: sdk.clone(),
            builds: builds.clone(),
            destination: cancelled_destination.clone(),
        },
        service.clone(),
        processes.clone(),
        cancelled_control,
        |event| {
            if matches!(event, ExportProgress::Rendering { .. }) {
                callback_control.request_cancel();
            }
        },
    );
    assert!(cancelled.unwrap_err().contains("cancelled"));
    assert!(!cancelled_destination.exists());
    assert!(
        fs::read_dir(temporary.path()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".fframes-export-")),
        "cancelled export must remove its owned temporary file"
    );

    let raced_destination = temporary.path().join("raced.mp4");
    let raced_source = controller.freeze_export_source().unwrap();
    let callback_destination = raced_destination.clone();
    let mut created_destination = false;
    let raced = export_mp4(
        ExportRequest {
            source: raced_source,
            sdk: sdk.clone(),
            builds: builds.clone(),
            destination: raced_destination.clone(),
        },
        service.clone(),
        processes.clone(),
        Arc::new(ExportControl::default()),
        |event| {
            if !created_destination && matches!(event, ExportProgress::Rendering { .. }) {
                fs::write(&callback_destination, b"destination created during export").unwrap();
                created_destination = true;
            }
        },
    );
    assert!(
        created_destination,
        "renderer must have started before the race"
    );
    assert!(
        raced
            .unwrap_err()
            .contains("destination appeared during export")
    );
    assert_eq!(
        fs::read(&raced_destination).unwrap(),
        b"destination created during export",
        "no-clobber publication must preserve bytes created during rendering"
    );
    fs::remove_file(&raced_destination).unwrap();

    let frozen = controller.freeze_export_source().unwrap();
    assert_eq!(frozen.revision(), &revision);

    // Later live edits cannot be substituted into the already captured export job.
    fs::write(
        &source_path,
        format!("{template}\n// external edit after capture\n"),
    )
    .unwrap();
    let destination = temporary.path().join("video.mp4");
    let control = Arc::new(ExportControl::default());
    let mut progress = Vec::new();
    export_mp4(
        ExportRequest {
            source: frozen,
            sdk: sdk.clone(),
            builds: builds.clone(),
            destination: destination.clone(),
        },
        service,
        processes,
        control,
        |event| progress.push(event),
    )
    .unwrap();
    assert!(destination.is_file());
    assert!(
        progress
            .iter()
            .any(|event| matches!(event, ExportProgress::Rendering { .. }))
    );
    assert!(progress.iter().any(|event| matches!(event, ExportProgress::Complete { revision: exported, .. } if exported == revision.as_str())));

    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,width,height",
            "-of",
            "json",
        ])
        .arg(&destination)
        .output()
        .expect("ffprobe is installed in Linux export qualification environment");
    assert!(
        probe.status.success(),
        "{}",
        String::from_utf8_lossy(&probe.stderr)
    );
    let streams: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
    let streams = streams["streams"].as_array().unwrap();
    assert!(streams.iter().any(|stream| stream["codec_type"] == "video"));
    assert!(streams.iter().any(|stream| stream["codec_type"] == "audio"));

    let compile_environment =
        fframes_studio::build_service::CompileEnvironment::resolve(&sdk, &manifest, &builds)
            .unwrap();
    let frame_directory = temporary.path().join("frames");
    fs::create_dir_all(&frame_directory).unwrap();
    let frame_output = frame_directory.to_string_lossy().into_owned();
    let frame = run_cli(
        &preview_build.root,
        &compile_environment,
        &["--json", "frame", "0", "-o", &frame_output],
    );
    assert!(
        frame.status.success(),
        "{}{}",
        String::from_utf8_lossy(&frame.stdout),
        String::from_utf8_lossy(&frame.stderr)
    );
    assert!(frame_directory.join("0.png").is_file());
    let inspect = run_cli(
        &preview_build.root,
        &compile_environment,
        &["--json", "inspect", "--fail-on", "warning"],
    );
    assert!(
        inspect.status.success(),
        "{}{}",
        String::from_utf8_lossy(&inspect.stdout),
        String::from_utf8_lossy(&inspect.stderr)
    );
    let strip_path = temporary.path().join("strip.png");
    let strip_output = strip_path.to_string_lossy().into_owned();
    let strip = run_cli(
        &preview_build.root,
        &compile_environment,
        &["--json", "strip", "0..1s", "-n", "3", "-o", &strip_output],
    );
    assert!(
        strip.status.success(),
        "{}{}",
        String::from_utf8_lossy(&strip.stdout),
        String::from_utf8_lossy(&strip.stderr)
    );
    assert!(strip_path.is_file());
}
