use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use studio_engine::{
    Controller, JobKind, JobState, app_paths::AppPaths, build_materialization::sdk_pin,
};
use studio_sdk::CompatibilityManifest;

fn reject_history_writes(db: &rusqlite::Connection) {
    db.execute_batch(
        "CREATE TRIGGER reject_history_insert BEFORE INSERT ON projects BEGIN
            SELECT RAISE(FAIL, 'injected history write failure'); END;
         CREATE TRIGGER reject_history_update BEFORE UPDATE ON projects BEGIN
            SELECT RAISE(FAIL, 'injected history write failure'); END;",
    )
    .unwrap();
}

fn allow_history_writes(db: &rusqlite::Connection) {
    db.execute_batch("DROP TRIGGER reject_history_insert; DROP TRIGGER reject_history_update;")
        .unwrap();
}

#[test]
fn killed_at_every_durable_job_boundary_replays_without_source_overwrite() {
    for boundary in [
        "intent", "objects", "draft", "commit", "database", "running",
    ] {
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
        let accepted = {
            let c = Controller::open(&root, &paths).unwrap();
            c.state().accepted().clone()
        };
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_child", "--ignored", "--nocapture"])
            .env("CRASH_ROOT", &root)
            .env("CRASH_DATA", &paths.data)
            .env("CRASH_BOUNDARY", boundary)
            .spawn()
            .unwrap();
        let marker = paths.data.join("boundary");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !marker.exists() {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child did not reach {boundary}");
        }
        fs::write(
            root.join("src/lib.rs"),
            format!("// external edit during {boundary}"),
        )
        .unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        let mut controller = Controller::open(&root, &paths).unwrap();
        assert_eq!(controller.state().accepted(), &accepted);
        assert!(
            matches!(controller.state().job(), JobState::Interrupted(_)),
            "{boundary}"
        );
        assert_ne!(controller.state().source(), &accepted);
        if !matches!(boundary, "intent" | "objects") {
            assert!(controller.draft().unwrap().join("src/lib.rs").is_file());
        }
        let draft = controller.draft().unwrap().to_owned();
        let source = fs::read(root.join("src/lib.rs")).unwrap();
        controller.close().unwrap();
        drop(controller);
        let reopened = Controller::open(&root, &paths).unwrap();
        assert_eq!(reopened.state().accepted(), &accepted);
        assert_eq!(reopened.draft(), Some(draft.as_path()));
        assert_eq!(fs::read(root.join("src/lib.rs")).unwrap(), source);
        assert_eq!(reopened.processes.active_count(), 0);
    }
}

#[test]
#[ignore = "child-process fault injection entry; invoked by boundary test"]
fn crash_child() {
    let root = std::env::var_os("CRASH_ROOT").unwrap();
    let data = std::env::var_os("CRASH_DATA").unwrap();
    let boundary = std::env::var("CRASH_BOUNDARY").unwrap();
    let paths = AppPaths::new(data).unwrap();
    let mut controller = Controller::open(Path::new(&root), &paths).unwrap();
    controller
        .begin_job_observed(JobKind::Checkpoint, |stage| {
            if stage == boundary {
                fs::write(paths.data.join("boundary"), stage).unwrap();
                loop {
                    std::thread::park();
                }
            }
        })
        .unwrap();
}

#[test]
fn missing_object_and_newer_database_fail_without_mutating_source() {
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
    let controller = Controller::open(&root, &paths).unwrap();
    let inventory = controller
        .checkpoints
        .load(controller.state().accepted())
        .unwrap();
    let object = controller
        .checkpoints
        .root
        .join("objects")
        .join(&inventory.files[0].sha256);
    drop(controller);
    fs::remove_file(object).unwrap();
    assert!(Controller::open(&root, &paths).is_err());
    assert_eq!(
        studio_project::open(&root).unwrap().inventory,
        project.inventory
    );
    let db = rusqlite::Connection::open(paths.database()).unwrap();
    db.pragma_update(None, "user_version", 999).unwrap();
    drop(db);
    assert!(Controller::open(&root, &paths).is_err());
    assert_eq!(
        studio_project::open(&root).unwrap().inventory,
        project.inventory
    );
}

