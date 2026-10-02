use std::fs;
use studio_engine::build_materialization::{materialize, sdk_pin};
use studio_sdk::CompatibilityManifest;

#[test]
fn overlay_keeps_source_and_sdk_unchanged_and_rejects_external_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let sdk = temp.path().join("sdk");
    let compatibility = CompatibilityManifest::default_linux_x64();
    for name in [
        "fframes",
        "fframes-studio-runtime",
        "fframes-studio-protocol",
    ] {
        let dir = sdk.join("framework/framework").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
    }
    fs::create_dir_all(sdk.join("framework/vendor")).unwrap();
    let project =
        studio_project::create(&root, "Video", sdk_pin(&compatibility), "1.1.0", "0.1.0").unwrap();
    let build = materialize(
        &project,
        &sdk,
        compatibility.clone(),
        &temp.path().join("builds"),
    )
    .unwrap();
    assert_eq!(
        studio_project::open(&root).unwrap().inventory,
        project.inventory
    );
    assert!(!root.join(".cargo").exists());
    assert!(
        build
            .environment
            .target_dir
            .starts_with(temp.path().join("builds"))
    );
    assert!(
        fs::read_to_string(build.root.join("Cargo.toml"))
            .unwrap()
            .contains("path =")
    );
    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!("{cargo}\n[dependencies.outside]\npath = '../sdk'\n"),
    )
    .unwrap();
    let project = studio_project::open(&root).unwrap();
    assert!(materialize(&project, &sdk, compatibility, &temp.path().join("builds")).is_err());
}

#[test]
fn contained_dependency_with_absolute_path_is_rewritten_to_copied_workspace() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("workspace");
    let sdk = temp.path().join("sdk");
    let compatibility = CompatibilityManifest::default_linux_x64();
    for name in [
        "fframes",
        "fframes-studio-runtime",
        "fframes-studio-protocol",
    ] {
        let dir = sdk.join("framework/framework").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
    }
    fs::create_dir_all(sdk.join("framework/vendor")).unwrap();

    let _project =
        studio_project::create(&root, "Video", sdk_pin(&compatibility), "1.1.0", "0.1.0").unwrap();

    let helper = root.join("helper");
    fs::create_dir_all(&helper).unwrap();
    fs::write(
        helper.join("Cargo.toml"),
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(helper.join("lib.rs"), "pub fn help() {}\n").unwrap();

    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "{cargo}\n[dependencies.helper]\npath = \"{}\"\n",
            helper.display()
        ),
    )
    .unwrap();

    let project = studio_project::open(&root).unwrap();
    let build = materialize(
        &project,
        &sdk,
        compatibility.clone(),
        &temp.path().join("builds"),
    )
    .unwrap();

    let materialized_cargo = fs::read_to_string(build.root.join("Cargo.toml")).unwrap();
    assert!(!materialized_cargo.contains(&helper.to_string_lossy().to_string()));
    let copied_helper = build.root.join("helper");
    assert!(materialized_cargo.contains(&copied_helper.to_string_lossy().to_string()));
}
