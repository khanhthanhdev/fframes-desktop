use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::{
    Manifest, ProjectError, ProjectPath, SourceInventory,
    manifest::{CargoEntry, DisplayMetadata, SCHEMA_VERSION, SdkPin},
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

/// Scaffold exact portable versions. Managed SDK paths are bound only in build materialization.
pub fn create(
    destination: &Path,
    name: &str,
    sdk: SdkPin,
    fframes_version: &str,
    runtime_version: &str,
) -> Result<OpenProject, ProjectError> {
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
        generated_instruction_version: 1,
        video_hints: None,
        preset: None,
    };
    manifest.validate()?;
    // Parse the assembled TOML before writing; versions cannot inject extra declarations.
    let mut cargo: toml::Value = include_str!("../templates/Cargo.toml").parse().unwrap();
    cargo["dependencies"]["fframes"]["version"] =
        toml::Value::String(format!("={fframes_version}"));
    for dep in ["fframes-studio-runtime", "fframes-studio-protocol"] {
        cargo["dependencies"][dep]["version"] = toml::Value::String(format!("={runtime_version}"));
    }
    let files: [(&str, &[u8]); 8] = [
        ("src/lib.rs", include_bytes!("../templates/src/lib.rs")),
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
    for (path, bytes) in files {
        atomic_write(&root.join(path), bytes)?;
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
        generated_instruction_version: 1,
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
