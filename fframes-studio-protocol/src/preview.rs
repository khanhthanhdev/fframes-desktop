//! Additive M2 preview protocol. The legacy v1 protocol remains in `lib.rs`.
use serde::{Deserialize, Serialize};
use std::fmt;

pub const PREVIEW_CONTRACT_VERSION: u32 = 1;
pub const MAX_PREVIEW_WIDTH: u32 = 1280;
pub const MAX_PREVIEW_HEIGHT: u32 = 720;
pub const MAX_INSPECT_FRAMES: usize = 256;
pub const MAX_DIAGNOSTICS: usize = 1024;
pub const MAX_EDITOR_OBJECTS_PER_FRAME: usize = 4096;
pub const MAX_EDITOR_METADATA_BYTES: usize = 1024 * 1024;
pub const MAX_AUDIO_READ_BYTES: usize = 256 * 1024;
pub const MAX_PREPARED_AUDIO_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_PREPARED_AUDIO_CACHE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const PREVIEW_CAPABILITIES: &[&str] = &[
    "preview_identity_v1",
    "scaled_frame_v1",
    "inspect_v1",
    "prepared_audio_v1",
];
/// Additive preview features that older M2 workers may omit without losing playback.
pub const OPTIONAL_PREVIEW_CAPABILITIES: &[&str] = &["editor_frame_v1"];

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PreviewIdentity {
    pub project_id: String,
    pub open_session: String,
    pub source_revision: String,
    pub worker_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewEnvelope {
    pub contract_version: u32,
    pub identity: PreviewIdentity,
    pub request_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewHelloRequest {
    pub offered_versions: Vec<u32>,
    pub required_capabilities: Vec<String>,
    pub request_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewHelloResponse {
    pub contract_version: u32,
    pub supported_versions: Vec<u32>,
    pub identity: PreviewIdentity,
    pub request_id: u64,
    pub fframes_version: String,
    pub runtime_version: String,
    pub sdk_version: String,
    pub backend: String,
    pub capabilities: Vec<String>,
    pub capability_gaps: Vec<String>,
    pub max_frame_bytes: usize,
    pub max_control_bytes: usize,
    pub max_preview_width: u32,
    pub max_preview_height: u32,
    pub audio_sample_rates: Vec<u32>,
    pub audio_sample_format: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackMixInfo {
    pub gain_db: f32,
    pub pan: f32,
    pub fade_in: f32,
    pub fade_out: f32,
    pub offset: f32,
    pub voice: bool,
    pub duck_under_voice: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewSceneInfo {
    pub instance_id: String,
    /// Stable author-supplied identity, absent for legacy positional scenes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_instance_key: Option<String>,
    pub index: usize,
    pub name: String,
    pub full_name: String,
    pub start_frame: usize,
    pub end_frame: usize,
    pub start_seconds: f32,
    pub end_seconds: f32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewAudioTrackInfo {
    pub file: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub mix: TrackMixInfo,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewTimelineResponse {
    pub envelope: PreviewEnvelope,
    pub fps: usize,
    pub width: usize,
    pub height: usize,
    pub total_frames: usize,
    pub duration_seconds: f32,
    pub scenes: Vec<PreviewSceneInfo>,
    pub audio_tracks: Vec<PreviewAudioTrackInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScaledFrameRequest {
    pub envelope: PreviewEnvelope,
    pub frame_index: usize,
    pub seek_serial: u64,
    pub scale: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScaledFrameResponse {
    pub envelope: PreviewEnvelope,
    pub frame_index: usize,
    pub seek_serial: u64,
    pub scale: f64,
    pub header: super::FrameHeader,
    pub render_duration_micros: u64,
    pub record: BinaryRecordHeader,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_metadata: Option<EditorFrameMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditorObjectIdentity {
    pub scene_instance_key: String,
    pub component_key: String,
    pub object_key: String,
    pub repeat_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorGeometrySupport {
    ExactBounds,
    ApproximateBounds,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorObjectGeometry {
    pub identity: EditorObjectIdentity,
    pub parent: Option<EditorObjectIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_anchor: Option<EditorSourceAnchor>,
    #[serde(default)]
    pub style_tokens: Vec<String>,
    pub bounds: super::Rect,
    pub paint_order: u32,
    pub support: EditorGeometrySupport,
}

/// Optional author-registered source hint. It is not authorization to read a path;
/// consumers must resolve it inside the immutable project inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSourceAnchor {
    pub path: String,
    pub symbol: String,
    pub marker: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorFrameStatus {
    Supported,
    Unannotated,
    Invalid,
}

/// Frame-specific metadata published atomically with the preview image that produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorFrameMetadata {
    pub frame_index: usize,
    pub seek_serial: u64,
    pub video_width: u32,
    pub video_height: u32,
    /// Digest of the sorted semantic identity set for this frame.
    pub editor_index_digest: String,
    /// Digest of identity, geometry, support and traversal order for this frame.
    pub frame_geometry_digest: String,
    pub status: EditorFrameStatus,
    pub reason: Option<String>,
    pub objects: Vec<EditorObjectGeometry>,
}

impl EditorFrameMetadata {
    pub fn validate_for_frame(&self, frame_index: usize, seek_serial: u64) -> Result<(), String> {
        if self.frame_index != frame_index
            || self.seek_serial != seek_serial
            || self.video_width == 0
            || self.video_height == 0
            || self.objects.len() > MAX_EDITOR_OBJECTS_PER_FRAME
            || !valid_sha256(&self.editor_index_digest)
            || !valid_sha256(&self.frame_geometry_digest)
            || self
                .reason
                .as_ref()
                .is_some_and(|reason| reason.len() > 256 || reason.chars().any(char::is_control))
        {
            return Err("invalid editor frame identity, digest, or bounds".into());
        }
        if (self.status == EditorFrameStatus::Supported && self.objects.is_empty())
            || (self.status != EditorFrameStatus::Supported && !self.objects.is_empty())
            || (self.status == EditorFrameStatus::Invalid && self.reason.is_none())
            || (self.status != EditorFrameStatus::Invalid && self.reason.is_some())
        {
            return Err("inconsistent editor frame support status".into());
        }
        let mut identities = std::collections::HashSet::new();
        for object in &self.objects {
            if !valid_key(&object.identity.scene_instance_key)
                || !valid_key(&object.identity.component_key)
                || !valid_key(&object.identity.object_key)
                || !valid_key(&object.identity.repeat_key)
                || !identities.insert(&object.identity)
                || !object.bounds.x.is_finite()
                || !object.bounds.y.is_finite()
                || !object.bounds.width.is_finite()
                || !object.bounds.height.is_finite()
                || object.bounds.x < 0.0
                || object.bounds.y < 0.0
                || object.bounds.width < 0.0
                || object.bounds.height < 0.0
                || object.bounds.x + object.bounds.width > self.video_width as f32
                || object.bounds.y + object.bounds.height > self.video_height as f32
                || object
                    .parent
                    .as_ref()
                    .is_some_and(|parent| parent == &object.identity)
                || object.source_anchor.as_ref().is_some_and(|anchor| {
                    !valid_relative_path(&anchor.path)
                        || anchor.symbol.is_empty()
                        || anchor.symbol.len() > 512
                        || anchor.symbol.chars().any(char::is_control)
                        || anchor.marker.as_ref().is_some_and(|marker| {
                            marker.is_empty()
                                || marker.len() > 256
                                || marker.chars().any(char::is_control)
                        })
                })
                || object.style_tokens.len() > 64
                || object.style_tokens.iter().any(|token| {
                    token.is_empty() || token.len() > 128 || token.chars().any(char::is_control)
                })
                || object
                    .style_tokens
                    .iter()
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    != object.style_tokens.len()
            {
                return Err("invalid editor object key or geometry".into());
            }
        }
        let parent_by_identity: std::collections::HashMap<_, _> = self
            .objects
            .iter()
            .map(|object| (&object.identity, object.parent.as_ref()))
            .collect();
        if self.objects.iter().any(|object| {
            object
                .parent
                .as_ref()
                .is_some_and(|parent| !parent_by_identity.contains_key(parent))
        }) {
            return Err("editor object parent is absent from the frame".into());
        }
        for object in &self.objects {
            let mut cursor = object.parent.as_ref();
            let mut depth = 0;
            while let Some(parent) = cursor {
                depth += 1;
                if depth > 128 || parent == &object.identity {
                    return Err("editor object hierarchy is cyclic or too deep".into());
                }
                cursor = parent_by_identity.get(parent).and_then(|parent| *parent);
            }
        }
        Ok(())
    }
}

fn valid_key(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('/')
        && !value.chars().any(|ch| matches!(ch, '\\' | ':' | '\0'))
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

#[cfg(test)]
mod editor_metadata_tests {
    use super::*;
    use crate::{AlphaMode, CURRENT_PROTOCOL_VERSION, ChannelOrder, ColorSpace, FrameHeader, Rect};

    fn identity() -> PreviewIdentity {
        PreviewIdentity {
            project_id: "project".into(),
            open_session: "session".into(),
            source_revision: "a".repeat(64),
            worker_generation: 1,
        }
    }

    fn object(index: usize, parent: Option<usize>) -> EditorObjectGeometry {
        let object_identity = |index: usize| EditorObjectIdentity {
            scene_instance_key: "scene".into(),
            component_key: "component".into(),
            object_key: format!("object-{index}"),
            repeat_key: "primary".into(),
        };
        EditorObjectGeometry {
            identity: object_identity(index),
            parent: parent.map(object_identity),
            source_anchor: None,
            style_tokens: Vec::new(),
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            paint_order: index as u32,
            support: EditorGeometrySupport::ExactBounds,
        }
    }

    fn metadata(objects: Vec<EditorObjectGeometry>) -> EditorFrameMetadata {
        EditorFrameMetadata {
            frame_index: 3,
            seek_serial: 7,
            video_width: 100,
            video_height: 100,
            editor_index_digest: "b".repeat(64),
            frame_geometry_digest: "c".repeat(64),
            status: if objects.is_empty() {
                EditorFrameStatus::Unannotated
            } else {
                EditorFrameStatus::Supported
            },
            reason: None,
            objects,
        }
    }

    #[test]
    fn frame_metadata_accepts_object_and_hierarchy_limits() {
        let mut objects = Vec::with_capacity(MAX_EDITOR_OBJECTS_PER_FRAME);
        for index in 0..MAX_EDITOR_OBJECTS_PER_FRAME {
            let parent = if (1..=128).contains(&index) {
                Some(index - 1)
            } else {
                None
            };
            objects.push(object(index, parent));
        }
        metadata(objects).validate_for_frame(3, 7).unwrap();
    }

    #[test]
    fn frame_metadata_rejects_object_count_missing_parents_cycles_and_excess_depth() {
        let too_many = (0..=MAX_EDITOR_OBJECTS_PER_FRAME)
            .map(|index| object(index, None))
            .collect();
        assert!(
            metadata(too_many)
                .validate_for_frame(3, 7)
                .unwrap_err()
                .contains("bounds")
        );

        assert!(
            metadata(vec![object(0, Some(99))])
                .validate_for_frame(3, 7)
                .unwrap_err()
                .contains("parent is absent")
        );
        assert!(
            metadata(vec![object(0, Some(1)), object(1, Some(0))])
                .validate_for_frame(3, 7)
                .unwrap_err()
                .contains("cyclic or too deep")
        );

        let too_deep = (0..130)
            .map(|index| object(index, (index > 0).then(|| index - 1)))
            .collect();
        assert!(
            metadata(too_deep)
                .validate_for_frame(3, 7)
                .unwrap_err()
                .contains("cyclic or too deep")
        );
    }

    #[test]
    fn older_preview_payloads_without_editor_fields_remain_deserializable() {
        let mut scene = serde_json::to_value(PreviewSceneInfo {
            instance_id: "scene-0".into(),
            editor_instance_key: None,
            index: 0,
            name: "Scene".into(),
            full_name: "video::Scene".into(),
            start_frame: 0,
            end_frame: 1,
            start_seconds: 0.0,
            end_seconds: 1.0,
        })
        .unwrap();
        scene.as_object_mut().unwrap().remove("editor_instance_key");
        let restored_scene: PreviewSceneInfo = serde_json::from_value(scene).unwrap();
        assert_eq!(restored_scene.editor_instance_key, None);

        let response = ScaledFrameResponse {
            envelope: PreviewEnvelope {
                contract_version: PREVIEW_CONTRACT_VERSION,
                identity: identity(),
                request_id: 9,
            },
            frame_index: 3,
            seek_serial: 7,
            scale: 1.0,
            header: FrameHeader {
                protocol_version: CURRENT_PROTOCOL_VERSION,
                source_revision: "a".repeat(64),
                worker_generation: 1,
                request_id: 9,
                frame_index: 3,
                width: 1,
                height: 1,
                stride_bytes: 4,
                channel_order: ChannelOrder::Rgba8,
                alpha_mode: AlphaMode::Straight,
                color_space: ColorSpace::Srgb,
                payload_len: 4,
            },
            render_duration_micros: 1,
            record: BinaryRecordHeader {
                kind: BinaryRecordKind::FrameRgba8,
                identity: identity(),
                request_id: 9,
                offset: 0,
                payload_len: 4,
            },
            editor_metadata: None,
        };
        let mut old_response = serde_json::to_value(response).unwrap();
        old_response
            .as_object_mut()
            .unwrap()
            .remove("editor_metadata");
        let restored: ScaledFrameResponse = serde_json::from_value(old_response).unwrap();
        assert!(restored.editor_metadata.is_none());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectRequest {
    pub envelope: PreviewEnvelope,
    pub frames: Vec<usize>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewDiagnostic {
    pub frame: usize,
    pub severity: DiagnosticSeverity,
    pub key: String,
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InspectResponse {
    pub envelope: PreviewEnvelope,
    pub diagnostics: Vec<PreviewDiagnostic>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepareAudioRequest {
    pub envelope: PreviewEnvelope,
    pub output_sample_rate: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadAudioRequest {
    pub envelope: PreviewEnvelope,
    pub artifact_id: String,
    pub offset: u64,
    pub length: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRequest {
    pub envelope: PreviewEnvelope,
    pub artifact_id: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedAudioDescriptor {
    pub envelope: PreviewEnvelope,
    pub artifact_id: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_count: u64,
    pub byte_count: u64,
    pub sha256: String,
    pub silent: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioReadResponse {
    pub envelope: PreviewEnvelope,
    pub artifact_id: String,
    pub offset: u64,
    pub record: BinaryRecordHeader,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewAck {
    pub envelope: PreviewEnvelope,
    pub released: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewError {
    pub envelope: Option<PreviewEnvelope>,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryRecordKind {
    FrameRgba8,
    AudioPcmF32Le,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryRecordHeader {
    pub kind: BinaryRecordKind,
    pub identity: PreviewIdentity,
    pub request_id: u64,
    pub offset: u64,
    pub payload_len: usize,
}
impl BinaryRecordHeader {
    pub fn validate(&self) -> Result<(), PreviewProtocolError> {
        let cap = match self.kind {
            BinaryRecordKind::FrameRgba8 => super::MAX_FRAME_PAYLOAD_BYTES,
            BinaryRecordKind::AudioPcmF32Le => MAX_AUDIO_READ_BYTES,
        };
        if self.payload_len > cap {
            Err(PreviewProtocolError::PayloadTooLarge {
                cap,
                actual: self.payload_len,
            })
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PreviewRequest {
    Hello(PreviewHelloRequest),
    Timeline(PreviewEnvelope),
    ScaledFrame(ScaledFrameRequest),
    Inspect(InspectRequest),
    PrepareAudio(PrepareAudioRequest),
    ReadAudio(ReadAudioRequest),
    CancelAudio(ArtifactRequest),
    ReleaseAudio(ArtifactRequest),
    Shutdown(PreviewEnvelope),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PreviewResponse {
    Hello(PreviewHelloResponse),
    Timeline(PreviewTimelineResponse),
    ScaledFrame(ScaledFrameResponse),
    Inspect(InspectResponse),
    PreparedAudio(PreparedAudioDescriptor),
    AudioRead(AudioReadResponse),
    Ack(PreviewAck),
    Error(PreviewError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewProtocolError {
    Version,
    Identity,
    RequestId,
    InvalidScale,
    TooManyFrames,
    PayloadTooLarge { cap: usize, actual: usize },
    InvalidAudioRead,
}
impl fmt::Display for PreviewProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PreviewProtocolError {}
impl PreviewEnvelope {
    pub fn validate(&self, identity: &PreviewIdentity) -> Result<(), PreviewProtocolError> {
        if self.contract_version != PREVIEW_CONTRACT_VERSION {
            return Err(PreviewProtocolError::Version);
        }
        if &self.identity != identity {
            return Err(PreviewProtocolError::Identity);
        }
        if self.request_id == 0 {
            return Err(PreviewProtocolError::RequestId);
        }
        Ok(())
    }
}
impl ScaledFrameRequest {
    pub fn validate(&self, id: &PreviewIdentity) -> Result<(), PreviewProtocolError> {
        self.envelope.validate(id)?;
        if !self.scale.is_finite() || self.scale <= 0. || self.scale > 1. {
            return Err(PreviewProtocolError::InvalidScale);
        }
        Ok(())
    }
}
impl InspectRequest {
    pub fn validate(&self, id: &PreviewIdentity) -> Result<(), PreviewProtocolError> {
        self.envelope.validate(id)?;
        if self.frames.len() > MAX_INSPECT_FRAMES {
            return Err(PreviewProtocolError::TooManyFrames);
        }
        Ok(())
    }
}
impl ReadAudioRequest {
    pub fn validate(&self, id: &PreviewIdentity, total: u64) -> Result<(), PreviewProtocolError> {
        self.envelope.validate(id)?;
        if self.length > MAX_AUDIO_READ_BYTES
            || self
                .offset
                .checked_add(self.length as u64)
                .is_none_or(|v| v > total)
        {
            return Err(PreviewProtocolError::InvalidAudioRead);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> PreviewIdentity {
        PreviewIdentity {
            project_id: "p".into(),
            open_session: "s".into(),
            source_revision: "r".into(),
            worker_generation: 1,
        }
    }
    #[test]
    fn rejects_nan_scale_and_stale_identity() {
        let mut r = ScaledFrameRequest {
            envelope: PreviewEnvelope {
                contract_version: 1,
                identity: id(),
                request_id: 1,
            },
            frame_index: 0,
            seek_serial: 1,
            scale: f64::NAN,
        };
        assert_eq!(r.validate(&id()), Err(PreviewProtocolError::InvalidScale));
        r.scale = 0.5;
        r.envelope.identity.source_revision = "old".into();
        assert_eq!(r.validate(&id()), Err(PreviewProtocolError::Identity));
    }
    #[test]
    fn bounds_audio_records() {
        let h = BinaryRecordHeader {
            kind: BinaryRecordKind::AudioPcmF32Le,
            identity: id(),
            request_id: 1,
            offset: 0,
            payload_len: MAX_AUDIO_READ_BYTES + 1,
        };
        assert!(h.validate().is_err())
    }
}
