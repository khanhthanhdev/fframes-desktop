//! Shared fixture: fake SDK tree, projects, and a counting injectable compiler that can
//! install the deterministic python preview worker as the isolated worker binary.
#![allow(dead_code)]
use fframes_studio::build_service::{BuildKey, CompileEnvironment, CompileRequest, Compiler};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use studio_bootstrap::ProcessTreeManager;
use studio_engine::build_materialization::{
    MaterializedBuild, materialize_in_environment, sdk_pin,
};
use studio_project::OpenProject;
use studio_sdk::CompatibilityManifest;

pub const FAKE_WORKER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/support/fake-preview-worker.py"
);
pub const FAKE_CONFIG: &str = "fframes-fake-worker.json";

pub fn fake_sdk(root: &Path) -> PathBuf {
    let sdk = root.join("sdk");
    for name in [
        "fframes",
        "fframes-studio-runtime",
        "fframes-studio-protocol",
    ] {
        let dir = sdk.join("framework/framework").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Cargo.toml"), "[package]").unwrap();
    }
    fs::create_dir_all(sdk.join("framework/vendor")).unwrap();
    sdk
}

pub fn manifest() -> CompatibilityManifest {
    CompatibilityManifest::default_linux_x64()
}

pub fn create_project(root: &Path) -> OpenProject {
    studio_project::create(root, "Video", sdk_pin(&manifest()), "1.1.0", "0.1.0").unwrap()
}

/// Rewrite `src/lib.rs` so the project has a different immutable revision.
pub fn with_revision(root: &Path, marker: &str) -> OpenProject {
    fs::write(root.join("src/lib.rs"), format!("// {marker}\n")).unwrap();
    studio_project::open(root).unwrap()
}

pub struct FakeCompiler {
    pub started: AtomicUsize,
    pub finished: AtomicUsize,
    pub killed: AtomicUsize,
    /// While false every compile blocks (like a long Cargo build) and notices kills.
    pub release: AtomicBool,
    pub fail: AtomicBool,
    pub install_worker: bool,
    /// When set, `accepts` refuses every key with this reason.
    pub refuse: parking_lot::Mutex<Option<String>>,
}

impl FakeCompiler {
    pub fn new(install_worker: bool) -> Arc<Self> {
        Arc::new(Self {
            started: AtomicUsize::new(0),
            finished: AtomicUsize::new(0),
            killed: AtomicUsize::new(0),
            release: AtomicBool::new(true),
            fail: AtomicBool::new(false),
            install_worker,
            refuse: parking_lot::Mutex::new(None),
        })
    }
    pub fn blocked(install_worker: bool) -> Arc<Self> {
        let compiler = Self::new(install_worker);
        compiler.release.store(false, Ordering::SeqCst);
        compiler
    }
    pub fn wait_started(&self, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while self.started.load(Ordering::SeqCst) < count {
            assert!(
                std::time::Instant::now() < deadline,
                "compile never started"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Compiler for FakeCompiler {
    fn accepts(&self, _key: &fframes_studio::build_service::BuildKey) -> Result<(), String> {
        match self.refuse.lock().clone() {
            Some(reason) => Err(reason),
            None => Ok(()),
        }
    }
    fn compile(
        &self,
        request: &CompileRequest,
        scope: &ProcessTreeManager,
    ) -> Result<Arc<MaterializedBuild>, String> {
        self.started.fetch_add(1, Ordering::SeqCst);
        while !self.release.load(Ordering::SeqCst) {
            if scope.is_shutdown() {
                self.killed.fetch_add(1, Ordering::SeqCst);
                return Err("compile killed".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err("error[E0425]: cannot find value `title` in this scope".into());
        }
        let build = materialize_in_environment(
            &request.project,
            request.environment.sdk_environment(),
            request.environment.builds(),
            &|| scope.is_shutdown(),
        )
        .map_err(|e| {
            eprintln!("FakeCompiler materialization failed: {e:?}");
            e.to_string()
        })?;
        if self.install_worker {
            let target = build.isolated_bin_dir.join(format!(
                "{}{}",
                build.worker_target,
                std::env::consts::EXE_SUFFIX
            ));
            fs::copy(FAKE_WORKER, &target).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
                    .map_err(|e| e.to_string())?;
            }
        }
        self.finished.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(build))
    }
}

pub fn environment(sdk: &Path, builds: &Path) -> CompileEnvironment {
    environment_for(sdk, builds, &manifest())
}

pub fn environment_for(
    sdk: &Path,
    builds: &Path,
    compatibility: &CompatibilityManifest,
) -> CompileEnvironment {
    CompileEnvironment::resolve(sdk, compatibility, builds).unwrap()
}

/// The key the service sees for `project` compiled against `sdk`/`builds`.
pub fn build_key(project: &OpenProject, sdk: &Path, builds: &Path) -> BuildKey {
    BuildKey::worker(project, &environment(sdk, builds))
}

pub fn request(project: &OpenProject, sdk: &Path, builds: &Path) -> CompileRequest {
    CompileRequest {
        project: project.clone(),
        environment: environment(sdk, builds),
        retained: None,
    }
}
