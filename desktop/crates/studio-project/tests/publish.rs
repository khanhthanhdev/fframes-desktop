//! Linux no-clobber publication primitives and exact transaction-name exclusion.
#![cfg(target_os = "linux")]

use std::{fs, io::ErrorKind, os::unix::fs::symlink};
use studio_project::{
    SourceInventory,
    paths::{TxRole, is_transaction_internal_name, transaction_file_name},
    publish::{Dir, NoClobber, probe_no_clobber},
};

fn tx() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[test]
fn the_probe_proves_no_clobber_publication_on_this_filesystem_and_cleans_up() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("user.txt"), "mine").unwrap();
    let dir = Dir::open_root(temp.path()).unwrap();
    let mechanism = probe_no_clobber(&dir).unwrap();
    // The probe offers exactly one mechanism for live names.
    assert_eq!(
        mechanism,
        NoClobber::Renameat2,
        "only an atomic no-replace rename qualifies; a link fallback is never offered"
    );
    // Recorded in the hand-off: which primitive this machine's filesystem proved.
    eprintln!("proved no-clobber mechanism: {mechanism:?}");
    let left: Vec<_> = fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["user.txt"]);
}

#[test]
fn both_mechanisms_refuse_to_replace_and_leave_both_files_intact() {
    for mechanism in [NoClobber::Renameat2, NoClobber::Link] {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("staged"), "new bytes").unwrap();
        fs::write(temp.path().join("destination"), "editor bytes").unwrap();
        let dir = Dir::open_root(temp.path()).unwrap();
        let error = dir
            .rename_noreplace(mechanism, "staged", &dir, "destination")
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists, "{mechanism:?}");
        assert_eq!(
            fs::read_to_string(temp.path().join("destination")).unwrap(),
            "editor bytes"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("staged")).unwrap(),
            "new bytes"
        );
        // A free name works and moves the inode.
        let before = dir.stat("staged").unwrap().unwrap().id;
        dir.rename_noreplace(mechanism, "staged", &dir, "free")
            .unwrap();
        assert_eq!(dir.stat("free").unwrap().unwrap().id, before);
        assert!(dir.stat("staged").unwrap().is_none());
    }
}

#[test]
fn names_never_follow_links_or_open_special_files() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), "outside").unwrap();
    symlink(outside.path(), temp.path().join("link-dir")).unwrap();
    symlink(outside.path().join("secret"), temp.path().join("link-file")).unwrap();
    fs::create_dir(temp.path().join("real")).unwrap();
    let fifo = std::process::Command::new("mkfifo")
        .arg(temp.path().join("pipe"))
        .status()
        .unwrap();
    assert!(fifo.success());
    let root = Dir::open_root(temp.path()).unwrap();
    assert!(root.open_relative("link-dir").is_err());
    assert!(root.open_relative("real").is_ok());
    assert!(root.open_regular("link-file").is_err());
    assert!(root.open_regular("pipe").is_err());
    assert!(root.open_regular("real").is_err());
    // Creation refuses an existing name, including a dangling or live link.
    assert_eq!(
        root.create_new("link-file", 0o600).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    symlink("nowhere", temp.path().join("dangling")).unwrap();
    assert!(root.create_new("dangling", 0o600).is_err());
    assert!(!temp.path().join("nowhere").exists());
    // Components must be single names.
    assert!(root.stat("real/inner").is_err());
    assert!(root.stat("..").is_err());
    assert_eq!(
        fs::read_to_string(outside.path().join("secret")).unwrap(),
        "outside"
    );
}

#[test]
fn a_swapped_directory_is_detected_by_identity() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("sub")).unwrap();
    let root = Dir::open_root(temp.path()).unwrap();
    let sub = root.open_relative("sub").unwrap();
    assert!(sub.is_still_at(&root, "sub"));
    fs::rename(temp.path().join("sub"), temp.path().join("moved")).unwrap();
    fs::create_dir(temp.path().join("sub")).unwrap();
    assert!(
        !sub.is_still_at(&root, "sub"),
        "a replacement directory has another identity"
    );
    fs::remove_dir(temp.path().join("sub")).unwrap();
    symlink(temp.path().join("moved"), temp.path().join("sub")).unwrap();
    assert!(!sub.is_still_at(&root, "sub"), "a symlink never matches");
}

#[test]
fn created_files_carry_exactly_the_requested_mode_whatever_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = Dir::open_root(temp.path()).unwrap();
    for (name, mode) in [("a", 0o755), ("b", 0o600), ("c", 0o644)] {
        drop(root.create_new(name, mode).unwrap());
        assert_eq!(
            fs::metadata(temp.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            mode
        );
    }
}

