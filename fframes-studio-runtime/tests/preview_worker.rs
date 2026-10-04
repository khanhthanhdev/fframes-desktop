use fframes::{Color, Duration, Frame, RenderOptions, Svgr, Video};
use fframes_studio_protocol::*;
use fframes_studio_runtime::{PreviewWorkerConfig, WorkerTransport, serve_preview_worker};
use parking_lot::Mutex;
use serde::Serialize;
use std::io::{self, Cursor, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration as Wait, Instant};

struct TestVideo(f32);
impl Video for TestVideo {
    const FPS: usize = 10;
    const WIDTH: usize = 320;
    const HEIGHT: usize = 180;
    const BACKGROUND_COLOR: Color = Color::BLACK;
    fn duration(&self) -> Duration<'_> {
        Duration::Seconds(self.0)
    }
    fn audio(&self) -> fframes::AudioMap<'_> {
        fframes::AudioMap::none()
    }
    fn render_frame<'a>(
        &'a self,
        _frame: Frame,
        _ctx: &fframes::FFramesContext<'a, '_>,
    ) -> Svgr<'a> {
        fframes::svgr!(<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 320 180"><rect width="320" height="180" fill="#123456" /></svg>)
    }
}

struct SharedWriter(Arc<Mutex<Vec<u8>>>);
impl Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn message<T: Serialize>(out: &mut Vec<u8>, value: &T) {
    let bytes = serde_json::to_vec(value).unwrap();
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&bytes)
}

#[test]
fn preview_lifecycle_prepares_chunked_pcm_and_cleans_on_exit() {
    let temp = tempfile::tempdir().unwrap();
    let identity = PreviewIdentity {
        project_id: "project".into(),
        open_session: "session".into(),
        source_revision: "rev".into(),
        worker_generation: 7,
    };
    let env = |request_id| PreviewEnvelope {
        contract_version: PREVIEW_CONTRACT_VERSION,
        identity: identity.clone(),
        request_id,
    };
    let mut requests = Vec::new();
    message(
        &mut requests,
        &PreviewRequest::Hello(PreviewHelloRequest {
            offered_versions: vec![1],
            required_capabilities: PREVIEW_CAPABILITIES.iter().map(|v| (*v).into()).collect(),
            request_id: 1,
        }),
    );
    message(&mut requests, &PreviewRequest::Timeline(env(2)));
    message(
        &mut requests,
        &PreviewRequest::ScaledFrame(ScaledFrameRequest {
            envelope: env(3),
            frame_index: 0,
            seek_serial: 9,
            scale: 0.5,
        }),
    );
    message(
        &mut requests,
        &PreviewRequest::PrepareAudio(PrepareAudioRequest {
            envelope: env(4),
            output_sample_rate: 48000,
        }),
    );
    message(&mut requests, &PreviewRequest::Shutdown(env(5)));
    let control = Arc::new(Mutex::new(Vec::new()));
    let bulk = Arc::new(Mutex::new(Vec::new()));
    let transport = WorkerTransport::new(
        Cursor::new(requests),
        SharedWriter(control.clone()),
        SharedWriter(bulk.clone()),
    );
    let mut config = PreviewWorkerConfig::new(identity, "sdk-v2", "1.1.0");
    config.cache_directory = temp.path().into();
    serve_preview_worker(
        &TestVideo(0.1),
        &RenderOptions::default(),
        transport,
        config,
    )
    .unwrap();
    assert!(!control.lock().is_empty());
    assert!(
        !bulk.lock().is_empty(),
        "tagged frame record must be written"
    );
    assert_eq!(
        std::fs::read_dir(temp.path()).unwrap().count(),
        0,
        "prepared PCM must be removed on shutdown"
    );
}

struct RunningWorker {
    control: TcpStream,
    bulk: TcpStream,
    task: Option<std::thread::JoinHandle<()>>,
    identity: PreviewIdentity,
}
impl RunningWorker {
    fn new(seconds: f32, cache: &std::path::Path) -> Self {
        fn pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            client.set_read_timeout(Some(Wait::from_secs(5))).unwrap();
            (client, listener.accept().unwrap().0)
        }
        let (control, input) = pair();
        let (bulk, output) = pair();
        let identity = PreviewIdentity {
            project_id: "probe".into(),
            open_session: "test".into(),
            source_revision: "immutable".into(),
            worker_generation: 11,
        };
        let mut config = PreviewWorkerConfig::new(identity.clone(), "sdk-v2", "1.1.0");
        config.cache_directory = cache.into();
        let task = std::thread::spawn(move || {
            let transport = WorkerTransport::new(input.try_clone().unwrap(), input, output);
            let _ = serve_preview_worker(
                &TestVideo(seconds),
                &RenderOptions::default(),
                transport,
                config,
            );
        });
        Self {
            control,
            bulk,
            task: Some(task),
            identity,
        }
    }
    fn envelope(&self, request_id: u64) -> PreviewEnvelope {
        PreviewEnvelope {
            contract_version: PREVIEW_CONTRACT_VERSION,
            identity: self.identity.clone(),
            request_id,
        }
    }
    fn send(&mut self, request: PreviewRequest) {
        let mut bytes = Vec::new();
        message(&mut bytes, &request);
        self.control.write_all(&bytes).unwrap();
    }
    fn receive<T: serde::de::DeserializeOwned>(stream: &mut TcpStream) -> T {
        let mut length = [0; 4];
        stream.read_exact(&mut length).unwrap();
        let length = u32::from_be_bytes(length) as usize;
        assert!(length <= 1024 * 1024);
        let mut bytes = vec![0; length];
        stream.read_exact(&mut bytes).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
    fn reply(&mut self) -> PreviewResponse {
        Self::receive(&mut self.control)
    }
    fn exchange(&mut self, request: PreviewRequest) -> PreviewResponse {
        self.send(request);
        self.reply()
    }
    fn hello(&mut self, id: u64) {
        assert!(matches!(
            self.exchange(PreviewRequest::Hello(PreviewHelloRequest {
                offered_versions: vec![1],
                required_capabilities: vec![],
                request_id: id
            })),
            PreviewResponse::Hello(_)
        ));
    }
    fn prepare(&mut self, id: u64) -> PreparedAudioDescriptor {
        match self.exchange(PreviewRequest::PrepareAudio(PrepareAudioRequest {
            envelope: self.envelope(id),
            output_sample_rate: 48000,
        })) {
            PreviewResponse::PreparedAudio(a) => a,
            other => panic!("{other:?}"),
        }
    }
}
impl Drop for RunningWorker {
    fn drop(&mut self) {
        let _ = self.control.shutdown(std::net::Shutdown::Both);
        let _ = self.bulk.shutdown(std::net::Shutdown::Both);
        if let Some(t) = self.task.take() {
            t.join().unwrap();
        }
    }
}

