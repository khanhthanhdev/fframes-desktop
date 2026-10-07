//! Tool contract, broker security/limits, MCP protocol and CLI/MCP subprocess parity.
//! A deterministic in-test backend and liveness oracle stand in for the app.
#![cfg(unix)]

use fframes_studio::agent_tools::{
    ArtifactRef, BoundRevision, FrameSelection, MAX_IMAGE_ARTIFACT_BYTES, MAX_QUEUED_CALLS,
    MAX_REQUEST_BYTES, MAX_STRIP_FRAMES, MAX_TEXT_REPLY_BYTES, RevisionInfo, RevisionLabel,
    TOOL_NAMES, TOOL_WORKERS, TaskLiveness, ToolBackend, ToolBinding, ToolCall, ToolDispatcher,
    ToolError, ToolErrorCode, ToolReply, ToolRequest,
    backend::{MAX_KNOWN_REVISIONS, RevisionHistory, STATUS_KNOWN_REVISIONS},
    broker::{BrokerConfig, BrokerError, ToolBroker},
    client::{BrokerClient, CAPABILITY_ENV, CallFailure, ClientError},
    mcp::{
        INVALID_PARAMS, INVALID_REQUEST, MAX_MCP_LINE_BYTES, MCP_PROTOCOL_VERSION,
        MCP_QUALIFICATION, METHOD_NOT_FOUND, McpServer, NOT_INITIALIZED, PARSE_ERROR, ToolCaller,
    },
    tool_descriptions,
};
use parking_lot::{Condvar, Mutex};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{PermissionsExt, symlink},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use studio_engine::{AgentTaskId, OpenSession, TaskIdentity};
use studio_project::{ProjectId, SourceRevision};

const REV_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const REV_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const WAIT: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------- fixtures

type Behavior = dyn Fn(&ToolBinding, &ToolRequest, &(dyn Fn() -> bool + Sync)) -> Result<ToolReply, ToolError>
    + Send
    + Sync;

fn revision_info() -> RevisionInfo {
    RevisionInfo {
        id: REV_A.into(),
        label: RevisionLabel::DraftSnapshot,
        validated: false,
    }
}

fn plain_reply(request: &ToolRequest) -> ToolReply {
    ToolReply {
        revision: revision_info(),
        result: json!({"tool": request.call.name()}),
        artifacts: vec![],
    }
}

fn artifact(bytes: u64) -> ArtifactRef {
    ArtifactRef {
        id: "art-0123456789abcdef".into(),
        media_type: "image/png".into(),
        path: "/tmp/fframes-artifacts/art-0123456789abcdef.png".into(),
        bytes,
        sha256: "c".repeat(64),
        width: 640,
        height: 360,
        expires_at_unix: 1_900_000_000,
    }
}

fn standard(
    _binding: &ToolBinding,
    request: &ToolRequest,
    _cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<ToolReply, ToolError> {
    match request.call {
        ToolCall::RenderFrame { frame: 999, .. } => Err(ToolError::new(
            ToolErrorCode::NotFound,
            "frame 999 does not exist",
        )),
        ToolCall::RenderFrame { .. } | ToolCall::RenderStrip { .. } => Ok(ToolReply {
            artifacts: vec![artifact(2048)],
            ..plain_reply(request)
        }),
        _ => Ok(plain_reply(request)),
    }
}

struct Gate {
    state: Mutex<(usize, bool)>,
    changed: Condvar,
}

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((0, false)),
            changed: Condvar::new(),
        })
    }

    /// Announce arrival, then block until opened or cancelled. True when opened.
    fn pass(&self, cancelled: &(dyn Fn() -> bool + Sync)) -> bool {
        let mut state = self.state.lock();
        state.0 += 1;
        self.changed.notify_all();
        while !state.1 {
            if cancelled() {
                return false;
            }
            self.changed.wait_for(&mut state, Duration::from_millis(10));
        }
        true
    }

    fn wait_entered(&self, count: usize) {
        let deadline = Instant::now() + WAIT;
        let mut state = self.state.lock();
        while state.0 < count {
            assert!(
                !self.changed.wait_until(&mut state, deadline).timed_out() || state.0 >= count,
                "gate was not entered in time"
            );
        }
    }

    fn open(&self) {
        self.state.lock().1 = true;
        self.changed.notify_all();
    }
}

struct FakeBackend {
    calls: Mutex<Vec<String>>,
    running: AtomicUsize,
    max_running: AtomicUsize,
    behavior: Mutex<Arc<Behavior>>,
}

impl FakeBackend {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            running: AtomicUsize::new(0),
            max_running: AtomicUsize::new(0),
            behavior: Mutex::new(Arc::new(standard)),
        })
    }

    fn set(
        &self,
        behavior: impl Fn(
            &ToolBinding,
            &ToolRequest,
            &(dyn Fn() -> bool + Sync),
        ) -> Result<ToolReply, ToolError>
        + Send
        + Sync
        + 'static,
    ) {
        *self.behavior.lock() = Arc::new(behavior);
    }

    fn call_count(&self) -> usize {
        self.calls.lock().len()
    }
}

struct RunningGuard<'a>(&'a AtomicUsize);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ToolBackend for FakeBackend {
    fn execute(
        &self,
        binding: &ToolBinding,
        request: &ToolRequest,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<ToolReply, ToolError> {
        self.calls.lock().push(request.call.name().into());
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_running.fetch_max(now, Ordering::SeqCst);
        let _guard = RunningGuard(&self.running);
        let behavior = self.behavior.lock().clone();
        behavior(binding, request, cancelled)
    }
}

#[derive(Default)]
struct FakeLiveness {
    dead: Mutex<HashSet<AgentTaskId>>,
}

impl FakeLiveness {
    fn kill(&self, task: &TaskIdentity) {
        self.dead.lock().insert(task.task.clone());
    }
}

impl TaskLiveness for FakeLiveness {
    fn is_live(&self, task: &TaskIdentity) -> bool {
        !self.dead.lock().contains(&task.task)
    }
}

fn task() -> TaskIdentity {
    TaskIdentity {
        task: AgentTaskId::new(),
        project: ProjectId::try_from("proj-1".to_string()).unwrap(),
        session: OpenSession::new(),
        generation: 1,
    }
}

fn draft(task: &TaskIdentity) -> ToolBinding {
    ToolBinding {
        task: task.clone(),
        revision: BoundRevision::Draft,
    }
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    runtime: PathBuf,
    backend: Arc<FakeBackend>,
    liveness: Arc<FakeLiveness>,
    broker: ToolBroker,
}

fn short_tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("fft")
        .tempdir_in("/tmp")
        .unwrap()
}

fn start_in(
    runtime: &Path,
    backend: &Arc<FakeBackend>,
    liveness: &Arc<FakeLiveness>,
) -> Result<ToolBroker, BrokerError> {
    ToolBroker::start(BrokerConfig {
        runtime_dir: runtime.to_path_buf(),
        dispatcher: ToolDispatcher::new(backend.clone()),
        liveness: liveness.clone(),
    })
}

fn harness() -> Harness {
    let dir = short_tempdir();
    let runtime = dir.path().join("rt");
    let backend = FakeBackend::new();
    let liveness = Arc::new(FakeLiveness::default());
    let broker = start_in(&runtime, &backend, &liveness).unwrap();
    Harness {
        _dir: dir,
        runtime,
        backend,
        liveness,
        broker,
    }
}

impl Harness {
    fn grant(&self, task: &TaskIdentity) -> Cap {
        let grant = self
            .broker
            .grant(draft(task), Duration::from_secs(600))
            .unwrap();
        Cap::read(&grant.capability_file)
    }
}

struct Cap {
    file: PathBuf,
    socket: PathBuf,
    capability: String,
    secret: String,
}

impl Cap {
    fn read(file: &Path) -> Self {
        let value: Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
        Self {
            file: file.to_path_buf(),
            socket: PathBuf::from(value["socket"].as_str().unwrap()),
            capability: value["capability"].as_str().unwrap().into(),
            secret: value["secret"].as_str().unwrap().into(),
        }
    }

    fn wire(&self) -> Wire {
        let mut wire = Wire::connect(&self.socket);
        let reply = wire.hello(&self.capability, &self.secret);
        assert_eq!(reply, json!({"ok": true}), "hello must succeed");
        wire
    }
}

struct Wire {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Wire {
    fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        Self {
            reader: BufReader::new(stream.try_clone().unwrap()),
            stream,
        }
    }

    fn send_raw(&mut self, text: &str) {
        self.stream.write_all(text.as_bytes()).unwrap();
    }

    fn send(&mut self, value: &Value) {
        self.send_raw(&format!("{value}\n"));
    }

    /// `None` on end of stream or a reset connection.
    fn recv(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(
                serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("non-JSON broker line {line:?}: {error}")),
            ),
        }
    }

    fn hello(&mut self, capability: &str, secret: &str) -> Value {
        self.send(&json!({"hello": {"capability": capability, "secret": secret}}));
        self.recv().expect("hello answer")
    }

    fn request(&mut self, id: u64, method: &str, params: Value) {
        self.send(&json!({"id": id, "method": method, "params": params}));
    }

    fn call(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.request(id, method, params);
        self.recv().expect("reply")
    }
}

fn error_code(reply: &Value) -> &str {
    reply["error"]["code"].as_str().unwrap_or("<no error>")
}

// ---------------------------------------------------------------- contract

#[test]
fn all_unique_tools_are_listed_by_mcp_and_descriptions() {
    let names: HashSet<&str> = TOOL_NAMES.iter().copied().collect();
    assert_eq!(TOOL_NAMES.len(), 9);
    assert_eq!(names.len(), 9, "tool names must be unique");
    let described: Vec<String> = tool_descriptions()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(described, TOOL_NAMES);
    for tool in tool_descriptions() {
        let schema = tool["inputSchema"].as_object().unwrap();
        assert!(
            ["oneOf", "anyOf", "allOf"]
                .iter()
                .all(|operator| !schema.contains_key(*operator)),
            "{} must not expose unsupported top-level schema combinators",
            tool["name"]
        );
    }

    let mut server = McpServer::new(|| Err(ToolError::new(ToolErrorCode::Unavailable, "none")));
    initialize(&mut server);
    let list = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    );
    let listed: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(listed, TOOL_NAMES);
    assert_eq!(list["result"]["tools"], Value::Array(tool_descriptions()));
}

