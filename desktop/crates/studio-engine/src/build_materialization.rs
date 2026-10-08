use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use studio_project::{
    OpenProject, ProjectError, ProjectPath, SourceInventory, lifecycle::read_cargo,
};
use studio_sdk::{CompatibilityManifest, environment::SdkEnvironment};

pub struct MaterializedBuild {
    pub root: PathBuf,
    pub environment: SdkEnvironment,
    pub package: String,
    pub worker_target: String,
    pub manifest: PathBuf,
    pub isolated_bin_dir: PathBuf,
    // The worker and any frame/audio consumers retain this lease. Never prune a live tree.
    _directory: tempfile::TempDir,
}

pub fn sdk_pin(manifest: &CompatibilityManifest) -> studio_project::manifest::SdkPin {
    studio_project::manifest::SdkPin {
        release: manifest.sdk_id.clone(),
        compatibility_sha256: manifest.digest(),
    }
}

/// SDK binding never mutates portable source, its lock/config, or the installed SDK.
pub fn materialize(
    project: &OpenProject,
    sdk: &Path,
    compatibility: CompatibilityManifest,
    app_builds: &Path,
) -> Result<MaterializedBuild, ProjectError> {
    materialize_with_cancel(project, sdk, compatibility, app_builds, &|| false)
}

/// Cargo target directory shared by every build with the same compatibility manifest.
pub fn target_dir(app_builds: &Path, compatibility: &CompatibilityManifest) -> PathBuf {
    app_builds
        .join("targets")
        .join(compatibility.digest())
        .join(&compatibility.target_triple)
}

pub fn materialize_with_cancel(
    project: &OpenProject,
    sdk: &Path,
    compatibility: CompatibilityManifest,
    app_builds: &Path,
    cancelled: &impl Fn() -> bool,
) -> Result<MaterializedBuild, ProjectError> {
    let sdk = fs::canonicalize(sdk).map_err(|e| {
        ProjectError::new(
            sdk,
            "managed build",
            e.to_string(),
            "Choose a compatible SDK or copy/relink dependencies inside the workspace",
        )
    })?;
    let target = target_dir(app_builds, &compatibility);
    let environment = SdkEnvironment::new(sdk, target, compatibility, true);
    materialize_in_environment(project, environment, app_builds, cancelled)
}

