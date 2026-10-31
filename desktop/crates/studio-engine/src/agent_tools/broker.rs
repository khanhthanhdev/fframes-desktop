//! App-owned local tool broker.
//!
//! The broker listens on a local endpoint (see [`super::transport`]: a unix domain socket
//! inside the private runtime directory, a named pipe on Windows) and serves the
//! [`ToolDispatcher`] to the `studio-tools` CLI and the `studio-mcp` server. Every
//! capability is bound to one task ([`ToolBinding`]); the secret lives only in the
//! private capability file `cap-<id>.json` (`{"version":1,"socket":..,"capability":..,
//! "secret":..}`, where `socket` names the endpoint) and in broker memory.
//!
//! # Security
//! - `runtime_dir` is created private and verified (a real directory owned by us that
//!   nobody else can open, not a symlink) or the broker refuses to start. The endpoint
//!   is as private: a 0600 socket, or a pipe whose DACL grants only the current user.
//! - Every connection's peer is checked against the current user (`SO_PEERCRED` on
//!   Linux, `getpeereid` on other unixes, the client's token on Windows).
//! - The secret is 32 random bytes (hex), compared in constant time, and never put on
//!   argv, in logs, errors, `Debug`/`Display` output or [`ToolGrant`]. An unknown
//!   capability id and a wrong secret are indistinguishable (`unauthorized`);
//!   `expired`/`stale_task` are only answered after the secret verified.
//!
//! # Wire protocol (newline-delimited JSON)
//! 1. Client: `{"hello":{"capability":"..","secret":".."}}` within 5 s (an absolute
//!    deadline, however slowly bytes arrive) and at most 4096 bytes.
//!    Broker: `{"ok":true}` or `{"error":{"code","message"}}` (then it closes).
//! 2. Client: `{"id":<u64>,"method":"..","params":{..}}` lines (pipelining allowed; one
//!    connection's calls execute and are answered strictly in order).
//!    Broker: `{"id":N,"result":<dispatcher value>}` or `{"id":N,"error":{..}}`.
//!
//! # Limits
//! Request lines are at most [`MAX_REQUEST_BYTES`] (longer: `invalid_params` and the
//! connection closes; nothing is executed); replies at most [`MAX_TEXT_REPLY_BYTES`];
//! 8 concurrent connections; 10 minutes idle timeout; [`MAX_QUEUED_CALLS`] waiting
//! calls plus [`TOOL_WORKERS`] executing calls shared by all connections (a full queue
//! answers `busy` at once and the call is not executed). Before every request, and
//! again when a worker picks the call up, the capability must be un-revoked, unexpired
//! and its task live, otherwise `expired`/`stale_task` and the connection closes. An
//! executing call's `cancelled` closure turns true when its connection drops, the grant
//! is revoked or expires, or the broker shuts down. A panicking backend becomes an
//! `internal` error and the worker survives.
use super::{
    MAX_QUEUED_CALLS, MAX_REQUEST_BYTES, MAX_TEXT_REPLY_BYTES, TOOL_NAMES, TOOL_WORKERS, ToolError,
    ToolErrorCode,
    client::{LineRead, LineReader},
    transport::{self, Accepted, Stream},
};
use super::{TaskLiveness, ToolBinding, ToolDispatcher};
use crate::TaskIdentity;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

/// Concurrent client connections.
pub const MAX_CONNECTIONS: usize = 8;
/// A client must send its hello within this long.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// An authenticated connection without a request for this long is closed.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Outstanding capabilities.
pub const MAX_GRANTS: usize = 256;
/// Longest accepted capability lifetime: a capability is short-lived and bound to one
/// task; the orchestrator re-grants if a task outlives it.
pub const MAX_GRANT_TTL: Duration = Duration::from_secs(60 * 60);

pub struct BrokerConfig {
    pub runtime_dir: PathBuf,
    pub dispatcher: ToolDispatcher,
    pub liveness: Arc<dyn TaskLiveness>,
}

