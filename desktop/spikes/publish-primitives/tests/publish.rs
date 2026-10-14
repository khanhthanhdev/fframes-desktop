//! The five test kinds of `studio-project/tests/publish.rs`, ported to the Windows and
//! macOS primitives: no-replace, editor-save race, links, swapped directory, root binding.
#![cfg(any(windows, target_os = "macos"))]

use std::{cell::Cell, fs, io::ErrorKind, path::Path};

use publish_primitives::{Dir, EntryKind, probe_no_clobber};

fn symlink_dir(target: &Path, link: &Path) {
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_dir(target, link);
    result.unwrap_or_else(|e| {
        panic!("creating a directory symlink failed ({e}); on Windows this needs SeCreateSymbolicLinkPrivilege (Administrator or Developer Mode)")
    });
}

fn symlink_file(target: &Path, link: &Path) {
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_file(target, link);
    result.unwrap_or_else(|e| {
        panic!("creating a file symlink failed ({e}); on Windows this needs SeCreateSymbolicLinkPrivilege (Administrator or Developer Mode)")
    });
}

/// A directory junction (`mklink /J`), which needs no privilege.
#[cfg(windows)]
fn junction(target: &Path, link: &Path) {
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "mklink /J failed");
}

/// An editor saving by writing a temporary file and renaming it over the name
/// (`std::fs::rename` is `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` on Windows and
/// `rename(2)` on macOS).
fn editor_save(dir: &Path, name: &str, content: &str) -> std::io::Result<()> {
    let temp = dir.join(".editor.tmp");
    fs::write(&temp, content)?;
    fs::rename(&temp, dir.join(name))
}

// ---- no-replace -------------------------------------------------------------------

#[test]
fn the_probe_proves_no_clobber_publication_on_this_filesystem_and_cleans_up() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("user.txt"), "mine").unwrap();
    let dir = Dir::open_root(temp.path()).unwrap();
    probe_no_clobber(&dir).unwrap();
    let left: Vec<_> = fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["user.txt"]);
}

#[test]
fn a_no_replace_rename_refuses_an_existing_destination_and_moves_onto_a_free_name() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("sub")).unwrap();
    fs::write(temp.path().join("staged"), "new bytes").unwrap();
    fs::write(temp.path().join("destination"), "editor bytes").unwrap();
    fs::write(
        temp.path().join("sub").join("destination"),
        "other editor bytes",
    )
    .unwrap();
    let dir = Dir::open_root(temp.path()).unwrap();
    let sub = dir.open_child("sub").unwrap();
    for (target, label) in [(&dir, "same directory"), (&sub, "other directory")] {
        let error = dir
            .rename_noreplace("staged", target, "destination")
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists, "{label}: {error}");
    }
    assert_eq!(
        fs::read_to_string(temp.path().join("destination")).unwrap(),
        "editor bytes"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("sub").join("destination")).unwrap(),
        "other editor bytes"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("staged")).unwrap(),
        "new bytes"
    );
    // A free name in another directory works and moves the file itself.
    let before = dir.stat("staged").unwrap().unwrap();
    assert_eq!(before.kind, EntryKind::Regular);
    eprintln!("staged file id: {}", before.id);
    dir.rename_noreplace("staged", &sub, "free").unwrap();
    assert_eq!(sub.stat("free").unwrap().unwrap().id, before.id);
    assert!(dir.stat("staged").unwrap().is_none());
    assert_eq!(
        fs::read_to_string(temp.path().join("sub").join("free")).unwrap(),
        "new bytes"
    );
}

// ---- editor-save race -------------------------------------------------------------

#[test]
fn the_rename_moves_the_original_and_an_editor_save_after_it_survives() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Dir::open_root(temp.path()).unwrap();
    fs::write(temp.path().join("old.txt"), "original A").unwrap();
    let window_opened = Cell::new(false);
    dir.rename_noreplace_observed("old.txt", &dir, "orig.slot", || window_opened.set(true))
        .unwrap();
    // macOS: one syscall, no window. Windows: open-then-rename, see the next test.
    assert_eq!(window_opened.get(), cfg!(windows));
    editor_save(temp.path(), "old.txt", "editor B").unwrap();
    assert_eq!(
        fs::read(temp.path().join("orig.slot")).unwrap(),
        b"original A"
    );
    assert_eq!(fs::read(temp.path().join("old.txt")).unwrap(), b"editor B");
}

