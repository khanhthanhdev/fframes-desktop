use std::{fs, path::Path};
use studio_project::{
    SourceInventory, SourceRevision,
    checkpoint::{Checkpoints, copy_draft},
};
fn write(root: &Path, name: &str, bytes: &[u8]) {
    let p = root.join(name);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}
#[test]
fn deterministic_relocation_order_and_exact_exclusions() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let files = [
        ("Cargo.toml", b"cargo".as_slice()),
        ("Cargo.lock", b"lock"),
        ("src/lib.rs", b"rust"),
        ("media/a.png", b"image"),
        ("style/tokens.json", b"style"),
        ("AGENTS.md", b"instructions"),
        ("nested/frames/a", b"real source"),
        ("nested/target/a", b"also source"),
        ("studio.json", b"metadata"),
    ];
    for (name, bytes) in files {
        write(a.path(), name, bytes);
    }
    for (name, bytes) in files.into_iter().rev() {
        write(b.path(), name, bytes);
    }
    let before = SourceInventory::scan(a.path()).unwrap();
    assert_eq!(before, SourceInventory::scan(b.path()).unwrap());
    for name in [
        ".git/objects/a",
        "target/a",
        ".fframes/cache/a",
        ".fframes/context/a",
    ] {
        write(a.path(), name, b"cache");
    }
    assert_eq!(before, SourceInventory::scan(a.path()).unwrap());
    for (name, bytes) in files {
        write(a.path(), name, b"edited");
        assert_ne!(
            before.revision,
            SourceInventory::scan(a.path()).unwrap().revision,
            "{name}"
        );
        write(a.path(), name, bytes);
    }
    write(a.path(), "nested/frames/new", b"new");
    assert_ne!(
        before.revision,
        SourceInventory::scan(a.path()).unwrap().revision
    );
}
#[test]
fn large_asset_is_streamed_and_rename_affects_identity() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "media/large", &vec![42; 2 * 1024 * 1024 + 7]);
    let a = SourceInventory::scan(d.path()).unwrap();
    assert_eq!(a.files[0].size, 2 * 1024 * 1024 + 7);
    fs::rename(d.path().join("media/large"), d.path().join("media/renamed")).unwrap();
    assert_ne!(
        a.revision,
        SourceInventory::scan(d.path()).unwrap().revision
    );
}

#[test]
fn versioned_and_legacy_hashes_match_independent_expected_values() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a.txt", b"abc");
    let inventory = SourceInventory::scan(d.path()).unwrap();
    assert_eq!(inventory.version, 1);
    assert_eq!(
        inventory.revision.as_str(),
        "624621a41f15c14522fc3612b9a47faa1eea64981d5cf6fd69787d59f47dae07"
    );
    for expected in [
        "2e6787042becae58ce66b2ec3192b7239a2ff4b6f3832c2f051af690cc920d80",
        "624621a41f15c14522fc3612b9a47faa1eea64981d5cf6fd69787d59f47dae07",
    ] {
        let revision = SourceRevision::try_from(expected.to_owned()).unwrap();
        assert!(inventory.matches_revision(&revision));
    }
}

