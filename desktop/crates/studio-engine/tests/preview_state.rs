use fframes_studio_protocol::*;
use studio_engine::*;

fn source(c: char) -> studio_project::SourceRevision {
    c.to_string().repeat(64).try_into().unwrap()
}
fn state() -> ProjectState {
    let mut s = ProjectState::opening(
        "preview-project".to_owned().try_into().unwrap(),
        OpenSession::new(),
        source('a'),
        source('b'),
    );
    s.finish_open(Ok(())).unwrap();
    s
}
fn envelope(t: &OperationTag, request_id: u64) -> PreviewEnvelope {
    PreviewEnvelope {
        contract_version: PREVIEW_CONTRACT_VERSION,
        identity: preview_identity(t),
        request_id,
    }
}
fn frame(t: &OperationTag, position: usize, serial: u64) -> PreviewFrame {
    PreviewFrame {
        response: ScaledFrameResponse {
            envelope: envelope(t, 4),
            frame_index: position,
            seek_serial: serial,
            scale: 1.,
            header: FrameHeader::new_straight_rgba(
                t.base_source.as_str(),
                t.generation,
                4,
                position,
                2,
                2,
            )
            .unwrap(),
            render_duration_micros: 10,
            record: BinaryRecordHeader {
                kind: BinaryRecordKind::FrameRgba8,
                identity: preview_identity(t),
                request_id: 4,
                offset: 0,
                payload_len: 16,
            },
            editor_metadata: None,
        },
        pixels: vec![37; 16],
    }
}
fn ready(t: &OperationTag, position: usize, serial: u64, total: usize) -> ReadyPreview {
    let timeline = PreviewTimelineResponse {
        envelope: envelope(t, 2),
        fps: 30,
        width: 2,
        height: 2,
        total_frames: total,
        duration_seconds: total as f32 / 30.,
        scenes: vec![],
        audio_tracks: vec![],
    };
    let samples = total as u64 * 1600;
    let start = (position as u64 * 1600).min(samples);
    ReadyPreview::new(
        t.clone(),
        timeline,
        InspectResponse {
            envelope: envelope(t, 3),
            diagnostics: vec![],
            truncated: false,
        },
        if total == 0 {
            None
        } else {
            Some(frame(t, position.min(total - 1), serial))
        },
        PreparedAudioDescriptor {
            envelope: envelope(t, 5),
            artifact_id: "mix".into(),
            sample_rate: 48000,
            channels: 2,
            sample_count: samples,
            byte_count: samples * 8,
            sha256: "0".repeat(64),
            silent: true,
        },
        start,
        vec![0; ((samples - start) * 8).min(256) as usize],
        position,
        serial,
    )
    .unwrap()
}
#[test]
fn build_and_checkpoint_success_are_not_preview_installation() {
    let mut s = state();
    let mut p = PreviewState::default();
    let t = s.queue(JobKind::Build).unwrap();
    s.start(&t).unwrap();
    p.begin(t.clone());
    let r = ready(&t, 0, 0, 60);
    assert!(p.install(&r, &s).is_err());
    s.complete(&t, source('a'), JobResult::Built(source('a')))
        .unwrap();
    assert!(p.displayed().is_none());
    assert_eq!(s.accepted(), &source('b'));
    p.install(&r, &s).unwrap();
    assert_eq!(p.displayed(), Some(&preview_identity(&t)));
    assert_eq!(s.accepted(), &source('b'));
    let checkpoint = s.queue(JobKind::Checkpoint).unwrap();
    s.start(&checkpoint).unwrap();
    s.complete(
        &checkpoint,
        source('a'),
        JobResult::Checkpointed(source('a')),
    )
    .unwrap();
    assert_eq!(p.displayed(), Some(&preview_identity(&t)));
}
#[test]
fn newest_seek_and_scale_only_and_failed_revision_does_not_invalidate_displayed_worker() {
    let mut s = state();
    let mut p = PreviewState::default();
    let t = s.queue(JobKind::Build).unwrap();
    s.start(&t).unwrap();
    p.begin(t.clone());
    s.complete(&t, source('a'), JobResult::Built(source('a')))
        .unwrap();
    p.install(&ready(&t, 0, 0, 60), &s).unwrap();
    let old = p.seek(11, 1.).unwrap();
    let latest = p.seek(37, 1.).unwrap();
    assert!(!p.accepts_frame(&frame(&t, 11, old)));
    assert!(p.accepts_frame(&frame(&t, 37, latest)));
    let serial = p.seek(37, 0.5).unwrap();
    assert!(
        !p.accepts_frame(&frame(&t, 37, serial)),
        "old scale in same worker is obsolete"
    );
    s.reconcile_source(source('c')).unwrap();
    let failed = s.queue(JobKind::Build).unwrap();
    s.start(&failed).unwrap();
    p.begin(failed.clone());
    s.complete(
        &failed,
        source('c'),
        JobResult::Failed("compiler failed".into()),
    )
    .unwrap();
    p.fail(&failed, "compiler failed".into());
    let serial = p.seek(29, 1.).unwrap();
    assert!(
        p.accepts_frame(&frame(&t, 29, serial)),
        "prior source remains a valid playback target"
    );
    p.close();
    assert!(!p.accepts_frame(&frame(&t, 29, serial)));
}
#[test]
fn seek_during_preparation_rejects_old_frame_and_clamps_shorter_and_empty_revision() {
    let mut s = state();
    let mut p = PreviewState::default();
    let t = s.queue(JobKind::Build).unwrap();
    s.start(&t).unwrap();
    p.begin(t.clone());
    s.complete(&t, source('a'), JobResult::Built(source('a')))
        .unwrap();
    p.install(&ready(&t, 0, 0, 60), &s).unwrap();
    p.seek(47, 1.).unwrap();
    let next = s.queue(JobKind::Build).unwrap();
    s.start(&next).unwrap();
    p.begin(next.clone());
    s.complete(&next, source('a'), JobResult::Built(source('a')))
        .unwrap();
    let prepared = ready(&next, 15, p.serial(), 15);
    p.seek(8, 1.).unwrap();
    assert!(p.install(&prepared, &s).is_err());
    p.install(&ready(&next, 8, p.serial(), 15), &s).unwrap();
    assert_eq!(p.position(), 8);
    p.seek(1000, 1.).unwrap();
    assert_eq!(p.position(), 15);
    let empty = s.queue(JobKind::Build).unwrap();
    s.start(&empty).unwrap();
    p.begin(empty.clone());
    s.complete(&empty, source('a'), JobResult::Built(source('a')))
        .unwrap();
    p.install(&ready(&empty, 0, p.serial(), 0), &s).unwrap();
    assert_eq!(p.position(), 0);
}
#[test]
fn every_identity_boundary_and_edit_back_is_rechecked_at_install() {
    let mut s = state();
    let mut p = PreviewState::default();
    let t = s.queue(JobKind::Build).unwrap();
    s.start(&t).unwrap();
    p.begin(t.clone());
    s.complete(&t, source('a'), JobResult::Built(source('a')))
        .unwrap();
    let r = ready(&t, 0, 0, 60);
    assert!(p.can_install(&r, &s.reopened(source('a'))).is_err());
    s.reconcile_source(source('c')).unwrap();
    s.reconcile_source(source('a')).unwrap();
    assert!(
        p.install(&r, &s).is_err(),
        "edit-back does not resurrect a compiled candidate"
    );
    assert!(p.displayed().is_none());
}

#[test]
fn readiness_rejects_incomplete_inspection_pcm_offset_and_cross_identity_data() {
    let mut s = state();
    let t = s.queue(JobKind::Build).unwrap();
    for mistake in 0..7 {
        let mut r = ready(&t, 19, 7, 61);
        match mistake {
            0 => r.inspection.truncated = true,
            1 => r.inspection.diagnostics.push(PreviewDiagnostic {
                frame: 19,
                severity: DiagnosticSeverity::Error,
                key: "missing_media".into(),
                message: "media gone".into(),
            }),
            2 => r.pcm.clear(),
            3 => r.pcm_start_sample += 1,
            4 => r.pcm[..4].copy_from_slice(&f32::NAN.to_le_bytes()),
            5 => r.audio.envelope.identity.project_id = "other-project".into(),
            6 => {
                r.audio.sample_count += 1;
                r.audio.byte_count += 8;
            }
            _ => unreachable!(),
        }
        assert!(
            ReadyPreview::new(
                t.clone(),
                r.timeline,
                r.inspection,
                r.frame,
                r.audio,
                r.pcm_start_sample,
                r.pcm,
                r.position,
                r.seek_serial
            )
            .is_err(),
            "readiness mistake {mistake}"
        );
    }
}
