use serde_json::{Value, json};
use studio_project::{Manifest, ProjectPath};
fn valid() -> Value {
    json!({"schema_version":1,"project_id":"project-123","display":{"name":"A video","description":null},"sdk":{"release":"sdk-1.0.0","compatibility_sha256":"a".repeat(64)},"entry":{"manifest":"Cargo.toml","package":"my-video","worker_target":"studio_worker"},"assets":["media/photo.png"],"generated_instruction_version":1,"video_hints":{"width":1920,"height":1080,"fps":30},"preset":null})
}
fn parse(v: Value) -> Result<Manifest, studio_project::ProjectError> {
    Manifest::parse(&serde_json::to_vec(&v).unwrap())
}
#[test]
fn round_trip_and_version_gate_are_read_only() {
    let m = parse(valid()).unwrap();
    assert_eq!(
        m,
        Manifest::parse(&serde_json::to_vec(&m).unwrap()).unwrap()
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("studio.json");
    for version in [0, 2, u64::MAX] {
        let mut v = valid();
        v["schema_version"] = json!(version);
        v["entry"] = json!("unknown newer shape");
        let bytes = serde_json::to_vec(&v).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let error = Manifest::parse(&std::fs::read(&path).unwrap()).unwrap_err();
        assert_eq!(error.field, "schema_version");
        if version > 1 {
            assert_eq!(error.action, "Update Studio");
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
#[test]
fn malformed_missing_invalid_and_nonportable_metadata() {
    assert!(Manifest::parse(b"{").is_err());
    for field in [
        "schema_version",
        "project_id",
        "sdk",
        "entry",
        "assets",
        "generated_instruction_version",
    ] {
        let mut v = valid();
        v.as_object_mut().unwrap().remove(field);
        assert!(parse(v).is_err(), "{field}");
    }
    for (field, value) in [
        ("project_id", json!("../bad")),
        ("assets", json!(["/media/x"])),
        ("assets", json!(["media/x", "media/x"])),
        ("generated_instruction_version", json!(0)),
        ("credentials", json!("secret")),
    ] {
        let mut v = valid();
        v[field] = value;
        assert!(parse(v).is_err(), "{field}");
    }
    let mut v = valid();
    v["sdk"]["release"] = json!("/opt/sdk");
    assert!(parse(v).is_err());
    let mut v = valid();
    v["sdk"]["compatibility_sha256"] = json!("bad");
    assert!(parse(v).is_err());
    let mut v = valid();
    v["video_hints"]["fps"] = json!(0);
    assert!(parse(v).is_err());
    let mut v = valid();
    v["display"]["name"] = json!("x".repeat(257));
    assert!(parse(v).is_err());
    assert!(Manifest::parse(&vec![b' '; 1024 * 1024 + 1]).is_err());
}
#[test]
fn declared_paths_are_inventoried_and_cargo_names_are_valid() {
    for excluded in [".git", "target", ".fframes/context", ".fframes/cache"] {
        for asset in [excluded.to_owned(), format!("{excluded}/asset.png")] {
            let mut v = valid();
            v["assets"] = json!([asset]);
            assert_eq!(parse(v).unwrap_err().field, "assets");
        }
        let mut v = valid();
        v["entry"]["manifest"] = json!(format!("{excluded}/Cargo.toml"));
        assert_eq!(parse(v).unwrap_err().field, "entry.manifest");

        let mut v = valid();
        v["assets"] = json!([format!("nested/{excluded}/asset.png")]);
        v["entry"]["manifest"] = json!(format!("nested/{excluded}/Cargo.toml"));
        assert!(parse(v).is_ok());
    }
    for field in ["package", "worker_target"] {
        for name in ["has.dot", "has space", "../escape", ""] {
            let mut v = valid();
            v["entry"][field] = json!(name);
            assert!(parse(v).is_err(), "{field}: {name}");
        }
    }
}
#[test]
fn portable_paths_and_containment() {
    for path in [
        "",
        "/tmp/file",
        "../a",
        "a/../b",
        "a/./b",
        "C:/x",
        "C:\\x",
        "\\\\server\\share",
        "//server/share",
        "a\0b",
        "a//b",
        "a/",
        "media/CON.png",
        "media/trailing.",
        "media/trailing ",
        "media/a*b",
    ] {
        assert!(ProjectPath::try_from(path.to_owned()).is_err(), "{path:?}");
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("media")).unwrap();
    std::fs::write(dir.path().join("media/ảnh space.png"), b"image").unwrap();
    let path = ProjectPath::try_from("media/ảnh space.png".to_owned()).unwrap();
    assert_eq!(
        path.resolve_existing(dir.path()).unwrap(),
        dir.path().join("media/ảnh space.png")
    );
    assert!(path.open_file(dir.path()).is_ok());
}
#[cfg(unix)]
#[test]
fn links_and_replacements_are_rejected() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), b"secret").unwrap();
    symlink(outside.path(), dir.path().join("media")).unwrap();
    let p = ProjectPath::try_from("media/secret".to_owned()).unwrap();
    assert!(
        p.open_file(dir.path())
            .unwrap_err()
            .reason
            .contains("unsupported link")
    );
    std::fs::remove_file(dir.path().join("media")).unwrap();
    std::fs::create_dir(dir.path().join("media")).unwrap();
    std::fs::write(dir.path().join("media/secret"), b"local").unwrap();
    p.resolve_existing(dir.path()).unwrap();
    std::fs::remove_file(dir.path().join("media/secret")).unwrap();
    symlink(
        outside.path().join("secret"),
        dir.path().join("media/secret"),
    )
    .unwrap();
    assert!(p.open_file(dir.path()).is_err());
}