#[test]
fn request_parsing_rejects_bad_shapes_and_accepts_documented_ones() {
    let code = |method: &str, params: Value| ToolRequest::parse(method, &params).unwrap_err().code;
    assert_eq!(code("shell", json!({})), ToolErrorCode::MethodNotFound);
    assert_eq!(code("export", json!({})), ToolErrorCode::MethodNotFound);
    assert_eq!(
        code("timeline", json!({"bogus": 1})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 1, "x": 1})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(code("timeline", json!([1])), ToolErrorCode::InvalidParams);
    assert_eq!(
        code("timeline", json!("text")),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 2_000_000_000u64})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": -1})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 1, "scale": 1.5})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 1, "scale": 0})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 1, "scale": -0.1})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_frame", json!({"frame": 1, "scale": "0.5"})),
        ToolErrorCode::InvalidParams
    );
    // NaN is not representable in JSON: it is a parse error on the wire, never a value.
    assert!(serde_json::from_str::<Value>(r#"{"frame":1,"scale":NaN}"#).is_err());
    let strip =
        |start: u64, end: u64, count: u64| json!({"start": start, "end": end, "count": count});
    assert_eq!(
        code("render_strip", strip(10, 5, 3)),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_strip", strip(0, 10, 0)),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_strip", strip(0, 10, MAX_STRIP_FRAMES as u64 + 1)),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("render_strip", strip(0, 2_000_000_000, 3)),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(code("inspect", json!({})), ToolErrorCode::InvalidParams);
    assert_eq!(
        code("inspect", json!({"frames": []})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code(
            "inspect",
            json!({"frames": [1], "start": 0, "end": 1, "count": 2})
        ),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("inspect", json!({"start": 4, "end": 1, "count": 2})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code("inspect", json!({"start": 0, "end": 1})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code(
            "inspect",
            json!({"frames": (0..1000).collect::<Vec<u64>>()})
        ),
        ToolErrorCode::InvalidParams
    );

    let ok = |method: &str, params: Value| ToolRequest::parse(method, &params).unwrap();
    assert_eq!(
        ok("project_context", Value::Null).call,
        ToolCall::ProjectContext
    );
    assert_eq!(ok("timeline", json!({})).call, ToolCall::Timeline);
    let asserted = ok(
        "build_status",
        json!({"project": "proj-1", "revision": REV_A}),
    );
    assert_eq!(asserted.call, ToolCall::BuildStatus);
    assert_eq!(asserted.assertions.project.as_deref(), Some("proj-1"));
    assert_eq!(asserted.assertions.revision.as_deref(), Some(REV_A));
    assert_eq!(
        ok("render_frame", json!({"frame": 7})).call,
        ToolCall::RenderFrame {
            frame: 7,
            scale: 0.5
        }
    );
    assert_eq!(
        ok("render_frame", json!({"frame": 7, "scale": 1.0})).call,
        ToolCall::RenderFrame {
            frame: 7,
            scale: 1.0
        }
    );
    assert_eq!(
        ok("render_strip", strip(0, 90, MAX_STRIP_FRAMES as u64)).call,
        ToolCall::RenderStrip {
            start: 0,
            end: 90,
            count: MAX_STRIP_FRAMES,
            scale: 0.5
        }
    );
    assert_eq!(
        ok("inspect", json!({"frames": [3, 1]})).call,
        ToolCall::Inspect(FrameSelection::List(vec![3, 1]))
    );
    assert_eq!(
        ok("inspect", json!({"start": 0, "end": 9, "count": 4})).call,
        ToolCall::Inspect(FrameSelection::Range {
            start: 0,
            end: 9,
            count: 4
        })
    );
    assert_eq!(
        ok(
            "selection_context",
            json!({"frame": 12, "seek_serial": 4, "frame_geometry_digest": REV_A})
        )
        .call,
        ToolCall::SelectionContext {
            frame: 12,
            seek_serial: 4,
            identity: None,
            frame_geometry_digest: Some(REV_A.into()),
        }
    );
    assert_eq!(
        ok("selection_context", json!({"frame": 12, "seek_serial": 4, "scene_instance_key": "scene-a", "component_key": "title", "object_key": "headline", "repeat_key": "primary"})).call,
        ToolCall::SelectionContext {
            frame: 12,
            seek_serial: 4,
            identity: Some(fframes_studio_protocol::EditorObjectIdentity {
                scene_instance_key: "scene-a".into(),
                component_key: "title".into(),
                object_key: "headline".into(),
                repeat_key: "primary".into(),
            }),
            frame_geometry_digest: None,
        }
    );
    assert_eq!(
        code(
            "selection_context",
            json!({"frame": 1, "seek_serial": 0, "object_key": "title"})
        ),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        ok(
            "source_lookup",
            json!({"path": "src/main.rs", "symbol": "main", "marker": "TITLE"})
        )
        .call,
        ToolCall::SourceLookup {
            path: Some(studio_project::ProjectPath::try_from("src/main.rs".to_string()).unwrap()),
            symbol: Some("main".into()),
            marker: Some("TITLE".into()),
            frame: None,
            seek_serial: None,
            identity: None,
            frame_geometry_digest: None,
        }
    );
    assert_eq!(
        ok("source_lookup", json!({"frame": 12, "seek_serial": 44, "scene_instance_key": "scene-a", "component_key": "title", "object_key": "headline", "repeat_key": "primary", "frame_geometry_digest": REV_A})).call,
        ToolCall::SourceLookup {
            path: None,
            symbol: None,
            marker: None,
            frame: Some(12),
            seek_serial: Some(44),
            identity: Some(fframes_studio_protocol::EditorObjectIdentity {
                scene_instance_key: "scene-a".into(),
                component_key: "title".into(),
                object_key: "headline".into(),
                repeat_key: "primary".into(),
            }),
            frame_geometry_digest: Some(REV_A.into()),
        }
    );
    assert_eq!(
        code("source_lookup", json!({"path": "src/main.rs"})),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code(
            "source_lookup",
            json!({"path": "src/main.rs", "symbol": "main", "frame": 12, "seek_serial": 4, "scene_instance_key": "scene-a", "component_key": "title", "object_key": "headline", "repeat_key": "primary"})
        ),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        code(
            "source_lookup",
            json!({"path": "../secret.rs", "symbol": "main"})
        ),
        ToolErrorCode::InvalidParams
    );
    assert_eq!(
        ok("style_context", json!({"token": "color.accent"})).call,
        ToolCall::StyleContext {
            token: Some("color.accent".into()),
            frame: None,
            seek_serial: None,
            identity: None,
            frame_geometry_digest: None,
        }
    );
    assert_eq!(
        ok("style_context", json!({"frame": 12, "seek_serial": 44, "scene_instance_key": "scene-a", "component_key": "title", "object_key": "headline", "repeat_key": "primary", "frame_geometry_digest": REV_A})).call,
        ToolCall::StyleContext {
            token: None,
            frame: Some(12),
            seek_serial: Some(44),
            identity: Some(fframes_studio_protocol::EditorObjectIdentity {
                scene_instance_key: "scene-a".into(),
                component_key: "title".into(),
                object_key: "headline".into(),
                repeat_key: "primary".into(),
            }),
            frame_geometry_digest: Some(REV_A.into()),
        }
    );
    assert_eq!(
        code("style_context", json!({"frame": 12, "seek_serial": 44})),
        ToolErrorCode::InvalidParams
    );
}

#[test]
fn frame_selection_resolve_is_bounded_sorted_and_deduplicated() {
    let list = FrameSelection::List(vec![5, 1, 5, 3]);
    assert_eq!(list.resolve(10).unwrap(), vec![1, 3, 5]);
    assert_eq!(
        list.resolve(5).unwrap_err().code,
        ToolErrorCode::InvalidParams
    );
    let range = FrameSelection::Range {
        start: 0,
        end: 9,
        count: 4,
    };
    assert_eq!(range.resolve(10).unwrap(), vec![0, 3, 6, 9]);
    assert_eq!(
        range.resolve(9).unwrap_err().code,
        ToolErrorCode::InvalidParams
    );
    let single = FrameSelection::Range {
        start: 4,
        end: 8,
        count: 1,
    };
    assert_eq!(single.resolve(5).unwrap(), vec![4]);
    let collapsed = FrameSelection::Range {
        start: 2,
        end: 3,
        count: 6,
    };
    assert_eq!(collapsed.resolve(4).unwrap(), vec![2, 3]);
}

fn never() -> bool {
    false
}

#[test]
fn dispatcher_refuses_cross_project_and_stale_revision() {
    let backend = FakeBackend::new();
    let dispatcher = ToolDispatcher::new(backend.clone());
    let task = task();
    let fixed = ToolBinding {
        task: task.clone(),
        revision: BoundRevision::Fixed(SourceRevision::try_from(REV_A.to_string()).unwrap()),
    };
    let err = dispatcher
        .dispatch(
            &fixed,
            "timeline",
            &json!({"project": "other-project"}),
            &never,
        )
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::CrossProject);
    let err = dispatcher
        .dispatch(&fixed, "timeline", &json!({"revision": REV_B}), &never)
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::StaleRevision);
    assert_eq!(
        backend.call_count(),
        0,
        "refused calls never reach the backend"
    );

    // Matching assertions pass through to the backend.
    let ok = dispatcher
        .dispatch(
            &fixed,
            "timeline",
            &json!({"project": "proj-1", "revision": REV_A}),
            &never,
        )
        .unwrap();
    assert_eq!(ok["revision"]["id"], REV_A);
    assert_eq!(backend.call_count(), 1);

    // A draft binding cannot be asserted against a revision the reply is not about.
    let err = dispatcher
        .dispatch(
            &draft(&task),
            "timeline",
            &json!({"revision": REV_B}),
            &never,
        )
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::StaleRevision);

    // A call cancelled before it starts never runs.
    let before = backend.call_count();
    let err = dispatcher
        .dispatch(&draft(&task), "timeline", &json!({}), &|| true)
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::Cancelled);
    assert_eq!(backend.call_count(), before);
}

