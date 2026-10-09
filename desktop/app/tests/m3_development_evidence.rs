//! M3 DEVELOPMENT evidence: machine-readable measurements of the native agent workflow
//! against the scripted ACP peer (`tests/support/acp-agent.py`), the deterministic fake
//! preview worker and the engine's real controller.
//!
//! This is fixture evidence. It proves resource bounds, build sharing, cleanup and tool
//! parity of OUR code under a scripted peer; it never qualifies a provider, an adapter,
//! authentication, a writer process-group model, a physical device or another OS, and the
//! ledger can never turn it into an authentic pass.
//!
//! Without `FFRAMES_M3_EVIDENCE_OUT` this is a quick smoke (a few cycles, nothing is
//! written). With it set to a directory the full profile runs (at least 20 accepted edits
//! and 20 failed/cancelled tasks, a long streamed transcript) and one JSON file per
//! measurement group is written there, plus `summary.json`. Run it as:
//!
//! ```text
//! FFRAMES_M3_EVIDENCE_OUT=/some/dir cargo test --locked -p fframes-studio \
//!     --test m3_development_evidence -- --nocapture
//! ```
#![cfg(unix)]

use fframes_studio::{
    agent_workflow::{log::RowLimits, *},
    build_service::{BuildLimits, BuildService, Compiler, Subscriber, SubscriberKind},
};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use studio_agent_spike::{AdapterConfig, ExecutableSearch, McpStdioSupport, driver::DriverLimits};
use studio_bootstrap::{ProcessTreeManager, WriterOwnership};
use studio_engine::{AgentTaskId, Controller, app_paths::AppPaths};

#[path = "support/build_fixture.rs"]
mod fixture;
use fixture::*;

const WAIT: Duration = Duration::from_secs(120);
const GOOD_CONFIG: &str = r#"{"frames":120,"tracks":[[0.0,2.0]],"audio":"tone","pixel":40}"#;
const MEASUREMENT_SCHEMA: u32 = 1;

