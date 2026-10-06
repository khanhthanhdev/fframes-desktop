//! Real SDK preset snapshot reproducibility (compiler + project CLI). `#[ignore]`; it is
//! driven by `desktop/scripts/test-preset-relocation.py`, which prepares the SDK, sets the
//! environment and independently re-checks everything this test writes.
//!
//! Environment (required; the test PANICS, it does not skip):
//! * `SDK_ACTIVE`: an installed SDK directory whose `fframes` carries the `styles` feature.
//! * `PRESET_RELOCATION_OUT`: an empty directory receiving `report.json`, the rendered PNGs
//!   and a copy of the project's style/preset files at every stage.
//!
//! One fresh generated project renders the *same* `src/lib.rs` under two bundled presets
//! (different pixels), takes a project override and a Reapply, is closed and reopened,
//! then moved to another folder; the token/font/resource files and the rendered pixels are
//! recorded at every stage. A legacy project without styles must open, build and render,
//! and only change `style/` once a preset is applied.
use fframes_studio::{
    build_service::{CompileEnvironment, CompileRequest, Compiler},
    worker_project::{CargoCompiler, acquire_build_lock},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::{
    Controller, PresetAction, PresetRequest, app_paths::AppPaths, build_materialization::sdk_pin,
};
use studio_sdk::{CompatibilityManifest, ProjectManager};

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `media/<name> -> sha256` of the preset's flat top-level media files.
fn preset_media(root: &Path) -> Value {
    let mut out = serde_json::Map::new();
    let mut names: Vec<_> = fs::read_dir(root.join("media"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("preset-"))
        .collect();
    names.sort();
    for name in names {
        out.insert(
            format!("media/{name}"),
            json!(sha(&fs::read(root.join("media").join(&name)).unwrap())),
        );
    }
    Value::Object(out)
}

/// `path -> sha256` of every file under `root/prefix`.
fn hashes(root: &Path, prefix: &str) -> Value {
    fn walk(base: &Path, dir: &Path, out: &mut serde_json::Map<String, Value>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.map(|e| e.unwrap()).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if entry.file_type().unwrap().is_dir() {
                walk(base, &entry.path(), out);
            } else {
                let rel = entry
                    .path()
                    .strip_prefix(base)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .replace('\\', "/");
                out.insert(rel, json!(sha(&fs::read(entry.path()).unwrap())));
            }
        }
    }
    let mut out = serde_json::Map::new();
    walk(root, &root.join(prefix), &mut out);
    Value::Object(out)
}

struct Sdk {
    dir: PathBuf,
    manifest: CompatibilityManifest,
    builds: PathBuf,
    scope: ProcessTreeManager,
}

impl Sdk {
    /// Renders frame zero of the controller's current source with the project CLI.
    fn render(&self, controller: &Controller) -> Vec<u8> {
        let environment =
            CompileEnvironment::resolve(&self.dir, &self.manifest, &self.builds).unwrap();
        let build = CargoCompiler
            .compile(
                &CompileRequest {
                    project: controller.project.clone(),
                    environment,
                    retained: None,
                },
                &self.scope,
            )
            .unwrap_or_else(|e| panic!("compile failed: {e}"));
        let lock = acquire_build_lock(
            &build.environment.target_dir.join(".build_lock"),
            &self.scope,
            Duration::from_secs(300),
        )
        .unwrap();
        ProjectManager::build_project(&build.root, &build.environment, &self.scope)
            .unwrap_or_else(|e| panic!("project build failed: {e}"));
        let png_dir = build.isolated_bin_dir.join("preset-frame");
        let (_, png) =
            ProjectManager::render_frame(&build.root, &build.environment, 0, &png_dir, &self.scope)
                .unwrap_or_else(|e| panic!("render failed: {e}"));
        drop(lock);
        png
    }
}

impl Sdk {
    /// Runs the generated project's own `inspect` (exit 2 on a missing font or glyph; system
    /// fonts are off) against the current source. With `remove_preset_fonts` the preset's
    /// top-level font files are deleted from the build copy first, so the run can only
    /// succeed if the renderer really takes its font from them.
    fn inspect(&self, controller: &Controller, remove_preset_fonts: bool) -> (i32, String) {
        let environment =
            CompileEnvironment::resolve(&self.dir, &self.manifest, &self.builds).unwrap();
        let build = CargoCompiler
            .compile(
                &CompileRequest {
                    project: controller.project.clone(),
                    environment,
                    retained: None,
                },
                &self.scope,
            )
            .unwrap_or_else(|e| panic!("compile failed: {e}"));
        let lock = acquire_build_lock(
            &build.environment.target_dir.join(".build_lock"),
            &self.scope,
            Duration::from_secs(300),
        )
        .unwrap();
        ProjectManager::build_project(&build.root, &build.environment, &self.scope)
            .unwrap_or_else(|e| panic!("project build failed: {e}"));
        drop(lock);
        let media = build.root.join("media");
        let fonts: Vec<_> = fs::read_dir(&media)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "ttf"))
            .collect();
        assert_eq!(
            fonts.len(),
            1,
            "exactly the preset's font is in the project's top-level media: {fonts:?}"
        );
        if remove_preset_fonts {
            for font in fonts {
                fs::remove_file(font).unwrap();
            }
        }
        let log = build.root.join("inspect.log");
        let mut options = studio_bootstrap::SpawnOptions::new(
            build.environment.target_dir.join("debug/studio-video"),
        );
        options.args(["inspect", "--fail-on", "warning"]);
        options.current_dir(&build.root);
        options.env = build.environment.build_child_environment();
        options.stdout = std::process::Stdio::from(fs::File::create(&log).unwrap());
        options.stderr =
            std::process::Stdio::from(fs::File::create(build.root.join("inspect.err")).unwrap());
        let child = self.scope.spawn(options).unwrap();
        let status = child.lock().child_mut().wait().unwrap();
        let text = format!(
            "{}{}",
            fs::read_to_string(&log).unwrap(),
            fs::read_to_string(build.root.join("inspect.err")).unwrap()
        );
        (status.code().unwrap_or(-1), text)
    }
}

