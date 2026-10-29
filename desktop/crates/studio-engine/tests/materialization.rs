use std::fs;
use studio_engine::build_materialization::{materialize, sdk_pin};
use studio_sdk::CompatibilityManifest;

#[test]
fn kit_patch_belongs_to_the_selected_packages_workspace_not_independent_tools() {
    for (entry_dir, owner, inherited) in [
        ("", None, false),
        ("", Some("Cargo.toml"), false),
        ("video", Some("Cargo.toml"), true),
        ("video", Some("video/Cargo.toml"), false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let sdk = temp.path().join("sdk");
        let mut compatibility = CompatibilityManifest::default_linux_x64();
        compatibility.kit_version = Some("0.3.0".into());
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
        let project = studio_project::create(
            &root,
            "Video",
            sdk_pin(&compatibility),
            &compatibility.fframes_version,
            &compatibility.runtime_version,
        )
        .unwrap();
        let mut cargo: toml::Value = fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
        cargo.as_table_mut().unwrap().remove("workspace");
        cargo["dependencies"]
            .as_table_mut()
            .unwrap()
            .insert("fframes-kit".into(), toml::Value::String("=0.3.0".into()));
        let entry = std::path::Path::new(entry_dir).join("Cargo.toml");
        if owner.is_some_and(|owner| std::path::Path::new(owner) == entry) {
            cargo
                .as_table_mut()
                .unwrap()
                .insert("workspace".into(), toml::Value::Table(toml::Table::new()));
        }
        if !entry_dir.is_empty() {
            fs::create_dir_all(root.join(entry_dir)).unwrap();
            fs::rename(root.join("src"), root.join(entry_dir).join("src")).unwrap();
            let mut workspace: toml::Value = if inherited {
                "[workspace]\nmembers = ['video']\nexclude = ['tools']\n"
            } else {
                "[workspace]\nmembers = []\nexclude = ['video', 'tools']\n"
            }
            .parse()
            .unwrap();
            if inherited {
                let mut dependencies = toml::Table::new();
                for name in ["fframes", "fframes-kit"] {
                    dependencies.insert(name.into(), cargo["dependencies"][name].clone());
                    cargo["dependencies"][name] =
                        "workspace = true".parse::<toml::Value>().unwrap();
                }
                workspace["workspace"]
                    .as_table_mut()
                    .unwrap()
                    .insert("dependencies".into(), toml::Value::Table(dependencies));
            }
            fs::write(
                root.join("Cargo.toml"),
                toml::to_string_pretty(&workspace).unwrap(),
            )
            .unwrap();
            let mut manifest = project.manifest;
            manifest.entry.manifest = entry
                .to_string_lossy()
                .replace('\\', "/")
                .try_into()
                .unwrap();
            studio_project::lifecycle::write_manifest(&root, &manifest).unwrap();
        }
        fs::write(root.join(&entry), toml::to_string_pretty(&cargo).unwrap()).unwrap();
        fs::create_dir(root.join("tools")).unwrap();
        fs::write(
            root.join("tools/Cargo.toml"),
            "[package]\nname = 'tools'\nversion = '0.1.0'\n[workspace]\n",
        )
        .unwrap();
        fs::create_dir(root.join("tools/src")).unwrap();
        fs::write(root.join("tools/src/main.rs"), "fn main() {}\n").unwrap();
        let project = studio_project::open(&root).unwrap();
        let build =
            materialize(&project, &sdk, compatibility, &temp.path().join("builds")).unwrap();
        let patch_owner = owner.map(std::path::Path::new).unwrap_or(&entry);
        for file in &project.inventory.files {
            if file.path.as_str().rsplit('/').next() != Some("Cargo.toml") {
                continue;
            }
            let staged: toml::Value = fs::read_to_string(build.root.join(file.path.as_str()))
                .unwrap()
                .parse()
                .unwrap();
            let original: toml::Value = fs::read_to_string(root.join(file.path.as_str()))
                .unwrap()
                .parse()
                .unwrap();
            assert!(
                original.get("patch").is_none(),
                "portable source stays unchanged"
            );
            if std::path::Path::new(file.path.as_str()) == patch_owner {
                assert_eq!(
                    std::path::Path::new(
                        staged["patch"]["crates-io"]["fframes"]["path"]
                            .as_str()
                            .unwrap()
                    ),
                    fs::canonicalize(&sdk)
                        .unwrap()
                        .join("framework/framework/fframes"),
                    "entry {entry:?}, owner {owner:?}"
                );
            } else {
                assert!(
                    staged.get("patch").is_none(),
                    "unrelated manifest {}",
                    file.path.as_str()
                );
            }
        }
        if inherited {
            let staged: toml::Value = fs::read_to_string(&build.manifest)
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(
                staged["dependencies"]["fframes"]["workspace"].as_bool(),
                Some(true)
            );
            assert_eq!(
                staged["dependencies"]["fframes-kit"]["workspace"].as_bool(),
                Some(true)
            );
        }
        assert_eq!(
            studio_project::open(&root).unwrap().inventory,
            project.inventory
        );
    }
}

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
    let project = studio_project::create(
        &root,
        "Video",
        sdk_pin(&compatibility),
        &compatibility.fframes_version,
        &compatibility.runtime_version,
    )
    .unwrap();
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
    assert_eq!(
        build.root.parent().unwrap().parent().unwrap(),
        temp.path().join("builds")
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

    let _project = studio_project::create(
        &root,
        "Video",
        sdk_pin(&compatibility),
        &compatibility.fframes_version,
        &compatibility.runtime_version,
    )
    .unwrap();

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
            "{cargo}\n[dependencies.helper]\npath = {}\n",
            toml::Value::String(helper.to_string_lossy().into_owned())
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

    let materialized_cargo: toml::Value = fs::read_to_string(build.root.join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let copied_helper = build.root.join("helper");
    let rewritten_path = materialized_cargo["dependencies"]["helper"]["path"]
        .as_str()
        .unwrap();
    assert_eq!(
        std::path::Path::new(rewritten_path),
        copied_helper.as_path()
    );
}

#[test]
fn cancelled_copy_removes_partial_tree_and_preserves_captured_source() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let sdk = temp.path().join("sdk");
    fs::create_dir(&sdk).unwrap();
    let compatibility = CompatibilityManifest::default_linux_x64();
    studio_project::create(
        &root,
        "Video",
        sdk_pin(&compatibility),
        &compatibility.fframes_version,
        &compatibility.runtime_version,
    )
    .unwrap();
    let bytes = vec![93; 2 * 1024 * 1024];
    fs::write(root.join("media/large.bin"), &bytes).unwrap();
    let project = studio_project::open(&root).unwrap();
    let calls = std::cell::Cell::new(0);
    let builds = temp.path().join("builds");
    let result = studio_engine::build_materialization::materialize_with_cancel(
        &project,
        &sdk,
        compatibility,
        &builds,
        &|| {
            calls.set(calls.get() + 1);
            calls.get() >= 4
        },
    );
    assert!(result.err().unwrap().to_string().contains("cancelled"));
    assert_eq!(fs::read(root.join("media/large.bin")).unwrap(), bytes);
    assert_eq!(
        studio_project::open(&root).unwrap().inventory,
        project.inventory
    );
    fn check(path: &std::path::Path) {
        for e in fs::read_dir(path).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                assert!(
                    !path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("build-")
                );
                check(&path);
            }
        }
    }
    check(&builds);
}

