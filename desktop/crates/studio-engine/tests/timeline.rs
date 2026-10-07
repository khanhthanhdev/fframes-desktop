use fframes_studio_protocol::*;
use studio_engine::PlaybackClock;
use studio_engine::timeline::*;

fn report(total_frames: usize) -> PreviewTimelineResponse {
    PreviewTimelineResponse {
        envelope: PreviewEnvelope {
            contract_version: PREVIEW_CONTRACT_VERSION,
            identity: PreviewIdentity {
                project_id: "project".into(),
                open_session: "session".into(),
                source_revision: "a".repeat(64),
                worker_generation: 7,
            },
            request_id: 1,
        },
        fps: 10,
        width: 1920,
        height: 1080,
        total_frames,
        duration_seconds: total_frames as f32 / 10.0,
        scenes: Vec::new(),
        audio_tracks: Vec::new(),
    }
}

fn scene(id: &str, index: usize, start: usize, end: usize) -> PreviewSceneInfo {
    PreviewSceneInfo {
        instance_id: format!("revision-7:{id}"),
        editor_instance_key: None,
        index,
        name: "Repeated".into(),
        full_name: "video::Repeated".into(),
        start_frame: start,
        end_frame: end,
        start_seconds: start as f32 / 10.0,
        end_seconds: end as f32 / 10.0,
    }
}

fn audio(start: f64, end: f64) -> PreviewAudioTrackInfo {
    PreviewAudioTrackInfo {
        file: "sound.wav".into(),
        start_seconds: start,
        end_seconds: end,
        mix: TrackMixInfo {
            gain_db: 0.0,
            pan: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            offset: 0.0,
            voice: false,
            duck_under_voice: false,
        },
    }
}

#[test]
fn cursor_conversions_include_exact_end_and_round_to_nearest_frame() {
    let model = TimelineModel::new(report(10)).unwrap();
    assert_eq!(model.report().total_frames, 10);
    assert_eq!(model.frame_at_seconds(0.049), Some(0));
    assert_eq!(model.frame_at_seconds(0.051), Some(1));
    assert_eq!(model.frame_at_seconds(100.0), Some(10));
    assert_eq!(model.seconds(usize::MAX), 1.0);
    assert_eq!(model.time_label(10), "1.000s / frame 10");
    assert_eq!(model.frame_at_seconds(f64::NAN), None);

    let empty = TimelineModel::new(report(0)).unwrap();
    assert_eq!(empty.frame_at_seconds(0.0), None);
    assert_eq!(empty.seconds(1), 0.0);
}

#[test]
fn half_open_overlaps_repeat_names_and_pack_lanes_deterministically() {
    let mut value = report(30);
    value.scenes = vec![
        scene("first", 0, 0, 10),
        scene("overlap", 1, 5, 15),
        scene("repeat", 2, 10, 20),
        scene("empty", 3, 12, 12),
    ];
    let model = TimelineModel::new(value).unwrap();
    assert_eq!(model.scene_hits(5), vec![0, 1]);
    assert_eq!(model.scene_hits(10), vec![1, 2]);
    assert_eq!(model.scene_hits(15), vec![2]);
    assert!(!model.scene_hits(12).contains(&3));

    let mut viewport = TimelineViewport::default();
    viewport.resize(300.0, &model);
    viewport.fit(&model);
    let lanes: Vec<_> = viewport
        .geometry(&model)
        .scenes
        .iter()
        .map(|r| r.lane)
        .collect();
    assert_eq!(lanes, vec![0, 1, 0, 0]);
}

#[test]
fn horizontal_pan_keeps_audio_below_offscreen_scene_lanes() {
    let mut value = report(100);
    value.scenes = vec![
        scene("first", 0, 0, 20),
        scene("overlay", 1, 5, 15),
        scene("last", 2, 80, 100),
    ];
    value.audio_tracks = vec![audio(0., 10.)];
    let model = TimelineModel::new(value).unwrap();
    let mut viewport = TimelineViewport::default();
    viewport.resize(100., &model);
    assert_eq!(viewport.geometry(&model).scene_lane_count, 2);
    viewport.scroll_by(900., &model);
    let geometry = viewport.geometry(&model);
    assert_eq!(geometry.scenes.len(), 1);
    assert_eq!(geometry.scenes[0].lane, 0);
    assert_eq!(geometry.scene_lane_count, 2);
    assert_eq!(geometry.audio.len(), 1);
}

#[test]
fn selection_normalizes_clamps_and_cycles_every_overlap() {
    let mut value = report(30);
    value.scenes = vec![scene("a", 0, 0, 20), scene("b", 1, 5, 15)];
    let model = TimelineModel::new(value).unwrap();
    let mut selection = TimelineSelection::default();
    selection.select_range(25, 3, &model);
    assert_eq!(selection.range, Some(3..25));
    selection.cycle_scene_at(7, &model);
    assert_eq!(selection.scene_id.as_deref(), Some("revision-7:a"));
    assert_eq!(selection.range, Some(0..20));
    selection.cycle_scene_at(7, &model);
    assert_eq!(selection.scene_id.as_deref(), Some("revision-7:b"));
    selection.cycle_scene_at(7, &model);
    assert_eq!(selection.scene_id.as_deref(), Some("revision-7:a"));

    let shorter = TimelineModel::new(report(4)).unwrap();
    selection.clamp(&shorter);
    assert_eq!(selection.scene_id, None);
    assert_eq!(selection.range, Some(0..4));
    selection.select_range(4, 4, &shorter);
    assert_eq!(selection.range, Some(4..4));
}