fn pixel_sha(png: &[u8]) -> String {
    sha(image::load_from_memory(png).unwrap().into_rgba8().as_raw())
}

struct Recorder {
    out: PathBuf,
    stages: Vec<Value>,
}

impl Recorder {
    fn stage(&mut self, name: &str, root: &Path, controller: &Controller, png: &[u8]) {
        let png_name = format!("{name}.png");
        let location = root
            .parent()
            .and_then(|parent| parent.file_name())
            .zip(root.file_name())
            .map(|(parent, name)| {
                format!("{}/{}", parent.to_string_lossy(), name.to_string_lossy())
            })
            .unwrap_or_else(|| root.file_name().unwrap().to_string_lossy().into_owned());
        fs::write(self.out.join(&png_name), png).unwrap();
        // The portable preset snapshot at this moment, for the script to re-hash.
        let snapshot = self.out.join("stages").join(name);
        copy_tree(&root.join("style"), &snapshot.join("style"));
        fs::create_dir_all(snapshot.join("media")).unwrap();
        for name in preset_media(root).as_object().unwrap().keys() {
            fs::copy(root.join(name), snapshot.join(name)).unwrap();
        }
        fs::copy(root.join("studio.json"), snapshot.join("studio.json")).unwrap();
        let style = controller.project_style();
        self.stages.push(json!({
            "name": name,
            "root": location,
            "source_revision": controller.state().source().as_str(),
            "preset": style.as_ref().map(|s| s.identity.id.clone()),
            "preset_hash": style.as_ref().map(|s| s.identity.hash.clone()),
            "overrides": style.as_ref().map(|s| s.overridden.len()),
            "png": png_name,
            "png_sha256": sha(png),
            "pixel_sha256": pixel_sha(png),
            "tokens": hashes(root, "style"),
            "resources": preset_media(root),
            "lib_rs": sha(&fs::read(root.join("src/lib.rs")).unwrap()),
        }));
    }
}

fn apply(controller: &mut Controller, request: PresetRequest<'_>) {
    let source = controller.state().source().clone();
    let outcome = controller
        .apply_preset(&source, request)
        .unwrap_or_else(|e| panic!("preset mutation failed: {e}"));
    assert!(outcome.record.is_some(), "expected a committed revision");
}

