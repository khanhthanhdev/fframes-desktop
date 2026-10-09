use std::fs;
use studio_engine::{
    Controller, app_paths::AppPaths, build_materialization::sdk_pin, store::Store,
};
use studio_sdk::CompatibilityManifest;

#[test]
fn relocation_relinks_but_duplicate_existing_id_is_never_attached() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    let compatibility = CompatibilityManifest::default_linux_x64();
    let project = studio_project::create(
        &root,
        "Video",
        sdk_pin(&compatibility),
        &compatibility.fframes_version,
        "0.1.0",
    )
    .unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    controller.close().unwrap();
    drop(controller);
    let duplicate = temp.path().join("duplicate");
    studio_project::checkpoint::copy_draft(&root, &duplicate).unwrap();
    assert!(
        Controller::open(&duplicate, &paths)
            .err()
            .unwrap()
            .to_string()
            .contains("Duplicate")
    );
    let moved = temp.path().join("moved");
    fs::rename(root, &moved).unwrap();
    let controller = Controller::open(&moved, &paths).unwrap();
    assert!(controller.history_was_available);
    drop(controller);
    let mut store = Store::open(&paths.database()).unwrap();
    assert_eq!(
        store.recents().unwrap()[0].location,
        fs::canonicalize(&moved).unwrap()
    );
    store.remove_recent(&project.manifest.project_id).unwrap();
    assert!(store.recents().unwrap().is_empty());
    assert!(moved.join("studio.json").exists());
    let foreign = AppPaths::new(temp.path().join("other-machine")).unwrap();
    let controller = Controller::open(&moved, &foreign).unwrap();
    assert!(!controller.history_was_available);
}

#[test]
fn both_unversioned_histories_reopen_relocate_and_export_without_rewriting_checkpoints() {
    use sha2::{Digest, Sha256};
    use studio_engine::{OpenSession, ProjectState, journal::Journal, store::Record};
    use studio_project::SourceRevision;

    for include_executable in [false, true] {
        for relocate in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("video");
            let paths = AppPaths::new(temp.path().join("history")).unwrap();
            let compatibility = CompatibilityManifest::default_linux_x64();
            let project = studio_project::create(
                &root,
                "Video",
                sdk_pin(&compatibility),
                &compatibility.fframes_version,
                "0.1.0",
            )
            .unwrap();
            let controller = Controller::open(&root, &paths).unwrap();
            let inventory = controller
                .checkpoints
                .load(controller.state().accepted())
                .unwrap();
            assert!(inventory.files.iter().all(|file| !file.executable));
            // Reconstruct the recorded wire format, not a current capture's revision.
            let mut hash = Sha256::new();
            hash.update(b"studio-source-v1\0");
            for file in &inventory.files {
                hash.update((file.path.as_str().len() as u64).to_le_bytes());
                hash.update(file.path.as_str().as_bytes());
                hash.update([file.kind as u8]);
                hash.update(file.size.to_le_bytes());
                if include_executable {
                    hash.update([0]);
                }
                let bytes = fs::read(root.join(file.path.as_str())).unwrap();
                hash.update(Sha256::digest(bytes));
            }
            let old = SourceRevision::try_from(format!("{:x}", hash.finalize())).unwrap();
            let mut manifest = serde_json::to_value(&inventory).unwrap();
            manifest.as_object_mut().unwrap().remove("version");
            manifest["revision"] = old.as_str().into();
            if !include_executable {
                for file in manifest["files"].as_array_mut().unwrap() {
                    file.as_object_mut().unwrap().remove("executable");
                }
            }
            let manifest_path = controller.checkpoints.manifest_path(&old);
            let bytes = serde_json::to_vec(&manifest).unwrap();
            fs::write(&manifest_path, &bytes).unwrap();
            drop(controller);
            let mut state = ProjectState::opening(
                project.manifest.project_id.clone(),
                OpenSession::new(),
                old.clone(),
                old.clone(),
            );
            state.finish_open(Ok(())).unwrap();
            state.close();
            let record = Record {
                location: project.root.clone(),
                name: "Video".into(),
                state,
                draft: None,
                sdk_path: None,
            };
            let (mut journal, _) = Journal::open(
                paths
                    .project(&project.manifest.project_id)
                    .join("lifecycle.jsonl"),
            )
            .unwrap();
            let tx = journal.intent(&record).unwrap();
            journal.commit(&tx).unwrap();
            Store::open(&paths.database())
                .unwrap()
                .save(&record)
                .unwrap();
            drop(journal);
            let opened = if relocate {
                let moved = temp.path().join("moved");
                fs::rename(&root, &moved).unwrap();
                moved
            } else {
                root
            };
            let mut controller = Controller::open(&opened, &paths).unwrap();
            assert!(controller.history_was_available);
            assert_eq!(controller.state().accepted(), &old);
            let exported = temp.path().join("export");
            controller.export_checkpoint(&old, &exported).unwrap();
            assert_eq!(
                fs::read(exported.join("src/lib.rs")).unwrap(),
                fs::read(opened.join("src/lib.rs")).unwrap()
            );
            assert_ne!(
                studio_project::open(&exported).unwrap().manifest.project_id,
                project.manifest.project_id
            );
            assert_eq!(fs::read(&manifest_path).unwrap(), bytes);
            controller.close().unwrap();
            drop(controller);
            let reopened = Controller::open(&opened, &paths).unwrap();
            assert_eq!(reopened.state().accepted(), &old);
            assert_eq!(fs::read(&manifest_path).unwrap(), bytes);
        }
    }
}
