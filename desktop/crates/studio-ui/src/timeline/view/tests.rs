use super::*;
use crate::timeline::model::tests::report;

#[test]
fn ruler_click_scrubs_and_lane_click_selects() {
    let ruler_h = METRICS.ruler_h;
    assert_eq!(ruler_h, 26.);
    assert_eq!(hit_zone(100., 100.), HitZone::Ruler);
    assert_eq!(hit_zone(100. + ruler_h - 0.5, 100.), HitZone::Ruler);
    assert_eq!(hit_zone(100. + ruler_h, 100.), HitZone::Lanes);
    assert_eq!(hit_zone(300., 100.), HitZone::Lanes);
}

#[test]
fn zoom_slider_runs_from_fit_to_the_closest_zoom() {
    let fit = 25.;
    assert_eq!(zoom_to_fraction(fit, fit), 0.);
    assert_eq!(zoom_to_fraction(ZOOM_MAX, fit), 1.);
    assert_eq!(zoom_to_fraction(fit / 10., fit), 0.);
    assert_eq!(zoom_to_fraction(ZOOM_MAX * 10., fit), 1.);
    assert_eq!(zoom_to_fraction(10., ZOOM_MAX), 0.);
    assert_eq!(fraction_to_zoom(-1., fit), fit);
    for pps in [30., 100., 4_000.] {
        let back = fraction_to_zoom(zoom_to_fraction(pps, fit), fit);
        assert!((back / pps - 1.).abs() < 1e-3, "{pps} -> {back}");
    }
    assert!(fraction_to_zoom(0.25, fit) < fraction_to_zoom(0.75, fit));
}

#[gpui::test]
fn the_controls_turn_on_with_a_timeline_and_follow_the_zoom(cx: &mut gpui::TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let reported = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let _subscription = cx.update(|_, cx| {
        let reported = reported.clone();
        cx.subscribe(&view, move |_, event, _| {
            if let TimelineEvent::Controls(controls) = event {
                reported.borrow_mut().push(*controls);
            }
        })
    });
    view.update(cx, |view, cx| {
        view.install(thirty_clips(), Arc::from([]), 0, cx)
    });
    cx.run_until_parked();
    let fitted = *reported.borrow().last().expect("the controls are reported");
    assert!(fitted.enabled, "{fitted:?}");
    assert_eq!(fitted.zoom_fraction, 0.);
    view.update(cx, |view, cx| view.zoom(2., cx));
    cx.run_until_parked();
    let zoomed = *reported.borrow().last().unwrap();
    assert!(zoomed.zoom_fraction > 0., "{zoomed:?}");
}

/// 30 one-second clips at 10 fps.
fn thirty_clips() -> TimelineModel {
    let keys: Vec<_> = (0..30).map(|index| format!("clip-{index}")).collect();
    let scenes: Vec<_> = keys
        .iter()
        .enumerate()
        .map(|(index, key)| (key.as_str(), index * 10, index * 10 + 10))
        .collect();
    TimelineModel::new(report(10, &scenes)).unwrap()
}

#[gpui::test]
fn only_segments_near_the_view_are_painted(cx: &mut gpui::TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    view.update(cx, |view, cx| {
        view.install(thirty_clips(), Arc::from([]), 0, cx)
    });
    cx.run_until_parked();
    assert_eq!(view.read_with(cx, |view, _| view.painted_segments), 30);
    view.update(cx, |view, cx| {
        let model = view.model.clone().unwrap();
        view.viewport.update_zoom(3., 0., &model);
        view.viewport.set_position(10., &model);
        cx.notify();
    });
    cx.run_until_parked();
    // [10 s, 13 s) and two seconds of padding either side: clips 7 to 15.
    let metrics = view.read_with(cx, |view, _| view.qualification_metrics());
    assert_eq!(metrics["painted_segments"], 9, "{metrics:#}");
    assert_eq!(metrics["clips"], 30);
}

