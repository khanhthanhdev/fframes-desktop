use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::{
    Manifest, ProjectError, ProjectPath, SourceInventory,
    manifest::{
        CURRENT_INSTRUCTION_VERSION, CargoEntry, DisplayMetadata, PresetReference, SCHEMA_VERSION,
        SdkPin, VideoHints,
    },
    paths::io_error,
};

#[derive(Debug, Clone)]
pub struct OpenProject {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub inventory: SourceInventory,
    /// Import never inserts a worker into existing source implicitly.
    pub worker_available: bool,
    pub missing_assets: Vec<ProjectPath>,
}

pub fn read_cargo(root: &Path, path: &ProjectPath) -> Result<toml::Value, ProjectError> {
    let mut bytes = String::new();
    path.open_file(root)?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut bytes)
        .map_err(|e| io_error(root, e))?;
    if bytes.len() > 1024 * 1024 {
        return Err(io_error(root, "Cargo manifest exceeds 1 MiB"));
    }
    bytes
        .parse()
        .map_err(|e| ProjectError::new(root.join(path.as_str()), "Cargo", e, "Correct Cargo.toml"))
}

fn validate_rust_entry(
    root: &Path,
    manifest: &ProjectPath,
    cargo: &toml::Value,
) -> Result<(), ProjectError> {
    let package_dir = Path::new(manifest.as_str()).parent().unwrap();
    let mut candidates = vec![
        cargo
            .get("lib")
            .and_then(|l| l.get("path"))
            .and_then(toml::Value::as_str)
            .unwrap_or("src/lib.rs")
            .to_owned(),
        "src/main.rs".to_owned(),
    ];
    if let Some(bins) = cargo.get("bin").and_then(toml::Value::as_array) {
        for bin in bins {
            if let Some(path) = bin.get("path").and_then(toml::Value::as_str) {
                candidates.push(path.to_owned());
            } else if let Some(name) = bin.get("name").and_then(toml::Value::as_str) {
                candidates.push(format!("src/bin/{name}.rs"));
                candidates.push(format!("src/bin/{name}/main.rs"));
            }
        }
    }
    for candidate in candidates {
        let path = ProjectPath::try_from(
            package_dir
                .join(candidate)
                .to_string_lossy()
                .replace('\\', "/"),
        )?;
        if root.join(path.as_str()).exists() {
            path.open_file(root)?;
            return Ok(());
        }
    }
    Err(ProjectError::new(
        root,
        "entry",
        "Rust entry source missing",
        "Locate or restore the selected package source",
    ))
}

/// Reads metadata and source without invoking Cargo, build scripts or SDK installation.
pub fn open(root: &Path) -> Result<OpenProject, ProjectError> {
    let root = fs::canonicalize(root).map_err(|e| io_error(root, e))?;
    let mut bytes = Vec::new();
    ProjectPath::try_from("studio.json".to_owned())?
        .open_file(&root)?
        .take(crate::manifest::MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(&root, e))?;
    let manifest = Manifest::parse(&bytes).map_err(|mut e| {
        e.file = root.join("studio.json");
        e
    })?;
    let cargo = read_cargo(&root, &manifest.entry.manifest)?;
    if cargo
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        != Some(&manifest.entry.package)
    {
        return Err(ProjectError::new(
            root.join(manifest.entry.manifest.as_str()),
            "package.name",
            "selected package does not match",
            "Select the package Cargo.toml, not a virtual workspace",
        ));
    }
    let package_dir = Path::new(manifest.entry.manifest.as_str())
        .parent()
        .unwrap();
    validate_rust_entry(&root, &manifest.entry.manifest, &cargo)?;
    for asset in &manifest.assets {
        asset.open_file(&root).map_err(|mut error| {
            error.field = "asset".into();
            error.action =
                "Locate or restore this asset, or repair its permissions, then reopen".into();
            error
        })?;
    }
    let worker = package_dir.join(format!("src/bin/{}.rs", manifest.entry.worker_target));
    let worker_available = root.join(worker).is_file()
        || cargo
            .get("bin")
            .and_then(toml::Value::as_array)
            .is_some_and(|bins| {
                bins.iter().any(|b| {
                    b.get("name").and_then(toml::Value::as_str)
                        == Some(&manifest.entry.worker_target)
                })
            });
    let inventory = SourceInventory::scan(&root)?;
    Ok(OpenProject {
        root,
        manifest,
        inventory,
        worker_available,
        missing_assets: Vec::new(),
    })
}

