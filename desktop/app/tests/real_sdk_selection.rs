//! Compile a generated project with an assembled managed SDK and verify its semantic
//! title identity, displayed-frame geometry, and explicit source anchor end to end.
//!
//! ```text
//! SDK_BUNDLE=/absolute/path/to/verified-bundle cargo test --locked \
//!   --manifest-path desktop/Cargo.toml -p fframes-studio --test real_sdk_selection -- --ignored
//! ```
use fframes_studio::worker_project;
use fframes_studio_protocol::{EditorFrameStatus, PreviewIdentity};
use std::{fs, path::PathBuf, process::Command, time::Duration};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::build_materialization::sdk_pin;
use studio_project::{
    ProjectPath, SourceRevision,
    revision::FileKind,
    source_index::{SourceAnchor, SourceIndex, SourceIndexInput},
};
use studio_sdk::{CompatibilityManifest, SdkInstaller};

fn run_video_cli(
    project_root: &std::path::Path,
    environment: &studio_sdk::environment::SdkEnvironment,
    arguments: &[&str],
) -> std::process::Output {
    let mut command = Command::new("cargo");
    command
        .current_dir(project_root)
        .args(["run", "--locked", "--offline", "--quiet", "--manifest-path"])
        .arg(project_root.join("Cargo.toml"))
        .arg("--")
        .args(arguments);
    environment
        .build_child_environment()
        .apply_to_command(&mut command);
    command.output().expect("run generated video CLI")
}