#[test]
fn dispatcher_bounds_text_replies_and_artifacts() {
    let backend = FakeBackend::new();
    let dispatcher = ToolDispatcher::new(backend.clone());
    let binding = draft(&task());

    backend.set(|_, request, _| {
        Ok(ToolReply {
            result: json!({"blob": "x".repeat(MAX_TEXT_REPLY_BYTES + 1)}),
            ..plain_reply(request)
        })
    });
    let err = dispatcher
        .dispatch(&binding, "timeline", &json!({}), &never)
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::TooLarge);

    backend.set(|_, request, _| {
        Ok(ToolReply {
            result: json!({"blob": "x".repeat(MAX_TEXT_REPLY_BYTES / 2)}),
            ..plain_reply(request)
        })
    });
    let value = dispatcher
        .dispatch(&binding, "timeline", &json!({}), &never)
        .unwrap();
    assert!(serde_json::to_vec(&value).unwrap().len() <= MAX_TEXT_REPLY_BYTES);

    backend.set(|_, request, _| {
        Ok(ToolReply {
            artifacts: vec![artifact(MAX_IMAGE_ARTIFACT_BYTES as u64 + 1)],
            ..plain_reply(request)
        })
    });
    let err = dispatcher
        .dispatch(&binding, "render_frame", &json!({"frame": 1}), &never)
        .unwrap_err();
    assert_eq!(err.code, ToolErrorCode::TooLarge);

    backend.set(|_, request, _| {
        Ok(ToolReply {
            artifacts: vec![artifact(MAX_IMAGE_ARTIFACT_BYTES as u64)],
            ..plain_reply(request)
        })
    });
    let value = dispatcher
        .dispatch(&binding, "render_frame", &json!({"frame": 1}), &never)
        .unwrap();
    assert_eq!(
        value["artifacts"][0]["bytes"],
        MAX_IMAGE_ARTIFACT_BYTES as u64
    );
}

fn fake_identity(n: usize) -> String {
    format!("{n:064x}")
}

#[test]
fn known_revision_history_is_bounded_lru_and_status_summary_stays_small() {
    let mut history = RevisionHistory::default();
    assert!(history.is_empty());
    // Far past the size the old unbounded list would have reached: 5000 x 64-hex ids
    // serialise to well over the 256 KiB reply limit.
    let captured = 5000;
    let unbounded = serde_json::to_vec(&(0..captured).map(fake_identity).collect::<Vec<_>>())
        .unwrap()
        .len();
    assert!(unbounded > MAX_TEXT_REPLY_BYTES, "{unbounded}");
    for n in 0..captured {
        history.remember(&fake_identity(n));
        assert!(history.len() <= MAX_KNOWN_REVISIONS);
    }
    assert_eq!(history.len(), MAX_KNOWN_REVISIONS);
    assert_eq!(history.evicted(), (captured - MAX_KNOWN_REVISIONS) as u64);
    // Oldest are evicted first: the newest MAX_KNOWN_REVISIONS stay, everything older is gone.
    assert!(!history.contains(&fake_identity(0)));
    assert!(!history.contains(&fake_identity(captured - MAX_KNOWN_REVISIONS - 1)));
    assert!(history.contains(&fake_identity(captured - MAX_KNOWN_REVISIONS)));
    assert!(history.contains(&fake_identity(captured - 1)));

    let summary = history.summary(STATUS_KNOWN_REVISIONS);
    assert_eq!(summary.recent.len(), STATUS_KNOWN_REVISIONS);
    assert_eq!(summary.total, captured as u64);
    assert_eq!(
        summary.recent.last().unwrap(),
        &fake_identity(captured - 1),
        "newest last"
    );
    let listed = serde_json::to_vec(&summary.recent).unwrap().len();
    assert!(listed < 2 * 1024, "{listed}");
    const {
        assert!(
            (MAX_KNOWN_REVISIONS * 70) < MAX_TEXT_REPLY_BYTES / 32,
            "even the whole retained set is a small fraction of the reply limit"
        )
    };

    // Using a retained identity refreshes it, so it outlives newer-but-unused ones.
    let oldest_kept = fake_identity(captured - MAX_KNOWN_REVISIONS);
    history.remember(&oldest_kept);
    for n in captured..captured + MAX_KNOWN_REVISIONS - 1 {
        history.remember(&fake_identity(n));
    }
    assert!(history.contains(&oldest_kept));
    assert!(!history.contains(&fake_identity(captured - MAX_KNOWN_REVISIONS + 1)));
    history.remember(&fake_identity(captured + MAX_KNOWN_REVISIONS));
    assert!(!history.contains(&oldest_kept), "now it is the oldest");

    // A short history reports everything and no truncation.
    let mut small = RevisionHistory::default();
    small.remember(REV_A);
    small.remember(REV_B);
    small.remember(REV_A);
    let summary = small.summary(STATUS_KNOWN_REVISIONS);
    assert_eq!(summary.recent, vec![REV_B.to_owned(), REV_A.to_owned()]);
    assert_eq!(summary.total, 2);
}

// ------------------------------------------------------------------ broker

#[test]
fn wrong_secret_and_unknown_capability_are_indistinguishable() {
    let h = harness();
    let cap = h.grant(&task());
    let zeros = "0".repeat(64);

    let mut wrong_secret = Wire::connect(&cap.socket);
    let first = wrong_secret.hello(&cap.capability, &zeros);
    assert!(
        wrong_secret.recv().is_none(),
        "connection closes after a failed hello"
    );

    let mut unknown = Wire::connect(&cap.socket);
    let second = unknown.hello("deadbeefdeadbeefdeadbeefdeadbeef", &cap.secret);
    assert!(unknown.recv().is_none());

    let mut garbled = Wire::connect(&cap.socket);
    garbled.send_raw("not json at all\n");
    let third = garbled.recv().unwrap();

    let mut short_secret = Wire::connect(&cap.socket);
    let fourth = short_secret.hello(&cap.capability, "abcd");

    for reply in [&first, &second, &third, &fourth] {
        assert_eq!(error_code(reply), "unauthorized");
        assert!(!reply.to_string().contains(&cap.secret));
    }
    assert_eq!(first, second, "no oracle for the capability id");
    assert_eq!(h.broker.stats().rejected_auth, 4);
    assert_eq!(h.backend.call_count(), 0);

    let mut good = cap.wire();
    assert_eq!(
        good.call(1, "timeline", json!({}))["result"]["result"]["tool"],
        "timeline"
    );
}

#[test]
fn capability_socket_and_runtime_dir_are_private() {
    let h = harness();
    let task = task();
    let grant = h
        .broker
        .grant(draft(&task), Duration::from_secs(60))
        .unwrap();
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&h.runtime), 0o700);
    assert_eq!(mode(h.broker.socket_path()), 0o600);
    assert_eq!(mode(&grant.capability_file), 0o600);

    let cap = Cap::read(&grant.capability_file);
    let file_name = grant.capability_file.file_name().unwrap().to_str().unwrap();
    assert_eq!(file_name, format!("cap-{}.json", cap.capability));
    assert!(cap.socket.is_absolute());
    assert_eq!(cap.socket, h.broker.socket_path());
    assert_eq!(cap.secret.len(), 64);
    assert!(cap.secret.bytes().all(|b| b.is_ascii_hexdigit()));
    let value: Value = serde_json::from_slice(&fs::read(&cap.file).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value.as_object().unwrap().len(), 4);

    let other = h
        .broker
        .grant(draft(&task), Duration::from_secs(60))
        .unwrap();
    let other = Cap::read(&other.capability_file);
    assert_ne!(other.secret, cap.secret);
    assert_ne!(other.capability, cap.capability);

    for text in [
        format!("{grant:?}"),
        format!("{:?}", h.broker),
        format!("{:?}", h.broker.stats()),
    ] {
        assert!(
            !text.contains(&cap.secret),
            "secret leaked into Debug output"
        );
        assert!(
            !text.contains(&other.secret),
            "secret leaked into Debug output"
        );
    }
    assert_eq!(h.broker.stats().grants, 2);
}

#[test]
fn insecure_runtime_directories_are_refused() {
    let backend = FakeBackend::new();
    let liveness = Arc::new(FakeLiveness::default());
    let dir = short_tempdir();

    let created = dir.path().join("a").join("b");
    let broker = start_in(&created, &backend, &liveness).unwrap();
    assert_eq!(
        fs::metadata(&created).unwrap().permissions().mode() & 0o777,
        0o700
    );
    drop(broker);

    for (name, mode) in [("group", 0o750), ("world", 0o705), ("groupwrite", 0o770)] {
        let path = dir.path().join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let error = start_in(&path, &backend, &liveness).unwrap_err();
        assert!(
            matches!(error, BrokerError::InsecureRuntimeDir { .. }),
            "{name}: {error}"
        );
        assert!(
            !path.join("broker.sock").exists(),
            "{name}: nothing may be bound"
        );
    }

    let real = dir.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let link = dir.path().join("link");
    symlink(&real, &link).unwrap();
    let error = start_in(&link, &backend, &liveness).unwrap_err();
    assert!(
        matches!(error, BrokerError::InsecureRuntimeDir { .. }),
        "{error}"
    );

    let file = dir.path().join("file");
    fs::write(&file, b"x").unwrap();
    let error = start_in(&file, &backend, &liveness).unwrap_err();
    assert!(
        matches!(error, BrokerError::InsecureRuntimeDir { .. }),
        "{error}"
    );

    let long = dir.path().join("x".repeat(120));
    let error = start_in(&long, &backend, &liveness).unwrap_err();
    assert!(
        matches!(error, BrokerError::SocketPathTooLong(_)),
        "{error}"
    );
}

#[test]
fn a_second_broker_in_the_same_runtime_dir_is_refused_and_stale_files_are_cleared() {
    let h = harness();
    let error = start_in(&h.runtime, &h.backend, &h.liveness).unwrap_err();
    assert!(matches!(error, BrokerError::AlreadyRunning(_)), "{error}");
    // The refused start left the running broker untouched.
    let cap = h.grant(&task());
    assert_eq!(
        cap.wire().call(1, "timeline", json!({}))["result"]["revision"]["id"],
        REV_A
    );

    // A crashed broker leaves a socket file and capability files behind.
    let dir = short_tempdir();
    let runtime = dir.path().join("rt");
    let first = start_in(&runtime, &h.backend, &h.liveness).unwrap();
    let stale_cap = first
        .grant(draft(&task()), Duration::from_secs(60))
        .unwrap();
    std::mem::forget(first);
    assert!(stale_cap.capability_file.exists());
    // `forget` leaked a live broker (threads keep serving); a fresh start must refuse.
    let error = start_in(&runtime, &h.backend, &h.liveness).unwrap_err();
    assert!(matches!(error, BrokerError::AlreadyRunning(_)), "{error}");
}

