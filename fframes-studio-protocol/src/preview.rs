//! Additive M2 preview protocol. The legacy v1 protocol remains in `lib.rs`.
use serde::{Deserialize, Serialize};
use std::fmt;

pub const PREVIEW_CONTRACT_VERSION: u32 = 1;
pub const MAX_PREVIEW_WIDTH: u32 = 1280;
pub const MAX_PREVIEW_HEIGHT: u32 = 720;
pub const MAX_INSPECT_FRAMES: usize = 256;
pub const MAX_DIAGNOSTICS: usize = 1024;
pub const MAX_AUDIO_READ_BYTES: usize = 256 * 1024;
pub const MAX_PREPARED_AUDIO_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const MAX_PREPARED_AUDIO_CACHE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const PREVIEW_CAPABILITIES: &[&str] = &[
    "preview_identity_v1",
    "scaled_frame_v1",
    "inspect_v1",
    "prepared_audio_v1",
];

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
