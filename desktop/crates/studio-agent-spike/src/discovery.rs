//! Adapter executable resolution, launch construction and readiness/auth probing.
//!
//! Resolution never consults the process-global `PATH` implicitly: only an absolute
//! configured path, managed adapter directories and a caller-supplied (explicitly
//! resolved GUI) `PATH` are searched. Launch uses an argument array, never a shell.

use crate::{
    AdapterConfig,
    driver::{
        AcpDriver, AgentFailure, DriverConfig, DriverLimits, DriverMode, FailureKind,
        InitializedInfo, McpStdioSupport, Phase,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};
use studio_bootstrap::{ChildEnvironment, ProcessTreeManager, WriterOwnership};

/// Where an adapter executable may be found besides an absolute configured path.
#[derive(Debug, Clone, Default)]
pub struct ExecutableSearch {
    /// Directories owned by the app that hold installed adapters (searched first).
    pub managed_dirs: Vec<PathBuf>,
    /// An explicitly resolved GUI `PATH` value (for example from a login shell).
    /// `None` means bare names resolve only inside `managed_dirs`.
    pub gui_path: Option<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    #[error("adapter executable not found; searched {}", display_paths(.searched))]
    NotFound { searched: Vec<PathBuf> },
    #[error(
        "adapter executable '{0}' is a relative path; configure an absolute path or a bare name"
    )]
    RelativePath(String),
    #[error("adapter executable is empty")]
    Empty,
    #[error("search root '{}' is relative; search roots must be absolute", .0.display())]
    RelativeRoot(PathBuf),
}

fn display_paths(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "no directories".to_owned()
    } else {
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let mut all = vec![dir.join(name)];
        for ext in ["exe", "cmd", "bat", "com"] {
            all.push(dir.join(format!("{name}.{ext}")));
        }
        all
    }
    #[cfg(not(windows))]
    {
        vec![dir.join(name)]
    }
}

impl ExecutableSearch {
    /// Every search root in order: managed directories, then the explicit GUI `PATH`.
    /// Relative roots would resolve against whatever directory the adapter later runs
    /// in, so any relative root is rejected.
    pub fn roots(&self) -> Result<Vec<PathBuf>, ResolveError> {
        let mut dirs: Vec<PathBuf> = self.managed_dirs.clone();
        if let Some(gui) = &self.gui_path {
            dirs.extend(std::env::split_paths(gui));
        }
        // Empty PATH entries mean the current working directory on some platforms.
        // Do not search it, but do not let an otherwise valid PATH entry block launch.
        dirs.retain(|dir| !dir.as_os_str().is_empty());
        match dirs.iter().find(|dir| !dir.is_absolute()) {
            Some(relative) => Err(ResolveError::RelativeRoot(relative.clone())),
            None => Ok(dirs),
        }
    }
}

