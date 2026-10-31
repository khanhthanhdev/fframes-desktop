//! Real-subprocess transport scenarios for the SDK-based ACP v1 driver.
//!
//! The peer is `tests/acp-peer.py`: it proves transport, framing, lifecycle and
//! process-ownership behavior only and can never qualify authentication or a provider.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use studio_bootstrap::ProcessTreeManager;
#[cfg(unix)]
use studio_bootstrap::WriterOwnership;
#[cfg(unix)]
use studio_harness::driver::DriverOutcome;
use studio_harness::{
    AdapterConfig,
    discovery::{
        AdapterLaunch, AdapterStatus, ExecutableSearch, ProbeOptions, ResolveError, probe_adapter,
    },
    driver::{
        AcpDriver, AgentEvent, AgentEventKind, DriverConfig, DriverError, DriverLimits, DriverMode,
        DriverStatus, FailureKind, McpConfigError, McpHttpServer, McpStdioServer, McpStdioSupport,
        MessageRole, OptionValue, OwnershipSource, PermissionReply, PermissionResolution, Phase,
        PromptImage, StopReasonKind, UserMcpLaunch,
    },
};

mod negotiation;
mod scenarios;
mod services;
mod turns;

const SECRET: &str = "sentinel-credential-7f3a9c";
const MCP_SECRET: &str = "mcp-sentinel-env-5d2b81";
const WAIT: Duration = Duration::from_secs(20);

fn peer_script() -> String {
    format!("{}/tests/acp-peer.py", env!("CARGO_MANIFEST_DIR"))
}

/// The fixture peer interpreter.
fn python() -> String {
    studio_test_support::python()
        .to_str()
        .expect("the Python path is UTF-8")
        .to_owned()
}

fn search() -> ExecutableSearch {
    // The "GUI PATH" is passed explicitly; nothing consults the global PATH implicitly.
    ExecutableSearch {
        managed_dirs: vec![],
        gui_path: std::env::var_os("PATH"),
        node: None,
    }
}

fn launch(root: &Path, mode: &str) -> AdapterLaunch {
    let config = AdapterConfig {
        executable: python(),
        args: vec![peer_script(), mode.into(), root.to_string_lossy().into()],
        auth_env_names: vec!["ACP_SECRET".into()],
    };
    AdapterLaunch::resolve_with_env(&config, &search(), |name| {
        (name == "ACP_SECRET").then(|| SECRET.to_owned())
    })
    .expect("Python resolves through the explicit search path")
}

fn config(root: &Path, mode: &str) -> DriverConfig {
    DriverConfig {
        provider: "fixture".into(),
        task: "task-1".into(),
        cwd: root.to_owned(),
        launch: launch(root, mode),
        limits: DriverLimits::default(),
        resume_session: None,
        ownership: OwnershipSource::Verified,
        mode: DriverMode::Full,
        mcp_servers: Vec::new(),
        user_mcp_servers: Vec::new(),
        mcp_stdio: McpStdioSupport::default(),
    }
}

fn start(root: &Path, mode: &str) -> (AcpDriver, ProcessTreeManager) {
    start_with(config(root, mode))
}

fn start_with(config: DriverConfig) -> (AcpDriver, ProcessTreeManager) {
    let manager = ProcessTreeManager::new();
    let driver = AcpDriver::start(config, &manager).expect("adapter spawns");
    (driver, manager)
}

fn ready(driver: &AcpDriver) {
    driver.wait_ready(WAIT).expect("session becomes ready");
}

fn collect_until(driver: &AcpDriver, mut done: impl FnMut(&AgentEvent) -> bool) -> Vec<AgentEvent> {
    let deadline = Instant::now() + WAIT;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        if let Some(event) = driver.next_event(Duration::from_millis(100)) {
            let stop = done(&event);
            events.push(event);
            if stop {
                return events;
            }
        }
    }
    panic!("expected event never arrived; saw {events:#?}");
}

fn wait_for<T>(mut poll: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(value) = poll() {
            return value;
        }
        assert!(Instant::now() < deadline, "condition never became true");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn failure_of(driver: &AcpDriver) -> studio_harness::driver::AgentFailure {
    driver
        .wait_ready(WAIT)
        .err()
        .or_else(|| driver.failure())
        .expect("a failure")
}

fn read_json(root: &Path, name: &str) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join(name)).unwrap()).unwrap()
}

#[cfg(unix)]
fn pid_running(pid: i32) -> bool {
    // SAFETY: signal 0 only probes existence.
    let exists = unsafe { libc::kill(pid, 0) } == 0;
    // A zombie still answers signal 0.
    exists && process_state(pid).is_some_and(|state| !state.starts_with('Z'))
}

#[cfg(target_os = "linux")]
fn process_state(pid: i32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit(") ").next().map(str::to_owned)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_state(pid: i32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let state = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!state.is_empty()).then_some(state)
}

#[cfg(unix)]
fn kill_known_pid(pid: i32) {
    // SAFETY: only ever called with a pid this test read from its own fixture.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(unix)]
fn helper_pid(root: &Path) -> i32 {
    wait_for(|| {
        std::fs::read_to_string(root.join("helper.pid"))
            .ok()
            .and_then(|t| t.trim().parse().ok())
    })
}
