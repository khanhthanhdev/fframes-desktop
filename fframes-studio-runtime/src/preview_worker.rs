use crate::{WorkerError, WorkerTransport};
use fframes::{AudioMixer, CpuFrameRenderer, Previewer, RenderOptions, Video};
use fframes_studio_protocol::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct PreviewWorkerConfig {
    pub identity: PreviewIdentity,
    pub fframes_version: String,
    pub sdk_version: String,
    pub cache_directory: PathBuf,
    pub preparation_deadline: Duration,
}
impl PreviewWorkerConfig {
    pub fn new(
        identity: PreviewIdentity,
        sdk_version: impl Into<String>,
        fframes_version: impl Into<String>,
    ) -> Self {
        Self {
            identity,
            fframes_version: fframes_version.into(),
            sdk_version: sdk_version.into(),
            cache_directory: std::env::temp_dir().join("fframes-preview-audio"),
            preparation_deadline: Duration::from_secs(120),
        }
    }
}
struct Artifact {
    descriptor: PreparedAudioDescriptor,
    path: PathBuf,
}
struct Artifacts {
    entries: VecDeque<Artifact>,
}
impl Drop for Artifacts {
    fn drop(&mut self) {
        for a in &self.entries {
            let _ = fs::remove_file(&a.path);
        }
    }
}
impl Artifacts {
    fn remove(&mut self, id: &str) -> bool {
        if let Some(i) = self
            .entries
            .iter()
            .position(|a| a.descriptor.artifact_id == id)
        {
            if let Some(a) = self.entries.remove(i) {
                let _ = fs::remove_file(a.path);
            }
            true
        } else {
            false
        }
    }
    fn get(&self, id: &str) -> Option<&Artifact> {
        self.entries.iter().find(|a| a.descriptor.artifact_id == id)
    }
    fn reserve(&self, bytes: u64) -> Result<(), String> {
        let retained: u64 = self.entries.iter().map(|a| a.descriptor.byte_count).sum();
        if self.entries.len() >= 2
            || retained
                .checked_add(bytes)
                .is_none_or(|v| v > MAX_PREPARED_AUDIO_CACHE_BYTES)
        {
            return Err("Release an unused prepared artifact before preparing another mix".into());
        }
        Ok(())
    }
    fn push(&mut self, a: Artifact) {
        self.entries.push_back(a)
    }
}

fn error(env: Option<PreviewEnvelope>, code: &str, message: impl ToString) -> PreviewResponse {
    PreviewResponse::Error(PreviewError {
        envelope: env,
        code: code.into(),
        message: message.to_string().chars().take(4096).collect(),
    })
}

