use std::{
    collections::BTreeMap,
    fs, io,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::SystemTime,
};

use studio_project::{
    OpenProject, ProjectError, ProjectPath, SourceInventory,
    lifecycle::read_cargo,
    revision::{FileKind, SourceFile, run_time_document},
};
use studio_sdk::{
    CompatibilityManifest, environment::SdkEnvironment, manifest::FRAMEWORK_REL_PATH,
};

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

/// The framework crates inside an SDK that one project builds against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameworkSet<'a> {
    pub fframes_version: &'a str,
    /// Version of `fframes-studio-runtime`, `fframes-studio-protocol` and
    /// `fframes-studio-kit`.
    pub runtime_version: &'a str,
    /// Version of the crates.io `fframes-kit` in the SDK's vendor folder, if it has one.
    pub kit_version: Option<&'a str>,
    /// The crates' directory inside the SDK.
    pub rel_path: &'a str,
}

/// The crates.io component kit. Projects take it from the SDK's vendor folder, where its
/// `fframes` dependency is patched to the SDK's framework.
const KIT: &str = "fframes-kit";

/// The framework `project` builds against in the SDK described by `compatibility`, or
/// `None` when the project is pinned to another SDK.
pub fn framework_for<'a>(
    project: &studio_project::manifest::Manifest,
    compatibility: &'a CompatibilityManifest,
) -> Option<FrameworkSet<'a>> {
    (project.sdk == sdk_pin(compatibility)).then_some(FrameworkSet {
        fframes_version: &compatibility.fframes_version,
        runtime_version: &compatibility.runtime_version,
        kit_version: compatibility.kit_version.as_deref(),
        rel_path: FRAMEWORK_REL_PATH,
    })
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

/// The sources the last successful build in a shared target compiled, and when that build
/// started. Lives in the target and exists only between builds.
#[derive(serde::Serialize, serde::Deserialize)]
struct BuiltSources {
    started: SystemTime,
    /// Inventory path to SHA-256 of every copied source.
    files: BTreeMap<String, String>,
    /// Cargo's fingerprint entries of the package after that build. A build outside this
    /// record (a manual `cargo` run in the target) changes them, and nothing is reused.
    fingerprints: BTreeMap<String, SystemTime>,
}

const BUILT_SOURCES: &str = "studio-built-sources.json";

fn package_fingerprints(build: &MaterializedBuild) -> io::Result<BTreeMap<String, SystemTime>> {
    let mut entries = BTreeMap::new();
    let dir = build
        .environment
        .target_dir
        .join("debug")
        .join(".fingerprint");
    let prefix = format!("{}-", build.package);
    let Ok(read) = fs::read_dir(&dir) else {
        return Ok(entries);
    };
    for entry in read {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&prefix) {
            continue;
        }
        for file in fs::read_dir(entry.path())? {
            let file = file?;
            entries.insert(
                format!("{name}/{}", file.file_name().to_string_lossy()),
                file.metadata()?.modified()?,
            );
        }
    }
    Ok(entries)
}

/// What [`reuse_unchanged_sources`] found before a build, for [`record_built_sources`].
pub struct SourceReuse {
    started: SystemTime,
    /// The time given to unchanged sources, when the record allowed reuse.
    reused: Option<SystemTime>,
    fingerprints: BTreeMap<String, SystemTime>,
}

/// Every copy is a new file, so Cargo would recompile the project's crates even when no
/// source changed. Gives each copied source that is byte-identical to what the last
/// successful build in this target compiled a time older than the outputs compiled from
/// it and newer than every output an earlier build left. Cargo then reuses exactly the
/// crates whose sources did not change, and rebuilds the rest.
///
/// Call with the target lock held, right before Cargo runs. The record is removed first, so
/// a build that fails or is killed leaves nothing to reuse. Pass the result to
/// [`record_built_sources`] after the build succeeds.
pub fn reuse_unchanged_sources(
    build: &MaterializedBuild,
    files: &[SourceFile],
) -> io::Result<SourceReuse> {
    let path = build.environment.target_dir.join(BUILT_SOURCES);
    let record = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<BuiltSources>(&bytes).ok());
    match fs::remove_file(&path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let mut reuse = SourceReuse {
        started: SystemTime::now(),
        reused: None,
        fingerprints: package_fingerprints(build)?,
    };
    let Some(record) = record.filter(|record| record.fingerprints == reuse.fingerprints) else {
        return Ok(reuse);
    };
    for file in files {
        // Cargo manifests are rewritten for the SDK, and Cargo does not track their times.
        if file.kind == FileKind::Cargo || run_time_document(file.path.as_str()) {
            continue;
        }
        if record.files.get(file.path.as_str()) == Some(&file.sha256) {
            fs::File::options()
                .write(true)
                .open(build.root.join(file.path.as_str()))?
                .set_modified(record.started)?;
        }
    }
    reuse.reused = Some(record.started);
    Ok(reuse)
}

