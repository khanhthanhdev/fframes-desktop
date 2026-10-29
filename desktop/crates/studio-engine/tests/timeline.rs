use fframes_studio_protocol::*;
use studio_engine::PlaybackClock;
use studio_engine::timeline::*;

fn report(total_frames: usize) -> PreviewTimelineResponse {
    PreviewTimelineResponse {
        timing_generation: 0,
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
        clip: None,
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

    let mut viewport = TimelineTransform::default();
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
    let mut viewport = TimelineTransform::default();
    viewport.resize(100., &model);
    assert_eq!(viewport.geometry(&model).scene_lane_count, 2);
    // One second across the width, then pan to the last one.
    viewport.zoom(10., 0., &model);
    viewport.scroll_by(900., &model);
    assert_eq!(viewport.position, 9.);
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
    let mut viewport = TimelineTransform::default();
    viewport.resize(1_000.0, &model);
    assert_eq!(viewport.geometry(&model).audio[0].x, 12.5);
    assert_eq!(viewport.geometry(&model).audio[0].width, 125.0);
}

#[test]
fn zoom_keeps_pointer_anchor_and_scroll_and_resize_are_bounded() {
    let model = TimelineModel::new(report(1_000)).unwrap();
    let mut viewport = TimelineTransform::default();
    viewport.resize(400.0, &model);
    viewport.zoom(25.0, 0.0, &model);
    viewport.scroll_by(250.0, &model);
    let before = viewport.frame_at_x(123.0, &model);
    viewport.zoom(2.0, 123.0, &model);
    assert_eq!(viewport.frame_at_x(123.0, &model), before);
    viewport.scroll_by(f64::MAX, &model);
    assert!(viewport.scroll_x().is_finite());
    assert_eq!(viewport.position, 100.0 - viewport.zoom);
    viewport.resize(1_000_000.0, &model);
    assert!(viewport.pixels_per_second() <= MAX_PIXELS_PER_SECOND);
    assert!((0.0..=100.0 - viewport.zoom).contains(&viewport.position));

    let snapshot = viewport.clone();
    viewport.zoom(f64::NAN, 5.0, &model);
    viewport.zoom(2.0, f64::INFINITY, &model);
    viewport.update_zoom(f64::NAN, 1.0, &model);
    viewport.set_position(f64::NAN, &model);
    viewport.resize(f64::INFINITY, &model);
    viewport.scroll_by(f64::NAN, &model);
    assert_eq!(snapshot, viewport);
}

#[test]
fn fit_shows_the_whole_video_and_zoom_steps_exactly_double() {
    // 3 s at 10 fps.
    let model = TimelineModel::new(report(30)).unwrap();
    let mut viewport = TimelineTransform::default();
    viewport.resize(1_000.0, &model);
    assert_eq!((viewport.zoom, viewport.position), (3.0, 0.0));
    let fit = viewport.pixels_per_second();
    viewport.zoom(2.0, 0.0, &model);
    assert_eq!(viewport.pixels_per_second(), fit * 2.0);
    viewport.zoom(2.0, 0.0, &model);
    assert_eq!(viewport.pixels_per_second(), fit * 4.0);
    // Never wider than the whole video.
    viewport.zoom(0.001, 0.0, &model);
    assert_eq!(viewport.pixels_per_second(), fit);
    viewport.zoom(4.0, 1_000.0, &model);
    assert_eq!(viewport.position, 3.0 - viewport.zoom);
    viewport.fit(&model);
    assert_eq!(
        (viewport.pixels_per_second(), viewport.position),
        (fit, 0.0)
    );
    // A resize keeps the visible seconds (Cap) and re-clamps the position.
    viewport.zoom(2.0, 1_000.0, &model);
    viewport.resize(500.0, &model);
    assert_eq!((viewport.zoom, viewport.position), (1.5, 1.5));
    // Never closer than the maximum pixels per second.
    viewport.zoom(f64::MAX, 0.0, &model);
    assert!((viewport.pixels_per_second() - MAX_PIXELS_PER_SECOND).abs() < 1e-6);
}

#[test]
fn update_zoom_keeps_the_origin_at_its_fraction_of_the_viewport() {
    let model = TimelineModel::new(report(1_000)).unwrap();
    let mut viewport = TimelineTransform::default();
    viewport.resize(1_000.0, &model);
    viewport.update_zoom(10.0, 0.0, &model);
    viewport.set_position(40.0, &model);
    // 42.5 s is a quarter into [40, 50).
    viewport.update_zoom(4.0, 42.5, &model);
    assert_eq!((viewport.zoom, viewport.position), (4.0, 41.5));
    assert_eq!(viewport.x_at_seconds(42.5), 250.0);
    assert_eq!(viewport.seconds_at_x(250.0), 42.5);
}

#[test]
fn segments_outside_the_padded_viewport_are_culled() {
    let mut value = report(300);
    value.scenes = (0..30)
        .map(|index| scene(&format!("clip-{index}"), index, index * 10, index * 10 + 10))
        .collect();
    let model = TimelineModel::new(value).unwrap();
    let mut viewport = TimelineTransform::default();
    viewport.resize(1_000.0, &model);
    assert_eq!(viewport.geometry(&model).scenes.len(), 30);
    // [10 s, 13 s) plus two seconds of padding either side touches clips 7..=15.
    viewport.update_zoom(3.0, 0.0, &model);
    viewport.set_position(10.0, &model);
    let painted: Vec<_> = viewport
        .geometry(&model)
        .scenes
        .iter()
        .map(|rect| rect.index)
        .collect();
    assert_eq!(painted, (7..=15).collect::<Vec<_>>());
    assert!(viewport.segment_visible(14.0, 15.0));
    assert!(!viewport.segment_visible(15.5, 16.0));
    assert!(!viewport.segment_visible(0.0, 7.5));
}

#[test]
fn ruler_marks_follow_caps_resolution_ladder_and_label_whole_seconds() {
    let model = TimelineModel::new(report(36_000)).unwrap();
    let mut viewport = TimelineTransform::default();
    viewport.resize(1_000.0, &model);
    for (zoom, resolution) in [
        (3_600.0, 300.0),
        (100.0, 5.0),
        (40.0, 2.5),
        (20.0, 1.0),
        (10.0, 0.5),
        (3.0, 0.25),
        (2.0, 0.1),
        (0.2, 0.01),
    ] {
        viewport.update_zoom(zoom, 0.0, &model);
        assert_eq!(viewport.marking_resolution(), resolution, "zoom {zoom}");
        let ticks = viewport.geometry(&model).ticks;
        assert!(ticks.len() <= 21, "zoom {zoom}: {}", ticks.len());
        for tick in &ticks {
            assert_eq!(tick.major, tick.label.is_some());
        }
    }
    viewport.update_zoom(40.0, 0.0, &model);
    let labels: Vec<_> = viewport
        .geometry(&model)
        .ticks
        .into_iter()
        .filter_map(|tick| tick.label)
        .take(3)
        .collect();
    assert_eq!(labels, ["0:00", "0:05", "0:10"]);
    viewport.update_zoom(2.0, 0.0, &model);
    let labels: Vec<_> = viewport
        .geometry(&model)
        .ticks
        .into_iter()
        .filter_map(|tick| tick.label)
        .collect();
    assert_eq!(labels, ["0:00", "0:00.5", "0:01", "0:01.5", "0:02"]);
    // The parity fixture's Fit: quarter seconds, labelled on whole ones.
    viewport.update_zoom(3.0, 0.0, &model);
    let ticks = viewport.geometry(&model).ticks;
    let labels: Vec<_> = ticks.iter().filter_map(|tick| tick.label.clone()).collect();
    assert_eq!(labels, ["0:00", "0:01", "0:02", "0:03"]);
    // Every mark is drawn once: minors never sit on a major.
    let mut xs: Vec<_> = ticks.iter().map(|tick| tick.x).collect();
    xs.dedup();
    assert_eq!(xs.len(), ticks.len());
    assert_eq!(ticks.len(), 13);
    assert_eq!(format_ruler_time(75.0, 1.0), "1:15");
    assert_eq!(format_ruler_time(59.999_999_9, 1.0), "1:00");
    assert_eq!(format_ruler_time(61.25, 0.25), "1:01.25");
}

#[test]
fn geometry_and_thumbnails_are_visible_and_strictly_bounded() {
    let mut value = report(10_000);
    value.scenes = vec![scene("offscreen", 0, 0, 2), scene("visible", 1, 500, 700)];
    let model = TimelineModel::new(value).unwrap();
    let mut viewport = TimelineTransform::default();
    viewport.resize(300.0, &model);
    viewport.update_zoom(3.0, 0.0, &model);
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
    let mut viewport = TimelineTransform::default();
    viewport.resize(f64::MAX, &one);
    viewport.zoom(f64::MAX, f64::MAX, &one);
    let geometry = viewport.geometry(&one);
    assert!(geometry.width.is_finite());
    assert!(viewport.pixels_per_second() <= MAX_PIXELS_PER_SECOND);
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
fn play_until_stops_exactly_at_the_bound_and_holds() {
    let mut t = PlaybackClock::install(30, 120, 10, 0.).unwrap();
    t.set_play_until(Some(40));
    t.toggle(0.).unwrap();
    t.fallback(0.).unwrap();
    assert_eq!(t.tick(0.5), 25);
    assert!(t.playing());
    // Far past the bound: exactly at it, and stopped.
    assert_eq!(t.tick(10.), 40);
    assert!(!t.playing());
    assert_eq!(t.next_frame_in(11.), None);
    // A later tick does not move it.
    assert_eq!(t.tick(20.), 40);
    assert_eq!(t.position(), 40);
}

#[test]
fn toggle_at_the_bound_plays_on_instead_of_restarting() {
    let mut t = PlaybackClock::install(30, 120, 0, 0.).unwrap();
    t.set_play_until(Some(40));
    t.toggle(0.).unwrap();
    t.fallback(0.).unwrap();
    assert_eq!(t.tick(5.), 40);
    t.toggle(5.).unwrap();
    assert!(t.playing());
    assert_eq!(t.position(), 40, "a held stop is not the video's end");
    assert_eq!(t.play_until(), None);
    // A new bound ahead stops the next play there.
    t.pause(5.).unwrap();
    t.set_play_until(Some(70));
    t.toggle(5.).unwrap();
    t.fallback(5.).unwrap();
    assert_eq!(t.tick(9.), 70);
    assert!(!t.playing());
    // Only the video's end restarts from frame 0.
    t.toggle(9.).unwrap();
    t.fallback(9.).unwrap();
    assert_eq!(t.tick(100.), 120);
    t.toggle(100.).unwrap();
    assert_eq!(t.position(), 0);
}

#[test]
fn seek_and_reinstall_clear_play_until() {
    let mut t = PlaybackClock::install(30, 120, 0, 0.).unwrap();
    t.set_play_until(Some(40));
    t.seek(10, 0.).unwrap();
    assert_eq!(t.play_until(), None);
    assert_eq!(t.end_frame(), 120);
    t.set_play_until(Some(40));
    t.reinstall(30, 90, 10, 1.).unwrap();
    assert_eq!(t.play_until(), None);
    t.toggle(1.).unwrap();
    t.fallback(1.).unwrap();
    assert_eq!(t.tick(2.), 40);
    assert!(t.playing(), "the cleared bound no longer stops playback");
    // A bound past the end stops at the end.
    t.set_play_until(Some(500));
    assert_eq!(t.end_frame(), 90);
}

#[test]
fn huge_thumbnail_interpolation_does_not_overflow() {
    let mut value = report(usize::MAX);
    value.fps = 1 << 40;
    value.duration_seconds = value.total_frames as f32 / value.fps as f32;
    let model = TimelineModel::new(value).unwrap();
    let mut viewport = TimelineTransform::default();
    viewport.resize(1_000_000., &model);
    viewport.fit(&model);
    let samples = viewport.thumbnail_frames(&model);
    assert_eq!(samples.len(), 12);
    assert_eq!(samples[0], 0);
    assert_eq!(*samples.last().unwrap(), usize::MAX - 1);
    assert!(samples.windows(2).all(|w| w[0] < w[1]));
}
