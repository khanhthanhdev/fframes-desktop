use fframes_studio::{
    agent_workflow::*,
    build_service::{BuildKey, BuildLimits, BuildService, CompileRequest, Compiler},
    preview_coordinator::{PreviewCoordinator, SeekIntent},
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use studio_agent_spike::{AdapterConfig, ExecutableSearch, McpStdioSupport, driver::OptionValue};
use studio_bootstrap::{ProcessTreeManager, WriterOwnership};
use studio_engine::{
    AgentTaskId, Boundary, CanvasTaskSelection, CanvasTaskSelectionKind, CompiledScope,
    Controller, DraftState, Fault, ScopeSelection, TaskScope, TaskState, TransactionHooks,
    TimelineSelection, VideoPixelRect, app_paths::AppPaths,
    build_materialization::MaterializedBuild,
};
use fframes_studio_protocol::{EditorGeometrySupport, EditorObjectIdentity, EditorSourceAnchor};

#[path = "build_fixture.rs"]
mod fixture;
use fixture::*;
#[path = "preview_fixture.rs"]
#[allow(dead_code)]
mod preview_fixture;

const WAIT: Duration = Duration::from_secs(90);
const GOOD_CONFIG: &str = r#"{"frames":120,"tracks":[[0.0,2.0]],"audio":"tone","pixel":40}"#;

fn agent_script() -> String {
    format!("{}/tests/support/acp-agent.py", env!("CARGO_MANIFEST_DIR"))
}

fn qualified() -> WriterOwnership {
    WriterOwnership::ProcessGroupContained {
        qualification: "scripted-fixture".into(),
    }
}

/// A script that validates: a new `src/lib.rs` plus the fake worker's timeline config.
fn good(marker: &str) -> Value {
    json!({
        "text": ["Working on it. ", "done"],
        "tool": 2,
        "write": {"src/lib.rs": format!("// {marker}\n"), FAKE_CONFIG: GOOD_CONFIG},
    })
}

/// A script whose edit the scripted compiler refuses.
fn broken() -> Value {
    json!({"text": "oops", "write": {"src/lib.rs": "// BROKEN\n", FAKE_CONFIG: GOOD_CONFIG}})
}

/// Fails to compile while the restored candidate's `src/lib.rs` says BROKEN.
struct Scripted {
    inner: Arc<FakeCompiler>,
}

impl Compiler for Scripted {
    fn accepts(&self, key: &BuildKey) -> Result<(), String> {
        self.inner.accepts(key)
    }
    fn compile(
        &self,
        request: &CompileRequest,
        scope: &ProcessTreeManager,
    ) -> Result<Arc<MaterializedBuild>, String> {
        let lib = fs::read_to_string(request.project.root.join("src/lib.rs")).unwrap_or_default();
        if lib.contains("BROKEN") {
            return Err(format!(
                "error[E0425]: cannot find value `BROKEN` in this scope\n  --> src/lib.rs:1\n   | {}",
                lib.trim()
            ));
        }
        self.inner.compile(request, scope)
    }
}

struct Options {
    mcp: McpStdioSupport,
    /// The `studio-mcp` helper is configured / the `studio-tools` helper is configured.
    mcp_helper: bool,
    cli_helper: bool,
    ownership: WriterOwnership,
    handoff: Option<Arc<dyn PreviewHandoff>>,
    row_limits: Option<fframes_studio::agent_workflow::log::RowLimits>,
    hooks: Option<Arc<dyn TransactionHooks>>,
    /// Command prefix before `python3` (e.g. `env VAR=x`).
    wrapper: Vec<String>,
    executable: Option<String>,
    adapter: bool,
    block_compiler: bool,
    /// The real SDK and its manifest: real Cargo compiles and the real preview worker.
    real: Option<(PathBuf, studio_sdk::CompatibilityManifest)>,
    /// Generate the production Studio starter instead of the synthetic timeline fixture.
    starter_project: bool,
    /// Failure injection for background jobs.
    job_faults: Option<JobFaults>,
    /// A configured credential value (auth variable `SCRIPTED_TOKEN`).
    secret: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mcp: McpStdioSupport::Baseline,
            mcp_helper: true,
            cli_helper: true,
            ownership: qualified(),
            handoff: None,
            row_limits: None,
            hooks: None,
            wrapper: Vec::new(),
            executable: None,
            adapter: true,
            block_compiler: false,
            real: None,
            starter_project: false,
            job_faults: None,
            secret: None,
        }
    }
}

struct World {
    _temp: tempfile::TempDir,
    runtime: tempfile::TempDir,
    sdk: PathBuf,
    paths: AppPaths,
    root: PathBuf,
    agent: PathBuf,
    controller: Option<Arc<Mutex<Controller>>>,
    service: BuildService,
    compiler: Arc<FakeCompiler>,
    workflow: Option<Arc<AgentWorkflow>>,
    options: Options,
    manifest: studio_sdk::CompatibilityManifest,
    limit: Duration,
}