#[test]
fn expiry_refuses_new_and_established_connections() {
    let h = harness();
    let task = task();
    let grant = h
        .broker
        .grant(draft(&task), Duration::from_millis(1500))
        .unwrap();
    let cap = Cap::read(&grant.capability_file);
    let mut established = cap.wire();
    assert!(
        established
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );

    thread::sleep(Duration::from_millis(1700));
    assert!(grant.expires_at < std::time::SystemTime::now());

    let mut fresh = Wire::connect(&cap.socket);
    let reply = fresh.hello(&cap.capability, &cap.secret);
    assert_eq!(
        error_code(&reply),
        "expired",
        "expiry is only revealed after the secret verified"
    );
    assert!(fresh.recv().is_none());

    let reply = established.call(2, "timeline", json!({}));
    assert_eq!(error_code(&reply), "expired");
    assert_eq!(reply["id"], 2);
    assert!(
        established.recv().is_none(),
        "the connection closes after expiry"
    );
    assert_eq!(h.backend.call_count(), 1);

    let mut wrong = Wire::connect(&cap.socket);
    let reply = wrong.hello(&cap.capability, &"1".repeat(64));
    assert_eq!(
        error_code(&reply),
        "unauthorized",
        "an expired capability does not reveal itself to a wrong secret"
    );
}

#[test]
fn revoke_refuses_reconnect_and_the_next_call() {
    let h = harness();
    let (one, two) = (task(), task());
    let cap_one = h.grant(&one);
    let cap_two = h.grant(&two);
    let mut established = cap_one.wire();
    let mut survivor = cap_two.wire();
    assert!(
        established
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );

    h.broker.revoke(&one);
    assert!(!cap_one.file.exists(), "the capability file is deleted");
    assert_eq!(h.broker.stats().grants, 1);

    let reply = established.call(2, "timeline", json!({}));
    assert_eq!(error_code(&reply), "expired");
    assert!(established.recv().is_none());

    let mut reconnect = Wire::connect(&cap_one.socket);
    let reply = reconnect.hello(&cap_one.capability, &cap_one.secret);
    assert_ne!(
        reply,
        json!({"ok": true}),
        "a revoked capability never reconnects"
    );
    assert!(reply.get("error").is_some());
    assert!(matches!(
        BrokerClient::connect(&cap_one.file),
        Err(ClientError::CapabilityFile(_))
    ));

    assert!(
        survivor
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );
}

#[test]
fn dead_tasks_are_stale_for_new_and_established_connections() {
    let h = harness();
    let task = task();
    let cap = h.grant(&task);
    let mut established = cap.wire();
    assert!(
        established
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );

    h.liveness.kill(&task);
    let reply = established.call(2, "timeline", json!({}));
    assert_eq!(error_code(&reply), "stale_task");
    assert!(established.recv().is_none());

    let mut fresh = Wire::connect(&cap.socket);
    let reply = fresh.hello(&cap.capability, &cap.secret);
    assert_eq!(error_code(&reply), "stale_task");
    assert_eq!(h.backend.call_count(), 1);
}

#[test]
fn oversized_request_lines_close_the_connection_without_executing() {
    let h = harness();
    let cap = h.grant(&task());

    let mut wire = cap.wire();
    let padding = "x".repeat(MAX_REQUEST_BYTES + 10);
    wire.send_raw(&format!(
        "{{\"id\":1,\"method\":\"timeline\",\"params\":{{\"project\":\"{padding}\"}}}}\n"
    ));
    let reply = wire.recv().expect("an error line before the close");
    assert_eq!(error_code(&reply), "invalid_params");
    assert!(wire.recv().is_none());

    // No newline at all: the broker must not buffer without bound.
    let mut endless = cap.wire();
    let mut writer = endless.stream.try_clone().unwrap();
    let flood = thread::spawn(move || {
        let chunk = vec![b'y'; 16 * 1024];
        for _ in 0..64 {
            if writer.write_all(&chunk).is_err() {
                return;
            }
        }
    });
    let reply = endless.recv().expect("an error line before the close");
    assert_eq!(error_code(&reply), "invalid_params");
    assert!(endless.recv().is_none());
    flood.join().unwrap();

    // A line exactly at the limit is still a request.
    let mut exact = cap.wire();
    let head = "{\"id\":1,\"method\":\"timeline\",\"params\":{\"project\":\"";
    let tail = "\"}}";
    let fill = "p".repeat(MAX_REQUEST_BYTES - head.len() - tail.len());
    exact.send_raw(&format!("{head}{fill}{tail}\n"));
    let reply = exact.recv().unwrap();
    assert_eq!(
        error_code(&reply),
        "cross_project",
        "parsed and dispatched, not size-rejected"
    );
    assert_eq!(h.backend.call_count(), 0);
    assert_eq!(
        h.broker.stats().executed,
        1,
        "the at-limit call reached the dispatcher only"
    );
}

#[test]
fn queue_holds_sixteen_behind_two_workers_and_the_nineteenth_is_busy() {
    let h = harness();
    let gate = Gate::new();
    {
        let gate = gate.clone();
        h.backend.set(move |_, request, cancelled| {
            gate.pass(cancelled);
            Ok(plain_reply(request))
        });
    }
    let cap = h.grant(&task());
    assert_eq!((MAX_QUEUED_CALLS, TOOL_WORKERS), (16, 2));

    let mut workers: Vec<Wire> = (0..2).map(|_| cap.wire()).collect();
    for (index, wire) in workers.iter_mut().enumerate() {
        wire.request(100 + index as u64, "timeline", json!({}));
    }
    gate.wait_entered(2);

    // Two pipelining connections queue 8 calls each: 16 waiting calls.
    let mut queued: Vec<Wire> = (0..2).map(|_| cap.wire()).collect();
    for wire in &mut queued {
        let batch: String = (1..=8)
            .map(|id| {
                format!(
                    "{}\n",
                    json!({"id": id, "method": "build_status", "params": {}})
                )
            })
            .collect();
        wire.send_raw(&batch);
    }
    wait_until("16 queued calls", || {
        h.broker.stats().queued_high_water == 16
    });

    let mut overflow = cap.wire();
    let started = Instant::now();
    let reply = overflow.call(7, "timeline", json!({}));
    assert_eq!(error_code(&reply), "busy");
    assert_eq!(reply["id"], 7);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "busy is immediate, not queued"
    );
    assert_eq!(
        h.backend.call_count(),
        2,
        "the busy call was not executed; the queue has not started"
    );

    gate.open();
    for (index, wire) in workers.iter_mut().enumerate() {
        let reply = wire.recv().unwrap();
        assert_eq!(reply["id"], 100 + index as u64);
        assert!(reply.get("result").is_some());
    }
    for wire in &mut queued {
        for id in 1..=8u64 {
            let reply = wire.recv().unwrap();
            assert_eq!(
                reply["id"], id,
                "one connection is answered strictly in order"
            );
            assert_eq!(reply["result"]["result"]["tool"], "build_status");
        }
    }
    // The overflow connection is still usable once the queue drained.
    assert!(
        overflow
            .call(8, "timeline", json!({}))
            .get("result")
            .is_some()
    );

    let stats = h.broker.stats();
    assert_eq!(h.backend.max_running.load(Ordering::SeqCst), TOOL_WORKERS);
    assert_eq!(stats.queued_high_water, MAX_QUEUED_CALLS);
    assert_eq!(stats.rejected_busy, 1);
    assert_eq!(stats.executed, 19);
}

#[test]
fn requests_on_one_connection_run_in_order() {
    let h = harness();
    let order = Arc::new(Mutex::new(Vec::new()));
    {
        let order = order.clone();
        h.backend.set(move |_, request, _| {
            if let ToolCall::RenderFrame { frame, .. } = request.call {
                // Earlier calls are slower: only ordering keeps the sequence.
                thread::sleep(Duration::from_millis(60 - 10 * frame as u64));
                order.lock().push(frame);
            }
            Ok(plain_reply(request))
        });
    }
    let mut wire = h.grant(&task()).wire();
    let batch: String = (0..5u64)
        .map(|frame| {
            format!(
                "{}\n",
                json!({"id": frame + 1, "method": "render_frame", "params": {"frame": frame}})
            )
        })
        .collect();
    wire.send_raw(&batch);
    for id in 1..=5u64 {
        assert_eq!(wire.recv().unwrap()["id"], id);
    }
    assert_eq!(*order.lock(), vec![0, 1, 2, 3, 4]);
}

#[test]
fn a_dropped_connection_cancels_its_executing_call_and_queued_calls() {
    let h = harness();
    let gate = Gate::new();
    let saw_cancel = Arc::new(AtomicBool::new(false));
    {
        let (gate, saw_cancel) = (gate.clone(), saw_cancel.clone());
        h.backend.set(move |_, _, cancelled| {
            if gate.pass(cancelled) {
                return Err(ToolError::new(ToolErrorCode::Internal, "gate opened"));
            }
            saw_cancel.store(true, Ordering::SeqCst);
            Err(ToolError::new(ToolErrorCode::Cancelled, "cancelled"))
        });
    }
    let cap = h.grant(&task());
    let mut wire = cap.wire();
    wire.send_raw(&format!(
        "{}\n{}\n",
        json!({"id": 1, "method": "timeline", "params": {}}),
        json!({"id": 2, "method": "build_status", "params": {}})
    ));
    gate.wait_entered(1);
    drop(wire);
    wait_until("cancellation of the executing call", || {
        saw_cancel.load(Ordering::SeqCst)
    });
    wait_until("connection teardown", || {
        h.broker.stats().live_connections == 0
    });
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        h.backend.call_count(),
        1,
        "the call queued behind it never ran"
    );
    assert_eq!(h.backend.running.load(Ordering::SeqCst), 0);

    // The workers are free again.
    h.backend.set(|_, request, _| Ok(plain_reply(request)));
    assert!(
        cap.wire()
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );
}

#[test]
fn revoke_cancels_an_executing_call() {
    let h = harness();
    let gate = Gate::new();
    let saw_cancel = Arc::new(AtomicBool::new(false));
    {
        let (gate, saw_cancel) = (gate.clone(), saw_cancel.clone());
        h.backend.set(move |_, _, cancelled| {
            if !gate.pass(cancelled) {
                saw_cancel.store(true, Ordering::SeqCst);
            }
            Err(ToolError::new(ToolErrorCode::Cancelled, "cancelled"))
        });
    }
    let task = task();
    let mut wire = h.grant(&task).wire();
    wire.request(1, "timeline", json!({}));
    gate.wait_entered(1);
    h.broker.revoke(&task);
    wait_until("cancellation by revoke", || {
        saw_cancel.load(Ordering::SeqCst)
    });
    let reply = wire.recv().unwrap();
    assert_eq!(reply["id"], 1);
    assert!(reply.get("error").is_some());
}