/// Resolves the configured executable to an absolute file path.
pub fn resolve_executable(
    configured: &str,
    search: &ExecutableSearch,
) -> Result<PathBuf, ResolveError> {
    if configured.trim().is_empty() {
        return Err(ResolveError::Empty);
    }
    let dirs = search.roots()?;
    let path = Path::new(configured);
    if path.is_absolute() {
        return if is_executable_file(path) {
            Ok(path.to_owned())
        } else {
            Err(ResolveError::NotFound {
                searched: vec![path.to_owned()],
            })
        };
    }
    if configured.contains('/') || configured.contains('\\') {
        return Err(ResolveError::RelativePath(configured.to_owned()));
    }
    for dir in &dirs {
        for candidate in candidates(dir, configured) {
            if is_executable_file(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err(ResolveError::NotFound { searched: dirs })
}

/// Everything needed to start one adapter process; no field is a shell string.
#[derive(Clone)]
pub struct AdapterLaunch {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub env: ChildEnvironment,
    /// Values of the configured auth variables; redacted from every output.
    pub secrets: Vec<String>,
    /// Names of the configured auth variables, whatever their names look like.
    pub auth_names: Vec<String>,
    /// Advertised authentication method to select explicitly, if the adapter needs one.
    pub auth_method: Option<String>,
}

impl std::fmt::Debug for AdapterLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The name heuristic of `export_redacted` misses neutral names such as
        // `ACP_OTHER`; every configured auth variable is redacted explicitly.
        let mut env = self.env.export_redacted();
        for name in &self.auth_names {
            if let Some(value) = env.get_mut(name) {
                *value = crate::driver::REDACTION_MARK.to_owned();
            }
        }
        f.debug_struct("AdapterLaunch")
            .field("executable", &self.executable)
            .field("args", &self.args)
            .field("env", &env)
            .field(
                "secrets",
                &format_args!("[{} redacted]", self.secrets.len()),
            )
            .field("auth_method", &self.auth_method)
            .finish()
    }
}

impl AdapterLaunch {
    pub fn resolve(
        config: &AdapterConfig,
        search: &ExecutableSearch,
    ) -> Result<Self, ResolveError> {
        Self::resolve_with_env(config, search, |name| std::env::var(name).ok())
    }

    /// `lookup` supplies auth variable values; only names listed in the configuration
    /// are read and forwarded, and their values become redaction secrets.
    pub fn resolve_with_env(
        config: &AdapterConfig,
        search: &ExecutableSearch,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ResolveError> {
        let executable = resolve_executable(&config.executable, search)?;
        let roots = search.roots()?;
        let mut env = ChildEnvironment::default_allowlist();
        // The ambient PATH is never inherited: an adapter that starts through
        // `/usr/bin/env node` must find its runtime in the explicit absolute roots only.
        env.remove("PATH");
        env.remove("Path");
        if !roots.is_empty() {
            let joined = std::env::join_paths(&roots)
                .map_err(|_| ResolveError::RelativeRoot(PathBuf::from("<invalid path entry>")))?;
            env.set(
                if cfg!(windows) { "Path" } else { "PATH" },
                joined.to_string_lossy(),
            );
        }
        let mut secrets = Vec::new();
        for name in &config.auth_env_names {
            if let Some(value) = lookup(name) {
                secrets.push(value.clone());
                env.set(name, value);
            }
        }
        Ok(Self {
            executable,
            args: config.args.clone(),
            env,
            secrets,
            auth_names: config.auth_env_names.clone(),
            auth_method: None,
        })
    }

    pub fn with_auth_method(mut self, method: impl Into<String>) -> Self {
        self.auth_method = Some(method.into());
        self
    }
}

/// Distinct adapter readiness states surfaced to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdapterStatus {
    /// No executable at the configured path or in the managed/GUI search roots.
    MissingExecutable { searched: Vec<String> },
    /// The adapter file exists but its runtime (interpreter, node, …) cannot run.
    MissingRuntime { detail: String },
    /// The adapter negotiated or violated something other than ACP v1.
    ProtocolMismatch { detail: String },
    /// Initialized, but authentication was not verified (no session was attempted).
    AuthUnknown { methods: Vec<String> },
    /// The adapter reported authentication is required.
    AuthRequired { methods: Vec<String> },
    /// An explicit authentication attempt was rejected.
    AuthRejected { detail: String },
    /// Initialized and a session was created: authentication is proven.
    Ready { agent: String, version: String },
    /// Any other launch/protocol failure, with its structured detail.
    Failed { failure: AgentFailure },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryReport {
    pub status: AdapterStatus,
    pub executable: Option<PathBuf>,
    pub initialized: Option<InitializedInfo>,
}
impl DiscoveryReport {
    /// Returns the negotiated capabilities if the adapter initialized.
    pub fn capabilities(&self) -> Option<&crate::driver::AgentCapabilityInfo> {
        self.initialized.as_ref().map(|i| &i.capabilities)
    }

    /// Whether session restoration (load or resume) was negotiated.
    pub fn supports_restoration(&self) -> bool {
        self.capabilities()
            .is_some_and(|c| c.load_session || c.resume_session)
    }

    /// Whether image prompts were negotiated.
    pub fn supports_images(&self) -> bool {
        self.capabilities().is_some_and(|c| c.prompt_image)
    }

    /// Whether MCP stdio support is active.
    pub fn supports_mcp_stdio(&self) -> bool {
        self.capabilities().is_some_and(|c| c.mcp_stdio)
    }

    /// Summary of available capabilities and limitations for UI display.
    pub fn capability_summary(&self) -> Option<String> {
        let caps = self.capabilities()?;
        let mut parts = Vec::new();
        if caps.load_session || caps.resume_session {
            parts.push("session restore");
        } else {
            parts.push("fresh sessions only");
        }
        if caps.prompt_image {
            parts.push("visual context");
        }
        if caps.mcp_stdio {
            parts.push("stdio MCP");
        } else {
            parts.push("CLI fallback");
        }
        Some(parts.join(", "))
    }
}

/// What a probe may do.
#[derive(Debug, Clone)]
pub struct ProbeOptions {
    /// Empty scratch directory used as the session cwd; never a project or draft.
    pub scratch_cwd: PathBuf,
    /// Create a session to prove authentication. Without it the best result is
    /// [`AdapterStatus::AuthUnknown`].
    pub verify_session: bool,
    pub auth_method: Option<String>,
    pub timeout: Duration,
}

impl ProbeOptions {
    pub fn new(scratch_cwd: PathBuf) -> Self {
        Self {
            scratch_cwd,
            verify_session: true,
            auth_method: None,
            timeout: Duration::from_secs(30),
        }
    }
}

fn status_from_failure(failure: AgentFailure, methods: Vec<String>) -> AdapterStatus {
    match failure.kind {
        FailureKind::MissingRuntime => AdapterStatus::MissingRuntime {
            detail: failure.message,
        },
        FailureKind::UnsupportedVersion => AdapterStatus::ProtocolMismatch {
            detail: failure.message,
        },
        FailureKind::AuthRequired => AdapterStatus::AuthRequired { methods },
        FailureKind::AuthRejected => AdapterStatus::AuthRejected {
            detail: failure.message,
        },
        FailureKind::MalformedMessage
        | FailureKind::TruncatedMessage
        | FailureKind::OversizedMessage
        | FailureKind::ProtocolViolation
            if failure.phase == Phase::Initialize =>
        {
            AdapterStatus::ProtocolMismatch {
                detail: failure.message,
            }
        }
        _ => AdapterStatus::Failed { failure },
    }
}

/// Resolves and launches the adapter once in a scratch directory to report readiness.
/// The probe process is always stopped and its group verified before returning.
pub fn probe_adapter(
    config: &AdapterConfig,
    search: &ExecutableSearch,
    options: &ProbeOptions,
    processes: &ProcessTreeManager,
) -> DiscoveryReport {
    let launch = match AdapterLaunch::resolve(config, search) {
        Ok(launch) => launch,
        Err(error) => {
            let searched = match error {
                ResolveError::NotFound { searched } => {
                    searched.iter().map(|p| p.display().to_string()).collect()
                }
                other => vec![other.to_string()],
            };
            return DiscoveryReport {
                status: AdapterStatus::MissingExecutable { searched },
                executable: None,
                initialized: None,
            };
        }
    };
    let launch = match &options.auth_method {
        Some(method) => launch.with_auth_method(method.clone()),
        None => launch,
    };
    let executable = Some(launch.executable.clone());
    let limits = DriverLimits {
        init_timeout: options.timeout,
        ..DriverLimits::default()
    };
    let driver = match AcpDriver::start(
        DriverConfig {
            provider: config.executable.clone(),
            task: "discovery".into(),
            cwd: options.scratch_cwd.clone(),
            launch,
            limits,
            resume_session: None,
            writer_ownership: WriterOwnership::Unknown,
            mode: if options.verify_session {
                DriverMode::Full
            } else {
                DriverMode::InitializeOnly
            },
            mcp_servers: Vec::new(),
            mcp_stdio: McpStdioSupport::Baseline,
        },
        processes,
    ) {
        Ok(driver) => driver,
        Err(failure) => {
            return DiscoveryReport {
                status: status_from_failure(failure, Vec::new()),
                executable,
                initialized: None,
            };
        }
    };
    let ready = driver.wait_ready(options.timeout + Duration::from_secs(5));
    let initialized = driver.initialized();
    let methods = initialized
        .as_ref()
        .map(|i| i.auth_methods.clone())
        .unwrap_or_default();
    let status = match ready {
        Ok(info) if options.verify_session => AdapterStatus::Ready {
            agent: info.initialized.agent_name,
            version: info.initialized.agent_version,
        },
        Ok(_) => AdapterStatus::AuthUnknown { methods },
        Err(failure) => status_from_failure(failure, methods),
    };
    drop(driver.shutdown());
    DiscoveryReport {
        status,
        executable,
        initialized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn executable(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn empty_and_relative_paths_are_rejected_without_searching() {
        let search = ExecutableSearch::default();
        assert_eq!(resolve_executable(" ", &search), Err(ResolveError::Empty));
        assert!(matches!(
            resolve_executable("bin/adapter", &search),
            Err(ResolveError::RelativePath(_))
        ));
    }

    #[test]
    fn bare_names_never_fall_back_to_the_global_path() {
        // `sh` exists on the global PATH but is not in the (empty) search roots.
        assert!(matches!(
            resolve_executable("sh", &ExecutableSearch::default()),
            Err(ResolveError::NotFound { .. })
        ));
    }

    #[test]
    fn empty_search_path_entries_are_ignored() {
        let cwd = std::env::current_dir().unwrap();
        let gui_path = std::env::join_paths([cwd.as_path(), Path::new("")]).unwrap();
        let search = ExecutableSearch {
            managed_dirs: vec![PathBuf::new()],
            gui_path: Some(gui_path),
        };

        assert_eq!(search.roots().unwrap(), vec![cwd]);
    }

    #[test]
    #[cfg(unix)]
    fn managed_dirs_win_over_the_explicit_gui_path() {
        let managed = tempfile::tempdir().unwrap();
        let gui = tempfile::tempdir().unwrap();
        let managed_exe = executable(managed.path(), "adapter");
        executable(gui.path(), "adapter");
        let search = ExecutableSearch {
            managed_dirs: vec![managed.path().to_owned()],
            gui_path: Some(std::env::join_paths([gui.path()]).unwrap()),
        };
        assert_eq!(resolve_executable("adapter", &search).unwrap(), managed_exe);
        let gui_only = ExecutableSearch {
            managed_dirs: vec![],
            gui_path: search.gui_path.clone(),
        };
        assert!(
            resolve_executable("adapter", &gui_only)
                .unwrap()
                .starts_with(gui.path())
        );
    }

    #[test]
    #[cfg(unix)]
    fn absolute_path_must_be_an_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain");
        std::fs::write(&plain, "x").unwrap();
        let search = ExecutableSearch::default();
        assert!(resolve_executable(plain.to_str().unwrap(), &search).is_err());
        let exe = executable(dir.path(), "exe");
        assert_eq!(
            resolve_executable(exe.to_str().unwrap(), &search).unwrap(),
            exe
        );
        assert!(resolve_executable(dir.path().to_str().unwrap(), &search).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn launch_forwards_only_named_auth_variables_and_keeps_args_as_an_array() {
        let dir = tempfile::tempdir().unwrap();
        let exe = executable(dir.path(), "adapter");
        let config = AdapterConfig {
            executable: exe.to_string_lossy().into(),
            args: vec!["--flag value".into(), "$(rm -rf)".into()],
            auth_env_names: vec!["ACP_KEY".into(), "ACP_OTHER".into()],
        };
        let launch = AdapterLaunch::resolve_with_env(&config, &ExecutableSearch::default(), |n| {
            (n == "ACP_KEY").then(|| "sentinel-secret".to_owned())
        })
        .unwrap();
        assert_eq!(launch.args, vec!["--flag value", "$(rm -rf)"]);
        assert_eq!(launch.env.get("ACP_KEY"), Some("sentinel-secret"));
        assert_eq!(launch.env.get("ACP_OTHER"), None);
        assert_eq!(launch.secrets, vec!["sentinel-secret"]);
        assert!(!format!("{launch:?}").contains("sentinel-secret"));
    }

    #[test]
    #[cfg(unix)]
    fn neutral_named_auth_values_are_redacted_from_debug_output() {
        let dir = tempfile::tempdir().unwrap();
        let exe = executable(dir.path(), "adapter");
        let config = AdapterConfig {
            executable: exe.to_string_lossy().into(),
            args: vec![],
            auth_env_names: vec!["ACP_OTHER".into()],
        };
        let launch = AdapterLaunch::resolve_with_env(&config, &ExecutableSearch::default(), |_| {
            Some("neutral-sentinel-value".to_owned())
        })
        .unwrap();
        // The variable name matches none of the sensitive-name heuristics.
        assert_eq!(
            launch
                .env
                .export_redacted()
                .get("ACP_OTHER")
                .map(String::as_str),
            Some("neutral-sentinel-value")
        );
        assert!(!format!("{launch:?}").contains("neutral-sentinel-value"));
        assert!(format!("{launch:?}").contains("ACP_OTHER"));
    }

    #[test]
    #[cfg(unix)]
    fn runtime_path_is_rebuilt_from_explicit_absolute_roots_only() {
        let dir = tempfile::tempdir().unwrap();
        let exe = executable(dir.path(), "adapter");
        let config = AdapterConfig {
            executable: exe.to_string_lossy().into(),
            args: vec![],
            auth_env_names: vec![],
        };
        // No explicit roots: no PATH at all, whatever the ambient PATH is.
        let launch =
            AdapterLaunch::resolve_with_env(&config, &ExecutableSearch::default(), |_| None)
                .unwrap();
        assert!(
            std::env::var_os("PATH").is_some(),
            "test needs an ambient PATH"
        );
        assert_eq!(launch.env.get("PATH"), None);
        assert_eq!(launch.env.get("Path"), None);

        let managed = tempfile::tempdir().unwrap();
        let gui = tempfile::tempdir().unwrap();
        let search = ExecutableSearch {
            managed_dirs: vec![managed.path().to_owned()],
            gui_path: Some(std::env::join_paths([gui.path()]).unwrap()),
        };
        let launch = AdapterLaunch::resolve_with_env(&config, &search, |_| None).unwrap();
        let expected = format!("{}:{}", managed.path().display(), gui.path().display());
        assert_eq!(launch.env.get("PATH"), Some(expected.as_str()));
    }

    #[test]
    #[cfg(unix)]
    fn relative_search_roots_are_rejected_everywhere() {
        let dir = tempfile::tempdir().unwrap();
        let exe = executable(dir.path(), "adapter");
        let relative_managed = ExecutableSearch {
            managed_dirs: vec![PathBuf::from("bin")],
            gui_path: None,
        };
        let relative_gui = ExecutableSearch {
            managed_dirs: vec![],
            gui_path: Some(OsString::from("/usr/bin:relative/bin")),
        };
        for search in [relative_managed, relative_gui] {
            assert!(matches!(
                resolve_executable("adapter", &search),
                Err(ResolveError::RelativeRoot(_))
            ));
            // Even an absolute executable is refused when a root could mislead the launch.
            let config = AdapterConfig {
                executable: exe.to_string_lossy().into(),
                args: vec![],
                auth_env_names: vec![],
            };
            assert!(matches!(
                AdapterLaunch::resolve_with_env(&config, &search, |_| None),
                Err(ResolveError::RelativeRoot(_))
            ));
        }
        // Every returned candidate is absolute.
        let search = ExecutableSearch {
            managed_dirs: vec![dir.path().to_owned()],
            gui_path: None,
        };
        assert!(
            resolve_executable("adapter", &search)
                .unwrap()
                .is_absolute()
        );
    }
}