impl World {
    fn new() -> Self {
        Self::with(Options::default())
    }

    fn with(options: Options) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let runtime = tempfile::Builder::new()
            .prefix("fft")
            .tempdir_in("/tmp")
            .unwrap();
        let root = temp.path().join("video");
        let (sdk, manifest, limit) = match &options.real {
            Some((sdk, manifest)) => {
                if options.starter_project {
                    studio_project::create(
                        &root,
                        "M5 selected-title workflow",
                        studio_engine::build_materialization::sdk_pin(manifest),
                        &manifest.fframes_version,
                        "0.1.0",
                    )
                    .unwrap();
                } else {
                    preview_fixture::create(&root, manifest);
                }
                (sdk.clone(), manifest.clone(), Duration::from_secs(1200))
            }
            None => {
                create_project(&root);
                (fake_sdk(temp.path()), manifest(), WAIT)
            }
        };
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let agent = temp.path().join("agent-evidence");
        fs::create_dir_all(&agent).unwrap();
        let compiler = if options.block_compiler {
            FakeCompiler::blocked(true)
        } else {
            FakeCompiler::new(true)
        };
        let real = options.real.is_some();
        let compile: Arc<dyn Compiler> = if real {
            Arc::new(fframes_studio::worker_project::CargoCompiler)
        } else {
            Arc::new(Scripted {
                inner: compiler.clone(),
            })
        };
        let service = BuildService::new(ProcessTreeManager::new(), compile, BuildLimits::default());
        let mut world = Self {
            _temp: temp,
            runtime,
            sdk,
            paths,
            root,
            agent,
            controller: None,
            service,
            compiler,
            workflow: None,
            options,
            manifest,
            limit,
        };
        world.open();
        world
    }

    fn open(&mut self) {
        let controller = match &self.options.hooks {
            Some(hooks) => {
                Controller::open_with_hooks(&self.root, &self.paths, hooks.clone()).unwrap()
            }
            None => Controller::open(&self.root, &self.paths).unwrap(),
        };
        let controller = Arc::new(Mutex::new(controller));
        let config = self.config();
        self.workflow = Some(Arc::new(
            AgentWorkflow::open(config, controller.clone()).unwrap(),
        ));
        self.controller = Some(controller);
    }

    fn config(&self) -> WorkflowConfig {
        let mut config = WorkflowConfig::new(self.paths.clone());
        config.search = ExecutableSearch {
            managed_dirs: vec![],
            gui_path: std::env::var_os("PATH"),
        };
        config.build = Some(BuildSettings {
            service: self.service.clone(),
            sdk: self.sdk.clone(),
            compatibility: self.manifest.clone(),
        });
        config.tools = Some(ToolSettings {
            runtime_dir: self.runtime.path().join("rt"),
            studio_mcp: self
                .options
                .mcp_helper
                .then(|| PathBuf::from(env!("CARGO_BIN_EXE_studio-mcp"))),
            studio_tools: self
                .options
                .cli_helper
                .then(|| PathBuf::from(env!("CARGO_BIN_EXE_studio-tools"))),
        });
        config.limits.shutdown_grace = Duration::from_secs(1);
        config.probe_timeout = Duration::from_secs(20);
        config.handoff = self.options.handoff.clone();
        config.job_faults = self.options.job_faults.clone();
        if let Some(secret) = self.options.secret.clone() {
            config.auth_env =
                Arc::new(move |name| (name == "SCRIPTED_TOKEN").then(|| secret.clone()));
        }
        if let Some(limits) = self.options.row_limits {
            config.row_limits = limits;
        }
        if self.options.adapter {
            config.adapter = Some(self.adapter_settings());
        }
        config
    }

    fn adapter_settings(&self) -> AdapterSettings {
        let mut args: Vec<String> = Vec::new();
        let executable = match (&self.options.executable, self.options.wrapper.split_first()) {
            (Some(executable), _) => executable.clone(),
            (None, Some((first, rest))) => {
                args.extend(rest.iter().cloned());
                args.push("python3".into());
                first.clone()
            }
            (None, None) => "python3".to_owned(),
        };
        args.push(agent_script());
        args.push(self.agent.to_string_lossy().into_owned());
        AdapterSettings {
            provider: "scripted".into(),
            adapter: AdapterConfig {
                executable,
                args,
                auth_env_names: if self.options.secret.is_some() {
                    vec!["SCRIPTED_TOKEN".into()]
                } else {
                    vec![]
                },
            },
            writer_ownership: self.options.ownership.clone(),
            ownership_probe: None,
            mcp: self.options.mcp,
            auth_method: None,
        }
    }

    /// Simulates a restart: the workflow closes, the controller is dropped and reopened
    /// (running the engine's open-time recovery), and a new workflow starts.
    fn restart(&mut self) {
        self.workflow = None;
        self.controller = None;
        self.open();
    }

    fn wf(&self) -> &AgentWorkflow {
        self.workflow.as_ref().unwrap()
    }

    fn ctl(&self) -> Arc<Mutex<Controller>> {
        self.controller.clone().unwrap()
    }

    fn snap(&self) -> Arc<WorkflowSnapshot> {
        self.wf().snapshot()
    }

    fn wait(
        &self,
        what: &str,
        predicate: impl Fn(&WorkflowSnapshot) -> bool,
    ) -> Arc<WorkflowSnapshot> {
        self.wf()
            .wait_for(self.limit, predicate)
            .unwrap_or_else(|| {
                let s = self.snap();
                panic!(
                    "timed out waiting for {what}; task phase {:?}, error {:?}, queue {}, rows {}",
                    s.task.as_ref().map(|t| t.phase),
                    s.task.as_ref().and_then(|t| t.error.clone()),
                    s.queue.len(),
                    s.rows.len()
                )
            })
    }

    /// Waits for something outside the snapshot (a file on disk): polls, never blocks on
    /// the snapshot condvar, which only fires when the workflow publishes.
    fn poll(&self, what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + self.limit;
        while !condition() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_phase(&self, phase: TaskPhase) -> TaskView {
        self.wait(&format!("phase {phase:?}"), |s| {
            s.task.as_ref().is_some_and(|t| t.phase == phase)
        })
        .task
        .clone()
        .unwrap()
    }

    /// The first terminal task other than `after`.
    fn wait_task(&self, after: Option<&AgentTaskId>) -> TaskView {
        self.wait("a finished task", |s| {
            s.task
                .as_ref()
                .is_some_and(|t| Some(&t.id) != after && t.phase.is_terminal())
        })
        .task
        .clone()
        .unwrap()
    }

    fn brief(script: &Value) -> String {
        format!("SCRIPT {script}\nplease make the edit")
    }

    fn submit(&self, script: &Value) {
        self.wf().submit(&Self::brief(script)).unwrap();
    }

    fn set_plan(&self, turns: &[Value]) {
        fs::write(
            self.agent.join("plan.json"),
            json!({"turns": turns, "default": good("default")}).to_string(),
        )
        .unwrap();
        let _ = fs::remove_file(self.agent.join("prompt-counter"));
    }

    fn submit_plan(&self, turns: &[Value]) {
        self.set_plan(turns);
        self.wf().submit("plan brief").unwrap();
    }

    fn submit_scoped_plan(&self, brief: &str, scope: TaskScope, turns: &[Value]) {
        self.set_plan(turns);
        self.wf().submit_scoped(brief, scope).unwrap();
    }

    fn evidence(&self, name: &str) -> Vec<Value> {
        fs::read_to_string(self.agent.join(name))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn wait_prompts(&self, count: usize) {
        let deadline = Instant::now() + self.limit;
        while self.evidence("prompts.jsonl").len() < count {
            assert!(
                Instant::now() < deadline,
                "the agent never got prompt {count}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn release(&self, name: &str) {
        fs::write(self.agent.join(name), "go").unwrap();
    }

    fn source(&self, relative: &str) -> String {
        fs::read_to_string(self.root.join(relative)).unwrap()
    }

    fn engine_state(&self) -> Option<TaskState> {
        self.ctl().lock().agent_task().map(|t| t.state())
    }

    fn draft_state(&self) -> Option<DraftState> {
        self.ctl().lock().agent_draft_state().unwrap()
    }

    fn processes(&self) -> usize {
        self.ctl().lock().processes.active_count()
    }

    /// No owned process, broker capability, tool worker or build lease remains.
    fn assert_clean(&self) {
        assert_eq!(self.processes(), 0, "an owned process survived");
        // Resource counts are published with snapshots: ask for a fresh one.
        let before = self.snap();
        if !before.closed && self.wf().refresh().is_ok() {
            self.wf()
                .wait_changed(before.revision, Duration::from_secs(5));
        }
        let s = self.snap();
        assert_eq!(s.resources.broker_grants, 0, "a broker capability survived");
        assert_eq!(s.resources.tool_workers, 0, "a tool worker survived");
        assert_eq!(s.resources.owned_processes, 0);
        let stats = self.service.stats();
        assert_eq!(stats.leased_entries, 0, "a build lease survived");
        assert_eq!(stats.in_flight, 0, "a compile subscription survived");
    }
}

impl Drop for World {
    fn drop(&mut self) {
        // Reap any helper a test deliberately left behind.
        for name in ["helper.pid", "escape.pid"] {
            if let Ok(text) = fs::read_to_string(self.agent.join(name)) {
                for pid in text
                    .split_whitespace()
                    .filter_map(|p| p.parse::<i32>().ok())
                {
                    unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
        }
    }
}

fn pid_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn has_error(s: &WorkflowSnapshot, code: &str) -> bool {
    s.rows
        .iter()
        .any(|r| matches!(&r.kind, RowKind::Error(e) if e.code == code))
}