/// Windows renames the *opened file*, so its window lies between the open and the
/// rename. The source handle excludes `FILE_SHARE_DELETE`, so an editor's rename-over in
/// that window fails (its bytes stay in its temporary file) instead of being moved or
/// unlinked; once the move is done, the editor's retry lands.
#[cfg(windows)]
#[test]
fn an_editor_save_inside_the_open_rename_window_fails_without_losing_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Dir::open_root(temp.path()).unwrap();
    fs::write(temp.path().join("old.txt"), "original A").unwrap();
    let in_window = Cell::new(None);
    dir.rename_noreplace_observed("old.txt", &dir, "orig.slot", || {
        in_window.set(Some(editor_save(temp.path(), "old.txt", "editor B")));
    })
    .unwrap();
    let error = in_window.take().expect("window opened").unwrap_err();
    eprintln!(
        "editor rename-over inside the window: {error} ({:?})",
        error.raw_os_error()
    );
    assert_eq!(
        error.raw_os_error(),
        Some(5),
        "ERROR_ACCESS_DENIED expected: {error}"
    );
    assert_eq!(
        fs::read(temp.path().join(".editor.tmp")).unwrap(),
        b"editor B"
    );
    assert_eq!(
        fs::read(temp.path().join("orig.slot")).unwrap(),
        b"original A"
    );
    // The editor retries after the move: nothing was lost or overwritten.
    fs::rename(temp.path().join(".editor.tmp"), temp.path().join("old.txt")).unwrap();
    assert_eq!(fs::read(temp.path().join("old.txt")).unwrap(), b"editor B");
}

#[test]
fn a_source_replaced_after_verification_is_detected_and_never_lost() {
    let temp = tempfile::tempdir().unwrap();
    let dir = Dir::open_root(temp.path()).unwrap();
    fs::write(temp.path().join("old.txt"), "original A").unwrap();
    let verified = dir.stat("old.txt").unwrap().unwrap().id;
    editor_save(temp.path(), "old.txt", "editor B").unwrap();
    assert_ne!(dir.stat("old.txt").unwrap().unwrap().id, verified);
    let error = dir
        .rename_noreplace_checked("old.txt", &dir, "orig.slot", verified)
        .unwrap_err();
    assert!(error.to_string().contains("was replaced"), "{error}");
    // Windows refuses before moving; macOS moves back. Either way B is at its name.
    assert_eq!(fs::read(temp.path().join("old.txt")).unwrap(), b"editor B");
    assert!(dir.stat("orig.slot").unwrap().is_none());
    // The unchanged source moves.
    let current = dir.stat("old.txt").unwrap().unwrap().id;
    dir.rename_noreplace_checked("old.txt", &dir, "orig.slot", current)
        .unwrap();
    assert_eq!(
        fs::read(temp.path().join("orig.slot")).unwrap(),
        b"editor B"
    );
}

// ---- links ------------------------------------------------------------------------