#[test]
fn shutdown_cancels_joins_and_removes_socket_and_capabilities() {
    let h = harness();
    let gate = Gate::new();
    {
        let gate = gate.clone();
        h.backend.set(move |_, _, cancelled| {
            gate.pass(cancelled);
            Err(ToolError::new(ToolErrorCode::Cancelled, "shutdown"))
        });
    }
    let task = task();
    let cap = h.grant(&task);
    let second = h.grant(&task);
    let mut wire = cap.wire();
    wire.request(1, "timeline", json!({}));
    gate.wait_entered(1);
    assert!(h.broker.stats().live_threads >= TOOL_WORKERS + 2);
    let socket = h.broker.socket_path().to_path_buf();
    assert!(socket.exists());

    h.broker.shutdown();
    assert!(!socket.exists(), "the socket file is removed");
    assert!(
        !cap.file.exists() && !second.file.exists(),
        "capability files are removed"
    );
    let stats = h.broker.stats();
    assert_eq!(stats.live_threads, 0, "every broker thread was joined");
    assert_eq!(stats.live_connections, 0);
    assert_eq!(stats.grants, 0);
    assert_eq!(
        h.backend.running.load(Ordering::SeqCst),
        0,
        "the in-flight call was cancelled"
    );
    assert!(wire.recv().is_none(), "established connections are closed");
    assert!(UnixStream::connect(&socket).is_err());
    assert!(matches!(
        h.broker.grant(draft(&task), Duration::from_secs(1)),
        Err(BrokerError::ShutDown)
    ));
    h.broker.shutdown();
    assert!(
        h.runtime.exists(),
        "the runtime directory itself is the app's"
    );
}

#[test]
fn dropping_the_broker_shuts_it_down() {
    let dir = short_tempdir();
    let runtime = dir.path().join("rt");
    let backend = FakeBackend::new();
    let liveness = Arc::new(FakeLiveness::default());
    let broker = start_in(&runtime, &backend, &liveness).unwrap();
    let grant = broker
        .grant(draft(&task()), Duration::from_secs(60))
        .unwrap();
    let socket = broker.socket_path().to_path_buf();
    let _held = Cap::read(&grant.capability_file).wire();
    drop(broker);
    assert!(!socket.exists());
    assert!(!grant.capability_file.exists());
    assert_eq!(
        fs::read_dir(&runtime).unwrap().count(),
        0,
        "nothing is left behind"
    );
    // The same runtime directory can host a new broker straight away.
    let again = start_in(&runtime, &backend, &liveness).unwrap();
    assert!(again.socket_path().exists());
}

#[test]
fn a_panicking_backend_is_an_internal_error_and_the_broker_keeps_serving() {
    let h = harness();
    h.backend.set(|binding, request, cancelled| {
        if let ToolCall::RenderFrame { frame: 13, .. } = request.call {
            panic!("backend exploded");
        }
        standard(binding, request, cancelled)
    });
    let cap = h.grant(&task());
    let mut wire = cap.wire();
    for id in 1..=(TOOL_WORKERS as u64 + 2) {
        let reply = wire.call(id, "render_frame", json!({"frame": 13}));
        assert_eq!(error_code(&reply), "internal");
        assert_eq!(reply["id"], id);
    }
    assert!(wire.call(10, "timeline", json!({})).get("result").is_some());
    assert!(
        cap.wire()
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );
    assert_eq!(h.backend.running.load(Ordering::SeqCst), 0);
    assert!(
        h.broker.stats().live_threads > TOOL_WORKERS,
        "workers and acceptor survived"
    );
}

#[test]
fn unknown_methods_and_bad_params_are_answered_without_closing() {
    let h = harness();
    let mut wire = h.grant(&task()).wire();
    assert_eq!(
        error_code(&wire.call(1, "shell", json!({}))),
        "method_not_found"
    );
    assert_eq!(
        error_code(&wire.call(2, "render_frame", json!({"frame": "x"}))),
        "invalid_params"
    );
    wire.send_raw("[1,2]\n");
    assert_eq!(error_code(&wire.recv().unwrap()), "invalid_params");
    wire.send_raw(
        "{\"id\":9,\"method\":\"render_frame\",\"params\":{\"frame\":1,\"scale\":NaN}}\n",
    );
    assert_eq!(error_code(&wire.recv().unwrap()), "invalid_params");
    wire.send_raw("{\"method\":\"timeline\"}\n");
    assert_eq!(error_code(&wire.recv().unwrap()), "invalid_params");
    assert!(wire.call(3, "timeline", json!({})).get("result").is_some());
    assert_eq!(h.backend.call_count(), 1);
}

#[test]
fn hello_must_arrive_within_five_seconds() {
    let h = harness();
    let cap = h.grant(&task());
    let mut silent = Wire::connect(&cap.socket);
    let started = Instant::now();
    assert!(
        silent.recv().is_none(),
        "the broker closes a connection that never says hello"
    );
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(4500),
        "closed too early: {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(9),
        "closed too late: {waited:?}"
    );
    assert_eq!(h.broker.stats().rejected_auth, 1);
}

#[test]
fn at_most_eight_connections_are_served() {
    let h = harness();
    let cap = h.grant(&task());
    let mut wires: Vec<Wire> = (0..8).map(|_| cap.wire()).collect();
    wait_until("8 live connections", || {
        h.broker.stats().live_connections == 8
    });

    let mut ninth = Wire::connect(&cap.socket);
    let reply = ninth.recv().expect("a refusal line");
    assert_eq!(error_code(&reply), "busy");
    assert!(ninth.recv().is_none());

    drop(wires.pop());
    wait_until("a free connection slot", || {
        h.broker.stats().live_connections == 7
    });
    assert!(
        cap.wire()
            .call(1, "timeline", json!({}))
            .get("result")
            .is_some()
    );
}

// ------------------------------------------------------- slow-peer adversaries

/// Connects, optionally authenticates, then sends one byte every `interval` (never a
/// newline) until the broker closes the connection, `stop` is set or 30 s pass. Returns
/// how long the connection stayed open after the first byte.
fn spawn_dripper(
    socket: PathBuf,
    auth: Option<(String, String)>,
    interval: Duration,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<Duration> {
    thread::spawn(move || {
        let mut wire = Wire::connect(&socket);
        if let Some((capability, secret)) = auth {
            assert_eq!(wire.hello(&capability, &secret), json!({"ok": true}));
        }
        let begun = Instant::now();
        while !stop.load(Ordering::SeqCst) && begun.elapsed() < Duration::from_secs(30) {
            if wire.stream.write_all(b" ").is_err() {
                break;
            }
            thread::sleep(interval);
        }
        begun.elapsed()
    })
}

#[test]
fn a_hello_dripped_without_a_newline_is_closed_at_the_deadline_and_frees_capacity() {
    let h = harness();
    let cap = h.grant(&task());
    let stop = Arc::new(AtomicBool::new(false));
    let drippers: Vec<_> = (0..8)
        .map(|_| {
            spawn_dripper(
                cap.socket.clone(),
                None,
                Duration::from_millis(30),
                stop.clone(),
            )
        })
        .collect();
    wait_until("8 dripping connections", || {
        h.broker.stats().live_connections == 8
    });
    let mut refused = Wire::connect(&cap.socket);
    assert_eq!(
        error_code(&refused.recv().expect("a refusal line")),
        "busy",
        "the broker is full while the drippers hold their slots"
    );
    for dripper in drippers {
        let open = dripper.join().unwrap();
        assert!(
            open >= Duration::from_millis(4500),
            "closed too early: {open:?}"
        );
        assert!(
            open < Duration::from_secs(7),
            "a dripping hello outlived the hello deadline: {open:?}"
        );
    }
    wait_until("every dripper's slot freed", || {
        h.broker.stats().live_connections == 0
    });
    assert_eq!(h.broker.stats().rejected_auth, 8);
    let mut wire = cap.wire();
    assert!(wire.call(1, "timeline", json!({})).get("result").is_some());
}

#[test]
fn an_oversized_hello_is_rejected_at_the_limit_before_any_newline() {
    let h = harness();
    let cap = h.grant(&task());
    let mut wire = Wire::connect(&cap.socket);
    let started = Instant::now();
    // Exactly the limit, no newline: still waiting for the rest of the line.
    for _ in 0..8 {
        wire.send_raw(&" ".repeat(512));
        thread::sleep(Duration::from_millis(10));
    }
    wire.stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let mut probe = [0u8; 1];
    match wire.stream.read(&mut probe) {
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) => {}
        other => panic!("the connection must stay open at exactly 4096 bytes: {other:?}"),
    }
    // One byte over: rejected without waiting for a newline or the deadline.
    wire.send_raw(" ");
    wire.stream.set_read_timeout(Some(WAIT)).unwrap();
    assert!(
        wire.recv().is_none(),
        "the oversized hello closes the connection"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "rejected only at the deadline: {:?}",
        started.elapsed()
    );
    wait_until("the slot is freed", || {
        h.broker.stats().live_connections == 0
    });
    assert_eq!(h.broker.stats().rejected_auth, 1);
    assert_eq!(h.backend.call_count(), 0);
}