/// Reads metadata and inventory for recovery export without failing on missing assets or workers.
pub fn open_for_recovery(root: &Path) -> Result<OpenProject, ProjectError> {
    let root = fs::canonicalize(root).map_err(|e| io_error(root, e))?;
    let mut bytes = Vec::new();
    ProjectPath::try_from("studio.json".to_owned())?
        .open_file(&root)?
        .take(crate::manifest::MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_error(&root, e))?;
    let manifest = Manifest::parse(&bytes).map_err(|mut e| {
        e.file = root.join("studio.json");
        e
    })?;
    let mut missing_assets = Vec::new();
    for asset in &manifest.assets {
        if asset.open_file(&root).is_err() {
            missing_assets.push(asset.clone());
        }
    }
    let cargo = read_cargo(&root, &manifest.entry.manifest).ok();
    let package_dir = Path::new(manifest.entry.manifest.as_str())
        .parent()
        .unwrap_or(Path::new(""));
    let worker = package_dir.join(format!("src/bin/{}.rs", manifest.entry.worker_target));
    let worker_available = root.join(&worker).is_file()
        || cargo
            .as_ref()
            .and_then(|c| c.get("bin"))
            .and_then(toml::Value::as_array)
            .is_some_and(|bins| {
                bins.iter().any(|b| {
                    b.get("name").and_then(toml::Value::as_str)
                        == Some(&manifest.entry.worker_target)
                })
            });
    let inventory = SourceInventory::scan(&root)?;
    Ok(OpenProject {
        root,
        manifest,
        inventory,
        worker_available,
        missing_assets,
    })
}

/// Same-directory durable replacement; callers serialize mutations and compare source first.
pub fn write_manifest(root: &Path, manifest: &Manifest) -> Result<(), ProjectError> {
    manifest.validate()?;
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|e| io_error(root, e))?;
    atomic_write(&root.join("studio.json"), &bytes)
}

/// Explicit user action for a duplicate/restored copy. Preserve the original sidecar first.
pub fn assign_independent_identity(root: &Path) -> Result<OpenProject, ProjectError> {
    let project = open(root)?;
    let original = fs::read(root.join("studio.json")).map_err(|e| io_error(root, e))?;
    let backup = root.join(format!("studio-manifest-{}.backup", uuid::Uuid::new_v4()));
    atomic_write(&backup, &original)?;
    let mut manifest = project.manifest;
    manifest.project_id = uuid::Uuid::new_v4().to_string().try_into().unwrap();
    write_manifest(root, &manifest)?;
    open(root)
}

pub fn sync_directory(path: &Path) -> Result<(), ProjectError> {
    #[cfg(unix)]
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| io_error(path, e))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), ProjectError> {
    let parent = path
        .parent()
        .ok_or_else(|| io_error(path, "missing parent"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| io_error(parent, e))?;
    tmp.write_all(bytes)
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| io_error(path, e))?;
    tmp.persist(path).map_err(|e| io_error(path, e))?;
    sync_directory(parent)
}

/// The exact framework releases an SDK provides, which a project's Cargo manifests name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameworkVersions<'a> {
    pub fframes: &'a str,
    /// `fframes-studio-runtime`, `fframes-studio-protocol` and `fframes-studio-kit`.
    pub runtime: &'a str,
    /// The crates.io `fframes-kit`, when the SDK vendors one.
    pub kit: Option<&'a str>,
}