fn agent_script() -> String {
    format!("{}/tests/support/acp-agent.py", env!("CARGO_MANIFEST_DIR"))
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
    fn accepts(&self, key: &fframes_studio::build_service::BuildKey) -> Result<(), String> {
        self.inner.accepts(key)
    }
    fn compile(
        &self,
        request: &fframes_studio::build_service::CompileRequest,
        scope: &ProcessTreeManager,
    ) -> Result<Arc<studio_engine::build_materialization::MaterializedBuild>, String> {
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

/// One open project with a real controller, the workflow, a shared build service and the
/// scripted peer. A trimmed copy of the `agent_workflow.rs` harness.
struct World {
    _temp: tempfile::TempDir,
    runtime: tempfile::TempDir,
    paths: AppPaths,
    root: PathBuf,
    agent: PathBuf,
    controller: Arc<Mutex<Controller>>,
    service: BuildService,
    workflow: Arc<AgentWorkflow>,
}

impl World {
    fn new(row_limits: Option<RowLimits>) -> Self {
        Self::build(row_limits, None)
    }

    /// `adapter`: a real adapter to configure instead of the scripted peer.
    fn build(row_limits: Option<RowLimits>, adapter: Option<AdapterConfig>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        // Unix socket paths are ~100 bytes: keep the broker's directory short.
        let runtime = tempfile::Builder::new()
            .prefix("fft")
            .tempdir_in("/tmp")
            .unwrap();
        let root = temp.path().join("video");
        create_project(&root);
        let sdk = fake_sdk(temp.path());
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let agent = temp.path().join("agent-evidence");
        fs::create_dir_all(&agent).unwrap();
        let compiler = FakeCompiler::new(true);
        let service = BuildService::new(
            ProcessTreeManager::new(),
            Arc::new(Scripted {
                inner: compiler.clone(),
            }),
            BuildLimits::default(),
        );
        let controller = Arc::new(Mutex::new(Controller::open(&root, &paths).unwrap()));
        let mut config = WorkflowConfig::new(paths.clone());
        config.search = ExecutableSearch {
            managed_dirs: vec![],
            gui_path: std::env::var_os("PATH"),
        };
        config.build = Some(BuildSettings {
            service: service.clone(),
            sdk,
            compatibility: manifest(),
        });
        config.tools = Some(ToolSettings {
            runtime_dir: runtime.path().join("rt"),
            studio_mcp: Some(PathBuf::from(env!("CARGO_BIN_EXE_studio-mcp"))),
            studio_tools: Some(PathBuf::from(env!("CARGO_BIN_EXE_studio-tools"))),
        });
        config.limits.shutdown_grace = Duration::from_secs(1);
        config.probe_timeout = Duration::from_secs(20);
        if let Some(limits) = row_limits {
            config.row_limits = limits;
        }
        config.adapter = Some(match adapter {
            Some(adapter) => AdapterSettings {
                provider: "operator-adapter".into(),
                adapter,
                // Nothing here qualifies a writer model: it stays unproven.
                writer_ownership: WriterOwnership::Unknown,
                ownership_probe: None,
                mcp: McpStdioSupport::Baseline,
                auth_method: None,
            },
            None => AdapterSettings {
                provider: "scripted".into(),
                adapter: AdapterConfig {
                    executable: "python3".into(),
                    args: vec![agent_script(), agent.to_string_lossy().into_owned()],
                    auth_env_names: vec![],
                },
                // The fixture is asserted to contain its processes in its own group; this
                // is test configuration, not an adapter qualification.
                writer_ownership: WriterOwnership::ProcessGroupContained {
                    qualification: "scripted-fixture".into(),
                },
                ownership_probe: None,
                mcp: McpStdioSupport::Baseline,
                auth_method: None,
            },
        });
        let workflow = Arc::new(AgentWorkflow::open(config, controller.clone()).unwrap());
        Self {
            _temp: temp,
            runtime,
            paths,
            root,
            agent,
            controller,
            service,
            workflow,
        }
    }

    fn snap(&self) -> Arc<WorkflowSnapshot> {
        self.workflow.snapshot()
    }

    fn wait(
        &self,
        what: &str,
        predicate: impl Fn(&WorkflowSnapshot) -> bool,
    ) -> Arc<WorkflowSnapshot> {
        self.workflow.wait_for(WAIT, predicate).unwrap_or_else(|| {
            let s = self.snap();
            panic!(
                "timed out waiting for {what}; phase {:?}, error {:?}, rows {}",
                s.task.as_ref().map(|t| t.phase),
                s.task.as_ref().and_then(|t| t.error.clone()),
                s.rows.len()
            )
        })
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

    fn submit(&self, script: &Value) {
        self.workflow
            .submit(&format!("SCRIPT {script}\nplease make the edit"))
            .unwrap();
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
        self.workflow.submit("plan brief").unwrap();
    }

    fn prompts(&self) -> Vec<Value> {
        fs::read_to_string(self.agent.join("prompts.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn wait_prompts(&self, count: usize) {
        let deadline = Instant::now() + WAIT;
        while self.prompts().len() < count {
            assert!(
                Instant::now() < deadline,
                "the agent never got prompt {count}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn processes(&self) -> usize {
        self.controller.lock().processes.active_count()
    }

    /// Capability files the broker still holds on disk (`cap-<id>.json`).
    fn capability_files(&self) -> usize {
        fs::read_dir(self.runtime.path().join("rt"))
            .map(|dir| {
                dir.filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().starts_with("cap-"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// Every adapter process the scripted peer recorded is gone.
    fn adapter_pids(&self) -> Vec<i64> {
        let mut pids: Vec<i64> = self
            .prompts()
            .iter()
            .filter_map(|p| p["pid"].as_i64())
            .collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// A point-in-time view of everything this project owns.
    fn ownership(&self) -> Value {
        let s = self.snap();
        let stats = self.service.stats();
        json!({
            "owned_processes": self.processes(),
            "workflow_owned_processes": s.resources.owned_processes,
            "broker_grants": s.resources.broker_grants,
            "tool_workers": s.resources.tool_workers,
            "capability_files": self.capability_files(),
            "build_leased_entries": stats.leased_entries,
            "build_in_flight": stats.in_flight,
            "build_cached_entries": stats.cached_entries,
            "open_permissions": s.resources.open_permissions,
            "resident_rows": s.resources.resident_rows,
        })
    }

    /// Waits (bounded) until nothing is owned any more and returns the settle time. The
    /// resource counters are published with snapshots, so a very fresh terminal task can
    /// briefly show the previous counts.
    fn settle(&self) -> (Duration, Value) {
        let started = Instant::now();
        loop {
            let view = self.ownership();
            let clean = [
                "owned_processes",
                "workflow_owned_processes",
                "broker_grants",
                "tool_workers",
                "capability_files",
                "build_leased_entries",
                "build_in_flight",
            ]
            .iter()
            .all(|k| view[k] == 0);
            if clean || started.elapsed() > Duration::from_secs(10) {
                return (started.elapsed(), view);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn pid_alive(pid: i64) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn assert_zero(view: &Value, context: &str) {
    for key in [
        "owned_processes",
        "workflow_owned_processes",
        "broker_grants",
        "tool_workers",
        "capability_files",
        "build_leased_entries",
        "build_in_flight",
    ] {
        assert_eq!(view[key], 0, "{context}: {key} in {view}");
    }
}

/// Every row of the conversation, resident or paged back from the log.
fn every_row(w: &World) -> std::collections::BTreeMap<u64, Arc<Row>> {
    let mut all = std::collections::BTreeMap::new();
    let mut before = RowId(u64::MAX);
    loop {
        let page = w.workflow.history_page(before, MAX_HISTORY_PAGE).unwrap();
        let Some(oldest) = page.first().map(|r| r.id) else {
            break;
        };
        before = oldest;
        for row in page {
            all.insert(row.id.0, row);
        }
    }
    for row in &w.snap().rows {
        all.insert(row.id.0, row.clone());
    }
    all
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

// ---- profile --------------------------------------------------------------------------------------

struct Profile {
    full: bool,
    out: Option<PathBuf>,
    edit_cycles: usize,
    failure_cycles_per_kind: usize,
    text_chunks: usize,
    flood_tools: usize,
}

impl Profile {
    fn from_env() -> Self {
        let out = std::env::var_os("FFRAMES_M3_EVIDENCE_OUT")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        let full = out.is_some();
        Self {
            full,
            out,
            edit_cycles: if full { 20 } else { 2 },
            failure_cycles_per_kind: if full { 5 } else { 1 },
            // Full: ~4.8 MB of text in 16 KiB rows and 1200 tool cards, both far beyond
            // the 400-row / 4 MiB resident window. Smoke: only the row bound.
            text_chunks: if full { 320 } else { 8 },
            flood_tools: if full { 1200 } else { 401 },
        }
    }

    fn write(&self, name: &str, mut value: Value) {
        let Some(out) = &self.out else { return };
        fs::create_dir_all(out).unwrap();
        let object = value.as_object_mut().unwrap();
        object.insert("schema_version".into(), json!(MEASUREMENT_SCHEMA));
        object.insert("evidence_kind".into(), json!("development"));
        object.insert("fixture_only".into(), json!(true));
        object.insert(
            "scope".into(),
            json!("scripted ACP peer + fake preview worker + real controller; no provider, adapter, account, device or other OS"),
        );
        let text = serde_json::to_string_pretty(&value).unwrap();
        fs::write(out.join(name), format!("{text}\n")).unwrap();
    }
}

// ---- 1. resource bounds under a long streamed transcript -------------------------------------------------------

fn resource_bounds(profile: &Profile) -> Value {
    let w = World::new(None);
    let chunk_len = 15_000;
    let chunks: Vec<String> = (0..profile.text_chunks)
        .map(|i| {
            format!(
                "{i:04} {}",
                "streamed words ".repeat(chunk_len / 15)[..chunk_len - 5].to_owned()
            )
        })
        .collect();
    let mut script = good("streamed");
    script["text"] = json!(chunks);
    script["flood_tools"] = json!(profile.flood_tools);
    // Keep each burst small and give the actor time to drain on slower native runners.
    // The smoke profile crosses the 400-row cap by one card; both profiles still send
    // more than the driver's 256-event queue without relying on an unbounded producer.
    script["flood_pace"] = json!({"every": 5, "sleep_ms": 250});
    w.set_plan(&[script]);
    let started = Instant::now();
    w.workflow.submit("plan brief").unwrap();

    let (mut peak_rows, mut peak_bytes, mut peak_queue, mut snapshots) =
        (0usize, 0usize, 0usize, 0u64);
    let mut last_revision = 0;
    let task = loop {
        let s = w.snap();
        if s.revision != last_revision {
            last_revision = s.revision;
            snapshots += 1;
        }
        peak_rows = peak_rows.max(s.rows.len()).max(s.resources.resident_rows);
        peak_bytes = peak_bytes.max(s.resources.resident_bytes);
        peak_queue = peak_queue.max(s.resources.queue_len);
        if let Some(task) = s
            .task
            .as_ref()
            .filter(|t| t.phase.is_terminal() || t.phase == TaskPhase::AwaitingReview)
        {
            break task.clone();
        }
        assert!(
            started.elapsed() < WAIT,
            "the flooded task never finished; phase {:?}, error {:?}, rows {}, queue {}",
            s.task.as_ref().map(|task| task.phase),
            s.task.as_ref().and_then(|task| task.error.as_ref()),
            s.rows.len(),
            s.resources.queue_len
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    let streamed_for = started.elapsed();
    #[cfg(target_os = "linux")]
    assert_eq!(task.phase, TaskPhase::Accepted, "{:?}", task.error);
    #[cfg(not(target_os = "linux"))]
    {
        assert_eq!(task.phase, TaskPhase::AwaitingReview, "{:?}", task.error);
        assert!(
            task.review
                .as_ref()
                .is_some_and(|review| review.apply_blocked.is_some()),
            "unsupported publication must retain the validated candidate for review"
        );
        assert_eq!(
            task.error.as_ref().map(|error| error.code.as_str()),
            Some("apply_blocked"),
            "unsupported publication must be reported as apply_blocked"
        );
    }
    let s = w.snap();
    assert!(
        !s.rows
            .iter()
            .any(|r| matches!(&r.kind, RowKind::Error(e) if e.code.contains("overflow"))),
        "the provider event queue overflowed"
    );
    let max_rows = s.resources.max_resident_rows;
    let max_bytes = s.resources.max_resident_bytes;
    assert_eq!(
        (max_rows, max_bytes),
        (MAX_RESIDENT_ROWS, MAX_RESIDENT_ROW_BYTES)
    );
    // Resident rows are hard-bounded; the newest row is never evicted, so the byte bound
    // may be exceeded by at most that one row (documented behaviour of the row store).
    let one_row = MAX_ROW_TEXT_BYTES + ROW_OVERHEAD_BYTES;
    assert!(
        peak_rows <= max_rows,
        "resident rows {peak_rows} > {max_rows}"
    );
    assert!(
        peak_bytes <= max_bytes + one_row,
        "resident bytes {peak_bytes} > {max_bytes}"
    );

    let all = every_row(&w);
    let (mut text_bytes, mut tool_rows, mut text_rows) = (0usize, 0usize, 0usize);
    for row in all.values() {
        match &row.kind {
            RowKind::Agent { text, .. } | RowKind::Thought { text, .. } => {
                text_bytes += text.len();
                text_rows += 1;
            }
            RowKind::Tool(_) => tool_rows += 1,
            _ => {}
        }
    }
    let events_sent = profile.text_chunks + profile.flood_tools + 2 * 3 + 1;
    let queue_limit = DriverLimits::default().max_queued_events;
    if profile.full {
        assert!(s.older_rows, "older rows must have been paged out");
        assert!(
            all.len() > max_rows,
            "the transcript must exceed the resident window"
        );
        assert!(
            text_bytes > max_bytes,
            "the streamed text must exceed the byte window: {text_bytes} bytes in {text_rows} text rows of {} persisted",
            all.len()
        );
        assert!(events_sent > queue_limit);
    }
    assert!(all.len() > s.rows.len(), "older rows are paged from disk");
    assert!(tool_rows >= profile.flood_tools, "{tool_rows}");

    w.workflow.close();
    let (settle, view) = w.settle();
    assert_zero(&view, "after the flooded task closed");
    json!({
        "group": "resource_bounds",
        "profile": if profile.full { "full" } else { "smoke" },
        "resident_rows": {"observed_peak": peak_rows, "limit": max_rows, "within": peak_rows <= max_rows},
        "resident_bytes": {
            "observed_peak": peak_bytes,
            "limit": max_bytes,
            "slack_one_row": one_row,
            "within_limit": peak_bytes <= max_bytes,
            "within_limit_plus_newest_row": peak_bytes <= max_bytes + one_row,
        },
        "transcript": {
            "rows_persisted_total": all.len(),
            "text_rows": text_rows,
            "tool_card_rows": tool_rows,
            "text_bytes_total": text_bytes,
            "rows_resident_final": s.rows.len(),
            "older_rows_paged_from_disk": s.older_rows,
            "page_size_limit": MAX_HISTORY_PAGE,
        },
        "event_queue": {
            "driver_queue_limit": queue_limit,
            "events_scripted": events_sent,
            "queue_overflow_failure": false,
            "task_phase": format!("{:?}", task.phase),
            "note": "the driver exposes no queue-depth gauge; the bound is shown by the absence of an overflow failure while the scripted event count exceeds the limit",
        },
        "brief_queue_peak": peak_queue,
        "snapshots_published": snapshots,
        "stream_seconds": streamed_for.as_secs_f64(),
        "cleanup": {"settle_ms": millis(settle), "ownership": view},
        "not_measured": ["playback/scrub concurrency (needs the native shell and a device)", "GPU/layout cost of rendering rows"],
    })
}

// ---- 2. shared build service: one compile per equal key ------------------------------------------------------------------------------

fn compiler_sharing() -> Value {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let project = create_project(&root);
    let sdk = fake_sdk(temp.path());
    let builds = temp.path().join("builds");
    let compiler = FakeCompiler::blocked(false);
    let service = BuildService::new(
        ProcessTreeManager::new(),
        compiler.clone(),
        BuildLimits::default(),
    );
    let key = build_key(&project, &sdk, &builds);
    let kinds = [
        SubscriberKind::Ui,
        SubscriberKind::Tool,
        SubscriberKind::Validation,
        SubscriberKind::Tool,
    ];
    let subscriptions: Vec<_> = kinds
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            service
                .subscribe(
                    key.clone(),
                    request(&project, &sdk, &builds),
                    Subscriber::new(*kind, format!("subscriber-{i}")),
                )
                .unwrap()
        })
        .collect();
    compiler.wait_started(1);
    compiler.release.store(true, Ordering::SeqCst);
    let leases: Vec<_> = subscriptions
        .into_iter()
        .map(|s| std::thread::spawn(move || s.wait(&|| false).unwrap()))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect();
    let equal_key_started = compiler.started.load(Ordering::SeqCst);
    assert_eq!(
        equal_key_started, 1,
        "four equal-key subscribers, one compile"
    );
    assert!(
        leases
            .windows(2)
            .all(|w| Arc::ptr_eq(w[0].build(), w[1].build()))
    );
    let hit = service
        .subscribe(
            key.clone(),
            request(&project, &sdk, &builds),
            Subscriber::new(SubscriberKind::Tool, "later"),
        )
        .unwrap();
    assert!(hit.is_ready());
    drop(hit);
    let after_equal = service.stats();
    assert_eq!(after_equal.compiles_started, 1);

    // A different revision is a different key: exactly one more compile.
    let other = with_revision(&root, "another revision");
    let other_key = build_key(&other, &sdk, &builds);
    assert_ne!(key.digest(), other_key.digest());
    let lease = service
        .subscribe(
            other_key.clone(),
            request(&other, &sdk, &builds),
            Subscriber::new(SubscriberKind::Validation, "other"),
        )
        .unwrap()
        .wait(&|| false)
        .unwrap();
    let after_other = service.stats();
    assert_eq!(after_other.compiles_started, 2);
    let leased_before_release = after_other.leased_entries;
    drop((leases, lease));
    let released = service.stats();
    assert_eq!(released.leased_entries, 0, "leases released");
    service.close_project(&key.project_id);
    let closed = service.stats();
    assert_eq!((closed.cached_entries, closed.in_flight), (0, 0));
    json!({
        "group": "compiler_count",
        "equal_key": {
            "subscribers": kinds.len() + 1,
            "subscriber_kinds": ["ui", "tool", "validation", "tool", "tool (later cache hit)"],
            "compiles_started": after_equal.compiles_started,
            "joins": after_equal.joins,
            "cache_hits": after_equal.cache_hits,
            "one_compile_per_equal_key": after_equal.compiles_started == 1,
        },
        "distinct_key": {
            "compiles_started_total": after_other.compiles_started,
            "key_digests_differ": key.digest() != other_key.digest(),
        },
        "leases": {"leased_before_release": leased_before_release, "leased_after_release": released.leased_entries},
        "after_project_close": {"cached_entries": closed.cached_entries, "in_flight": closed.in_flight, "leased_entries": closed.leased_entries},
    })
}

// ---- 3. >= 20 edit cycles and >= 20 failed/cancelled task cycles with per-cycle cleanup -----------------------------------------------

#[derive(Clone, Copy)]
enum Failure {
    RepairExhausted,
    ProviderCrash,
    StopWhileEditing,
    StopWhileWaitingPermission,
}

impl Failure {
    const ALL: [Failure; 4] = [
        Self::RepairExhausted,
        Self::ProviderCrash,
        Self::StopWhileEditing,
        Self::StopWhileWaitingPermission,
    ];
    fn name(self) -> &'static str {
        match self {
            Self::RepairExhausted => "repair_exhausted",
            Self::ProviderCrash => "provider_crash",
            Self::StopWhileEditing => "stop_while_editing",
            Self::StopWhileWaitingPermission => "stop_while_waiting_permission",
        }
    }
    fn expected(self) -> TaskPhase {
        match self {
            Self::RepairExhausted | Self::ProviderCrash => TaskPhase::Failed,
            _ => TaskPhase::Cancelled,
        }
    }
}

fn cycles(profile: &Profile) -> Value {
    let w = World::new(None);
    let mut plan: Vec<Option<Failure>> = Vec::new();
    let mut failures = Vec::new();
    for _ in 0..profile.failure_cycles_per_kind {
        failures.extend(Failure::ALL);
    }
    // Alternate edits and failures so every failure follows an accepted edit and every
    // edit follows a failure that left a retained draft.
    let mut failures = failures.into_iter();
    for _ in 0..profile.edit_cycles {
        plan.push(None);
        if let Some(f) = failures.next() {
            plan.push(Some(f));
        }
    }
    plan.extend(failures.map(Some));

    let mut records = Vec::new();
    let mut last: Option<AgentTaskId> = None;
    let (mut accepted, mut failed, mut cancelled) = (0usize, 0usize, 0usize);
    for (cycle, kind) in plan.iter().enumerate() {
        let started = Instant::now();
        let compiles_before = w.service.stats().compiles_started;
        let (name, expected) = match kind {
            None => {
                w.submit(&good(&format!("cycle {cycle}")));
                ("accepted_edit", TaskPhase::Accepted)
            }
            Some(kind) => {
                match kind {
                    Failure::RepairExhausted => w.submit_plan(&[broken(), broken()]),
                    Failure::ProviderCrash => {
                        w.submit(&json!({"write": {"c.txt": "x"}, "crash": 3}))
                    }
                    Failure::StopWhileEditing => {
                        let before = w.prompts().len();
                        w.submit(&json!({"hang": true}));
                        w.wait_prompts(before + 1);
                        w.wait_phase(TaskPhase::Editing);
                        w.workflow.stop().unwrap();
                    }
                    Failure::StopWhileWaitingPermission => {
                        w.submit(
                            &json!({"permission": {"title": "Edit"}, "write": {"y.txt": "y"}}),
                        );
                        w.wait_phase(TaskPhase::WaitingPermission);
                        w.workflow.stop().unwrap();
                    }
                }
                (kind.name(), kind.expected())
            }
        };
        let task = w.wait_task(last.as_ref());
        assert_eq!(
            task.phase, expected,
            "cycle {cycle} ({name}): {:?}",
            task.error
        );
        match task.phase {
            TaskPhase::Accepted => accepted += 1,
            TaskPhase::Failed => failed += 1,
            _ => cancelled += 1,
        }
        let repair_used = task.repair.used;
        let (settle, view) = w.settle();
        assert_zero(&view, &format!("cycle {cycle} ({name})"));
        assert!(view["resident_rows"].as_u64().unwrap() <= MAX_RESIDENT_ROWS as u64);
        let stats = w.service.stats();
        records.push(json!({
            "cycle": cycle,
            "kind": name,
            "expected_phase": format!("{expected:?}"),
            "phase": format!("{:?}", task.phase),
            "repair_attempts_used": repair_used,
            "compiles_started_in_cycle": stats.compiles_started - compiles_before,
            "compiles_started_total": stats.compiles_started,
            "cache_hits_total": stats.cache_hits,
            "settle_ms": millis(settle),
            "duration_ms": millis(started.elapsed()),
            "ownership_after_cycle": view,
        }));
        last = Some(task.id);
    }
    let history = w.snap().history.len();
    assert_eq!(
        history, accepted,
        "every accepted edit is one history entry"
    );
    assert!(accepted >= profile.edit_cycles);
    assert!(failed + cancelled >= profile.failure_cycles_per_kind * Failure::ALL.len());
    let compiles = w.service.stats().compiles_started;
    // One compile per distinct candidate key: an accepted edit compiles once, a repair
    // exhausted task compiles its candidate and its repaired candidate.
    for record in &records {
        if record["kind"] == "accepted_edit" {
            assert_eq!(record["compiles_started_in_cycle"], 1, "{record}");
        }
    }

    w.workflow.close();
    let (settle, view) = w.settle();
    assert_zero(&view, "after close");
    let adapter_pids = w.adapter_pids();
    let survivors: Vec<i64> = adapter_pids
        .iter()
        .copied()
        .filter(|p| pid_alive(*p))
        .collect();
    assert!(
        survivors.is_empty(),
        "adapter processes survived close: {survivors:?}"
    );
    let closed = w.service.stats();
    assert_eq!(closed.cached_entries, 0, "materializations survived close");
    let runtime_entries = fs::read_dir(w.runtime.path().join("rt"))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(
        runtime_entries, 0,
        "broker socket or capability files survived close"
    );
    let session_dirs = fs::read_dir(w.paths.builds())
        .map(|d| d.count())
        .unwrap_or(0);
    json!({
        "group": "cycles",
        "profile": if profile.full { "full" } else { "smoke" },
        "requested": {"accepted_edits": profile.edit_cycles, "failed_or_cancelled": profile.failure_cycles_per_kind * Failure::ALL.len()},
        "totals": {"cycles": records.len(), "accepted": accepted, "failed": failed, "cancelled": cancelled, "history_entries": history},
        "compiles_started_total": compiles,
        "adapter_processes_started": adapter_pids.len(),
        "cycles": records,
        "after_close": {
            "settle_ms": millis(settle),
            "ownership": view,
            "adapter_pids_alive": survivors,
            "build_cached_entries": closed.cached_entries,
            "build_leased_entries": closed.leased_entries,
            "runtime_dir_entries": runtime_entries,
            "build_directories_on_disk": session_dirs,
            "build_directories_note": "on-disk materialization directories are cache, not leases; the cache entries and leases above are the owned state",
        },
    })
}

// ---- 4. CLI / MCP parity against one broker --------------------------------------------------------------------------------------------

fn run_cli(capability: &Path, tool: &str, params: Option<&Value>) -> Result<Value, String> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_studio-tools"));
    command.arg("--capability").arg(capability).arg(tool);
    if let Some(params) = params {
        command.arg("--json").arg(params.to_string());
    }
    let output = command.output().unwrap();
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout).unwrap())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// `initialize`, `tools/list` and one `tools/call` per entry, over one `studio-mcp`.
fn run_mcp(capability: &Path, calls: &[(&str, Option<Value>)]) -> (Value, Vec<Value>) {
    let mut lines = vec![
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"m3-evidence","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    ];
    for (i, (tool, params)) in calls.iter().enumerate() {
        lines.push(json!({"jsonrpc":"2.0","id":10 + i,"method":"tools/call","params":{"name":tool,"arguments":params.clone().unwrap_or_else(|| json!({}))}}));
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_studio-mcp"))
        .arg("--capability")
        .arg(capability)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    let mut stdin = child.stdin.take().unwrap();
    let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
    stdin.write_all(input.as_bytes()).unwrap();
    drop(stdin);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(output) => output.unwrap(),
        Err(_) => {
            unsafe { libc::kill(pid, libc::SIGKILL) };
            panic!("studio-mcp did not exit after its input ended");
        }
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replies: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let by_id = |id: u64| {
        replies
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("no reply {id}: {replies:?}"))
            .clone()
    };
    let list = by_id(2);
    let results = (0..calls.len()).map(|i| by_id(10 + i as u64)).collect();
    (list, results)
}

/// Keys whose value is per-call identity or lifetime, not project data.
const VOLATILE: &[&str] = &[
    "id",
    "artifact",
    "artifact_id",
    "path",
    "expires_at",
    // Per-artifact lifetime stamped when the artifact is minted (a second boundary can
    // fall between the CLI call and the MCP call).
    "expires_at_unix",
    "expiry",
    "expires_in_seconds",
    "snapshot_id",
];

fn normalized(value: &Value, dropped: &mut std::collections::BTreeSet<String>) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, inner) in map {
                if VOLATILE.contains(&key.as_str()) {
                    dropped.insert(key.clone());
                } else {
                    out.insert(key.clone(), normalized(inner, dropped));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| normalized(v, dropped)).collect()),
        other => other.clone(),
    }
}

fn cli_mcp_parity(_profile: &Profile) -> Value {
    let w = World::new(None);
    w.submit(&json!({"write": {FAKE_CONFIG: GOOD_CONFIG}, "hang": true}));
    w.wait_phase(TaskPhase::Editing);
    w.wait_prompts(1);
    // The draft already holds the good timeline config the fake worker serves.
    let deadline = Instant::now() + WAIT;
    let draft = w.snap().task.as_ref().unwrap().draft.clone();
    while fs::read_to_string(draft.join(FAKE_CONFIG)).ok().as_deref() != Some(GOOD_CONFIG) {
        assert!(Instant::now() < deadline, "the scripted write never landed");
        std::thread::sleep(Duration::from_millis(20));
    }
    let s = w.snap();
    assert!(s.mcp.active && s.mcp.cli_active, "{:?}", s.mcp.note);
    let capability = s.mcp.capability_file.clone().unwrap();
    let inspect = json!({"start": 0, "end": 8, "count": 8});
    let frame = json!({"frame": 0});
    let calls: Vec<(&str, Option<Value>)> = vec![
        ("project_context", None),
        ("timeline", None),
        ("inspect", Some(inspect)),
        ("render_frame", Some(frame)),
    ];
    let cli: Vec<Result<Value, String>> = calls
        .iter()
        .map(|(tool, params)| run_cli(&capability, tool, params.as_ref()))
        .collect();
    let (list, mcp) = run_mcp(&capability, &calls);
    let listed: Vec<String> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(listed.len(), 9, "{listed:?}");

    let mut results = Vec::new();
    let mut dropped = std::collections::BTreeSet::new();
    for (i, (tool, _)) in calls.iter().enumerate() {
        let cli_value = cli[i]
            .clone()
            .unwrap_or_else(|e| panic!("{tool} over the CLI: {e}"));
        assert_eq!(
            mcp[i]["result"]["isError"], false,
            "{tool} over MCP: {}",
            mcp[i]
        );
        let mcp_value = mcp[i]["result"]["structuredContent"].clone();
        let text: Value =
            serde_json::from_str(mcp[i]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            text, mcp_value,
            "{tool}: MCP text and structuredContent differ"
        );
        let (a, b) = (
            normalized(&cli_value, &mut dropped),
            normalized(&mcp_value, &mut dropped),
        );
        assert_eq!(
            a, b,
            "{tool}: CLI and MCP disagree\ncli {cli_value}\nmcp {mcp_value}"
        );
        assert!(
            a.as_object().is_some_and(|o| !o.is_empty()),
            "{tool}: an empty reply proves nothing"
        );
        results.push(json!({
            "tool": tool,
            "identical_raw": cli_value == mcp_value,
            "identical_after_dropping_volatile_fields": true,
            "reply_bytes_cli": serde_json::to_string(&cli_value).unwrap().len(),
            "reply_bytes_mcp": serde_json::to_string(&mcp_value).unwrap().len(),
            "reply_keys": a.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()),
            "reply_sha256_normalized": sha256_hex(&serde_json::to_vec(&a).unwrap()),
        }));
    }
    let frame_hashes = frame_hash(&cli[3]).zip(frame_hash(&Ok(
        mcp[3]["result"]["structuredContent"].clone(),
    )));
    let frame_parity = frame_hashes.as_ref().map(|(a, b)| a == b);
    if let Some((a, b)) = &frame_hashes {
        assert_eq!(a, b, "frame bytes differ between routes");
    }

    // The same broker answers both: it was told apart only by connection.
    w.workflow.stop().unwrap();
    w.wait_task(None);
    w.workflow.close();
    let (settle, view) = w.settle();
    assert_zero(&view, "after the parity task");
    assert_eq!(
        view["build_cached_entries"], 0,
        "materializations survived close"
    );
    let gone = run_cli(&capability, "project_context", None);
    assert!(gone.is_err(), "the capability must die with the task");
    json!({
        "group": "cli_mcp_parity",
        "broker": "one app-owned ToolBroker, one task capability file, two helper routes (studio-tools, studio-mcp)",
        "mcp_protocol": "2025-06-18",
        "tools_listed": listed,
        "compared": results,
        "volatile_fields_dropped": dropped,
        "frame_parity": {"compared": frame_parity.is_some(), "identical": frame_parity, "png_sha256": frame_hashes.as_ref().map(|(a, _)| a.clone())},
        "fixture_note": "fake preview worker and scripted compiler; no pixel comparison against the project CLI (the real-SDK frame-zero parity lives in real_sdk_candidates, run with SDK_ACTIVE)",
        "capability_dead_after_task": gone.is_err(),
        "cleanup": {"settle_ms": millis(settle), "ownership": view},
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The content hash the broker reports for a rendered frame, wherever the reply keeps it.
fn frame_hash(reply: &Result<Value, String>) -> Option<String> {
    fn find(value: &Value) -> Option<String> {
        match value {
            Value::Object(map) => {
                for key in ["sha256", "hash"] {
                    if let Some(Value::String(hash)) = map.get(key) {
                        return Some(hash.clone());
                    }
                }
                map.values().find_map(find)
            }
            Value::Array(items) => items.iter().find_map(find),
            _ => None,
        }
    }
    reply.as_ref().ok().and_then(find)
}

// ---- 5. the Linux publication primitive the Apply gate selected on this filesystem ---------------------------------------------------

/// The mount (point, type, source) that holds `path`, from `/proc/self/mountinfo`.
fn mount_of(path: &Path) -> Option<(String, String, String)> {
    let path = fs::canonicalize(path).ok()?;
    let table = fs::read_to_string("/proc/self/mountinfo").ok()?;
    let mut best: Option<(String, String, String)> = None;
    for line in table.lines() {
        let (left, right) = line.split_once(" - ")?;
        let point = left.split(' ').nth(4)?.to_owned();
        let mut tail = right.split(' ');
        let (kind, source) = (tail.next()?.to_owned(), tail.next()?.to_owned());
        if path.starts_with(&point) && best.as_ref().is_none_or(|(b, _, _)| point.len() >= b.len())
        {
            best = Some((point, kind, source));
        }
    }
    best
}

fn publication_primitive() -> Value {
    use studio_engine::edit_transaction::{ApplyGate, NoClobber};
    let w = World::new(None);
    let (real, forced_link) = {
        let mut controller = w.controller.lock();
        let real = controller.apply_gate().clone();
        controller.force_publication_mechanism(Some(NoClobber::Link));
        let forced = controller.apply_gate().clone();
        controller.force_publication_mechanism(None);
        (real, forced)
    };
    assert!(
        matches!(forced_link, ApplyGate::Blocked(_)),
        "a link-only filesystem must block Apply: {forced_link:?}"
    );
    let mount = mount_of(&w.root);
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease")
        .unwrap_or_default()
        .trim()
        .to_owned();
    let (selected, blocked_reason) = match &real {
        ApplyGate::Ready(mechanism) => (Some(format!("{mechanism:?}")), None),
        ApplyGate::Blocked(reason) => (None, Some(reason.clone())),
    };
    w.workflow.close();
    json!({
        "group": "publication_primitive",
        "kernel": kernel,
        "project_filesystem": mount.map(|(point, kind, source)| json!({"mount_point": point, "type": kind, "source": source})),
        "apply_gate": if real.is_ready() { "ready" } else { "blocked" },
        "selected_mechanism": selected,
        "blocked_reason": blocked_reason,
        "link_only_filesystem_gate": match &forced_link {
            ApplyGate::Blocked(reason) => json!({"blocked": true, "reason": reason}),
            ApplyGate::Ready(_) => json!({"blocked": false}),
        },
        "note": "the probe ran on the temporary project's filesystem under /tmp; the link fallback is refused for live project names",
    })
}

// ---- authentic: a real adapter's initialize + session handshake (never run by default) ------------------------------------------------

/// Probes the operator's REAL adapter: executable resolution, ACP v1 negotiation, auth
/// readiness (a session is created in a scratch directory; no prompt is ever sent) and the
/// advertised capabilities. Its only output is `adapter-probe.json`; it claims no writer
/// containment and runs no edit. The harness (`scripts/qualify-m3-agent.py --adapter`)
/// decides what the result may be recorded as.
///
/// Environment: `FFRAMES_M3_ADAPTER` (executable), `FFRAMES_M3_ADAPTER_ARGS` (JSON array of
/// arguments, optional), `FFRAMES_M3_AUTH_ENV` (comma-separated variable NAMES forwarded to
/// the adapter, optional), `FFRAMES_M3_EVIDENCE_OUT` (directory).
#[test]
#[ignore = "authentic: needs a real ACP adapter (FFRAMES_M3_ADAPTER) and its account"]
fn authentic_adapter_probe() {
    let executable = std::env::var("FFRAMES_M3_ADAPTER").expect("FFRAMES_M3_ADAPTER");
    let args: Vec<String> = std::env::var("FFRAMES_M3_ADAPTER_ARGS")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| serde_json::from_str(&v).expect("FFRAMES_M3_ADAPTER_ARGS is a JSON array"))
        .unwrap_or_default();
    let auth_env_names: Vec<String> = std::env::var("FFRAMES_M3_AUTH_ENV")
        .unwrap_or_default()
        .split(',')
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect();
    let out = PathBuf::from(
        std::env::var_os("FFRAMES_M3_EVIDENCE_OUT").expect("FFRAMES_M3_EVIDENCE_OUT"),
    );
    let w = World::build(
        None,
        Some(AdapterConfig {
            executable: executable.clone(),
            args: args.clone(),
            auth_env_names: auth_env_names.clone(),
        }),
    );
    let started = Instant::now();
    w.workflow.check_adapter().unwrap();
    let snapshot = w.wait("the adapter probe", |s| {
        matches!(s.adapter.readiness, AdapterReadiness::Checked { .. })
    });
    let probe_seconds = started.elapsed().as_secs_f64();
    let AdapterReadiness::Checked { report, .. } = &snapshot.adapter.readiness else {
        unreachable!()
    };
    let digest = report
        .executable
        .as_ref()
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| sha256_hex(&bytes));
    w.workflow.close();
    let (settle, view) = w.settle();
    assert_zero(&view, "after the adapter probe");
    let record = json!({
        "group": "adapter_probe",
        "evidence_kind": "authentic",
        "schema_version": MEASUREMENT_SCHEMA,
        "requested_executable": executable,
        // Arguments can name paths but never credentials: values come from the
        // environment, whose NAMES (not values) are all that is recorded.
        "adapter_arg_count": args.len(),
        "auth_env_names": auth_env_names,
        "report": serde_json::to_value(report).unwrap(),
        "resolved_executable_sha256": digest,
        "probe_seconds": probe_seconds,
        "prompt_sent": false,
        "edit_attempted": false,
        "writer_ownership_claimed": "unknown",
        "cleanup": {"settle_ms": millis(settle), "ownership": view},
    });
    fs::create_dir_all(&out).unwrap();
    fs::write(
        out.join("adapter-probe.json"),
        format!("{}\n", serde_json::to_string_pretty(&record).unwrap()),
    )
    .unwrap();
}

// ---- the test ------------------------------------------------------------------------------------------------------------------------

type Group<'a> = &'a dyn Fn(&Profile) -> Value;

#[test]
fn m3_development_evidence() {
    let profile = Profile::from_env();
    let started = Instant::now();
    let mut summary = Map::new();
    let groups: [(&str, Group); 5] = [
        ("resource-bounds.json", &resource_bounds),
        ("compiler-count.json", &|_| compiler_sharing()),
        ("cycles.json", &cycles),
        ("cli-mcp-parity.json", &cli_mcp_parity),
        ("publication-primitive.json", &|_| publication_primitive()),
    ];
    for (file, run) in groups {
        let group_started = Instant::now();
        let value = run(&profile);
        summary.insert(
            file.into(),
            json!({"seconds": group_started.elapsed().as_secs_f64(), "group": value["group"]}),
        );
        profile.write(file, value);
    }
    profile.write(
        "summary.json",
        json!({
            "group": "summary",
            "profile": if profile.full { "full" } else { "smoke" },
            "test": "m3_development_evidence",
            "groups": summary,
            "total_seconds": started.elapsed().as_secs_f64(),
        }),
    );
}