#[gpui::test]
fn the_engine_snapshot_drives_the_clip_track(cx: &mut gpui::TestAppContext) {
    use crate::timeline::model::tests::snapshot;
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    view.update(cx, |view, cx| {
        view.install(thirty_clips(), Arc::from([]), 0, cx)
    });
    cx.run_until_parked();
    // The document plays two clips; until the report catches up the second has no scene.
    view.update(cx, |view, cx| {
        view.set_snapshot(
            Some(snapshot(&[("clip-3", 10, 0, 0), ("clip-3", 10, 5, 0)])),
            cx,
        )
    });
    cx.run_until_parked();
    let (clips, painted) = view.read_with(cx, |view, _| {
        let spans: Vec<_> = view
            .clips
            .iter()
            .map(|c| (c.start, c.end, c.scene))
            .collect();
        (spans, view.painted_segments)
    });
    // The first clip keeps its scene's frames, the second follows it, and the 29
    // scenes no clip plays stay on their rows.
    assert_eq!(clips[..2], [(30, 40, Some(3)), (40, 45, None)]);
    assert_eq!(clips.len(), 31);
    assert!(!clips[2..].iter().any(|(_, _, scene)| *scene == Some(3)));
    assert_eq!(painted, 31);
    view.update(cx, |view, cx| view.set_snapshot(None, cx));
    assert_eq!(view.read_with(cx, |view, _| view.clips.len()), 30);
}

#[gpui::test]
fn a_minimap_press_centres_the_view_and_a_fitted_one_ignores_it(cx: &mut gpui::TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    view.update(cx, |view, cx| {
        view.install(thirty_clips(), Arc::from([]), 0, cx)
    });
    cx.run_until_parked();
    let centre = |cx: &mut gpui::VisualTestContext| {
        let [left, top, width, height] = view.read_with(cx, |view, _| view.qualification_metrics())
            ["minimap_bounds"]
            .as_array()
            .expect("the minimap reports where it is")
            .iter()
            .map(|value| value.as_f64().unwrap() as f32)
            .collect::<Vec<_>>()[..]
        else {
            panic!("four numbers")
        };
        gpui::point(px(left + width / 2.), px(top + height / 2.))
    };
    // Fitted: the minimap is hidden and the press changes nothing.
    let at = centre(cx);
    cx.simulate_click(at, gpui::Modifiers::none());
    assert_eq!(view.read_with(cx, |view, _| view.viewport.position), 0.);

    view.update(cx, |view, cx| {
        let model = view.model.clone().unwrap();
        view.viewport.update_zoom(3., 0., &model);
        cx.notify();
    });
    cx.run_until_parked();
    // The bar's middle is 15 s; the 3 s view is centred on it.
    let at = centre(cx);
    cx.simulate_click(at, gpui::Modifiers::none());
    let position = view.read_with(cx, |view, _| view.viewport.position);
    assert!((position - 13.5).abs() < 1e-3, "{position}");
}

/// Three three-second clips at 10 fps, editable, with every edit the view asks of the
/// engine collected.
fn editable(
    cx: &mut gpui::TestAppContext,
) -> (
    gpui::Entity<TimelineView>,
    &mut gpui::VisualTestContext,
    std::rc::Rc<std::cell::RefCell<Vec<TimelineCommand>>>,
) {
    use crate::timeline::model::tests::snapshot;
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let edits = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = edits.clone();
    cx.update(|_, app| {
        app.subscribe(&view, move |_, event: &TimelineEvent, _| {
            if let TimelineEvent::Edit(command) = event {
                log.borrow_mut().push(command.clone());
            }
        })
        .detach();
    });
    view.update(cx, |view, cx| {
        let model =
            TimelineModel::new(report(10, &[("a", 0, 30), ("b", 30, 60), ("c", 60, 90)])).unwrap();
        view.install(model, Arc::from([]), 0, cx);
        view.set_snapshot(
            Some(snapshot(&[
                ("a", 30, 0, 0),
                ("b", 30, 0, 0),
                ("c", 30, 0, 0),
            ])),
            cx,
        );
    });
    cx.run_until_parked();
    // The first frame measures the track and fits it; the handles are laid out on the next.
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    (view, cx, edits)
}

/// The window bounds of clip `clip`'s box: left, top, width, height.
fn clip_bounds(
    view: &gpui::Entity<TimelineView>,
    cx: &mut gpui::VisualTestContext,
    clip: usize,
) -> [f32; 4] {
    let metrics = view.read_with(cx, |view, _| view.qualification_metrics());
    let found = metrics["edit"]["clips"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["clip"] == clip)
        .unwrap_or_else(|| panic!("clip {clip} in {metrics:#}"));
    let at = |i: usize| found["bounds"][i].as_f64().unwrap() as f32;
    [at(0), at(1), at(2), at(3)]
}

