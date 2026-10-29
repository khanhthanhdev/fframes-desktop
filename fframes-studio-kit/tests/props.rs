//! A kit component marked editable in a real render: the document changes what it draws,
//! Studio's render id names it, and a listening store records its schema and values.

use std::sync::{Arc, Mutex};

use fframes::{EditorObjectKey, Previewer, RenderOptions};
use fframes_kit::prelude::*;
use fframes_kit::props::{PropsSource, SuggestedStop};
use fframes_studio_kit::{PropsStore, REPEAT_KEY, VIDEO_SCENE_KEY};
use fframes_studio_protocol::{EditorObjectIdentity, PropsDocument};
use serde_json::json;

#[test]
fn stop_reports_replace_origins_and_stay_bounded_before_serialization() {
    let store = Arc::new(PropsStore::default());
    let stop = |seconds| SuggestedStop {
        seconds,
        label: Some("Bullet".into()),
        notes: None,
    };
    store.view().suggest_stops("list", &[stop(1.0)]);
    assert!(store.suggested_stops().stops.is_empty(), "not listening");
    store.listen();
    let view = store.view();
    view.suggest_stops("list", &[stop(1.0), stop(2.0)]);
    view.suggest_stops("other-list", &[stop(3.0)]);
    view.suggest_stops("list", &[stop(1.5)]);
    let snapshot = store.suggested_stops();
    assert_eq!(
        snapshot.stops.len(),
        2,
        "repeated renders replace, independent lists remain"
    );
    assert_eq!(snapshot.stops[0].seconds, 1.5);
    assert_eq!(snapshot.stops[1].seconds, 3.0);
    let hooks = store.hooks();
    assert_eq!(hooks.suggested_stops(), snapshot);
    hooks.clear_suggestions();

    let mut input = vec![
        stop(f32::NAN),
        stop(-1.0),
        SuggestedStop {
            notes: Some("x".repeat(10 * 1024)),
            ..stop(0.5)
        },
    ];
    input.extend((0..1000).map(|i| stop(i as f32)));
    view.suggest_stops("large", &input);
    assert_eq!(store.suggested_stops().stops.len(), 64);
    for i in 0..1000 {
        view.suggest_stops(&format!("origin-{i}"), &[stop(0.75)]);
    }
    let snapshot = store.suggested_stops();
    assert_eq!(snapshot.stops.len(), 300);
    assert!(snapshot.truncated);
    assert!(
        snapshot
            .stops
            .iter()
            .all(|s| s.seconds.is_finite() && s.seconds >= 0.0)
    );
    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(encoded.len() < 1024 * 1024);
    let decoded: fframes_studio_protocol::StopSuggestions = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, snapshot);
    hooks.clear_suggestions();
    assert_eq!(
        store.suggested_stops(),
        fframes_studio_protocol::StopSuggestions::default()
    );
}

#[derive(Debug)]
struct Probe {
    theme: Theme,
    store: Arc<PropsStore>,
    drawn: Mutex<String>,
}

impl Video for Probe {
    const FPS: usize = 30;
    const WIDTH: usize = 640;
    const HEIGHT: usize = 360;

    fn duration(&self) -> Duration<'_> {
        Duration::Seconds(1.0)
    }

    fn audio(&self) -> AudioMap<'_> {
        AudioMap::none()
    }

    fn render_frame<'a>(&'a self, frame: Frame, ctx: &FFramesContext<'a, '_>) -> Svgr<'a> {
        let mut ui = Ui::new(frame, ctx, &self.theme).with_props(self.store.view());
        let tree = ui!(<Circle editable="dot" cx={320.0} cy={180.0} radius={40.0} />);
        *self.drawn.lock().unwrap() = tree.to_string();
        svgr!(<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 360" width="640" height="360">{tree}</svg>)
    }
}

fn drawn(probe: &Probe) -> String {
    let options = RenderOptions::default();
    let mut previewer = Previewer::new(probe, &options).expect("previewer");
    previewer.svg(10).expect("frame");
    probe.drawn.lock().unwrap().clone()
}

#[test]
fn a_document_changes_what_editable_components_draw() {
    let store = Arc::new(PropsStore::default());
    store.listen();
    let probe = Probe {
        theme: Theme::default(),
        store: store.clone(),
        drawn: Mutex::new(String::new()),
    };
    let before = drawn(&probe);

    let dot = EditorObjectIdentity {
        scene_instance_key: VIDEO_SCENE_KEY.into(),
        component_key: "Circle".into(),
        object_key: "dot".into(),
        repeat_key: REPEAT_KEY.into(),
    };
    let id = EditorObjectKey::new("video", "Circle", "dot", REPEAT_KEY)
        .unwrap()
        .render_id();
    assert!(before.contains(&id), "{before}");

    let info = store.info(&dot).expect("recorded while listening");
    assert_eq!(info.code_values["radius"], 40.0);
    assert_eq!(info.schema.component, "Circle");
    assert!(info.schema.fields.iter().any(|f| f.name == "radius"));

    let mut doc = PropsDocument::default();
    doc.set(&dot, "stroke", Some(json!("#ff0000")));
    store.set(Some(doc));
    let after = drawn(&probe);
    assert!(after.contains("rgb(255, 0, 0)"), "{after}");
    let info = store.info(&dot).unwrap();
    assert_eq!(info.code_values["stroke"], "#4f8cff");
    assert_eq!(info.values["stroke"], "#ff0000");

    store.set(None);
    assert_eq!(drawn(&probe), before);
}
