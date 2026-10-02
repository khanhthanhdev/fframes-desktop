use fframes_studio::{WorkerClient, selection_spike::DisplayedSourceFrame};
use std::{path::PathBuf, time::Duration};
use studio_agent_spike::source_revision;
use studio_bootstrap::{ChildEnvironment, ProcessTreeManager};

#[test]
#[ignore = "requires the compiled native annotated fixture and loopback sockets"]
fn real_worker_frame_anchor_crash_and_restart() {
    let project =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/annotated-video-overlay");
    let binary = std::env::var_os("ANNOTATED_WORKER_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let metadata = std::process::Command::new("cargo")
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
                "annotated-video-overlay.exe"
            } else {
                "annotated-video-overlay"
            };
            let debug = root.join("debug").join(name);
            if debug.is_file() {
                debug
            } else {
                root.join("release").join(name)
            }
        });
    let revision = source_revision(&project).unwrap();
    let manager = ProcessTreeManager::new();
    let mut worker = WorkerClient::new(&revision, 41);
    for generation in [41, 42] {
        worker
            .spawn_worker(
                &binary,
                &["--worker"],
                Some(&project),
                ChildEnvironment::default_allowlist(),
                &manager,
            )
            .unwrap();
        let hello = worker.send_hello().unwrap();
        assert_eq!(hello.worker_generation, generation);
        assert_eq!(hello.source_revision, revision);
        worker.request_render_frame(0).unwrap();
        let header = worker.latest_header().unwrap().clone();
        assert_eq!((header.width, header.height), (1920, 1080));
        assert_eq!(worker.latest_pixels().unwrap().len(), header.payload_len);
        let frame = DisplayedSourceFrame {
            elements: worker.request_elements(&header).unwrap(),
            header,
            project_root: project.clone(),
        };
        let selected = frame.select(150., 250., generation, &revision).unwrap();
        assert!(selected.code_snippet.contains("let title_text ="));
        let mut duplicated = frame.clone();
        duplicated.elements.push(duplicated.elements[0].clone());
        assert!(
            duplicated
                .select(150., 250., generation, &revision)
                .is_err()
        );
        worker.force_crash().unwrap();
        assert!(worker.request_render_frame(1).is_err());
        assert!(worker.latest_header().is_some());
        worker.restart_generation();
    }
    drop(worker);
    manager.terminate_all(Duration::from_millis(300));
    assert_eq!(manager.active_count(), 0);
}
