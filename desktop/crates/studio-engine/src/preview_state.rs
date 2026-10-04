//! Ephemeral preview readiness. Neither compilation nor checkpoint acceptance installs pixels.
use crate::{JobState, OperationTag, ProjectState, StateError};
use fframes_studio_protocol::*;
use std::{fs::File, sync::Arc};

pub const MAX_PCM_WINDOW_BYTES: usize = 1024 * 1024;

pub fn preview_identity(tag: &OperationTag) -> PreviewIdentity {
    PreviewIdentity {
        project_id: String::from(tag.project.clone()),
        open_session: uuid::Uuid::from_bytes(tag.session.0).to_string(),
        source_revision: tag.base_source.as_str().into(),
        worker_generation: tag.generation,
    }
}

/// Validate compiled metadata before any geometry, allocation or playback arithmetic.
pub fn validate_preview_timeline(t: &PreviewTimelineResponse) -> Result<(), String> {
    if t.fps == 0
        || t.width == 0
        || t.height == 0
        || !t.duration_seconds.is_finite()
        || t.duration_seconds < 0.
        || ((t.total_frames as f64 / t.fps as f64) - t.duration_seconds as f64).abs()
            > (1. / t.fps as f64).max(0.001)
    {
        return Err("Invalid compiled timeline timebase/dimensions".into());
    }
    let mut ids = std::collections::HashSet::new();
    for scene in &t.scenes {
        if scene.start_frame > scene.end_frame
            || scene.end_frame > t.total_frames
            || scene.instance_id.is_empty()
            || !ids.insert(&scene.instance_id)
            || !scene.start_seconds.is_finite()
            || !scene.end_seconds.is_finite()
            || scene.start_seconds < 0.
            || scene.end_seconds < scene.start_seconds
        {
            return Err("Invalid compiled scene identity/range".into());
        }
    }
    for a in &t.audio_tracks {
        if !a.start_seconds.is_finite()
            || !a.end_seconds.is_finite()
            || a.start_seconds < 0.
            || a.end_seconds < a.start_seconds
            || [
                a.mix.gain_db,
                a.mix.pan,
                a.mix.fade_in,
                a.mix.fade_out,
                a.mix.offset,
            ]
            .iter()
            .any(|v| !v.is_finite())
            || !(-1. ..=1.).contains(&a.mix.pan)
            || a.mix.fade_in < 0.
            || a.mix.fade_out < 0.
            || a.mix.offset < 0.
        {
            return Err("Invalid compiled audio range/mix".into());
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct PreviewFrame {
    pub response: ScaledFrameResponse,
    pub pixels: Vec<u8>,
}
impl PreviewFrame {
    pub fn validate(&self, identity: &PreviewIdentity) -> Result<(), String> {
        let r = &self.response;
        r.envelope.validate(identity).map_err(|e| e.to_string())?;
        r.header.validate().map_err(|e| e.to_string())?;
        r.record.validate().map_err(|e| e.to_string())?;
        if r.header.source_revision != identity.source_revision
            || r.header.worker_generation != identity.worker_generation
            || r.header.request_id != r.envelope.request_id
            || r.header.frame_index != r.frame_index
            || r.record.identity != *identity
            || r.record.request_id != r.envelope.request_id
            || r.record.kind != BinaryRecordKind::FrameRgba8
            || r.record.offset != 0
            || r.record.payload_len != r.header.payload_len
            || self.pixels.len() != r.header.payload_len
            || r.header.channel_order != ChannelOrder::Rgba8
            || r.header.alpha_mode != AlphaMode::Straight
            || r.header.width > MAX_PREVIEW_WIDTH
            || r.header.height > MAX_PREVIEW_HEIGHT
            || !r.scale.is_finite()
            || r.scale <= 0.
            || r.scale > 1.
        {
            return Err("Invalid preview frame identity/geometry/payload".into());
        }
        Ok(())
    }
}

/// A host-validated, open PCM file retaining its immutable materialization.
/// Readers use positioned reads; they never share a seek cursor or reopen a worker path.
pub struct PreparedAudioSource {
    pub file: File,
    pub descriptor: PreparedAudioDescriptor,
    _lease: Arc<crate::build_materialization::MaterializedBuild>,
}
impl PreparedAudioSource {
    pub fn build(&self) -> &Arc<crate::build_materialization::MaterializedBuild> {
        &self._lease
    }
    pub fn new(
        file: File,
        descriptor: PreparedAudioDescriptor,
        lease: Arc<crate::build_materialization::MaterializedBuild>,
    ) -> Self {
        Self {
            file,
            descriptor,
            _lease: lease,
        }
    }
}
impl std::fmt::Debug for PreparedAudioSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedAudioSource")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

/// One complete candidate; callers cannot construct a partially ready preview.
#[derive(Debug)]
pub struct ReadyPreview {
    tag: OperationTag,
    pub timeline: PreviewTimelineResponse,
    pub inspection: InspectResponse,
    pub frame: Option<PreviewFrame>,
    pub audio: PreparedAudioDescriptor,
    pub pcm_start_sample: u64,
    pub pcm: Vec<u8>,
    pub audio_source: Option<Arc<PreparedAudioSource>>,
    pub position: usize,
    pub seek_serial: u64,
}
impl ReadyPreview {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tag: OperationTag,
        timeline: PreviewTimelineResponse,
        inspection: InspectResponse,
        frame: Option<PreviewFrame>,
        audio: PreparedAudioDescriptor,
        pcm_start_sample: u64,
        pcm: Vec<u8>,
        position: usize,
        seek_serial: u64,
    ) -> Result<Self, String> {
        let id = preview_identity(&tag);
        validate_preview_timeline(&timeline)?;
        for e in [&timeline.envelope, &inspection.envelope, &audio.envelope] {
            e.validate(&id).map_err(|e| e.to_string())?;
        }
        if inspection.truncated
            || inspection
                .diagnostics
                .iter()
                .any(|d| d.severity == DiagnosticSeverity::Error)
        {
            return Err(
                "Critical or incomplete preview inspection; prior revision retained".into(),
            );
        }
        if position > timeline.total_frames || (timeline.total_frames == 0) != frame.is_none() {
            return Err("First frame does not match empty/nonempty timeline".into());
        }
        if let Some(f) = &frame {
            f.validate(&id)?;
            if f.response.seek_serial != seek_serial
                || f.response.frame_index != position.min(timeline.total_frames - 1)
            {
                return Err("First frame is from an obsolete playhead intent".into());
            }
        }
        let expected_samples = (timeline.total_frames as u128 * audio.sample_rate as u128)
            .div_ceil(timeline.fps as u128);
        if !(8000..=192000).contains(&audio.sample_rate)
            || audio.channels != 2
            || audio.sample_count as u128 != expected_samples
            || audio.sample_count.checked_mul(8) != Some(audio.byte_count)
            || audio.byte_count > MAX_PREPARED_AUDIO_BYTES
            || audio.artifact_id.is_empty()
            || audio.sha256.len() != 64
            || !audio.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || audio.silent != timeline.audio_tracks.is_empty()
            || pcm.len() > MAX_PCM_WINDOW_BYTES
            || pcm.len() as u64 > audio.sample_rate as u64 * 2 * 8
            || !pcm.len().is_multiple_of(8)
            || pcm_start_sample
                != (position as u128 * audio.sample_rate as u128 / timeline.fps as u128)
                    .min(audio.sample_count as u128) as u64
            || (pcm_start_sample < audio.sample_count && pcm.is_empty())
            || pcm_start_sample
                .checked_add(pcm.len() as u64 / 8)
                .is_none_or(|v| v > audio.sample_count)
            || pcm
                .as_chunks::<4>()
                .0
                .iter()
                .any(|b| !f32::from_le_bytes(*b).is_finite())
        {
            return Err("Invalid or mismatched prepared audio/window".into());
        }
        Ok(Self {
            tag,
            timeline,
            inspection,
            frame,
            audio,
            pcm_start_sample,
            pcm,
            audio_source: None,
            position,
            seek_serial,
        })
    }
    pub fn with_audio_source(mut self, source: Arc<PreparedAudioSource>) -> Result<Self, String> {
        if source.descriptor != self.audio {
            return Err("PCM file and prepared revision do not match".into());
        }
        self.audio_source = Some(source);
        Ok(self)
    }
    pub fn tag(&self) -> &OperationTag {
        &self.tag
    }
    pub fn identity(&self) -> &PreviewIdentity {
        &self.timeline.envelope.identity
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewStatus {
    Absent,
    Building,
    Preparing,
    Ready,
    Displayed,
    Error(String),
    Closed,
}

#[derive(Debug)]
pub struct PreviewState {
    pub status: PreviewStatus,
    displayed: Option<PreviewIdentity>,
    candidate: Option<OperationTag>,
    serial: u64,
    position: usize,
    scale: f64,
    total_frames: usize,
}
impl Default for PreviewState {
    fn default() -> Self {
        Self {
            status: PreviewStatus::Absent,
            displayed: None,
            candidate: None,
            serial: 0,
            position: 0,
            scale: 1.,
            total_frames: 0,
        }
    }
}
impl PreviewState {
    pub fn begin(&mut self, tag: OperationTag) {
        self.candidate = Some(tag);
        self.status = PreviewStatus::Building;
    }
    pub fn cancel_build(&mut self) {
        self.candidate = None;
        if self.status != PreviewStatus::Closed {
            self.status = if self.displayed.is_some() {
                PreviewStatus::Displayed
            } else {
                PreviewStatus::Absent
            };
        }
    }
    pub fn preparing(&mut self, tag: &OperationTag) -> Result<(), StateError> {
        if self.candidate.as_ref() != Some(tag) {
            return Err(StateError::StaleResult);
        }
        self.status = PreviewStatus::Preparing;
        Ok(())
    }
    pub fn displayed(&self) -> Option<&PreviewIdentity> {
        self.displayed.as_ref()
    }
    pub fn serial(&self) -> u64 {
        self.serial
    }
    pub fn position(&self) -> usize {
        self.position
    }
    pub fn scale(&self) -> f64 {
        self.scale
    }
    pub fn seek(&mut self, position: usize, scale: f64) -> Result<u64, StateError> {
        if !scale.is_finite() || scale <= 0. || scale > 1. || self.status == PreviewStatus::Closed {
            return Err(StateError::InvalidTransition);
        }
        self.serial = self
            .serial
            .checked_add(1)
            .ok_or(StateError::CounterExhausted)?;
        self.position = position.min(self.total_frames);
        self.scale = scale;
        Ok(self.serial)
    }
    pub fn can_install(
        &self,
        ready: &ReadyPreview,
        source: &ProjectState,
    ) -> Result<(), StateError> {
        let t = ready.tag();
        if self.candidate.as_ref() != Some(t)
            || t.project != *source.project()
            || t.session != *source.session()
            || t.base_source != *source.source()
            || t.generation != source.generation()
            || !matches!(source.job(), JobState::Succeeded(s) if s == t)
            || ready.seek_serial != self.serial
            || ready.position != self.position.min(ready.timeline.total_frames)
        {
            return Err(StateError::StaleResult);
        }
        Ok(())
    }
    /// The source owner must reconcile immediately before this guarded transition.
    pub fn install(
        &mut self,
        ready: &ReadyPreview,
        source: &ProjectState,
    ) -> Result<(), StateError> {
        self.can_install(ready, source)?;
        self.total_frames = ready.timeline.total_frames;
        self.position = ready.position;
        self.scale = ready.frame.as_ref().map_or(1., |f| f.response.scale);
        self.displayed = Some(ready.identity().clone());
        self.candidate = None;
        self.status = PreviewStatus::Displayed;
        Ok(())
    }
    /// Older displayed source is still a legitimate target while a new build runs/fails.
    pub fn accepts_frame(&self, f: &PreviewFrame) -> bool {
        self.displayed
            .as_ref()
            .is_some_and(|id| f.validate(id).is_ok())
            && f.response.seek_serial == self.serial
            && f.response.scale == self.scale
            && self.total_frames > 0
            && f.response.frame_index == self.position.min(self.total_frames - 1)
            && self.status != PreviewStatus::Closed
    }
    pub fn fail(&mut self, tag: &OperationTag, error: String) {
        if self.candidate.as_ref() == Some(tag) {
            self.candidate = None;
            self.status = PreviewStatus::Error(error);
        }
    }
    pub fn close(&mut self) {
        self.candidate = None;
        self.displayed = None;
        self.status = PreviewStatus::Closed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exhausted_seek_counter_preserves_position_and_rejects_wrapping() {
        let mut state = PreviewState {
            serial: u64::MAX,
            position: 13,
            total_frames: 60,
            ..Default::default()
        };
        assert_eq!(state.seek(29, 1.), Err(StateError::CounterExhausted));
        assert_eq!(state.position(), 13);
        assert_eq!(state.serial(), u64::MAX);
    }
}
