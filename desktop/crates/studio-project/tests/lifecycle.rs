use std::fs;
use studio_project::{
    ProjectTemplate, create, create_from_template, import,
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
    let tokens = fs::read_to_string(root.join("style/tokens.json")).unwrap();
    assert!(tokens.contains("\"schema\":1"));
    assert!(
        project
            .inventory
            .files
            .iter()
            .any(|f| f.path.as_str() == "style/tokens.json")
    );
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
    assert_eq!(
        error.file,
        reopened.root.join("media").join("DMSans-Medium.ttf")
    );
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

/// Every file of a new project, relative to its root, with `/` separators.
fn files(root: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy();
                found.push(relative.replace('\\', "/"));
            }
        }
    }
    found.sort();
    found
}

#[test]
fn the_blank_template_writes_the_same_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("blank");
    let via_template = temp.path().join("blank-template");
    create(&root, "Video", pin(), "1.3.0", "0.2.0").unwrap();
    create_from_template(
        &via_template,
        "Video",
        pin(),
        "1.3.0",
        "0.2.0",
        ProjectTemplate::Blank,
    )
    .unwrap();
    let expected = [
        ".gitignore",
        "AGENTS.md",
        "Cargo.toml",
        "media/DMSans-Medium.ttf",
        "media/OFL.txt",
        "src/bin/studio_worker.rs",
        "src/lib.rs",
        "src/main.rs",
        "src/video.rs",
        "studio.json",
        "style/tokens.json",
    ];
    assert_eq!(files(&root), expected);
    assert_eq!(files(&via_template), expected);
    let template = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    for source in [
        "AGENTS.md",
        "src/bin/studio_worker.rs",
        "src/lib.rs",
        "src/main.rs",
        "src/video.rs",
        "style/tokens.json",
    ] {
        assert_eq!(
            fs::read(root.join(source)).unwrap(),
            fs::read(template.join(source)).unwrap(),
            "{source}"
        );
    }
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert_eq!(
        cargo,
        fs::read_to_string(via_template.join("Cargo.toml")).unwrap()
    );
    assert!(!cargo.contains("fframes-kit"));
}

#[test]
fn the_slide_deck_template_pins_the_kit_and_ships_its_documents() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("deck");
    let project = create_from_template(
        &root,
        "Deck",
        pin(),
        "1.3.0",
        "0.2.0",
        ProjectTemplate::SlideDeck,
    )
    .unwrap();
    assert!(project.worker_available);
    assert_eq!(
        files(&root),
        [
            ".gitignore",
            "AGENTS.md",
            "Cargo.toml",
            "media/DMSans-Medium.ttf",
            "media/OFL.txt",
            "slides.json",
            "src/bin/studio_worker.rs",
            "src/lib.rs",
            "src/main.rs",
            "src/video.rs",
            "studio.json",
            "style/tokens.json",
        ]
    );
    let cargo: toml::Table = fs::read_to_string(root.join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let deps = cargo["dependencies"].as_table().unwrap();
    let version = |name: &str| deps[name]["version"].as_str().unwrap().to_owned();
    assert_eq!(version("fframes"), "=1.3.0");
    for runtime in [
        "fframes-studio-runtime",
        "fframes-studio-protocol",
        "fframes-studio-kit",
    ] {
        assert_eq!(version(runtime), "=0.2.0", "{runtime}");
    }
    assert_eq!(
        version("fframes-kit"),
        format!("={}", ProjectTemplate::SlideDeck.kit_version().unwrap())
    );
    assert_eq!(deps["fframes-kit"]["features"][0].as_str(), Some("studio"));
    assert!(cargo.get("patch").is_none());
    let agents = fs::read_to_string(root.join("AGENTS.md")).unwrap();
    let blank = include_str!("../templates/AGENTS.md");
    assert!(agents.starts_with(blank), "the Blank text comes first");
    assert!(agents[blank.len()..].starts_with("Slides template:"));
    // The project's own props are absent: the deck draws the code's text.
    assert!(!root.join("props.json").exists());
}

/// The template's kit is the one fframes-studio-kit (and so the SDK) is built with.
#[test]
fn the_slide_deck_kit_is_the_sdk_kit() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../fframes-studio-kit/Cargo.toml");
    let cargo: toml::Table = fs::read_to_string(manifest).unwrap().parse().unwrap();
    let kit = cargo["dependencies"]["fframes-kit"]["version"]
        .as_str()
        .unwrap();
    assert_eq!(
        kit,
        format!("={}", ProjectTemplate::SlideDeck.kit_version().unwrap())
    );
    let template: toml::Table = include_str!("../templates-slides/Cargo.toml")
        .parse()
        .unwrap();
    assert_eq!(
        template["dependencies"]["fframes-kit"]["version"].as_str(),
        Some(kit)
    );
}

#[test]
fn upgrade_rewrites_every_framework_version_and_nothing_else() {
    use studio_project::lifecycle::{FrameworkVersions, upgrade_framework};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    create(&root, "Video", pin(), "1.1.0", "0.1.0").unwrap();
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "studio-video"
version = "0.1.0"
edition = "2024"

[dependencies]
fframes = { version = "=1.1.0", default-features = false } # framework
fframes-studio-runtime = "=0.1.0"
fframes-kit = { version = "=0.1.0", features = ["studio"] }
studio-protocol = { package = "fframes-studio-protocol", version = "=0.1.0" }
helper = { path = "helper" }
serde = "1"

[target.'cfg(windows)'.dev-dependencies.fframes-studio-kit]
version = "=0.1.0"

[workspace]
members = ["helper"]

[workspace.dependencies]
fframes = "=1.1.0"
"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("helper/src")).unwrap();
    fs::write(root.join("helper/src/lib.rs"), "").unwrap();
    let helper = "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n\n[dependencies]\nfframes = { workspace = true }\nfframes-studio-runtime = { path = \"../vendor\" }\n";
    fs::write(root.join("helper/Cargo.toml"), helper).unwrap();
    let next = SdkPin {
        release: "sdk-v2".into(),
        compatibility_sha256: "b".repeat(64),
    };

    // An SDK without the kit leaves the kit's line for the build to report.
    upgrade_framework(
        &root,
        next.clone(),
        FrameworkVersions {
            fframes: "1.3.0",
            runtime: "0.2.0",
            kit: None,
        },
    )
    .unwrap();

    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(
        cargo.contains(r#"fframes = { version = "=1.3.0", default-features = false } # framework"#),
        "{cargo}"
    );
    assert!(
        cargo.contains(r#"fframes-studio-runtime = "=0.2.0""#),
        "{cargo}"
    );
    assert!(
        cargo.contains(r#"fframes-kit = { version = "=0.1.0", features = ["studio"] }"#),
        "{cargo}"
    );
    assert!(
        cargo.contains(
            r#"studio-protocol = { package = "fframes-studio-protocol", version = "=0.2.0" }"#
        ),
        "{cargo}"
    );
    assert!(
        cargo.contains(
            "[target.'cfg(windows)'.dev-dependencies.fframes-studio-kit]\nversion = \"=0.2.0\""
        ),
        "{cargo}"
    );
    assert!(
        cargo.contains("[workspace.dependencies]\nfframes = \"=1.3.0\""),
        "{cargo}"
    );
    assert!(cargo.contains(r#"serde = "1""#), "{cargo}");
    // Inherited and versionless dependencies keep their lines byte for byte.
    assert_eq!(
        fs::read_to_string(root.join("helper/Cargo.toml")).unwrap(),
        helper
    );
    assert_eq!(open(&root).unwrap().manifest.sdk, next);
}
