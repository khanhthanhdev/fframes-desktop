use fframes_studio::worker_project::launch_portable_worker;
use std::{fs, path::PathBuf, time::Duration};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::build_materialization::sdk_pin;
use studio_sdk::{CompatibilityManifest, SdkInstaller};

#[test]
#[ignore = "requires SDK_BUNDLE containing assembled native SDK artifacts"]
fn portable_project_builds_after_source_and_sdk_relocation() {
    let bundle = PathBuf::from(std::env::var_os("SDK_BUNDLE").expect("SDK_BUNDLE"));
    let compatibility = CompatibilityManifest::from_json_str(
        &fs::read_to_string(bundle.join("compatibility.json")).unwrap(),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let artifacts: Vec<_> = compatibility
        .artifacts
        .iter()
        .map(|a| (a.clone(), bundle.join(a.url.trim_start_matches("file://"))))
        .collect();
    let sdk = SdkInstaller::new(temp.path().join("sdk"))
        .install_from_local_artifacts(&compatibility, &artifacts)
        .unwrap();
    let root = temp.path().join("portable");
    let project = studio_project::create(
        &root,
        "Video",
        sdk_pin(&compatibility),
        &compatibility.fframes_version,
        "0.1.0",
    )
    .unwrap();
    let original = temp.path().join("test.txt");
    fs::write(&original, b"copied").unwrap();
    let project = studio_project::assets::copy_asset(&project, &original).unwrap();
    fs::remove_file(original).unwrap();
    let moved = temp.path().join("relocated video");
    fs::rename(root, &moved).unwrap();
    let moved_sdk = temp.path().join("relocated-sdk");
    fs::rename(&sdk, &moved_sdk).unwrap();
    let project_after = studio_project::open(&moved).unwrap();
    assert_eq!(project_after.inventory, project.inventory);
    let manager = ProcessTreeManager::new();
    let mut worker = launch_portable_worker(
        &project_after,
        &moved_sdk,
        compatibility.clone(),
        &temp.path().join("builds"),
        41,
        &manager,
    )
    .unwrap();
    worker.request_render_frame(0).unwrap();
    assert_eq!(worker.latest_pixels().unwrap().len(), 1920 * 1080 * 4);
    assert_eq!(&worker.latest_pixels().unwrap()[..4], &[13, 17, 23, 255]);
    assert_eq!(worker.latest_header().unwrap().worker_generation, 41);
    assert_eq!(
        studio_project::open(&moved).unwrap().inventory,
        project.inventory
    );
    drop(worker);
    manager.terminate_all(Duration::from_millis(300));
    assert_eq!(manager.active_count(), 0);

    // Import a contained workspace with both runtime package-relative media and
    // compile-time embedded media. Runtime and Cargo require different cwd roots.
    let workspace = temp.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let member = workspace.join("video");
    fs::rename(&moved, &member).unwrap();
    fs::remove_file(member.join("studio.json")).unwrap();
    let cargo = fs::read_to_string(member.join("Cargo.toml")).unwrap();
    fs::write(member.join("Cargo.toml"), cargo.replace("[workspace]", "")).unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        "[workspace]\nmembers=['video']\nresolver='3'\n",
    )
    .unwrap();
    let worker_path = member.join("src/bin/studio_worker.rs");
    let source = fs::read_to_string(&worker_path).unwrap();
    let source = source.replace(
        "let directory = fframes::MediaDirectory::read_folder(\"media\")?;\n    let media = directory.process_media_source()?;",
        "let directory = fframes::MediaDirectory::read_folder(\"media\")?;\n    let _runtime_media = directory.process_media_source()?;\n    assert_eq!(std::fs::read(\"media/test.txt\")?, b\"copied\");\n    let media = WorkspaceMedia::prepare()?;",
    );
    assert!(source.contains("WorkspaceMedia::prepare"));
    assert!(source.contains("MediaDirectory::read_folder(\"media\")"));
    assert!(source.contains("std::fs::read(\"media/test.txt\")"));
    // The compile-time macro accepts media types only; keep licenses and the
    // copied text asset in media while embedding a separate font-only folder.
    fs::create_dir(member.join("static-media")).unwrap();
    fs::copy(
        member.join("media/DMSans-Medium.ttf"),
        member.join("static-media/DMSans-Medium.ttf"),
    )
    .unwrap();
    fs::write(
        &worker_path,
        format!(
            "use fframes::StaticMediaProvider;\nfframes::include_media_dir!(struct WorkspaceMedia, \"video/static-media\");\n{source}"
        ),
    )
    .unwrap();
    let project = studio_project::import(
        &workspace,
        studio_project::manifest::CargoEntry {
            manifest: "video/Cargo.toml".to_owned().try_into().unwrap(),
            package: "studio-video".into(),
            worker_target: "studio_worker".into(),
        },
        sdk_pin(&compatibility),
    )
    .unwrap();
    let mut worker = launch_portable_worker(
        &project,
        &moved_sdk,
        compatibility,
        &temp.path().join("builds"),
        42,
        &manager,
    )
    .unwrap();
    worker.request_render_frame(0).unwrap();
    assert_eq!(&worker.latest_pixels().unwrap()[..4], &[13, 17, 23, 255]);
    assert_eq!(worker.latest_header().unwrap().worker_generation, 42);
    assert_eq!(
        studio_project::open(&workspace).unwrap().inventory,
        project.inventory
    );
    drop(worker);
    manager.terminate_all(Duration::from_millis(300));
    assert_eq!(manager.active_count(), 0);
}
