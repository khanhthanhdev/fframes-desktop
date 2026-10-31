//! A kit component marked editable in a real render: the document changes what it draws,
//! Studio's render id names it, and a listening store records its schema and values.

use std::sync::{Arc, Mutex};

use fframes::{EditorObjectKey, Previewer, RenderOptions};
use fframes_kit::prelude::*;
use fframes_studio_kit::{PropsStore, REPEAT_KEY, VIDEO_SCENE_KEY};
use fframes_studio_protocol::{EditorObjectIdentity, PropsDocument};
use serde_json::json;

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
