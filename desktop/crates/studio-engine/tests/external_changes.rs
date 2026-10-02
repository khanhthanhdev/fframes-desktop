use std::fs;
use studio_engine::{
    Controller, JobKind, JobResult, JobState, app_paths::AppPaths, build_materialization::sdk_pin,
};
use studio_sdk::CompatibilityManifest;

#[test]
fn external_edit_rejects_late_completion_and_preserves_baseline_and_draft() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    studio_project::create(
        &root,
        "Video",
        sdk_pin(&CompatibilityManifest::default_linux_x64()),
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let accepted = controller.state().accepted().clone();
    assert!(Controller::open(&root, &paths).is_err());
    let tag = controller.begin_job(JobKind::Checkpoint).unwrap();
    let draft = controller.draft().unwrap().to_owned();
    fs::write(draft.join("src/lib.rs"), "// retained draft").unwrap();
    fs::write(root.join("src/lib.rs"), "// manual edit").unwrap();
    assert!(
        controller
            .complete(&tag, JobResult::Checkpointed(accepted.clone()))
            .is_err()
    );
    assert!(matches!(controller.state().job(), JobState::Interrupted(_)));
    assert_eq!(controller.state().accepted(), &accepted);
    assert_eq!(
        fs::read(root.join("src/lib.rs")).unwrap(),
        b"// manual edit"
    );
    controller.close().unwrap();
    drop(controller);
    let mut reopened = Controller::open(&root, &paths).unwrap();
    assert!(
        reopened
            .complete(&tag, JobResult::Built(accepted.clone()))
            .is_err()
    );
    assert_eq!(
        fs::read(draft.join("src/lib.rs")).unwrap(),
        b"// retained draft"
    );
    reopened.checkpoint().unwrap();
    assert_ne!(reopened.state().accepted(), &accepted);
}

#[test]
fn invalid_source_interrupts_work_and_edit_back_cannot_resurrect_it() {
    for fault in ["manifest", "cargo", "asset", "identity"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("video");
        let paths = AppPaths::new(temp.path().join("history")).unwrap();
        let project = studio_project::create(
            &root,
            "Video",
            sdk_pin(&CompatibilityManifest::default_linux_x64()),
            "1.1.0",
            "0.1.0",
        )
        .unwrap();
        let mut controller = Controller::open(&root, &paths).unwrap();
        let accepted = controller.state().accepted().clone();
        let tag = controller.begin_job(JobKind::Build).unwrap();
        controller
            .complete(&tag, JobResult::Built(accepted.clone()))
            .unwrap();
        assert_eq!(controller.state().built(), Some(&accepted));
        let tag = controller.begin_job(JobKind::Build).unwrap();
        let draft = controller.draft().unwrap().to_owned();
        let generation = controller.state().generation();
        let manifest = fs::read(root.join("studio.json")).unwrap();
        let cargo = fs::read(root.join("Cargo.toml")).unwrap();
        let font = fs::read(root.join("media/DMSans-Medium.ttf")).unwrap();
        let rust = fs::read(root.join("src/lib.rs")).unwrap();
        match fault {
            "manifest" => {
                fs::write(root.join("studio.json"), b"{").unwrap();
                fs::write(
                    root.join("src/lib.rs"),
                    b"// changed while manifest was invalid",
                )
                .unwrap();
            }
            "cargo" => fs::write(root.join("Cargo.toml"), b"[").unwrap(),
            "asset" => fs::remove_file(root.join("media/DMSans-Medium.ttf")).unwrap(),
            "identity" => {
                let mut changed = project.manifest.clone();
                changed.project_id = "different-project".to_owned().try_into().unwrap();
                studio_project::lifecycle::write_manifest(&root, &changed).unwrap();
                fs::remove_file(root.join("media/DMSans-Medium.ttf")).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            controller
                .complete(&tag, JobResult::Built(accepted.clone()))
                .is_err(),
            "{fault}"
        );
        assert!(
            matches!(controller.state().job(), JobState::Interrupted(_)),
            "{fault}"
        );
        assert!(controller.state().generation() > generation);
        assert!(controller.state().built().is_none());
        assert_eq!(controller.state().accepted(), &accepted);
        assert_eq!(
            controller.project.manifest.project_id,
            project.manifest.project_id
        );
        assert_eq!(
            controller.state().source(),
            &controller.project.inventory.revision
        );
        assert!(controller.recovery_notice.is_some());
        let restored = temp.path().join("export");
        controller.export_checkpoint(&accepted, &restored).unwrap();
        assert!(studio_project::open(&restored).is_ok());
        fs::write(root.join("studio.json"), manifest).unwrap();
        fs::write(root.join("Cargo.toml"), cargo).unwrap();
        fs::write(root.join("media/DMSans-Medium.ttf"), font).unwrap();
        fs::write(root.join("src/lib.rs"), rust).unwrap();
        controller.reconcile().unwrap();
        assert!(
            controller
                .complete(&tag, JobResult::Built(accepted.clone()))
                .is_err(),
            "{fault}"
        );
        assert!(controller.state().built().is_none());
        controller.checkpoint().unwrap();
        assert_eq!(controller.state().accepted(), &accepted);
        assert!(draft.join("src/lib.rs").is_file());
    }
}

#[test]
fn restore_comparison_is_strict_and_independent_export_survives_invalid_source() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    studio_project::create(
        &root,
        "Video",
        sdk_pin(&CompatibilityManifest::default_linux_x64()),
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let accepted = controller.state().accepted().clone();
    fs::write(root.join("src/lib.rs"), b"// external edit").unwrap();
    let destination = temp.path().join("stale-restore");
    assert!(controller.restore_as_copy(&destination, &accepted).is_err());
    assert!(!destination.exists());
    let current = controller.state().source().clone();
    fs::write(root.join("studio.json"), b"invalid").unwrap();
    assert!(controller.restore_as_copy(&destination, &current).is_err());
    assert!(!destination.exists());
    controller
        .export_checkpoint(&accepted, &destination)
        .unwrap();
    assert!(studio_project::open(&destination).is_ok());
    assert_eq!(
        fs::read(root.join("src/lib.rs")).unwrap(),
        b"// external edit"
    );
    assert_eq!(fs::read(root.join("studio.json")).unwrap(), b"invalid");
}
