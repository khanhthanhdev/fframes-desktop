use crate::worker_client::WorkerClient;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
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
    let build = compile_portable_worker(project, sdk, manifest, builds, manager)?;
    let isolated_binary = worker_binary(&build);
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
    client.retain_build(build);
    client.send_hello().map_err(|e| e.to_string())?;
    Ok(client)
}

fn worker_binary(build: &studio_engine::build_materialization::MaterializedBuild) -> PathBuf {
    build.isolated_bin_dir.join(format!(
        "{}{}",
        build.worker_target,
        std::env::consts::EXE_SUFFIX
    ))
}

/// Nonblocking lock acquisition off the UI thread. The returned file alone owns the lock.
pub fn acquire_build_lock(
    path: &Path,
    manager: &ProcessTreeManager,
    timeout: Duration,
) -> Result<std::fs::File, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        if manager.is_shutdown() {
            return Err("Build cancelled while waiting for target lock".into());
        }
        match file.try_lock() {
            Ok(()) => {
                if manager.is_shutdown() {
                    return Err("Build cancelled after target lock".into());
                }
                return Ok(file);
            }
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(e) => return Err(format!("Target lock unavailable/deadline exceeded: {e}")),
        }
    }
}

pub fn compile_portable_worker(
    project: &studio_project::OpenProject,
    sdk: &Path,
    manifest: CompatibilityManifest,
    builds: &Path,
    manager: &ProcessTreeManager,
) -> Result<Arc<studio_engine::build_materialization::MaterializedBuild>, String> {
    let build = studio_engine::build_materialization::materialize_with_cancel(
        project,
        sdk,
        manifest,
        builds,
        &|| manager.is_shutdown(),
    )
    .map_err(|e| e.to_string())?;
    let lock_file_path = build.environment.target_dir.join(".build_lock");
    std::fs::create_dir_all(&build.environment.target_dir).map_err(|e| e.to_string())?;
    let lock_file = acquire_build_lock(&lock_file_path, manager, Duration::from_secs(120))?;

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
    if studio_project::SourceInventory::scan_with_cancel(&project.root, &|| manager.is_shutdown())
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
    if manager.is_shutdown() {
        return Err("Build cancelled before launch".into());
    }
    Ok(Arc::new(build))
}

pub fn launch_preview_worker(
    build: Arc<studio_engine::build_materialization::MaterializedBuild>,
    identity: fframes_studio_protocol::PreviewIdentity,
    manager: &ProcessTreeManager,
) -> Result<crate::preview_worker_client::PreviewWorkerClient, String> {
    let binary = worker_binary(&build);
    let cwd = build.manifest.parent().unwrap_or(&build.root);
    let cache = build.isolated_bin_dir.join("audio");
    let cache = cache.to_str().ok_or("Preview cache path must be UTF-8")?;
    let mut transport = WorkerClient::new(&identity.source_revision, identity.worker_generation);
    transport
        .spawn_worker(
            &binary,
            &[
                "--preview-worker",
                "--project-id",
                &identity.project_id,
                "--open-session",
                &identity.open_session,
                "--audio-cache",
                cache,
                "--sdk-version",
                &build.environment.manifest.sdk_id,
            ],
            Some(cwd),
            build.environment.build_child_environment(),
            manager,
        )
        .map_err(|e| e.to_string())?;
    transport.retain_build(build);
    let mut client = crate::preview_worker_client::PreviewWorkerClient::new(transport, identity);
    client.negotiate().map_err(|e| e.to_string())?;
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