#[test]
fn names_never_follow_links_or_open_special_files() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), "outside").unwrap();
    symlink_dir(outside.path(), &temp.path().join("link-dir"));
    symlink_file(
        &outside.path().join("secret"),
        &temp.path().join("link-file"),
    );
    symlink_file(&temp.path().join("nowhere"), &temp.path().join("dangling"));
    #[cfg(windows)]
    junction(outside.path(), &temp.path().join("junction"));
    #[cfg(target_os = "macos")]
    assert!(
        std::process::Command::new("mkfifo")
            .arg(temp.path().join("pipe"))
            .status()
            .unwrap()
            .success()
    );
    fs::create_dir(temp.path().join("real")).unwrap();
    fs::write(temp.path().join("plain"), "plain").unwrap();
    let root = Dir::open_root(temp.path()).unwrap();

    let mut links = vec!["link-dir", "link-file", "dangling"];
    if cfg!(windows) {
        links.push("junction");
    }
    for link in &links {
        assert_eq!(
            root.stat(link).unwrap().unwrap().kind,
            EntryKind::Link,
            "{link}"
        );
        let error = root.open_relative(link).unwrap_err();
        eprintln!("open_relative({link}): {error}");
        assert!(root.open_regular(link).is_err(), "{link}");
        // Creation and publication refuse an existing name, a dangling link included.
        assert_eq!(
            root.create_new(link).unwrap_err().kind(),
            ErrorKind::AlreadyExists,
            "{link}"
        );
        fs::write(temp.path().join("staged"), "staged").unwrap();
        assert_eq!(
            root.rename_noreplace("staged", &root, link)
                .unwrap_err()
                .kind(),
            ErrorKind::AlreadyExists,
            "{link}"
        );
        fs::remove_file(temp.path().join("staged")).unwrap();
    }
    // A junction opened as a directory must not be traversed to the outside.
    #[cfg(windows)]
    assert!(root.open_child("junction").is_err());
    assert!(root.open_relative("real").is_ok());
    assert!(
        root.open_regular("real").is_err(),
        "a directory is not a regular file"
    );
    assert!(root.open_regular("plain").is_ok());
    #[cfg(target_os = "macos")]
    assert!(
        root.open_regular("pipe").is_err(),
        "a FIFO is not a regular file"
    );
    assert!(!temp.path().join("nowhere").exists());
    // Unlinking a link removes the link, never its target.
    root.unlink("link-file").unwrap();
    assert!(root.stat("link-file").unwrap().is_none());
    // Components must be single names.
    assert!(root.stat("real/inner").is_err());
    assert!(root.stat("..").is_err());
    #[cfg(windows)]
    {
        assert!(root.stat("real\\inner").is_err());
        assert!(
            root.create_new("plain:stream").is_err(),
            "no alternate data streams"
        );
    }
    assert_eq!(
        fs::read_to_string(outside.path().join("secret")).unwrap(),
        "outside"
    );
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 1);
}

// ---- swapped directory ------------------------------------------------------------

#[test]
fn a_swapped_directory_is_detected_by_identity() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("sub")).unwrap();
    let root = Dir::open_root(temp.path()).unwrap();
    let sub = root.open_relative("sub").unwrap();
    assert!(sub.is_still_at(&root, "sub"));
    // The held handle does not stop the user from moving the directory.
    fs::rename(temp.path().join("sub"), temp.path().join("moved")).unwrap();
    fs::create_dir(temp.path().join("sub")).unwrap();
    assert!(
        !sub.is_still_at(&root, "sub"),
        "a replacement directory has another identity"
    );
    fs::remove_dir(temp.path().join("sub")).unwrap();
    symlink_dir(&temp.path().join("moved"), &temp.path().join("sub"));
    assert!(!sub.is_still_at(&root, "sub"), "a symlink never matches");
    #[cfg(windows)]
    {
        fs::remove_dir(temp.path().join("sub")).unwrap();
        junction(&temp.path().join("moved"), &temp.path().join("sub"));
        assert!(!sub.is_still_at(&root, "sub"), "a junction never matches");
    }
    // Operations through the held handle still reach the moved directory, not the name.
    sub.create_new("through-handle").unwrap();
    assert!(temp.path().join("moved").join("through-handle").exists());
}

// ---- root binding -----------------------------------------------------------------

fn root_replaced_by(make_link: impl Fn(&Path, &Path)) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let root = Dir::open_root(&project).unwrap();
    root.check_root_binding().unwrap();
    root.try_clone().unwrap().check_root_binding().unwrap();
    fs::rename(&project, temp.path().join("moved")).unwrap();
    assert!(
        root.check_root_binding().is_err(),
        "a missing root pathname is not the opened directory"
    );
    make_link(&temp.path().join("moved"), &project);
    let error = root.check_root_binding().unwrap_err();
    eprintln!("{error}");
    assert!(
        error.contains("link") || error.contains("replaced"),
        "{error}"
    );
    // The held handle still reaches the real directory.
    assert!(root.is_still_at(&root, ""));
}

#[test]
fn a_root_renamed_away_and_replaced_by_a_symlink_fails_the_pathname_binding() {
    root_replaced_by(symlink_dir);
}

#[cfg(windows)]
#[test]
fn a_root_renamed_away_and_replaced_by_a_junction_fails_the_pathname_binding() {
    root_replaced_by(junction);
}

