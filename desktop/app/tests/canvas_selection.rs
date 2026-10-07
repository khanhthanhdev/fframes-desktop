use fframes_studio::canvas_view::CanvasViewState;
use fframes_studio_protocol::*;
use studio_engine::PreviewFrame;

fn identity(revision: &str, generation: u64) -> PreviewIdentity {
    PreviewIdentity {
        project_id: "project".into(),
        open_session: "open-session".into(),
        source_revision: revision.into(),
        worker_generation: generation,
    }
}

fn metadata(frame_index: usize, seek_serial: u64, object: bool) -> EditorFrameMetadata {
    EditorFrameMetadata {
        frame_index,
        seek_serial,
        video_width: 4,
        video_height: 4,
        editor_index_digest: "a".repeat(64),
        frame_geometry_digest: format!("{:0>64}", frame_index + 1),
        status: if object {
            EditorFrameStatus::Supported
        } else {
            EditorFrameStatus::Unannotated
        },
        reason: None,
        objects: if object {
            vec![EditorObjectGeometry {
                identity: EditorObjectIdentity {
                    scene_instance_key: "scene-1".into(),
                    component_key: "title".into(),
                    object_key: "headline".into(),
                    repeat_key: "primary".into(),
                },
                parent: None,
                source_anchor: None,
                style_tokens: vec!["color.text".into()],
                bounds: Rect {
                    x: frame_index as f32,
                    y: 0.0,
                    width: 2.0,
                    height: 2.0,
                },
                paint_order: 1,
                support: EditorGeometrySupport::ExactBounds,
            }]
        } else {
            Vec::new()
        },
    }
}

fn frame(
    preview: &PreviewIdentity,
    frame_index: usize,
    seek_serial: u64,
    editor_metadata: Option<EditorFrameMetadata>,
) -> PreviewFrame {
    let request_id = frame_index as u64 + 10;
    let header = FrameHeader {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        source_revision: preview.source_revision.clone(),
        worker_generation: preview.worker_generation,
        request_id,
        frame_index,
        width: 4,
        height: 4,
        stride_bytes: 16,
        channel_order: ChannelOrder::Rgba8,
        alpha_mode: AlphaMode::Straight,
        color_space: ColorSpace::Srgb,
        payload_len: 64,
    };
    PreviewFrame {
        response: ScaledFrameResponse {
            envelope: PreviewEnvelope {
                contract_version: PREVIEW_CONTRACT_VERSION,
                identity: preview.clone(),
                request_id,
            },
            frame_index,
            seek_serial,
            scale: 1.0,
            header,
            render_duration_micros: 1,
            record: BinaryRecordHeader {
                kind: BinaryRecordKind::FrameRgba8,
                identity: preview.clone(),
                request_id,
                offset: 0,
                payload_len: 64,
            },
            editor_metadata,
        },
        pixels: vec![0; 64],
    }
}

#[test]
fn accepted_frame_keeps_identity_geometry_and_image_pairing_atomic() {
    let preview = identity(&"b".repeat(64), 3);
    let mut canvas = CanvasViewState::default();
    let first = frame(&preview, 0, 4, Some(metadata(0, 4, true)));
    canvas.install_frame(preview.clone(), &first).unwrap();
    canvas.resize(100.0, 100.0, 4, 4).unwrap();
    assert_eq!(
        canvas
            .select_at(25.0, 25.0, false)
            .unwrap()
            .unwrap()
            .identity
            .object_key,
        "headline"
    );
    let scoped = canvas.task_scope_selection().unwrap();
    assert!(matches!(
        scoped.selection,
        studio_engine::CanvasTaskSelectionKind::Element { style_tokens, .. }
            if style_tokens == ["color.text"]
    ));

    let second = frame(&preview, 1, 5, Some(metadata(1, 5, true)));
    canvas.install_frame(preview.clone(), &second).unwrap();
    assert_eq!(canvas.selection.as_ref().unwrap().bounds.x, 1.0);
    assert_eq!(canvas.displayed.as_ref().unwrap().frame_index, 1);
    assert_eq!(canvas.displayed.as_ref().unwrap().seek_serial, 5);

    let stale = frame(&preview, 2, 6, Some(metadata(1, 5, true)));
    assert!(canvas.install_frame(preview.clone(), &stale).is_err());
    assert_eq!(canvas.displayed.as_ref().unwrap().frame_index, 1);
    assert_eq!(canvas.selection.as_ref().unwrap().bounds.x, 1.0);

    let reopened = identity(&"c".repeat(64), 4);
    canvas
        .install_frame(
            reopened,
            &frame(
                &identity(&"c".repeat(64), 4),
                1,
                5,
                Some(metadata(1, 5, true)),
            ),
        )
        .unwrap();
    assert!(canvas.selection.is_none());
    assert!(canvas.message.as_deref().unwrap().contains("cleared"));
}