fn editor_frame_metadata(
    geometry: Result<fframes::EditorFrameGeometry, fframes::EditorMetadataError>,
    frame_index: usize,
    seek_serial: u64,
    video_width: usize,
    video_height: usize,
    ambiguous_scene_keys: &HashSet<String>,
) -> EditorFrameMetadata {
    let invalid = |reason: String| {
        let digest = format!("{:x}", Sha256::digest(reason.as_bytes()));
        EditorFrameMetadata {
            frame_index,
            seek_serial,
            video_width: video_width as u32,
            video_height: video_height as u32,
            editor_index_digest: digest.clone(),
            frame_geometry_digest: digest,
            status: EditorFrameStatus::Invalid,
            reason: Some(reason.chars().take(256).collect()),
            objects: Vec::new(),
        }
    };
    let geometry = match geometry {
        Ok(geometry) => geometry,
        Err(reason) => return invalid(reason.to_string()),
    };
    if geometry.video_width as usize != video_width
        || geometry.video_height as usize != video_height
    {
        return invalid("video dimensions do not match preview timeline".into());
    }
    let mut objects: Vec<_> = geometry
        .objects
        .into_iter()
        .map(|object| {
            let identity = EditorObjectIdentity {
                scene_instance_key: object.key.scene_instance_key,
                component_key: object.key.component_key,
                object_key: object.key.object_key,
                repeat_key: object.key.repeat_key,
            };
            EditorObjectGeometry {
                identity,
                parent: object.parent.map(|parent| EditorObjectIdentity {
                    scene_instance_key: parent.scene_instance_key,
                    component_key: parent.component_key,
                    object_key: parent.object_key,
                    repeat_key: parent.repeat_key,
                }),
                source_anchor: object.source_anchor.map(|anchor| EditorSourceAnchor {
                    path: anchor.path,
                    symbol: anchor.symbol,
                    marker: anchor.marker,
                }),
                style_tokens: object.style_tokens,
                bounds: Rect {
                    x: object.bounds.x,
                    y: object.bounds.y,
                    width: object.bounds.width,
                    height: object.bounds.height,
                },
                paint_order: object.paint_order,
                support: match object.support {
                    fframes::EditorGeometrySupport::ExactBounds => {
                        EditorGeometrySupport::ExactBounds
                    }
                    fframes::EditorGeometrySupport::ApproximateBounds => {
                        EditorGeometrySupport::ApproximateBounds
                    }
                    fframes::EditorGeometrySupport::Unsupported => {
                        EditorGeometrySupport::Unsupported
                    }
                },
            }
        })
        .collect();
    objects.sort_by(|a, b| {
        (
            &a.identity.scene_instance_key,
            &a.identity.component_key,
            &a.identity.object_key,
            &a.identity.repeat_key,
        )
            .cmp(&(
                &b.identity.scene_instance_key,
                &b.identity.component_key,
                &b.identity.object_key,
                &b.identity.repeat_key,
            ))
    });
    if objects
        .iter()
        .any(|object| ambiguous_scene_keys.contains(&object.identity.scene_instance_key))
    {
        return invalid("editor scene instance key is duplicated in the timeline".into());
    }
    let editor_index: Vec<_> = objects
        .iter()
        .map(|object| {
            (
                &object.identity,
                &object.source_anchor,
                &object.style_tokens,
            )
        })
        .collect();
    let editor_index_digest = match serde_json::to_vec(&editor_index) {
        Ok(bytes) => format!("{:x}", Sha256::digest(bytes)),
        Err(error) => return invalid(error.to_string()),
    };
    let frame_geometry_digest = match serde_json::to_vec(&objects) {
        Ok(bytes) => format!("{:x}", Sha256::digest(bytes)),
        Err(error) => return invalid(error.to_string()),
    };
    let status = if objects.is_empty() {
        EditorFrameStatus::Unannotated
    } else {
        EditorFrameStatus::Supported
    };
    let metadata = EditorFrameMetadata {
        frame_index,
        seek_serial,
        video_width: geometry.video_width,
        video_height: geometry.video_height,
        editor_index_digest,
        frame_geometry_digest,
        status,
        reason: None,
        objects,
    };
    if metadata
        .validate_for_frame(frame_index, seek_serial)
        .is_err()
        || serde_json::to_vec(&metadata)
            .is_ok_and(|bytes| bytes.len() > MAX_EDITOR_METADATA_BYTES - 4096)
    {
        return invalid("editor metadata failed validation or exceeds the 1 MiB limit".into());
    }
    metadata
}

fn duplicate_editor_scene_keys(scenes: &[fframes::SceneReport]) -> HashSet<String> {
    let mut counts = HashMap::<&str, usize>::new();
    for key in scenes
        .iter()
        .filter_map(|scene| scene.editor_instance_key.as_deref())
    {
        *counts.entry(key).or_default() += 1;
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(key, _)| key.to_owned())
        .collect()
}

fn preview_scene_infos(
    scenes: Vec<fframes::SceneReport>,
    identity: &PreviewIdentity,
) -> Vec<PreviewSceneInfo> {
    let duplicate_keys = duplicate_editor_scene_keys(&scenes);
    scenes
        .into_iter()
        .map(|scene| {
            let editor_instance_key = scene
                .editor_instance_key
                .filter(|key| !duplicate_keys.contains(key));
            let instance_id = editor_instance_key
                .clone()
                .unwrap_or_else(|| format!("{}:scene:{}", identity.source_revision, scene.index));
            PreviewSceneInfo {
                instance_id,
                editor_instance_key,
                index: scene.index,
                name: scene.name,
                full_name: scene.full_name,
                start_frame: scene.start_frame,
                end_frame: scene.end_frame,
                start_seconds: scene.start_seconds,
                end_seconds: scene.end_seconds,
            }
        })
        .collect()
}

