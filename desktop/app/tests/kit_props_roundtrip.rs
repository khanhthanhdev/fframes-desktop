//! A compiled worker that draws an editable fframes-kit component answers the props panel:
//! the frame names the component, `PropsInfo` gives its schema and values, a props
//! document redraws it, and its export capabilities say it exports with props.
//!
//! The fixture binary comes from `KIT_PROPS_WORKER_BINARY`, else from the fixture's own
//! target directory (build it first):
//!
//! ```text
//! cargo build --manifest-path desktop/fixtures/kit-props/Cargo.toml
//! cargo test --locked --manifest-path desktop/Cargo.toml -p fframes-studio \
//!   --test kit_props_roundtrip -- --ignored
//! ```
use fframes_studio_protocol::export::{EXPORT_CAPABILITIES_FLAG, ExportCapabilities};
use fframes_studio_protocol::{EditorObjectIdentity, PreviewIdentity, PropsDocument};
use serde_json::json;
use std::{path::PathBuf, process::Command, time::Duration};
use studio_bootstrap::{ChildEnvironment, ProcessTreeManager};
use studio_engine::services::{
    preview_worker_client::PreviewWorkerClient, worker_client::WorkerClient,
};

fn fixture_binary(project: &std::path::Path) -> PathBuf {
    if let Some(binary) = std::env::var_os("KIT_PROPS_WORKER_BINARY") {
        return PathBuf::from(binary);
    }
    let metadata = Command::new("cargo")
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(project.join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(metadata.status.success());
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
    let root = PathBuf::from(metadata["target_directory"].as_str().unwrap());
    let name = if cfg!(windows) {
        "kit-props.exe"
    } else {
        "kit-props"
    };
    let debug = root.join("debug").join(name);
    if debug.is_file() {
        debug
    } else {
        root.join("release").join(name)
    }
}

/// How many pixels of the frame are bright (the dot on the black background).
fn lit(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[..3].iter().any(|&c| c > 60))
        .count()
}

#[test]
#[ignore = "requires the compiled kit-props fixture and loopback sockets"]
fn real_kit_worker_answers_props_info_and_redraws_with_a_props_document() {
    let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/kit-props");
    let binary = fixture_binary(&project);

    let capabilities = Command::new(&binary)
        .arg(EXPORT_CAPABILITIES_FLAG)
        .output()
        .unwrap();
    assert!(capabilities.status.success(), "{capabilities:?}");
    let capabilities: ExportCapabilities = serde_json::from_slice(&capabilities.stdout).unwrap();
    assert!(capabilities.props, "the worker exports with --props-file");

    let identity = PreviewIdentity {
        project_id: "kit-props".into(),
        open_session: "props-roundtrip".into(),
        source_revision: "rev".into(),
        worker_generation: 3,
    };
    let manager = ProcessTreeManager::new();
    let mut transport = WorkerClient::new(&identity.source_revision, identity.worker_generation);
    transport
        .spawn_worker(
            &binary,
            &[
                "--preview-worker",
                "--project-id",
                &identity.project_id,
                "--open-session",
                &identity.open_session,
            ],
            Some(&project),
            ChildEnvironment::default_allowlist(),
            &manager,
        )
        .unwrap();
    let mut worker = PreviewWorkerClient::new(transport, identity);
    worker.negotiate().unwrap();
    assert!(worker.supports_props());
    assert!(worker.supports_suggested_stops());
    let timeline = worker.timeline().unwrap();

    // The frame names the editable circle; the store recorded it while drawing.
    let before = worker.frame(0, 1, 1.0).unwrap();
    let metadata = before.response.editor_metadata.as_ref().unwrap();
    let dot = metadata
        .objects
        .iter()
        .find(|object| object.identity.object_key == "dot")
        .expect("the editable circle is in the frame metadata");
    let dot: EditorObjectIdentity = dot.identity.clone();
    assert_eq!(dot.component_key, "Circle");
    let info = worker
        .props_info(dot.clone())
        .unwrap()
        .info
        .expect("the circle is editable");
    assert!(
        info.schema
            .fields
            .iter()
            .any(|field| field.name == "radius")
    );
    assert_eq!(info.code_values["radius"], json!(40.0));
    assert_eq!(info.values["radius"], json!(40.0));

    // A document triples the radius: the next frame draws it (the outline is three times as
    // long) and the info reports it.
    let mut doc = PropsDocument::default();
    doc.set(&dot, "radius", Some(json!(120.0)));
    worker.set_props(Some(doc.to_json()), 1).unwrap();
    let after = worker.frame(0, 2, 1.0).unwrap();
    assert!(
        lit(&after.pixels) > lit(&before.pixels) * 5 / 2,
        "{} lit pixels before, {} after",
        lit(&before.pixels),
        lit(&after.pixels)
    );
    let answer = worker.props_info(dot.clone()).unwrap();
    assert_eq!(answer.props_generation, 1);
    let info = answer.info.unwrap();
    assert_eq!(info.code_values["radius"], json!(40.0));
    assert_eq!(info.values["radius"], json!(120.0));

    // No document draws the code's values again.
    worker.set_props(None, 2).unwrap();
    let restored = worker.frame(0, 3, 1.0).unwrap();
    assert_eq!(lit(&restored.pixels), lit(&before.pixels));

    // Sample twice with an ordinary seek between batches. Observations replace the
    // same origin, carry notes, and do not duplicate on another render or a fresh scan.
    let first = worker.suggest_stops(vec![0, 15], 0, true).unwrap();
    assert_eq!(first.stops.len(), 2);
    assert!(!first.truncated);
    assert_eq!(first.stops[0].scene, "video");
    assert_eq!(first.stops[0].seconds, 0.5);
    assert_eq!(first.stops[1].seconds, 1.25);
    assert_eq!(first.stops[0].label.as_deref(), Some("Introduce the dot"));
    assert_eq!(
        first.stops[0].notes.as_deref(),
        Some("The Props panel changes its radius.")
    );
    let seek = worker.frame(37, 4, 1.0).unwrap();
    assert_eq!(seek.response.frame_index, 37);
    assert_eq!(worker.suggest_stops(vec![30, 45], 0, false).unwrap(), first);
    assert_eq!(worker.suggest_stops(vec![0], 0, true).unwrap(), first);

    // This fixture has no scenes: its root-video suggestions must still become usable
    // slide stops, with fractional reveals rounded up and their speaker notes retained.
    use studio_engine::slides::{SlidesDoc, resolve, suggest};
    assert!(timeline.scenes.is_empty());
    let proposals = suggest::proposals(&first, &timeline, &SlidesDoc::default());
    let accepted = suggest::accepting(&SlidesDoc::default(), &proposals).unwrap();
    let (stops, problems) = resolve::resolve(&accepted, &timeline);
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        proposals.iter().map(|p| p.video_frame).collect::<Vec<_>>(),
        [15, 38]
    );
    assert_eq!(
        stops[0].notes.as_deref(),
        Some("The Props panel changes its radius.")
    );
    assert!(suggest::proposals(&first, &timeline, &accepted).is_empty());

    drop(worker);
    manager.terminate_all(Duration::from_millis(300));
    assert_eq!(manager.active_count(), 0);
}