fn press(cx: &mut gpui::VisualTestContext, x: f32, y: f32) {
    // The first move paints the hover the press is tested against.
    for at in [x + 12., x] {
        cx.simulate_mouse_move(gpui::point(px(at), px(y)), None, gpui::Modifiers::none());
        cx.run_until_parked();
    }
    cx.simulate_mouse_down(
        gpui::point(px(x), px(y)),
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
}

#[gpui::test]
fn dragging_a_clip_edge_trims_in_one_gesture(cx: &mut gpui::TestAppContext) {
    let (view, cx, edits) = editable(cx);
    let [left, top, width, height] = clip_bounds(&view, cx, 1);
    let per_second = width / 3.;
    // A press on the end handle that never moves asks for nothing.
    press(cx, left + width - 2., top + height / 2.);
    assert!(view.read_with(cx, |view, _| view.edit_dragging()));
    view.update(cx, |view, cx| view.end_edit_drag(cx));
    assert!(edits.borrow().is_empty(), "{:?}", edits.borrow());

    // Pulling the end back by a second leaves 20 of the clip's 30 frames.
    press(cx, left + width - 2., top + height / 2.);
    view.update(cx, |view, cx| {
        view.edit_drag_move(left + width - 2. - per_second, true, cx);
        view.edit_drag_move(left + width - 2. - per_second, true, cx);
        view.end_edit_drag(cx);
    });
    assert_eq!(
        *edits.borrow(),
        [
            TimelineCommand::BeginGesture,
            TimelineCommand::Trim {
                clip: 1,
                edge: TrimEdge::End,
                frames: 10
            },
            TimelineCommand::EndGesture,
        ]
    );
    assert!(!view.read_with(cx, |view, _| view.edit_dragging()));
}

#[gpui::test]
fn the_end_edge_snaps_to_the_playhead_unless_shift_is_held(cx: &mut gpui::TestAppContext) {
    let (view, cx, edits) = editable(cx);
    view.update(cx, |view, _| view.set_position(70));
    cx.run_until_parked();
    let [left, top, width, height] = clip_bounds(&view, cx, 2);
    let per_second = width / 3.;
    // The end of the last clip, 9 s, dragged to 7.04 s: within 8 px of the playhead.
    press(cx, left + width - 2., top + height / 2.);
    let near = left + width - 2. - 1.96 * per_second;
    view.update(cx, |view, cx| view.edit_drag_move(near, false, cx));
    let snapped =
        view.read_with(cx, |view, _| view.qualification_metrics())["edit"]["snapped"].clone();
    assert_eq!(snapped, 7.0);
    view.update(cx, |view, cx| view.edit_drag_move(near, true, cx));
    let held =
        view.read_with(cx, |view, _| view.qualification_metrics())["edit"]["snapped"].clone();
    assert!(held.is_null(), "{held}");
    view.update(cx, |view, cx| view.end_edit_drag(cx));
    assert_eq!(edits.borrow().last(), Some(&TimelineCommand::EndGesture));
}

#[gpui::test]
fn dragging_a_clip_along_the_track_moves_it(cx: &mut gpui::TestAppContext) {
    let (view, cx, edits) = editable(cx);
    let [left, top, width, height] = clip_bounds(&view, cx, 0);
    let [last_left, _, last_width, _] = clip_bounds(&view, cx, 2);
    // Press in the first clip's middle, away from its handles.
    press(cx, left + width / 2., top + height / 2.);
    view.update(cx, |view, cx| {
        view.edit_drag_move(last_left + last_width - 4., false, cx)
    });
    let metrics = view.read_with(cx, |view, _| view.qualification_metrics());
    assert_eq!(
        (&metrics["edit"]["drag"], &metrics["edit"]["drop"]),
        (&"reorder".into(), &2.into())
    );
    view.update(cx, |view, cx| view.end_edit_drag(cx));
    assert_eq!(*edits.borrow(), [TimelineCommand::Move { clip: 0, to: 2 }]);
}

#[gpui::test]
fn a_rebuilt_preview_drops_a_reorder_but_closes_a_trim(cx: &mut gpui::TestAppContext) {
    let rebuilt = || {
        let mut report = report(10, &[("a", 0, 30), ("b", 30, 60), ("c", 60, 90)]);
        report.envelope.identity.source_revision = "b".repeat(64);
        TimelineModel::new(report).unwrap()
    };
    let (view, cx, edits) = editable(cx);
    let [left, top, width, height] = clip_bounds(&view, cx, 0);
    let [last_left, _, last_width, _] = clip_bounds(&view, cx, 2);
    press(cx, left + width / 2., top + height / 2.);
    view.update(cx, |view, cx| {
        view.edit_drag_move(last_left + last_width - 4., false, cx)
    });
    view.update(cx, |view, cx| view.install(rebuilt(), Arc::from([]), 0, cx));
    assert!(edits.borrow().is_empty(), "{:?}", edits.borrow());
    assert!(!view.read_with(cx, |view, _| view.edit_dragging()));

    let [left, top, width, height] = clip_bounds(&view, cx, 2);
    press(cx, left + width - 2., top + height / 2.);
    view.update(cx, |view, cx| {
        view.edit_drag_move(left + width - 20., false, cx)
    });
    view.update(cx, |view, cx| view.clear(cx));
    assert_eq!(edits.borrow().last(), Some(&TimelineCommand::EndGesture));
}

#[gpui::test]
fn the_playhead_clamps_to_a_shorter_timeline_and_runs_ahead_only_while_playing(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx, _) = editable(cx);
    view.update(cx, |view, _| view.set_position(40));
    view.read_with(cx, |view, _| {
        assert_eq!(view.playhead_frame(90, 30.), 40.);
    });
    view.update(cx, |view, _| {
        view.set_playing(true);
        view.anchor = Instant::now() - std::time::Duration::from_secs(5);
    });
    // A whole frame at most, however late the next one is.
    view.read_with(cx, |view, _| assert_eq!(view.playhead_frame(90, 30.), 41.));
    view.update(cx, |view, _| view.set_position(89));
    view.update(cx, |view, _| {
        view.anchor = Instant::now() - std::time::Duration::from_secs(5);
    });
    view.read_with(cx, |view, _| assert_eq!(view.playhead_frame(89, 30.), 89.));
    // Another timeline, shorter than where the playhead was.
    let shorter = TimelineModel::new(report(10, &[("a", 0, 30)])).unwrap();
    view.update(cx, |view, cx| view.install(shorter, Arc::from([]), 70, cx));
    view.read_with(cx, |view, _| assert_eq!(view.position, 30));
}

