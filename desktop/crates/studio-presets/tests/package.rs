mod common;

use std::{fs, path::Path};

use common::*;
use serde_json::json;
use studio_presets::{Code, Package, export_dir, import_dir, package::read_dir_files};

fn write_tree(root: &Path, files: &Files) {
    for (path, bytes) in files {
        let target = root.join(path.as_str());
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
}

/// Every file path + bytes under `root` (including staging leftovers), for byte-identity checks.
fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if path.is_dir() {
                out.push((format!("{rel}/"), Vec::new()));
                stack.push(path);
            } else {
                out.push((rel, fs::read(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

fn source_dir(files: &Files) -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("src");
    write_tree(&src, files);
    fs::create_dir(temp.path().join("store")).unwrap();
    (temp, src)
}

fn assert_rejected(files: &Files, code: Code) {
    expect_code(Package::from_files(files.clone()), code);
}

#[test]
fn directory_round_trips_with_identical_hash_files_and_licenses() {
    let (temp, src) = source_dir(&base());
    let store = temp.path().join("store");
    let imported = import_dir(&src, &store.join("editorial")).unwrap();
    let reread = Package::from_files(read_dir_files(&store.join("editorial")).unwrap()).unwrap();
    assert_eq!(imported.hash(), reread.hash());
    assert_eq!(imported.files(), &base());
    assert!(reread.files().contains_key(&p("licenses/DMSans-OFL.txt")));
    assert!(reread.files().contains_key(&p("LICENSE.txt")));
    export_dir(&reread, &store.join("exported")).unwrap();
    let again = Package::from_files(read_dir_files(&store.join("exported")).unwrap()).unwrap();
    assert_eq!(again.hash(), imported.hash());
    // No staging directories remain.
    let names: Vec<_> = fs::read_dir(&store)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        names.iter().all(|n| !n.starts_with(".preset-import-")),
        "{names:?}"
    );
}

#[test]
fn existing_destination_is_never_replaced() {
    let (temp, src) = source_dir(&base());
    let dest = temp.path().join("store/editorial");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("keep.txt"), b"mine").unwrap();
    let before = snapshot(&temp.path().join("store"));
    let e = import_dir(&src, &dest).unwrap_err();
    assert!(e.has(Code::AlreadyExists), "{e}");
    assert_eq!(snapshot(&temp.path().join("store")), before);
    // An empty pre-existing directory and a file are also protected.
    let empty = temp.path().join("store/empty");
    fs::create_dir(&empty).unwrap();
    assert!(
        import_dir(&src, &empty)
            .unwrap_err()
            .has(Code::AlreadyExists)
    );
    assert!(fs::read_dir(&empty).unwrap().next().is_none());
    let file = temp.path().join("store/file");
    fs::write(&file, b"x").unwrap();
    assert!(
        import_dir(&src, &file)
            .unwrap_err()
            .has(Code::AlreadyExists)
    );
    assert_eq!(fs::read(&file).unwrap(), b"x");
}

#[test]
fn failed_imports_leave_the_store_byte_identical() {
    let mut files = base();
    files.get_mut(&p("tokens.json")).unwrap().push(b' '); // digest mismatch
    let (temp, src) = source_dir(&files);
    let store = temp.path().join("store");
    fs::write(store.join("existing.txt"), b"data").unwrap();
    let before = snapshot(&store);
    let e = import_dir(&src, &store.join("editorial")).unwrap_err();
    assert!(e.has(Code::HashMismatch), "{e}");
    assert_eq!(snapshot(&store), before);
}

#[cfg(unix)]
#[test]
fn symlinks_and_special_files_are_refused() {
    let (temp, src) = source_dir(&base());
    let store = temp.path().join("store");
    let before = snapshot(&store);

    // symlinked file
    let outside = temp.path().join("outside.txt");
    fs::write(&outside, b"secret").unwrap();
    std::os::unix::fs::symlink(&outside, src.join("link.txt")).unwrap();
    let e = import_dir(&src, &store.join("a")).unwrap_err();
    assert!(e.has(Code::UnsafeFile), "{e}");
    fs::remove_file(src.join("link.txt")).unwrap();

    // symlinked directory
    std::os::unix::fs::symlink(temp.path(), src.join("dirlink")).unwrap();
    assert!(
        import_dir(&src, &store.join("b"))
            .unwrap_err()
            .has(Code::UnsafeFile)
    );
    fs::remove_file(src.join("dirlink")).unwrap();

    // replacing a declared file by a link
    fs::remove_file(src.join("guide.md")).unwrap();
    std::os::unix::fs::symlink(&outside, src.join("guide.md")).unwrap();
    assert!(
        import_dir(&src, &store.join("c"))
            .unwrap_err()
            .has(Code::UnsafeFile)
    );
    fs::remove_file(src.join("guide.md")).unwrap();
    fs::write(src.join("guide.md"), &base()[&p("guide.md")]).unwrap();

    // fifo
    let fifo = std::ffi::CString::new(src.join("pipe").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(
        import_dir(&src, &store.join("d"))
            .unwrap_err()
            .has(Code::UnsafeFile)
    );
    fs::remove_file(src.join("pipe")).unwrap();

    // root itself a symlink
    let alias = temp.path().join("root-link");
    std::os::unix::fs::symlink(&src, &alias).unwrap();
    assert!(
        import_dir(&alias, &store.join("e"))
            .unwrap_err()
            .has(Code::UnsafeFile)
    );

    assert_eq!(snapshot(&store), before);
    // Sanity: with the hazards removed the same directory imports.
    import_dir(&src, &store.join("ok")).unwrap();
}

#[test]
fn traversal_and_non_portable_paths_are_refused() {
    let (temp, src) = source_dir(&base());
    if cfg!(windows) {
        // NTFS reads `bad:name.txt` as a data stream of `bad`; a verbatim (`\\?\`) path is
        // the route by which a trailing dot, invalid in portable paths, reaches the disk.
        fs::write(fs::canonicalize(&src).unwrap().join("bad."), b"x").unwrap();
    } else {
        fs::write(src.join("bad:name.txt"), b"x").unwrap();
    }
    let e = import_dir(&src, &temp.path().join("store/x")).unwrap_err();
    assert!(e.has(Code::InvalidPath), "{e}");
    // Manifest declared traversal never reaches the filesystem: ProjectPath rejects it.
    let mut files = base();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&files[&p("preset.json")]).unwrap();
    manifest["guide"]["path"] = json!("../guide.md");
    files.insert(p("preset.json"), serde_json::to_vec(&manifest).unwrap());
    assert_rejected(&files, Code::Malformed);
    manifest["guide"]["path"] = json!("/etc/passwd");
    files.insert(p("preset.json"), serde_json::to_vec(&manifest).unwrap());
    assert_rejected(&files, Code::Malformed);
    assert!(!temp.path().join("store/x").exists());
}

#[test]
fn duplicate_and_colliding_paths_are_refused() {
    let mut files = base();
    files.insert(p("GUIDE.md"), b"x".to_vec());
    assert_rejected(&files, Code::DuplicatePath);

    let mut files = base();
    edit_manifest(&mut files, |m| {
        let guide = m["guide"].clone();
        m["examples"][0]["file"] = guide; // same path declared for two roles
    });
    assert_rejected(&files, Code::DuplicatePath);

    let mut files = base();
    edit_manifest(&mut files, |m| {
        let font = m["fonts"][0].clone();
        m["fonts"].as_array_mut().unwrap().push(font);
    });
    assert_rejected(&files, Code::DuplicatePath);
}

#[test]
fn limits_on_files_depth_names_and_sizes() {
    let mut files = base();
    for i in 0..70 {
        files.insert(p(&format!("extra/f{i}.txt")), vec![b'x']);
    }
    assert_rejected(&files, Code::TooLarge);

    let mut files = base();
    files.insert(p("a/b/c/d/e/f.txt"), b"x".to_vec());
    assert_rejected(&files, Code::InvalidPath);

    let mut files = base();
    files.insert(p(&format!("{}.txt", "n".repeat(70))), b"x".to_vec());
    assert_rejected(&files, Code::InvalidPath);

    let mut files = base();
    files.insert(p("fonts/DMSans-Medium.ttf"), vec![0; 9 * 1024 * 1024]);
    reseal(&mut files);
    assert_rejected(&files, Code::TooLarge);

    let mut files = base();
    files.insert(p("guide.md"), vec![b'a'; 300 * 1024]);
    reseal(&mut files);
    assert_rejected(&files, Code::TooLarge);

    // total budget
    let mut files = base();
    for i in 0..4 {
        files.insert(p(&format!("big{i}.bin")), vec![0; 7 * 1024 * 1024]);
    }
    assert_rejected(&files, Code::TooLarge);

    // directory reader enforces the same bounds before reading everything
    let (temp, src) = source_dir(&base());
    fs::create_dir_all(src.join("a/b/c/d/e")).unwrap();
    fs::write(src.join("a/b/c/d/e/x.txt"), b"x").unwrap();
    let e = import_dir(&src, &temp.path().join("store/x")).unwrap_err();
    assert!(e.has(Code::TooDeep), "{e}");
    fs::remove_dir_all(src.join("a")).unwrap();
    fs::write(src.join("huge.bin"), vec![0; 9 * 1024 * 1024]).unwrap();
    assert!(
        import_dir(&src, &temp.path().join("store/y"))
            .unwrap_err()
            .has(Code::TooLarge)
    );
}

#[test]
fn schema_versions_and_manifest_fields_are_validated() {
    for (key, value) in [("schema", json!(2)), ("schema", json!(0))] {
        let mut files = base();
        edit_manifest(&mut files, |m| m[key] = value);
        assert_rejected(&files, Code::UnsupportedSchema);
    }
    let mut files = base();
    edit_manifest(&mut files, |m| m["token_schema"] = json!(2));
    assert_rejected(&files, Code::UnsupportedSchema);
    let mut files = base();
    edit_manifest(&mut files, |m| m["unexpected"] = json!(1));
    assert_rejected(&files, Code::Malformed);
    for (key, value) in [
        ("id", json!("Bad Id")),
        ("version", json!("1.0")),
        ("name", json!("")),
        ("author", json!("x".repeat(200))),
    ] {
        let mut files = base();
        edit_manifest(&mut files, |m| m[key] = value);
        assert_rejected(&files, Code::InvalidValue);
    }
    let mut files = base();
    files.insert(p("preset.json"), b"{not json".to_vec());
    assert_rejected(&files, Code::Malformed);
    files.remove(&p("preset.json"));
    assert_rejected(&files, Code::MissingResource);
}

#[test]
fn digests_and_undeclared_or_missing_files_are_enforced() {
    let mut files = base();
    files.get_mut(&p("guide.md")).unwrap().push(b'!');
    assert_rejected(&files, Code::HashMismatch);

    let mut files = base();
    files.insert(p("notes.txt"), b"x".to_vec());
    assert_rejected(&files, Code::UndeclaredFile);

    let mut files = base();
    files.remove(&p("fonts/DMSans-Medium.ttf"));
    assert_rejected(&files, Code::MissingResource);

    let mut files = base();
    edit_manifest(&mut files, |m| m["guide"]["sha256"] = json!("XYZ"));
    // fix() only rewrites when the file exists, so corrupt after the fact
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&files[&p("preset.json")]).unwrap();
    manifest["guide"]["sha256"] = json!("00");
    files.insert(p("preset.json"), serde_json::to_vec(&manifest).unwrap());
    assert_rejected(&files, Code::HashMismatch);
}

#[test]
fn fonts_must_exist_parse_and_match_their_declared_family() {
    let mut files = base();
    files.insert(p("fonts/DMSans-Medium.ttf"), b"not a font at all".to_vec());
    reseal(&mut files);
    assert_rejected(&files, Code::UnsupportedMedia);

    let mut files = base();
    edit_manifest(&mut files, |m| {
        m["fonts"][0]["family"] = json!("Other Family")
    });
    assert_rejected(&files, Code::FontMismatch);

    // A token bound to a family nobody bundled is an actionable error, not a system fallback.
    let mut files = base();
    let tokens = String::from_utf8(files[&p("tokens.json")].clone())
        .unwrap()
        .replace("\"DM Sans\"", "\"Helvetica\"");
    files.insert(p("tokens.json"), tokens.into_bytes());
    reseal(&mut files);
    let e = expect_code(Package::from_files(files), Code::FontMismatch);
    assert!(
        e.diagnostics[0].field.starts_with("tokens.typography."),
        "{e}"
    );
    assert!(e.to_string().contains("system fonts"));

    let mut files = base();
    edit_manifest(&mut files, |m| {
        m["fonts"][0]["file"]["path"] = json!("fonts/DMSans-Medium.woff2");
    });
    assert!(Package::from_files(files).is_err());

    let mut files = base();
    let font = files.remove(&p("fonts/DMSans-Medium.ttf")).unwrap();
    files.insert(p("fonts/font.exe"), font);
    edit_manifest(&mut files, |m| {
        m["fonts"][0]["file"]["path"] = json!("fonts/font.exe")
    });
    assert_rejected(&files, Code::UnsupportedMedia);

    let mut files = base();
    let mut collection = b"ttcf".to_vec();
    collection.extend_from_slice(&[0; 64]);
    files.insert(p("fonts/DMSans-Medium.ttf"), collection);
    reseal(&mut files);
    assert_rejected(&files, Code::UnsupportedMedia);
}

#[test]
fn licenses_are_required_known_and_match_their_text() {
    let mut files = base();
    edit_manifest(&mut files, |m| {
        m["fonts"][0]["license"] = json!("licenses/missing.txt")
    });
    assert_rejected(&files, Code::MissingLicense);

    let mut files = base();
    edit_manifest(&mut files, |m| m["license"] = json!("nowhere.txt"));
    assert_rejected(&files, Code::MissingLicense);

    let mut files = base();
    edit_manifest(&mut files, |m| {
        m["licenses"][1]["spdx"] = json!("Proprietary")
    });
    assert_rejected(&files, Code::InvalidLicense);

    let mut files = base();
    files.insert(
        p("licenses/DMSans-OFL.txt"),
        b"All rights reserved, do whatever, trust us, thanks.".to_vec(),
    );
    reseal(&mut files);
    assert_rejected(&files, Code::InvalidLicense);

    let mut files = base();
    files.insert(p("licenses/DMSans-OFL.txt"), vec![0xFF; 100]);
    reseal(&mut files);
    assert_rejected(&files, Code::InvalidLicense);

    // example without license reference
    let mut files = base();
    edit_manifest(&mut files, |m| {
        m["examples"][0]["license"] = json!("elsewhere.txt")
    });
    assert_rejected(&files, Code::MissingLicense);
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

fn with_asset(path: &str, bytes: &[u8]) -> Files {
    let mut files = base();
    files.insert(p(path), bytes.to_vec());
    edit_manifest(&mut files, |m| {
        m["assets"] =
            json!([{"file": {"path": path, "sha256": "0", "bytes": 0}, "license": "LICENSE.txt"}]);
    });
    files
}

#[test]
fn media_assets_are_type_checked_and_licensed() {
    for (path, bytes) in [
        ("assets/logo.png", PNG),
        ("assets/photo.jpg", &[0xFF, 0xD8, 0xFF, 0xE0]),
        (
            "assets/mark.svg",
            b"<svg xmlns='http://www.w3.org/2000/svg'/>",
        ),
    ] {
        let package =
            Package::from_files(with_asset(path, bytes)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let style = studio_presets::materialize(&package, &Default::default()).unwrap();
        assert!(
            style
                .files
                .iter()
                .any(|(k, _)| k.as_str() == format!("media/preset-{}", path.replace('/', "-")))
        );
    }
    // Same bytes under the wrong extension, wrong signature, scripts, unknown types, oversize.
    assert_rejected(&with_asset("assets/logo.jpg", PNG), Code::UnsupportedMedia);
    assert_rejected(
        &with_asset("assets/logo.png", b"GIF89a"),
        Code::UnsupportedMedia,
    );
    assert_rejected(
        &with_asset("assets/x.svg", b"<svg><script>alert(1)</script></svg>"),
        Code::UnsupportedMedia,
    );
    assert_rejected(
        &with_asset(
            "assets/x.svg",
            b"<svg><image href=\"http://evil/x.png\"/></svg>",
        ),
        Code::UnsupportedMedia,
    );
    assert_rejected(
        &with_asset("assets/x.gif", b"GIF89a"),
        Code::UnsupportedMedia,
    );
    assert_rejected(
        &with_asset(
            "assets/x.png",
            &[b"\x89PNG\r\n\x1a\n".as_slice(), &vec![0; 9 * 1024 * 1024]].concat(),
        ),
        Code::TooLarge,
    );
    // license required
    let mut files = with_asset("assets/logo.png", PNG);
    edit_manifest(&mut files, |m| {
        m["assets"][0]["license"] = json!("licenses/none.txt")
    });
    assert_rejected(&files, Code::MissingLicense);
}

#[test]
fn hash_covers_tokens_licenses_and_resources_but_not_formatting() {
    let original = Package::from_files(base()).unwrap();

    // Pretty-printing the manifest or re-formatting tokens.json does not change the hash.
    let mut files = base();
    let tokens: serde_json::Value = serde_json::from_slice(&files[&p("tokens.json")]).unwrap();
    files.insert(p("tokens.json"), serde_json::to_vec(&tokens).unwrap());
    reseal(&mut files);
    assert_eq!(Package::from_files(files).unwrap().hash(), original.hash());

    // Material changes do: a token value, a font byte, a license byte, the guide, metadata.
    type Mutation = Box<dyn Fn(&mut Files)>;
    let mutate: Vec<Mutation> = vec![
        Box::new(|f| {
            let t = String::from_utf8(f[&p("tokens.json")].clone())
                .unwrap()
                .replace("#B3261E", "#B3261F");
            f.insert(p("tokens.json"), t.into_bytes());
        }),
        Box::new(|f| {
            f.get_mut(&p("fonts/DMSans-Medium.ttf"))
                .unwrap()
                .extend_from_slice(&[0; 4])
        }),
        Box::new(|f| {
            f.get_mut(&p("licenses/DMSans-OFL.txt"))
                .unwrap()
                .extend_from_slice(b"\n")
        }),
        Box::new(|f| f.get_mut(&p("guide.md")).unwrap().extend_from_slice(b"\n")),
        Box::new(|f| {
            f.get_mut(&p("examples/title-card.md"))
                .unwrap()
                .extend_from_slice(b"\n")
        }),
        Box::new(|f| edit_manifest(f, |m| m["version"] = json!("1.0.1"))),
        Box::new(|f| edit_manifest(f, |m| m["design"]["width"] = json!(1280))),
    ];
    for (i, change) in mutate.iter().enumerate() {
        let mut files = base();
        change(&mut files);
        reseal(&mut files);
        // Appending to a font keeps it parseable, so the package stays valid.
        let package = Package::from_files(files).unwrap_or_else(|e| panic!("mutation {i}: {e}"));
        assert_ne!(package.hash(), original.hash(), "mutation {i}");
    }
}
