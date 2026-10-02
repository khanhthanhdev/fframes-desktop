use crate::worker_client::WorkerClient;
use std::path::{Path, PathBuf};
use studio_agent_spike::source_revision;
use studio_bootstrap::ProcessTreeManager;
use studio_sdk::{CompatibilityManifest, ProjectManager, environment::SdkEnvironment};

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
