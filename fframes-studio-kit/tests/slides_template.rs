//! Studio's Slide deck template: its `slides.json` stops are the ones its deck's
//! `reveal_times()` give (a stop once each bullet is in, and the end of every slide), so
//! the document never drifts from the sample deck. The deck renders through
//! `PropsStore::slide_view` with Studio's identities.

use std::sync::Arc;

use fframes::{Previewer, RenderOptions, Video};
use fframes_studio_kit::PropsStore;
use serde_json::{Value, json};

#[path = "../../desktop/crates/studio-project/templates-slides/src/video.rs"]
mod video;

use video::StudioVideo;

const SLIDES_JSON: &str =
    include_str!("../../desktop/crates/studio-project/templates-slides/slides.json");

/// The first scene frame at or after `seconds`: the bullet is fully in on it.
fn frame_at(seconds: f32) -> u64 {
    // Within a thousandth of a frame is on the frame (0.9 s × 30 is not exactly 27 in f32).
    (f64::from(seconds) * StudioVideo::FPS as f64 - 1e-3).ceil() as u64
}

#[test]
fn template_stops_follow_the_reveal_times() {
    let video = StudioVideo::new(Arc::new(PropsStore::default())).unwrap();
    let mut expected = Vec::new();
    for slide in video.deck().slides() {
        for time in slide.reveal_times() {
            expected.push(json!({ "scene": slide.key, "at": frame_at(time) }));
        }
        expected.push(json!({ "scene": slide.key, "at": "end" }));
    }
    let doc: Value = serde_json::from_str(SLIDES_JSON).unwrap();
    assert_eq!(doc["version"], 1);
    let stops: Vec<Value> = doc["stops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|stop| json!({ "scene": stop["scene"], "at": stop["at"] }))
        .collect();
    assert_eq!(
        stops, expected,
        "update templates-slides/slides.json to the deck's reveal times"
    );
    assert!(
        doc["stops"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stop| stop["notes"].is_string()),
        "the template shows speaker notes"
    );
}

#[test]
fn bullet_stops_come_before_the_next_slide_fades_in() {
    let video = StudioVideo::new(Arc::new(PropsStore::default())).unwrap();
    let fade = video.deck().transition_seconds();
    for slide in video.deck().slides() {
        let end = frame_at(slide.seconds() - fade);
        let mut last = 0;
        for time in slide.reveal_times() {
            let frame = frame_at(time);
            assert!(frame > last && frame < end, "{} at {frame}", slide.key);
            last = frame;
        }
    }
}

#[test]
fn the_deck_renders_with_studio_identities() {
    let store = Arc::new(PropsStore::default());
    store.listen();
    let video = StudioVideo::new(store.clone()).unwrap();
    let keys: Vec<&str> = video.deck().slides().map(|slide| slide.key).collect();
    assert_eq!(
        keys,
        ["intro", "agenda", "present", "outputs", "edit", "thanks"]
    );
    let options = RenderOptions::default();
    let mut previewer = Previewer::new(&video, &options).expect("previewer");
    // The first frame of the deck: the intro's title.
    let svg = previewer.svg(15).expect("frame");
    let title = fframes::EditorObjectKey::new("intro", "SlideText", "title", "main")
        .unwrap()
        .render_id();
    assert!(svg.contains(&title), "{svg}");
}

#[test]
fn scanning_the_real_template_finds_each_bullet_once_in_its_scene_time() {
    let store = Arc::new(PropsStore::default());
    store.listen();
    let video = StudioVideo::new(store.clone()).unwrap();
    let options = RenderOptions::default();
    let mut previewer = Previewer::new(&video, &options).unwrap();
    let total = previewer.timeline_report().duration_frames;
    for frame in (0..total).step_by(15) {
        previewer.inspect(frame).unwrap();
    }
    let snapshot = store.suggested_stops();
    assert!(!snapshot.truncated);
    let positions: Vec<_> = snapshot
        .stops
        .iter()
        .map(|s| (s.scene.as_str(), frame_at(s.seconds)))
        .collect();
    assert_eq!(
        positions,
        [
            ("agenda", 27),
            ("agenda", 45),
            ("agenda", 63),
            ("edit", 27),
            ("edit", 45),
            ("edit", 63),
        ],
        "scene keys and times must not inherit the video's scene offsets"
    );
    assert_eq!(
        snapshot.stops[0].label.as_deref(),
        Some("Every slide is a scene of the video")
    );
    previewer.inspect(total - 1).unwrap();
    assert_eq!(
        store.suggested_stops(),
        snapshot,
        "ordinary renders do not duplicate observations"
    );
}