#[test]
fn a_root_renamed_away_and_replaced_by_a_look_alike_directory_fails_the_pathname_binding() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let root = Dir::open_root(&project).unwrap();
    fs::rename(&project, temp.path().join("moved")).unwrap();
    fs::create_dir(&project).unwrap();
    let error = root.check_root_binding().unwrap_err();
    assert!(
        error.contains("no longer leads to the opened directory"),
        "{error}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_swapped_ancestor_of_the_root_fails_the_pathname_binding() {
    let temp = tempfile::tempdir().unwrap();
    let outer = temp.path().join("outer");
    fs::create_dir_all(outer.join("project")).unwrap();
    let root = Dir::open_root(&outer.join("project")).unwrap();
    root.check_root_binding().unwrap();
    fs::rename(&outer, temp.path().join("outer-moved")).unwrap();
    fs::create_dir_all(outer.join("project")).unwrap();
    assert!(
        root.check_root_binding().is_err(),
        "same names through a different ancestor are another directory"
    );
}

/// NTFS refuses to rename a directory while any handle below it is open, so an
/// ancestor of a held root cannot be swapped at all (the root itself can, see above).
#[cfg(windows)]
#[test]
fn an_ancestor_of_a_held_root_cannot_be_swapped() {
    let temp = tempfile::tempdir().unwrap();
    let outer = temp.path().join("outer");
    fs::create_dir_all(outer.join("project")).unwrap();
    let root = Dir::open_root(&outer.join("project")).unwrap();
    let error = fs::rename(&outer, temp.path().join("outer-moved")).unwrap_err();
    assert_eq!(
        error.raw_os_error(),
        Some(5),
        "ERROR_ACCESS_DENIED expected: {error}"
    );
    root.check_root_binding().unwrap();
    // Once the handle is closed the ancestor moves freely.
    drop(root);
    fs::rename(&outer, temp.path().join("outer-moved")).unwrap();
}

#[test]
fn a_root_reached_through_a_link_is_bound_to_its_real_path() {
    // Opening through a link canonicalizes once; the binding then holds the real path,
    // so removing the link later does not break it, and the link is never re-followed.
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    fs::create_dir(&real).unwrap();
    symlink_dir(&real, &temp.path().join("alias"));
    let root = Dir::open_root(&temp.path().join("alias")).unwrap();
    root.check_root_binding().unwrap();
    #[cfg(windows)]
    fs::remove_dir(temp.path().join("alias")).unwrap();
    #[cfg(unix)]
    fs::remove_file(temp.path().join("alias")).unwrap();
    root.check_root_binding().unwrap();
    assert_eq!(root.id(), Dir::open_root(&real).unwrap().id());
}

// ---- filesystems without the primitive --------------------------------------------

/// Experiment, run by hand (`cargo test -- --ignored`): the probe over SMB through the
/// administrative share `\\localhost\C$` of a local temporary directory. Needs the share
/// to exist; changes no configuration.
#[cfg(windows)]
#[test]
#[ignore = "needs the C$ administrative share"]
fn the_probe_over_an_smb_share() {
    let temp = tempfile::tempdir().unwrap();
    let local = fs::canonicalize(temp.path()).unwrap().display().to_string();
    let rest = local.trim_start_matches(r"\\?\C:\");
    let nt_path = format!(r"\??\UNC\localhost\C$\{rest}");
    let dir = publish_primitives::open_unbound(&nt_path).unwrap();
    let result = probe_no_clobber(&dir);
    eprintln!("probe over {nt_path}: {result:?}");
    // Observed on Windows Server 2022: the SMB redirector rejects a rename with a
    // RootDirectory (ERROR_INVALID_PARAMETER), so the probe refuses the share.
    assert!(result.is_err());
    // Also the cross-directory form the transaction uses.
    fs::create_dir(temp.path().join("sub")).unwrap();
    fs::write(temp.path().join("a"), "a").unwrap();
    fs::write(temp.path().join("sub").join("a"), "b").unwrap();
    let sub = dir.open_child("sub").unwrap();
    eprintln!(
        "cross-directory onto existing: {:?}",
        dir.rename_noreplace("a", &sub, "a")
    );
    eprintln!(
        "cross-directory onto free: {:?}",
        dir.rename_noreplace("a", &sub, "free")
    );
    eprintln!(
        "sub entries: {:?}",
        fs::read_dir(temp.path().join("sub"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );
}