/// Records what a successful build compiled for the next [`reuse_unchanged_sources`].
pub fn record_built_sources(
    build: &MaterializedBuild,
    files: &[SourceFile],
    reuse: SourceReuse,
) -> io::Result<()> {
    let fingerprints = package_fingerprints(build)?;
    // When Cargo compiled nothing of the package, its outputs are still the ones the
    // reused time is older than; this build's start is not.
    let started = match reuse.reused {
        Some(reused) if fingerprints == reuse.fingerprints => reused,
        _ => reuse.started,
    };
    let record = BuiltSources {
        started,
        files: files
            .iter()
            .map(|file| (file.path.as_str().to_owned(), file.sha256.clone()))
            .collect(),
        fingerprints,
    };
    fs::write(
        build.environment.target_dir.join(BUILT_SOURCES),
        serde_json::to_vec(&record).map_err(io::Error::other)?,
    )
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
    let Some(framework) = framework_for(&project.manifest, &compatibility) else {
        return Err(error(&sdk, "SDK pin mismatch".into()));
    };
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
    // Keep the transient tree shallow: Windows can reject a child process whose working
    // directory inherits the former project/revision/compatibility/triple nesting.
    fs::create_dir_all(app_builds).map_err(|e| error(app_builds, e.to_string()))?;
    let staging = tempfile::Builder::new()
        .prefix("build-")
        .tempdir_in(app_builds)
        .map_err(|e| error(app_builds, e.to_string()))?;
    let root = staging.path().join("project");
    fs::create_dir(&root).map_err(|e| error(&root, e.to_string()))?;
    let mut buffer = [0; 64 * 1024];
    // The timing and props documents stay out: the build is shared by every timing and
    // props of the same code, which the preview and export hand to the worker at run time.
    let build_revision = project.inventory.build_revision();
    for source in &project.inventory.files {
        if run_time_document(source.path.as_str()) {
            continue;
        }
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
    if SourceInventory::scan_with_cancel(&root, cancelled)?.build_revision() != build_revision
        || SourceInventory::scan_with_cancel(&project.root, cancelled)?.build_revision()
            != build_revision
    {
        return Err(error(
            &project.root,
            "Source changed during materialization; refresh and retry".into(),
        ));
    }
    let framework_dir = sdk.join(framework.rel_path);
    let mut manifests = Vec::new();
    let mut uses_kit = false;
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
            &framework_dir,
            framework,
            &mut uses_kit,
        )?;
        manifests.push((file, cargo));
    }
    // Like project import, only the nearest workspace on the selected package's
    // ancestor chain owns it; independent tools workspaces do not govern this build.
    let entry = Path::new(project.manifest.entry.manifest.as_str());
    let patch_manifest = entry
        .parent()
        .unwrap()
        .ancestors()
        .find_map(|directory| {
            let candidate = directory.join("Cargo.toml");
            manifests
                .iter()
                .any(|(file, cargo)| {
                    Path::new(file.path.as_str()) == candidate && cargo.get("workspace").is_some()
                })
                .then_some(candidate)
        })
        .unwrap_or_else(|| entry.to_owned());
    for (file, mut cargo) in manifests {
        // The kit comes from crates.io and depends on crates.io `fframes`; the workspace root
        // (or the entry package without one) redirects that to the SDK's framework, so the
        // graph has one `fframes`. Only this staging copy gets the `[patch]`: projects may
        // not have one.
        if uses_kit && Path::new(file.path.as_str()) == patch_manifest {
            patch_framework(&mut cargo, &framework_dir)?;
        }
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
    // Line tables keep file and line in backtraces; full debug info only made each preview
    // build link (and write the PDB) about two seconds longer. The framework and other
    // dependencies are optimized: unoptimized, they rendered 12 times slower, and level 1
    // compiles faster than none while rendering as fast as level 3. The project's own crates
    // stay unoptimized for quick edits.
    let cargo_config = format!(
        "[source.crates-io]\nreplace-with = \"studio-vendor\"\n[source.studio-vendor]\ndirectory = {}\n[profile.dev]\ndebug = \"line-tables-only\"\n[profile.dev.package.\"*\"]\nopt-level = 1\n",
        toml::Value::String(vendor.to_string_lossy().into_owned())
    );
    // This file is also inside the private staging tree; a failed write discards that tree.
    fs::write(&cargo_config_path, cargo_config.as_bytes())
        .map_err(|e| error(&cargo_config_path, e.to_string()))?;
    // SDK path substitution has a distinct graph/lock, never overwrite the portable lock.
    if root.join("Cargo.lock").exists() {
        fs::remove_file(root.join("Cargo.lock")).map_err(|e| error(&root, e.to_string()))?;
    }
    if SourceInventory::scan_with_cancel(&project.root, cancelled)?.build_revision()
        != build_revision
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
    framework_dir: &Path,
    framework: FrameworkSet<'_>,
    uses_kit: &mut bool,
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
                        if crate_name == KIT {
                            check_kit(dep, project_root.join(package_dir), name, framework)?;
                        }
                        // fframes-studio-kit depends on the crates.io kit too.
                        *uses_kit |= matches!(crate_name.as_str(), KIT | "fframes-studio-kit");
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
                            "fframes"
                                | "fframes-studio-runtime"
                                | "fframes-studio-protocol"
                                | "fframes-studio-kit"
                        ) && dep.get("workspace").and_then(toml::Value::as_bool) != Some(true)
                        {
                            let expected = if crate_name == "fframes" {
                                framework.fframes_version
                            } else {
                                framework.runtime_version
                            };
                            let version = dep
                                .as_str()
                                .or_else(|| dep.get("version").and_then(toml::Value::as_str));
                            // Every framework crate names its exact release, so a project
                            // never builds against a different one than it was written for.
                            if version.is_none_or(|v| v.trim_start_matches('=') != expected) {
                                return Err(ProjectError::new(
                                    project_root.join(package_dir),
                                    name,
                                    format!(
                                        "dependency version does not match SDK framework {expected}"
                                    ),
                                    "Select an SDK matching this version",
                                ));
                            }
                            if dep.is_str() {
                                *dep = toml::Value::Table(toml::map::Map::new());
                            }
                            let crate_dir = framework_dir.join(&crate_name);
                            if !crate_dir.join("Cargo.toml").is_file() {
                                return Err(ProjectError::new(
                                    crate_dir,
                                    name,
                                    "SDK crate missing",
                                    "Install the complete compatible SDK",
                                ));
                            }
                            dep.as_table_mut().unwrap().insert(
                                "path".into(),
                                toml::Value::String(crate_dir.to_string_lossy().into_owned()),
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
                    framework_dir,
                    framework,
                    uses_kit,
                )?;
            }
        }
    }
    Ok(())
}