#[test]
fn a_client_call_against_a_dripping_reply_fails_at_its_deadline() {
    let dir = short_tempdir();
    let socket = dir.path().join("fake.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let capability = private_copy(
        dir.path(),
        "cap-fake.json",
        json!({
            "version": 1,
            "socket": socket.to_str().unwrap(),
            "capability": "fake",
            "secret": "0".repeat(64),
        })
        .to_string()
        .as_bytes(),
        0o600,
    );
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut writer = stream;
        let mut line = String::new();
        reader.read_line(&mut line).unwrap(); // hello
        writer.write_all(b"{\"ok\":true}\n").unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap(); // the request
        // A reply that never completes: one byte per 20 ms, no newline.
        let begun = Instant::now();
        while begun.elapsed() < Duration::from_secs(30) {
            if writer.write_all(b"x").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    let timeout = Duration::from_millis(700);
    let mut client = BrokerClient::connect(&capability)
        .unwrap()
        .with_call_timeout(timeout);
    let started = Instant::now();
    let failure = client.try_call("timeline", json!({})).unwrap_err();
    let took = started.elapsed();
    assert!(
        matches!(&failure, CallFailure::Transport(message) if message.contains("timed out")),
        "{failure:?}"
    );
    assert!(took >= timeout, "returned before the deadline: {took:?}");
    assert!(
        took < timeout + Duration::from_millis(400),
        "the dripping reply held the call past its deadline: {took:?}"
    );
    assert!(matches!(
        client.try_call("timeline", json!({})).unwrap_err(),
        CallFailure::Transport(message) if message.contains("closed")
    ));
    drop(client);
    server.join().unwrap();
}

#[test]
fn shutdown_with_live_drippers_is_bounded_and_peers_see_the_close() {
    let h = harness();
    let cap = h.grant(&task());
    let stop = Arc::new(AtomicBool::new(false));
    let interval = Duration::from_millis(20);
    let mut drippers: Vec<_> = (0..5)
        .map(|_| spawn_dripper(cap.socket.clone(), None, interval, stop.clone()))
        .collect();
    // Authenticated connections that drip a request line forever.
    drippers.extend((0..2).map(|_| {
        spawn_dripper(
            cap.socket.clone(),
            Some((cap.capability.clone(), cap.secret.clone())),
            interval,
            stop.clone(),
        )
    }));
    wait_until("7 dripping connections", || {
        h.broker.stats().live_connections == 7
    });
    thread::sleep(Duration::from_millis(200));
    let started = Instant::now();
    h.broker.shutdown();
    let took = started.elapsed();
    assert!(
        took < Duration::from_secs(2),
        "shutdown waited for dripping peers: {took:?}"
    );
    for dripper in drippers {
        let open = dripper.join().unwrap();
        assert!(
            open < Duration::from_secs(5),
            "a peer never saw the connection close: {open:?}"
        );
    }
    let stats = h.broker.stats();
    assert_eq!(stats.live_connections, 0);
    assert_eq!(stats.live_threads, 0);
    assert!(!h.broker.socket_path().exists());
}

// ------------------------------------------------------------------ client

fn private_copy(dir: &Path, name: &str, bytes: &[u8], mode: u32) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn client_roundtrip_and_error_mapping() {
    let h = harness();
    let task = task();
    let cap = h.grant(&task);
    let mut client = BrokerClient::connect(&cap.file).unwrap();
    let value = client.call("render_frame", json!({"frame": 3})).unwrap();
    assert_eq!(value["artifacts"][0]["id"], "art-0123456789abcdef");
    let error = client
        .call("render_frame", json!({"frame": 999}))
        .unwrap_err();
    assert_eq!(error.code, ToolErrorCode::NotFound);
    let error = client
        .call("render_frame", json!({"frame": -1}))
        .unwrap_err();
    assert_eq!(error.code, ToolErrorCode::InvalidParams);
    let error = client.call("nope", json!({})).unwrap_err();
    assert_eq!(error.code, ToolErrorCode::MethodNotFound);
    // The client refuses an oversized request itself and stays usable.
    let huge = json!({"project": "p".repeat(MAX_REQUEST_BYTES)});
    assert_eq!(
        client.call("timeline", huge).unwrap_err().code,
        ToolErrorCode::TooLarge
    );
    assert!(client.call("timeline", json!({})).is_ok());

    let text = format!("{client:?}");
    assert!(!text.contains(&cap.secret));

    h.liveness.kill(&task);
    let failure = client.try_call("timeline", json!({})).unwrap_err();
    assert!(matches!(&failure, CallFailure::Tool(e) if e.code == ToolErrorCode::StaleTask));
    let failure = client.try_call("timeline", json!({})).unwrap_err();
    assert!(
        matches!(failure, CallFailure::Transport(_)),
        "a closed connection is a transport failure"
    );
    assert_eq!(
        client.call("timeline", json!({})).unwrap_err().code,
        ToolErrorCode::Unavailable
    );
    match BrokerClient::connect(&cap.file) {
        Err(ClientError::Rejected(error)) => assert_eq!(error.code, ToolErrorCode::StaleTask),
        other => panic!("expected a rejection, got {other:?}"),
    }
}

#[test]
fn client_call_timeout_marks_the_connection_broken() {
    let h = harness();
    let gate = Gate::new();
    {
        let gate = gate.clone();
        h.backend.set(move |_, request, cancelled| {
            gate.pass(cancelled);
            Ok(plain_reply(request))
        });
    }
    let cap = h.grant(&task());
    let mut client = BrokerClient::connect(&cap.file)
        .unwrap()
        .with_call_timeout(Duration::from_millis(300));
    let started = Instant::now();
    let error = client.call("timeline", json!({})).unwrap_err();
    assert_eq!(error.code, ToolErrorCode::Unavailable);
    assert!(started.elapsed() < Duration::from_secs(10));
    // A late reply must never be taken for the answer to the next call.
    gate.open();
    assert_eq!(
        client.call("timeline", json!({})).unwrap_err().code,
        ToolErrorCode::Unavailable
    );
}

#[test]
fn client_refuses_unsafe_or_malformed_capability_files() {
    let h = harness();
    let cap = h.grant(&task());
    let good = fs::read(&cap.file).unwrap();
    let dir = short_tempdir();

    let message = |result: Result<BrokerClient, ClientError>| match result {
        Err(ClientError::CapabilityFile(message)) => message,
        other => panic!("expected CapabilityFile, got {other:?}"),
    };

    let link = dir.path().join("link.json");
    symlink(&cap.file, &link).unwrap();
    message(BrokerClient::connect(&link));

    let readable = private_copy(dir.path(), "group.json", &good, 0o640);
    assert!(message(BrokerClient::connect(&readable)).contains("0600"));
    let world = private_copy(dir.path(), "world.json", &good, 0o604);
    message(BrokerClient::connect(&world));

    let mut bloated = good.clone();
    bloated.resize(5000, b' ');
    let bloated = private_copy(dir.path(), "big.json", &bloated, 0o600);
    assert!(message(BrokerClient::connect(&bloated)).contains("4096"));

    let garbage = private_copy(dir.path(), "garbage.json", b"{\"secret\": 12", 0o600);
    message(BrokerClient::connect(&garbage));
    let mut wrong_version: Value = serde_json::from_slice(&good).unwrap();
    wrong_version["version"] = json!(2);
    let wrong_version = private_copy(
        dir.path(),
        "v2.json",
        wrong_version.to_string().as_bytes(),
        0o600,
    );
    message(BrokerClient::connect(&wrong_version));
    let mut short_secret: Value = serde_json::from_slice(&good).unwrap();
    short_secret["secret"] = json!("abc");
    let short_secret = private_copy(
        dir.path(),
        "short.json",
        short_secret.to_string().as_bytes(),
        0o600,
    );
    let error = message(BrokerClient::connect(&short_secret));
    assert!(!error.contains(&cap.secret));
    message(BrokerClient::connect(&dir.path().join("missing.json")));
    message(BrokerClient::connect(dir.path()));

    // Owned by someone else (only checkable when the test runs as root).
    let foreign = private_copy(dir.path(), "foreign.json", &good, 0o600);
    if std::os::unix::fs::chown(&foreign, Some(65534), None).is_ok() {
        assert!(message(BrokerClient::connect(&foreign)).contains("owned"));
    }

    // A well-formed file naming a socket nobody serves is "unreachable", not "unusable".
    let mut dangling: Value = serde_json::from_slice(&good).unwrap();
    dangling["socket"] = json!(dir.path().join("nobody-home.sock").to_str().unwrap());
    let dangling = private_copy(
        dir.path(),
        "dangling.json",
        dangling.to_string().as_bytes(),
        0o600,
    );
    assert!(matches!(
        BrokerClient::connect(&dangling),
        Err(ClientError::Unreachable(_))
    ));
    let mut relative: Value = serde_json::from_slice(&good).unwrap();
    relative["socket"] = json!("relative.sock");
    let relative = private_copy(
        dir.path(),
        "relative.json",
        relative.to_string().as_bytes(),
        0o600,
    );
    message(BrokerClient::connect(&relative));

    // The unmodified original still works.
    assert!(BrokerClient::connect(&cap.file).is_ok());
}

// --------------------------------------------------------------------- MCP

fn rpc(server: &mut McpServer, request: Value) -> Value {
    let line = server
        .handle_line(&request.to_string())
        .expect("a response");
    assert!(!line.contains('\n'), "one response per line");
    serde_json::from_str(&line).unwrap()
}

fn initialize(server: &mut McpServer) -> Value {
    rpc(
        server,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion": MCP_PROTOCOL_VERSION, "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}}}),
    )
}