fn select(controller: &mut Controller, id: &str, action: PresetAction) {
    let package = studio_presets::builtin::get(id).expect("bundled preset");
    apply(controller, PresetRequest::Select { package, action });
}

#[test]
#[ignore = "needs SDK_ACTIVE and PRESET_RELOCATION_OUT; run via desktop/scripts/test-preset-relocation.py"]
fn real_sdk_preset_snapshots_render_distinctly_and_survive_reopen_and_relocation() {
    let dir = PathBuf::from(std::env::var_os("SDK_ACTIVE").expect("SDK_ACTIVE"));
    let out =
        PathBuf::from(std::env::var_os("PRESET_RELOCATION_OUT").expect("PRESET_RELOCATION_OUT"));
    fs::create_dir_all(out.join("stages")).unwrap();
    let manifest = CompatibilityManifest::from_json_str(
        &fs::read_to_string(dir.join("compatibility.json")).unwrap(),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let sdk = Sdk {
        dir,
        builds: temp.path().join("builds"),
        scope: ProcessTreeManager::new(),
        manifest: manifest.clone(),
    };
    let paths = AppPaths::new(temp.path().join("data")).unwrap();
    let mut recorder = Recorder {
        out: out.clone(),
        stages: Vec::new(),
    };

    // ---- a fresh generated project, the same source under two presets -------------------
    let first = temp.path().join("first");
    fs::create_dir_all(&first).unwrap();
    let root = first.join("video");
    studio_project::create(
        &root,
        "Preset relocation",
        sdk_pin(&manifest),
        &manifest.fframes_version,
        "0.1.0",
    )
    .unwrap();
    // The scaffold ships its own DM Sans in `media/`; drop it (and its manifest entry) so the
    // only font this project can render with is the one a preset brings.
    fs::remove_file(root.join("media/DMSans-Medium.ttf")).unwrap();
    let mut project_manifest = studio_project::open_for_recovery(&root).unwrap().manifest;
    project_manifest
        .assets
        .retain(|asset| asset.as_str() != "media/DMSans-Medium.ttf");
    studio_project::lifecycle::write_manifest(&root, &project_manifest).unwrap();
    assert!(studio_project::open(&root).is_ok());
    let lib_before = fs::read(root.join("src/lib.rs")).unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();

    select(&mut controller, "editorial", PresetAction::Apply);
    let editorial = sdk.render(&controller);
    recorder.stage("editorial", &root, &controller, &editorial);
    let mut inspections = serde_json::Map::new();
    let mut inspect = |label: &str, c: &Controller, remove: bool| {
        let (code, text) = sdk.inspect(c, remove);
        inspections.insert(label.to_owned(), json!({"exit": code, "output": text}));
    };
    inspect("editorial_with_font", &controller, false);
    inspect("editorial_font_removed", &controller, true);

    select(&mut controller, "pulse", PresetAction::Apply);
    let pulse = sdk.render(&controller);
    recorder.stage("pulse", &root, &controller, &pulse);
    inspect("pulse_with_font", &controller, false);
    assert_ne!(
        pixel_sha(&editorial),
        pixel_sha(&pulse),
        "two presets must render differently"
    );

    // ---- a project override and a Reapply ------------------------------------------------
    apply(
        &mut controller,
        PresetRequest::SetOverride {
            name: "color.background",
            value: &json!("#ff00aa"),
        },
    );
    // Reapplying the same package is a no-op for the files and keeps the override.
    let source = controller.state().source().clone();
    let reapplied = controller
        .apply_preset(
            &source,
            PresetRequest::Select {
                package: studio_presets::builtin::get("pulse").unwrap(),
                action: PresetAction::Reapply,
            },
        )
        .unwrap();
    assert!(reapplied.record.is_none());
    let overridden = sdk.render(&controller);
    recorder.stage("overridden", &root, &controller, &overridden);
    assert_ne!(
        pixel_sha(&pulse),
        pixel_sha(&overridden),
        "the override must show"
    );
    assert_eq!(fs::read(root.join("src/lib.rs")).unwrap(), lib_before);

    // ---- close and reopen -----------------------------------------------------------------
    controller.close().unwrap();
    drop(controller);
    let controller = Controller::open(&root, &paths).unwrap();
    assert_eq!(controller.project_style().unwrap().overridden.len(), 1);
    let reopened = sdk.render(&controller);
    recorder.stage("reopened", &root, &controller, &reopened);
    drop(controller);

    // ---- relocate (the old location no longer exists) ------------------------------------
    let second = temp.path().join("elsewhere");
    fs::create_dir_all(&second).unwrap();
    let moved = second.join("renamed-video");
    fs::rename(&root, &moved).unwrap();
    let controller = Controller::open(&moved, &paths).unwrap();
    assert_eq!(controller.project_style().unwrap().identity.id, "pulse");
    let relocated = sdk.render(&controller);
    recorder.stage("relocated", &moved, &controller, &relocated);
    drop(controller);

    // ---- switching to a preset with another font file replaces the first one ---------------
    let mut controller = Controller::open(&moved, &paths).unwrap();
    select(&mut controller, "quiet-motion", PresetAction::Apply);
    assert!(!moved.join("media/preset-fonts-DMSans-Medium.ttf").exists());
    assert!(
        moved
            .join("media/preset-fonts-DMSans-Regular.ttf")
            .is_file()
    );
    inspect("quiet_motion_with_font", &controller, false);
    inspect("quiet_motion_font_removed", &controller, true);
    drop(controller);

    // ---- legacy project: no style files, builds and opens untouched ----------------------
    let legacy_root = temp.path().join("legacy").join("video");
    fs::create_dir_all(legacy_root.parent().unwrap()).unwrap();
    studio_project::create(
        &legacy_root,
        "Legacy",
        sdk_pin(&manifest),
        &manifest.fframes_version,
        "0.1.0",
    )
    .unwrap();
    fs::remove_dir_all(legacy_root.join("style")).unwrap();
    fs::write(
        legacy_root.join("Cargo.toml"),
        include_str!("fixtures/pre_m4_project/Cargo.toml"),
    )
    .unwrap();
    for (relative, contents) in [
        (
            "src/lib.rs",
            include_str!("fixtures/pre_m4_project/src/lib.rs"),
        ),
        (
            "src/main.rs",
            include_str!("fixtures/pre_m4_project/src/main.rs"),
        ),
        (
            "src/bin/studio_worker.rs",
            include_str!("fixtures/pre_m4_project/src/bin/studio_worker.rs"),
        ),
    ] {
        fs::write(legacy_root.join(relative), contents).unwrap();
    }
    assert!(
        !fs::read_to_string(legacy_root.join("Cargo.toml"))
            .unwrap()
            .contains("styles"),
        "the legacy Cargo manifest is the pre-M4 template"
    );
    let mut legacy = Controller::open(&legacy_root, &paths).unwrap();
    assert!(legacy.project_style().is_none());
    assert!(!legacy_root.join("style").exists());
    let legacy_png = sdk.render(&legacy);
    fs::write(out.join("legacy.png"), &legacy_png).unwrap();
    let legacy_lib = fs::read(legacy_root.join("src/lib.rs")).unwrap();
    let legacy_cargo = fs::read(legacy_root.join("Cargo.toml")).unwrap();
    // Choosing a preset later adds the style snapshot and rewrites no Rust or Cargo source.
    select(&mut legacy, "quiet-motion", PresetAction::Apply);
    assert!(legacy_root.join("style/tokens.json").is_file());
    assert_eq!(
        fs::read(legacy_root.join("src/lib.rs")).unwrap(),
        legacy_lib
    );
    assert_eq!(
        fs::read(legacy_root.join("Cargo.toml")).unwrap(),
        legacy_cargo
    );
    let legacy_after = sdk.render(&legacy);

    let report = json!({
        "sdk_id": manifest.sdk_id,
        "stages": recorder.stages,
        "inspections": inspections,
        "legacy": {
            "opened_without_styles": true,
            "template": "pre-m4",
            "png_sha256": sha(&legacy_png),
            "pixel_sha256": pixel_sha(&legacy_png),
            "pixel_sha256_after_preset": pixel_sha(&legacy_after),
            "lib_rs_unchanged": true,
            "lib_rs_sha256": sha(&legacy_lib),
            "cargo_unchanged": true,
            "cargo_sha256": sha(&legacy_cargo),
        },
    });
    fs::write(
        out.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}