/// Refuses an `fframes-kit` dependency the SDK's vendor folder cannot satisfy, naming the
/// line that works, instead of Cargo's resolution error. A dependency inherited from the
/// workspace is checked where the workspace declares it.
fn check_kit(
    dep: &toml::Value,
    manifest_dir: PathBuf,
    name: &str,
    framework: FrameworkSet<'_>,
) -> Result<(), ProjectError> {
    if dep.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
        return Ok(());
    }
    let Some(kit) = framework.kit_version else {
        return Err(ProjectError::new(
            manifest_dir,
            name,
            "This SDK does not include fframes-kit",
            "Install an SDK that includes fframes-kit, or remove the dependency",
        ));
    };
    let version = dep
        .as_str()
        .or_else(|| dep.get("version").and_then(toml::Value::as_str));
    let from_crates_io = ["git", "path", "registry"]
        .iter()
        .all(|key| dep.get(key).is_none());
    if !from_crates_io || version.is_none_or(|v| v.trim().trim_start_matches('=') != kit) {
        return Err(ProjectError::new(
            manifest_dir,
            name,
            format!("This SDK includes fframes-kit {kit}; set `fframes-kit = \"={kit}\"`"),
            "Use the crates.io release the SDK includes (keep the dependency's features), or select an SDK with the kit version you need",
        ));
    }
    Ok(())
}