#[test]
fn projects_bind_to_the_framework_of_the_sdk_they_are_pinned_to() {
    let temp = tempfile::tempdir().unwrap();
    let sdk = temp.path().join("sdk");
    let builds = temp.path().join("builds");
    let mut compatibility = CompatibilityManifest::default_linux_x64();
    compatibility.fframes_version = "9.0.0".into();
    compatibility.runtime_version = "9.1.0".into();
    compatibility.validate().unwrap();
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
    let fframes_path = |project: &studio_project::OpenProject| {
        let build = materialize(project, &sdk, compatibility.clone(), &builds)?;
        let cargo: toml::Value = fs::read_to_string(build.root.join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let path = cargo["dependencies"]["fframes"]["path"].as_str().unwrap();
        Ok::<_, studio_project::ProjectError>(std::path::PathBuf::from(path))
    };

    let root = temp.path().join("current");
    let current =
        studio_project::create(&root, "Video", sdk_pin(&compatibility), "9.0.0", "9.1.0").unwrap();
    assert_eq!(
        fframes_path(&current).unwrap(),
        fs::canonicalize(&sdk)
            .unwrap()
            .join("framework/framework/fframes")
    );

    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    for (edited, reason) in [
        (
            cargo.replacen("=9.1.0", "=0.1.0", 1),
            "mixed framework releases",
        ),
        (
            cargo.replace("version = \"=9.0.0\"\n", ""),
            "a framework crate without a version",
        ),
    ] {
        assert_ne!(edited, cargo, "{reason}");
        fs::write(root.join("Cargo.toml"), edited).unwrap();
        let project = studio_project::open(&root).unwrap();
        assert!(fframes_path(&project).is_err(), "{reason}");
    }
    fs::write(root.join("Cargo.toml"), &cargo).unwrap();

    // A project pinned to any other SDK release is not built with this one.
    let mut manifest = current.manifest.clone();
    manifest.sdk.release = "studio-sdk-earlier".into();
    studio_project::lifecycle::write_manifest(&root, &manifest).unwrap();
    let project = studio_project::open(&root).unwrap();
    let error = fframes_path(&project).unwrap_err().to_string();
    assert!(error.contains("SDK pin mismatch"), "{error}");
}

/// Studio copies a project before every build. Unchanged sources must reuse the shared
/// target's compiled crates, and no build may ever run a binary compiled from other sources.
#[test]
fn unchanged_sources_reuse_the_build_and_changed_sources_never_run_a_stale_binary() {
    use studio_engine::build_materialization::{record_built_sources, reuse_unchanged_sources};
    let temp = tempfile::tempdir().unwrap();
    let sdk = temp.path().join("sdk");
    fs::create_dir_all(sdk.join("framework/vendor")).unwrap();
    let builds = temp.path().join("builds");
    let compatibility = CompatibilityManifest::default_linux_x64();
    let create = |name: &str| {
        let root = temp.path().join(name);
        studio_project::create(
            &root,
            "Video",
            sdk_pin(&compatibility),
            &compatibility.fframes_version,
            &compatibility.runtime_version,
        )
        .unwrap();
        // The starter project without the framework, so the host Cargo can build it.
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"studio-video\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )
        .unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            root.join("src/bin/studio_worker.rs"),
            "fn main() { print!(\"{}\", studio_video::text()); }\n",
        )
        .unwrap();
        root
    };
    let write_text = |root: &std::path::Path, text: &str| {
        fs::write(
            root.join("src/lib.rs"),
            format!("pub fn text() -> &'static str {{ \"{text}\" }}\n"),
        )
        .unwrap();
    };
    // One Studio build: copy, reuse, cargo, record. Returns what the worker printed and
    // whether Cargo compiled the project.
    let build = |root: &std::path::Path| {
        let project = studio_project::open(root).unwrap();
        let build = materialize(&project, &sdk, compatibility.clone(), &builds).unwrap();
        let reuse = reuse_unchanged_sources(&build, &project.inventory.files).unwrap();
        let output = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "--offline",
                "--bin",
                "studio_worker",
                "--manifest-path",
            ])
            .arg(&build.manifest)
            .env("CARGO_TARGET_DIR", &build.environment.target_dir)
            // A runner with colored Cargo output would split "Compiling studio-video".
            .env("CARGO_TERM_COLOR", "never")
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "{log}");
        record_built_sources(&build, &project.inventory.files, reuse).unwrap();
        let worker = build
            .environment
            .target_dir
            .join("debug")
            .join(format!("studio_worker{}", std::env::consts::EXE_SUFFIX));
        let printed = std::process::Command::new(worker).output().unwrap().stdout;
        (
            String::from_utf8(printed).unwrap(),
            log.contains("Compiling studio-video"),
        )
    };

    let first = create("first");
    write_text(&first, "A");
    assert_eq!(build(&first), ("A".into(), true));
    // Reopening, repeatedly: a build that compiled nothing must keep the next one fresh.
    assert_eq!(build(&first), ("A".into(), false));
    assert_eq!(build(&first), ("A".into(), false));
    write_text(&first, "B");
    assert_eq!(build(&first), ("B".into(), true));
    // Back to sources an earlier build compiled: the target holds B's crates now.
    write_text(&first, "A");
    assert_eq!(build(&first), ("A".into(), true));
    // Another project with the same package shares the target's crates.
    let second = create("second");
    write_text(&second, "C");
    assert_eq!(build(&second), ("C".into(), true));
    assert_eq!(build(&first), ("A".into(), true));
    assert_eq!(build(&first), ("A".into(), false));
}
