use fframes_studio_protocol::{
    EditorFrameMetadata, EditorFrameStatus, EditorGeometrySupport, EditorObjectGeometry,
    EditorObjectIdentity, PreviewIdentity, Rect,
};
use studio_engine::{
    CanvasRect, CanvasSelectionError, CanvasViewport, DisplayedFrameTag, cycle_hit_candidates,
    hit_test, revalidate_selection,
};

fn preview() -> PreviewIdentity {
    PreviewIdentity {
        project_id: "project".into(),
        open_session: "session".into(),
        source_revision: "a".repeat(64),
        worker_generation: 8,
    }
}

fn identity(key: &str, repeat: &str) -> EditorObjectIdentity {
    EditorObjectIdentity {
        scene_instance_key: "scene-instance-2".into(),
        component_key: "title".into(),
        object_key: key.into(),
        repeat_key: repeat.into(),
    }
}

fn metadata(frame_index: usize, seek_serial: u64) -> EditorFrameMetadata {
    let group = identity("group", "primary");
    let under = identity("title-under", "instance-1");
    let top = identity("title-top", "instance-2");
    EditorFrameMetadata {
        frame_index,
        seek_serial,
        video_width: 1600,
        video_height: 900,
        editor_index_digest: "b".repeat(64),
        frame_geometry_digest: if frame_index == 12 {
            "c".repeat(64)
        } else {
            "d".repeat(64)
        },
        status: EditorFrameStatus::Supported,
        reason: None,
        objects: vec![
            EditorObjectGeometry {
                identity: group.clone(),
                parent: None,
                source_anchor: None,
                style_tokens: vec![],
                bounds: Rect {
                    x: 100.0,
                    y: 100.0,
                    width: 700.0,
                    height: 500.0,
                },
                paint_order: 1,
                support: EditorGeometrySupport::ExactBounds,
            },
            EditorObjectGeometry {
                identity: under,
                parent: Some(group.clone()),
                source_anchor: None,
                style_tokens: vec![],
                bounds: Rect {
                    x: 200.0,
                    y: 200.0,
                    width: 300.0,
                    height: 120.0,
                },
                paint_order: 3,
                support: EditorGeometrySupport::ExactBounds,
            },
            EditorObjectGeometry {
                identity: top,
                parent: Some(group),
                source_anchor: None,
                style_tokens: vec![],
                bounds: Rect {
                    x: 250.0,
                    y: 220.0,
                    width: 250.0,
                    height: 100.0,
                },
                paint_order: 4,
                support: EditorGeometrySupport::ApproximateBounds,
            },
        ],
    }
}

fn viewport() -> CanvasViewport {
    CanvasViewport::new(
        CanvasRect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        },
        1600,
        900,
    )
    .unwrap()
}

#[test]
fn selection_uses_the_painted_transform_and_cycles_repeated_instances() {
    let frame = metadata(12, 4);
    let tag = DisplayedFrameTag::from_metadata(preview(), &frame).unwrap();
    let viewport = viewport();
    assert_eq!(viewport.image_bounds().height, 450.0);
    assert!(
        hit_test(&frame, &tag, &viewport, 400.0, 20.0)
            .unwrap()
            .is_none()
    );

    // Fit is 0.5 logical points per video pixel, with 75 points of top letterbox.
    let (x, y) = (160.0, 200.0);
    let picked = hit_test(&frame, &tag, &viewport, x, y).unwrap().unwrap();
    assert_eq!(picked.identity.object_key, "title-top");
    assert_eq!(picked.support, EditorGeometrySupport::ApproximateBounds);

    let next = cycle_hit_candidates(&frame, &tag, &viewport, x, y, Some(&picked.identity))
        .unwrap()
        .unwrap();
    assert_eq!(next.identity.object_key, "title-under");
    assert_ne!(next.identity.repeat_key, picked.identity.repeat_key);
}

#[test]
fn stale_frame_and_removed_identity_cannot_rebind_selection() {
    let first = metadata(12, 4);
    let first_tag = DisplayedFrameTag::from_metadata(preview(), &first).unwrap();
    let selected = hit_test(&first, &first_tag, &viewport(), 150.0, 250.0)
        .unwrap()
        .unwrap();

    let next = metadata(13, 5);
    let next_tag = DisplayedFrameTag::from_metadata(preview(), &next).unwrap();
    assert_eq!(
        hit_test(&next, &first_tag, &viewport(), 150.0, 250.0),
        Err(CanvasSelectionError::StaleFrame)
    );
    assert!(revalidate_selection(&selected, &next_tag, &next).is_ok());

    let mut removed = next.clone();
    removed
        .objects
        .retain(|object| object.identity != selected.identity);
    // A semantically unavailable frame may not contain a partial object set.
    removed.objects.clear();
    removed.status = EditorFrameStatus::Unannotated;
    removed.frame_geometry_digest = "e".repeat(64);
    let removed_tag = DisplayedFrameTag::from_metadata(preview(), &removed).unwrap();
    assert_eq!(
        revalidate_selection(&selected, &removed_tag, &removed),
        Err(CanvasSelectionError::MissingObject)
    );
}