#[test]
fn hashing_reports_identity_mode_and_detects_a_concurrent_in_place_change() {
    use std::os::unix::fs::{FileExt, PermissionsExt};
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("f");
    fs::write(&path, "abc").unwrap();
    let root = Dir::open_root(temp.path()).unwrap();
    let (stat, hash) = root.hash_regular("f").unwrap();
    assert_eq!(stat.size, 3);
    assert_eq!(
        hash,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(stat.id, root.stat("f").unwrap().unwrap().id);
    assert!(!stat.executable());
    // An uninterrupted second read agrees, so the check below is not a false positive.
    assert_eq!(root.hash_regular("f").unwrap().1, hash);

    // Backdate the modification time: a write that lands within the filesystem's
    // timestamp granularity must still be told apart from the original state.
    let backdate = || {
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000))
            .unwrap();
    };

    // A same-length write between the last byte read and the final fstat.
    backdate();
    let error = root
        .hash_regular_observed("f", || {
            fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .write_at(b"xyz", 0)
                .unwrap();
        })
        .unwrap_err();
    assert!(error.to_string().contains("changed while"), "{error}");

    // A write that changes the length.
    fs::write(&path, "abc").unwrap();
    backdate();
    let error = root
        .hash_regular_observed("f", || {
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap()
                .write_all_at(b"d", 3)
                .unwrap();
        })
        .unwrap_err();
    assert!(error.to_string().contains("changed while"), "{error}");

    // A mode change with identical bytes.
    fs::write(&path, "abc").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let error = root
        .hash_regular_observed("f", || {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        })
        .unwrap_err();
    assert!(error.to_string().contains("changed while"), "{error}");

    // After the interleavings stop, the file hashes again and reports its new mode.
    let (stat, _) = root.hash_regular("f").unwrap();
    assert_eq!(stat.mode, 0o755);
    assert!(stat.executable());
}

#[test]
fn a_root_renamed_away_and_replaced_by_a_link_fails_the_pathname_binding() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let root = Dir::open_root(&project).unwrap();
    root.check_root_binding().unwrap();
    // A clone and a relative open keep (or drop) the binding consistently.
    root.try_clone().unwrap().check_root_binding().unwrap();
    fs::rename(&project, temp.path().join("moved")).unwrap();
    assert!(
        root.check_root_binding().is_err(),
        "a missing root pathname is not the opened directory"
    );
    symlink(temp.path().join("moved"), &project).unwrap();
    let error = root.check_root_binding().unwrap_err();
    assert!(
        error.contains("link") || error.contains("replaced"),
        "{error}"
    );
    // The held descriptor still reaches the real directory: the swap is only visible
    // through the pathname, which is exactly what the binding re-checks.
    assert!(root.is_still_at(&root, ""));
}

#[test]
fn a_swapped_ancestor_of_the_root_fails_the_pathname_binding() {
    let temp = tempfile::tempdir().unwrap();
    let outer = temp.path().join("outer");
    fs::create_dir_all(outer.join("project")).unwrap();
    let root = Dir::open_root(&outer.join("project")).unwrap();
    root.check_root_binding().unwrap();
    fs::rename(&outer, temp.path().join("outer-moved")).unwrap();
    // A look-alike tree appears under the old ancestor name.
    fs::create_dir_all(outer.join("project")).unwrap();
    assert!(
        root.check_root_binding().is_err(),
        "same names through a different ancestor are another directory"
    );
}

#[test]
fn link_then_unlink_loses_an_editor_save_between_the_two_syscalls() {
    use std::cell::Cell;
    let temp = tempfile::tempdir().unwrap();
    let dir = Dir::open_root(temp.path()).unwrap();
    let tag = tx();
    let orig = transaction_file_name(&tag, 0, TxRole::Original);
    let editor_saved = |content: &str| {
        // An editor saves by writing a temporary file and renaming it over the name.
        fs::write(temp.path().join(".editor.tmp"), content).unwrap();
        fs::rename(temp.path().join(".editor.tmp"), temp.path().join("old.txt")).unwrap();
    };

    // Link based: the editor's rename lands after `linkat` and before `unlinkat`.
    fs::write(temp.path().join("old.txt"), "original A").unwrap();
    let window_opened = Cell::new(false);
    dir.rename_noreplace_observed(NoClobber::Link, "old.txt", &dir, &orig, || {
        window_opened.set(true);
        editor_saved("editor B");
    })
    .unwrap();
    assert!(window_opened.get());
    // The slot verifies perfectly (it holds A), yet the editor's file is gone: the
    // `unlinkat` removed B. This is why links never move a live source name.
    assert_eq!(fs::read(temp.path().join(&orig)).unwrap(), b"original A");
    assert!(
        !temp.path().join("old.txt").exists(),
        "editor B was silently unlinked"
    );

    // Atomic rename: there is no window. The editor saving after the move simply
    // creates its file; nothing of it can be lost.
    fs::remove_file(temp.path().join(&orig)).unwrap();
    fs::write(temp.path().join("old.txt"), "original A").unwrap();
    let window_opened = Cell::new(false);
    dir.rename_noreplace_observed(NoClobber::Renameat2, "old.txt", &dir, &orig, || {
        window_opened.set(true);
    })
    .unwrap();
    assert!(!window_opened.get(), "renameat2 has no two-syscall window");
    editor_saved("editor B");
    assert_eq!(fs::read(temp.path().join(&orig)).unwrap(), b"original A");
    assert_eq!(fs::read(temp.path().join("old.txt")).unwrap(), b"editor B");
}

