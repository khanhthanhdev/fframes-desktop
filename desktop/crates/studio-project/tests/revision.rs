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
fn the_inventory_hash_matches_an_independent_expected_value() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a.txt", b"abc");
    let inventory = SourceInventory::scan(d.path()).unwrap();
    assert_eq!(inventory.version, 2);
    assert_eq!(
        inventory.revision.as_str(),
        "9d3c68c820e87abb567ed8cd620fa9a00e17106492743040d80732df36699184"
    );
    assert!(inventory.matches_revision(&inventory.revision));
    // The hashes of the two earlier formats (without the executable bit, and with it)
    // no longer name this source.
    for earlier in [
        "2e6787042becae58ce66b2ec3192b7239a2ff4b6f3832c2f051af690cc920d80",
        "624621a41f15c14522fc3612b9a47faa1eea64981d5cf6fd69787d59f47dae07",
    ] {
        let revision = SourceRevision::try_from(earlier.to_owned()).unwrap();
        assert!(!inventory.matches_revision(&revision));
    }
}

const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

/// Writes a checkpoint manifest (and its object) the way an older Studio did. `version`
/// `None` is the unversioned format, which also had no `executable` field.
fn install_old_checkpoint(store: &Checkpoints, revision: &str, version: Option<u32>) -> Vec<u8> {
    fs::write(store.root.join("objects").join(ABC), b"abc").unwrap();
    let mut file = serde_json::json!({
        "path": "a.txt", "kind": "Other", "size": 3, "sha256": ABC
    });
    let mut manifest = serde_json::json!({"files": [], "revision": revision});
    if let Some(version) = version {
        file["executable"] = false.into();
        manifest["version"] = version.into();
    }
    manifest["files"] = serde_json::json!([file]);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(
        store
            .root
            .join("checkpoints")
            .join(format!("{revision}.json")),
        &bytes,
    )
    .unwrap();
    bytes
}