#[test]
fn failed_checkpoint_transitions_to_failed_allowing_retry() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    let _project = studio_project::create(
        &root,
        "Video",
        sdk_pin(&CompatibilityManifest::default_linux_x64()),
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();

    let objects_dir = controller.checkpoints.root.join("objects");
    let _err = controller
        .checkpoint_observed(|stage| {
            if stage == "job_started" {
                fs::remove_dir_all(&objects_dir).unwrap();
                fs::write(&objects_dir, b"blocked").unwrap();
            }
        })
        .unwrap_err();
    assert!(matches!(controller.state().job(), JobState::Failed(_, _)));
    fs::remove_file(&objects_dir).unwrap();
    fs::create_dir_all(&objects_dir).unwrap();
    assert!(controller.checkpoint().is_ok());
    assert!(matches!(controller.state().job(), JobState::Succeeded(_)));
}

#[test]
fn missing_declared_asset_allows_checkpoint_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    let _project = studio_project::create(
        &root,
        "Video",
        sdk_pin(&CompatibilityManifest::default_linux_x64()),
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let accepted_before = controller.state().accepted().clone();

    fs::remove_file(root.join("media/DMSans-Medium.ttf")).unwrap();
    controller.close().unwrap();
    drop(controller);

    let controller = Controller::open(&root, &paths).unwrap();
    assert!(controller.recovery_notice.is_some());
    assert_eq!(controller.state().accepted(), &accepted_before);

    let destination = temp.path().join("restored");
    controller
        .export_checkpoint(&accepted_before, &destination)
        .unwrap();

    assert!(destination.join("media/DMSans-Medium.ttf").is_file());
    assert!(studio_project::open(&destination).is_ok());
}

#[test]
fn database_failure_after_journal_commit_does_not_rollback_accepted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    let _project = studio_project::create(
        &root,
        "Video",
        sdk_pin(&CompatibilityManifest::default_linux_x64()),
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let initial_accepted = controller.state().accepted().clone();

    fs::write(root.join("src/lib.rs"), b"// checkpoint 1").unwrap();
    controller.checkpoint().unwrap();
    let checkpoint1 = controller.state().accepted().clone();
    assert_ne!(initial_accepted, checkpoint1);

    fs::write(root.join("src/lib.rs"), b"// checkpoint 2").unwrap();
    let newest = studio_project::SourceInventory::scan(&root)
        .unwrap()
        .revision;
    assert_ne!(newest, checkpoint1);
    let db = rusqlite::Connection::open(paths.database()).unwrap();
    // Inject only after startup so the checkpoint commits before the SQLite write fails.
    let error = controller
        .checkpoint_observed(|stage| {
            if stage == "job_started" {
                reject_history_writes(&db);
            }
        })
        .unwrap_err();
    assert!(error.to_string().contains("injected history write failure"));
    assert_eq!(controller.state().accepted(), &newest);
    assert!(matches!(controller.state().job(), JobState::Succeeded(_)));
    let store = studio_engine::store::Store::open(&paths.database()).unwrap();
    assert_eq!(
        store
            .get(controller.state().project())
            .unwrap()
            .unwrap()
            .state
            .accepted(),
        &checkpoint1
    );
    allow_history_writes(&db);
    // Closing used to overwrite the newest journal record with the stale memory record.
    controller.close().unwrap();
    drop(controller);
    let reopened = Controller::open(&root, &paths).unwrap();
    assert_eq!(reopened.state().accepted(), &newest);
    assert_eq!(
        store
            .get(reopened.state().project())
            .unwrap()
            .unwrap()
            .state
            .accepted(),
        &newest
    );
    let restored = temp.path().join("newest-checkpoint");
    reopened.export_checkpoint(&newest, &restored).unwrap();
    assert_eq!(
        fs::read(restored.join("src/lib.rs")).unwrap(),
        b"// checkpoint 2"
    );
}

#[test]
fn database_failure_during_startup_settles_job_and_preserves_draft_for_retry() {
    for boundary in ["commit", "database"] {
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
        fs::write(root.join("src/lib.rs"), b"// new source, not yet accepted").unwrap();
        controller.reconcile().unwrap();
        let db = rusqlite::Connection::open(paths.database()).unwrap();
        let error = controller
            .checkpoint_observed(|stage| {
                if stage == boundary {
                    reject_history_writes(&db);
                }
            })
            .unwrap_err();
        assert!(
            error.to_string().contains("injected history write failure"),
            "{boundary}: {error}"
        );
        assert!(
            matches!(controller.state().job(), JobState::Failed(_, _)),
            "{boundary}"
        );
        assert_eq!(controller.state().accepted(), &accepted);
        assert!(
            controller
                .recovery_notice
                .as_ref()
                .unwrap()
                .contains("recovery required")
        );
        let draft = controller.draft().unwrap().to_owned();
        assert_eq!(
            fs::read(draft.join("src/lib.rs")).unwrap(),
            b"// new source, not yet accepted"
        );
        allow_history_writes(&db);
        controller.checkpoint().unwrap();
        assert_ne!(controller.state().accepted(), &accepted);
        assert!(matches!(controller.state().job(), JobState::Succeeded(_)));
        assert!(draft.join("src/lib.rs").exists());
    }
}

