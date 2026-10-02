use std::fs;
use studio_project::{assets::copy_asset, create, manifest::SdkPin};

#[test]
fn collisions_and_relocation_keep_owned_copies() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let project = create(
        &root,
        "Video",
        SdkPin {
            release: "v1".into(),
            compatibility_sha256: "0".repeat(64),
        },
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let original = temp.path().join("image.png");
    fs::write(&original, b"first-image").unwrap();
    let project = copy_asset(&project, &original).unwrap();
    fs::write(&original, b"second-image").unwrap();
    let project = copy_asset(&project, &original).unwrap();
    fs::remove_file(original).unwrap();
    assert_eq!(
        fs::read(root.join("media/image.png")).unwrap(),
        b"first-image"
    );
    assert_eq!(
        fs::read(root.join("media/1-image.png")).unwrap(),
        b"second-image"
    );
    let moved = temp.path().join("moved");
    fs::rename(root, &moved).unwrap();
    assert_eq!(
        studio_project::open(&moved).unwrap().inventory,
        project.inventory
    );
}

#[cfg(unix)]
#[test]
fn symlink_and_failed_manifest_do_not_destroy_existing_asset() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let project = create(
        &root,
        "Video",
        SdkPin {
            release: "v1".into(),
            compatibility_sha256: "0".repeat(64),
        },
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let original = temp.path().join("real.png");
    fs::write(&original, b"asset").unwrap();
    let link = temp.path().join("link.png");
    std::os::unix::fs::symlink(&original, &link).unwrap();
    assert!(copy_asset(&project, &link).is_err());
    fs::write(root.join("src/lib.rs"), "// external edit").unwrap();
    assert!(copy_asset(&project, &original).is_err());
    assert!(!root.join("media/real.png").exists());
}

#[test]
fn external_manifest_edit_during_stream_is_preserved_without_publication() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let project = create(
        &root,
        "Video",
        SdkPin {
            release: "v1".into(),
            compatibility_sha256: "0".repeat(64),
        },
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let original = temp.path().join("asset.png");
    fs::write(&original, b"asset bytes").unwrap();
    let mut external = project.manifest.clone();
    external.display.name = "Externally renamed".into();
    let bytes = serde_json::to_vec_pretty(&external).unwrap();
    let result = studio_project::assets::copy_asset_observed(&project, &original, || {
        fs::write(root.join("studio.json"), &bytes).unwrap();
    });
    assert!(result.unwrap_err().reason.contains("source changed"));
    assert_eq!(fs::read(root.join("studio.json")).unwrap(), bytes);
    assert!(!root.join("media/asset.png").exists());
    assert_eq!(fs::read_dir(root.join("media")).unwrap().count(), 2);
}

#[cfg(unix)]
#[test]
fn media_directory_replacement_during_copy_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let project = create(
        &root,
        "Video",
        SdkPin {
            release: "v1".into(),
            compatibility_sha256: "0".repeat(64),
        },
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let original = temp.path().join("asset.png");
    fs::write(&original, b"asset bytes").unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    assert!(
        studio_project::assets::copy_asset_observed(&project, &original, || {
            fs::rename(root.join("media"), root.join("old-media")).unwrap();
            std::os::unix::fs::symlink(&outside, root.join("media")).unwrap();
        })
        .is_err()
    );
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    assert_eq!(
        fs::read(root.join("studio.json")).unwrap(),
        serde_json::to_vec_pretty(&project.manifest).unwrap()
    );
}

#[test]
fn cancelled_chunked_copy_leaves_manifest_and_source_exact() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let project = create(
        &root,
        "Video",
        SdkPin {
            release: "v1".into(),
            compatibility_sha256: "0".repeat(64),
        },
        "1.1.0",
        "0.1.0",
    )
    .unwrap();
    let original = temp.path().join("large.png");
    let bytes = vec![79; 200 * 1024];
    fs::write(&original, &bytes).unwrap();
    let checks = std::cell::Cell::new(0);
    let result = studio_project::assets::copy_asset_with_cancel(&project, &original, || {
        checks.set(checks.get() + 1);
        checks.get() > 3
    });
    assert!(result.unwrap_err().reason.contains("cancelled"));
    assert_eq!(
        studio_project::open(&root).unwrap().inventory,
        project.inventory
    );
    assert_eq!(fs::read(original).unwrap(), bytes);
}