/// A granted capability. Only the file path leaves the broker; the secret is inside the
/// file, never in this struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolGrant {
    pub capability_file: PathBuf,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BrokerStats {
    /// Most calls ever waiting for a worker at once (never above `MAX_QUEUED_CALLS`).
    pub queued_high_water: usize,
    /// Calls handed to the dispatcher (finished, failed, cancelled or panicked).
    pub executed: u64,
    pub rejected_busy: u64,
    /// Failed authentications (wrong/unknown capability, bad peer, hello timeout).
    pub rejected_auth: u64,
    pub live_connections: usize,
    /// Active (not revoked) capabilities, including expired ones not yet revoked.
    pub grants: usize,
    /// Broker threads (acceptor, workers, connections) currently alive; 0 after shutdown.
    pub live_threads: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("runtime directory {} is not private: {reason}", path.display())]
    InsecureRuntimeDir { path: PathBuf, reason: String },
    #[error("socket path {} is too long for a unix domain socket", .0.display())]
    SocketPathTooLong(PathBuf),
    #[error("another tool broker is already serving {}", .0.display())]
    AlreadyRunning(PathBuf),
    #[error("the tool broker is shut down")]
    ShutDown,
    #[error("too many outstanding capabilities")]
    TooManyGrants,
    #[error("tool broker i/o error ({context}): {source}")]
    Io {
        context: &'static str,
        #[source]
        source: io::Error,
    },
}

fn io_error(context: &'static str) -> impl FnOnce(io::Error) -> BrokerError {
    move |source| BrokerError::Io { context, source }
}