#[gpui::test]
fn split_cuts_the_clip_under_the_playhead(cx: &mut gpui::TestAppContext) {
    let (view, cx, edits) = editable(cx);
    view.update(cx, |view, _| view.set_position(40));
    view.update(cx, |view, cx| view.split_at_playhead(cx));
    assert_eq!(
        *edits.borrow(),
        [TimelineCommand::Split { clip: 1, at: 10 }]
    );
    // On a cut between clips nothing is under the playhead.
    view.update(cx, |view, _| view.set_position(30));
    view.update(cx, |view, cx| view.split_at_playhead(cx));
    assert_eq!(edits.borrow().len(), 1);
}

#[gpui::test]
fn edits_are_refused_with_the_engines_reason(cx: &mut gpui::TestAppContext) {
    use crate::timeline::model::tests::snapshot;
    let (view, cx, edits) = editable(cx);
    view.update(cx, |view, cx| {
        let mut locked = snapshot(&[("a", 30, 0, 0), ("b", 30, 0, 0), ("c", 30, 0, 0)]);
        locked.editing = TimingEditing::Disabled("Build the preview to edit".into());
        view.set_snapshot(Some(locked), cx);
        view.set_position(40);
        view.split_at_playhead(cx);
    });
    cx.run_until_parked();
    let metrics = view.read_with(cx, |view, _| view.qualification_metrics());
    assert_eq!(metrics["edit"]["enabled"], false);
    assert_eq!(metrics["edit"]["reason"], "Build the preview to edit");
    // A press on a clip edge grabs nothing.
    let [left, top, width, height] = clip_bounds(&view, cx, 1);
    press(cx, left + width - 2., top + height / 2.);
    assert!(!view.read_with(cx, |view, _| view.edit_dragging()));
    assert!(edits.borrow().is_empty());
}

/// The resolved stops of `doc` on the view's timeline, editable.
fn slides_on(view: &TimelineView, doc: studio_engine::slides::SlidesDoc) -> SlidesSnapshot {
    let (stops, problems) =
        studio_engine::slides::resolve::resolve(&doc, view.model.as_ref().unwrap().report());
    SlidesSnapshot {
        generation: 1,
        doc: Some(doc),
        stops,
        problems,
        unavailable: None,
        editing: SlidesEditing::Enabled,
        notice: None,
    }
}