#[test]
fn mcp_lifecycle_errors_and_lazy_connection() {
    let connects = Arc::new(AtomicUsize::new(0));
    let mut server = {
        let connects = connects.clone();
        McpServer::new(move || {
            connects.fetch_add(1, Ordering::SeqCst);
            Err(ToolError::new(ToolErrorCode::Unavailable, "no broker"))
        })
    };
    // Requests other than ping/initialize need initialize first.
    let early = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":9,"method":"tools/list"}),
    );
    assert_eq!(early["error"]["code"], NOT_INITIALIZED);
    assert_eq!(early["id"], 9);
    assert_eq!(
        rpc(&mut server, json!({"jsonrpc":"2.0","id":8,"method":"ping"}))["result"],
        json!({})
    );

    let init = initialize(&mut server);
    assert_eq!(init["jsonrpc"], "2.0");
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
    assert_eq!(
        init["result"]["capabilities"],
        json!({"tools": {"listChanged": false}})
    );
    assert_eq!(init["result"]["serverInfo"]["name"], "fframes-studio");
    assert!(init["result"]["serverInfo"]["version"].is_string());

    // A client asking for another version still gets ours.
    let other = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":"abc","method":"initialize","params":{"protocolVersion":"1999-01-01"}}),
    );
    assert_eq!(other["id"], "abc");
    assert_eq!(other["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
    let bad = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":3,"method":"initialize"}),
    );
    assert_eq!(bad["error"]["code"], INVALID_PARAMS);

    assert!(
        server
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none()
    );
    assert!(
        server
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/whatever","params":{}}"#)
            .is_none()
    );
    assert!(
        server
            .handle_line(r#"{"jsonrpc":"2.0","method":"nonexistent/notification"}"#)
            .is_none()
    );
    assert!(server.handle_line("").is_none());
    assert!(server.handle_line("   ").is_none());
    assert!(
        server
            .handle_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)
            .is_none(),
        "responses are ignored"
    );

    assert!(
        rpc(
            &mut server,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/list"})
        )
        .get("result")
        .is_some()
    );
    assert_eq!(
        connects.load(Ordering::SeqCst),
        0,
        "no broker connection before the first tool call"
    );

    let unknown = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":5,"method":"resources/list"}),
    );
    assert_eq!(unknown["error"]["code"], METHOD_NOT_FOUND);

    let parse = server.handle_line("{not json").unwrap();
    let parse: Value = serde_json::from_str(&parse).unwrap();
    assert_eq!(parse["error"]["code"], PARSE_ERROR);
    assert_eq!(parse["id"], Value::Null);
    for invalid in [
        "[]",
        "[{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}]",
        "42",
        "\"text\"",
        "null",
        "{}",
    ] {
        let reply: Value = serde_json::from_str(&server.handle_line(invalid).unwrap()).unwrap();
        assert_eq!(reply["error"]["code"], INVALID_REQUEST, "{invalid}");
        assert_eq!(reply["id"], Value::Null);
    }
    let bad_id = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":{"a":1},"method":"ping"}),
    );
    assert_eq!(bad_id["error"]["code"], INVALID_REQUEST);
    let bad_version = rpc(&mut server, json!({"jsonrpc":"1.0","id":6,"method":"ping"}));
    assert_eq!(bad_version["error"]["code"], INVALID_REQUEST);
    assert_eq!(bad_version["id"], 6);
    let bad_method = rpc(&mut server, json!({"jsonrpc":"2.0","id":7,"method":5}));
    assert_eq!(bad_method["error"]["code"], INVALID_REQUEST);

    let oversized = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"ping","pad":"{}"}}"#,
        "x".repeat(MAX_MCP_LINE_BYTES)
    );
    let reply: Value = serde_json::from_str(&server.handle_line(&oversized).unwrap()).unwrap();
    assert_eq!(reply["error"]["code"], INVALID_REQUEST);

    for (id, params) in [
        (10, json!({})),
        (11, json!({"name": 5})),
        (12, json!({"name": "shell"})),
        (13, json!({"name": "timeline", "arguments": [1]})),
        (14, json!("timeline")),
    ] {
        let reply = rpc(
            &mut server,
            json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":params}),
        );
        assert_eq!(reply["error"]["code"], INVALID_PARAMS, "{params}");
    }
    assert_eq!(
        connects.load(Ordering::SeqCst),
        0,
        "rejected calls never connect"
    );

    // A tool failure is a successful JSON-RPC response with isError.
    let call = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"timeline"}}),
    );
    assert!(call.get("error").is_none());
    assert_eq!(call["result"]["isError"], true);
    let text: Value =
        serde_json::from_str(call["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, json!({"code": "unavailable", "message": "no broker"}));
    assert_eq!(connects.load(Ordering::SeqCst), 1);
}

type Answer = dyn Fn(&str) -> Result<Value, ToolError> + Send + Sync;

struct RecordingCaller {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    answer: Arc<Answer>,
}

impl ToolCaller for RecordingCaller {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, ToolError> {
        self.calls.lock().push((method.into(), params));
        (self.answer)(method)
    }
}

#[test]
fn mcp_forwards_calls_verbatim_and_maps_replies_and_errors() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let connects = Arc::new(AtomicUsize::new(0));
    let reply_value = json!({"revision": {"id": REV_A, "label": "draft_snapshot", "validated": false},
        "result": {"zebra": 1, "alpha": [1, 2, {"deep": null}]}, "artifacts": []});
    let mut server = {
        let (calls, connects, reply_value) = (calls.clone(), connects.clone(), reply_value.clone());
        McpServer::new(move || {
            connects.fetch_add(1, Ordering::SeqCst);
            let reply_value = reply_value.clone();
            Ok(Box::new(RecordingCaller {
                calls: calls.clone(),
                answer: Arc::new(move |method| match method {
                    "render_frame" => Err(ToolError::new(ToolErrorCode::Unavailable, "link down")),
                    "build_status" => Err(ToolError::new(ToolErrorCode::Busy, "queue is full")),
                    _ => Ok(reply_value.clone()),
                }),
            }))
        })
    };
    initialize(&mut server);

    let arguments =
        json!({"frame": 3, "scale": 0.25, "project": "proj-1", "z": {"nested": [true]}});
    let call = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
        "params":{"name":"timeline","arguments":arguments.clone()}}),
    );
    assert_eq!(call["result"]["isError"], false);
    assert_eq!(call["result"]["structuredContent"], reply_value);
    let content = &call["result"]["content"];
    assert_eq!(content.as_array().unwrap().len(), 1);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(
        content[0]["text"],
        serde_json::to_string(&reply_value).unwrap()
    );
    assert_eq!(
        calls.lock().last().unwrap(),
        &("timeline".to_string(), arguments),
        "method and arguments are forwarded verbatim"
    );

    // Missing arguments become an empty object, exactly like the CLI.
    rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"project_context"}}),
    );
    assert_eq!(
        calls.lock().last().unwrap(),
        &("project_context".to_string(), json!({}))
    );

    // A tool error: isError content, no structuredContent, not a JSON-RPC error.
    let busy = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"build_status"}}),
    );
    assert!(busy.get("error").is_none());
    assert_eq!(busy["result"]["isError"], true);
    assert!(busy["result"].get("structuredContent").is_none());
    let text: Value =
        serde_json::from_str(busy["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, json!({"code": "busy", "message": "queue is full"}));
    assert_eq!(
        connects.load(Ordering::SeqCst),
        1,
        "a tool error keeps the connection"
    );

    // A transport failure drops the connection; the next call reconnects.
    let down = rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"render_frame","arguments":{"frame":1}}}),
    );
    assert_eq!(down["result"]["isError"], true);
    rpc(
        &mut server,
        json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"timeline"}}),
    );
    assert_eq!(connects.load(Ordering::SeqCst), 2);
    assert!(!server.drain_diagnostics().is_empty());
}

// -------------------------------------------------------------- subprocess

struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn cli(cap: Option<&Path>, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_studio-tools"));
    command.env_remove(CAPABILITY_ENV);
    if let Some(cap) = cap {
        command.arg("--capability").arg(cap);
    }
    command.args(args);
    command
}

struct Finished {
    code: i32,
    stdout: String,
    stderr: String,
}

fn finish(command: &mut Command) -> Finished {
    let output = command.output().unwrap();
    Finished {
        code: output.status.code().expect("exited normally"),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

fn cmdline(child: &Child) -> String {
    let bytes = fs::read(format!("/proc/{}/cmdline", child.id())).expect("child is alive");
    String::from_utf8_lossy(&bytes).replace('\0', " ")
}

struct McpChild {
    child: Reaped,
    stdin: Option<std::process::ChildStdin>,
    lines: mpsc::Receiver<String>,
    stderr: Option<thread::JoinHandle<Vec<u8>>>,
}

impl McpChild {
    fn spawn(cap: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_studio-mcp"));
        command
            .env_remove(CAPABILITY_ENV)
            .arg("--capability")
            .arg(cap)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        let stderr = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });
        Self {
            child: Reaped(child),
            stdin,
            lines,
            stderr: Some(stderr),
        }
    }

    fn send(&mut self, value: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{value}").unwrap();
        stdin.flush().unwrap();
    }

    fn recv_line(&mut self) -> String {
        self.lines.recv_timeout(WAIT).expect("an MCP response line")
    }

    fn recv(&mut self) -> Value {
        let line = self.recv_line();
        let value: Value = serde_json::from_str(&line)
            .unwrap_or_else(|error| panic!("stdout carried a non-JSON line {line:?}: {error}"));
        assert_eq!(
            value["jsonrpc"], "2.0",
            "stdout carries JSON-RPC only: {line}"
        );
        value
    }

    /// Close stdin, expect a clean exit and return what is left on stdout/stderr.
    fn close(mut self) -> (Vec<String>, Vec<u8>) {
        drop(self.stdin.take());
        let status = self.child.0.wait().unwrap();
        assert_eq!(status.code(), Some(0), "stdin EOF is a clean exit");
        let rest: Vec<String> = self.lines.iter().collect();
        let stderr = self.stderr.take().unwrap().join().unwrap();
        (rest, stderr)
    }
}

#[test]
fn cli_and_mcp_return_byte_identical_replies_without_the_secret_on_argv() {
    let h = harness();
    let gate = Gate::new();
    {
        let gate = gate.clone();
        h.backend.set(move |binding, request, cancelled| {
            if matches!(request.call, ToolCall::BuildStatus) {
                gate.pass(cancelled);
            }
            standard(binding, request, cancelled)
        });
    }
    let cap = h.grant(&task());

    // CLI: the artifact reply and the error case.
    let args = r#"{"frame":3,"scale":0.25}"#;
    let ok = finish(&mut cli(Some(&cap.file), &["render_frame", "--json", args]));
    assert_eq!(ok.code, 0, "{}", ok.stderr);
    assert!(ok.stderr.is_empty());
    assert!(
        ok.stdout.ends_with('\n') && ok.stdout.matches('\n').count() == 1,
        "one compact line"
    );
    let cli_text = ok.stdout.trim_end_matches('\n').to_string();
    let cli_value: Value = serde_json::from_str(&cli_text).unwrap();
    assert_eq!(cli_value["artifacts"][0]["media_type"], "image/png");
    assert_eq!(
        cli_text,
        serde_json::to_string(&cli_value).unwrap(),
        "compact JSON"
    );

    let failed = finish(&mut cli(
        Some(&cap.file),
        &["render_frame", "--json", r#"{"frame":999}"#],
    ));
    assert_eq!(failed.code, 1);
    assert!(failed.stdout.is_empty());
    let cli_error: Value = serde_json::from_str(failed.stderr.trim_end()).unwrap();
    assert_eq!(cli_error["error"]["code"], "not_found");

    // The same calls through MCP.
    let mut mcp = McpChild::spawn(&cap.file);
    mcp.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion": MCP_PROTOCOL_VERSION}}));
    let init = mcp.recv();
    assert_eq!(init["result"]["protocolVersion"], MCP_PROTOCOL_VERSION);
    mcp.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    mcp.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let list = mcp.recv();
    assert_eq!(list["id"], 2, "the notification produced no output");
    let listed: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(listed, TOOL_NAMES);

    let argv = cmdline(&mcp.child.0);
    assert!(
        !argv.contains(&cap.secret),
        "the secret must never be on argv"
    );
    assert!(
        argv.contains(cap.file.to_str().unwrap()),
        "only the capability file path is passed"
    );

    mcp.send(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"render_frame","arguments":serde_json::from_str::<Value>(args).unwrap()}}));
    let call = mcp.recv();
    assert_eq!(call["id"], 3);
    assert_eq!(call["result"]["isError"], false);
    assert_eq!(call["result"]["structuredContent"], cli_value);
    assert_eq!(
        call["result"]["content"][0]["text"].as_str().unwrap(),
        cli_text,
        "byte-identical compact text"
    );
    assert_eq!(
        serde_json::to_string(&call["result"]["structuredContent"]).unwrap(),
        cli_text
    );

    mcp.send(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"render_frame","arguments":{"frame":999}}}));
    let error = mcp.recv();
    assert_eq!(error["result"]["isError"], true);
    let error_text: Value =
        serde_json::from_str(error["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        error_text, cli_error["error"],
        "CLI stderr and MCP isError carry the same error"
    );

    mcp.send(
        &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"launch_missiles"}}),
    );
    assert_eq!(mcp.recv()["error"]["code"], INVALID_PARAMS);
    mcp.send(&json!({"jsonrpc":"2.0","id":6,"method":"ping"}));
    assert_eq!(mcp.recv()["result"], json!({}));
    mcp.send(&json!({"jsonrpc":"2.0","id":7,"method":"bogus"}));
    assert_eq!(mcp.recv()["error"]["code"], METHOD_NOT_FOUND);
    mcp.stdin
        .as_mut()
        .unwrap()
        .write_all(b"this is not json\n")
        .unwrap();
    let parse = mcp.recv();
    assert_eq!(parse["error"]["code"], PARSE_ERROR);
    assert_eq!(parse["id"], Value::Null);
    let (rest, stderr) = mcp.close();
    assert!(
        rest.is_empty(),
        "stdout carried nothing beyond the responses: {rest:?}"
    );
    assert!(stderr.len() <= 16 * 1024, "stderr stays bounded");
    assert!(!String::from_utf8_lossy(&stderr).contains(&cap.secret));
    assert!(
        String::from_utf8_lossy(&stderr).contains("unqualified"),
        "the qualification is stated at startup"
    );

    // The CLI's argv while it runs (held inside the backend by the gate).
    let mut command = cli(Some(&cap.file), &["build_status"]);
    let args_for_secret_check: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(
        args_for_secret_check
            .iter()
            .all(|arg| !arg.contains(&cap.secret))
    );
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = Reaped(command.spawn().unwrap());
    gate.wait_entered(1);
    let argv = cmdline(&child.0);
    assert!(
        !argv.contains(&cap.secret),
        "the secret must never be on argv"
    );
    assert!(argv.contains("build_status"));
    gate.open();
    let output = {
        let mut child = child;
        let status = child.0.wait().unwrap();
        let mut out = String::new();
        child
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        (status.code(), out)
    };
    assert_eq!(output.0, Some(0));
    let value: Value = serde_json::from_str(output.1.trim_end()).unwrap();
    assert_eq!(value["result"]["tool"], "build_status");
}