pub fn serve_preview_worker<V: Video>(
    video: &V,
    options: &RenderOptions,
    mut transport: WorkerTransport,
    config: PreviewWorkerConfig,
) -> Result<(), WorkerError> {
    fs::create_dir_all(&config.cache_directory)?;
    let mut previewer = Previewer::new(video, options)?;
    let mut renderer = CpuFrameRenderer::default();
    let mut artifacts = Artifacts {
        entries: VecDeque::new(),
    };
    // The reader owns only control bytes. Cancellation can interrupt mixing between
    // chunks without transferring the borrowed Video/media or blocking the UI.
    let input = std::mem::replace(&mut transport.control_in, Box::new(std::io::empty()));
    let (sender, receiver) = mpsc::sync_channel(2);
    let cancel = Arc::new(AtomicU64::new(0));
    let signal = cancel.clone();
    let identity = config.identity.clone();
    std::thread::spawn(move || {
        let mut reader = WorkerTransport::new(input, std::io::sink(), std::io::sink());
        let mut previous = 0;
        loop {
            let message = reader.read_control_message::<PreviewRequest>();
            let mut shutdown = false;
            if let Ok(Some(request)) = &message {
                let (id, envelope) = request_identity(request);
                if id > previous {
                    previous = id;
                    if matches!(
                        request,
                        PreviewRequest::Shutdown(_) | PreviewRequest::CancelAudio(_)
                    ) && envelope.is_some_and(|e| e.validate(&identity).is_ok())
                    {
                        signal.store(id, Ordering::Release);
                        shutdown = matches!(request, PreviewRequest::Shutdown(_));
                    }
                }
            }
            let eof = !matches!(message, Ok(Some(_)));
            if eof {
                signal.store(u64::MAX, Ordering::Release);
            }
            let done = eof || shutdown;
            if sender.send(message).is_err() || done {
                break;
            }
        }
    });
    let mut negotiated = false;
    let mut last_request = 0;
    while let Ok(message) = receiver.recv() {
        let Some(request) = message? else {
            break;
        };
        let (id, envelope) = request_identity(&request);
        if id == 0 || id <= last_request {
            transport.write_control_message(&error(
                envelope.cloned(),
                "STALE_REQUEST",
                "request IDs must increase without wrapping",
            ))?;
            continue;
        }
        last_request = id;
        if !matches!(request, PreviewRequest::Hello(_)) && !negotiated {
            transport.write_control_message(&error(
                envelope.cloned(),
                "HELLO_REQUIRED",
                "negotiate the preview contract first",
            ))?;
            continue;
        }
        match request {
            PreviewRequest::Hello(req) => {
                let gaps: Vec<_> = req
                    .required_capabilities
                    .iter()
                    .filter(|capability| {
                        !PREVIEW_CAPABILITIES.contains(&capability.as_str())
                            && !OPTIONAL_PREVIEW_CAPABILITIES.contains(&capability.as_str())
                    })
                    .cloned()
                    .collect();
                if !req.offered_versions.contains(&PREVIEW_CONTRACT_VERSION) || !gaps.is_empty() {
                    transport.write_control_message(&error(None,"INCOMPATIBLE_PREVIEW_BRIDGE",format!("install an SDK/bridge supporting preview contract {PREVIEW_CONTRACT_VERSION}; missing: {}",gaps.join(", "))))?;
                    continue;
                }
                negotiated = true;
                transport.write_control_message(&PreviewResponse::Hello(PreviewHelloResponse {
                    contract_version: PREVIEW_CONTRACT_VERSION,
                    supported_versions: vec![PREVIEW_CONTRACT_VERSION],
                    identity: config.identity.clone(),
                    request_id: req.request_id,
                    fframes_version: config.fframes_version.clone(),
                    runtime_version: env!("CARGO_PKG_VERSION").into(),
                    sdk_version: config.sdk_version.clone(),
                    backend: "cpu".into(),
                    capabilities: PREVIEW_CAPABILITIES
                        .iter()
                        .chain(OPTIONAL_PREVIEW_CAPABILITIES)
                        .map(|s| (*s).into())
                        .collect(),
                    capability_gaps: vec!["shader_preview".into()],
                    max_frame_bytes: MAX_FRAME_PAYLOAD_BYTES,
                    max_control_bytes: crate::worker::MAX_CONTROL_MESSAGE_SIZE,
                    max_preview_width: MAX_PREVIEW_WIDTH,
                    max_preview_height: MAX_PREVIEW_HEIGHT,
                    audio_sample_rates: vec![
                        8000, 16000, 22050, 24000, 44100, 48000, 88200, 96000, 192000,
                    ],
                    audio_sample_format: "f32le_stereo_interleaved".into(),
                }))?;
            }
            PreviewRequest::Timeline(env) => {
                if let Err(e) = env.validate(&config.identity) {
                    transport.write_control_message(&error(Some(env), "INVALID_ENVELOPE", e))?;
                    continue;
                }
                let report = previewer.timeline_report();
                let response = PreviewTimelineResponse {
                    envelope: env,
                    fps: report.fps,
                    width: report.width,
                    height: report.height,
                    total_frames: report.duration_frames,
                    duration_seconds: report.duration_seconds,
                    scenes: preview_scene_infos(report.scenes, &config.identity),
                    audio_tracks: report
                        .audio
                        .into_iter()
                        .map(|a| PreviewAudioTrackInfo {
                            file: a.file,
                            start_seconds: a.start_seconds,
                            end_seconds: a.end_seconds,
                            mix: TrackMixInfo {
                                gain_db: a.mix.gain_db,
                                pan: a.mix.pan,
                                fade_in: a.mix.fade_in,
                                fade_out: a.mix.fade_out,
                                offset: a.mix.offset,
                                voice: a.mix.voice,
                                duck_under_voice: a.mix.duck.is_some(),
                            },
                        })
                        .collect(),
                };
                transport.write_control_message(&PreviewResponse::Timeline(response))?
            }
            PreviewRequest::ScaledFrame(req) => {
                if let Err(e) = req.validate(&config.identity) {
                    transport.write_control_message(&error(
                        Some(req.envelope),
                        "INVALID_FRAME_REQUEST",
                        e,
                    ))?;
                    continue;
                }
                let report = previewer.timeline_report();
                if req.frame_index >= report.duration_frames {
                    transport.write_control_message(&error(
                        Some(req.envelope),
                        "FRAME_OUT_OF_RANGE",
                        "requested frame is outside timeline",
                    ))?;
                    continue;
                }
                let max_scale = (MAX_PREVIEW_WIDTH as f64 / report.width as f64)
                    .min(MAX_PREVIEW_HEIGHT as f64 / report.height as f64)
                    .min(1.);
                let scale = req.scale.min(max_scale);
                previewer.set_scale(scale);
                let started = Instant::now();
                match previewer.render_with_editor_geometry(req.frame_index, &mut renderer) {
                    Ok((frame, geometry)) => {
                        let expected = (frame.width as usize)
                            .checked_mul(frame.height as usize)
                            .and_then(|v| v.checked_mul(4));
                        if expected != Some(frame.pixels.len()) {
                            transport.write_control_message(&error(
                                Some(req.envelope),
                                "FRAME_GEOMETRY",
                                "invalid renderer stride/payload",
                            ))?;
                            continue;
                        }
                        let record = BinaryRecordHeader {
                            kind: BinaryRecordKind::FrameRgba8,
                            identity: config.identity.clone(),
                            request_id: req.envelope.request_id,
                            offset: 0,
                            payload_len: frame.pixels.len(),
                        };
                        let ambiguous_scene_keys = duplicate_editor_scene_keys(&report.scenes);
                        let editor_metadata = editor_frame_metadata(
                            geometry,
                            req.frame_index,
                            req.seek_serial,
                            report.width,
                            report.height,
                            &ambiguous_scene_keys,
                        );
                        let response = PreviewResponse::ScaledFrame(ScaledFrameResponse {
                            header: FrameHeader::new_straight_rgba(
                                &config.identity.source_revision,
                                config.identity.worker_generation,
                                req.envelope.request_id,
                                req.frame_index,
                                frame.width,
                                frame.height,
                            )?,
                            envelope: req.envelope,
                            frame_index: req.frame_index,
                            seek_serial: req.seek_serial,
                            scale,
                            render_duration_micros: started.elapsed().as_micros() as u64,
                            record: record.clone(),
                            editor_metadata: Some(editor_metadata),
                        });
                        transport.write_control_message(&response)?;
                        transport.write_binary_record(&record, &frame.pixels)?
                    }
                    Err(e) => transport.write_control_message(&error(
                        Some(req.envelope),
                        "RENDER_ERROR",
                        e,
                    ))?,
                }
            }
            PreviewRequest::Inspect(req) => {
                if let Err(e) = req.validate(&config.identity) {
                    transport.write_control_message(&error(
                        Some(req.envelope),
                        "INVALID_INSPECT_REQUEST",
                        e,
                    ))?;
                    continue;
                }
                let total = previewer.timeline_report().duration_frames;
                if req.frames.iter().any(|f| *f >= total) {
                    transport.write_control_message(&error(
                        Some(req.envelope),
                        "FRAME_OUT_OF_RANGE",
                        "inspection frame is outside timeline",
                    ))?;
                    continue;
                }
                let mut diagnostics = Vec::new();
                let mut truncated = false;
                for frame in req.frames {
                    match previewer.inspect(frame) {
                        Ok(report) => {
                            for d in report.diagnostics {
                                if diagnostics.len() == MAX_DIAGNOSTICS {
                                    truncated = true;
                                    break;
                                }
                                let severity = match d.severity {
                                    fframes::diagnostics::Severity::Info => {
                                        DiagnosticSeverity::Info
                                    }
                                    fframes::diagnostics::Severity::Warning => {
                                        DiagnosticSeverity::Warning
                                    }
                                    fframes::diagnostics::Severity::Error => {
                                        DiagnosticSeverity::Error
                                    }
                                };
                                diagnostics.push(PreviewDiagnostic {
                                    frame,
                                    severity,
                                    key: d.key,
                                    message: d.message.chars().take(4096).collect(),
                                })
                            }
                        }
                        Err(e) => diagnostics.push(PreviewDiagnostic {
                            frame,
                            severity: DiagnosticSeverity::Error,
                            key: "inspect_error".into(),
                            message: e.to_string().chars().take(4096).collect(),
                        }),
                    }
                    if truncated {
                        break;
                    }
                }
                transport.write_control_message(&PreviewResponse::Inspect(InspectResponse {
                    envelope: req.envelope,
                    diagnostics,
                    truncated,
                }))?
            }
            PreviewRequest::PrepareAudio(req) => {
                if let Err(e) = req.envelope.validate(&config.identity) {
                    transport.write_control_message(&error(
                        Some(req.envelope),
                        "INVALID_ENVELOPE",
                        e,
                    ))?;
                    continue;
                }
                if req.output_sample_rate < 8000 || req.output_sample_rate > 192000 {
                    transport.write_control_message(&error(
                        Some(req.envelope),
                        "INVALID_SAMPLE_RATE",
                        "sample rate must be 8000..192000",
                    ))?;
                    continue;
                }
                let report = previewer.timeline_report();
                let bytes =
                    audio_byte_count(report.duration_frames, report.fps, req.output_sample_rate);
                if let Err(e) = bytes.and_then(|bytes| artifacts.reserve(bytes)) {
                    transport.write_control_message(&error(
                        Some(req.envelope),
                        "AUDIO_CACHE_FULL",
                        e,
                    ))?;
                    continue;
                }
                match prepare_audio(&previewer, &config, &req, &cancel) {
                    Ok(a) => {
                        let descriptor = a.descriptor.clone();
                        artifacts.push(a);
                        transport
                            .write_control_message(&PreviewResponse::PreparedAudio(descriptor))?
                    }
                    Err(e) => transport.write_control_message(&error(
                        Some(req.envelope),
                        "AUDIO_PREPARE_ERROR",
                        e,
                    ))?,
                }
            }
            PreviewRequest::ReadAudio(req) => {
                let Some(a) = artifacts.get(&req.artifact_id) else {
                    transport.write_control_message(&error(
                        Some(req.envelope),
                        "UNKNOWN_ARTIFACT",
                        "audio artifact is not retained",
                    ))?;
                    continue;
                };
                if let Err(e) = req.validate(&config.identity, a.descriptor.byte_count) {
                    transport.write_control_message(&error(
                        Some(req.envelope),
                        "INVALID_AUDIO_READ",
                        e,
                    ))?;
                    continue;
                }
                let mut file = File::open(&a.path)?;
                file.seek(SeekFrom::Start(req.offset))?;
                let mut payload = vec![0; req.length];
                file.read_exact(&mut payload)?;
                let record = BinaryRecordHeader {
                    kind: BinaryRecordKind::AudioPcmF32Le,
                    identity: config.identity.clone(),
                    request_id: req.envelope.request_id,
                    offset: req.offset,
                    payload_len: payload.len(),
                };
                transport.write_control_message(&PreviewResponse::AudioRead(
                    AudioReadResponse {
                        envelope: req.envelope,
                        artifact_id: req.artifact_id,
                        offset: req.offset,
                        record: record.clone(),
                    },
                ))?;
                transport.write_binary_record(&record, &payload)?
            }
            PreviewRequest::CancelAudio(req) | PreviewRequest::ReleaseAudio(req) => {
                if let Err(e) = req.envelope.validate(&config.identity) {
                    transport.write_control_message(&error(
                        Some(req.envelope),
                        "INVALID_ENVELOPE",
                        e,
                    ))?;
                    continue;
                }
                let released = artifacts.remove(&req.artifact_id);
                transport.write_control_message(&PreviewResponse::Ack(PreviewAck {
                    envelope: req.envelope,
                    released,
                }))?
            }
            PreviewRequest::Shutdown(env) => {
                if let Err(e) = env.validate(&config.identity) {
                    transport.write_control_message(&error(Some(env), "INVALID_ENVELOPE", e))?;
                    continue;
                }
                transport.write_control_message(&PreviewResponse::Ack(PreviewAck {
                    envelope: env,
                    released: false,
                }))?;
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod editor_metadata_tests {
    use super::*;
    use fframes::{EditorGeometrySupport, EditorObjectGeometry, EditorObjectKey, EditorRect};

    fn identity() -> PreviewIdentity {
        PreviewIdentity {
            project_id: "project".into(),
            open_session: "session".into(),
            source_revision: "a".repeat(64),
            worker_generation: 1,
        }
    }

    fn geometry(object_count: usize, object_key_bytes: usize) -> fframes::EditorFrameGeometry {
        let objects = (0..object_count)
            .map(|index| {
                let suffix = format!("{index:04}");
                let object = if object_key_bytes > suffix.len() {
                    format!("{}{suffix}", "x".repeat(object_key_bytes - suffix.len()))
                } else {
                    suffix
                };
                EditorObjectGeometry {
                    key: EditorObjectKey::new("s", "c", object, "r").unwrap(),
                    style_tokens: Vec::new(),
                    parent: None,
                    source_anchor: None,
                    bounds: EditorRect {
                        x: 0.0,
                        y: 0.0,
                        width: 1.0,
                        height: 1.0,
                    },
                    paint_order: index as u32,
                    support: EditorGeometrySupport::ExactBounds,
                }
            })
            .collect();
        fframes::EditorFrameGeometry {
            video_width: 320,
            video_height: 180,
            objects,
        }
    }

    #[test]
    fn object_count_limit_is_reachable_with_compact_semantic_keys() {
        let metadata = editor_frame_metadata(
            Ok(geometry(MAX_EDITOR_OBJECTS_PER_FRAME, 4)),
            0,
            1,
            320,
            180,
            &HashSet::new(),
        );
        assert_eq!(metadata.status, EditorFrameStatus::Supported);
        assert_eq!(metadata.objects.len(), MAX_EDITOR_OBJECTS_PER_FRAME);
        assert!(serde_json::to_vec(&metadata).unwrap().len() <= MAX_EDITOR_METADATA_BYTES - 4096);
    }

    #[test]
    fn metadata_byte_limit_rejects_overflow_without_publishing_partial_objects() {
        let metadata = editor_frame_metadata(
            Ok(geometry(MAX_EDITOR_OBJECTS_PER_FRAME, 256)),
            0,
            1,
            320,
            180,
            &HashSet::new(),
        );
        assert_eq!(metadata.status, EditorFrameStatus::Invalid);
        assert!(metadata.objects.is_empty());
        assert!(metadata.reason.as_deref().unwrap().contains("1 MiB limit"));
        metadata.validate_for_frame(0, 1).unwrap();
    }

    #[test]
    fn duplicate_scene_keys_fall_back_to_unique_positional_timeline_ids() {
        let scene = |index| fframes::SceneReport {
            index,
            editor_instance_key: Some("duplicate".into()),
            name: format!("Scene {index}"),
            full_name: format!("video::scene_{index}"),
            start_frame: index * 10,
            end_frame: (index + 1) * 10,
            start_seconds: index as f32,
            end_seconds: (index + 1) as f32,
        };
        let preview = identity();
        let scenes = preview_scene_infos(vec![scene(0), scene(1)], &preview);

        assert_eq!(
            scenes[0].instance_id,
            format!("{}:scene:0", preview.source_revision)
        );
        assert_eq!(
            scenes[1].instance_id,
            format!("{}:scene:1", preview.source_revision)
        );
        assert!(
            scenes
                .iter()
                .all(|scene| scene.editor_instance_key.is_none())
        );
    }
}

fn request_identity(request: &PreviewRequest) -> (u64, Option<&PreviewEnvelope>) {
    let envelope = match request {
        PreviewRequest::Hello(r) => return (r.request_id, None),
        PreviewRequest::Timeline(e) | PreviewRequest::Shutdown(e) => e,
        PreviewRequest::ScaledFrame(r) => &r.envelope,
        PreviewRequest::Inspect(r) => &r.envelope,
        PreviewRequest::PrepareAudio(r) => &r.envelope,
        PreviewRequest::ReadAudio(r) => &r.envelope,
        PreviewRequest::CancelAudio(r) | PreviewRequest::ReleaseAudio(r) => &r.envelope,
    };
    (envelope.request_id, Some(envelope))
}

fn audio_byte_count(frames: usize, fps: usize, rate: u32) -> Result<u64, String> {
    if fps == 0 {
        return Err("invalid audio timebase".into());
    }
    let bytes = (frames as u128 * rate as u128).div_ceil(fps as u128) * 8;
    if bytes > MAX_PREPARED_AUDIO_BYTES as u128 {
        return Err("prepared audio exceeds 4 GiB bound".into());
    }
    Ok(bytes as u64)
}

fn prepare_audio<V: Video>(
    previewer: &Previewer<V>,
    config: &PreviewWorkerConfig,
    req: &PrepareAudioRequest,
    cancel: &AtomicU64,
) -> Result<Artifact, String> {
    let report = previewer.timeline_report();
    let rate = req.output_sample_rate as usize;
    let bytes = audio_byte_count(report.duration_frames, report.fps, req.output_sample_rate)?;
    let samples = bytes / 8;
    let timeline = previewer.resolved_timeline();
    let mut mixer = AudioMixer::new_rescaled(
        timeline.audio_map.as_ref(),
        previewer.options().audio_encoder_options.sample_rate,
        previewer.media(),
        rate,
        0..samples as usize,
        samples as usize,
        previewer.options().audio_mix,
    );
    if !mixer.missing_files().is_empty() {
        return Err(format!(
            "missing audio media: {}",
            mixer.missing_files().join(", ")
        ));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let id = format!("pcm-{nonce:x}");
    let temp = config.cache_directory.join(format!(".{id}.tmp"));
    let final_path = config.cache_directory.join(format!("{id}.pcm"));
    let mut file = File::create(&temp).map_err(|e| e.to_string())?;
    let result: Result<String, String> = (|| {
        let started = Instant::now();
        let mut hasher = Sha256::new();
        let mut offset = 0usize;
        while offset < samples as usize {
            if cancel.load(Ordering::Acquire) > req.envelope.request_id {
                return Err("audio preparation cancelled".into());
            }
            if started.elapsed() > config.preparation_deadline {
                return Err("audio preparation deadline exceeded".into());
            }
            let len = 4096.min(samples as usize - offset);
            let mut left = vec![0.; len];
            let mut right = vec![0.; len];
            mixer.render(offset, &mut left, &mut right);
            let mut chunk = Vec::with_capacity(len * 8);
            for (l, r) in left.into_iter().zip(right) {
                chunk.extend_from_slice(&l.to_le_bytes());
                chunk.extend_from_slice(&r.to_le_bytes())
            }
            file.write_all(&chunk).map_err(|e| e.to_string())?;
            hasher.update(&chunk);
            offset += len
        }
        if cancel.load(Ordering::Acquire) > req.envelope.request_id {
            return Err("audio preparation cancelled".into());
        }
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temp, &final_path).map_err(|e| e.to_string())?;
        Ok(format!("{:x}", hasher.finalize()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    let sha256 = result?;
    Ok(Artifact {
        descriptor: PreparedAudioDescriptor {
            envelope: req.envelope.clone(),
            artifact_id: id,
            sample_rate: req.output_sample_rate,
            channels: 2,
            sample_count: samples,
            byte_count: bytes,
            sha256,
            silent: report.audio.is_empty(),
        },
        path: final_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fframes::{
        AudioData, AudioMap, AudioTimestamp::Second, AudioTrack, Duration, DynamicMediaProvider,
        Frame, Svgr,
    };
    use std::{borrow::Cow, collections::HashMap};

    struct MixedVideo;
    impl Video for MixedVideo {
        const FPS: usize = 30;
        const WIDTH: usize = 2;
        const HEIGHT: usize = 2;
        fn duration(&self) -> Duration<'_> {
            Duration::Seconds(1.)
        }
        fn audio(&self) -> AudioMap<'_> {
            AudioMap::from([
                AudioTrack::new("tone", Second(0.)..Second(1.))
                    .gain_db(-3.)
                    .pan(-0.4)
                    .fade_in(0.13)
                    .fade_out(0.11)
                    .duck_under_voice(),
                AudioTrack::new("voice", Second(0.371)..Second(0.639))
                    .voice()
                    .offset(0.031)
                    .pan(0.6),
            ])
        }
        fn render_frame<'a>(&'a self, _: Frame, _: &fframes::FFramesContext<'a, '_>) -> Svgr<'a> {
            Svgr::empty()
        }
    }
    #[test]
    fn disk_chunks_match_core_stereo_mix_with_resampling_fades_ducking_and_offset() {
        let stereo = |left, right| {
            AudioData::Preloaded(fframes::media::PreloadedAudioData {
                samples: Cow::Owned(left),
                sample_rate: 44100,
                right: Some(Cow::Owned(right)),
            })
        };
        let media = DynamicMediaProvider::new(
            HashMap::from([
                (
                    "tone".into(),
                    stereo(
                        (0..44100).map(|i| (i as f32 * 0.07).sin() * 0.8).collect(),
                        vec![-0.37; 44100],
                    ),
                ),
                ("voice".into(), stereo(vec![0.61; 22050], vec![0.19; 22050])),
            ]),
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
            vec![],
        );
        let options = RenderOptions {
            media: Some(&media),
            ..Default::default()
        };
        let previewer = Previewer::new(&MixedVideo, &options).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let identity = PreviewIdentity {
            project_id: "p".into(),
            open_session: "s".into(),
            source_revision: "r".into(),
            worker_generation: 1,
        };
        let mut config = PreviewWorkerConfig::new(identity.clone(), "sdk", "1.1.0");
        config.cache_directory = directory.path().into();
        let request = PrepareAudioRequest {
            envelope: PreviewEnvelope {
                contract_version: 1,
                identity,
                request_id: 1,
            },
            output_sample_rate: 48000,
        };
        let artifact = prepare_audio(&previewer, &config, &request, &AtomicU64::new(0)).unwrap();
        assert_eq!(artifact.descriptor.sample_count, 48000);
        assert!(!artifact.descriptor.silent);
        let mut reference = AudioMixer::new_rescaled(
            previewer.resolved_timeline().audio_map.as_ref(),
            options.audio_encoder_options.sample_rate,
            Some(&media),
            48000,
            0..48000,
            48000,
            options.audio_mix,
        );
        let (left, right) = reference.render_all();
        assert!(left.iter().any(|s| s.abs() > 0.1));
        assert_ne!(left, right);
        let expected: Vec<u8> = left
            .iter()
            .zip(&right)
            .flat_map(|(l, r)| l.to_le_bytes().into_iter().chain(r.to_le_bytes()))
            .collect();
        let actual = fs::read(&artifact.path).unwrap();
        assert_eq!(
            actual, expected,
            "transport chunking must not restart limiter/mix state"
        );
        assert_eq!(
            artifact.descriptor.sha256,
            format!("{:x}", Sha256::digest(&actual))
        );
        let mut artifacts = Artifacts {
            entries: VecDeque::from([artifact]),
        };
        assert!(artifacts.remove(&artifacts.entries[0].descriptor.artifact_id.clone()));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}