impl FrameworkVersions<'_> {
    fn for_crate(&self, name: &str) -> Option<&str> {
        match name {
            "fframes" => Some(self.fframes),
            "fframes-studio-runtime" | "fframes-studio-protocol" | "fframes-studio-kit" => {
                Some(self.runtime)
            }
            "fframes-kit" => self.kit,
            _ => None,
        }
    }
}

/// Moves the project at `root` to another SDK: every exact framework version in its Cargo
/// manifests becomes `versions`' release and `studio.json` is pinned to `sdk`. Comments and
/// formatting of the manifests are kept, and dependencies inherited from a workspace, or
/// given without a version, are left alone. Every manifest is parsed before anything is
/// written, so a manifest that does not parse changes nothing.
pub fn upgrade_framework(
    root: &Path,
    sdk: SdkPin,
    versions: FrameworkVersions<'_>,
) -> Result<(), ProjectError> {
    let project = open(root)?;
    let mut rewritten = Vec::new();
    for file in &project.inventory.files {
        let path = &file.path;
        if Path::new(path.as_str()).file_name() != Some("Cargo.toml".as_ref()) {
            continue;
        }
        let mut text = String::new();
        path.open_file(&project.root)?
            .take(1024 * 1024 + 1)
            .read_to_string(&mut text)
            .map_err(|e| io_error(&project.root, e))?;
        let native = project.root.join(path.as_str());
        if text.len() > 1024 * 1024 {
            return Err(io_error(&native, "Cargo manifest exceeds 1 MiB"));
        }
        let mut document: toml_edit::DocumentMut = text
            .parse()
            .map_err(|e| ProjectError::new(&native, "Cargo", e, "Correct Cargo.toml"))?;
        if set_framework_versions(document.as_item_mut(), versions) {
            rewritten.push((native, document.to_string()));
        }
    }
    for (path, text) in rewritten {
        atomic_write(&path, text.as_bytes())?;
    }
    let mut manifest = project.manifest;
    manifest.sdk = sdk;
    write_manifest(&project.root, &manifest)
}

/// Sets the framework dependencies in every dependency table under `item` (including
/// `[workspace.dependencies]` and `[target.*.dependencies]`); whether one changed.
fn set_framework_versions(item: &mut toml_edit::Item, versions: FrameworkVersions<'_>) -> bool {
    let Some(table) = item.as_table_like_mut() else {
        return false;
    };
    let mut changed = false;
    for (key, value) in table.iter_mut() {
        if !matches!(
            key.get(),
            "dependencies" | "dev-dependencies" | "build-dependencies"
        ) {
            changed |= set_framework_versions(value, versions);
            continue;
        }
        let Some(dependencies) = value.as_table_like_mut() else {
            continue;
        };
        for (name, dependency) in dependencies.iter_mut() {
            let crate_name = dependency
                .get("package")
                .and_then(toml_edit::Item::as_str)
                .unwrap_or(name.get())
                .to_owned();
            let Some(release) = versions.for_crate(&crate_name) else {
                continue;
            };
            let version = if dependency.is_str() {
                dependency.as_value_mut()
            } else {
                dependency
                    .as_table_like_mut()
                    .and_then(|table| table.get_mut("version"))
                    .and_then(toml_edit::Item::as_value_mut)
            };
            let wanted = format!("={release}");
            if let Some(version) = version.filter(|v| v.as_str() != Some(&wanted)) {
                let decor = version.decor().clone();
                *version = toml_edit::Value::from(wanted);
                *version.decor_mut() = decor;
                changed = true;
            }
        }
    }
    changed
}

/// What a new project starts from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ProjectTemplate {
    /// Three title scenes drawn with design tokens.
    #[default]
    Blank,
    /// A six-slide deck built with fframes-kit's `slides` module, with slide stops and
    /// speaker notes in `slides.json` and slide text editable in the Props panel.
    SlideDeck,
}

impl ProjectTemplate {
    pub const ALL: [ProjectTemplate; 2] = [ProjectTemplate::Blank, ProjectTemplate::SlideDeck];

