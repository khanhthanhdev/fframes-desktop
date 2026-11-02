//! Finding and installing the managed SDK: the manifest for this host, the default SDK
//! folder, and installation from the packaged artifacts or a user-selected bundle. Callers
//! run these off the UI thread.
use std::{
    fs,
    path::{Path, PathBuf},
};
use studio_bootstrap::ProcessTreeManager;
use studio_sdk::{install::SdkInstaller, manifest::CompatibilityManifest};

/// Names the folder of installed SDKs, overriding the detected one.
pub const SDK_HOME_ENV: &str = "FFRAMES_SDK_HOME";

/// The folder of installed SDKs: `FFRAMES_SDK_HOME` when set, otherwise `.fframes/sdk` under
/// `HOME` or `USERPROFILE`, preferring the one that already holds an installed SDK (they
/// differ on Windows when one tool set `HOME` and another did not).
pub fn default_sdk_home() -> PathBuf {
    if let Some(explicit) = std::env::var_os(SDK_HOME_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(explicit);
    }
    let homes: Vec<PathBuf> = ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|v| !v.is_empty())
        .map(|home| PathBuf::from(home).join(".fframes").join("sdk"))
        .collect();
    pick_sdk_home(&homes).unwrap_or_else(|| std::env::temp_dir().join("fframes_sdk"))
}

/// The first of `homes` with an installed SDK, else the first of them.
fn pick_sdk_home(homes: &[PathBuf]) -> Option<PathBuf> {
    homes
        .iter()
        .find(|home| !installed_sdks(home).is_empty())
        .or(homes.first())
        .cloned()
}

fn read_sdk_manifest(dir: &Path) -> Result<CompatibilityManifest, String> {
    fs::read_to_string(dir.join("compatibility.json"))
        .map_err(|e| e.to_string())
        .and_then(|json| CompatibilityManifest::from_json_str(&json).map_err(|e| e.to_string()))
}

/// The SDKs installed under `sdk_home` that target this host: the active installation,
/// then the one it replaced (kept by the installer as `previous`).
pub fn installed_sdks(sdk_home: &Path) -> Vec<(PathBuf, CompatibilityManifest)> {
    let installer = SdkInstaller::new(sdk_home);
    let host = CompatibilityManifest::host_target();
    [installer.active_sdk_dir(), installer.previous_sdk_dir()]
        .into_iter()
        .filter_map(|dir| {
            let manifest = read_sdk_manifest(&dir).ok()?;
            (Some(manifest.target_triple.as_str()) == host).then_some((dir, manifest))
        })
        .collect()
}

/// The installed SDK `project` is pinned to, if any is installed under `sdk_home`.
pub fn installed_sdk_for(
    project: &studio_project::manifest::Manifest,
    sdk_home: &Path,
) -> Option<PathBuf> {
    installed_sdks(sdk_home)
        .into_iter()
        .find(|(_, manifest)| {
            crate::build_materialization::framework_for(project, manifest).is_some()
        })
        .map(|(dir, _)| dir)
}

/// The framework releases the SDK described by `manifest` provides.
pub fn framework_versions(
    manifest: &CompatibilityManifest,
) -> studio_project::lifecycle::FrameworkVersions<'_> {
    studio_project::lifecycle::FrameworkVersions {
        fframes: &manifest.fframes_version,
        runtime: &manifest.runtime_version,
        kit: manifest.kit_version.as_deref(),
    }
}

/// Whether `project` builds and renders with the installed SDK `sdk` once upgraded to it.
/// Upgrades a copy of the project's source under `builds`, then compiles it and renders
/// its first frame like a preview; the project itself is not touched. Blocks for as long
/// as the build takes.
pub fn check_project_upgrade(
    project: &studio_project::OpenProject,
    sdk: &Path,
    manifest: &CompatibilityManifest,
    builds: &Path,
    processes: &ProcessTreeManager,
) -> Result<(), String> {
    fs::create_dir_all(builds).map_err(|e| e.to_string())?;
    let scratch = tempfile::Builder::new()
        .prefix("upgrade-check-")
        .tempdir_in(builds)
        .map_err(|e| e.to_string())?;
    let copy = scratch.path().join("project");
    for file in &project.inventory.files {
        let to = copy.join(file.path.as_str());
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::copy(project.root.join(file.path.as_str()), &to)
            .map_err(|e| format!("Copying {}: {e}", file.path.as_str()))?;
    }
    studio_project::lifecycle::upgrade_framework(
        &copy,
        crate::build_materialization::sdk_pin(manifest),
        framework_versions(manifest),
    )
    .map_err(|e| e.to_string())?;
    let upgraded = studio_project::open(&copy).map_err(|e| e.to_string())?;
    crate::services::worker_project::check_project_build(
        &upgraded, sdk, manifest, builds, processes,
    )
}

