use crate::anchors::ElementRegistration;
use fframes::{CpuFrameRenderer, Previewer, RenderOptions, Video};
use fframes_studio_protocol::{
    AudioTrackInfo, CURRENT_PROTOCOL_VERSION, ElementMetadataResponse, ErrorMessage, FrameHeader,
    HelloResponse, MAX_FRAME_PAYLOAD_BYTES, RenderFrameResponse, SceneInfo, ShutdownResponse,
    TimelineResponse, WorkerRequest, WorkerResponse,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{self, Read, Write};
use std::time::Instant;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("json deserialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("protocol error: {0}")]
    Protocol(#[from] fframes_studio_protocol::ProtocolError),
    #[error("fframes renderer error: {0}")]
    Renderer(#[from] fframes::FFramesRendererError),
    #[error("worker connection closed")]
    ConnectionClosed,
    #[error("worker error: {0}")]
    Custom(String),
}
/// Maximum control message size to protect against unbounded allocations from malformed lengths.
pub const MAX_CONTROL_MESSAGE_SIZE: usize = 1024 * 1024; // 1 MiB

pub struct WorkerTransport {
    pub control_in: Box<dyn Read + Send>,
    pub control_out: Box<dyn Write + Send>,
    pub frame_out: Box<dyn Write + Send>,
}

impl WorkerTransport {
    pub fn new(
        control_in: impl Read + Send + 'static,
        control_out: impl Write + Send + 'static,
        frame_out: impl Write + Send + 'static,
    ) -> Self {
        Self {
            control_in: Box::new(control_in),
            control_out: Box::new(control_out),
            frame_out: Box::new(frame_out),
        }
    }

    /// Reads a 4-byte big-endian length prefixed JSON message from control stream.
    pub fn read_control_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>, WorkerError> {
        let mut len_bytes = [0u8; 4];
        match self.control_in.read_exact(&mut len_bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(WorkerError::Io(e)),
        }

        let len = u32::from_be_bytes(len_bytes) as usize;
        if len > MAX_CONTROL_MESSAGE_SIZE {
            return Err(WorkerError::Custom(format!(
                "control message length {len} exceeds limit of {MAX_CONTROL_MESSAGE_SIZE} bytes"
            )));
        }
        let mut buffer = vec![0u8; len];
        self.control_in.read_exact(&mut buffer)?;

        let msg: T = serde_json::from_slice(&buffer)?;
        Ok(Some(msg))
    }

    /// Writes a 4-byte big-endian length prefixed JSON message to control stream.
    pub fn write_control_message<T: Serialize>(&mut self, msg: &T) -> Result<(), WorkerError> {
        let json_bytes = serde_json::to_vec(msg)?;
        let len_bytes = (json_bytes.len() as u32).to_be_bytes();
        self.control_out.write_all(&len_bytes)?;
        self.control_out.write_all(&json_bytes)?;
        self.control_out.flush()?;
        Ok(())
    }

    /// Writes a raw binary frame payload to the frame pipe and flushes it.
    pub fn write_frame_payload(&mut self, payload: &[u8]) -> Result<(), WorkerError> {
        self.frame_out.write_all(payload)?;
        self.frame_out.flush()?;
        Ok(())
    }

    /// Writes an M2 tagged record: a bounded JSON header followed by its exact payload.
    pub fn write_binary_record(
        &mut self,
        header: &fframes_studio_protocol::BinaryRecordHeader,
        payload: &[u8],
    ) -> Result<(), WorkerError> {
        header
            .validate()
            .map_err(|error| WorkerError::Custom(error.to_string()))?;
        if header.payload_len != payload.len() {
            return Err(WorkerError::Custom(
                "binary record payload length mismatch".into(),
            ));
        }
        let encoded = serde_json::to_vec(header)?;
        if encoded.len() > MAX_CONTROL_MESSAGE_SIZE {
            return Err(WorkerError::Custom(
                "binary record header exceeds control bound".into(),
            ));
        }
        self.frame_out
            .write_all(&(encoded.len() as u32).to_be_bytes())?;
        self.frame_out.write_all(&encoded)?;
        self.frame_out.write_all(payload)?;
        self.frame_out.flush()?;
        Ok(())
    }
}

/// The borrowed worker serving loop.
/// The concrete `video`, `options`, and `Previewer` locals are stack-allocated
/// by the caller and borrowed here.
pub fn serve_worker<V: Video>(
    video: &V,
    options: &RenderOptions,
    registrations: &[ElementRegistration],
    mut transport: WorkerTransport,
    source_revision: &str,
    worker_generation: u64,
) -> Result<(), WorkerError> {
    let mut previewer = Previewer::new(video, options)?;
    let mut renderer = CpuFrameRenderer::default();

    loop {
        let req_opt: Option<WorkerRequest> = match transport.read_control_message() {
            Ok(Some(r)) => Some(r),
            Ok(None) => break, // EOF, graceful close
            Err(err) => return Err(err),
        };

        let Some(req) = req_opt else {
            break;
        };

        match req {
            WorkerRequest::Hello(_) => {
                let resp = WorkerResponse::Hello(HelloResponse {
                    protocol_version: CURRENT_PROTOCOL_VERSION,
                    project_id: "studio-worker".into(),
                    source_revision: source_revision.to_string(),
                    worker_generation,
                    fframes_version: "1.1.0".into(),
                    sdk_version: "1.0.0".into(),
                    runtime_version: "1.0.0".into(),
                    frame_contract_version: 1,
                    max_frame_bytes: MAX_FRAME_PAYLOAD_BYTES,
                    capabilities: vec![
                        "timeline".into(),
                        "frame".into(),
                        "element_metadata".into(),
                        "shutdown".into(),
                    ],
                });
                transport.write_control_message(&resp)?;
            }
            WorkerRequest::Timeline(req) => {
                let report = previewer.timeline_report();
                let scenes = report
                    .scenes
                    .iter()
                    .map(|s| SceneInfo {
                        id: format!("scene_{}", s.index),
                        name: s.name.clone(),
                        start_frame: s.start_frame,
                        frame_count: s.end_frame.saturating_sub(s.start_frame),
                        duration_seconds: (s.end_seconds - s.start_seconds) as f64,
                    })
                    .collect();

                let audio_tracks = report
                    .audio
                    .iter()
                    .map(|a| AudioTrackInfo {
                        name: a.file.clone(),
                        source_path: a.file.clone(),
                        start_second: a.start_seconds,
                        duration_seconds: a.end_seconds - a.start_seconds,
                    })
                    .collect();

                let resp = WorkerResponse::Timeline(TimelineResponse {
                    protocol_version: CURRENT_PROTOCOL_VERSION,
                    source_revision: source_revision.to_string(),
                    worker_generation,
                    request_id: req.request_id,
                    fps: report.fps as f64,
                    total_frames: report.duration_frames,
                    duration_seconds: report.duration_seconds as f64,
                    width: report.width as u32,
                    height: report.height as u32,
                    scenes,
                    audio_tracks,
                });
                transport.write_control_message(&resp)?;
            }
            WorkerRequest::RenderFrame(req) => {
                let start = Instant::now();
                match previewer.render(req.frame_index, &mut renderer) {
                    Ok(rgba_frame) => {
                        let render_duration_micros = start.elapsed().as_micros() as u64;
                        let header = FrameHeader::new_straight_rgba(
                            source_revision,
                            worker_generation,
                            req.request_id,
                            req.frame_index,
                            rgba_frame.width,
                            rgba_frame.height,
                        )?;

                        let resp = WorkerResponse::RenderFrame(RenderFrameResponse {
                            protocol_version: CURRENT_PROTOCOL_VERSION,
                            source_revision: source_revision.to_string(),
                            worker_generation,
                            request_id: req.request_id,
                            frame_index: req.frame_index,
                            render_duration_micros,
                            header,
                        });

                        transport.write_control_message(&resp)?;
                        transport.write_frame_payload(&rgba_frame.pixels)?;
                    }
                    Err(err) => {
                        let err_msg = WorkerResponse::Error(ErrorMessage {
                            protocol_version: CURRENT_PROTOCOL_VERSION,
                            source_revision: source_revision.to_string(),
                            worker_generation,
                            request_id: Some(req.request_id),
                            error_code: "RENDER_ERROR".into(),
                            message: err.to_string(),
                        });
                        transport.write_control_message(&err_msg)?;
                    }
                }
            }
            WorkerRequest::ElementMetadata(req) => {
                let elements = registrations
                    .iter()
                    .map(|r| r.to_protocol_metadata())
                    .collect();
                let resp = WorkerResponse::ElementMetadata(ElementMetadataResponse {
                    protocol_version: CURRENT_PROTOCOL_VERSION,
                    source_revision: source_revision.to_string(),
                    worker_generation,
                    request_id: req.request_id,
                    elements,
                });
                transport.write_control_message(&resp)?;
            }
            WorkerRequest::Shutdown(req) => {
                let resp = WorkerResponse::Shutdown(ShutdownResponse {
                    protocol_version: CURRENT_PROTOCOL_VERSION,
                    source_revision: source_revision.to_string(),
                    worker_generation,
                    request_id: req.request_id,
                    ok: true,
                });
                transport.write_control_message(&resp)?;
                break;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fframes::{Color, Duration, Frame, Svgr, Video};

    struct TestVideo;
    impl Video for TestVideo {
        const FPS: usize = 30;
        const WIDTH: usize = 320;
        const HEIGHT: usize = 180;
        const BACKGROUND_COLOR: Color = Color::BLACK;

        fn duration(&self) -> Duration<'_> {
            Duration::Seconds(1.0)
        }
        fn audio(&self) -> fframes::AudioMap<'_> {
            fframes::AudioMap::none()
        }

        fn render_frame<'a>(
            &'a self,
            _frame: Frame,
            _ctx: &fframes::FFramesContext<'a, '_>,
        ) -> Svgr<'a> {
            fframes::svgr!(
                <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 320 180">
                    <rect width="320" height="180" fill="#112233" />
                </svg>
            )
        }
    }

    #[test]
    fn test_worker_serving_loop_lifecycle() {
        use parking_lot::Mutex;
        use std::io::Cursor;
        use std::sync::Arc;

        // Prepare request stream: Hello -> Timeline -> RenderFrame(0) -> Shutdown
        let mut client_reqs = Vec::new();
        fn write_msg<T: Serialize>(buf: &mut Vec<u8>, msg: &T) {
            let json = serde_json::to_vec(msg).unwrap();
            buf.extend_from_slice(&(json.len() as u32).to_be_bytes());
            buf.extend_from_slice(&json);
        }

        write_msg(
            &mut client_reqs,
            &WorkerRequest::Hello(fframes_studio_protocol::HelloRequest {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                client_version: "test".into(),
            }),
        );
        write_msg(
            &mut client_reqs,
            &WorkerRequest::Timeline(fframes_studio_protocol::TimelineRequest {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                source_revision: "rev1".into(),
                worker_generation: 1,
                request_id: 1,
            }),
        );
        write_msg(
            &mut client_reqs,
            &WorkerRequest::RenderFrame(fframes_studio_protocol::RenderFrameRequest {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                source_revision: "rev1".into(),
                worker_generation: 1,
                request_id: 2,
                frame_index: 0,
            }),
        );
        write_msg(
            &mut client_reqs,
            &WorkerRequest::Shutdown(fframes_studio_protocol::ShutdownRequest {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                source_revision: "rev1".into(),
                worker_generation: 1,
                request_id: 3,
            }),
        );

        let control_in = Cursor::new(client_reqs);
        let control_out = Arc::new(Mutex::new(Vec::new()));
        let frame_out = Arc::new(Mutex::new(Vec::new()));

        struct SharedWriter(Arc<Mutex<Vec<u8>>>);
        impl Write for SharedWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().write(buf)
            }
            fn flush(&mut self) -> io::Result<()> {
                self.0.lock().flush()
            }
        }

        let transport = WorkerTransport::new(
            control_in,
            SharedWriter(Arc::clone(&control_out)),
            SharedWriter(Arc::clone(&frame_out)),
        );

        let video = TestVideo;
        let options = RenderOptions::default();

        serve_worker(&video, &options, &[], transport, "rev1", 1)
            .expect("serving loop runs and terminates cleanly");

        // Verify that control messages were written back
        let ctrl_bytes = control_out.lock().clone();
        assert!(!ctrl_bytes.is_empty());

        // Verify that frame output received pixels
        let frame_bytes = frame_out.lock().clone();
        assert_eq!(frame_bytes.len(), 320 * 180 * 4); // 320x180 RGBA
    }
}