#[test]
fn audio_geometry_preserves_fractional_seconds() {
    let mut value = report(100);
    value.audio_tracks = vec![audio(0.125, 1.375)];
    let model = TimelineModel::new(value).unwrap();
    let mut viewport = TimelineViewport::default();
    viewport.resize(1_000.0, &model);
    assert_eq!(viewport.geometry(&model).audio[0].x, 12.5);
    assert_eq!(viewport.geometry(&model).audio[0].width, 125.0);
}

#[test]
fn zoom_keeps_pointer_anchor_and_scroll_and_resize_are_bounded() {
    let model = TimelineModel::new(report(1_000)).unwrap();
    let mut viewport = TimelineViewport::default();
    viewport.resize(400.0, &model);
    viewport.scroll_by(250.0, &model);
    let before = viewport.frame_at_x(123.0, &model);
    viewport.zoom(2.0, 123.0, &model);
    assert_eq!(viewport.frame_at_x(123.0, &model), before);
    viewport.scroll_by(f64::MAX, &model);
    assert!(viewport.scroll_x.is_finite());
    viewport.resize(1_000_000.0, &model);
    assert_eq!(viewport.scroll_x, 0.0);

    let snapshot = (
        viewport.width,
        viewport.pixels_per_second,
        viewport.scroll_x,
    );
    viewport.zoom(f64::NAN, 5.0, &model);
    viewport.resize(f64::INFINITY, &model);
    viewport.scroll_by(f64::NAN, &model);
    assert_eq!(
        snapshot,
        (
            viewport.width,
            viewport.pixels_per_second,
            viewport.scroll_x
        )
    );
}

#[test]
fn geometry_and_thumbnails_are_visible_and_strictly_bounded() {
    let mut value = report(10_000);
    value.scenes = vec![scene("offscreen", 0, 0, 2), scene("visible", 1, 500, 700)];
    let model = TimelineModel::new(value).unwrap();
    let mut viewport = TimelineViewport::default();
    viewport.resize(300.0, &model);
    viewport.scroll_by(5_000.0, &model);
    let geometry = viewport.geometry(&model);
    assert_eq!(
        geometry.scenes.iter().map(|r| r.index).collect::<Vec<_>>(),
        vec![1]
    );
    assert!(geometry.ticks.len() <= 2_048);
    let thumbnails = viewport.thumbnail_frames(&model);
    assert!(!thumbnails.is_empty());
    assert!(thumbnails.len() <= 12);
    assert!(thumbnails.iter().all(|frame| *frame < 10_000));
}

#[test]
fn invalid_reports_are_rejected_without_overflow_or_nonfinite_geometry() {
    let mut invalid = report(10);
    invalid.fps = 0;
    assert!(TimelineModel::new(invalid).is_err());

    let mut invalid = report(10);
    invalid.duration_seconds = f32::INFINITY;
    assert!(TimelineModel::new(invalid).is_err());

    let mut invalid = report(10);
    invalid.scenes = vec![scene("duplicate", 0, 0, 2), scene("duplicate", 1, 2, 4)];
    assert!(TimelineModel::new(invalid).is_err());

    let mut invalid = report(10);
    invalid.audio_tracks = vec![audio(0.0, f64::INFINITY)];
    assert!(TimelineModel::new(invalid).is_err());

    let one = TimelineModel::new(report(1)).unwrap();
    let mut viewport = TimelineViewport::default();
    viewport.resize(f64::MAX, &one);
    viewport.zoom(f64::MAX, f64::MAX, &one);
    let geometry = viewport.geometry(&one);
    assert!(geometry.width.is_finite());
    assert!(geometry.ticks.len() <= 2_048);
}

#[test]
fn transport_pause_seek_end_replay_and_shorter_rebuild() {
    let mut t = PlaybackClock::install(30, 120, 17, 10.).unwrap();
    t.toggle(10.).unwrap();
    t.fallback(10.).unwrap();
    assert_eq!(t.tick(10.101), 20);
    t.pause(10.2).unwrap();
    let frozen = t.position();
    assert_eq!(t.tick(90.), frozen);
    t.seek(81, 90.).unwrap();
    assert!(!t.playing());
    t.toggle(90.).unwrap();
    t.fallback(90.).unwrap();
    assert_eq!(t.tick(90.11), 84);
    t.seek(11, 91.).unwrap();
    t.fallback(91.).unwrap();
    assert!(t.playing());
    assert_eq!(t.tick(91.21), 17);
    t.reinstall(24, 13, 17, 92.).unwrap();
    assert_eq!(t.position(), 13);
    assert!(!t.playing());
    t.toggle(93.).unwrap();
    t.fallback(93.).unwrap();
    assert_eq!(t.position(), 0);
    assert!(t.playing());
    assert_eq!(t.tick(100.), 13);
    assert!(!t.playing());
    t.reinstall(30, 0, 0, 101.).unwrap();
    t.toggle(101.).unwrap();
    assert!(!t.playing());
    assert_eq!(t.tick(f64::INFINITY), 0);
}

#[test]
fn huge_thumbnail_interpolation_does_not_overflow() {
    let mut value = report(usize::MAX);
    value.fps = 1 << 40;
    value.duration_seconds = value.total_frames as f32 / value.fps as f32;
    let model = TimelineModel::new(value).unwrap();
    let mut viewport = TimelineViewport::default();
    viewport.resize(1_000_000., &model);
    viewport.fit(&model);
    let samples = viewport.thumbnail_frames(&model);
    assert_eq!(samples.len(), 12);
    assert_eq!(samples[0], 0);
    assert_eq!(*samples.last().unwrap(), usize::MAX - 1);
    assert!(samples.windows(2).all(|w| w[0] < w[1]));
}
