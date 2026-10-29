use fframes::{Duration, Frame, RenderOptions, Svgr, Video};
use fframes_studio_protocol::*;
use fframes_studio_runtime::{
    PreviewWorkerConfig, PropsHooks, WorkerTransport, serve_preview_worker,
};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{self, Cursor, Write};
use std::sync::Arc;

/// Draws a bar whose width a props document can change, and records the width it drew.
struct PropsVideo {
    doc: Arc<Mutex<Option<PropsDocument>>>,
    drawn: Mutex<Vec<f64>>,
}

fn bar() -> EditorObjectIdentity {
    EditorObjectIdentity {
        scene_instance_key: "video".into(),
        component_key: "Bar".into(),
        object_key: "bar".into(),
        repeat_key: "main".into(),
    }
}

fn width(doc: Option<&PropsDocument>) -> f64 {
    doc.and_then(|doc| doc.values(&bar()))
        .and_then(|values| values.get("width"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(10.0)
}

impl Video for PropsVideo {
    const FPS: usize = 10;
    const WIDTH: usize = 32;
    const HEIGHT: usize = 18;
    fn duration(&self) -> Duration<'_> {
        Duration::Frames(10)
    }
    fn audio(&self) -> fframes::AudioMap<'_> {
        fframes::AudioMap::none()
    }
    fn render_frame<'a>(&'a self, _: Frame, _: &fframes::FFramesContext<'a, '_>) -> Svgr<'a> {
        let width = width(self.doc.lock().as_ref());
        self.drawn.lock().push(width);
        fframes::svgr!(<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 18"><rect width={width} height="4" fill="#fff" /></svg>)
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

fn identity() -> PreviewIdentity {
    PreviewIdentity {
        project_id: "project".into(),
        open_session: "session".into(),
        source_revision: "rev".into(),
        worker_generation: 1,
    }
}
fn env(request_id: u64) -> PreviewEnvelope {
    PreviewEnvelope {
        contract_version: PREVIEW_CONTRACT_VERSION,
        identity: identity(),
        request_id,
    }
}
fn hello(required: &[&str]) -> PreviewRequest {
    PreviewRequest::Hello(PreviewHelloRequest {
        offered_versions: vec![PREVIEW_CONTRACT_VERSION],
        required_capabilities: required.iter().map(|c| (*c).into()).collect(),
        request_id: 1,
    })
}
fn frame(request_id: u64) -> PreviewRequest {
    PreviewRequest::ScaledFrame(ScaledFrameRequest {
        envelope: env(request_id),
        frame_index: 0,
        seek_serial: request_id,
        scale: 1.0,
    })
}
fn set(request_id: u64, doc: Option<&str>) -> PreviewRequest {
    PreviewRequest::SetProps(SetPropsRequest {
        envelope: env(request_id),
        props_generation: request_id * 10,
        props_json: doc.map(str::to_owned),
    })
}

/// Runs the worker over `requests`; `hooks` wires the video's document in when set.
fn serve(
    requests: Vec<u8>,
    hooks: bool,
    configure: impl FnOnce(PreviewWorkerConfig) -> PreviewWorkerConfig,
) -> (Vec<PreviewResponse>, Vec<f64>) {
    let video = PropsVideo {
        doc: Arc::new(Mutex::new(None)),
        drawn: Mutex::new(Vec::new()),
    };
    let temp = tempfile::tempdir().unwrap();
    let control = Arc::new(Mutex::new(Vec::new()));
    let transport = WorkerTransport::new(
        Cursor::new(requests),
        SharedWriter(control.clone()),
        SharedWriter(Arc::new(Mutex::new(Vec::new()))),
    );
    let mut config = PreviewWorkerConfig::new(identity(), "sdk-v2", "1.1.0");
    config.cache_directory = temp.path().into();
    if hooks {
        let apply = video.doc.clone();
        let read = video.doc.clone();
        config = config.with_props(PropsHooks::new(
            move |doc| *apply.lock() = doc,
            move |identity| {
                (identity == &bar()).then(|| PropsInfo {
                    schema: PropsSchema {
                        component: "Bar".into(),
                        fields: vec![PropField {
                            name: "width".into(),
                            kind: PropKind::Number,
                            min: Some(0.0),
                            max: Some(32.0),
                            step: None,
                            unit: Some("px".into()),
                        }],
                    },
                    code_values: BTreeMap::from([("width".into(), 10.0.into())]),
                    values: BTreeMap::from([("width".into(), width(read.lock().as_ref()).into())]),
                })
            },
        ));
    }
    let config = configure(config);
    serve_preview_worker(&video, &RenderOptions::default(), transport, config).unwrap();
    let drawn = video.drawn.lock().clone();
    (responses(&control.lock()), drawn)
}

fn doc(width: f64) -> String {
    let mut doc = PropsDocument::default();
    doc.set(&bar(), "width", Some(width.into()));
    doc.to_json()
}

#[test]
fn set_props_is_offered_with_hooks_and_changes_the_next_frame() {
    let mut requests = Vec::new();
    message(&mut requests, &hello(&[SET_PROPS_CAPABILITY]));
    message(&mut requests, &frame(2));
    message(&mut requests, &set(3, Some(&doc(24.0))));
    message(&mut requests, &frame(4));
    message(
        &mut requests,
        &PreviewRequest::PropsInfo(PropsInfoRequest {
            envelope: env(5),
            identity: bar(),
        }),
    );
    // rejected as a whole: the previous document and generation stay
    message(
        &mut requests,
        &set(6, Some(r#"{"version":1,"objects":[],"extra":1}"#)),
    );
    message(&mut requests, &frame(7));
    message(&mut requests, &set(8, None));
    message(&mut requests, &frame(9));
    message(&mut requests, &PreviewRequest::Shutdown(env(10)));

    let (responses, drawn) = serve(requests, true, |config| config);
    let PreviewResponse::Hello(hello) = &responses[0] else {
        panic!("{:?}", responses[0])
    };
    assert!(hello.capabilities.iter().any(|c| c == SET_PROPS_CAPABILITY));
    assert!(
        matches!(&responses[2], PreviewResponse::PropsApplied(r) if r.props_generation == 30),
        "{:?}",
        responses[2]
    );
    let PreviewResponse::PropsInfo(info) = &responses[4] else {
        panic!("{:?}", responses[4])
    };
    assert_eq!(info.props_generation, 30);
    let info = info.info.as_ref().expect("the bar is editable");
    assert_eq!(info.values["width"], 24.0);
    assert_eq!(info.code_values["width"], 10.0);
    assert!(
        matches!(&responses[5], PreviewResponse::Error(e) if e.code == "INVALID_PROPS"),
        "{:?}",
        responses[5]
    );
    assert!(matches!(&responses[7], PreviewResponse::PropsApplied(r) if r.props_generation == 80));
    assert_eq!(drawn, vec![10.0, 24.0, 24.0, 10.0]);
}

#[test]
fn startup_props_apply_before_the_first_frame() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("props.json");
    std::fs::write(&path, doc(16.0)).unwrap();
    let args: Vec<String> = [
        "--props-file",
        path.to_str().unwrap(),
        "--props-generation",
        "4",
    ]
    .map(str::to_owned)
    .into();
    let mut requests = Vec::new();
    message(&mut requests, &hello(&[]));
    message(&mut requests, &frame(2));
    message(
        &mut requests,
        &PreviewRequest::PropsInfo(PropsInfoRequest {
            envelope: env(3),
            identity: bar(),
        }),
    );
    message(&mut requests, &PreviewRequest::Shutdown(env(4)));
    let (responses, drawn) = serve(requests, true, |config| config.with_props_args(&args));
    assert_eq!(drawn, vec![16.0]);
    assert!(matches!(&responses[2], PreviewResponse::PropsInfo(r) if r.props_generation == 4));
}

#[test]
fn workers_without_hooks_neither_offer_nor_accept_props() {
    let mut requests = Vec::new();
    message(&mut requests, &hello(&[SET_PROPS_CAPABILITY]));
    message(
        &mut requests,
        &PreviewRequest::Hello(PreviewHelloRequest {
            offered_versions: vec![PREVIEW_CONTRACT_VERSION],
            required_capabilities: Vec::new(),
            request_id: 2,
        }),
    );
    message(&mut requests, &set(3, Some(&doc(24.0))));
    message(&mut requests, &frame(4));
    message(&mut requests, &PreviewRequest::Shutdown(env(5)));

    let (responses, drawn) = serve(requests, false, |config| config);
    assert!(
        matches!(&responses[0], PreviewResponse::Error(e) if e.code == "INCOMPATIBLE_PREVIEW_BRIDGE"),
        "{:?}",
        responses[0]
    );
    let PreviewResponse::Hello(hello) = &responses[1] else {
        panic!("{:?}", responses[1])
    };
    assert!(!hello.capabilities.iter().any(|c| c == SET_PROPS_CAPABILITY));
    assert!(
        matches!(&responses[2], PreviewResponse::Error(e) if e.code == "PROPS_UNSUPPORTED"),
        "{:?}",
        responses[2]
    );
    assert_eq!(drawn, vec![10.0]);
}

#[test]
fn stop_scans_bound_project_reports_and_reject_bad_batches_without_losing_preview() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut requests = Vec::new();
    message(&mut requests, &hello(&[SUGGESTED_STOPS_CAPABILITY]));
    let scan = |id, frames, generation, reset| {
        PreviewRequest::SuggestStops(SuggestStopsRequest {
            envelope: env(id),
            frames,
            timing_generation: generation,
            reset,
        })
    };
    message(&mut requests, &scan(2, vec![1, 3], 0, true));
    message(
        &mut requests,
        &scan(3, vec![0; MAX_SUGGEST_FRAMES + 1], 0, false),
    );
    message(&mut requests, &scan(4, vec![0], 7, false));
    message(&mut requests, &scan(5, vec![10], 0, false));
    message(&mut requests, &scan(6, vec![5], 0, false));
    message(&mut requests, &frame(7));
    message(&mut requests, &scan(8, vec![6], 0, true));
    message(&mut requests, &PreviewRequest::Shutdown(env(9)));

    let cleared = Arc::new(AtomicUsize::new(0));
    let clear = cleared.clone();
    let reads = AtomicUsize::new(0);
    let stop = |seconds| SuggestedSlideStop {
        scene: "intro".into(),
        origin: "bullets".into(),
        seconds,
        label: Some("Next bullet".into()),
        notes: None,
    };
    let (responses, drawn) = serve(requests, false, |config| {
        config.with_props(PropsHooks::new(|_| {}, |_| None).with_suggestions(
            move || {
                let stops = match reads.fetch_add(1, Ordering::Relaxed) {
                    0 => vec![
                        stop(0.5),
                        stop(f32::NAN),
                        stop(-1.0),
                        SuggestedSlideStop {
                            notes: Some("x".repeat(10 * 1024)),
                            ..stop(1.0)
                        },
                    ],
                    // All fields fit individually, but JSON escaping exceeds the wire cap.
                    1 => (0..300)
                        .map(|i| SuggestedSlideStop {
                            origin: format!("item-{i}"),
                            notes: Some("\0".repeat(8192)),
                            ..stop(0.7)
                        })
                        .collect(),
                    _ => vec![stop(0.8)],
                };
                StopSuggestions {
                    stops,
                    truncated: false,
                }
            },
            move || {
                clear.fetch_add(1, Ordering::Relaxed);
            },
        ))
    });
    assert!(
        matches!(&responses[0], PreviewResponse::Hello(h) if h.capabilities.iter().any(|c| c == SUGGESTED_STOPS_CAPABILITY))
    );
    let PreviewResponse::SuggestedStops(first) = &responses[1] else {
        panic!("{:?}", responses[1]);
    };
    assert_eq!(first.suggestions.stops.len(), 1);
    assert_eq!(first.suggestions.stops[0].seconds, 0.5);
    assert!(first.suggestions.truncated);
    for response in &responses[2..5] {
        assert!(
            matches!(response, PreviewResponse::Error(e) if e.code == "INVALID_SUGGEST_REQUEST"),
            "{response:?}"
        );
    }
    assert!(
        matches!(&responses[5], PreviewResponse::Error(e) if e.code == "SUGGESTIONS_TOO_LARGE")
    );
    assert!(matches!(&responses[6], PreviewResponse::ScaledFrame(_)));
    assert!(
        matches!(&responses[7], PreviewResponse::SuggestedStops(r) if r.suggestions.stops[0].seconds == 0.8)
    );
    assert_eq!(cleared.load(Ordering::Relaxed), 2);
    assert_eq!(drawn.len(), 5, "only valid scan frames and the seek render");
}