/// [`editable`] with one stop at scene frame 10 of `b` (video frame 40), and every slide
/// stop edit the view asks of the engine collected.
fn with_a_stop(
    cx: &mut gpui::TestAppContext,
) -> (
    gpui::Entity<TimelineView>,
    &mut gpui::VisualTestContext,
    std::rc::Rc<std::cell::RefCell<Vec<SlidesCommand>>>,
) {
    use studio_engine::slides::{SlideStop, SlidesDoc, StopAt};
    let (view, cx, _) = editable(cx);
    let stops = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = stops.clone();
    cx.update(|_, app| {
        app.subscribe(&view, move |_, event: &TimelineEvent, _| {
            if let TimelineEvent::Stop(command) = event {
                log.borrow_mut().push(command.clone());
            }
        })
        .detach();
    });
    view.update(cx, |view, cx| {
        let doc = SlidesDoc {
            version: 1,
            stops: vec![SlideStop {
                scene: "b".into(),
                at: StopAt::Frame(10),
                title: None,
                notes: None,
            }],
        };
        let slides = slides_on(view, doc);
        view.set_slides(Some(slides), cx);
    });
    cx.run_until_parked();
    (view, cx, stops)
}

/// The window x of the marker drawn at video frame `frame`, and the y of the ruler.
fn marker_at(
    view: &gpui::Entity<TimelineView>,
    cx: &mut gpui::VisualTestContext,
    frame: usize,
) -> (f32, f32) {
    let metrics = view.read_with(cx, |view, _| view.qualification_metrics());
    let marker = metrics["stops"]["markers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|marker| marker["frame"] == frame)
        .unwrap_or_else(|| panic!("a marker at {frame} in {metrics:#}"));
    let ruler = &metrics["ruler_bounds"];
    let y = ruler[1].as_f64().unwrap() as f32 + METRICS.ruler_h / 2.;
    (marker["x"].as_f64().unwrap() as f32, y)
}

/// Pixels per second of the editable fixture's track.
fn per_second(view: &gpui::Entity<TimelineView>, cx: &mut gpui::VisualTestContext) -> f32 {
    view.read_with(cx, |view, _| view.viewport.pixels_per_second()) as f32
}

#[gpui::test]
fn dragging_a_marker_moves_its_stop_to_the_snapped_frame(cx: &mut gpui::TestAppContext) {
    use studio_engine::slides::StopAt;
    let (view, cx, stops) = with_a_stop(cx);
    let (x, y) = marker_at(&view, cx, 40);
    let pps = per_second(&view, cx);
    // A press 4 px beside the marker still grabs it, and seeks nowhere.
    press(cx, x + 4., y);
    assert!(view.read_with(cx, |view, _| view.edit_dragging()));
    // To 4 px before 5.9 s, the end of `b` (frame 59): it snaps there and is written as
    // "end".
    view.update(cx, |view, cx| view.edit_drag_move(x + 1.9 * pps, false, cx));
    let snapped =
        view.read_with(cx, |view, _| view.qualification_metrics())["stops"]["snapped"].clone();
    assert_eq!(snapped, 5.9);
    // With the snap held off it goes where the pointer is: 5.5 s, scene frame 25.
    view.update(cx, |view, cx| {
        view.edit_drag_move(x + 4. + 1.5 * pps, true, cx)
    });
    view.update(cx, |view, cx| view.end_edit_drag(cx));
    let moved = |at| SlidesCommand::MoveStop {
        stop: 0,
        scene: "b".into(),
        at,
    };
    assert_eq!(
        *stops.borrow(),
        [
            SlidesCommand::BeginGesture,
            moved(StopAt::End),
            moved(StopAt::Frame(25)),
            SlidesCommand::EndGesture,
        ]
    );
    assert!(!view.read_with(cx, |view, _| view.edit_dragging()));
}

#[gpui::test]
fn a_dragged_marker_clamps_to_its_clip(cx: &mut gpui::TestAppContext) {
    use studio_engine::slides::StopAt;
    let (view, cx, stops) = with_a_stop(cx);
    let (x, y) = marker_at(&view, cx, 40);
    let pps = per_second(&view, cx);
    press(cx, x, y);
    // Far past the end of `b` into `c`: the stop stays on b's last frame.
    view.update(cx, |view, cx| view.edit_drag_move(x + 4. * pps, true, cx));
    let drawn =
        view.read_with(cx, |view, _| view.qualification_metrics())["stops"]["markers"][0]["frame"]
            .clone();
    assert_eq!(drawn, 59);
    // Back before its start into `a`: b's first frame, scene frame 0.
    view.update(cx, |view, cx| view.edit_drag_move(x - 2. * pps, true, cx));
    view.update(cx, |view, cx| view.end_edit_drag(cx));
    let moved = |at| SlidesCommand::MoveStop {
        stop: 0,
        scene: "b".into(),
        at,
    };
    assert_eq!(
        *stops.borrow(),
        [
            SlidesCommand::BeginGesture,
            moved(StopAt::End),
            moved(StopAt::Frame(0)),
            SlidesCommand::EndGesture,
        ]
    );
}

#[test]
fn ruler_labels_skip_stop_flags_and_stay_inside_the_ruler() {
    use crate::timeline::ruler::{LabelAnchor, flag_left, label_box, label_hits_flag};
    let (label_w, flag_w) = (METRICS.layout.ruler_label_w, METRICS.layout.stop_flag_w);
    let width = 1000.;
    // Centred on its mark, anchored to it at either end: the last label is not cut.
    assert_eq!(
        label_box(500., width),
        (500. - label_w / 2., LabelAnchor::Centre)
    );
    assert_eq!(label_box(0., width), (0., LabelAnchor::Start));
    assert_eq!(label_box(width, width), (width - label_w, LabelAnchor::End));
    // The last flag points back, inside the ruler.
    assert_eq!(flag_left(500., width), 500.);
    assert_eq!(flag_left(width - 2., width), width - 2. - flag_w);
    assert!(flag_left(width, width) + flag_w <= width);
    // A stop just before a mark hides the mark's label; one between marks hides none.
    let (left, _) = label_box(300., width);
    assert!(label_hits_flag(left, &[298.], width));
    assert!(label_hits_flag(left, &[200., 320.], width));
    assert!(!label_hits_flag(left, &[340.], width));
    assert!(!label_hits_flag(left, &[], width));
    // The video's end stop hides the last label, whose flag points back over it.
    let (end, _) = label_box(width, width);
    assert!(label_hits_flag(end, &[width - 2.], width));
}

#[gpui::test]
fn a_press_near_a_default_stop_flag_seeks_to_the_stop(cx: &mut gpui::TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let seeks = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let log = seeks.clone();
    cx.update(|_, app| {
        app.subscribe(&view, move |_, event: &TimelineEvent, _| {
            if let TimelineEvent::Seek(frame) = event {
                log.borrow_mut().push(*frame);
            }
        })
        .detach();
    });
    // Two three-second clips at 100 fps: without stops of its own, each clip end is a
    // default stop (frames 299 and 599).
    view.update(cx, |view, cx| {
        let model = TimelineModel::new(report(100, &[("a", 0, 300), ("b", 300, 600)])).unwrap();
        view.install(model, Arc::from([]), 0, cx);
    });
    cx.run_until_parked();
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        let slides = slides_on(view, studio_engine::slides::SlidesDoc::default());
        view.set_slides(Some(slides), cx);
    });
    cx.run_until_parked();
    let (x, y) = marker_at(&view, cx, 299);
    // 5 px is more than a frame here, so a press there without the hit area seeks
    // elsewhere.
    assert!(5. * 100. / per_second(&view, cx) >= 1.);
    for offset in [-5., 5., 12.] {
        cx.simulate_click(gpui::point(px(x + offset), px(y)), gpui::Modifiers::none());
        cx.run_until_parked();
    }
    let seeks = seeks.borrow();
    // Within 6 px either side of the pole (a 12 px hit area) it lands on the stop.
    assert_eq!(seeks[..2], [299, 299]);
    // Further out the ruler scrubs to the pointer as before.
    assert!(seeks[2] > 299, "{seeks:?}");
    // Nothing was added or moved.
    let markers = view.read_with(cx, |view, _| view.qualification_metrics())["stops"]["markers"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(markers, 2);
}