#[test]
fn live_preparation_exact_samples_bounded_reads_retention_and_release() {
    use sha2::{Digest, Sha256};
    let cache = tempfile::tempdir().unwrap();
    let mut worker = RunningWorker::new(0.1, cache.path());
    worker.hello(1);
    let a = worker.prepare(2);
    // One frame at 10 fps is exactly 4800 samples, not ceil(f32(0.1) * 48000).
    assert_eq!(
        (a.sample_count, a.byte_count, a.silent),
        (4800, 38400, true)
    );
    assert_eq!(a.sha256, format!("{:x}", Sha256::digest(vec![0; 38400])));
    let _b = worker.prepare(3);
    assert!(
        matches!(worker.exchange(PreviewRequest::PrepareAudio(PrepareAudioRequest {
        envelope: worker.envelope(4), output_sample_rate: 48000 })), PreviewResponse::Error(e) if e.code == "AUDIO_CACHE_FULL")
    );
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 2);
    let r = worker.exchange(PreviewRequest::ReadAudio(ReadAudioRequest {
        envelope: worker.envelope(5),
        artifact_id: a.artifact_id.clone(),
        offset: 38384,
        length: 16,
    }));
    let PreviewResponse::AudioRead(read) = r else {
        panic!("{r:?}")
    };
    let record: BinaryRecordHeader = RunningWorker::receive(&mut worker.bulk);
    assert_eq!(record, read.record);
    assert_eq!(record.offset, 38384);
    let mut pcm = [1; 16];
    worker.bulk.read_exact(&mut pcm).unwrap();
    assert_eq!(pcm, [0; 16]);
    assert!(matches!(
        worker.exchange(PreviewRequest::ReadAudio(ReadAudioRequest {
            envelope: worker.envelope(6),
            artifact_id: a.artifact_id.clone(),
            offset: 0,
            length: MAX_AUDIO_READ_BYTES + 1
        })),
        PreviewResponse::Error(_)
    ));
    for (id, released) in [(7, true), (8, false)] {
        assert!(
            matches!(worker.exchange(PreviewRequest::ReleaseAudio(ArtifactRequest {
            envelope: worker.envelope(id), artifact_id: a.artifact_id.clone() })), PreviewResponse::Ack(r) if r.released == released)
        );
    }
    worker.prepare(9);
    assert!(matches!(
        worker.exchange(PreviewRequest::Shutdown(worker.envelope(10))),
        PreviewResponse::Ack(_)
    ));
    drop(worker);
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn hello_request_order_and_identity_are_required_before_work() {
    let cache = tempfile::tempdir().unwrap();
    let mut worker = RunningWorker::new(0.1, cache.path());
    assert!(
        matches!(worker.exchange(PreviewRequest::Timeline(worker.envelope(1))), PreviewResponse::Error(e) if e.code == "HELLO_REQUIRED")
    );
    worker.hello(2);
    assert!(
        matches!(worker.exchange(PreviewRequest::Timeline(worker.envelope(2))), PreviewResponse::Error(e) if e.code == "STALE_REQUEST")
    );
    let mut wrong = worker.envelope(3);
    wrong.identity.open_session = "other-session".into();
    assert!(
        matches!(worker.exchange(PreviewRequest::Timeline(wrong)), PreviewResponse::Error(e) if e.code == "INVALID_ENVELOPE")
    );
    assert!(matches!(
        worker.exchange(PreviewRequest::Timeline(worker.envelope(4))),
        PreviewResponse::Timeline(_)
    ));
}

#[test]
fn cancellation_interrupts_inflight_mix_and_removes_partial_artifact() {
    let cache = tempfile::tempdir().unwrap();
    let mut worker = RunningWorker::new(3600., cache.path());
    worker.hello(1);
    worker.send(PreviewRequest::PrepareAudio(PrepareAudioRequest {
        envelope: worker.envelope(2),
        output_sample_rate: 48000,
    }));
    let started = Instant::now();
    while std::fs::read_dir(cache.path()).unwrap().count() == 0 {
        assert!(started.elapsed() < Wait::from_secs(2));
        std::thread::sleep(Wait::from_millis(1));
    }
    worker.send(PreviewRequest::CancelAudio(ArtifactRequest {
        envelope: worker.envelope(3),
        artifact_id: String::new(),
    }));
    assert!(matches!(worker.reply(), PreviewResponse::Error(e) if e.message.contains("cancelled")));
    assert!(matches!(worker.reply(), PreviewResponse::Ack(_)));
    assert!(started.elapsed() < Wait::from_secs(2));
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}