fn install_legacy_checkpoint(store: &Checkpoints, revision: &str, executable_field: Option<bool>) {
    let digest = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    fs::write(store.root.join("objects").join(digest), b"abc").unwrap();
    let mut file = serde_json::json!({
        "path": "a.txt", "kind": "Other", "size": 3, "sha256": digest
    });
    if let Some(executable) = executable_field {
        file["executable"] = executable.into();
    }
    let manifest = serde_json::json!({"files": [file], "revision": revision});
    fs::write(
        store
            .root
            .join("checkpoints")
            .join(format!("{revision}.json")),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn old_checkpoint_loads_and_drafts_without_rewriting_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let store = Checkpoints::new(temp.path()).unwrap();
    for (index, (old, executable)) in [
        (
            "2e6787042becae58ce66b2ec3192b7239a2ff4b6f3832c2f051af690cc920d80",
            None,
        ),
        (
            "624621a41f15c14522fc3612b9a47faa1eea64981d5cf6fd69787d59f47dae07",
            Some(false),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        install_legacy_checkpoint(&store, old, executable);
        let revision = SourceRevision::try_from(old.to_owned()).unwrap();
        let manifest = store.manifest_path(&revision);
        let before = fs::read(&manifest).unwrap();
        assert_eq!(store.load(&revision).unwrap().version, 0);
        let draft = store
            .draft(&revision, &format!("restored-{index}"))
            .unwrap();
        assert_eq!(fs::read(draft.join("a.txt")).unwrap(), b"abc");
        assert_eq!(fs::read(manifest).unwrap(), before);
    }
}

#[test]
fn checkpoint_versions_and_unprotected_executable_metadata_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let store = Checkpoints::new(temp.path()).unwrap();
    let old = "2e6787042becae58ce66b2ec3192b7239a2ff4b6f3832c2f051af690cc920d80";
    install_legacy_checkpoint(&store, old, Some(true));
    let revision = SourceRevision::try_from(old.to_owned()).unwrap();
    assert!(store.load(&revision).is_err());

    let current = "624621a41f15c14522fc3612b9a47faa1eea64981d5cf6fd69787d59f47dae07";
    install_legacy_checkpoint(&store, current, Some(false));
    let path = store
        .root
        .join("checkpoints")
        .join(format!("{current}.json"));
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["version"] = 999.into();
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(
        store
            .load(&SourceRevision::try_from(current.to_owned()).unwrap())
            .unwrap_err()
            .reason
            .contains("unsupported inventory version")
    );
}

#[test]
fn current_checkpoint_load_draft_and_copy_preserve_content() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    write(&source, "src/lib.rs", b"pub fn video() {}\n");
    let store = Checkpoints::new(&temp.path().join("store")).unwrap();
    let revision = store.capture(&source).unwrap();
    assert_eq!(store.load(&revision).unwrap().version, 1);
    let draft = store.draft(&revision, "draft").unwrap();
    let copied = temp.path().join("copied");
    copy_draft(&draft, &copied).unwrap();
    assert_eq!(
        SourceInventory::scan(&copied).unwrap(),
        SourceInventory::scan(&source).unwrap()
    );
}
#[cfg(unix)]
#[test]
fn links_special_files_and_non_utf8_names_are_errors() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "src/lib.rs", b"rust");
    symlink("src", d.path().join("linked")).unwrap();
    assert!(
        SourceInventory::scan(d.path())
            .unwrap_err()
            .reason
            .contains("unsupported link")
    );
    fs::remove_file(d.path().join("linked")).unwrap();
    let socket = std::os::unix::net::UnixListener::bind(d.path().join("socket")).unwrap();
    assert!(
        SourceInventory::scan(d.path())
            .unwrap_err()
            .reason
            .contains("unsupported special file")
    );
    drop(socket);
    fs::remove_file(d.path().join("socket")).unwrap();
    fs::write(
        d.path().join(std::ffi::OsString::from_vec(vec![0xff])),
        b"bad",
    )
    .unwrap();
    assert!(
        SourceInventory::scan(d.path())
            .unwrap_err()
            .reason
            .contains("non-UTF-8")
    );
}

#[cfg(unix)]
#[test]
fn executable_permission_changes_revision_and_is_restored_by_draft() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("proj");
    fs::create_dir(&root).unwrap();
    let script = root.join("run.sh");
    fs::write(&script, b"#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o644)).unwrap();
    let non_exec_inv = SourceInventory::scan(&root).unwrap();
    assert!(!non_exec_inv.files[0].executable);

    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let exec_inv = SourceInventory::scan(&root).unwrap();
    assert!(exec_inv.files[0].executable);
    assert_ne!(non_exec_inv.revision, exec_inv.revision);
    assert_eq!(
        exec_inv.revision.as_str(),
        "2567cbb748e86d438aef4506b3d52a70ea1cf413aad4eb3f2cd71d47e892aa7b"
    );
    let old_unprotected = SourceRevision::try_from(
        "b62c467bdb49b455d1bfa6b7043c8e593f6a84ed9c0812035f527b24bd6d56a0".to_owned(),
    )
    .unwrap();
    assert!(!exec_inv.matches_revision(&old_unprotected));

    let checkpoints_dir = temp.path().join("checkpoints");
    let checkpoints = studio_project::checkpoint::Checkpoints::new(&checkpoints_dir).unwrap();
    let rev = checkpoints.capture(&root).unwrap();
    let draft = checkpoints.draft(&rev, "draft-1").unwrap();
    let restored_script = draft.join("run.sh");
    let mode = fs::metadata(&restored_script).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
}
