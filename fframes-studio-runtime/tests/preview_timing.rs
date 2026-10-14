use fframes::{Duration, Frame, RenderOptions, Svgr, Video};
use fframes_studio_protocol::*;
use fframes_studio_runtime::{PreviewWorkerConfig, WorkerTransport, serve_preview_worker};
use parking_lot::Mutex;
use serde::Serialize;
use std::io::{self, Cursor, Write};
use std::sync::Arc;

#[derive(Debug)]
struct KeyedScene(&'static str);
impl fframes::Scene for KeyedScene {
    fn duration(&self) -> Duration<'_> {
        Duration::Frames(10)
    }
    fn editor_instance_key(&self) -> Option<&str> {
        Some(self.0)
    }
    fn render_frame<'a>(&'a self, _: Frame, _: &fframes::FFramesContext<'a, '_>) -> Svgr<'a> {
        Svgr::empty()
    }
}
struct ScenesVideo([KeyedScene; 3]);
impl Video for ScenesVideo {
    const FPS: usize = 10;
    const WIDTH: usize = 32;
    const HEIGHT: usize = 18;
    fn duration(&self) -> Duration<'_> {
        Duration::Auto
    }
    fn audio(&self) -> fframes::AudioMap<'_> {
        fframes::AudioMap::none()
    }
    fn define_scenes(&self) -> fframes::Scenes<'_> {
        let scenes: Vec<&dyn fframes::Scene> = self.0.iter().map(|s| s as _).collect();
        fframes::Scenes::from(scenes)
    }
    fn render_frame<'a>(&'a self, frame: Frame, ctx: &fframes::FFramesContext<'a, '_>) -> Svgr<'a> {
        fframes::svgr!(<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 18">{ctx.render_scenes(&frame)}</svg>)
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
fn responses(control: &[u8]) -> Vec<PreviewResponse> {
    let mut out = Vec::new();
    let mut rest = control;
    while rest.len() >= 4 {
        let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        out.push(serde_json::from_slice(&rest[4..4 + len]).unwrap());
        rest = &rest[4 + len..];
    }
    out
}

#[test]
fn set_timing_is_offered_in_hello_and_changes_the_next_timeline() {
    let identity = PreviewIdentity {
        project_id: "project".into(),
        open_session: "session".into(),
        source_revision: "rev".into(),
        worker_generation: 1,
    };
    let env = |request_id| PreviewEnvelope {
        contract_version: PREVIEW_CONTRACT_VERSION,
        identity: identity.clone(),
        request_id,
    };
    let set = |request_id, json: Option<&str>| {
        PreviewRequest::SetTiming(SetTimingRequest {
            envelope: env(request_id),
            timing_json: json.map(str::to_owned),
        })
    };
    let mut requests = Vec::new();
    message(
        &mut requests,
        &PreviewRequest::Hello(PreviewHelloRequest {
            offered_versions: vec![PREVIEW_CONTRACT_VERSION],
            required_capabilities: vec![SET_TIMING_CAPABILITY.into()],
            request_id: 1,
        }),
    );
    message(&mut requests, &PreviewRequest::Timeline(env(2)));
    let split = r#"{"version":1,"clips":[
        {"key":"c","trim_start_frames":0,"trim_end_frames":0},
        {"key":"a","trim_start_frames":0,"trim_end_frames":6},
        {"key":"gone","trim_start_frames":0,"trim_end_frames":0},
        {"key":"a","trim_start_frames":4,"trim_end_frames":0}]}"#;
    message(&mut requests, &set(3, Some(split)));
    message(&mut requests, &PreviewRequest::Timeline(env(4)));
    // Rejected as a whole; the split layout stays.
    message(
        &mut requests,
        &set(5, Some(r#"{"version":1,"clips":[],"extra":1}"#)),
    );
    message(&mut requests, &PreviewRequest::Timeline(env(6)));
    message(&mut requests, &set(7, None));
    message(&mut requests, &PreviewRequest::Timeline(env(8)));
    message(&mut requests, &PreviewRequest::Shutdown(env(9)));

    let temp = tempfile::tempdir().unwrap();
    let control = Arc::new(Mutex::new(Vec::new()));
    let transport = WorkerTransport::new(
        Cursor::new(requests),
        SharedWriter(control.clone()),
        SharedWriter(Arc::new(Mutex::new(Vec::new()))),
    );
    let mut config = PreviewWorkerConfig::new(identity.clone(), "sdk-v2", "1.1.0");
    config.cache_directory = temp.path().into();
    let video = ScenesVideo([KeyedScene("a"), KeyedScene("b"), KeyedScene("c")]);
    serve_preview_worker(&video, &RenderOptions::default(), transport, config).unwrap();

    let responses = responses(&control.lock());
    let PreviewResponse::Hello(hello) = &responses[0] else {
        panic!("{:?}", responses[0])
    };
    assert!(
        hello
            .capabilities
            .iter()
            .any(|c| c == SET_TIMING_CAPABILITY)
    );
    let layout = |response: &PreviewResponse| {
        let PreviewResponse::Timeline(timeline) = response else {
            panic!("{response:?}")
        };
        timeline
            .scenes
            .iter()
            .map(|s| {
                (
                    s.instance_id.clone(),
                    s.start_frame,
                    s.end_frame,
                    s.clip.as_ref().unwrap().trim_start_frames,
                )
            })
            .collect::<Vec<_>>()
    };
    let code_order = vec![
        ("a".to_owned(), 0, 10, 0),
        ("b".to_owned(), 10, 20, 0),
        ("c".to_owned(), 20, 30, 0),
    ];
    assert_eq!(layout(&responses[1]), code_order);
    let PreviewResponse::TimingApplied(applied) = &responses[2] else {
        panic!("{:?}", responses[2])
    };
    assert_eq!(applied.total_frames, 30);
    assert_eq!(applied.orphan_clips, ["gone"]);
    assert_eq!(applied.appended_scenes, ["b"]);
    let split_layout = vec![
        ("c".to_owned(), 0, 10, 0),
        ("a".to_owned(), 10, 14, 0),
        ("a#1".to_owned(), 14, 20, 4),
        ("b".to_owned(), 20, 30, 0),
    ];
    assert_eq!(layout(&responses[3]), split_layout);
    let PreviewResponse::Error(rejected) = &responses[4] else {
        panic!("{:?}", responses[4])
    };
    assert_eq!(rejected.code, "INVALID_TIMING");
    assert_eq!(layout(&responses[5]), split_layout);
    assert!(matches!(&responses[6], PreviewResponse::TimingApplied(a) if a.total_frames == 30));
    assert_eq!(layout(&responses[7]), code_order);
}

/// A worker built before `set_timing_v1` cannot parse the request, so clients gate it
/// on the Hello capability; without it they keep previewing in code order.
#[test]
fn workers_without_the_capability_cannot_parse_set_timing() {
    #[derive(serde::Deserialize, Debug)]
    #[serde(tag = "type")]
    #[allow(dead_code)]
    enum OldRequest {
        Hello(serde_json::Value),
        Timeline(serde_json::Value),
        Shutdown(serde_json::Value),
    }
    let request = PreviewRequest::SetTiming(SetTimingRequest {
        envelope: PreviewEnvelope {
            contract_version: PREVIEW_CONTRACT_VERSION,
            identity: PreviewIdentity {
                project_id: "p".into(),
                open_session: "s".into(),
                source_revision: "r".into(),
                worker_generation: 1,
            },
            request_id: 2,
        },
        timing_json: None,
    });
    let json = serde_json::to_string(&request).unwrap();
    assert!(serde_json::from_str::<OldRequest>(&json).is_err());
    assert!(!PREVIEW_CAPABILITIES.contains(&SET_TIMING_CAPABILITY));
}