/// Materialize `project` for exactly `environment`: its (canonical) SDK directory, its
/// compatibility manifest and its target directory. The returned build keeps that
/// environment, so a frozen one is the very environment the compile and every worker use.
pub fn materialize_in_environment(
    project: &OpenProject,
    environment_binding: SdkEnvironment,
    app_builds: &Path,
    cancelled: &impl Fn() -> bool,
) -> Result<MaterializedBuild, ProjectError> {
    let sdk = environment_binding.sdk_dir.clone();
    let compatibility = environment_binding.manifest.clone();
    let error = |path: &Path, reason: String| {
        ProjectError::new(
            path,
            "managed build",
            reason,
            "Choose a compatible SDK or copy/relink dependencies inside the workspace",
        )
    };
    compatibility
        .validate()
        .map_err(|e| error(&sdk, e.to_string()))?;
    if project.manifest.sdk != sdk_pin(&compatibility) {
        return Err(error(&sdk, "SDK pin mismatch".into()));
    }
    if !project.worker_available {
        return Err(error(
            &project.root,
            "Worker bridge missing; add an explicit worker entry before building".into(),
        ));
    }
    for ancestor in project.root.ancestors().skip(1) {
        if ancestor.join(".cargo/config.toml").exists() || ancestor.join(".cargo/config").exists() {
            return Err(error(
                ancestor,
                "Inherited Cargo configuration is not portable".into(),
            ));
        }
    }
    for file in &project.inventory.files {
        if matches!(file.path.as_str(), ".cargo/config.toml" | ".cargo/config") {
            return Err(error(
                &project.root.join(file.path.as_str()),
                "Imported Cargo configuration conflicts with SDK binding; original retained".into(),
            ));
        }
    }
    let build_key = app_builds
        .join(String::from(project.manifest.project_id.clone()))
        .join(project.inventory.revision.as_str())
        .join(compatibility.digest())
        .join(&compatibility.target_triple);
    fs::create_dir_all(&build_key).map_err(|e| error(&build_key, e.to_string()))?;
    let staging = tempfile::Builder::new()
        .prefix("build-")
        .tempdir_in(&build_key)
        .map_err(|e| error(&build_key, e.to_string()))?;
    let root = staging.path().join("project");
    fs::create_dir(&root).map_err(|e| error(&root, e.to_string()))?;
    let mut buffer = [0; 64 * 1024];
    for source in &project.inventory.files {
        let target = root.join(source.path.as_str());
        fs::create_dir_all(target.parent().unwrap()).map_err(|e| error(&target, e.to_string()))?;
        let mut input = source.path.open_file(&project.root)?;
        let mut output = fs::File::create(&target).map_err(|e| error(&target, e.to_string()))?;
        loop {
            if cancelled() {
                return Err(error(&root, "Materialization cancelled".into()));
            }
            let n = input
                .read(&mut buffer)
                .map_err(|e| error(&target, e.to_string()))?;
            if n == 0 {
                break;
            }
            output
                .write_all(&buffer[..n])
                .map_err(|e| error(&target, e.to_string()))?;
        }
        #[cfg(unix)]
        if source.executable {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
                .map_err(|e| error(&target, e.to_string()))?;
        }
    }
    if SourceInventory::scan_with_cancel(&root, cancelled)?.revision != project.inventory.revision
        || SourceInventory::scan_with_cancel(&project.root, cancelled)?.revision
            != project.inventory.revision
    {
        return Err(error(
            &project.root,
            "Source changed during materialization; refresh and retry".into(),
        ));
    }
    for file in &project.inventory.files {
        if cancelled() {
            return Err(error(&root, "Materialization cancelled".into()));
        }
        if file.path.as_str().rsplit('/').next() != Some("Cargo.toml") {
            continue;
        }
        // Read exactly the captured Cargo bytes, not a later edit in the live source.
        let mut cargo = read_cargo(&root, &file.path)?;
        bind(
            &mut cargo,
            &project.root,
            &root,
            Path::new(file.path.as_str()).parent().unwrap(),
            &sdk,
            &compatibility,
        )?;
        let cargo_path = root.join(file.path.as_str());
        let cargo_text =
            toml::to_string_pretty(&cargo).map_err(|e| error(&cargo_path, e.to_string()))?;
        // This tree is private staging, not the user's portable project. Replace the
        // copied manifest directly: atomic rename-over-existing is not supported on Windows.
        fs::write(&cargo_path, cargo_text.as_bytes())
            .map_err(|e| error(&cargo_path, e.to_string()))?;
    }
    fs::create_dir_all(root.join(".cargo")).map_err(|e| error(&root, e.to_string()))?;
    let vendor = sdk.join("framework/vendor");
    if !vendor.is_dir() {
        return Err(error(&vendor, "SDK vendor directory missing".into()));
    }
    let cargo_config_path = root.join(".cargo/config.toml");
    let cargo_config = format!(
        "[source.crates-io]\nreplace-with = \"studio-vendor\"\n[source.studio-vendor]\ndirectory = {}\n",
        toml::Value::String(vendor.to_string_lossy().into_owned())
    );
    // This file is also inside the private staging tree; a failed write discards that tree.
    fs::write(&cargo_config_path, cargo_config.as_bytes())
        .map_err(|e| error(&cargo_config_path, e.to_string()))?;
    // SDK path substitution has a distinct graph/lock, never overwrite the portable lock.
    if root.join("Cargo.lock").exists() {
        fs::remove_file(root.join("Cargo.lock")).map_err(|e| error(&root, e.to_string()))?;
    }
    if SourceInventory::scan_with_cancel(&project.root, cancelled)?.revision
        != project.inventory.revision
    {
        return Err(error(
            &project.root,
            "Source changed during SDK binding; refresh and retry".into(),
        ));
    }
    let environment = environment_binding;
    let staging_path = staging.path();
    let isolated_bin_dir = staging_path.join("bin");
    fs::create_dir_all(&isolated_bin_dir).map_err(|e| error(&isolated_bin_dir, e.to_string()))?;
    let root = staging_path.join("project");
    Ok(MaterializedBuild {
        manifest: root.join(project.manifest.entry.manifest.as_str()),
        root,
        environment,
        package: project.manifest.entry.package.clone(),
        worker_target: project.manifest.entry.worker_target.clone(),
        isolated_bin_dir,
        _directory: staging,
    })
}