#[test]
fn missing_metadata_keeps_rectangle_scope_usable_and_never_infers_an_anchor() {
    let preview = identity(&"d".repeat(64), 5);
    let mut canvas = CanvasViewState::default();
    canvas
        .install_frame(preview.clone(), &frame(&preview, 3, 9, None))
        .unwrap();
    assert!(canvas.message.as_deref().unwrap().contains("unavailable"));
    canvas.resize(100.0, 100.0, 4, 4).unwrap();
    assert!(canvas.begin_rectangle(10.0, 10.0));
    let rectangle = canvas.update_rectangle(80.0, 80.0).unwrap();
    assert!((rectangle.x - 0.4).abs() < 0.0001);
    assert!((rectangle.y - 0.4).abs() < 0.0001);
    assert!((rectangle.width - 2.8).abs() < 0.0001);
    assert!((rectangle.height - 2.8).abs() < 0.0001);
    assert!(canvas.end_rectangle().is_some());
    assert!(canvas.selection_identity().is_none());
    let task_selection = canvas.task_scope_selection().unwrap();
    assert_eq!(task_selection.editor_index_digest, None);
    assert_eq!(task_selection.frame_geometry_digest, None);
    assert!(matches!(
        task_selection.selection,
        studio_engine::CanvasTaskSelectionKind::Rectangle { .. }
    ));
}

#[test]
fn clicking_unannotated_or_invalid_frames_preserves_the_support_message() {
    let preview = identity(&"f".repeat(64), 10);
    let mut canvas = CanvasViewState::default();
    canvas
        .install_frame(
            preview.clone(),
            &frame(&preview, 0, 1, Some(metadata(0, 1, false))),
        )
        .unwrap();
    canvas.resize(100.0, 100.0, 4, 4).unwrap();
    assert!(canvas.select_at(25.0, 25.0, false).unwrap().is_none());
    assert!(
        canvas
            .message
            .as_deref()
            .unwrap()
            .contains("No semantic canvas objects")
    );

    let mut invalid = metadata(1, 2, false);
    invalid.status = EditorFrameStatus::Invalid;
    invalid.reason = Some("duplicate semantic editor key".into());
    canvas
        .install_frame(preview.clone(), &frame(&preview, 1, 2, Some(invalid)))
        .unwrap();
    assert!(canvas.select_at(25.0, 25.0, false).unwrap().is_none());
    assert_eq!(
        canvas.message.as_deref(),
        Some("duplicate semantic editor key")
    );
}

#[test]
fn missing_identity_clears_semantic_selection_on_the_next_accepted_frame() {
    let preview = identity(&"e".repeat(64), 9);
    let mut canvas = CanvasViewState::default();
    canvas
        .install_frame(
            preview.clone(),
            &frame(&preview, 0, 1, Some(metadata(0, 1, true))),
        )
        .unwrap();
    canvas.resize(100.0, 100.0, 4, 4).unwrap();
    canvas.select_at(25.0, 25.0, false).unwrap();

    canvas
        .install_frame(
            preview.clone(),
            &frame(&preview, 1, 2, Some(metadata(1, 2, false))),
        )
        .unwrap();
    assert!(canvas.selection.is_none());
    assert!(canvas.message.as_deref().unwrap().contains("cleared"));
}