#[gpui::test]
fn a_dragged_stop_shows_its_time(cx: &mut gpui::TestAppContext) {
    let (view, cx, _) = with_a_stop(cx);
    let (x, y) = marker_at(&view, cx, 40);
    let pps = per_second(&view, cx);
    press(cx, x, y);
    assert!(
        cx.debug_bounds("stop-readout").is_none(),
        "not before it moves"
    );
    // One second right: frame 50.
    view.update(cx, |view, cx| view.edit_drag_move(x + pps, true, cx));
    cx.run_until_parked();
    let pill = cx
        .debug_bounds("stop-readout")
        .expect("the readout follows the dragged stop");
    assert!(f32::from(pill.top()) >= y, "under the ruler: {pill:?}");
    view.update(cx, |view, cx| view.end_edit_drag(cx));
    cx.run_until_parked();
    assert!(cx.debug_bounds("stop-readout").is_none());
}

#[gpui::test]
fn a_double_click_on_the_ruler_adds_a_stop(cx: &mut gpui::TestAppContext) {
    use studio_engine::slides::{SlideStop, StopAt};
    let (view, cx, stops) = with_a_stop(cx);
    let (x, y) = marker_at(&view, cx, 40);
    let pps = per_second(&view, cx);
    // One second right of the marker, frame 50: scene frame 20 of `b`.
    let at = gpui::point(px(x + pps), px(y));
    for click_count in [1, 2] {
        cx.simulate_event(gpui::MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: gpui::Modifiers::none(),
            click_count,
            ..Default::default()
        });
        cx.simulate_event(gpui::MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: gpui::Modifiers::none(),
            click_count,
        });
    }
    assert_eq!(
        *stops.borrow(),
        [SlidesCommand::AddStop(SlideStop {
            scene: "b".into(),
            at: StopAt::Frame(20),
            title: None,
            notes: None,
        })]
    );
    // Delete removes the marker pressed last.
    press(cx, x, y);
    view.update(cx, |view, cx| {
        view.end_edit_drag(cx);
        view.remove_selected_stop(cx);
    });
    assert_eq!(
        stops.borrow().last(),
        Some(&SlidesCommand::RemoveStop { stop: 0 })
    );
}