fn bind(
    value: &mut toml::Value,
    project_root: &Path,
    copied_root: &Path,
    package_dir: &Path,
    sdk: &Path,
    compatibility: &CompatibilityManifest,
) -> Result<(), ProjectError> {
    if let Some(table) = value.as_table_mut() {
        for (key, value) in table {
            if matches!(key.as_str(), "patch" | "replace") {
                return Err(ProjectError::new(
                    project_root.join(package_dir),
                    key,
                    "Cargo overrides need explicit SDK compatibility",
                    "Keep source unchanged and resolve the override before managed build",
                ));
            }
            if matches!(
                key.as_str(),
                "dependencies" | "dev-dependencies" | "build-dependencies"
            ) {
                if let Some(dependencies) = value.as_table_mut() {
                    for (name, dep) in dependencies {
                        let crate_name = dep
                            .get("package")
                            .and_then(toml::Value::as_str)
                            .unwrap_or(name)
                            .to_owned();
                        if let Some(path) = dep.get("path").and_then(toml::Value::as_str) {
                            let path_obj = Path::new(path);
                            let native = if path_obj.is_absolute() {
                                path_obj.to_path_buf()
                            } else {
                                project_root.join(package_dir).join(path)
                            };
                            let contained = fs::canonicalize(&native)
                                .ok()
                                .filter(|p| p.starts_with(project_root));
                            if contained.is_none() {
                                return Err(ProjectError::new(
                                    native,
                                    name,
                                    "external path dependency",
                                    "Copy this dependency into the project workspace and relink it explicitly",
                                ));
                            }
                            let relative = contained
                                .unwrap()
                                .strip_prefix(project_root)
                                .unwrap()
                                .to_string_lossy()
                                .replace('\\', "/");
                            if !relative.is_empty() {
                                ProjectPath::try_from(relative.clone())?
                                    .resolve_existing(project_root)?;
                            }
                            if path_obj.is_absolute() {
                                let rewritten = copied_root.join(&relative);
                                dep.as_table_mut().unwrap().insert(
                                    "path".into(),
                                    toml::Value::String(rewritten.to_string_lossy().into_owned()),
                                );
                            }
                        }
                        if matches!(
                            crate_name.as_str(),
                            "fframes" | "fframes-studio-runtime" | "fframes-studio-protocol"
                        ) && dep.get("workspace").and_then(toml::Value::as_bool) != Some(true)
                        {
                            let expected = if crate_name == "fframes" {
                                compatibility.fframes_version.as_str()
                            } else {
                                "0.1.0"
                            };
                            let version = dep
                                .as_str()
                                .or_else(|| dep.get("version").and_then(toml::Value::as_str));
                            if version.is_some_and(|v| v.trim_start_matches('=') != expected) {
                                return Err(ProjectError::new(
                                    project_root.join(package_dir),
                                    name,
                                    "dependency version does not match SDK",
                                    "Select an SDK matching this version",
                                ));
                            }
                            if dep.is_str() {
                                *dep = toml::Value::Table(toml::map::Map::new());
                            }
                            let framework = sdk.join("framework/framework").join(&crate_name);
                            if !framework.join("Cargo.toml").is_file() {
                                return Err(ProjectError::new(
                                    framework,
                                    name,
                                    "SDK crate missing",
                                    "Install the complete compatible SDK",
                                ));
                            }
                            dep.as_table_mut().unwrap().insert(
                                "path".into(),
                                toml::Value::String(framework.to_string_lossy().into_owned()),
                            );
                        }
                    }
                }
            } else {
                bind(
                    value,
                    project_root,
                    copied_root,
                    package_dir,
                    sdk,
                    compatibility,
                )?;
            }
        }
    }
    Ok(())
}