#[test]
fn a_crash_inside_the_link_pair_leaves_both_names_and_never_loses_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Dir::open_root(temp.path()).unwrap();
    let tag = tx();
    let orig = transaction_file_name(&tag, 0, TxRole::Original);
    fs::write(temp.path().join("old.txt"), "original A").unwrap();
    // Partial success: only the first syscall ran before the process died.
    dir.link_noreplace("old.txt", &dir, &orig).unwrap();
    let source = dir.stat("old.txt").unwrap().unwrap();
    let slot = dir.stat(&orig).unwrap().unwrap();
    assert_eq!(source.id, slot.id);
    assert_eq!(source.nlink, 2);
    assert_eq!(
        fs::read(temp.path().join("old.txt")).unwrap(),
        b"original A"
    );
    // Linking onto an existing name refuses and changes nothing.
    let error = dir.link_noreplace("old.txt", &dir, &orig).unwrap_err();
    assert_eq!(error.kind(), ErrorKind::AlreadyExists);
}

#[test]
fn only_exact_app_generated_names_are_internal() {
    let id = tx();
    for role in [
        TxRole::Stage,
        TxRole::Original,
        TxRole::Rollback,
        TxRole::Variant(2),
    ] {
        let name = transaction_file_name(&id, 17, role);
        assert!(is_transaction_internal_name(&name), "{name}");
    }
    for name in [
        ".fframes-tx-".to_owned(),
        format!(".fframes-tx-{id}-0"),
        format!(".fframes-tx-{id}-0.stage.bak"),
        format!(".fframes-tx-{id}-01.stage"),
        format!(".fframes-tx-{id}--1.stage"),
        format!(".fframes-tx-{id}-0.other"),
        format!(".fframes-tx-{id}-0.var"),
        format!(".fframes-tx-{}-0.stage", id.to_uppercase()),
        format!(".fframes-tx-{}-0.stage", &id[..31]),
        format!("x.fframes-tx-{id}-0.stage"),
        "notes.stage".to_owned(),
    ] {
        assert!(!is_transaction_internal_name(&name), "{name}");
    }
}

#[test]
fn the_source_inventory_excludes_exact_internal_files_but_never_user_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("a.txt"), "a").unwrap();
    let plain = SourceInventory::scan(root).unwrap();
    let id = tx();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(
        root.join(transaction_file_name(&id, 0, TxRole::Stage)),
        "staged",
    )
    .unwrap();
    fs::write(
        root.join("sub")
            .join(transaction_file_name(&id, 3, TxRole::Original)),
        "orig",
    )
    .unwrap();
    fs::write(
        root.join(transaction_file_name(&id, 1, TxRole::Variant(1))),
        "variant",
    )
    .unwrap();
    let with_internal = SourceInventory::scan(root).unwrap();
    assert_eq!(with_internal.revision, plain.revision);
    assert_eq!(with_internal.files, plain.files);
    // Similar-looking filenames are user content.
    fs::write(root.join(".fframes-tx-mine.stage"), "user").unwrap();
    fs::write(root.join(format!(".fframes-tx-{id}-0.stage.bak")), "user").unwrap();
    let with_user = SourceInventory::scan(root).unwrap();
    assert_ne!(with_user.revision, plain.revision);
    let names: Vec<_> = with_user.files.iter().map(|f| f.path.as_str()).collect();
    assert!(names.contains(&".fframes-tx-mine.stage"));
    assert!(names.iter().any(|n| n.ends_with(".stage.bak")));
    // An internal-looking name that is not a regular file is still refused.
    let other = tempfile::tempdir().unwrap();
    fs::write(other.path().join("a.txt"), "a").unwrap();
    symlink(
        "a.txt",
        other
            .path()
            .join(transaction_file_name(&id, 9, TxRole::Stage)),
    )
    .unwrap();
    assert!(SourceInventory::scan(other.path()).is_err());
}
