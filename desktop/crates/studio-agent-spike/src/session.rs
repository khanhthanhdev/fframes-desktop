use crate::{
    AgentSupervisor, SupervisorError,
    driver::{McpStdioServer, redact_sensitive_string},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use studio_bootstrap::{ChildEnvironment, ProcessTreeManager, TrackedChild};

pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const MAX_TRANSCRIPT_BYTES: usize = 64 * 1024;

#[derive(Clone, Deserialize, Serialize)]
pub struct AdapterConfig {
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Explicit variable names to forward; values are never stored in configuration.
    #[serde(default)]
    pub auth_env_names: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}
#[derive(Clone, Debug)]
pub struct PendingPermission {
    pub id: Value,
    pub title: String,
    pub options: Vec<PermissionOption>,
}
#[derive(Clone, Debug, Default)]
pub struct SessionProgress {
    pub status: String,
    pub transcript: String,
    pub diagnostics: String,
    pub session_id: Option<String>,
    pub permissions: Vec<PendingPermission>,
    pub agent_info: Option<Value>,
    pub permission_requests: usize,
    pub streamed_chunks: usize,
    pub stop_reason: Option<String>,
    pub awaiting_reply: bool,
}

fn bounded_append(text: &mut String, extra: &str) {
    text.push_str(extra);
    if text.len() > MAX_TRANSCRIPT_BYTES {
        let mut split = text.len() - MAX_TRANSCRIPT_BYTES;
        while !text.is_char_boundary(split) {
            split += 1;
        }
        text.drain(..split);
    }
}

/// One-line JSON-RPC framing capped before allocation can grow unbounded.
fn read_message(reader: &mut impl BufRead) -> Result<Option<Value>, SupervisorError> {
    let mut bytes = Vec::new();
    let n = reader
        .take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if n == 0 {
        return Ok(None);
    }
    if n > MAX_MESSAGE_BYTES || bytes.last() != Some(&b'\n') {
        return Err(SupervisorError::Other(
            "oversized or truncated ACP message".into(),
        ));
    }
    let message: Value = serde_json::from_slice(&bytes)?;
    if message["jsonrpc"] != "2.0" {
        return Err(SupervisorError::Other("invalid JSON-RPC version".into()));
    }
    Ok(Some(message))
}

#[derive(Clone)]
pub struct SessionControl {
    writer: mpsc::SyncSender<Vec<u8>>,
    raw_transcript: Arc<Mutex<String>>,
    child: Arc<Mutex<TrackedChild>>,
    cancelled: Arc<AtomicBool>,
    pub progress: Arc<Mutex<SessionProgress>>,
    secrets: Arc<Vec<String>>,
    teardown: Arc<Mutex<()>>,
    followup: Arc<Mutex<Option<String>>>,
}
impl SessionControl {
    /// Queues one user clarification for the next turn in the same session.
    pub fn submit_reply(&self, text: &str) -> Result<(), SupervisorError> {
        let progress = self.progress.lock();
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(SupervisorError::Cancelled);
        }
        if !progress.awaiting_reply {
            return Err(SupervisorError::Other(
                "Adapter is not awaiting a reply".into(),
            ));
        }
        if text.trim().is_empty() || text.len() > MAX_TRANSCRIPT_BYTES {
            return Err(SupervisorError::Other(
                "Reply is empty or exceeds the text limit".into(),
            ));
        }
        let mut queued = self.followup.lock();
        if queued.is_some() {
            return Err(SupervisorError::Other("A reply is already queued".into()));
        }
        *queued = Some(text.to_owned());
        Ok(())
    }
    fn sanitize(&self, value: &str) -> String {
        let mut text = value.to_owned();
        for secret in self.secrets.iter().filter(|s| !s.is_empty()) {
            text = text.replace(secret, "[REDACTED]");
        }
        for secret in self.secrets.iter().filter(|s| !s.is_empty()) {
            for (end, _) in secret.char_indices().rev().filter(|(end, _)| *end > 0) {
                if text.ends_with(&secret[..end]) {
                    text.truncate(text.len() - end);
                    text.push_str("[REDACTED]");
                    break;
                }
            }
        }
        redact_sensitive_string(&text)
    }
    fn write(&self, message: &Value) -> Result<(), SupervisorError> {
        let mut bytes = serde_json::to_vec(message)?;
        if bytes.len() >= MAX_MESSAGE_BYTES {
            return Err(SupervisorError::Other(
                "outbound ACP message exceeds limit".into(),
            ));
        }
        bytes.push(b'\n');
        self.writer
            .try_send(bytes)
            .map_err(|_| SupervisorError::Other("ACP writer is closed or backpressured".into()))?;
        Ok(())
    }
    pub fn choose_permission(&self, id: &Value, option: &str) -> Result<(), SupervisorError> {
        let mut progress = self.progress.lock();
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(SupervisorError::Cancelled);
        }
        let index = progress
            .permissions
            .iter()
            .position(|p| &p.id == id)
            .ok_or_else(|| SupervisorError::Other("permission is no longer pending".into()))?;
        if !progress.permissions[index]
            .options
            .iter()
            .any(|p| p.option_id == option)
        {
            return Err(SupervisorError::Other("unknown permission option".into()));
        }
        self.write(&json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"selected","optionId":option}}}))?;
        progress.permissions.remove(index);
        progress.status = "Running".into();
        Ok(())
    }
    /// Resolves outstanding permissions, sends session/cancel and reaps the owned tree.
    pub fn cancel(&self) -> Result<(), SupervisorError> {
        let _teardown = self.teardown.lock();
        self.cancelled.store(true, Ordering::SeqCst);
        {
            let mut progress = self.progress.lock();
            for permission in progress.permissions.drain(..) {
                let _=self.write(&json!({"jsonrpc":"2.0","id":permission.id,"result":{"outcome":{"outcome":"cancelled"}}}));
            }
            if let Some(id) = &progress.session_id {
                let _ = self.write(
                    &json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":id}}),
                );
            }
            progress.status = "Cancelled".into();
        }
        std::thread::sleep(Duration::from_millis(50));
        AgentSupervisor::cancel_task(&self.child)
    }
}