#[test]
#[ignore = "requires SDK_BUNDLE containing the current assembled managed SDK"]
fn generated_title_identity_geometry_and_source_anchor_survive_managed_render() {
    let bundle = PathBuf::from(std::env::var_os("SDK_BUNDLE").expect("SDK_BUNDLE"));
    let manifest = CompatibilityManifest::from_json_str(
        &fs::read_to_string(bundle.join("compatibility.json")).expect("SDK compatibility manifest"),
    )
    .expect("valid SDK compatibility manifest");
    let temporary = tempfile::tempdir().unwrap();
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
    let sdk = SdkInstaller::new(temporary.path().join("sdk"))
        .install_from_local_artifacts(&manifest, &artifacts)
        .expect("install verified local SDK bundle");
    let project_root = temporary.path().join("generated-video");
    let project = studio_project::create(
        &project_root,
        "M5 managed selection fixture",
        sdk_pin(&manifest),
        &manifest.fframes_version,
        "0.1.0",
    )
    .expect("generate current Studio starter project");

    let manager = ProcessTreeManager::new();
    let builds =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/m5-managed-selection-builds");
    let build = worker_project::compile_portable_worker(
        &project,
        &sdk,
        manifest.clone(),
        &builds,
        &manager,
    )
    .expect("compile generated project without an overlay");
    let render_evidence = std::env::var_os("M5_RENDER_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| temporary.path().join("render-evidence"));
    let frame_directory = render_evidence.join("m5-managed-title-frame");
    let strip_path = render_evidence.join("m5-managed-title-strip.png");
    fs::create_dir_all(&frame_directory).unwrap();
    let frame_output = frame_directory.to_string_lossy().into_owned();
    let frame_cli = run_video_cli(
        &build.root,
        &build.environment,
        &["--json", "frame", "0", "-o", &frame_output],
    );
    assert!(
        frame_cli.status.success(),
        "generated frame CLI failed: {}{}",
        String::from_utf8_lossy(&frame_cli.stdout),
        String::from_utf8_lossy(&frame_cli.stderr)
    );
    assert!(frame_directory.join("0.png").is_file());
    let inspect_cli = run_video_cli(
        &build.root,
        &build.environment,
        &["--json", "inspect", "--fail-on", "warning"],
    );
    assert!(
        inspect_cli.status.success(),
        "generated inspect CLI found a warning/error: {}{}",
        String::from_utf8_lossy(&inspect_cli.stdout),
        String::from_utf8_lossy(&inspect_cli.stderr)
    );
    let inspect: serde_json::Value = serde_json::from_slice(&inspect_cli.stdout).unwrap();
    assert!(inspect["checked_frames"].as_u64().unwrap() > 0);
    assert!(
        inspect["findings"].as_array().unwrap().is_empty(),
        "{inspect}"
    );
    fs::create_dir_all(&render_evidence).unwrap();
    fs::write(
        render_evidence.join("m5-managed-inspect-summary.json"),
        serde_json::to_vec_pretty(&inspect).unwrap(),
    )
    .unwrap();
    let strip_output = strip_path.to_string_lossy().into_owned();
    let strip_cli = run_video_cli(
        &build.root,
        &build.environment,
        &["--json", "strip", "0..5s", "-n", "12", "-o", &strip_output],
    );
    assert!(
        strip_cli.status.success(),
        "generated strip CLI failed: {}{}",
        String::from_utf8_lossy(&strip_cli.stdout),
        String::from_utf8_lossy(&strip_cli.stderr)
    );
    assert!(strip_path.is_file());
    let preview_identity = PreviewIdentity {
        project_id: project.manifest.project_id.clone().into(),
        open_session: uuid::Uuid::new_v4().to_string(),
        source_revision: project.inventory.revision.as_str().to_owned(),
        worker_generation: 1,
    };
    let mut worker =
        worker_project::launch_preview_worker(build, preview_identity.clone(), &manager)
            .expect("negotiate generated managed preview worker");
    assert!(worker.supports_editor_frames());
    let timeline = worker.timeline().expect("compiled managed timeline");
    assert_eq!((timeline.width, timeline.height), (1920, 1080));
    let frame = worker.frame(0, 17, 1.0).expect("render title frame");
    assert_eq!(frame.pixels.len(), frame.response.header.payload_len);
    let metadata = frame
        .response
        .editor_metadata
        .expect("frame-coupled metadata");
    assert_eq!(metadata.status, EditorFrameStatus::Supported);
    assert_eq!((metadata.frame_index, metadata.seek_serial), (0, 17));
    assert_eq!((metadata.video_width, metadata.video_height), (1920, 1080));
    let title = metadata
        .objects
        .iter()
        .find(|object| {
            object.identity.scene_instance_key == "starter-video"
                && object.identity.component_key == "starter-title"
                && object.identity.object_key == "headline"
                && object.identity.repeat_key == "primary"
        })
        .expect("starter title carries its four-part semantic identity");
    assert!(title.bounds.width > 0.0 && title.bounds.height > 0.0);
    assert!(title.bounds.x >= 0.0 && title.bounds.y >= 0.0);
    let header = &frame.response.header;
    let pixel_scale_x = f64::from(header.width) / f64::from(metadata.video_width);
    let pixel_scale_y = f64::from(header.height) / f64::from(metadata.video_height);
    let mut visible_title_pixels = 0;
    let min_y = (f64::from(title.bounds.y) * pixel_scale_y).floor() as usize;
    let max_y = (f64::from(title.bounds.y + title.bounds.height) * pixel_scale_y).ceil() as usize;
    let min_x = (f64::from(title.bounds.x) * pixel_scale_x).floor() as usize;
    let max_x = (f64::from(title.bounds.x + title.bounds.width) * pixel_scale_x).ceil() as usize;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let offset = y * header.stride_bytes as usize + x * 4;
            if frame.pixels[offset] > 180
                && frame.pixels[offset + 1] > 180
                && frame.pixels[offset + 2] > 180
                && frame.pixels[offset + 3] > 0
            {
                visible_title_pixels += 1;
            }
        }
    }
    assert!(
        visible_title_pixels > 0,
        "managed frame metadata names the title but no bright rendered title pixels appear inside its bounds"
    );
    let scaled_frame = worker
        .frame(0, 18, 0.111)
        .expect("render fitted preview frame");
    let scaled_header = &scaled_frame.response.header;
    let scale_x = f64::from(scaled_header.width) / f64::from(metadata.video_width);
    let scale_y = f64::from(scaled_header.height) / f64::from(metadata.video_height);
    let mut scaled_title_pixels = 0;
    let min_y = (f64::from(title.bounds.y) * scale_y).floor() as usize;
    let max_y = (f64::from(title.bounds.y + title.bounds.height) * scale_y).ceil() as usize;
    let min_x = (f64::from(title.bounds.x) * scale_x).floor() as usize;
    let max_x = (f64::from(title.bounds.x + title.bounds.width) * scale_x).ceil() as usize;
    for y in min_y..max_y {
        for x in min_x..max_x {
            let offset = y * scaled_header.stride_bytes as usize + x * 4;
            if scaled_frame.pixels[offset] > 180
                && scaled_frame.pixels[offset + 1] > 180
                && scaled_frame.pixels[offset + 2] > 180
                && scaled_frame.pixels[offset + 3] > 0
            {
                scaled_title_pixels += 1;
            }
        }
    }
    assert!(
        scaled_title_pixels > 0,
        "fitted managed frame omitted visible title pixels at {}×{}",
        scaled_header.width,
        scaled_header.height
    );
    let anchor = title
        .source_anchor
        .as_ref()
        .expect("explicit source anchor");
    assert_eq!(anchor.path, "src/lib.rs");
    assert_eq!(anchor.symbol, "render_frame");
    assert_eq!(anchor.marker.as_deref(), Some("studio-title-source-anchor"));
    assert_eq!(title.style_tokens.len(), 4);
    drop(worker);
    manager.terminate_all(Duration::from_millis(300));
    assert_eq!(manager.active_count(), 0);

    let rust_inputs = project
        .inventory
        .files
        .iter()
        .filter(|file| file.kind == FileKind::Rust)
        .map(|file| SourceIndexInput {
            path: file.path.clone(),
            kind: file.kind,
            expected_sha256: file.sha256.clone(),
            expected_size: file.size,
            bytes: fs::read(project.root.join(file.path.as_str())).unwrap(),
        })
        .collect();
    let index = SourceIndex::build(
        SourceRevision::try_from(project.inventory.revision.as_str().to_owned()).unwrap(),
        rust_inputs,
        &|| false,
    )
    .expect("hash-verified immutable source syntax index");
    let lookup = index
        .lookup(&SourceAnchor {
            path: ProjectPath::try_from(anchor.path.clone()).unwrap(),
            symbol: anchor.symbol.clone(),
            expected_sha256: project
                .inventory
                .files
                .iter()
                .find(|file| file.path.as_str() == anchor.path)
                .unwrap()
                .sha256
                .clone(),
            marker: anchor.marker.clone(),
        })
        .expect("anchor resolves against immutable Rust bytes");
    assert_eq!(lookup.snippets[0].confidence, "explicit_marker");
    assert!(
        lookup.snippets[0]
            .text
            .contains("studio-title-source-anchor")
    );
}
