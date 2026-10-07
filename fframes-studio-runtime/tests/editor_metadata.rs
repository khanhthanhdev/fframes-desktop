use fframes::{
    AudioMap, Color, Duration, EditorObjectKey, FFramesContext, Frame, RenderOptions, Svgr, Video,
};
use fframes_studio_protocol::*;
use fframes_studio_runtime::{PreviewWorkerConfig, WorkerTransport, serve_preview_worker};
use parking_lot::Mutex;
use serde::Serialize;
use std::io::{self, Cursor, Read, Write};
use std::sync::Arc;

struct AnnotatedVideo;

impl Video for AnnotatedVideo {
    const FPS: usize = 30;
    const WIDTH: usize = 320;
    const HEIGHT: usize = 180;
    const BACKGROUND_COLOR: Color = Color::BLACK;

    fn duration(&self) -> Duration<'_> {
        Duration::Frames(1)
    }

    fn audio(&self) -> AudioMap<'_> {
        AudioMap::none()
    }

    fn render_frame<'a>(&'a self, _: Frame, _: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let object_id = EditorObjectKey::new("intro", "title-card", "headline", "main")
            .expect("valid identity")
            .with_style_tokens(["color.text", "typography.title"])
            .expect("style bindings do not require a source anchor")
            .render_id();
        fframes::svgr!(
            <svg xmlns="http://www.w3.org/2000/svg" width="160" height="90" viewBox="0 0 160 90">
                <g id={object_id} transform="translate(20 10)">
                    <rect width="30" height="10" fill="#ff0000" />
                </g>
            </svg>
        )
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

fn write_message<T: Serialize>(stream: &mut Vec<u8>, value: &T) {
    let bytes = serde_json::to_vec(value).expect("serializable request");
    stream.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    stream.extend_from_slice(&bytes);
}

fn read_message<T: serde::de::DeserializeOwned>(stream: &mut Cursor<Vec<u8>>) -> T {
    let mut length = [0; 4];
    stream.read_exact(&mut length).expect("response length");
    let length = u32::from_be_bytes(length) as usize;
    assert!(length <= 1024 * 1024);
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).expect("response body");
    serde_json::from_slice(&bytes).expect("valid response JSON")
}

#[test]
fn active_preview_binds_transformed_video_pixel_geometry_to_exact_frame_and_seek() {
    let temp = tempfile::tempdir().expect("temporary cache");
    let identity = PreviewIdentity {
        project_id: "project".into(),
        open_session: "session".into(),
        source_revision: "a".repeat(64),
        worker_generation: 7,
    };
    let envelope = |request_id| PreviewEnvelope {
        contract_version: PREVIEW_CONTRACT_VERSION,
        identity: identity.clone(),
        request_id,
    };
    let mut input = Vec::new();
    write_message(
        &mut input,
        &PreviewRequest::Hello(PreviewHelloRequest {
            offered_versions: vec![PREVIEW_CONTRACT_VERSION],
            required_capabilities: PREVIEW_CAPABILITIES
                .iter()
                .chain(OPTIONAL_PREVIEW_CAPABILITIES)
                .map(|capability| (*capability).into())
                .collect(),
            request_id: 1,
        }),
    );
    write_message(&mut input, &PreviewRequest::Timeline(envelope(2)));
    write_message(
        &mut input,
        &PreviewRequest::ScaledFrame(ScaledFrameRequest {
            envelope: envelope(3),
            frame_index: 0,
            seek_serial: 19,
            scale: 0.5,
        }),
    );
    write_message(&mut input, &PreviewRequest::Shutdown(envelope(4)));

    let control = Arc::new(Mutex::new(Vec::new()));
    let bulk = Arc::new(Mutex::new(Vec::new()));
    let transport = WorkerTransport::new(
        Cursor::new(input),
        SharedWriter(control.clone()),
        SharedWriter(bulk.clone()),
    );
    let mut config = PreviewWorkerConfig::new(identity.clone(), "managed-sdk", "1.1.0");
    config.cache_directory = temp.path().into();
    serve_preview_worker(
        &AnnotatedVideo,
        &RenderOptions::default(),
        transport,
        config,
    )
    .expect("serve preview requests");

    let mut output = Cursor::new(control.lock().clone());
    let PreviewResponse::Hello(hello) = read_message(&mut output) else {
        panic!("first response must negotiate capabilities");
    };
    assert!(
        hello
            .capabilities
            .iter()
            .any(|capability| capability == "editor_frame_v1")
    );
    assert!(matches!(
        read_message::<PreviewResponse>(&mut output),
        PreviewResponse::Timeline(_)
    ));
    let PreviewResponse::ScaledFrame(response) = read_message(&mut output) else {
        panic!("scaled frame response expected");
    };
    assert_eq!((response.header.width, response.header.height), (160, 90));
    let metadata = response
        .editor_metadata
        .expect("metadata travels with frame");
    assert_eq!(metadata.frame_index, 0);
    assert_eq!(metadata.seek_serial, 19);
    assert_eq!((metadata.video_width, metadata.video_height), (320, 180));
    assert_eq!(metadata.status, EditorFrameStatus::Supported);
    metadata
        .validate_for_frame(0, 19)
        .expect("validated metadata");
    assert_eq!(metadata.objects.len(), 1);
    let object = &metadata.objects[0];
    assert_eq!(object.identity.scene_instance_key, "intro");
    assert_eq!(object.identity.component_key, "title-card");
    assert_eq!(object.identity.object_key, "headline");
    assert_eq!(object.identity.repeat_key, "main");
    assert_eq!(object.source_anchor, None);
    assert_eq!(object.style_tokens, ["color.text", "typography.title"]);
    assert!((object.bounds.x - 40.0).abs() < 0.01);
    assert!((object.bounds.y - 20.0).abs() < 0.01);
    assert!((object.bounds.width - 60.0).abs() < 0.01);
    assert!((object.bounds.height - 20.0).abs() < 0.01);
    assert!(matches!(
        read_message::<PreviewResponse>(&mut output),
        PreviewResponse::Ack(_)
    ));

    let mut bulk_output = Cursor::new(bulk.lock().clone());
    let record = read_message::<BinaryRecordHeader>(&mut bulk_output);
    assert_eq!(record.identity, identity);
    assert_eq!(record.request_id, 3);
    let mut pixels = vec![0; record.payload_len];
    bulk_output.read_exact(&mut pixels).expect("frame pixels");
    let red = (15 * 160 + 25) * 4;
    assert_eq!(&pixels[red..red + 4], &[255, 0, 0, 255]);
}
