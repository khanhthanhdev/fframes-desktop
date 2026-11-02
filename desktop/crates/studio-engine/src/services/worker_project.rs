use super::{
    build_service::{
        BuildKey, BuildLimits, BuildProfile, BuildService, CompileEnvironment, CompileRequest,
        Compiler, Subscriber, SubscriberKind, WORKER_BUILD_BACKEND,
    },
    worker_client::{WorkerClient, WorkerClientError},
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
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

pub fn worker_binary(build: &crate::build_materialization::MaterializedBuild) -> PathBuf {
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

/// The cargo-backed compile step used by the shared service.
pub struct CargoCompiler;

impl Compiler for CargoCompiler {
    fn compile(
        &self,
        request: &CompileRequest,
        scope: &ProcessTreeManager,
    ) -> Result<Arc<crate::build_materialization::MaterializedBuild>, String> {
        compile_with_cargo(&request.project, &request.environment, scope)
    }

    fn accepts(&self, key: &BuildKey) -> Result<(), String> {
        // `compile_with_cargo` builds the default-feature debug worker and nothing else.
        if !key.features.is_empty() {
            return Err(format!("features {:?} are not supported", key.features));
        }
        if !key.options.is_empty() {
            return Err(format!("options {:?} are not supported", key.options));
        }
        if key.backend != WORKER_BUILD_BACKEND {
            return Err(format!(
                "backend {:?} is not supported ({WORKER_BUILD_BACKEND} only)",
                key.backend
            ));
        }
        match key.profile {
            BuildProfile::Debug => Ok(()),
        }
    }

    fn verify(&self, build: &crate::build_materialization::MaterializedBuild) -> bool {
        build.manifest.is_file() && worker_binary(build).is_file()
    }
}

/// Process-wide compile service. UI preparation and agent tools subscribe to it so equal
/// build keys compile once.
pub fn shared_build_service() -> BuildService {
    static SERVICE: OnceLock<BuildService> = OnceLock::new();
    SERVICE
        .get_or_init(|| {
            BuildService::new(
                ProcessTreeManager::new(),
                Arc::new(CargoCompiler),
                BuildLimits::default(),
            )
        })
        .clone()
}

/// Build (or join/lease) the worker for `project` through the shared service. The
/// returned lease belongs to `subscriber`; cancelling `manager` detaches only this caller.
pub fn compile_portable_worker_via(
    service: &BuildService,
    subscriber: Subscriber,
    project: &studio_project::OpenProject,
    sdk: &Path,
    manifest: CompatibilityManifest,
    builds: &Path,
    manager: &ProcessTreeManager,
) -> Result<Arc<crate::build_materialization::MaterializedBuild>, String> {
    let environment = CompileEnvironment::resolve(sdk, &manifest, builds)?;
    let key = BuildKey::worker(project, &environment);
    let request = CompileRequest {
        project: project.clone(),
        environment,
        retained: None,
    };
    let lease = service
        .subscribe(key, request, subscriber)
        .map_err(|e| e.to_string())?
        .wait(&|| manager.is_shutdown())
        .map_err(|e| e.to_string())?;
    if manager.is_shutdown() {
        return Err("Build cancelled before launch".into());
    }
    Ok(lease.into_build())
}

pub fn compile_portable_worker(
    project: &studio_project::OpenProject,
    sdk: &Path,
    manifest: CompatibilityManifest,
    builds: &Path,
    manager: &ProcessTreeManager,
) -> Result<Arc<crate::build_materialization::MaterializedBuild>, String> {
    compile_portable_worker_via(
        &shared_build_service(),
        Subscriber::new(SubscriberKind::Ui, "portable worker"),
        project,
        sdk,
        manifest,
        builds,
        manager,
    )
}

/// [`check_sdk_build`] for an installed `sdk`, reading its manifest.
pub fn warm_build_cache(
    sdk: &Path,
    builds: &Path,
    manager: &ProcessTreeManager,
) -> Result<(), String> {
    let manifest = std::fs::read_to_string(sdk.join("compatibility.json"))
        .map_err(|e| e.to_string())
        .and_then(|json| CompatibilityManifest::from_json_str(&json).map_err(|e| e.to_string()))?;
    check_sdk_build(sdk, &manifest, builds, manager)
}

/// Compiles a throwaway Studio starter project against `sdk` into the shared target under
/// `builds` and renders its first frame with a preview worker: proof that the SDK builds
/// and renders Studio projects, and a warm cache for the first preview. Cargo's artifacts
/// are bound to the SDK's path, so this only warms the cache from the SDK's final folder.
pub fn check_sdk_build(
    sdk: &Path,
    manifest: &CompatibilityManifest,
    builds: &Path,
    manager: &ProcessTreeManager,
) -> Result<(), String> {
    std::fs::create_dir_all(builds).map_err(|e| e.to_string())?;
    let scratch = tempfile::Builder::new()
        .prefix("warm-")
        .tempdir_in(builds)
        .map_err(|e| e.to_string())?;
    let project = studio_project::create(
        &scratch.path().join("project"),
        "Studio build cache",
        crate::build_materialization::sdk_pin(manifest),
        &manifest.fframes_version,
        &manifest.runtime_version,
    )
    .map_err(|e| e.to_string())?;
    check_project_build(&project, sdk, manifest, builds, manager)
}

/// Compiles `project` against `sdk` into the shared target under `builds` and renders its
/// timeline and first frame with a preview worker. The project must be pinned to `sdk`.
pub fn check_project_build(
    project: &studio_project::OpenProject,
    sdk: &Path,
    manifest: &CompatibilityManifest,
    builds: &Path,
    manager: &ProcessTreeManager,
) -> Result<(), String> {
    let environment = CompileEnvironment::resolve(sdk, manifest, builds)?;
    let build = compile_with_cargo(project, &environment, manager)?;
    let identity = fframes_studio_protocol::PreviewIdentity {
        project_id: project.manifest.project_id.clone().into(),
        open_session: uuid::Uuid::new_v4().to_string(),
        source_revision: project.inventory.revision.as_str().to_owned(),
        worker_generation: 1,
    };
    let workers = manager.sub_manager();
    let rendered = launch_preview_worker(build, identity, &workers).and_then(|mut worker| {
        worker.timeline().map_err(|e| e.to_string())?;
        worker.frame(0, 1, 1.0).map_err(|e| e.to_string()).map(drop)
    });
    workers.shutdown(Duration::from_secs(5));
    rendered
}

fn compile_with_cargo(
    project: &studio_project::OpenProject,
    environment: &CompileEnvironment,
    manager: &ProcessTreeManager,
) -> Result<Arc<crate::build_materialization::MaterializedBuild>, String> {
    let build = crate::build_materialization::materialize_in_environment(
        project,
        environment.sdk_environment(),
        environment.builds(),
        &|| manager.is_shutdown(),
    )
    .map_err(|e| e.to_string())?;
    let lock_file_path = build.environment.target_dir.join(".build_lock");
    std::fs::create_dir_all(&build.environment.target_dir).map_err(|e| e.to_string())?;
    let lock_file = acquire_build_lock(&lock_file_path, manager, Duration::from_secs(120))?;

    ProjectManager::ensure_lockfile(&build.root, &build.environment, manager)
        .map_err(|e| e.to_string())?;
    let files = &project.inventory.files;
    let reuse = crate::build_materialization::reuse_unchanged_sources(&build, files)
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
    crate::build_materialization::record_built_sources(&build, files, reuse)
        .map_err(|e| e.to_string())?;
    if studio_project::SourceInventory::scan_with_cancel(&project.root, &|| manager.is_shutdown())
        .map_err(|e| e.to_string())?
        .build_revision()
        != project.inventory.build_revision()
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

/// Why a preview worker could not be launched and negotiated.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PreviewLaunchError {
    /// The worker answered but is not a compatible CPU M2 preview worker (version,
    /// capability, backend or limits). A property of the SDK, not of the edit.
    #[error("{0}")]
    Incompatible(String),
    #[error("{0}")]
    Failed(String),
}

pub fn launch_preview_worker(
    build: Arc<crate::build_materialization::MaterializedBuild>,
    identity: fframes_studio_protocol::PreviewIdentity,
    manager: &ProcessTreeManager,
) -> Result<crate::services::preview_worker_client::PreviewWorkerClient, String> {
    launch_preview_worker_checked(build, identity, manager).map_err(|e| e.to_string())
}

/// [`launch_preview_worker`] with the failure class preserved.
pub fn launch_preview_worker_checked(
    build: Arc<crate::build_materialization::MaterializedBuild>,
    identity: fframes_studio_protocol::PreviewIdentity,
    manager: &ProcessTreeManager,
) -> Result<crate::services::preview_worker_client::PreviewWorkerClient, PreviewLaunchError> {
    let binary = worker_binary(&build);
    let cwd = build.manifest.parent().unwrap_or(&build.root);
    let cache = build.isolated_bin_dir.join("audio");
    let cache = cache
        .to_str()
        .ok_or_else(|| PreviewLaunchError::Failed("Preview cache path must be UTF-8".into()))?;
    let [renderer_flag, renderer] = crate::services::renderer_preference::worker_args();
    let mut transport = WorkerClient::new(&identity.source_revision, identity.worker_generation);
    transport
        .spawn_worker(
            &binary,
            &[
                "--preview-worker",
                renderer_flag,
                renderer,
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
        .map_err(|e| PreviewLaunchError::Failed(e.to_string()))?;
    transport.retain_build(build);
    let mut client =
        crate::services::preview_worker_client::PreviewWorkerClient::new(transport, identity);
    client.negotiate().map_err(|e| match e {
        WorkerClientError::Other(message)
            if message == crate::services::preview_worker_client::INCOMPATIBLE_BRIDGE =>
        {
            PreviewLaunchError::Incompatible(message)
        }
        other => PreviewLaunchError::Failed(other.to_string()),
    })?;
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
    let revision = source_revision(root)?;
    ProjectManager::build_project(root, &env, manager).map_err(|e| e.to_string())?;
    if source_revision(root)? != revision {
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

/// Hashes portable source in stable order; compiled output never defines a revision.
fn source_revision(root: &Path) -> Result<String, String> {
    studio_project::SourceInventory::scan(root)
        .map(|inventory| inventory.revision.as_str().to_owned())
        .map_err(|e| e.to_string())
}