#[gpui::test]
fn ctrl_z_undoes_a_marker_drag_before_the_trim_made_before_it(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};
    use studio_engine::slides::{SlideStop, StopAt};
    use studio_engine::{Controller, JobKind, JobResult, app_paths::AppPaths};

    // An open project whose preview laid out `a`, `b` and `c`, 30 frames each at 10 fps,
    // with a stop at scene frame 10 of `b`.
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("video");
    let paths = AppPaths::new(temp.path().join("history")).unwrap();
    let compatibility = studio_sdk::CompatibilityManifest::default_linux_x64();
    studio_project::create(
        &root,
        "Video",
        studio_engine::build_materialization::sdk_pin(&compatibility),
        &compatibility.fframes_version,
        &compatibility.runtime_version,
    )
    .unwrap();
    let mut controller = Controller::open(&root, &paths).unwrap();
    let tag = controller.begin_job(JobKind::Build).unwrap();
    controller
        .complete(&tag, JobResult::Built(tag.base_source.clone()))
        .unwrap();
    // What the worker lays out for the engine's clips: end to end, trims applied.
    let laid_out = move |controller: &mut Controller| {
        let snapshot = controller.timeline_snapshot();
        let clips: Vec<_> = if snapshot.clips.is_empty() {
            vec![("a", 30, 0, 0), ("b", 30, 0, 0), ("c", 30, 0, 0)]
                .into_iter()
                .map(|(key, frames, start, end)| (key.to_owned(), frames, start, end))
                .collect()
        } else {
            snapshot
                .clips
                .iter()
                .map(|c| {
                    (
                        c.key.clone(),
                        c.scene_frames,
                        c.trim_start_frames,
                        c.trim_end_frames,
                    )
                })
                .collect()
        };
        let mut start = 0;
        let mut report = report(10, &[]);
        report.envelope.identity = studio_engine::preview_identity(&tag);
        report.timing_generation = snapshot.generation;
        for (index, (key, frames, trim_start, trim_end)) in clips.into_iter().enumerate() {
            let played = frames - trim_start - trim_end;
            let mut scene = crate::timeline::model::tests::report(10, &[(key.as_str(), 0, played)])
                .scenes
                .remove(0);
            scene.instance_id = format!("scene-{index}");
            scene.index = index;
            scene.start_frame = start;
            scene.end_frame = start + played;
            let clip = scene.clip.as_mut().unwrap();
            clip.trim_start_frames = trim_start;
            clip.trim_end_frames = trim_end;
            clip.scene_frames = Some(frames);
            clip.scene_index = Some(index);
            start += played;
            report.scenes.push(scene);
        }
        report.total_frames = start;
        report.duration_seconds = start as f32 / 10.;
        controller.learn_timeline(&tag, &report);
        report
    };
    laid_out(&mut controller);
    controller
        .slides_command(SlidesCommand::AddStop(SlideStop {
            scene: "b".into(),
            at: StopAt::Frame(10),
            title: None,
            notes: None,
        }))
        .unwrap();
    let controller = Rc::new(RefCell::new(controller));

    // The view shows what the engine says after every edit, as the shell has it shown.
    let (view, cx) = cx.add_window_view(|_, cx| TimelineView::new(cx));
    let show = {
        let (controller, view) = (controller.clone(), view.clone());
        let laid_out = laid_out.clone();
        Rc::new(move |cx: &mut gpui::App| {
            let mut controller = controller.borrow_mut();
            let report = laid_out(&mut controller);
            let (timeline, slides) = (controller.timeline_snapshot(), controller.slides_snapshot());
            view.update(cx, |view, cx| {
                view.install(TimelineModel::new(report).unwrap(), Arc::from([]), 0, cx);
                view.set_snapshot(Some(timeline), cx);
                view.set_slides(Some(slides), cx);
            });
        })
    };
    cx.update(|_, app| {
        crate::shell::bind_keys(app);
        let (engine, refresh) = (controller.clone(), show.clone());
        app.subscribe(&view, move |_, event: &TimelineEvent, cx| {
            let done = match event {
                TimelineEvent::Edit(command) => engine
                    .borrow_mut()
                    .timeline_command(command.clone())
                    .map(drop),
                TimelineEvent::Stop(command) => engine
                    .borrow_mut()
                    .slides_command(command.clone())
                    .map(drop),
                _ => return,
            };
            done.unwrap();
            refresh(cx);
        })
        .detach();
        // The shell's handler: Ctrl+Z in the Timeline context undoes the latest step of
        // the history the timing and the slides share.
        let (engine, refresh) = (controller.clone(), show.clone());
        app.on_action(move |_: &crate::actions::UndoTimelineEdit, cx| {
            engine
                .borrow_mut()
                .timeline_command(TimelineCommand::Undo)
                .unwrap();
            refresh(cx);
        });
        show(app);
    });
    cx.run_until_parked();
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let trim_end = |controller: &Rc<RefCell<Controller>>| {
        controller.borrow_mut().timeline_snapshot().clips[0].trim_end_frames
    };
    let stop_of = |controller: &Rc<RefCell<Controller>>| {
        let snapshot = controller.borrow_mut().slides_snapshot();
        (
            snapshot.doc.unwrap().stops[0].at,
            snapshot.stops[0].video_frame,
        )
    };
    assert_eq!(stop_of(&controller), (StopAt::Frame(10), 40));

    // Trim a second off the end of `a`: `b` and its stop move 10 frames earlier.
    let [left, top, width, height] = clip_bounds(&view, cx, 0);
    let pps = width / 3.;
    press(cx, left + width - 2., top + height / 2.);
    view.update(cx, |view, cx| {
        view.edit_drag_move(left + width - 2. - pps, true, cx);
        view.end_edit_drag(cx);
    });
    cx.run_until_parked();
    assert_eq!(trim_end(&controller), 10);
    assert_eq!(stop_of(&controller), (StopAt::Frame(10), 30));

    // Drag the marker a second right.
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
    let (x, y) = marker_at(&view, cx, 30);
    let pps = per_second(&view, cx);
    press(cx, x, y);
    view.update(cx, |view, cx| {
        view.edit_drag_move(x + pps, true, cx);
        view.end_edit_drag(cx);
    });
    cx.run_until_parked();
    assert_eq!(stop_of(&controller), (StopAt::Frame(20), 40));

    // Ctrl+Z in the timeline: the marker returns, the trim stays.
    view.update_in(cx, |view, window, cx| view.focus(window, cx));
    cx.simulate_keystrokes("ctrl-z");
    cx.run_until_parked();
    assert_eq!(stop_of(&controller), (StopAt::Frame(10), 30));
    assert_eq!(trim_end(&controller), 10);
    // Again: the trim goes.
    cx.simulate_keystrokes("ctrl-z");
    cx.run_until_parked();
    assert_eq!(trim_end(&controller), 0);
    assert_eq!(stop_of(&controller), (StopAt::Frame(10), 40));
    // The live files agree.
    let live = std::fs::read_to_string(root.join(studio_project::revision::SLIDES_FILE)).unwrap();
    assert!(live.contains("\"at\": 10"), "{live}");
}