/// What to do when no SDK bundle targets this host.
pub fn missing_sdk_bundle_message(host: &str) -> String {
    let assemble = if cfg!(windows) {
        "desktop/scripts/build-phase-zero-sdk.ps1 -FfmpegRoot <ffmpeg> -Out desktop/target/phase-zero-sdk"
    } else {
        "desktop/scripts/build-phase-zero-sdk.sh --ffmpeg-root <ffmpeg> --out desktop/target/phase-zero-sdk"
    };
    format!(
        "No SDK bundle for {host} found. Import an SDK bundle, set FFRAMES_SDK_BUNDLE to an \
         assembled bundle, or assemble one with `{assemble}`."
    )
}

/// The SDK manifest for this host and the folder of SDKs installed under `sdk_home`.
pub fn defaults_in(sdk_home: PathBuf) -> Result<(CompatibilityManifest, PathBuf), String> {
    let packaged = std::env::var_os("FFRAMES_SDK_BUNDLE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| Some(p.parent()?.parent()?.join("sdk")))
        });
    // Bundles assembled in a repository checkout, relative to the working folder. Only a
    // development build looks there: a release build never takes its trust root (the
    // manifest that pins every artifact hash) from wherever it was started.
    let dev_bundles: Vec<PathBuf> = if cfg!(debug_assertions) {
        vec![
            PathBuf::from("target/phase-zero-sdk"),
            PathBuf::from("desktop/target/phase-zero-sdk"),
        ]
    } else {
        Vec::new()
    };
    let manifest = resolve_manifest(packaged.as_deref(), &dev_bundles, &sdk_home)?;
    Ok((manifest, sdk_home))
}

/// Picks the SDK manifest for this host: a packaged bundle, the built-in manifest when it
/// targets this host, an assembled development bundle for this host, then the installed
/// SDK's receipt. A host with none of these gets instructions instead of a foreign target.
pub fn resolve_manifest(
    packaged: Option<&Path>,
    dev_bundles: &[PathBuf],
    sdk_home: &Path,
) -> Result<CompatibilityManifest, String> {
    let read = read_sdk_manifest;
    if let Some(bundle) = packaged.filter(|p| p.join("compatibility.json").is_file()) {
        return read(bundle);
    }
    let host = CompatibilityManifest::host_target();
    let for_host = |manifest: &CompatibilityManifest| Some(manifest.target_triple.as_str()) == host;
    let built_in = CompatibilityManifest::default_linux_x64();
    if for_host(&built_in) {
        return Ok(built_in);
    }
    let active = sdk_home.join("active");
    dev_bundles
        .iter()
        .chain([&active])
        .filter_map(|bundle| read(bundle).ok())
        .find(for_host)
        .ok_or_else(|| missing_sdk_bundle_message(host.unwrap_or("this platform")))
}

/// The installer the app uses: a first install is checked in place by building and
/// rendering the starter project into the shared target under `builds`, which also warms
/// it for the first preview.
pub fn installer(sdk_home: &Path, processes: &ProcessTreeManager, builds: &Path) -> SdkInstaller {
    let (check_processes, builds) = (processes.clone(), builds.to_owned());
    SdkInstaller::new(sdk_home)
        .with_process_manager(processes.clone())
        .with_candidate_check(std::sync::Arc::new(move |sdk, manifest| {
            crate::services::worker_project::check_sdk_build(
                sdk,
                manifest,
                &builds,
                &check_processes,
            )
        }))
}