#[test]
fn missing_asset_during_checkpoint_never_accepts_and_retry_after_repair_succeeds() {
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
    let font = root.join("media/DMSans-Medium.ttf");
    let bytes = fs::read(&font).unwrap();
    let error = controller
        .checkpoint_observed(|stage| {
            if stage == "job_started" {
                fs::remove_file(&font).unwrap();
            }
        })
        .unwrap_err();
    assert!(error.to_string().contains("DMSans-Medium.ttf"));
    assert!(matches!(controller.state().job(), JobState::Interrupted(_)));
    assert_eq!(controller.state().accepted(), &accepted);
    assert_eq!(
        controller.state().source(),
        &controller.project.inventory.revision
    );
    let draft = controller.draft().unwrap().to_owned();
    assert_eq!(
        fs::read(draft.join("media/DMSans-Medium.ttf")).unwrap(),
        bytes
    );
    let restored = temp.path().join("recovery");
    controller.export_checkpoint(&accepted, &restored).unwrap();
    assert_eq!(
        fs::read(restored.join("media/DMSans-Medium.ttf")).unwrap(),
        bytes
    );
    fs::write(font, bytes).unwrap();
    controller.checkpoint().unwrap();
    assert!(matches!(controller.state().job(), JobState::Succeeded(_)));
    assert_eq!(controller.state().accepted(), &accepted);
}

#[test]
fn failed_invalidation_write_cannot_resurrect_an_old_result_after_source_repair() {
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
    let tag = controller.begin_job(JobKind::Build).unwrap();
    let manifest = fs::read(root.join("studio.json")).unwrap();
    fs::write(root.join("studio.json"), b"invalid").unwrap();
    let db = rusqlite::Connection::open(paths.database()).unwrap();
    reject_history_writes(&db);
    let error = controller.reconcile().unwrap_err();
    assert!(error.to_string().contains("injected history write failure"));
    assert!(matches!(controller.state().job(), JobState::Interrupted(_)));
    assert_eq!(controller.state().accepted(), &accepted);
    allow_history_writes(&db);
    fs::write(root.join("studio.json"), manifest).unwrap();
    assert!(
        controller
            .complete(&tag, studio_engine::JobResult::Built(accepted.clone()))
            .is_err()
    );
    assert!(controller.state().built().is_none());
    controller.checkpoint().unwrap();
    assert_eq!(controller.state().accepted(), &accepted);
}

#[test]
fn failed_draft_publication_retains_attempt_and_retry_uses_a_new_directory() {
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
    let journal_path = paths
        .project(&project.manifest.project_id)
        .join("lifecycle.jsonl");
    controller
        .checkpoint_observed(|stage| {
            if stage == "objects" {
                let (_, replay) =
                    studio_engine::journal::Journal::open(journal_path.clone()).unwrap();
                let draft = replay.pending.unwrap().draft.unwrap();
                // Model an incomplete draft left at the attempted publication path.
                fs::create_dir(&draft).unwrap();
                fs::write(draft.join("partial.rs"), b"retained partial bytes").unwrap();
            }
        })
        .unwrap_err();
    assert!(matches!(controller.state().job(), JobState::Failed(_, _)));
    assert_eq!(controller.state().accepted(), &accepted);
    let draft = controller.draft().unwrap().to_owned();
    assert_eq!(
        fs::read(draft.join("partial.rs")).unwrap(),
        b"retained partial bytes"
    );
    controller.checkpoint().unwrap();
    assert!(matches!(controller.state().job(), JobState::Succeeded(_)));
    assert_ne!(controller.draft().unwrap(), draft);
    assert_eq!(
        fs::read(draft.join("partial.rs")).unwrap(),
        b"retained partial bytes"
    );
    assert_eq!(controller.state().accepted(), &accepted);
}
