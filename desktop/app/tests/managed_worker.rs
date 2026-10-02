use fframes_studio::worker_project;
use studio_bootstrap::ProcessTreeManager;
use studio_sdk::{CompatibilityManifest, SdkInstaller};

#[test]
#[ignore = "requires SDK_BUNDLE containing assembled native SDK artifacts"]
fn installed_bundle_builds_and_launches_managed_worker() {
    let bundle = std::path::PathBuf::from(std::env::var_os("SDK_BUNDLE").expect("SDK_BUNDLE"));
    let manifest = CompatibilityManifest::from_json_str(
        &std::fs::read_to_string(bundle.join("compatibility.json")).unwrap(),
    )
    .unwrap();
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
    let installer = SdkInstaller::new(temporary.path().join("sdk"));
    let sdk = installer
        .install_from_local_artifacts(&manifest, &artifacts)
        .unwrap();
    let project = temporary.path().join("video");
    worker_project::create_worker_project(&project, &sdk).unwrap();
    let manager = ProcessTreeManager::new();
    let mut worker = worker_project::launch_worker(&project, &sdk, manifest, 1, &manager).unwrap();
    worker.request_render_frame(0).unwrap();
    let header = worker.latest_header().unwrap().clone();
    assert_eq!(worker.latest_pixels().unwrap().len(), 1920 * 1080 * 4);
    assert_eq!(
        worker.request_elements(&header).unwrap()[0].element_id,
        "intro.title"
    );
    drop(worker);
    manager.terminate_all(std::time::Duration::from_millis(300));
    assert_eq!(manager.active_count(), 0);
}