/// Points crates.io `fframes` at the SDK's framework crate in a staging manifest.
fn patch_framework(cargo: &mut toml::Value, framework_dir: &Path) -> Result<(), ProjectError> {
    let fframes = framework_dir.join("fframes");
    if !fframes.join("Cargo.toml").is_file() {
        return Err(ProjectError::new(
            fframes,
            KIT,
            "SDK crate missing",
            "Install the complete compatible SDK",
        ));
    }
    let path = toml::Value::String(fframes.to_string_lossy().into_owned());
    let patch: toml::Value = format!("[crates-io.fframes]\npath = {path}\n")
        .parse()
        .expect("a TOML table with one escaped string");
    // `bind` refused any `[patch]` of the project, so this is the only one.
    if let Some(table) = cargo.as_table_mut() {
        table.insert("patch".into(), patch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIT_LINE: &str = "This SDK includes fframes-kit 0.2.0; set `fframes-kit = \"=0.2.0\"`";

    struct Fixture {
        temp: tempfile::TempDir,
        compatibility: CompatibilityManifest,
    }

    impl Fixture {
        /// An SDK with stub framework crates, vendoring `kit` when given.
        fn new(kit: Option<&str>) -> Self {
            let temp = tempfile::tempdir().unwrap();
            for name in [
                "fframes",
                "fframes-studio-runtime",
                "fframes-studio-protocol",
                "fframes-studio-kit",
            ] {
                let dir = temp.path().join("sdk").join(FRAMEWORK_REL_PATH).join(name);
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
            }
            fs::create_dir_all(temp.path().join("sdk/framework/vendor")).unwrap();
            let mut compatibility = CompatibilityManifest::default_linux_x64();
            compatibility.kit_version = kit.map(str::to_owned);
            Self {
                temp,
                compatibility,
            }
        }

        /// A new project whose root `Cargo.toml` gains `extra` (TOML tables).
        fn project(&self, extra: &str) -> OpenProject {
            let root = self.temp.path().join("video");
            studio_project::create(
                &root,
                "Video",
                sdk_pin(&self.compatibility),
                &self.compatibility.fframes_version,
                &self.compatibility.runtime_version,
            )
            .unwrap();
            let mut cargo: toml::Table = fs::read_to_string(root.join("Cargo.toml"))
                .unwrap()
                .parse()
                .unwrap();
            for (key, value) in extra.parse::<toml::Table>().unwrap() {
                match (cargo.get_mut(&key), value) {
                    (Some(toml::Value::Table(table)), toml::Value::Table(more)) => {
                        table.extend(more);
                    }
                    (_, value) => {
                        cargo.insert(key, value);
                    }
                }
            }
            fs::write(root.join("Cargo.toml"), cargo.to_string()).unwrap();
            studio_project::open(&root).unwrap()
        }

        fn materialize(&self, project: &OpenProject) -> Result<MaterializedBuild, ProjectError> {
            materialize(
                project,
                &self.temp.path().join("sdk"),
                self.compatibility.clone(),
                &self.temp.path().join("builds"),
            )
        }

        fn framework(&self, name: &str) -> String {
            fs::canonicalize(self.temp.path().join("sdk"))
                .unwrap()
                .join(FRAMEWORK_REL_PATH)
                .join(name)
                .to_string_lossy()
                .into_owned()
        }
    }

    fn staged(build: &MaterializedBuild) -> toml::Table {
        fs::read_to_string(build.root.join("Cargo.toml"))
            .unwrap()
            .parse()
            .unwrap()
    }

    #[test]
    fn kit_binds_and_only_the_staging_manifest_gets_the_fframes_patch() {
        let sdk = Fixture::new(Some("0.2.0"));
        let project = sdk.project(
            "[dependencies]\n\
             fframes-kit = { version = \"=0.2.0\", features = [\"studio\"] }\n\
             fframes-studio-kit = \"=0.2.0\"\n",
        );
        let source = fs::read(project.root.join("Cargo.toml")).unwrap();
        let build = sdk.materialize(&project).unwrap();
        assert_eq!(
            fs::read(project.root.join("Cargo.toml")).unwrap(),
            source,
            "the project's Cargo.toml bytes are unchanged"
        );

        let cargo = staged(&build);
        assert_eq!(
            cargo["patch"]["crates-io"]["fframes"]["path"].as_str(),
            Some(sdk.framework("fframes").as_str())
        );
        let dependencies = &cargo["dependencies"];
        assert_eq!(
            dependencies["fframes-studio-kit"]["path"].as_str(),
            Some(sdk.framework("fframes-studio-kit").as_str())
        );
        // The kit stays a crates.io dependency, served by the SDK's vendor folder.
        let kit = dependencies["fframes-kit"].as_table().unwrap();
        assert_eq!(kit["version"].as_str(), Some("=0.2.0"));
        assert!(!kit.contains_key("path"));
    }

    #[test]
    fn projects_without_the_kit_get_no_patch() {
        let sdk = Fixture::new(Some("0.2.0"));
        let build = sdk.materialize(&sdk.project("")).unwrap();
        assert!(!staged(&build).contains_key("patch"));
    }

    #[test]
    fn studio_kit_alone_also_gets_the_patch() {
        let sdk = Fixture::new(Some("0.2.0"));
        let project = sdk.project("[dependencies]\nfframes-studio-kit = \"=0.2.0\"\n");
        let build = sdk.materialize(&project).unwrap();
        assert!(staged(&build).contains_key("patch"));
    }

    #[test]
    fn a_kit_the_sdk_cannot_serve_names_the_line_to_use() {
        for dependency in [
            "fframes-kit = \"=0.0.9\"",
            "fframes-kit = \"0.2\"",
            "fframes-kit = { git = \"https://github.com/khanhthanhdev/fframes-kit\" }",
            "fframes-kit = { git = \"https://github.com/khanhthanhdev/fframes-kit\", version = \"0.2.0\" }",
            "fframes-kit = { path = \"../fframes-kit\" }",
            "kit = { package = \"fframes-kit\", path = \"kit\" }",
        ] {
            let sdk = Fixture::new(Some("0.2.0"));
            let project = sdk.project(&format!("[dependencies]\n{dependency}\n"));
            let error = sdk.materialize(&project).err().expect(dependency);
            assert_eq!(error.reason, KIT_LINE, "{dependency}");
        }
    }

    #[test]
    fn an_sdk_without_the_kit_says_so() {
        let sdk = Fixture::new(None);
        let project = sdk.project("[dependencies]\nfframes-kit = \"=0.2.0\"\n");
        let error = sdk.materialize(&project).err().unwrap();
        assert_eq!(error.reason, "This SDK does not include fframes-kit");
    }

    #[test]
    fn studio_kit_must_match_the_runtime_version() {
        let sdk = Fixture::new(Some("0.2.0"));
        let project = sdk.project("[dependencies]\nfframes-studio-kit = \"=0.1.0\"\n");
        let error = sdk.materialize(&project).err().unwrap();
        assert_eq!(error.field, "fframes-studio-kit");
        assert!(error.reason.contains("0.2.0"), "{}", error.reason);
    }

    #[test]
    fn a_project_patch_is_still_refused() {
        let sdk = Fixture::new(Some("0.2.0"));
        let project = sdk.project(
            "[dependencies]\nfframes-kit = \"=0.2.0\"\n\
             [patch.crates-io]\nfframes = { path = \"fframes\" }\n",
        );
        let error = sdk.materialize(&project).err().unwrap();
        assert_eq!(error.field, "patch");
    }
}
