use fframes_studio_protocol::{
    CURRENT_PROTOCOL_VERSION, FrameHeader, HelloRequest, HelloResponse, RenderFrameRequest,
    TimelineRequest, TimelineResponse, WorkerRequest, WorkerResponse,
};
use parking_lot::Mutex;
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use studio_bootstrap::{
    ChildEnvironment, ProcessError, ProcessTreeManager, SpawnOptions, TrackedChild,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkerClientError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("worker process error: {0}")]
    Process(#[from] ProcessError),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protocol error: {0}")]
    Protocol(#[from] fframes_studio_protocol::ProtocolError),
    #[error("worker crash or unexpected exit: {0}")]
    WorkerCrashed(String),
    #[error("worker timeout")]
    Timeout,
    #[error("client error: {0}")]
    Other(String),
}

static WORKER_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Allocates identities across GUI launches, restarts and agent candidate workers.
pub fn allocate_worker_generation() -> u64 {
    WORKER_GENERATION
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
            value.checked_add(1)
        })
        .expect("worker generation exhausted")
        + 1
}

struct RequestDeadline {
    done: std::sync::mpsc::Sender<()>,
    timed_out: Arc<std::sync::atomic::AtomicBool>,
    task: Option<std::thread::JoinHandle<()>>,
}
impl RequestDeadline {
    fn new(child: Arc<Mutex<TrackedChild>>, timeout: Duration) -> Self {
        let (done, receiver) = std::sync::mpsc::channel();
        let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = timed_out.clone();
        let task = std::thread::spawn(move || {
            if matches!(
                receiver.recv_timeout(timeout),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                flag.store(true, Ordering::SeqCst);
                let _ = child.lock().kill_forcefully();
            }
        });
        Self {
            done,
            timed_out,
            task: Some(task),
        }
    }
    fn finish(&mut self) -> bool {
        let _ = self.done.send(());
        if let Some(task) = self.task.take() {
            let _ = task.join();
        }
        self.timed_out.load(Ordering::SeqCst)
    }
}
impl Drop for RequestDeadline {
    fn drop(&mut self) {
        self.finish();
    }
}

pub struct WorkerPipes {
    pub control_writer: Box<dyn Write + Send>,
    pub control_reader: Box<dyn Read + Send>,
    pub frame_reader: Box<dyn Read + Send>,
}

pub struct WorkerClient {
    generation: u64,
    request_timeout: Duration,
    revision: String,
    request_counter: AtomicU64,
    active_child: Option<Arc<Mutex<TrackedChild>>>,
    pipes: Option<WorkerPipes>,
    pending_seek: Option<usize>,
    is_active_seek: bool,
    latest_header: Option<FrameHeader>,
    latest_pixels: Option<Vec<u8>>,
    crashed: bool,
    stderr_logs: Arc<Mutex<String>>,
}

impl WorkerClient {
    pub fn new(revision: impl Into<String>, generation: u64) -> Self {
        WORKER_GENERATION.fetch_max(generation, Ordering::SeqCst);
        Self {
            generation,
            request_timeout: Duration::from_secs(30),
            revision: revision.into(),
            request_counter: AtomicU64::new(1),
            active_child: None,
            pending_seek: None,
            is_active_seek: false,
            latest_header: None,
            latest_pixels: None,
            pipes: None,
            crashed: false,
            stderr_logs: Arc::new(Mutex::new(String::new())),
        }
    }

    pub fn set_request_timeout(&mut self, timeout: Duration) {
        self.request_timeout = timeout;
    }

