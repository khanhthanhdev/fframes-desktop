use fframes_studio::project_view::ProjectPresentation;
use std::fs;
use studio_engine::{
    Controller, JobKind, JobResult, app_paths::AppPaths, build_materialization::sdk_pin,
};
use studio_sdk::CompatibilityManifest;

#[test]
fn complete_foundation_flow_retains_source_assets_draft_and_checkpoint() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let paths = AppPaths::new(temp.path().join("app")).unwrap();
    studio_project::create(
        &root,
        "Video",
        sdk_pin(&CompatibilityManifest::default_linux_x64()),
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let asset = temp.path().join("asset.dat");
    fs::write(&asset, b"real asset bytes").unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let source = controller.state().source().clone();
    let error = controller
        .select_sdk(root.join("media"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("media/compatibility.json") || error.contains("media\\compatibility.json")
    );
    assert!(error.contains("select a complete compatible installed SDK"));
    assert_eq!(controller.sdk_path(), None);
    assert_eq!(controller.state().source(), &source);
    controller.copy_asset(&asset).unwrap();
    controller.checkpoint().unwrap();
    fs::remove_file(asset).unwrap();
    let accepted = controller.state().accepted().clone();
    controller.close().unwrap();
    drop(controller);
    let moved = temp.path().join("relocated");
    fs::rename(&root, &moved).unwrap();
    let mut controller = Controller::open(&moved, &paths).unwrap();
    assert_eq!(controller.state().accepted(), &accepted);
    let tag = controller.begin_job(JobKind::Checkpoint).unwrap();
    let draft = controller.draft().unwrap().to_owned();
    fs::write(draft.join("src/lib.rs"), "// draft modified before crash").unwrap();
    drop(controller);
    fs::write(moved.join("src/lib.rs"), "// external source after crash").unwrap();
    let mut controller = Controller::open(&moved, &paths).unwrap();
    let view = ProjectPresentation::from_controller(&controller);
    assert!(view.interrupted);
    assert_ne!(view.source, view.accepted);
    assert_eq!(view.accepted, accepted);
    assert!(
        controller
            .complete(&tag, JobResult::Checkpointed(accepted))
            .is_err()
    );
    assert_eq!(
        fs::read(moved.join("media/asset.dat")).unwrap(),
        b"real asset bytes"
    );
    assert_eq!(
        fs::read(draft.join("src/lib.rs")).unwrap(),
        b"// draft modified before crash"
    );
    let recovery_copy = temp.path().join("recovery-copy");
    controller
        .restore_as_copy(&recovery_copy, &view.source)
        .unwrap();
    assert_ne!(
        studio_project::open(&recovery_copy)
            .unwrap()
            .manifest
            .project_id,
        view.id
    );
    assert_eq!(
        fs::read(moved.join("src/lib.rs")).unwrap(),
        b"// external source after crash"
    );
    assert_ne!(
        fs::read(recovery_copy.join("src/lib.rs")).unwrap(),
        b"// external source after crash"
    );
    assert!(Controller::open(&recovery_copy, &paths).is_ok());
    controller.close().unwrap();
    assert_eq!(controller.processes.active_count(), 0);
}