    /// The exact fframes-kit release the template depends on; the SDK must vendor it.
    pub fn kit_version(self) -> Option<&'static str> {
        match self {
            ProjectTemplate::Blank => None,
            ProjectTemplate::SlideDeck => Some(SLIDES_KIT_VERSION),
        }
    }
}

/// The fframes-kit release of the Slide deck template (`fframes-studio-kit`'s, which the
/// SDK vendors).
pub const SLIDES_KIT_VERSION: &str = "0.2.1";

/// Scaffold exact portable versions. Managed SDK paths are bound only in build materialization.
pub fn create(
    destination: &Path,
    name: &str,
    sdk: SdkPin,
    fframes_version: &str,
    runtime_version: &str,
) -> Result<OpenProject, ProjectError> {
    create_from_template(
        destination,
        name,
        sdk,
        fframes_version,
        runtime_version,
        ProjectTemplate::Blank,
    )
}

/// The Slide deck's own files; the media are the Blank template's.
const SLIDES_FILES: [(&str, &[u8]); 7] = [
    (
        "src/lib.rs",
        include_bytes!("../templates-slides/src/lib.rs"),
    ),
    (
        "src/video.rs",
        include_bytes!("../templates-slides/src/video.rs"),
    ),
    (
        "src/main.rs",
        include_bytes!("../templates-slides/src/main.rs"),
    ),
    (
        "src/bin/studio_worker.rs",
        include_bytes!("../templates-slides/src/bin/studio_worker.rs"),
    ),
    (
        "style/tokens.json",
        include_bytes!("../templates-slides/style/tokens.json"),
    ),
    ("AGENTS.md", include_bytes!("../templates-slides/AGENTS.md")),
    (
        "slides.json",
        include_bytes!("../templates-slides/slides.json"),
    ),
];

/// How a new project is set up, besides its folder, name and SDK.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectSetup {
    pub template: ProjectTemplate,
    /// The frame size and rate written into `src/video.rs` and `studio.json`'s hints. A
    /// Slide deck draws on a 16:9 canvas, so its size must be 16:9.
    pub format: VideoHints,
    /// The style preset the project starts with, instead of the template's starter tokens.
    pub style: Option<StarterStyle>,
}

/// A style preset's materialized project files (`style/*`, `media/preset-*`) and the
/// reference `studio.json` keeps to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarterStyle {
    pub reference: PresetReference,
    pub files: Vec<(ProjectPath, Vec<u8>)>,
}