    fn with_deadline<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, WorkerClientError>,
    ) -> Result<T, WorkerClientError> {
        let mut deadline = self
            .active_child
            .as_ref()
            .map(|child| RequestDeadline::new(child.clone(), self.request_timeout));
        let result = operation(self);
        if deadline.as_mut().is_some_and(RequestDeadline::finish) {
            self.crashed = true;
            self.pipes = None;
            return Err(WorkerClientError::Timeout);
        }
        result
    }

    pub fn recent_logs(&self) -> String {
        self.stderr_logs.lock().clone()
    }

    pub fn attach_pipes(
        &mut self,
        control_writer: impl Write + Send + 'static,
        control_reader: impl Read + Send + 'static,
        frame_reader: impl Read + Send + 'static,
    ) {
        self.pipes = Some(WorkerPipes {
            control_writer: Box::new(control_writer),
            control_reader: Box::new(control_reader),
            frame_reader: Box::new(frame_reader),
        });
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn process_id(&self) -> Option<u32> {
        self.active_child.as_ref().map(|child| child.lock().pid())
    }

    pub fn is_crashed(&self) -> bool {
        self.crashed
    }

    pub fn latest_header(&self) -> Option<&FrameHeader> {
        self.latest_header.as_ref()
    }

    pub fn latest_pixels(&self) -> Option<&[u8]> {
        self.latest_pixels.as_deref()
    }

    pub fn attach_child(&mut self, child: Arc<Mutex<TrackedChild>>) {
        self.active_child = Some(child);
        self.crashed = false;
    }

    /// Simulates a worker crash or forced termination for crash-isolation testing.
    pub fn force_crash(&mut self) -> Result<(), WorkerClientError> {
        if let Some(child_arc) = &self.active_child {
            let mut child = child_arc.lock();
            let _ = child.kill_forcefully();
        }
        self.crashed = true;
        Ok(())
    }

    /// Restarts the worker with an incremented generation.
    pub fn restart_generation(&mut self) {
        if let Some(child_arc) = self.active_child.take() {
            let mut child = child_arc.lock();
            let _ = child.terminate_gracefully(Duration::from_millis(100));
        }
        self.pipes = None;
        self.generation = allocate_worker_generation();
        self.crashed = false;
        self.is_active_seek = false;
        self.pending_seek = None;
    }

    /// Latest-wins seek coalescing:
    /// If a seek is currently in flight, coalesces pending seek requests into a single latest target.
    pub fn request_seek(&mut self, frame_index: usize) -> Option<usize> {
        if self.is_active_seek {
            self.pending_seek = Some(frame_index);
            None
        } else {
            self.is_active_seek = true;
            Some(frame_index)
        }
    }

    /// Completes the active seek and returns the next pending seek if one was coalesced.
    /// Completes the active seek, validates frame identity and payload integrity,
    /// and returns the next pending seek if one was coalesced.
    pub fn finish_seek(
        &mut self,
        header: FrameHeader,
        pixels: Vec<u8>,
    ) -> Result<Option<usize>, WorkerClientError> {
        if header.protocol_version != CURRENT_PROTOCOL_VERSION {
            return Err(WorkerClientError::Protocol(
                fframes_studio_protocol::ProtocolError::UnsupportedProtocolVersion {
                    expected: CURRENT_PROTOCOL_VERSION,
                    actual: header.protocol_version,
                },
            ));
        }

        if header.worker_generation != self.generation {
            return Err(WorkerClientError::Protocol(
                fframes_studio_protocol::ProtocolError::StaleGeneration {
                    expected: self.generation,
                    actual: header.worker_generation,
                },
            ));
        }

        if header.source_revision != self.revision {
            return Err(WorkerClientError::Protocol(
                fframes_studio_protocol::ProtocolError::StaleRevision {
                    expected: self.revision.clone(),
                    actual: header.source_revision,
                },
            ));
        }

        header.validate().map_err(WorkerClientError::Protocol)?;
        if pixels.len() != header.payload_len {
            return Err(WorkerClientError::Protocol(
                fframes_studio_protocol::ProtocolError::PayloadLengthMismatch {
                    expected: header.payload_len,
                    actual: pixels.len(),
                },
            ));
        }
        self.latest_header = Some(header);
        self.latest_pixels = Some(pixels);

        // Maintain active state if next seek is returned to be dispatched
        if let Some(next_frame) = self.pending_seek.take() {
            self.is_active_seek = true;
            Ok(Some(next_frame))
        } else {
            self.is_active_seek = false;
            Ok(None)
        }
    }

    pub fn write_control_request(&mut self, req: &WorkerRequest) -> Result<(), WorkerClientError> {
        let pipes = self
            .pipes
            .as_mut()
            .ok_or_else(|| WorkerClientError::Other("worker pipes not attached".into()))?;
        let json_bytes = serde_json::to_vec(req)?;
        let len_bytes = (json_bytes.len() as u32).to_be_bytes();
        pipes.control_writer.write_all(&len_bytes)?;
        pipes.control_writer.write_all(&json_bytes)?;
        pipes.control_writer.flush()?;
        Ok(())
    }

    pub fn read_control_response(&mut self) -> Result<WorkerResponse, WorkerClientError> {
        let pipes = self
            .pipes
            .as_mut()
            .ok_or_else(|| WorkerClientError::Other("worker pipes not attached".into()))?;
        let mut len_bytes = [0u8; 4];
        pipes
            .control_reader
            .read_exact(&mut len_bytes)
            .map_err(|e| {
                self.crashed = true;
                WorkerClientError::Io(e)
            })?;
        let len = u32::from_be_bytes(len_bytes) as usize;
        if len > 1024 * 1024 {
            return Err(WorkerClientError::Other(format!(
                "worker response message length {len} exceeds 1MB limit"
            )));
        }
        let mut buf = vec![0u8; len];
        pipes.control_reader.read_exact(&mut buf).map_err(|e| {
            self.crashed = true;
            WorkerClientError::Io(e)
        })?;
        let resp: WorkerResponse = serde_json::from_slice(&buf)?;
        Ok(resp)
    }

    pub fn send_hello(&mut self) -> Result<HelloResponse, WorkerClientError> {
        self.with_deadline(|client| client.send_hello_inner())
    }

    fn send_hello_inner(&mut self) -> Result<HelloResponse, WorkerClientError> {
        let req = WorkerRequest::Hello(HelloRequest {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            client_version: "fframes-studio 0.1.0".into(),
        });
        self.write_control_request(&req)?;
        let resp = self.read_control_response()?;
        match resp {
            WorkerResponse::Hello(hello) => Ok(hello),
            WorkerResponse::Error(err) => Err(WorkerClientError::Other(err.message)),
            other => Err(WorkerClientError::Other(format!(
                "unexpected response to Hello: {other:?}"
            ))),
        }
    }

    pub fn request_timeline(&mut self) -> Result<TimelineResponse, WorkerClientError> {
        self.with_deadline(|client| client.request_timeline_inner())
    }

    fn request_timeline_inner(&mut self) -> Result<TimelineResponse, WorkerClientError> {
        let req_id = self.request_counter.fetch_add(1, Ordering::SeqCst);
        let req = WorkerRequest::Timeline(TimelineRequest {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            source_revision: self.revision.clone(),
            worker_generation: self.generation,
            request_id: req_id,
        });
        self.write_control_request(&req)?;
        let resp = self.read_control_response()?;
        match resp {
            WorkerResponse::Timeline(tl) => Ok(tl),
            WorkerResponse::Error(err) => Err(WorkerClientError::Other(err.message)),
            other => Err(WorkerClientError::Other(format!(
                "unexpected response to Timeline: {other:?}"
            ))),
        }
    }

    pub fn request_render_frame(
        &mut self,
        frame_index: usize,
    ) -> Result<Option<usize>, WorkerClientError> {
        let result = self.with_deadline(|client| client.request_render_frame_inner(frame_index));
        if result.is_err() {
            let _ = self.force_crash();
            self.pipes = None;
        }
        result
    }

    fn request_render_frame_inner(
        &mut self,
        frame_index: usize,
    ) -> Result<Option<usize>, WorkerClientError> {
        let req_id = self.request_counter.fetch_add(1, Ordering::SeqCst);
        let req = WorkerRequest::RenderFrame(RenderFrameRequest {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            source_revision: self.revision.clone(),
            worker_generation: self.generation,
            request_id: req_id,
            frame_index,
        });

        self.write_control_request(&req)?;
        let resp = self.read_control_response()?;

        match resp {
            WorkerResponse::RenderFrame(render_resp) => {
                if render_resp.request_id != req_id
                    || render_resp.frame_index != frame_index
                    || render_resp.protocol_version != CURRENT_PROTOCOL_VERSION
                    || render_resp.source_revision != self.revision
                    || render_resp.worker_generation != self.generation
                    || render_resp.header.frame_index != frame_index
                {
                    return Err(WorkerClientError::Other(
                        "Frame response identity mismatch".into(),
                    ));
                }
                // 1. Pre-allocation validation: verify protocol version
                if render_resp.header.protocol_version != CURRENT_PROTOCOL_VERSION {
                    return Err(WorkerClientError::Protocol(
                        fframes_studio_protocol::ProtocolError::UnsupportedProtocolVersion {
                            expected: CURRENT_PROTOCOL_VERSION,
                            actual: render_resp.header.protocol_version,
                        },
                    ));
                }

                // 2. Pre-allocation validation: verify request correlation
                if render_resp.header.request_id != req_id {
                    return Err(WorkerClientError::Other(format!(
                        "mismatched request_id: expected {req_id}, got {}",
                        render_resp.header.request_id
                    )));
                }

                // 3. Pre-allocation validation: verify worker generation
                if render_resp.header.worker_generation != self.generation {
                    return Err(WorkerClientError::Protocol(
                        fframes_studio_protocol::ProtocolError::StaleGeneration {
                            expected: self.generation,
                            actual: render_resp.header.worker_generation,
                        },
                    ));
                }

                // 4. Pre-allocation validation: verify source revision
                if render_resp.header.source_revision != self.revision {
                    return Err(WorkerClientError::Protocol(
                        fframes_studio_protocol::ProtocolError::StaleRevision {
                            expected: self.revision.clone(),
                            actual: render_resp.header.source_revision,
                        },
                    ));
                }

                // 5. Pre-allocation validation: geometry & stride invariants
                render_resp
                    .header
                    .validate()
                    .map_err(WorkerClientError::Protocol)?;

                // 6. Pre-allocation validation: bounds check against max frame payload limit
                if render_resp.header.payload_len > fframes_studio_protocol::MAX_FRAME_PAYLOAD_BYTES
                {
                    return Err(WorkerClientError::Protocol(
                        fframes_studio_protocol::ProtocolError::PayloadExceedsCap {
                            cap: fframes_studio_protocol::MAX_FRAME_PAYLOAD_BYTES,
                            actual: render_resp.header.payload_len,
                        },
                    ));
                }

                // ONLY after all validations pass do we allocate memory for frame pixels:
                let payload_len = render_resp.header.payload_len;
                let pipes = self
                    .pipes
                    .as_mut()
                    .ok_or_else(|| WorkerClientError::Other("worker pipes not attached".into()))?;
                let mut pixels = vec![0u8; payload_len];
                pipes.frame_reader.read_exact(&mut pixels).map_err(|e| {
                    self.crashed = true;
                    WorkerClientError::Io(e)
                })?;

                self.finish_seek(render_resp.header, pixels)
            }
            WorkerResponse::Error(err) => Err(WorkerClientError::Other(err.message)),
            other => Err(WorkerClientError::Other(format!(
                "unexpected worker response: {other:?}"
            ))),
        }
    }

    pub fn request_elements(
        &mut self,
        frame: &FrameHeader,
    ) -> Result<Vec<fframes_studio_protocol::ElementMetadata>, WorkerClientError> {
        self.with_deadline(|client| client.request_elements_inner(frame))
    }

    fn request_elements_inner(
        &mut self,
        frame: &FrameHeader,
    ) -> Result<Vec<fframes_studio_protocol::ElementMetadata>, WorkerClientError> {
        let id = self.request_counter.fetch_add(1, Ordering::SeqCst);
        self.write_control_request(&WorkerRequest::ElementMetadata(
            fframes_studio_protocol::ElementMetadataRequest {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                source_revision: self.revision.clone(),
                worker_generation: self.generation,
                request_id: id,
                frame_index: frame.frame_index,
            },
        ))?;
        match self.read_control_response()? {
            WorkerResponse::ElementMetadata(response)
                if response.protocol_version == CURRENT_PROTOCOL_VERSION
                    && response.source_revision == frame.source_revision
                    && response.worker_generation == frame.worker_generation
                    && response.request_id == id =>
            {
                Ok(response.elements)
            }
            other => Err(WorkerClientError::Other(format!(
                "Metadata identity mismatch: {other:?}"
            ))),
        }
    }

    pub fn spawn_worker(
        &mut self,
        program: &std::path::Path,
        args: &[&str],
        current_dir: Option<&std::path::Path>,
        env: ChildEnvironment,
        process_tree: &ProcessTreeManager,
    ) -> Result<(), WorkerClientError> {
        // Bind local ephemeral TCP port for dedicated binary frame transport
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let frame_port = listener.local_addr()?.port();

        let mut opts = SpawnOptions::new(program);
        for a in args {
            opts.arg(a);
        }
        opts.arg("--frame-port");
        opts.arg(frame_port.to_string());
        opts.arg("--generation");
        opts.arg(self.generation.to_string());
        opts.arg("--revision");
        opts.arg(&self.revision);

        if let Some(dir) = current_dir {
            opts.current_dir(dir);
        }
        opts.env = env;
        opts.stdin = std::process::Stdio::piped();
        opts.stdout = std::process::Stdio::piped();
        opts.stderr = std::process::Stdio::piped();

        let child_arc = process_tree.spawn(opts)?;
        let (stdin, stdout, mut stderr) =
            {
                let mut lock = child_arc.lock();
                let stdin = lock.child_mut().stdin.take().ok_or_else(|| {
                    WorkerClientError::Other("failed to take worker stdin".into())
                })?;
                let stdout = lock.child_mut().stdout.take().ok_or_else(|| {
                    WorkerClientError::Other("failed to take worker stdout".into())
                })?;
                let stderr = lock.child_mut().stderr.take().ok_or_else(|| {
                    WorkerClientError::Other("failed to take worker stderr".into())
                })?;
                (stdin, stdout, stderr)
            };

        // Concurrently drain stderr into diagnostic logs, keeping binary frames isolated
        let stderr_logs = Arc::clone(&self.stderr_logs);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 1024];
            while let Ok(n) = stderr.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&chunk[..n]);
                let mut logs = stderr_logs.lock();
                if logs.len() < 64 * 1024 {
                    logs.push_str(&text);
                }
            }
        });

        // Accept incoming dedicated binary frame connection from worker
        listener.set_nonblocking(true)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let frame_stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child_arc.lock().kill_forcefully();
                        return Err(WorkerClientError::Timeout);
                    }
                    if child_arc.lock().child_mut().try_wait()?.is_some() {
                        let _ = child_arc.lock().kill_forcefully();
                        return Err(WorkerClientError::WorkerCrashed(
                            "Exited before frame connection".into(),
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    let _ = child_arc.lock().kill_forcefully();
                    return Err(error.into());
                }
            }
        };
        frame_stream.set_read_timeout(Some(Duration::from_secs(30)))?;

        self.attach_child(child_arc);
        self.attach_pipes(stdin, stdout, frame_stream);
        self.crashed = false;
        Ok(())
    }

    /// Synchronous mock / fixture roundtrip helper for non-GUI unit tests.
    pub fn test_execute_frame_render(
        &mut self,
        frame_index: usize,
        width: u32,
        height: u32,
    ) -> Result<FrameHeader, WorkerClientError> {
        let req_id = self.request_counter.fetch_add(1, Ordering::SeqCst);
        let (header, payload) = crate::frame_image::generate_reference_frame(
            &self.revision,
            self.generation,
            req_id,
            frame_index,
            width,
            height,
            false,
        );

        self.latest_header = Some(header.clone());
        self.latest_pixels = Some(payload);
        Ok(header)
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        self.pipes = None;
        if let Some(child) = self.active_child.take() {
            let _ = child.lock().kill_forcefully();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_control_request_times_out_and_reaps_worker() {
        let manager = ProcessTreeManager::new();
        let mut worker = WorkerClient::new("stalled", allocate_worker_generation());
        worker.set_request_timeout(Duration::from_millis(100));
        let script = format!("{}/tests/stalled-worker.py", env!("CARGO_MANIFEST_DIR"));
        worker
            .spawn_worker(
                std::path::Path::new(if cfg!(windows) { "python" } else { "python3" }),
                &[&script],
                None,
                ChildEnvironment::default_allowlist(),
                &manager,
            )
            .unwrap();
        let start = std::time::Instant::now();
        assert!(matches!(
            worker.send_hello(),
            Err(WorkerClientError::Timeout)
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(worker.is_crashed());
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn generations_are_shared_and_strictly_increase() {
        let first = allocate_worker_generation();
        let candidate = allocate_worker_generation();
        let restart = allocate_worker_generation();
        assert!(first < candidate && candidate < restart);
    }

    #[test]
    fn test_latest_wins_seek_coalescing() {
        let mut client = WorkerClient::new("rev1", 1);

        // Seek 1 starts immediately
        let seek1 = client.request_seek(10);
        assert_eq!(seek1, Some(10));

        // Rapid seeks arrive while seek 1 is in-flight: 11, 12, 13
        assert_eq!(client.request_seek(11), None);
        assert_eq!(client.request_seek(12), None);
        assert_eq!(client.request_seek(13), None);

        // Finish seek 1 -> should immediately return the coalesced latest seek (13), skipping 11 and 12
        let dummy_header = FrameHeader::new_straight_rgba("rev1", 1, 1, 10, 10, 10).unwrap();
        let next_seek = client.finish_seek(dummy_header, vec![0; 400]).unwrap();
        assert_eq!(next_seek, Some(13));

        // Active seek invariant is preserved while coalesced seek 13 is dispatched
        assert_eq!(client.request_seek(14), None);

        // Completing seek 13 with no further pending returns None
        let dummy_header2 = FrameHeader::new_straight_rgba("rev1", 1, 2, 13, 10, 10).unwrap();
        let next_seek2 = client.finish_seek(dummy_header2, vec![0; 400]).unwrap();
        assert_eq!(next_seek2, Some(14));
    }

    #[test]
    fn test_finish_seek_rejects_invalid_and_stale() {
        let mut client = WorkerClient::new("rev1", 1);
        client.request_seek(10);

        // 1. Stale generation
        let stale_gen_header = FrameHeader::new_straight_rgba("rev1", 0, 1, 10, 10, 10).unwrap();
        assert!(client.finish_seek(stale_gen_header, vec![0; 400]).is_err());
        assert!(client.latest_header().is_none());

        // 2. Mismatched revision
        let wrong_rev_header =
            FrameHeader::new_straight_rgba("rev_other", 1, 1, 10, 10, 10).unwrap();
        assert!(client.finish_seek(wrong_rev_header, vec![0; 400]).is_err());
        assert!(client.latest_header().is_none());

        // 3. Corrupt payload length
        let valid_header = FrameHeader::new_straight_rgba("rev1", 1, 1, 10, 10, 10).unwrap();
        assert!(client.finish_seek(valid_header, vec![0; 200]).is_err()); // expected 400 bytes
        assert!(client.latest_header().is_none());
    }
    #[test]
    fn test_crash_isolation_and_restart() {
        let mut client = WorkerClient::new("rev1", 1);
        assert_eq!(client.generation(), 1);
        assert!(!client.is_crashed());

        // Force crash
        client.force_crash().expect("force crash succeeds");
        assert!(client.is_crashed());

        // Restart worker generation
        client.restart_generation();
        assert!(client.generation() > 1);
        assert!(!client.is_crashed());
    }
}
