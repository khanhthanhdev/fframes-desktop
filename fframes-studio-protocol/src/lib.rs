use serde::{Deserialize, Serialize};
use std::fmt;

pub const CURRENT_PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_PAYLOAD_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelOrder {
    Rgba8,
    Bgra8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlphaMode {
    Straight,
    Premultiplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorSpace {
    Srgb,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameHeader {
    pub protocol_version: u32,
    pub source_revision: String,
    pub worker_generation: u64,
    pub request_id: u64,
    pub frame_index: usize,
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub channel_order: ChannelOrder,
    pub alpha_mode: AlphaMode,
    pub color_space: ColorSpace,
    pub payload_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    UnsupportedProtocolVersion { expected: u32, actual: u32 },
    ZeroDimensions { width: u32, height: u32 },
    StrideTooSmall { min_expected: u32, actual: u32 },
    IntegerOverflow,
    PayloadLengthMismatch { expected: usize, actual: usize },
    PayloadExceedsCap { cap: usize, actual: usize },
    StaleGeneration { expected: u64, actual: u64 },
    StaleRevision { expected: String, actual: String },
    JsonError(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProtocolVersion { expected, actual } => {
                write!(
                    f,
                    "unsupported protocol version: expected {expected}, got {actual}"
                )
            }
            Self::ZeroDimensions { width, height } => {
                write!(f, "zero dimensions: {width}x{height}")
            }
            Self::StrideTooSmall {
                min_expected,
                actual,
            } => {
                write!(
                    f,
                    "stride too small: min {min_expected} bytes, got {actual}"
                )
            }
            Self::IntegerOverflow => write!(f, "integer overflow during geometry calculation"),
            Self::PayloadLengthMismatch { expected, actual } => {
                write!(
                    f,
                    "payload length mismatch: expected {expected} bytes, got {actual}"
                )
            }
            Self::PayloadExceedsCap { cap, actual } => {
                write!(f, "payload exceeds max cap: cap {cap} bytes, got {actual}")
            }
            Self::StaleGeneration { expected, actual } => {
                write!(
                    f,
                    "stale worker generation: expected {expected}, got {actual}"
                )
            }
            Self::StaleRevision { expected, actual } => {
                write!(
                    f,
                    "stale source revision: expected '{expected}', got '{actual}'"
                )
            }
            Self::JsonError(err) => write!(f, "json protocol error: {err}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl FrameHeader {
    pub fn new_straight_rgba(
        source_revision: impl Into<String>,
        worker_generation: u64,
        request_id: u64,
        frame_index: usize,
        width: u32,
        height: u32,
    ) -> Result<Self, ProtocolError> {
        let min_stride = width.checked_mul(4).ok_or(ProtocolError::IntegerOverflow)?;
        let height_usize = usize::try_from(height).map_err(|_| ProtocolError::IntegerOverflow)?;
        let stride_usize =
            usize::try_from(min_stride).map_err(|_| ProtocolError::IntegerOverflow)?;
        let payload_len = height_usize
            .checked_mul(stride_usize)
            .ok_or(ProtocolError::IntegerOverflow)?;

        let header = Self {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            source_revision: source_revision.into(),
            worker_generation,
            request_id,
            frame_index,
            width,
            height,
            stride_bytes: min_stride,
            channel_order: ChannelOrder::Rgba8,
            alpha_mode: AlphaMode::Straight,
            color_space: ColorSpace::Srgb,
            payload_len,
        };
        header.validate()?;
        Ok(header)
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.protocol_version != CURRENT_PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedProtocolVersion {
                expected: CURRENT_PROTOCOL_VERSION,
                actual: self.protocol_version,
            });
        }
        if self.width == 0 || self.height == 0 {
            return Err(ProtocolError::ZeroDimensions {
                width: self.width,
                height: self.height,
            });
        }
        let min_stride = self
            .width
            .checked_mul(4)
            .ok_or(ProtocolError::IntegerOverflow)?;
        if self.stride_bytes < min_stride {
            return Err(ProtocolError::StrideTooSmall {
                min_expected: min_stride,
                actual: self.stride_bytes,
            });
        }

        let stride_usize =
            usize::try_from(self.stride_bytes).map_err(|_| ProtocolError::IntegerOverflow)?;
        let height_usize =
            usize::try_from(self.height).map_err(|_| ProtocolError::IntegerOverflow)?;
        let expected_payload = stride_usize
            .checked_mul(height_usize)
            .ok_or(ProtocolError::IntegerOverflow)?;

        if self.payload_len != expected_payload {
            return Err(ProtocolError::PayloadLengthMismatch {
                expected: expected_payload,
                actual: self.payload_len,
            });
        }

        if self.payload_len > MAX_FRAME_PAYLOAD_BYTES {
            return Err(ProtocolError::PayloadExceedsCap {
                cap: MAX_FRAME_PAYLOAD_BYTES,
                actual: self.payload_len,
            });
        }

        Ok(())
    }
}

// Control Plane Messages
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloRequest {
    pub protocol_version: u32,
    pub client_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResponse {
    pub protocol_version: u32,
    pub project_id: String,
    pub source_revision: String,
    pub worker_generation: u64,
    pub fframes_version: String,
    pub sdk_version: String,
    pub runtime_version: String,
    pub frame_contract_version: u32,
    pub max_frame_bytes: usize,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneInfo {
    pub id: String,
    pub name: String,
    pub start_frame: usize,
    pub frame_count: usize,
    pub duration_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioTrackInfo {
    pub name: String,
    pub source_path: String,
    pub start_second: f64,
    pub duration_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineRequest {
    pub protocol_version: u32,
    pub source_revision: String,
    pub worker_generation: u64,
    pub request_id: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineResponse {
    pub protocol_version: u32,
    pub source_revision: String,
    pub worker_generation: u64,
    pub request_id: u64,
    pub fps: f64,
    pub total_frames: usize,
    pub duration_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub scenes: Vec<SceneInfo>,
    pub audio_tracks: Vec<AudioTrackInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderFrameRequest {
    pub protocol_version: u32,
    pub source_revision: String,
    pub worker_generation: u64,
    pub request_id: u64,
    pub frame_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderFrameResponse {
    pub protocol_version: u32,
    pub source_revision: String,
    pub worker_generation: u64,
    pub request_id: u64,
    pub frame_index: usize,
    pub render_duration_micros: u64,
    pub header: FrameHeader,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElementMetadata {
    pub source_revision: String,
    pub scene_instance_id: String,
    pub element_id: String,
    pub instance_key: String,
    pub bounds: Rect,
    pub paint_order: u32,
    pub source_path: String,
    pub containing_symbol: String,
    pub source_hash: String,
    pub byte_start: usize,
    pub byte_end: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElementMetadataRequest {
    pub protocol_version: u32,
    pub source_revision: String,
    pub worker_generation: u64,
    pub request_id: u64,
    pub frame_index: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ElementMetadataResponse {
    pub protocol_version: u32,
    pub source_revision: String,
    pub worker_generation: u64,
    pub request_id: u64,
    pub elements: Vec<ElementMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownRequest {
    pub protocol_version: u32,
    pub source_revision: String,
    pub worker_generation: u64,
    pub request_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownResponse {
    pub protocol_version: u32,
    pub source_revision: String,
    pub worker_generation: u64,
    pub request_id: u64,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMessage {
    pub protocol_version: u32,
    pub source_revision: String,
    pub worker_generation: u64,
    pub request_id: Option<u64>,
    pub error_code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkerRequest {
    Hello(HelloRequest),
    Timeline(TimelineRequest),
    RenderFrame(RenderFrameRequest),
    ElementMetadata(ElementMetadataRequest),
    Shutdown(ShutdownRequest),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkerResponse {
    Hello(HelloResponse),
    Timeline(TimelineResponse),
    RenderFrame(RenderFrameResponse),
    ElementMetadata(ElementMetadataResponse),
    Shutdown(ShutdownResponse),
    Error(ErrorMessage),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_frame_header() {
        let header =
            FrameHeader::new_straight_rgba("rev1", 1, 42, 0, 1920, 1080).expect("valid header");
        assert_eq!(header.protocol_version, CURRENT_PROTOCOL_VERSION);
        assert_eq!(header.width, 1920);
        assert_eq!(header.height, 1080);
        assert_eq!(header.stride_bytes, 1920 * 4);
        assert_eq!(header.payload_len, 1920 * 1080 * 4);
        assert_eq!(header.channel_order, ChannelOrder::Rgba8);
        assert_eq!(header.alpha_mode, AlphaMode::Straight);
        assert_eq!(header.color_space, ColorSpace::Srgb);
        header.validate().expect("should validate");
    }

    #[test]
    fn test_zero_dimensions_rejected() {
        let err = FrameHeader {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            source_revision: "rev1".into(),
            worker_generation: 1,
            request_id: 1,
            frame_index: 0,
            width: 0,
            height: 1080,
            stride_bytes: 0,
            channel_order: ChannelOrder::Rgba8,
            alpha_mode: AlphaMode::Straight,
            color_space: ColorSpace::Srgb,
            payload_len: 0,
        }
        .validate()
        .unwrap_err();

        match err {
            ProtocolError::ZeroDimensions { width, height } => {
                assert_eq!(width, 0);
                assert_eq!(height, 1080);
            }
            other => panic!("expected ZeroDimensions, got {:?}", other),
        }
    }

    #[test]
    fn test_stride_too_small_rejected() {
        let err = FrameHeader {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            source_revision: "rev1".into(),
            worker_generation: 1,
            request_id: 1,
            frame_index: 0,
            width: 100,
            height: 100,
            stride_bytes: 300, // < 400
            channel_order: ChannelOrder::Rgba8,
            alpha_mode: AlphaMode::Straight,
            color_space: ColorSpace::Srgb,
            payload_len: 30000,
        }
        .validate()
        .unwrap_err();

        match err {
            ProtocolError::StrideTooSmall {
                min_expected,
                actual,
            } => {
                assert_eq!(min_expected, 400);
                assert_eq!(actual, 300);
            }
            other => panic!("expected StrideTooSmall, got {:?}", other),
        }
    }

    #[test]
    fn test_payload_length_mismatch_rejected() {
        let err = FrameHeader {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            source_revision: "rev1".into(),
            worker_generation: 1,
            request_id: 1,
            frame_index: 0,
            width: 10,
            height: 10,
            stride_bytes: 40,
            channel_order: ChannelOrder::Rgba8,
            alpha_mode: AlphaMode::Straight,
            color_space: ColorSpace::Srgb,
            payload_len: 399, // expected 400
        }
        .validate()
        .unwrap_err();

        match err {
            ProtocolError::PayloadLengthMismatch { expected, actual } => {
                assert_eq!(expected, 400);
                assert_eq!(actual, 399);
            }
            other => panic!("expected PayloadLengthMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_payload_cap_exceeded_rejected() {
        // Cap is 64 MiB. Let's create an 8000x3000 frame = 24M pixels * 4 = 96 MB > 64 MB
        let width = 8000;
        let height = 3000;
        let stride = width * 4;
        let payload_len = (stride * height) as usize;
        let err = FrameHeader {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            source_revision: "rev1".into(),
            worker_generation: 1,
            request_id: 1,
            frame_index: 0,
            width,
            height,
            stride_bytes: stride,
            channel_order: ChannelOrder::Rgba8,
            alpha_mode: AlphaMode::Straight,
            color_space: ColorSpace::Srgb,
            payload_len,
        }
        .validate()
        .unwrap_err();

        match err {
            ProtocolError::PayloadExceedsCap { cap, actual } => {
                assert_eq!(cap, MAX_FRAME_PAYLOAD_BYTES);
                assert_eq!(actual, payload_len);
            }
            other => panic!("expected PayloadExceedsCap, got {:?}", other),
        }
    }

    #[test]
    fn test_unsupported_version_rejected() {
        let err = FrameHeader {
            protocol_version: 999,
            source_revision: "rev1".into(),
            worker_generation: 1,
            request_id: 1,
            frame_index: 0,
            width: 10,
            height: 10,
            stride_bytes: 40,
            channel_order: ChannelOrder::Rgba8,
            alpha_mode: AlphaMode::Straight,
            color_space: ColorSpace::Srgb,
            payload_len: 400,
        }
        .validate()
        .unwrap_err();

        match err {
            ProtocolError::UnsupportedProtocolVersion { expected, actual } => {
                assert_eq!(expected, CURRENT_PROTOCOL_VERSION);
                assert_eq!(actual, 999);
            }
            other => panic!("expected UnsupportedProtocolVersion, got {:?}", other),
        }
    }

    #[test]
    fn test_serialization_roundtrip() {
        let header = FrameHeader::new_straight_rgba("rev_abc", 2, 99, 15, 640, 480).unwrap();
        let serialized = serde_json::to_string(&header).unwrap();
        let deserialized: FrameHeader = serde_json::from_str(&serialized).unwrap();
        assert_eq!(header, deserialized);
    }
}