/// Hex of random bytes, from the OS RNG through `uuid` (v4 = `getrandom`).
fn random_bytes_32() -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    out[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    out
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn unhex_32(text: &str) -> Option<[u8; 32]> {
    let bytes = text.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(bytes.as_chunks::<2>().0) {
        *slot = nibble(pair[0])? << 4 | nibble(pair[1])?;
    }
    Some(out)
}

mod imp {
    use super::*;
    use parking_lot::{Condvar, Mutex};
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::{
        collections::{HashMap, VecDeque},
        fs,
        io::Write,
        panic::{AssertUnwindSafe, catch_unwind},
        sync::{
            atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
            mpsc,
        },
        thread::{self, JoinHandle},
        time::Instant,
    };

    const MAX_HELLO_BYTES: usize = 4096;
    /// Replies buffered per connection (pipelined calls awaiting their in-order turn).
    const MAX_PIPELINED: usize = 64;
    const IDLE_TICK: Duration = Duration::from_millis(100);
    const BUSY_TICK: Duration = Duration::from_millis(5);
    const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

    /// 32 secret bytes; never printed, compared in constant time.
    struct Secret([u8; 32]);

    impl Secret {
        fn matches(&self, candidate: &[u8; 32]) -> bool {
            let mut difference = 0u8;
            for (a, b) in self.0.iter().zip(candidate) {
                difference |= a ^ b;
            }
            std::hint::black_box(difference) == 0
        }
    }

    impl std::fmt::Debug for Secret {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Secret(<redacted>)")
        }
    }

    impl Drop for Secret {
        fn drop(&mut self) {
            for byte in &mut self.0 {
                // SAFETY: `byte` is a valid, aligned, exclusive reference.
                unsafe { std::ptr::write_volatile(byte, 0) };
            }
        }
    }

    struct Grant {
        binding: ToolBinding,
        /// The tool names this capability may call.
        tools: Vec<String>,
        secret: Secret,
        deadline: Instant,
        revoked: AtomicBool,
        file: PathBuf,
    }

    impl Grant {
        /// The per-request gate: revoked / expired / dead task.
        fn check(&self, liveness: &dyn TaskLiveness) -> Result<(), ToolError> {
            if self.revoked.load(Ordering::SeqCst) {
                return Err(ToolError::new(
                    ToolErrorCode::Expired,
                    "the capability was revoked",
                ));
            }
            if Instant::now() >= self.deadline {
                return Err(ToolError::new(
                    ToolErrorCode::Expired,
                    "the capability expired",
                ));
            }
            if !liveness.is_live(&self.binding.task) {
                return Err(ToolError::new(
                    ToolErrorCode::StaleTask,
                    "the task is no longer live",
                ));
            }
            Ok(())
        }

        fn dead(&self) -> bool {
            self.revoked.load(Ordering::SeqCst) || Instant::now() >= self.deadline
        }
    }

    /// Per-connection state shared with the workers.
    struct ConnShared {
        /// The connection is gone or closing: its calls are cancelled.
        dropped: AtomicBool,
        /// A worker is executing one of this connection's calls (keeps order).
        executing: AtomicBool,
    }

    struct Job {
        conn: Arc<ConnShared>,
        grant: Arc<Grant>,
        method: String,
        params: Value,
        reply: mpsc::Sender<Result<Value, ToolError>>,
    }

    struct Shared {
        socket: PathBuf,
        runtime_dir: PathBuf,
        dispatcher: ToolDispatcher,
        liveness: Arc<dyn TaskLiveness>,
        /// Compared against when the capability id is unknown, so both failures cost the same.
        decoy: Secret,
        grants: Mutex<HashMap<String, Arc<Grant>>>,
        shutdown: AtomicBool,
        queue: Mutex<VecDeque<Job>>,
        queue_changed: Condvar,
        connections: Mutex<Vec<JoinHandle<()>>>,
        /// One shutdown handle per live connection; removed when its thread ends.
        streams: Mutex<HashMap<u64, Stream>>,
        next_connection: AtomicU64,
        live_connections: AtomicUsize,
        live_threads: AtomicUsize,
        high_water: AtomicUsize,
        executed: AtomicU64,
        rejected_busy: AtomicU64,
        rejected_auth: AtomicU64,
    }

    impl Shared {
        fn shutting_down(&self) -> bool {
            self.shutdown.load(Ordering::SeqCst)
        }

        /// Cuts every live connection off.
        fn close_connections(&self) {
            for stream in self.streams.lock().values() {
                transport::close(stream);
            }
        }
    }

    /// Counts a broker thread for as long as it runs.
    struct ThreadGuard(Arc<Shared>);

    impl ThreadGuard {
        fn new(shared: &Arc<Shared>) -> Self {
            shared.live_threads.fetch_add(1, Ordering::SeqCst);
            Self(shared.clone())
        }
    }

    impl Drop for ThreadGuard {
        fn drop(&mut self) {
            self.0.live_threads.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn spawn_named(
        shared: &Arc<Shared>,
        name: &str,
        body: impl FnOnce() + Send + 'static,
    ) -> io::Result<JoinHandle<()>> {
        let guard = ThreadGuard::new(shared);
        thread::Builder::new().name(name.into()).spawn(move || {
            let _guard = guard;
            body();
        })
    }

    pub struct ToolBroker {
        shared: Arc<Shared>,
        /// Acceptor and workers.
        threads: Mutex<Vec<JoinHandle<()>>>,
    }

    impl std::fmt::Debug for ToolBroker {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("ToolBroker")
                .field("socket", &self.shared.socket)
                .finish_non_exhaustive()
        }
    }

    fn fatal(code: ToolErrorCode) -> bool {
        matches!(
            code,
            ToolErrorCode::Expired | ToolErrorCode::StaleTask | ToolErrorCode::Unauthorized
        )
    }

    fn error_line(id: Option<u64>, error: &ToolError) -> String {
        json!({"id": id, "error": error}).to_string()
    }

    fn reply_line(id: u64, result: Result<Value, ToolError>) -> (String, bool) {
        match result {
            Ok(value) => match serde_json::to_string(&value) {
                Ok(text) if text.len() <= MAX_TEXT_REPLY_BYTES => {
                    (format!("{{\"id\":{id},\"result\":{text}}}"), false)
                }
                Ok(_) => (
                    error_line(
                        Some(id),
                        &ToolError::new(ToolErrorCode::TooLarge, "reply exceeds the text limit"),
                    ),
                    false,
                ),
                Err(error) => (
                    error_line(
                        Some(id),
                        &ToolError::new(ToolErrorCode::Internal, error.to_string()),
                    ),
                    false,
                ),
            },
            Err(error) => (error_line(Some(id), &error), fatal(error.code)),
        }
    }

    fn write_line(stream: &mut Stream, line: &str) -> io::Result<()> {
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        stream.write_all(&bytes)?;
        stream.flush()
    }

    impl ToolBroker {
        pub fn start(config: BrokerConfig) -> Result<ToolBroker, BrokerError> {
            let runtime_dir = transport::prepare_runtime_dir(&config.runtime_dir)?;
            let listener = transport::Listener::bind(&runtime_dir)?;
            let socket = listener.endpoint().to_path_buf();
            let shared = Arc::new(Shared {
                socket: socket.clone(),
                runtime_dir,
                dispatcher: config.dispatcher,
                liveness: config.liveness,
                decoy: Secret(random_bytes_32()),
                grants: Mutex::new(HashMap::new()),
                shutdown: AtomicBool::new(false),
                queue: Mutex::new(VecDeque::new()),
                queue_changed: Condvar::new(),
                connections: Mutex::new(Vec::new()),
                streams: Mutex::new(HashMap::new()),
                next_connection: AtomicU64::new(0),
                live_connections: AtomicUsize::new(0),
                live_threads: AtomicUsize::new(0),
                high_water: AtomicUsize::new(0),
                executed: AtomicU64::new(0),
                rejected_busy: AtomicU64::new(0),
                rejected_auth: AtomicU64::new(0),
            });
            let broker = ToolBroker {
                shared: shared.clone(),
                threads: Mutex::new(Vec::new()),
            };
            let mut spawned = Vec::new();
            for index in 0..TOOL_WORKERS {
                let shared = shared.clone();
                spawned.push(spawn_named(
                    &broker.shared,
                    &format!("tool-worker-{index}"),
                    move || worker_loop(&shared),
                ));
            }
            {
                let shared = shared.clone();
                spawned.push(spawn_named(&broker.shared, "tool-acceptor", move || {
                    accept_loop(&shared, &listener)
                }));
            }
            let mut failure = None;
            for handle in spawned {
                match handle {
                    Ok(handle) => broker.threads.lock().push(handle),
                    Err(error) => failure = failure.or(Some(error)),
                }
            }
            if let Some(error) = failure {
                // Dropping the broker shuts the threads that did start down.
                return Err(io_error("spawn broker thread")(error));
            }
            Ok(broker)
        }

        pub fn socket_path(&self) -> &Path {
            &self.shared.socket
        }

        /// Create a capability for `binding`, valid for `ttl` (clamped to
        /// [`MAX_GRANT_TTL`]).
        pub fn grant(&self, binding: ToolBinding, ttl: Duration) -> Result<ToolGrant, BrokerError> {
            self.grant_tools(binding, ttl, &TOOL_NAMES)
        }

        /// [`Self::grant`] for only `tools`: a call to any other tool is refused, and the
        /// capability file lists them so `studio-mcp` offers no others.
        pub fn grant_tools(
            &self,
            binding: ToolBinding,
            ttl: Duration,
            tools: &[&str],
        ) -> Result<ToolGrant, BrokerError> {
            let ttl = ttl.min(MAX_GRANT_TTL);
            let tools: Vec<String> = tools.iter().map(|tool| (*tool).to_owned()).collect();
            let id = uuid::Uuid::new_v4().simple().to_string();
            let secret = random_bytes_32();
            let file = self.shared.runtime_dir.join(format!("cap-{id}.json"));
            let socket = self
                .shared
                .socket
                .to_str()
                .ok_or(BrokerError::SocketPathTooLong(self.shared.socket.clone()))?;
            let mut body = serde_json::to_vec(&json!({
                "version": 1,
                "socket": socket,
                "capability": id,
                "secret": hex(&secret),
                "tools": tools,
            }))
            .map_err(|error| BrokerError::Io {
                context: "encode capability",
                source: io::Error::other(error),
            })?;
            body.push(b'\n');
            let mut grants = self.shared.grants.lock();
            if self.shared.shutting_down() {
                return Err(BrokerError::ShutDown);
            }
            // Expired capabilities (and their files) never linger until shutdown.
            let now = Instant::now();
            let expired: Vec<String> = grants
                .iter()
                .filter(|(_, grant)| grant.deadline <= now)
                .map(|(id, _)| id.clone())
                .collect();
            for id in expired {
                if let Some(grant) = grants.remove(&id) {
                    grant.revoked.store(true, Ordering::SeqCst);
                    let _ = fs::remove_file(&grant.file);
                }
            }
            if grants.len() >= MAX_GRANTS {
                return Err(BrokerError::TooManyGrants);
            }
            if let Err(error) = transport::create_private_file(&file, &body) {
                let _ = fs::remove_file(&file);
                return Err(io_error("write capability file")(error));
            }
            let expires_at = SystemTime::now()
                .checked_add(ttl)
                .unwrap_or_else(|| SystemTime::now() + MAX_GRANT_TTL);
            grants.insert(
                id,
                Arc::new(Grant {
                    binding,
                    tools,
                    secret: Secret(secret),
                    deadline: Instant::now() + ttl,
                    revoked: AtomicBool::new(false),
                    file: file.clone(),
                }),
            );
            Ok(ToolGrant {
                capability_file: file,
                expires_at,
            })
        }

        /// Revoke every capability of `task`: delete the files, cancel executing calls and
        /// refuse the next call of every established connection.
        pub fn revoke(&self, task: &TaskIdentity) {
            let removed: Vec<Arc<Grant>> = {
                let mut grants = self.shared.grants.lock();
                let ids: Vec<String> = grants
                    .iter()
                    .filter(|(_, grant)| grant.binding.task == *task)
                    .map(|(id, _)| id.clone())
                    .collect();
                ids.iter().filter_map(|id| grants.remove(id)).collect()
            };
            for grant in removed {
                grant.revoked.store(true, Ordering::SeqCst);
                let _ = fs::remove_file(&grant.file);
            }
            // Queued calls of revoked grants are answered `expired` when picked up.
            self.shared.queue_changed.notify_all();
        }

        pub fn stats(&self) -> BrokerStats {
            let shared = &self.shared;
            BrokerStats {
                queued_high_water: shared.high_water.load(Ordering::SeqCst),
                executed: shared.executed.load(Ordering::SeqCst),
                rejected_busy: shared.rejected_busy.load(Ordering::SeqCst),
                rejected_auth: shared.rejected_auth.load(Ordering::SeqCst),
                live_connections: shared.live_connections.load(Ordering::SeqCst),
                grants: shared.grants.lock().len(),
                live_threads: shared.live_threads.load(Ordering::SeqCst),
            }
        }

        /// Stop accepting, cancel in-flight calls, join every thread, remove the socket
        /// and capability files. Idempotent; also runs on drop. Joining waits for a
        /// backend call that ignores its `cancelled` closure.
        pub fn shutdown(&self) {
            let shared = &self.shared;
            if shared.shutdown.swap(true, Ordering::SeqCst) {
                return;
            }
            // Peers (including ones dripping bytes or sending nothing) are cut off before
            // any thread is joined.
            shared.close_connections();
            drop(shared.queue.lock());
            shared.queue_changed.notify_all();
            let threads: Vec<JoinHandle<()>> = std::mem::take(&mut *self.threads.lock());
            for handle in threads {
                let _ = handle.join();
            }
            // The acceptor is gone: sweep connections it accepted after the first pass;
            // no connection thread can be added any more.
            shared.close_connections();
            let connections: Vec<JoinHandle<()>> = std::mem::take(&mut *shared.connections.lock());
            for handle in connections {
                let _ = handle.join();
            }
            shared.queue.lock().clear();
            let grants: Vec<Arc<Grant>> = shared.grants.lock().drain().map(|(_, g)| g).collect();
            for grant in grants {
                grant.revoked.store(true, Ordering::SeqCst);
                let _ = fs::remove_file(&grant.file);
            }
            transport::remove_endpoint(&shared.socket);
        }
    }

    impl Drop for ToolBroker {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    fn accept_loop(shared: &Arc<Shared>, listener: &transport::Listener) {
        while !shared.shutting_down() {
            let stream = match listener.accept(IDLE_TICK) {
                Accepted::Peer(stream) => stream,
                Accepted::Refused => {
                    shared.rejected_auth.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                Accepted::Idle => continue,
            };
            if shared.live_connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                shared.live_connections.fetch_sub(1, Ordering::SeqCst);
                let mut stream = stream;
                let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                let _ = write_line(
                    &mut stream,
                    &error_line(
                        None,
                        &ToolError::new(ToolErrorCode::Busy, "too many broker connections"),
                    ),
                );
                continue;
            }
            // A second handle for the same stream lets `shutdown()` wake a thread blocked
            // in a read or write without waiting for the peer.
            let Ok(closer) = stream.try_clone() else {
                shared.live_connections.fetch_sub(1, Ordering::SeqCst);
                continue;
            };
            let id = shared.next_connection.fetch_add(1, Ordering::SeqCst);
            shared.streams.lock().insert(id, closer);
            let slot = ConnectionSlot {
                shared: shared.clone(),
                id,
            };
            let conn_shared = shared.clone();
            let spawned = spawn_named(shared, "tool-connection", move || {
                let _slot = slot;
                serve_connection(&conn_shared, stream);
            });
            // On failure the closure, and with it the slot count, was dropped.
            if let Ok(handle) = spawned {
                let mut connections = shared.connections.lock();
                connections.retain(|handle| !handle.is_finished());
                connections.push(handle);
            }
        }
    }

    /// Holds a connection slot (and its shutdown handle) taken by the acceptor; both are
    /// released when the connection thread ends or never started.
    struct ConnectionSlot {
        shared: Arc<Shared>,
        id: u64,
    }

    impl Drop for ConnectionSlot {
        fn drop(&mut self) {
            self.shared.streams.lock().remove(&self.id);
            self.shared.live_connections.fetch_sub(1, Ordering::SeqCst);
        }
    }

    enum Pending {
        Ready(String, bool),
        Waiting(u64, mpsc::Receiver<Result<Value, ToolError>>),
    }

    #[derive(Deserialize)]
    struct HelloEnvelope {
        hello: Hello,
    }

    #[derive(Deserialize)]
    struct Hello {
        capability: String,
        secret: String,
    }

    fn serve_connection(shared: &Arc<Shared>, mut stream: Stream) {
        let conn = Arc::new(ConnShared {
            dropped: AtomicBool::new(false),
            executing: AtomicBool::new(false),
        });
        let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
        if let Some(grant) = authenticate(shared, &mut stream) {
            connection_loop(shared, &mut stream, &conn, &grant);
        }
        conn.dropped.store(true, Ordering::SeqCst);
        // Calls still waiting for a worker never run.
        shared
            .queue
            .lock()
            .retain(|job| !Arc::ptr_eq(&job.conn, &conn));
        transport::finish(stream);
    }

    fn authenticate(shared: &Arc<Shared>, stream: &mut Stream) -> Option<Arc<Grant>> {
        // The hello line has its own small limit: an oversized or newline-less drip is
        // rejected before any newline arrives.
        let mut reader = LineReader::new(MAX_HELLO_BYTES);
        let deadline = Instant::now() + HELLO_TIMEOUT;
        let line = loop {
            if shared.shutting_down() {
                return None;
            }
            let now = Instant::now();
            if now >= deadline {
                shared.rejected_auth.fetch_add(1, Ordering::SeqCst);
                return None;
            }
            if stream.set_read_timeout(Some(IDLE_TICK)).is_err() {
                return None;
            }
            // The reader returns at the tick even while the peer keeps sending bytes, so
            // the deadline and shutdown checks above run at least every tick.
            let tick_end = deadline.min(now + IDLE_TICK);
            match reader.next(stream, true, Some(tick_end)) {
                Ok(LineRead::Line(line)) => break line,
                Ok(LineRead::Idle) => {}
                Ok(LineRead::TooLong) => {
                    shared.rejected_auth.fetch_add(1, Ordering::SeqCst);
                    return None;
                }
                Ok(LineRead::Eof) | Err(_) => return None,
            }
        };
        // A pipe client's token can only be read once it has sent something.
        if !transport::verify_peer(stream) {
            shared.rejected_auth.fetch_add(1, Ordering::SeqCst);
            return None;
        }
        let rejected = |stream: &mut Stream, error: ToolError| {
            let _ = write_line(stream, &error_line(None, &error));
        };
        let unauthorized = || {
            ToolError::new(
                ToolErrorCode::Unauthorized,
                "the capability or secret was rejected",
            )
        };
        let hello = serde_json::from_slice::<HelloEnvelope>(&line).ok();
        let Some(HelloEnvelope { hello }) = hello else {
            shared.rejected_auth.fetch_add(1, Ordering::SeqCst);
            rejected(stream, unauthorized());
            return None;
        };
        let grant = shared.grants.lock().get(&hello.capability).cloned();
        let decoded = unhex_32(&hello.secret);
        let candidate = decoded.unwrap_or([0u8; 32]);
        let secret_ok = match &grant {
            Some(grant) => grant.secret.matches(&candidate),
            None => {
                let _ = shared.decoy.matches(&candidate);
                false
            }
        };
        let verified = secret_ok && decoded.is_some();
        let grant = match grant {
            Some(grant) if verified => grant,
            _ => {
                shared.rejected_auth.fetch_add(1, Ordering::SeqCst);
                rejected(stream, unauthorized());
                return None;
            }
        };
        if let Err(error) = grant.check(&*shared.liveness) {
            rejected(stream, error);
            return None;
        }
        write_line(stream, "{\"ok\":true}").ok()?;
        Some(grant)
    }

    fn connection_loop(
        shared: &Arc<Shared>,
        stream: &mut Stream,
        conn: &Arc<ConnShared>,
        grant: &Arc<Grant>,
    ) {
        let mut reader = LineReader::new(MAX_REQUEST_BYTES);
        let mut pending: VecDeque<Pending> = VecDeque::new();
        let mut last_activity = Instant::now();
        let mut closing = false;
        loop {
            if shared.shutting_down() {
                return;
            }
            // Replies leave strictly in request order.
            while let Some(front) = pending.front_mut() {
                let (line, close) = match front {
                    Pending::Ready(line, close) => (std::mem::take(line), *close),
                    Pending::Waiting(id, receiver) => match receiver.try_recv() {
                        Ok(result) => reply_line(*id, result),
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => reply_line(
                            *id,
                            Err(ToolError::new(ToolErrorCode::Cancelled, "call was dropped")),
                        ),
                    },
                };
                pending.pop_front();
                if write_line(stream, &line).is_err() || close {
                    return;
                }
            }
            if closing && pending.is_empty() {
                return;
            }
            let may_read = !closing && pending.len() < MAX_PIPELINED;
            let tick = if pending.is_empty() {
                IDLE_TICK
            } else {
                BUSY_TICK
            };
            if !may_read {
                thread::sleep(BUSY_TICK);
                continue;
            }
            if stream.set_read_timeout(Some(tick)).is_err() {
                return;
            }
            match reader.next(stream, true, Some(Instant::now() + tick)) {
                Ok(LineRead::Line(line)) => {
                    last_activity = Instant::now();
                    closing |= handle_request(shared, conn, grant, &line, &mut pending);
                }
                Ok(LineRead::Idle) => {
                    if pending.is_empty() && last_activity.elapsed() >= IDLE_TIMEOUT {
                        return;
                    }
                }
                Ok(LineRead::TooLong) => {
                    pending.push_back(Pending::Ready(
                        error_line(
                            None,
                            &ToolError::invalid(format!(
                                "request line exceeds {MAX_REQUEST_BYTES} bytes"
                            )),
                        ),
                        true,
                    ));
                    closing = true;
                }
                Ok(LineRead::Eof) | Err(_) => return,
            }
        }
    }

    /// Returns true when the connection must close after the queued replies.
    fn handle_request(
        shared: &Arc<Shared>,
        conn: &Arc<ConnShared>,
        grant: &Arc<Grant>,
        line: &[u8],
        pending: &mut VecDeque<Pending>,
    ) -> bool {
        let ready = |pending: &mut VecDeque<Pending>, id: Option<u64>, error: ToolError| {
            pending.push_back(Pending::Ready(error_line(id, &error), fatal(error.code)));
        };
        let Ok(Value::Object(request)) = serde_json::from_slice::<Value>(line) else {
            ready(
                pending,
                None,
                ToolError::invalid("request must be a JSON object"),
            );
            return false;
        };
        let Some(id) = request.get("id").and_then(Value::as_u64) else {
            ready(
                pending,
                None,
                ToolError::invalid("request needs a numeric `id`"),
            );
            return false;
        };
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            ready(
                pending,
                Some(id),
                ToolError::invalid("request needs a `method`"),
            );
            return false;
        };
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        if let Err(error) = grant.check(&*shared.liveness) {
            ready(pending, Some(id), error);
            return true;
        }
        if TOOL_NAMES.contains(&method) && !grant.tools.iter().any(|tool| tool == method) {
            ready(
                pending,
                Some(id),
                ToolError::new(
                    ToolErrorCode::MethodNotFound,
                    format!("`{method}` is turned off in Studio's settings"),
                ),
            );
            return false;
        }
        let (sender, receiver) = mpsc::channel();
        {
            let mut queue = shared.queue.lock();
            if queue.len() >= MAX_QUEUED_CALLS {
                drop(queue);
                shared.rejected_busy.fetch_add(1, Ordering::SeqCst);
                ready(
                    pending,
                    Some(id),
                    ToolError::new(ToolErrorCode::Busy, "the tool queue is full; retry shortly"),
                );
                return false;
            }
            queue.push_back(Job {
                conn: conn.clone(),
                grant: grant.clone(),
                method: method.to_string(),
                params,
                reply: sender,
            });
            shared.high_water.fetch_max(queue.len(), Ordering::SeqCst);
        }
        shared.queue_changed.notify_all();
        pending.push_back(Pending::Waiting(id, receiver));
        false
    }

    fn worker_loop(shared: &Arc<Shared>) {
        loop {
            let job = {
                let mut queue = shared.queue.lock();
                loop {
                    if shared.shutting_down() {
                        return;
                    }
                    let next = queue
                        .iter()
                        .position(|job| !job.conn.executing.load(Ordering::SeqCst));
                    if let Some(index) = next
                        && let Some(job) = queue.remove(index)
                    {
                        job.conn.executing.store(true, Ordering::SeqCst);
                        break job;
                    }
                    shared.queue_changed.wait(&mut queue);
                }
            };
            let conn = job.conn.clone();
            run_job(shared, job);
            // Under the queue lock, so a worker deciding to sleep cannot miss it.
            let queue = shared.queue.lock();
            conn.executing.store(false, Ordering::SeqCst);
            drop(queue);
            shared.queue_changed.notify_all();
        }
    }

    fn run_job(shared: &Arc<Shared>, job: Job) {
        if job.conn.dropped.load(Ordering::SeqCst) {
            return;
        }
        let result = match job.grant.check(&*shared.liveness) {
            Err(error) => Err(error),
            Ok(()) => {
                let cancelled = || {
                    job.conn.dropped.load(Ordering::SeqCst)
                        || job.grant.dead()
                        || shared.shutting_down()
                };
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    shared.dispatcher.dispatch(
                        &job.grant.binding,
                        &job.method,
                        &job.params,
                        &cancelled,
                    )
                }));
                shared.executed.fetch_add(1, Ordering::SeqCst);
                outcome.unwrap_or_else(|_| {
                    Err(ToolError::new(
                        ToolErrorCode::Internal,
                        "the tool backend panicked",
                    ))
                })
            }
        };
        let _ = job.reply.send(result);
    }
}

pub use imp::ToolBroker;