/// `source` (a template's `src/video.rs`) with `format`'s frame rate and size; the
/// viewBox follows the size only when the scenes draw on the video's own frame.
fn with_format(source: &[u8], format: &VideoHints, view_box: bool) -> Vec<u8> {
    let mut text = String::from_utf8(source.to_vec()).expect("templates are UTF-8");
    let mut set = |from: &str, to: String| {
        assert!(text.contains(from), "template lacks `{from}`");
        text = text.replacen(from, &to, 1);
    };
    set(
        "const FPS: usize = 30;",
        format!("const FPS: usize = {};", format.fps),
    );
    set(
        "const WIDTH: usize = 1920;",
        format!("const WIDTH: usize = {};", format.width),
    );
    set(
        "const HEIGHT: usize = 1080;",
        format!("const HEIGHT: usize = {};", format.height),
    );
    if view_box {
        set(
            r#"viewBox="0 0 1920 1080""#,
            format!(r#"viewBox="0 0 {} {}""#, format.width, format.height),
        );
    }
    text.into_bytes()
}

/// The Slide deck's `slides.json`, written for 30 fps, with its frame stops moved to `fps`
/// (rounded up, as the template's stops are).
fn slides_at_fps(source: &[u8], fps: u32) -> Vec<u8> {
    if fps == 30 {
        return source.to_vec();
    }
    let mut slides: serde_json::Value = serde_json::from_slice(source).expect("valid template");
    for stop in slides["stops"].as_array_mut().expect("template stops") {
        if let Some(frame) = stop["at"].as_u64() {
            stop["at"] = (frame * u64::from(fps)).div_ceil(30).into();
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&slides).expect("serializes");
    bytes.push(b'\n');
    bytes
}

/// [`create`] from `template`.
pub fn create_from_template(
    destination: &Path,
    name: &str,
    sdk: SdkPin,
    fframes_version: &str,
    runtime_version: &str,
    template: ProjectTemplate,
) -> Result<OpenProject, ProjectError> {
    create_with_setup(
        destination,
        name,
        sdk,
        fframes_version,
        runtime_version,
        &ProjectSetup {
            template,
            ..ProjectSetup::default()
        },
    )
}

/// [`create`] from `setup`'s template, at its format, with its style.
pub fn create_with_setup(
    destination: &Path,
    name: &str,
    sdk: SdkPin,
    fframes_version: &str,
    runtime_version: &str,
    setup: &ProjectSetup,
) -> Result<OpenProject, ProjectError> {
    let ProjectSetup {
        template,
        format,
        style,
    } = setup;
    let template = *template;
    if template == ProjectTemplate::SlideDeck && format.width * 9 != format.height * 16 {
        return Err(ProjectError::new(
            destination,
            "video_hints",
            "a slide deck is drawn on a 16:9 canvas",
            "Choose a 16:9 size for a slide deck",
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| io_error(destination, "missing parent"))?;
    let staging = tempfile::Builder::new()
        .prefix(".studio-create-")
        .tempdir_in(parent)
        .map_err(|e| io_error(parent, e))?;
    let root = staging.path();
    fs::create_dir_all(root.join("src/bin")).map_err(|e| io_error(root, e))?;
    fs::create_dir(root.join("media")).map_err(|e| io_error(root, e))?;
    fs::create_dir(root.join("style")).map_err(|e| io_error(root, e))?;
    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        project_id: uuid::Uuid::new_v4().to_string().try_into().unwrap(),
        display: DisplayMetadata {
            name: name.into(),
            description: None,
        },
        sdk,
        entry: CargoEntry {
            manifest: "Cargo.toml".to_owned().try_into()?,
            package: "studio-video".into(),
            worker_target: "studio_worker".into(),
        },
        assets: vec![
            "media/DMSans-Medium.ttf".to_owned().try_into()?,
            "media/OFL.txt".to_owned().try_into()?,
        ],
        generated_instruction_version: CURRENT_INSTRUCTION_VERSION,
        video_hints: Some(format.clone()),
        preset: style.as_ref().map(|style| style.reference.clone()),
    };
    manifest.validate()?;
    // Parse the assembled TOML before writing; versions cannot inject extra declarations.
    let (cargo, runtime_crates): (&str, &[&str]) = match template {
        ProjectTemplate::Blank => (
            include_str!("../templates/Cargo.toml"),
            &["fframes-studio-runtime", "fframes-studio-protocol"],
        ),
        ProjectTemplate::SlideDeck => (
            include_str!("../templates-slides/Cargo.toml"),
            &[
                "fframes-studio-runtime",
                "fframes-studio-protocol",
                "fframes-studio-kit",
            ],
        ),
    };
    let mut cargo: toml::Value = cargo.parse().unwrap();
    cargo["dependencies"]["fframes"]["version"] =
        toml::Value::String(format!("={fframes_version}"));
    for dep in runtime_crates {
        cargo["dependencies"][dep]["version"] = toml::Value::String(format!("={runtime_version}"));
    }
    let blank: [(&str, &[u8]); 9] = [
        ("src/lib.rs", include_bytes!("../templates/src/lib.rs")),
        ("src/video.rs", include_bytes!("../templates/src/video.rs")),
        ("src/main.rs", include_bytes!("../templates/src/main.rs")),
        (
            "src/bin/studio_worker.rs",
            include_bytes!("../templates/src/bin/studio_worker.rs"),
        ),
        (
            "style/tokens.json",
            include_bytes!("../templates/style/tokens.json"),
        ),
        ("AGENTS.md", include_bytes!("../templates/AGENTS.md")),
        (".gitignore", b"/target/\n/out.*\n"),
        (
            "media/DMSans-Medium.ttf",
            include_bytes!("../../../fixtures/annotated-video-overlay/media/DMSans-Medium.ttf"),
        ),
        (
            "media/OFL.txt",
            include_bytes!("../../../fixtures/annotated-video-overlay/media/OFL.txt"),
        ),
    ];
    let files: Vec<(&str, &[u8])> = match template {
        ProjectTemplate::Blank => blank.to_vec(),
        // The same .gitignore and media, the deck's sources and documents.
        ProjectTemplate::SlideDeck => blank
            .into_iter()
            .filter(|(path, _)| path.starts_with("media/") || *path == ".gitignore")
            .chain(SLIDES_FILES)
            .collect(),
    };
    for (path, bytes) in files {
        let bytes = match path {
            "src/video.rs" => with_format(bytes, format, template == ProjectTemplate::Blank),
            "slides.json" => slides_at_fps(bytes, format.fps),
            _ => bytes.to_vec(),
        };
        atomic_write(&root.join(path), &bytes)?;
    }
    // The preset's files replace the starter style/tokens.json.
    for (path, bytes) in style.iter().flat_map(|style| &style.files) {
        let path = root.join(path.as_str());
        fs::create_dir_all(path.parent().expect("a project file has a parent"))
            .map_err(|e| io_error(root, e))?;
        atomic_write(&path, bytes)?;
    }
    atomic_write(
        &root.join("Cargo.toml"),
        toml::to_string_pretty(&cargo).unwrap().as_bytes(),
    )?;
    write_manifest(root, &manifest)?;
    sync_directory(&root.join("src/bin"))?;
    sync_directory(&root.join("src"))?;
    sync_directory(&root.join("media"))?;
    sync_directory(&root.join("style"))?;
    // rename can replace an empty directory but never a nonempty destination.
    if destination.exists()
        && fs::read_dir(destination)
            .map_err(|e| io_error(destination, e))?
            .next()
            .is_some()
    {
        return Err(ProjectError::new(
            destination,
            "destination",
            "folder is not empty",
            "Choose an empty or new folder",
        ));
    }
    fs::rename(root, destination).map_err(|e| io_error(destination, e))?;
    sync_directory(parent)?;
    open(destination)
}

/// Intentional additive sidecar import. Existing Cargo files, instructions and Git stay untouched.
pub fn import(root: &Path, entry: CargoEntry, sdk: SdkPin) -> Result<OpenProject, ProjectError> {
    if root.join("studio.json").exists() {
        return open(root);
    }
    let cargo = read_cargo(root, &entry.manifest)?;
    let name = cargo
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| {
            ProjectError::new(
                root,
                "entry",
                "ambiguous or virtual workspace",
                "Choose a package Cargo.toml inside this workspace",
            )
        })?;
    if name != entry.package {
        return Err(io_error(root, "selected package mismatch"));
    }
    validate_rust_entry(root, &entry.manifest, &cargo)?;
    SourceInventory::scan(root)?;
    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        project_id: uuid::Uuid::new_v4().to_string().try_into().unwrap(),
        display: DisplayMetadata {
            name: name.into(),
            description: None,
        },
        sdk,
        entry,
        assets: vec![],
        generated_instruction_version: CURRENT_INSTRUCTION_VERSION,
        video_hints: None,
        preset: None,
    };
    manifest.validate()?;
    let mut file = tempfile::NamedTempFile::new_in(root).map_err(|e| io_error(root, e))?;
    file.write_all(&serde_json::to_vec_pretty(&manifest).unwrap())
        .and_then(|_| file.as_file().sync_all())
        .map_err(|e| io_error(root, e))?;
    file.persist_noclobber(root.join("studio.json"))
        .map_err(|e| io_error(root, e))?;
    sync_directory(root)?;
    open(root)
}