/// Install only the SDK. Product setup must not generate a project.
/// `app_version` is the running app's version the SDK must be compatible with; `builds`
/// is the app's build folder.
pub fn install_sdk(
    manifest: &CompatibilityManifest,
    sdk_home: &Path,
    processes: &ProcessTreeManager,
    app_version: &str,
    builds: &Path,
) -> Result<PathBuf, String> {
    manifest
        .validate_for_current_app_version(app_version)
        .map_err(|error| format!("SDK compatibility rejected before setup: {error}"))?;
    let installer = installer(sdk_home, processes, builds);
    installer
        .recover_interrupted_install()
        .map_err(|error| format!("Interrupted SDK install recovery failed: {error}"))?;
    if let Some(active_sdk) = installer
        .reusable_active_for_app(manifest, app_version, Some(processes))
        .map_err(|error| format!("Installed SDK receipt check failed: {error}"))?
    {
        return Ok(active_sdk);
    }

    // 1. Resolve artifacts strictly from manifest across candidate directories
    let mut candidate_artifact_dirs = vec![
        PathBuf::from("desktop/packaging/sdk/artifacts"),
        PathBuf::from("packaging/sdk/artifacts"),
        PathBuf::from("target/phase-zero-sdk/artifacts"),
        PathBuf::from("desktop/target/phase-zero-sdk/artifacts"),
    ];
    if let Some(bundle) = std::env::var_os("FFRAMES_SDK_BUNDLE") {
        candidate_artifact_dirs.insert(0, PathBuf::from(bundle).join("artifacts"));
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(package) = executable.parent().and_then(|p| p.parent())
    {
        candidate_artifact_dirs.insert(0, package.join("sdk/artifacts"));
    }

    let mut artifact_files = Vec::new();
    for artifact in &manifest.artifacts {
        let filename = artifact
            .url
            .strip_prefix("file://artifacts/")
            .or_else(|| artifact.url.strip_prefix("file://"))
            .unwrap_or(&artifact.name);
        let relative = PathBuf::from(filename);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "SDK artifact '{}' has an unsafe local bundle path",
                artifact.name
            ));
        }
        let mut resolved = None;
        for dir in &candidate_artifact_dirs {
            let root = dir.canonicalize().ok();
            let candidate = dir.join(&relative).canonicalize().ok();
            if let (Some(root), Some(candidate)) = (root, candidate)
                && candidate.starts_with(root)
                && candidate.is_file()
            {
                resolved = Some(candidate);
                break;
            }
        }
        let file_path = resolved.ok_or_else(|| {
            format!(
                "required artifact '{}' ({}) not found in candidate artifact directories",
                artifact.name, filename
            )
        })?;
        artifact_files.push((artifact.clone(), file_path));
    }

    // 2. A non-receipted or incompatible active SDK is replaced only after the
    // local bundle has been resolved and the new candidate passes build/render probes.
    installer
        .install_from_local_artifacts_for_app(manifest, &artifact_files, app_version)
        .map_err(|e| format!("SDK installation failed: {e}"))?;

    Ok(installer.active_sdk_dir())
}