#[test]
fn cli_exit_codes_and_list() {
    let h = harness();
    let task = task();
    let cap = h.grant(&task);

    let ok = finish(&mut cli(Some(&cap.file), &["timeline"]));
    assert_eq!(ok.code, 0, "{}", ok.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&ok.stdout).unwrap()["result"]["tool"],
        "timeline"
    );

    // The capability path may come from the environment.
    let via_env = finish(cli(None, &["project_context"]).env(CAPABILITY_ENV, &cap.file));
    assert_eq!(via_env.code, 0, "{}", via_env.stderr);

    let listing = finish(&mut cli(None, &["--list"]));
    assert_eq!(listing.code, 0);
    assert_eq!(
        serde_json::from_str::<Value>(&listing.stdout).unwrap(),
        Value::Array(tool_descriptions())
    );
    assert_eq!(finish(&mut cli(None, &["--help"])).code, 0);

    // 1: a tool error.
    for params in [r#"{"frame":999}"#, r#"{"frame":1,"scale":9}"#] {
        let failed = finish(&mut cli(
            Some(&cap.file),
            &["render_frame", "--json", params],
        ));
        assert_eq!(failed.code, 1, "{params}");
        assert!(failed.stdout.is_empty());
        let error: Value = serde_json::from_str(failed.stderr.trim_end()).unwrap();
        assert!(error["error"]["code"].is_string() && error["error"]["message"].is_string());
    }

    // 2: usage errors.
    for args in [
        vec![],
        vec!["launch_missiles"],
        vec!["timeline", "extra"],
        vec!["render_frame", "--json", "{not json"],
        vec!["render_frame", "--json", "[1]"],
        vec!["render_frame", "--json"],
        vec!["--bogus"],
        vec!["--list", "timeline"],
    ] {
        let usage = finish(&mut cli(Some(&cap.file), &args));
        assert_eq!(usage.code, 2, "{args:?}: {}", usage.stderr);
        assert!(usage.stdout.is_empty());
    }
    let no_capability = finish(&mut cli(None, &["timeline"]));
    assert_eq!(no_capability.code, 2);
    assert!(no_capability.stderr.contains(CAPABILITY_ENV));

    // 3: broker unreachable / capability unusable.
    let dir = short_tempdir();
    let missing = dir.path().join("missing.json");
    let unusable = finish(&mut cli(Some(&missing), &["timeline"]));
    assert_eq!(unusable.code, 3);
    let error: Value = serde_json::from_str(unusable.stderr.trim_end()).unwrap();
    assert_eq!(error["error"]["code"], "unavailable");
    let readable = private_copy(
        dir.path(),
        "readable.json",
        &fs::read(&cap.file).unwrap(),
        0o644,
    );
    assert_eq!(finish(&mut cli(Some(&readable), &["timeline"])).code, 3);
    let copy = private_copy(
        dir.path(),
        "copy.json",
        &fs::read(&cap.file).unwrap(),
        0o600,
    );

    h.liveness.kill(&task);
    let stale = finish(&mut cli(Some(&cap.file), &["timeline"]));
    assert_eq!(stale.code, 3, "a rejected capability is unusable");
    assert!(stale.stderr.contains("stale_task"));

    h.broker.shutdown();
    let down = finish(&mut cli(Some(&copy), &["timeline"]));
    assert_eq!(down.code, 3, "{}", down.stderr);
    assert!(!down.stderr.contains(&cap.secret));
}

#[test]
fn mcp_flags_and_a_missing_broker() {
    let protocol = Command::new(env!("CARGO_BIN_EXE_studio-mcp"))
        .arg("--protocol")
        .output()
        .unwrap();
    assert_eq!(protocol.status.code(), Some(0));
    let text = String::from_utf8(protocol.stdout).unwrap();
    assert!(text.contains(MCP_PROTOCOL_VERSION));
    assert!(text.contains(MCP_QUALIFICATION));
    assert!(MCP_QUALIFICATION.starts_with("unqualified"));

    let usage = Command::new(env!("CARGO_BIN_EXE_studio-mcp"))
        .env_remove(CAPABILITY_ENV)
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
    let usage = Command::new(env!("CARGO_BIN_EXE_studio-mcp"))
        .arg("--bogus")
        .output()
        .unwrap();
    assert_eq!(usage.status.code(), Some(2));
    // stdout carries JSON-RPC only: a usage error is a bounded stderr message.
    assert!(usage.stdout.is_empty());
    assert!(String::from_utf8_lossy(&usage.stderr).contains("usage: studio-mcp"));

    // initialize/tools/list work with no broker; a tool call reports it as a tool error.
    let dir = short_tempdir();
    let mut mcp = McpChild::spawn(&dir.path().join("no-such-capability.json"));
    mcp.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion": MCP_PROTOCOL_VERSION}}));
    assert!(mcp.recv().get("result").is_some());
    mcp.send(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    assert_eq!(mcp.recv()["result"]["tools"].as_array().unwrap().len(), 9);
    mcp.send(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"timeline"}}));
    let call = mcp.recv();
    assert_eq!(call["result"]["isError"], true);
    let error: Value =
        serde_json::from_str(call["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(error["code"], "unavailable");
    // A line over the limit is rejected and the stream resynchronizes at the newline.
    let mut oversized = vec![b'z'; MAX_MCP_LINE_BYTES + 100];
    oversized.push(b'\n');
    mcp.stdin.as_mut().unwrap().write_all(&oversized).unwrap();
    assert_eq!(mcp.recv()["error"]["code"], INVALID_REQUEST);
    mcp.send(&json!({"jsonrpc":"2.0","id":4,"method":"ping"}));
    assert_eq!(mcp.recv()["id"], 4);
    let (rest, _) = mcp.close();
    assert!(rest.is_empty());
}

#[test]
fn mcp_stderr_is_bounded_to_sixteen_kib() {
    let dir = short_tempdir();
    let mut mcp = McpChild::spawn(&dir.path().join("no-such-capability.json"));
    let mut stdin = mcp.stdin.take().unwrap();
    let writer = thread::spawn(move || {
        let mut requests = vec![
            json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion": MCP_PROTOCOL_VERSION}}),
        ];
        requests.extend((1..=700).map(|id| json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"timeline"}})));
        for request in requests {
            if writeln!(stdin, "{request}").is_err() {
                return;
            }
        }
    });
    for id in 0..=700u64 {
        assert_eq!(mcp.recv()["id"], id);
    }
    writer.join().unwrap();
    let (rest, stderr) = mcp.close();
    assert!(rest.is_empty());
    assert!(
        stderr.len() <= 16 * 1024,
        "stderr was {} bytes",
        stderr.len()
    );
    assert!(String::from_utf8_lossy(&stderr).contains("log limit reached"));
}

#[test]
fn grants_are_short_lived_and_expired_ones_do_not_linger() {
    use fframes_studio::agent_tools::broker::MAX_GRANT_TTL;
    assert!(MAX_GRANT_TTL <= Duration::from_secs(60 * 60));
    let h = harness();
    // A request for ten days is clamped to the maximum lifetime.
    let long = h
        .broker
        .grant(draft(&task()), Duration::from_secs(10 * 24 * 60 * 60))
        .unwrap();
    let latest = std::time::SystemTime::now() + MAX_GRANT_TTL + Duration::from_secs(5);
    assert!(long.expires_at <= latest, "ttl must be clamped");

    let brief = h
        .broker
        .grant(draft(&task()), Duration::from_millis(300))
        .unwrap();
    assert!(brief.capability_file.is_file());
    thread::sleep(Duration::from_millis(450));
    // The next grant sweeps the expired capability and its secret-bearing file.
    let fresh = h.grant(&task());
    assert!(!brief.capability_file.exists(), "expired file was swept");
    assert!(long.capability_file.is_file());
    assert!(fresh.file.is_file());
    assert_eq!(h.broker.stats().grants, 2);
}

#[test]
fn initialize_answers_the_one_revision_this_server_implements_whatever_the_client_asks() {
    assert_eq!(MCP_PROTOCOL_VERSION, "2025-06-18");
    for asked in ["2025-11-25", "2025-06-18", "2024-11-05", "nonsense"] {
        let mut server = McpServer::new(|| Err(ToolError::new(ToolErrorCode::Unavailable, "n/a")));
        let reply = rpc(
            &mut server,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion": asked, "capabilities": {},
                "clientInfo": {"name": "t", "version": "0"}}}),
        );
        assert_eq!(
            reply["result"]["protocolVersion"], MCP_PROTOCOL_VERSION,
            "client asked {asked}"
        );
        assert_eq!(
            reply["result"]["capabilities"]["tools"]["listChanged"],
            false
        );
    }
}