pub struct AcpSession {
    pub control: SessionControl,
    incoming: Option<mpsc::Receiver<Result<Value, SupervisorError>>>,
    readers: Vec<JoinHandle<()>>,
    next_id: u64,
    project_root: PathBuf,
    mcp_servers: Vec<McpStdioServer>,
}
impl AcpSession {
    pub fn spawn(
        config: &AdapterConfig,
        project: &Path,
        manager: ProcessTreeManager,
    ) -> Result<Self, SupervisorError> {
        Self::spawn_with_mcp_servers(config, project, manager, Vec::new())
    }
    /// Like [`Self::spawn`], but `session/new` carries these stdio MCP servers (ACP v1
    /// wire shape). Env values are redacted from diagnostics and permission titles.
    pub fn spawn_with_mcp_servers(
        config: &AdapterConfig,
        project: &Path,
        manager: ProcessTreeManager,
        mcp_servers: Vec<McpStdioServer>,
    ) -> Result<Self, SupervisorError> {
        for server in &mcp_servers {
            server
                .validate()
                .map_err(|error| SupervisorError::Other(error.to_string()))?;
        }
        let project_root = project.canonicalize()?;
        let mut env = ChildEnvironment::default_allowlist();
        let mut secrets: Vec<String> = mcp_servers
            .iter()
            .flat_map(|server| server.secrets().map(str::to_owned))
            .collect();
        for name in &config.auth_env_names {
            if let Ok(value) = std::env::var(name) {
                secrets.push(value.clone());
                env.set(name, value);
            }
        }
        let child = AgentSupervisor::new(manager).spawn_adapter(
            Path::new(&config.executable),
            &config.args,
            project,
            Some(env),
        )?;
        let (stdin, stdout, stderr) = {
            let mut child = child.lock();
            let process = child.child_mut();
            (
                process
                    .stdin
                    .take()
                    .ok_or_else(|| SupervisorError::Other("no adapter stdin".into()))?,
                process
                    .stdout
                    .take()
                    .ok_or_else(|| SupervisorError::Other("no adapter stdout".into()))?,
                process
                    .stderr
                    .take()
                    .ok_or_else(|| SupervisorError::Other("no adapter stderr".into()))?,
            )
        };
        let progress = Arc::new(Mutex::new(SessionProgress {
            status: "Initializing".into(),
            ..Default::default()
        }));
        let (writer_tx, writer_rx) = mpsc::sync_channel::<Vec<u8>>(24);
        let writer_thread = std::thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok(bytes) = writer_rx.recv() {
                if bytes.is_empty() || stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err()
                {
                    break;
                }
            }
        });
        let control = SessionControl {
            writer: writer_tx,
            raw_transcript: Arc::new(Mutex::new(String::new())),
            child,
            cancelled: Arc::new(AtomicBool::new(false)),
            progress,
            secrets: Arc::new(secrets),
            teardown: Arc::new(Mutex::new(())),
            followup: Arc::new(Mutex::new(None)),
        };
        let (tx, rx) = mpsc::sync_channel(32);
        let reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => {
                        if tx.send(Ok(message)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => {
                        let _ =
                            tx.send(Err(SupervisorError::Other("adapter stdout closed".into())));
                        break;
                    }
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        break;
                    }
                }
            }
        });
        let stderr_control = control.clone();
        let log_reader = std::thread::spawn(move || {
            let mut stderr = stderr;
            let mut chunk = [0u8; 4096];
            let mut retained = String::new();
            while let Ok(n) = stderr.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                bounded_append(&mut retained, &String::from_utf8_lossy(&chunk[..n]));
                stderr_control.progress.lock().diagnostics = stderr_control.sanitize(&retained);
            }
        });
        Ok(Self {
            control,
            incoming: Some(rx),
            readers: vec![reader, log_reader, writer_thread],
            next_id: 1,
            project_root,
            mcp_servers,
        })
    }
    fn request(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, SupervisorError> {
        let id = self.next_id;
        self.next_id += 1;
        if self.control.cancelled.load(Ordering::SeqCst) {
            return Err(SupervisorError::Cancelled);
        }
        let written = self
            .control
            .write(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        if self.control.cancelled.load(Ordering::SeqCst) {
            return Err(SupervisorError::Cancelled);
        }
        written?;
        let deadline = Instant::now() + timeout;
        loop {
            if self.control.cancelled.load(Ordering::SeqCst) {
                return Err(SupervisorError::Cancelled);
            }
            if Instant::now() > deadline {
                return Err(SupervisorError::Other(format!("ACP {method} timed out")));
            }
            let incoming = self
                .incoming
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_millis(100));
            if self.control.cancelled.load(Ordering::SeqCst) {
                return Err(SupervisorError::Cancelled);
            }
            let message = match incoming {
                Ok(message) => message?,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => return Err(SupervisorError::Other("ACP transport closed".into())),
            };
            if message.get("method").is_some() {
                self.handle_agent_message(&message)?;
                continue;
            }
            if message["id"] != id {
                return Err(SupervisorError::Other("unknown ACP response ID".into()));
            }
            if message.get("error").is_some() {
                let code = message["error"]["code"].as_i64().unwrap_or(-32603);
                let text = self.control.sanitize(
                    message["error"]["message"]
                        .as_str()
                        .unwrap_or("adapter error"),
                );
                return Err(SupervisorError::Other(format!("ACP error {code}: {text}")));
            }
            return message
                .get("result")
                .cloned()
                .ok_or_else(|| SupervisorError::Other("missing ACP result".into()));
        }
    }
    fn handle_agent_message(&self, message: &Value) -> Result<(), SupervisorError> {
        let method = message["method"].as_str().unwrap_or("");
        let params = &message["params"];
        let expected = self.control.progress.lock().session_id.clone();
        if method.starts_with("session/") && params["sessionId"].as_str() != expected.as_deref() {
            return Err(SupervisorError::Other("session identity mismatch".into()));
        }
        match method {
            "session/update" => {
                let update=&params["update"];
                if update["sessionUpdate"]=="agent_message_chunk" && let Some(text)=update["content"]["text"].as_str() {
                    let mut progress=self.control.progress.lock();
                    let mut raw=self.control.raw_transcript.lock();
                    bounded_append(&mut raw,text);
                    progress.transcript=self.control.sanitize(&raw);
                    progress.streamed_chunks+=1;
                }
            },
            "session/request_permission"=>{
                let id=message.get("id").cloned().ok_or_else(||SupervisorError::Other("permission missing ID".into()))?;
                let mut options:Vec<PermissionOption>=serde_json::from_value(params["options"].clone())
                    .map_err(|_|SupervisorError::Other("invalid ACP permission options".into()))?;
                for option in &mut options { option.name=self.control.sanitize(&option.name); }
                let mut progress=self.control.progress.lock();
                if progress.permissions.len()>=16 || progress.permissions.iter().any(|p|p.id==id) {
                    return Err(SupervisorError::Other("too many or duplicate permission requests".into()));
                }
                progress.permission_requests+=1; progress.status="Awaiting permission".into();
                let title=self.control.sanitize(params["toolCall"]["title"].as_str().unwrap_or("Tool permission"));
                progress.permissions.push(PendingPermission{id,title,options});
            },
            _ if message.get("id").is_some()=> self.control.write(&json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"Client capability not supported"}}))?,
            _=>{}
        }
        Ok(())
    }
    pub fn run_prompt(&mut self, project: &Path, prompt: &str) -> Result<String, SupervisorError> {
        let cwd = project.canonicalize()?;
        if cwd != self.project_root {
            return Err(SupervisorError::Other(
                "Cannot change an active session's project".into(),
            ));
        }
        if self.control.progress.lock().session_id.is_none() {
            let initialize=self.request("initialize",json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"fframes-studio","version":"0.1.0"}}),Duration::from_secs(30))?;
            if initialize["protocolVersion"] != 1 {
                return Err(SupervisorError::Other(
                    "unsupported ACP protocol version".into(),
                ));
            }
            self.control.progress.lock().agent_info = Some(
                json!({"name":self.control.sanitize(initialize["agentInfo"]["name"].as_str().unwrap_or("unknown")),"version":self.control.sanitize(initialize["agentInfo"]["version"].as_str().unwrap_or("unknown"))}),
            );
            let session = self.request(
                "session/new",
                json!({"cwd":cwd,"mcpServers":self.mcp_servers.iter().map(McpStdioServer::to_wire_json).collect::<Vec<_>>()}),
                Duration::from_secs(30),
            )?;
            let id = session["sessionId"]
                .as_str()
                .ok_or_else(|| SupervisorError::Other("missing ACP session ID".into()))?
                .to_string();
            {
                let mut progress = self.control.progress.lock();
                progress.session_id = Some(id.clone());
            }
        }
        let id = {
            let mut progress = self.control.progress.lock();
            progress.status = "Running".into();
            progress.awaiting_reply = false;
            progress.stop_reason = None;
            progress
                .session_id
                .clone()
                .ok_or_else(|| SupervisorError::Other("Missing active session".into()))?
        };
        let result = self.request(
            "session/prompt",
            json!({"sessionId":id,"prompt":[{"type":"text","text":prompt}]}),
            Duration::from_secs(900),
        )?;
        if self.control.cancelled.load(Ordering::SeqCst) {
            return Err(SupervisorError::Cancelled);
        }
        let reason = result["stopReason"]
            .as_str()
            .ok_or_else(|| SupervisorError::Other("missing authoritative stop reason".into()))?
            .to_string();
        if !matches!(
            reason.as_str(),
            "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled"
        ) {
            return Err(SupervisorError::Other("invalid ACP stop reason".into()));
        }
        let mut progress = self.control.progress.lock();
        if !progress.permissions.is_empty() {
            return Err(SupervisorError::Other(
                "completion with unresolved permissions".into(),
            ));
        }
        progress.status = format!("Completed: {reason}");
        progress.stop_reason = Some(reason.clone());
        Ok(reason)
    }

    /// Waits for an explicit user reply while preserving the adapter and draft.
    pub fn wait_for_reply(&mut self, timeout: Duration) -> Result<String, SupervisorError> {
        {
            let mut progress = self.control.progress.lock();
            if progress.stop_reason.as_deref() != Some("end_turn") {
                return Err(SupervisorError::Other(
                    "Reply requires a completed turn".into(),
                ));
            }
            progress.awaiting_reply = true;
            progress.status =
                "No source changes yet; answer in the prompt field and Send Reply, or Stop".into();
        }
        let deadline = Instant::now() + timeout;
        let result = (|| loop {
            if self.control.cancelled.load(Ordering::SeqCst) {
                return Err(SupervisorError::Cancelled);
            }
            let reply = {
                let mut progress = self.control.progress.lock();
                let reply = self.control.followup.lock().take();
                if reply.is_some() {
                    progress.awaiting_reply = false;
                }
                reply
            };
            if let Some(reply) = reply {
                return Ok(reply);
            }
            if Instant::now() >= deadline {
                return Err(SupervisorError::Other(
                    "User reply timed out; draft retained".into(),
                ));
            }
            match self
                .incoming
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_millis(100))
            {
                Ok(message) => {
                    let message = message?;
                    if message.get("method").is_none() {
                        return Err(SupervisorError::Other(
                            "Unexpected response between ACP turns".into(),
                        ));
                    }
                    self.handle_agent_message(&message)?;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => {
                    return Err(SupervisorError::Other(
                        "ACP transport closed while awaiting reply".into(),
                    ));
                }
            }
        })();
        {
            let mut progress = self.control.progress.lock();
            progress.awaiting_reply = false;
            self.control.followup.lock().take();
        }
        if self.control.cancelled.load(Ordering::SeqCst) {
            Err(SupervisorError::Cancelled)
        } else {
            result
        }
    }
}
impl Drop for AcpSession {
    fn drop(&mut self) {
        let _teardown = self.control.teardown.lock();
        let _ = AgentSupervisor::cancel_task(&self.control.child);
        self.incoming.take();
        let _ = self.control.writer.try_send(Vec::new());
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

/// Copies portable source to a new isolated draft, rejecting symlinks.
pub fn copy_draft(source: &Path, destination: &Path) -> Result<(), SupervisorError> {
    studio_project::checkpoint::copy_draft(source, destination)
        .map_err(|e| std::io::Error::other(e.to_string()).into())
}

/// Hashes portable source in stable order; compiled output never defines a revision.
pub fn source_revision(root: &Path) -> Result<String, SupervisorError> {
    studio_project::SourceInventory::scan(root)
        .map(|inventory| inventory.revision.as_str().to_owned())
        .map_err(|e| std::io::Error::other(e.to_string()).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_oversized_truncated_and_wrong_version_messages() {
        assert!(
            read_message(&mut BufReader::new(
                &b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n"[..]
            ))
            .unwrap()
            .is_some()
        );
        assert!(read_message(&mut BufReader::new(&b"{\"jsonrpc\":\"2.0\"}"[..])).is_err());
        assert!(read_message(&mut BufReader::new(&b"{\"jsonrpc\":\"1.0\"}\n"[..])).is_err());
        assert!(
            read_message(&mut BufReader::new(
                vec![b'x'; MAX_MESSAGE_BYTES + 1].as_slice()
            ))
            .is_err()
        );
    }
    #[test]
    fn revision_detects_other_source_changes_and_ignores_targets() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.rs"), "title").unwrap();
        let first = source_revision(root.path()).unwrap();
        std::fs::create_dir(root.path().join("target")).unwrap();
        std::fs::write(root.path().join("target/output"), "artifact").unwrap();
        assert_eq!(first, source_revision(root.path()).unwrap());
        std::fs::write(root.path().join("helper.rs"), "changed helper").unwrap();
        assert_ne!(first, source_revision(root.path()).unwrap());
    }

    fn peer(root: &Path, mode: &str) -> (AcpSession, ProcessTreeManager) {
        let manager = ProcessTreeManager::new();
        let config = AdapterConfig {
            executable: if cfg!(windows) { "python" } else { "python3" }.into(),
            args: vec![
                format!("{}/tests/acp-peer.py", env!("CARGO_MANIFEST_DIR")),
                mode.into(),
                root.to_string_lossy().into(),
            ],
            auth_env_names: vec![],
        };
        let mut session = AcpSession::spawn(&config, root, manager.clone()).unwrap();
        session.control.secrets = Arc::new(vec!["split-secret".into()]);
        (session, manager)
    }
    fn wait_permission(control: &SessionControl) -> PendingPermission {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(permission) = control.progress.lock().permissions.first() {
                return permission.clone();
            }
            assert!(Instant::now() < deadline, "permission did not arrive");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn real_transport_streams_permission_edit_and_authoritative_completion() {
        let root = tempfile::tempdir().unwrap();
        let (mut session, manager) = peer(root.path(), "edit");
        let control = session.control.clone();
        let path = root.path().to_owned();
        let worker = std::thread::spawn(move || session.run_prompt(&path, "Edit title"));
        let permission = wait_permission(&control);
        assert!(!control.progress.lock().transcript.contains("split-secret"));
        assert!(
            control
                .choose_permission(&permission.id, "invalid")
                .is_err()
        );
        control.choose_permission(&permission.id, "allow").unwrap();
        assert_eq!(worker.join().unwrap().unwrap(), "end_turn");
        assert!(
            std::fs::read_to_string(root.path().join("main.rs"))
                .unwrap()
                .contains("Edited title")
        );
        let progress = control.progress.lock();
        assert_eq!(progress.streamed_chunks, 4);
        assert_eq!(progress.permission_requests, 1);
        assert!(!progress.transcript.contains("split-secret"));
        assert!(!progress.diagnostics.contains("split-secret"));
        assert!(progress.transcript.contains("[REDACTED]"));
        assert!(progress.diagnostics.len() <= MAX_TRANSCRIPT_BYTES);
        assert_eq!(manager.active_count(), 0);
    }
    #[test]
    fn clarification_reply_preserves_session_and_waits_for_next_authoritative_turn() {
        let root = tempfile::tempdir().unwrap();
        let (mut session, manager) = peer(root.path(), "question");
        let control = session.control.clone();
        let before_revision = source_revision(root.path()).unwrap();
        assert!(control.submit_reply("too early").is_err());
        assert_eq!(
            session.run_prompt(root.path(), "Edit title").unwrap(),
            "end_turn"
        );
        assert!(!root.path().join("main.rs").exists());
        assert_eq!(source_revision(root.path()).unwrap(), before_revision);
        let project = root.path().to_owned();
        let task = std::thread::spawn(move || {
            let reply = session.wait_for_reply(Duration::from_secs(5)).unwrap();
            session.run_prompt(&project, &reply).unwrap()
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !control.progress.lock().awaiting_reply {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(control.submit_reply("").is_err());
        control.submit_reply("Use the clarified title").unwrap();
        assert!(
            control
                .submit_reply("Do not dispatch an extra reply")
                .is_err()
        );
        assert_eq!(task.join().unwrap(), "end_turn");
        let turns: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("target/turns.json")).unwrap())
                .unwrap();
        assert_eq!(turns["initializations"], 1);
        assert_eq!(turns["sessions"], 1);
        assert_eq!(
            turns["prompts"],
            json!(["Edit title", "Use the clarified title"])
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("main.rs")).unwrap(),
            "Use the clarified title"
        );
        assert!(!control.progress.lock().awaiting_reply);
        assert_ne!(source_revision(root.path()).unwrap(), before_revision);
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn cancelled_clarification_reaps_adapter_and_rejects_late_reply() {
        let root = tempfile::tempdir().unwrap();
        let (mut session, manager) = peer(root.path(), "question");
        let control = session.control.clone();
        session.run_prompt(root.path(), "Edit title").unwrap();
        let task = std::thread::spawn(move || session.wait_for_reply(Duration::from_secs(5)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !control.progress.lock().awaiting_reply {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        control.cancel().unwrap();
        assert!(matches!(
            task.join().unwrap(),
            Err(SupervisorError::Cancelled)
        ));
        assert!(control.submit_reply("late reply").is_err());
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn cancellation_resolves_permission_and_reaps_real_peer() {
        let root = tempfile::tempdir().unwrap();
        let (mut session, manager) = peer(root.path(), "edit");
        let control = session.control.clone();
        let path = root.path().to_owned();
        let worker = std::thread::spawn(move || session.run_prompt(&path, "Edit title"));
        wait_permission(&control);
        control.cancel().unwrap();
        assert!(matches!(
            worker.join().unwrap(),
            Err(SupervisorError::Cancelled)
        ));
        let outcome: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("permission.json")).unwrap())
                .unwrap();
        assert_eq!(outcome["outcome"], "cancelled");
        assert_eq!(manager.active_count(), 0);
        assert!(!root.path().join("main.rs").exists());
    }
    #[test]
    fn nonreading_adapter_cannot_block_cancellation() {
        let root = tempfile::tempdir().unwrap();
        let (session, manager) = peer(root.path(), "blocked");
        session.control.write(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"padding":"x".repeat(500_000)}})).unwrap();
        let start = Instant::now();
        session.control.cancel().unwrap();
        drop(session);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(manager.active_count(), 0);
    }
    #[test]
    fn invalid_stop_reason_never_enters_progress() {
        let root = tempfile::tempdir().unwrap();
        let (mut session, _) = peer(root.path(), "bad-stop");
        let control = session.control.clone();
        let path = root.path().to_owned();
        let worker = std::thread::spawn(move || session.run_prompt(&path, "Edit title"));
        let permission = wait_permission(&control);
        control.choose_permission(&permission.id, "allow").unwrap();
        let error = worker.join().unwrap().unwrap_err().to_string();
        assert!(!error.contains("split-secret"));
        assert!(control.progress.lock().stop_reason.is_none());
    }
    #[test]
    fn session_new_carries_configured_stdio_servers_in_the_acp_wire_shape() {
        let manager = ProcessTreeManager::new();
        let config = AdapterConfig {
            executable: if cfg!(windows) { "python" } else { "python3" }.into(),
            args: vec![
                format!("{}/tests/acp-peer.py", env!("CARGO_MANIFEST_DIR")),
                "question".into(),
                String::new(),
            ],
            auth_env_names: vec![],
        };
        let run = |servers: Vec<McpStdioServer>| {
            let root = tempfile::tempdir().unwrap();
            let mut config = config.clone();
            config.args[2] = root.path().to_string_lossy().into_owned();
            let mut session =
                AcpSession::spawn_with_mcp_servers(&config, root.path(), manager.clone(), servers)
                    .unwrap();
            session.run_prompt(root.path(), "Edit title").unwrap();
            let wire: Value = serde_json::from_slice(
                &std::fs::read(root.path().join("target/session-new.json")).unwrap(),
            )
            .unwrap();
            wire["mcpServers"].clone()
        };
        let command = std::env::temp_dir().join("studio-mcp");
        let server = McpStdioServer::new(
            "fframes-project",
            command.clone(),
            vec!["--task".into(), "t1".into()],
            vec![("TOKEN".into(), "wire-secret".into())],
        )
        .unwrap();
        assert_eq!(
            run(vec![server]),
            json!([{"name":"fframes-project","command":command,"args":["--task","t1"],"env":[{"name":"TOKEN","value":"wire-secret"}]}])
        );
        assert_eq!(run(Vec::new()), json!([]));
        let invalid = McpStdioServer {
            name: "srv".into(),
            command: "relative".into(),
            args: vec![],
            env: vec![],
        };
        let root = tempfile::tempdir().unwrap();
        let error = AcpSession::spawn_with_mcp_servers(
            &config,
            root.path(),
            manager.clone(),
            vec![invalid],
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("absolute"));
        assert_eq!(manager.active_count(), 0);
    }
}
