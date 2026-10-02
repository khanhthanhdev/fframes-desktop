use std::fs;
use studio_project::{
    create, import,
    manifest::{CargoEntry, SdkPin},
    open,
};

fn pin() -> SdkPin {
    SdkPin {
        release: "sdk-v1".into(),
        compatibility_sha256: "a".repeat(64),
    }
}

#[test]
fn create_move_and_refuse_nonempty() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("Vídeo with spaces");
    let project = create(&root, "My video", pin(), "1.1.0", "0.1.0").unwrap();
    assert!(project.worker_available);
    assert!(create(&root, "Replacement", pin(), "1.1.0", "0.1.0").is_err());
    let moved = temp.path().join("Moved");
    fs::rename(root, &moved).unwrap();
    let reopened = open(&moved).unwrap();
    assert_eq!(reopened.inventory, project.inventory);
    let cargo = fs::read_to_string(moved.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("=1.1.0"));
    assert!(!cargo.contains("path ="));
    fs::remove_file(moved.join("media/DMSans-Medium.ttf")).unwrap();
    let error = open(&moved).unwrap_err();
    assert_eq!(error.field, "asset");
    assert_eq!(error.file, moved.join("media/DMSans-Medium.ttf"));
    assert!(error.action.contains("Locate or restore"));
}

#[test]
fn dirty_import_preserves_every_existing_byte() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='existing'\nversion='0.1.0'\n",
    )
    .unwrap();
    fs::write(root.join("AGENTS.md"), "User instructions").unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        output.stdout
    };
    git(&["init", "--quiet"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Studio test",
        "-c",
        "user.email=studio@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "--no-verify",
        "-m",
        "Baseline",
    ]);
    fs::write(root.join("src/main.rs"), "// dirty working source").unwrap();
    fs::write(root.join("untracked.txt"), "Untracked user content").unwrap();
    let head = git(&["rev-parse", "HEAD"]);
    let diff = git(&["diff", "--binary"]);
    assert!(!diff.is_empty());
    let before = studio_project::SourceInventory::scan(root).unwrap();
    let project = import(
        root,
        CargoEntry {
            manifest: "Cargo.toml".to_owned().try_into().unwrap(),
            package: "existing".into(),
            worker_target: "studio_worker".into(),
        },
        pin(),
    )
    .unwrap();
    assert!(!project.worker_available);
    for file in before.files {
        assert_eq!(
            file,
            project
                .inventory
                .files
                .iter()
                .find(|f| f.path == file.path)
                .unwrap()
                .clone()
        );
    }
    assert_eq!(git(&["rev-parse", "HEAD"]), head);
    assert_eq!(git(&["diff", "--binary"]), diff);
    assert_eq!(
        fs::read(root.join("untracked.txt")).unwrap(),
        b"Untracked user content"
    );
    let bytes = fs::read(root.join("studio.json")).unwrap();
    fs::write(
        root.join("studio.json"),
        String::from_utf8(bytes.clone())
            .unwrap()
            .replace("\"schema_version\": 1", "\"schema_version\": 999"),
    )
    .unwrap();
    assert_eq!(open(root).unwrap_err().action, "Update Studio");
    assert_ne!(fs::read(root.join("studio.json")).unwrap(), bytes);
}

#[test]
fn import_validates_source_before_publishing_sidecar_and_supports_custom_bins() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='custom'\nversion='0.1.0'\n[[bin]]\nname='movie'\npath='movie.rs'\n",
    )
    .unwrap();
    let entry = CargoEntry {
        manifest: "Cargo.toml".to_owned().try_into().unwrap(),
        package: "custom".into(),
        worker_target: "studio_worker".into(),
    };
    let before = studio_project::SourceInventory::scan(root).unwrap();
    assert!(
        import(root, entry.clone(), pin())
            .unwrap_err()
            .reason
            .contains("source missing")
    );
    assert_eq!(studio_project::SourceInventory::scan(root).unwrap(), before);
    assert!(!root.join("studio.json").exists());
    fs::write(root.join("movie.rs"), "fn main() {}\n").unwrap();
    assert!(!import(root, entry, pin()).unwrap().worker_available);
    assert_eq!(fs::read(root.join("movie.rs")).unwrap(), b"fn main() {}\n");
}