/// Explicit import path for a user-selected offline SDK bundle. No network lookup
/// occurs and a receipt-backed compatible active SDK can still be reused archive-free.
/// `app_version` is the running app's version the SDK must be compatible with.
pub fn install_sdk_from_bundle(
    bundle: &Path,
    sdk_home: &Path,
    processes: &ProcessTreeManager,
    app_version: &str,
    builds: &Path,
) -> Result<PathBuf, String> {
    let manifest_path = bundle.join("compatibility.json");
    let json = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("SDK bundle manifest {}: {error}", manifest_path.display()))?;
    let manifest = CompatibilityManifest::from_json_str(&json)
        .map_err(|error| format!("SDK bundle manifest is invalid: {error}"))?;
    manifest
        .validate_for_current_app_version(app_version)
        .map_err(|error| format!("SDK bundle is incompatible: {error}"))?;

    let installer = installer(sdk_home, processes, builds);
    installer
        .recover_interrupted_install()
        .map_err(|error| format!("Interrupted SDK install recovery failed: {error}"))?;
    if let Some(active_sdk) = installer
        .reusable_active_for_app(&manifest, app_version, Some(processes))
        .map_err(|error| format!("Installed SDK receipt check failed: {error}"))?
    {
        return Ok(active_sdk);
    }

    let bundle_root = bundle
        .canonicalize()
        .map_err(|error| format!("SDK bundle folder is unavailable: {error}"))?;
    let artifacts_dir = bundle_root
        .join("artifacts")
        .canonicalize()
        .map_err(|error| format!("SDK bundle artifacts folder is unavailable: {error}"))?;
    let mut artifacts = Vec::with_capacity(manifest.artifacts.len());
    for artifact in &manifest.artifacts {
        let filename = artifact
            .url
            .strip_prefix("file://artifacts/")
            .ok_or_else(|| {
                format!(
                    "SDK bundle artifact '{}' has a non-local URL",
                    artifact.name
                )
            })?;
        let relative = PathBuf::from(filename);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "SDK bundle artifact '{}' has an unsafe path",
                artifact.name
            ));
        }
        let path = artifacts_dir.join(relative);
        let canonical = path.canonicalize().map_err(|error| {
            format!(
                "SDK bundle artifact '{}' is unavailable: {error}",
                artifact.name
            )
        })?;
        if !canonical.starts_with(&artifacts_dir) || !canonical.is_file() {
            return Err(format!(
                "SDK bundle artifact '{}' resolves outside its artifacts directory",
                artifact.name,
            ));
        }
        artifacts.push((artifact.clone(), canonical));
    }
    installer
        .install_from_local_artifacts_for_app(&manifest, &artifacts, app_version)
        .map_err(|error| format!("SDK bundle installation failed: {error}"))?;
    Ok(installer.active_sdk_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_bundle(dir: &Path, target: &str) -> CompatibilityManifest {
        let mut manifest = CompatibilityManifest::default_linux_x64();
        manifest.target_triple = target.into();
        manifest.sdk_id = format!("studio-sdk-{target}-test");
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("compatibility.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
        manifest
    }

    #[test]
    fn a_project_builds_with_the_installed_sdk_of_its_pin() {
        let host = CompatibilityManifest::host_target().expect("supported host");
        let root = tempfile::tempdir().unwrap();
        let (empty, home) = (root.path().join("empty"), root.path().join("home"));
        let older = write_bundle(&home.join("previous"), host);
        let mut newer = older.clone();
        newer.sdk_id = "studio-sdk-newer".into();
        fs::create_dir_all(home.join("active")).unwrap();
        fs::write(
            home.join("active").join("compatibility.json"),
            serde_json::to_string(&newer).unwrap(),
        )
        .unwrap();
        // A foreign SDK is never one of this host's installations.
        write_bundle(
            &root.path().join("foreign").join("active"),
            "riscv64gc-unknown-linux-gnu",
        );

        assert_eq!(installed_sdks(&home).len(), 2);
        assert!(installed_sdks(&root.path().join("foreign")).is_empty());
        // The home that holds an installation wins over an earlier empty one.
        assert_eq!(
            pick_sdk_home(&[empty.clone(), home.clone()]),
            Some(home.clone())
        );
        assert_eq!(
            pick_sdk_home(&[empty.clone(), root.path().join("foreign")]),
            Some(empty)
        );

        let project = |sdk: &CompatibilityManifest| {
            let dir = tempfile::tempdir().unwrap();
            studio_project::create(
                &dir.path().join("video"),
                "Video",
                crate::build_materialization::sdk_pin(sdk),
                &sdk.fframes_version,
                &sdk.runtime_version,
            )
            .unwrap()
            .manifest
        };
        assert_eq!(
            installed_sdk_for(&project(&newer), &home),
            Some(home.join("active"))
        );
        assert_eq!(
            installed_sdk_for(&project(&older), &home),
            Some(home.join("previous"))
        );
        let mut missing = older.clone();
        missing.sdk_id = "studio-sdk-not-installed".into();
        assert_eq!(installed_sdk_for(&project(&missing), &home), None);
    }

    #[test]
    fn manifest_resolution_never_falls_back_to_a_foreign_target() {
        let host = CompatibilityManifest::host_target().expect("supported host");
        let foreign = if host == "x86_64-unknown-linux-gnu" {
            "x86_64-pc-windows-msvc"
        } else {
            "x86_64-unknown-linux-gnu"
        };
        let root = tempfile::tempdir().unwrap();
        let sdk_home = root.path().join("sdk");
        let dev = root.path().join("dev");
        write_bundle(&dev, foreign);

        let resolved = resolve_manifest(None, std::slice::from_ref(&dev), &sdk_home);
        if host == "x86_64-unknown-linux-gnu" {
            assert_eq!(
                resolved.unwrap(),
                CompatibilityManifest::default_linux_x64()
            );
            return;
        }
        let error = resolved.expect_err("a foreign bundle is not this host's manifest");
        assert!(error.contains(host), "{error}");

        // The installed SDK's receipt is picked up, and an assembled bundle wins over it.
        let installed = write_bundle(&sdk_home.join("active"), host);
        let resolved = resolve_manifest(None, std::slice::from_ref(&dev), &sdk_home);
        assert_eq!(resolved.unwrap(), installed);
        let assembled = write_bundle(&dev, host);
        let resolved = resolve_manifest(None, &[dev], &sdk_home);
        assert_eq!(resolved.unwrap(), assembled);
    }
}
