use crate::worker_client::WorkerClient;
use std::path::{Path, PathBuf};
use studio_agent_spike::source_revision;
use studio_bootstrap::ProcessTreeManager;
use studio_sdk::{CompatibilityManifest, ProjectManager, environment::SdkEnvironment};

/// Build and launch a portable project from an isolated SDK-bound copy.
pub fn launch_portable_worker(
    project: &studio_project::OpenProject,
    sdk: &Path,
    manifest: CompatibilityManifest,
    builds: &Path,
    generation: u64,
    manager: &ProcessTreeManager,
) -> Result<WorkerClient, String> {
    let build = studio_engine::build_materialization::materialize(project, sdk, manifest, builds)
        .map_err(|e| e.to_string())?;
    let lock_file_path = build.environment.target_dir.join(".build_lock");
    std::fs::create_dir_all(&build.environment.target_dir).map_err(|e| e.to_string())?;
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_file_path)
        .map_err(|e| e.to_string())?;
    lock_file.lock().map_err(|e| e.to_string())?;

    ProjectManager::ensure_lockfile(&build.root, &build.environment, manager)
        .map_err(|e| e.to_string())?;
    ProjectManager::build_worker_target(
        &build.root,
        &build.manifest,
        &build.package,
        &build.worker_target,
        &build.environment,
        manager,
    )
    .map_err(|e| e.to_string())?;
    if studio_project::SourceInventory::scan(&project.root)
        .map_err(|e| e.to_string())?
        .revision
        != project.inventory.revision
    {
        return Err("Source changed during worker build; refresh and retry".into());
    }
    let built_binary = build.environment.target_dir.join("debug").join(format!(
        "{}{}",
        build.worker_target,
        std::env::consts::EXE_SUFFIX
    ));
    let isolated_binary = build.isolated_bin_dir.join(format!(
        "{}{}",
        build.worker_target,
        std::env::consts::EXE_SUFFIX
    ));
    std::fs::copy(&built_binary, &isolated_binary).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&isolated_binary, std::fs::Permissions::from_mode(0o755));
    }
    drop(lock_file);

    let runtime_cwd = build.manifest.parent().unwrap_or(&build.root);
    let mut client = WorkerClient::new(project.inventory.revision.as_str(), generation);
    client
        .spawn_worker(
            &isolated_binary,
            &["--worker"],
            Some(runtime_cwd),
            build.environment.build_child_environment(),
            manager,
        )
        .map_err(|e| e.to_string())?;
    client.send_hello().map_err(|e| e.to_string())?;
    Ok(client)
}

/// Creates an annotated project using the managed SDK sources.
pub fn create_worker_project(root: &Path, sdk: &Path) -> Result<(), String> {
    ProjectManager::generate_annotated_worker_project(root, sdk).map_err(|e| e.to_string())
}

pub fn build_worker(
    root: &Path,
    sdk: &Path,
    manifest: CompatibilityManifest,
    manager: &ProcessTreeManager,
) -> Result<(PathBuf, String, SdkEnvironment), String> {
    let env = SdkEnvironment::new(sdk, root.join("target"), manifest, true);
    ProjectManager::ensure_lockfile(root, &env, manager).map_err(|e| e.to_string())?;
    let revision = source_revision(root).map_err(|e| e.to_string())?;
    ProjectManager::build_project(root, &env, manager).map_err(|e| e.to_string())?;
    if source_revision(root).map_err(|e| e.to_string())? != revision {
        return Err("Source changed during worker build; retry with stable inputs".into());
    }
    let binary = root.join("target/debug").join(if cfg!(windows) {
        "studio-annotated-video.exe"
    } else {
        "studio-annotated-video"
    });
    if !binary.is_file() {
        return Err(format!("Compiled worker missing: {}", binary.display()));
    }
    Ok((binary, revision, env))
}

pub fn launch_worker(
    root: &Path,
    sdk: &Path,
    manifest: CompatibilityManifest,
    generation: u64,
    manager: &ProcessTreeManager,
) -> Result<WorkerClient, String> {
    let (binary, revision, env) = build_worker(root, sdk, manifest, manager)?;
    let mut client = WorkerClient::new(revision, generation);
    client
        .spawn_worker(
            &binary,
            &["--worker"],
            Some(root),
            env.build_child_environment(),
            manager,
        )
        .map_err(|e| e.to_string())?;
    let hello = client.send_hello().map_err(|e| e.to_string())?;
    if hello.worker_generation != generation || hello.source_revision != client.revision() {
        return Err("Worker hello identity mismatch".into());
    }
    Ok(client)
}