fn listing(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            out.push(path.to_string_lossy().into_owned());
            if path.is_dir() {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

#[test]
fn a_checkpoint_of_an_older_inventory_format_is_refused_and_left_as_it_is() {
    // Unversioned (the two historical hash forms) and version 1 (before the timing
    // document had its own kind).
    for (revision, version) in [
        (
            "2e6787042becae58ce66b2ec3192b7239a2ff4b6f3832c2f051af690cc920d80",
            None,
        ),
        (
            "624621a41f15c14522fc3612b9a47faa1eea64981d5cf6fd69787d59f47dae07",
            None,
        ),
        (
            "624621a41f15c14522fc3612b9a47faa1eea64981d5cf6fd69787d59f47dae07",
            Some(1),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = Checkpoints::new(temp.path()).unwrap();
        let bytes = install_old_checkpoint(&store, revision, version);
        let revision = SourceRevision::try_from(revision.to_owned()).unwrap();
        let manifest = store.manifest_path(&revision);
        let files = listing(temp.path());
        for error in [
            store.load(&revision).unwrap_err(),
            store.verified_inventory(&revision).unwrap_err(),
            store.draft(&revision, "restored").unwrap_err(),
        ] {
            let message = error.to_string();
            assert!(
                message.contains(&manifest.to_string_lossy().into_owned()),
                "{message}"
            );
            assert!(message.contains("older Studio"), "{message}");
        }
        assert_eq!(fs::read(&manifest).unwrap(), bytes, "{version:?}");
        // Nothing was recreated, discarded, quarantined or drafted.
        assert_eq!(listing(temp.path()), files, "{version:?}");
    }
}

#[test]
fn a_checkpoint_of_a_newer_inventory_format_is_refused_and_left_as_it_is() {
    let temp = tempfile::tempdir().unwrap();
    let store = Checkpoints::new(temp.path()).unwrap();
    let current = "624621a41f15c14522fc3612b9a47faa1eea64981d5cf6fd69787d59f47dae07";
    install_old_checkpoint(&store, current, Some(1));
    let path = store
        .root
        .join("checkpoints")
        .join(format!("{current}.json"));
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["version"] = 999.into();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(&path, &bytes).unwrap();
    let error = store
        .load(&SourceRevision::try_from(current.to_owned()).unwrap())
        .unwrap_err();
    assert!(error.reason.contains("newer Studio"), "{error}");
    assert_eq!(fs::read(&path).unwrap(), bytes);
}
#[test]
fn current_checkpoint_load_draft_and_copy_preserve_content() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    write(&source, "src/lib.rs", b"pub fn video() {}\n");
    let store = Checkpoints::new(&temp.path().join("store")).unwrap();
    let revision = store.capture(&source).unwrap();
    assert_eq!(store.load(&revision).unwrap().version, 2);
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
    use std::os::unix::fs::symlink;
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
    // APFS refuses names that are not UTF-8 (EILSEQ), so macOS cannot hold one.
    #[cfg(not(target_os = "macos"))]
    {
        use std::os::unix::ffi::OsStringExt;
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
        "d137e0a57dd9eb1f91167678549de1c95ccad49e808331117956e52d9bbc3699"
    );

    let checkpoints_dir = temp.path().join("checkpoints");
    let checkpoints = studio_project::checkpoint::Checkpoints::new(&checkpoints_dir).unwrap();
    let rev = checkpoints.capture(&root).unwrap();
    let draft = checkpoints.draft(&rev, "draft-1").unwrap();
    let restored_script = draft.join("run.sh");
    let mode = fs::metadata(&restored_script).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755);
}

#[test]
fn timing_document_changes_the_source_but_not_the_build_revision() {
    use studio_project::revision::{FileKind, TIMING_FILE};
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "src/lib.rs", b"rust");
    let without = SourceInventory::scan(temp.path()).unwrap();
    write(temp.path(), TIMING_FILE, br#"{"version":1,"clips":[]}"#);
    let first = SourceInventory::scan(temp.path()).unwrap();
    assert_eq!(first.timing_file().unwrap().kind, FileKind::Timing);
    write(
        temp.path(),
        TIMING_FILE,
        br#"{"version":1,"clips":[{"key":"a","trim_start_frames":1,"trim_end_frames":0}]}"#,
    );
    let second = SourceInventory::scan(temp.path()).unwrap();
    assert_ne!(first.revision, second.revision);
    assert_eq!(first.build_revision(), second.build_revision());
    assert_eq!(without.build_revision(), first.build_revision());
    assert_ne!(first.build_revision(), first.revision);
    // Only the root document is timing; a nested file of that name is ordinary source.
    write(temp.path(), "nested/timeline.json", b"{}");
    assert_ne!(
        SourceInventory::scan(temp.path()).unwrap().build_revision(),
        first.build_revision()
    );
}

#[test]
fn slides_document_changes_the_source_but_not_the_build_revision() {
    use studio_project::revision::{FileKind, SLIDES_FILE};
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), "src/lib.rs", b"rust");
    let without = SourceInventory::scan(temp.path()).unwrap();
    write(temp.path(), SLIDES_FILE, br#"{"version":1,"stops":[]}"#);
    let first = SourceInventory::scan(temp.path()).unwrap();
    assert_eq!(first.slides_file().unwrap().kind, FileKind::Other);
    write(
        temp.path(),
        SLIDES_FILE,
        br#"{"version":1,"stops":[{"scene":"a","at":"end"}]}"#,
    );
    let second = SourceInventory::scan(temp.path()).unwrap();
    assert_ne!(first.revision, second.revision);
    assert_eq!(first.build_revision(), second.build_revision());
    assert_eq!(without.build_revision(), first.build_revision());
    // Only the root document is run-time; a nested file of that name is ordinary source.
    write(temp.path(), "nested/slides.json", b"{}");
    assert_ne!(
        SourceInventory::scan(temp.path()).unwrap().build_revision(),
        first.build_revision()
    );
}

#[test]
fn the_timing_document_is_hashed_as_timing() {
    use studio_project::revision::{FileKind, TIMING_FILE};
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), TIMING_FILE, br#"{"version":1,"clips":[]}"#);
    let current = SourceInventory::scan(temp.path()).unwrap();
    // Independently computed over kind 7 (Timing).
    assert_eq!(
        current.revision.as_str(),
        "c460551dadb517407f1cecf1db4355b3aed76c445d96bc193293048fada0062c"
    );
    current.verify().unwrap();
    // An inventory that lists it as another kind is not what this Studio records.
    let mut recorded = current.clone();
    recorded.files[0].kind = FileKind::Other;
    assert!(recorded.verify().is_err());
    assert!(!recorded.matches_revision(&current.revision));
}
