//! Strict preview-protocol consumer over the supervised, bounded `WorkerClient` transport.
use crate::services::worker_client::{WorkerClient, WorkerClientError};
use crate::{PreviewFrame, validate_preview_timeline};
use fframes_studio_protocol::*;

pub struct PreviewWorkerClient {
    transport: WorkerClient,
    identity: PreviewIdentity,
    next_request: u64,
    hello: Option<PreviewHelloResponse>,
    timeline: Option<PreviewTimelineResponse>,
    /// Why the worker was stopped; later requests report it instead of a broken pipe.
    stopped: Option<String>,
}
fn invalid(message: impl Into<String>) -> WorkerClientError {
    WorkerClientError::Other(message.into())
}

fn scaled_dimensions(width: usize, height: usize, scale: f64) -> (u32, u32) {
    let scaled = |n: usize| {
        if scale == 1. {
            n as u32
        } else {
            ((n as f64 * scale / 2.).round() as u32 * 2).max(2)
        }
    };
    (scaled(width), scaled(height))
}

fn frame_geometry_matches(
    timeline: &PreviewTimelineResponse,
    scale: f64,
    dimensions: (u32, u32),
) -> bool {
    if scaled_dimensions(timeline.width, timeline.height, scale) == dimensions {
        return true;
    }
    // A tiny scale change can cross the renderer's even-pixel rounding boundary.
    // Accept only exact sizes produced by the same bounded JSON rounding allowance.
    (scale.to_bits().saturating_sub(4)..=scale.to_bits() + 4).any(|bits| {
        let candidate = f64::from_bits(bits);
        candidate > 0.
            && candidate <= 1.
            && scaled_dimensions(timeline.width, timeline.height, candidate) == dimensions
    })
}

/// The negotiation failed because the worker is not a compatible preview worker
/// (version, capability, backend or limits): an SDK/environment problem, not an edit.
pub const INCOMPATIBLE_BRIDGE: &str = "Incompatible preview SDK/worker bridge; source unchanged. Select an SDK with preview support and an explicit preview entry.";

/// The backends a preview worker may report: the built-in CPU renderer, Skia on the CPU,
/// Skia on a GPU.
pub const RUNTIME_BACKENDS: [&str; 3] = ["cpu", "skia-cpu", "skia-gpu"];

impl PreviewWorkerClient {
    pub fn new(transport: WorkerClient, identity: PreviewIdentity) -> Self {
        Self {
            transport,
            identity,
            next_request: 0,
            hello: None,
            timeline: None,
            stopped: None,
        }
    }
    /// Whether the worker was killed or exited: it missed a deadline, crashed or broke the
    /// protocol. A worker that answered a request with an error is still running.
    pub fn has_stopped(&self) -> bool {
        self.stopped.is_some() || self.transport.is_crashed()
    }
    /// Kills a worker whose connection can no longer be trusted and keeps the first reason.
    fn stop(&mut self, reason: &WorkerClientError) {
        let _ = self.transport.force_crash();
        self.stopped.get_or_insert_with(|| reason.to_string());
    }
    /// The backend the worker renders with now (`cpu`, `skia-cpu`, `skia-gpu`). Empty
    /// before negotiation.
    pub fn backend(&self) -> &str {
        self.hello.as_ref().map_or("", |h| h.backend.as_str())
    }
    /// What the backend runs on, or why the GPU is not used.
    pub fn backend_detail(&self) -> Option<&str> {
        self.hello
            .as_ref()
            .and_then(|h| h.backend_detail.as_deref())
    }
    /// A worker left its GPU for the CPU renderer during the session: later frames and
    /// gaps are the new backend's.
    fn apply_backend_change(&mut self, change: &BackendChange) -> Result<(), WorkerClientError> {
        if !RUNTIME_BACKENDS.contains(&change.backend.as_str())
            || change
                .backend_detail
                .as_ref()
                .is_some_and(|d| d.len() > MAX_BACKEND_DETAIL_BYTES)
        {
            let error = invalid("Invalid preview backend change");
            self.stop(&error);
            return Err(error);
        }
        if let Some(hello) = self.hello.as_mut() {
            hello.backend = change.backend.clone();
            hello.capability_gaps = change.capability_gaps.clone();
            hello.backend_detail = change.backend_detail.clone();
        }
        Ok(())
    }
    /// Capabilities the negotiated worker declared unavailable (for example
    /// `shader_preview`). Empty before negotiation.
    pub fn capability_gaps(&self) -> Vec<String> {
        self.hello
            .as_ref()
            .map(|h| h.capability_gaps.clone())
            .unwrap_or_default()
    }
    /// Whether the worker implements optional, frame-coupled semantic selection metadata.
    pub fn supports_editor_frames(&self) -> bool {
        self.hello.as_ref().is_some_and(|hello| {
            hello
                .capabilities
                .iter()
                .any(|capability| capability == "editor_frame_v1")
        })
    }
    /// Whether the worker accepts `SetTiming`. Workers built before it cannot parse the
    /// request at all, so callers check this first and keep the code order otherwise.
    pub fn supports_timing(&self) -> bool {
        self.hello.as_ref().is_some_and(|hello| {
            hello
                .capabilities
                .iter()
                .any(|capability| capability == SET_TIMING_CAPABILITY)
        })
    }
    /// Whether the worker accepts `SetProps` and `PropsInfo`: only workers whose project
    /// wired props hooks offer it.
    pub fn supports_props(&self) -> bool {
        self.hello.as_ref().is_some_and(|hello| {
            hello
                .capabilities
                .iter()
                .any(|capability| capability == SET_PROPS_CAPABILITY)
        })
    }
    pub fn identity(&self) -> &PreviewIdentity {
        &self.identity
    }

    /// Whether this worker can collect component-authored slide stops.
    pub fn supports_suggested_stops(&self) -> bool {
        self.hello.as_ref().is_some_and(|hello| {
            hello
                .capabilities
                .iter()
                .any(|cap| cap == SUGGESTED_STOPS_CAPABILITY)
        })
    }

    /// Samples one bounded batch without changing the displayed playhead.
    pub fn suggest_stops(
        &mut self,
        frames: Vec<usize>,
        timing_generation: u64,
        reset: bool,
    ) -> Result<StopSuggestions, WorkerClientError> {
        if !self.supports_suggested_stops()
            || frames.is_empty()
            || frames.len() > MAX_SUGGEST_FRAMES
            || self.timeline.as_ref().is_none_or(|timeline| {
                timeline.timing_generation != timing_generation
                    || frames.iter().any(|frame| *frame >= timeline.total_frames)
            })
        {
            return Err(invalid(
                "Slide-stop sampling is unsupported or outside the current timeline",
            ));
        }
        let envelope = self.envelope()?;
        let request = PreviewRequest::SuggestStops(SuggestStopsRequest {
            envelope: envelope.clone(),
            frames,
            timing_generation,
            reset,
        });
        match self.exchange(&request, &envelope, false)?.0 {
            PreviewResponse::SuggestedStops(mut response)
                if response.timing_generation == timing_generation =>
            {
                response.suggestions.bound();
                Ok(response.suggestions)
            }
            _ => Err(invalid("Slide-stop response has a stale timing generation")),
        }
    }
    pub fn process_id(&self) -> Option<u32> {
        self.transport.process_id()
    }
    /// Resolve only our own audio cache, hash its bounded regular file off the UI thread,
    /// and retain an open handle so worker retirement cannot invalidate a live reader.
    pub fn retain_audio_source(
        &self,
        audio: &PreparedAudioDescriptor,
        cancelled: impl Fn() -> bool,
    ) -> Result<std::sync::Arc<crate::PreparedAudioSource>, String> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let lease = self
            .transport
            .build_lease()
            .ok_or("Missing PCM build lease")?;
        let suffix = audio
            .artifact_id
            .strip_prefix("pcm-")
            .ok_or("Invalid PCM artifact id")?;
        if suffix.is_empty() || suffix.len() > 32 || !suffix.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("Invalid PCM artifact id".into());
        }
        let cache = lease.isolated_bin_dir.join("audio");
        let path = cache.join(format!("{}.pcm", audio.artifact_id));
        if std::fs::symlink_metadata(&cache)
            .map_err(|e| e.to_string())?
            .is_symlink()
            || std::fs::symlink_metadata(&path)
                .map_err(|e| e.to_string())?
                .is_symlink()
        {
            return Err("PCM cache must not contain symlinks".into());
        }
        let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file()
            || metadata.len() != audio.byte_count
            || metadata.len() > MAX_PREPARED_AUDIO_BYTES
        {
            return Err("Prepared PCM file length/type mismatch".into());
        }
        let mut buffer = [0u8; 65536];
        let mut hash = Sha256::new();
        // The hash pass reads every sample once; the waveform is folded in on the way.
        let mut peaks = crate::waveform::PeakAccumulator::new(audio.sample_rate, audio.channels);
        let mut remaining = audio.byte_count;
        while remaining > 0 {
            if cancelled() {
                return Err("PCM validation cancelled".into());
            }
            let length = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..length])
                .map_err(|e| e.to_string())?;
            for bytes in buffer[..length].as_chunks::<4>().0 {
                let sample = f32::from_le_bytes(*bytes);
                if !sample.is_finite() {
                    return Err("Prepared PCM contains nonfinite samples".into());
                }
                peaks.push(sample);
            }
            hash.update(&buffer[..length]);
            remaining -= length as u64;
        }
        if format!("{:x}", hash.finalize()) != audio.sha256.to_lowercase() {
            return Err("Prepared PCM checksum mismatch".into());
        }
        Ok(std::sync::Arc::new(
            crate::PreparedAudioSource::new(file, audio.clone(), lease)
                .with_waveform(peaks.finish()),
        ))
    }
    fn envelope(&mut self) -> Result<PreviewEnvelope, WorkerClientError> {
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or_else(|| invalid("Preview request counter exhausted"))?;
        Ok(PreviewEnvelope {
            contract_version: PREVIEW_CONTRACT_VERSION,
            identity: self.identity.clone(),
            request_id: self.next_request,
        })
    }
    fn exchange(
        &mut self,
        request: &PreviewRequest,
        e: &PreviewEnvelope,
        bulk: bool,
    ) -> Result<(PreviewResponse, Option<Vec<u8>>), WorkerClientError> {
        if let Some(reason) = &self.stopped {
            return Err(invalid(format!(
                "The preview worker was stopped after an error ({reason}); build the preview again"
            )));
        }
        let mut refused = false;
        let dimensions = match (request, &self.timeline) {
            (PreviewRequest::ScaledFrame(req), Some(t)) => {
                Some(scaled_dimensions(t.width, t.height, req.scale))
            }
            _ => None,
        };
        let result = self.transport.with_deadline(|t| {
            t.write_message(request)?;
            let mut response: PreviewResponse = t.read_message()?;
            let envelope = match &response {
                PreviewResponse::Timeline(r) => Some(&r.envelope),
                PreviewResponse::ScaledFrame(r) => Some(&r.envelope),
                PreviewResponse::Inspect(r) => Some(&r.envelope),
                PreviewResponse::PreparedAudio(r) => Some(&r.envelope),
                PreviewResponse::AudioRead(r) => Some(&r.envelope),
                PreviewResponse::Ack(r) => Some(&r.envelope),
                PreviewResponse::TimingApplied(r) => Some(&r.envelope),
                PreviewResponse::PropsApplied(r) => Some(&r.envelope),
                PreviewResponse::PropsInfo(r) => Some(&r.envelope),
                PreviewResponse::SuggestedStops(r) => Some(&r.envelope),
                PreviewResponse::Error(r) => r.envelope.as_ref(),
                PreviewResponse::Hello(_) => None,
            };
            if envelope != Some(e) {
                return Err(invalid("Preview response identity/request mismatch"));
            }
            if let PreviewResponse::Error(error) = &response {
                // A refusal of this request is a whole message: the worker keeps serving.
                refused = true;
                return Err(invalid(format!("{}: {}", error.code, error.message)));
            }
            if let (PreviewRequest::ScaledFrame(request), PreviewResponse::ScaledFrame(frame)) =
                (request, &mut response)
                && let Some(metadata) = &frame.editor_metadata
                && (metadata
                    .validate_for_frame(request.frame_index, request.seek_serial)
                    .is_err()
                    || self.timeline.as_ref().is_none_or(|timeline| {
                        metadata.video_width as usize != timeline.width
                            || metadata.video_height as usize != timeline.height
                    }))
            {
                frame.editor_metadata = Some(invalid_editor_metadata(
                    request.frame_index,
                    request.seek_serial,
                    self.timeline.as_ref().map_or(1, |timeline| timeline.width) as u32,
                    self.timeline.as_ref().map_or(1, |timeline| timeline.height) as u32,
                ));
            }
            match (request, &response) {
                (PreviewRequest::ScaledFrame(req), PreviewResponse::ScaledFrame(r)) => {
                    r.header.validate()?;
                    if r.frame_index != req.frame_index
                        || r.seek_serial != req.seek_serial
                        || !r.scale.is_finite()
                        || r.scale <= 0.
                        || r.scale > 1.
                        || r.scale.to_bits().abs_diff(req.scale.to_bits()) > 4
                        || self.timeline.as_ref().is_none_or(|timeline| {
                            !frame_geometry_matches(timeline, req.scale, (r.header.width, r.header.height))
                        })
                        || r.header.source_revision != e.identity.source_revision
                        || r.header.worker_generation != e.identity.worker_generation
                        || r.header.request_id != e.request_id
                        || r.header.frame_index != req.frame_index
                        || r.header.channel_order != ChannelOrder::Rgba8
                        || r.header.alpha_mode != AlphaMode::Straight
                        || r.header.width > MAX_PREVIEW_WIDTH
                        || r.header.height > MAX_PREVIEW_HEIGHT
                        || r.record.kind != BinaryRecordKind::FrameRgba8
                        || r.record.offset != 0
                        || r.record.payload_len != r.header.payload_len
                    {
                        return Err(invalid(format!(
                            "Frame geometry/seek/scale mismatch before payload allocation: expected dimensions {dimensions:?}, frame {}, seek {}, scale {}; received {}x{}, frame {}, seek {}, scale {}",
                            req.frame_index, req.seek_serial, req.scale,
                            r.header.width, r.header.height, r.frame_index, r.seek_serial, r.scale,
                        )));
                    }
                }
                (PreviewRequest::ReadAudio(req), PreviewResponse::AudioRead(r))
                    if r.artifact_id == req.artifact_id
                        && r.offset == req.offset
                        && r.record.offset == req.offset
                        && r.record.payload_len == req.length
                        && r.record.kind == BinaryRecordKind::AudioPcmF32Le => {}
                (PreviewRequest::Timeline(_), PreviewResponse::Timeline(_))
                | (PreviewRequest::Inspect(_), PreviewResponse::Inspect(_))
                | (PreviewRequest::PrepareAudio(_), PreviewResponse::PreparedAudio(_))
                | (PreviewRequest::ReleaseAudio(_), PreviewResponse::Ack(_))
                | (PreviewRequest::SetTiming(_), PreviewResponse::TimingApplied(_))
                | (PreviewRequest::SetProps(_), PreviewResponse::PropsApplied(_))
                | (PreviewRequest::PropsInfo(_), PreviewResponse::PropsInfo(_))
                | (PreviewRequest::SuggestStops(_), PreviewResponse::SuggestedStops(_))
                | (PreviewRequest::Shutdown(_), PreviewResponse::Ack(_)) => (),
                _ => return Err(invalid("Unexpected typed preview destination")),
            }
            if let (PreviewRequest::ScaledFrame(req), PreviewResponse::ScaledFrame(frame)) =
                (request, &mut response)
            {
                // Default JSON parsers can shift an f64 by a few ULPs across the
                // request/response round trip. Geometry was checked exactly above;
                // retain the requested scale for the playback identity checks too.
                frame.scale = req.scale;
            }
            let record = match &response {
                PreviewResponse::ScaledFrame(r) => Some(&r.record),
                PreviewResponse::AudioRead(r) => Some(&r.record),
                _ => None,
            };
            if bulk != record.is_some() {
                return Err(invalid("Unexpected preview bulk destination"));
            }
            let bytes = if let Some(r) = record {
                if r.identity != e.identity || r.request_id != e.request_id {
                    return Err(invalid("Preview bulk identity mismatch"));
                }
                Some(t.read_record(r)?)
            } else {
                None
            };
            Ok((response, bytes))
        });
        // Once framing is untrusted, never reuse a potentially half-drained connection.
        // A refusal that came after the deadline was discarded with the killed worker.
        if let Err(error) = &result
            && !(refused && !self.transport.is_crashed())
        {
            self.stop(error);
        }
        result
    }
    pub fn negotiate(&mut self) -> Result<PreviewHelloResponse, WorkerClientError> {
        let e = self.envelope()?;
        let id = &self.identity;
        let result = self.transport.with_deadline(|t| {
            t.write_message(&PreviewRequest::Hello(PreviewHelloRequest {
                offered_versions: vec![PREVIEW_CONTRACT_VERSION],
                required_capabilities: PREVIEW_CAPABILITIES.iter().map(|s| (*s).into()).collect(),
                request_id: e.request_id,
            }))?;
            match t.read_message::<PreviewResponse>()? {
                PreviewResponse::Hello(h)
                    if h.identity == *id
                        && h.request_id == e.request_id
                        && h.contract_version == PREVIEW_CONTRACT_VERSION
                        && h.supported_versions.contains(&PREVIEW_CONTRACT_VERSION)
                        && PREVIEW_CAPABILITIES
                            .iter()
                            .all(|s| h.capabilities.iter().any(|c| c == s))
                        && RUNTIME_BACKENDS.contains(&h.backend.as_str())
                        && h.backend_detail
                            .as_ref()
                            .is_none_or(|d| d.len() <= MAX_BACKEND_DETAIL_BYTES)
                        && h.max_frame_bytes <= MAX_FRAME_PAYLOAD_BYTES
                        && h.max_control_bytes <= 1024 * 1024
                        && h.max_preview_width == MAX_PREVIEW_WIDTH
                        && h.max_preview_height == MAX_PREVIEW_HEIGHT
                        && h.audio_sample_format == "f32le_stereo_interleaved" =>
                {
                    Ok(h)
                }
                _ => Err(invalid(INCOMPATIBLE_BRIDGE)),
            }
        });
        match result {
            Ok(h) => {
                self.hello = Some(h.clone());
                Ok(h)
            }
            Err(e) => {
                self.stop(&e);
                Err(e)
            }
        }
    }
    pub fn timeline(&mut self) -> Result<PreviewTimelineResponse, WorkerClientError> {
        if self.hello.is_none() {
            return Err(invalid("Preview hello required"));
        }
        let e = self.envelope()?;
        match self
            .exchange(&PreviewRequest::Timeline(e.clone()), &e, false)?
            .0
        {
            PreviewResponse::Timeline(t) => {
                validate_preview_timeline(&t).map_err(invalid)?;
                self.timeline = Some(t.clone());
                Ok(t)
            }
            _ => Err(invalid("Expected compiled timeline")),
        }
    }
    pub fn frame(
        &mut self,
        frame_index: usize,
        seek_serial: u64,
        scale: f64,
    ) -> Result<PreviewFrame, WorkerClientError> {
        let t = self
            .timeline
            .as_ref()
            .ok_or_else(|| invalid("Compiled timeline required"))?;
        if !scale.is_finite() || scale <= 0. || scale > 1. {
            return Err(invalid("Invalid preview scale"));
        }
        let scale = scale.min(
            (MAX_PREVIEW_WIDTH as f64 / t.width as f64)
                .min(MAX_PREVIEW_HEIGHT as f64 / t.height as f64)
                .min(1.),
        );
        if frame_index >= t.total_frames || !scale.is_finite() || scale <= 0. {
            return Err(invalid("Invalid preview frame/scale"));
        }
        let e = self.envelope()?;
        let request = PreviewRequest::ScaledFrame(ScaledFrameRequest {
            envelope: e.clone(),
            frame_index,
            seek_serial,
            scale,
        });
        let (r, bytes) = self.exchange(&request, &e, true)?;
        match r {
            PreviewResponse::ScaledFrame(r)
                if r.frame_index == frame_index
                    && r.seek_serial == seek_serial
                    && r.scale == scale =>
            {
                let f = PreviewFrame {
                    response: r,
                    pixels: bytes.unwrap(),
                };
                f.validate(&self.identity).map_err(invalid)?;
                if let Some(change) = &f.response.backend_change {
                    self.apply_backend_change(change)?;
                }
                Ok(f)
            }
            _ => {
                let error = invalid("Preview frame geometry/seek/scale mismatch");
                self.stop(&error);
                Err(error)
            }
        }
    }
    pub fn inspect(&mut self, frames: Vec<usize>) -> Result<InspectResponse, WorkerClientError> {
        let e = self.envelope()?;
        let request = PreviewRequest::Inspect(InspectRequest {
            envelope: e.clone(),
            frames,
        });
        match self.exchange(&request, &e, false)?.0 {
            PreviewResponse::Inspect(r) if r.diagnostics.len() <= MAX_DIAGNOSTICS => Ok(r),
            _ => Err(invalid("Invalid inspection response")),
        }
    }
    /// Applies a timing document (`None`: code order) and returns the timeline laid out
    /// with it. A rejected document leaves the worker's previous timing in place and
    /// returns the worker's reason. Prepared audio describes the previous layout, so the
    /// caller prepares it again.
    pub fn set_timing(
        &mut self,
        timing_json: Option<String>,
        timing_generation: u64,
    ) -> Result<PreviewTimelineResponse, WorkerClientError> {
        if !self.supports_timing() {
            return Err(invalid("Preview worker does not support timing documents"));
        }
        if timing_json
            .as_ref()
            .is_some_and(|json| json.len() > MAX_TIMING_DOC_BYTES)
        {
            return Err(invalid(format!(
                "Timing document exceeds {MAX_TIMING_DOC_BYTES} bytes"
            )));
        }
        let e = self.envelope()?;
        match self
            .exchange(
                &PreviewRequest::SetTiming(SetTimingRequest {
                    envelope: e.clone(),
                    timing_generation,
                    timing_json,
                }),
                &e,
                false,
            )?
            .0
        {
            PreviewResponse::TimingApplied(applied)
                if applied.timing_generation == timing_generation => {}
            _ => return Err(invalid("Expected timing acknowledgment")),
        }
        let timeline = self.timeline()?;
        if timeline.timing_generation != timing_generation {
            return Err(invalid("Timeline does not reflect the applied timing"));
        }
        Ok(timeline)
    }
    /// Applies a props document (`None`: the code's values) to the frames rendered from
    /// now on. A rejected document leaves the worker's previous props in place and returns
    /// the worker's reason. Props change no layout and no audio.
    pub fn set_props(
        &mut self,
        props_json: Option<String>,
        props_generation: u64,
    ) -> Result<(), WorkerClientError> {
        if !self.supports_props() {
            return Err(invalid("Preview worker does not support props documents"));
        }
        if props_json
            .as_ref()
            .is_some_and(|json| json.len() > MAX_PROPS_DOC_BYTES)
        {
            return Err(invalid(format!(
                "Props document exceeds {MAX_PROPS_DOC_BYTES} bytes"
            )));
        }
        let e = self.envelope()?;
        match self
            .exchange(
                &PreviewRequest::SetProps(SetPropsRequest {
                    envelope: e.clone(),
                    props_generation,
                    props_json,
                }),
                &e,
                false,
            )?
            .0
        {
            PreviewResponse::PropsApplied(applied)
                if applied.props_generation == props_generation =>
            {
                Ok(())
            }
            _ => Err(invalid("Expected props acknowledgment")),
        }
    }
    /// The schema and values of an editable object as it last rendered; `None` when it is
    /// not editable or has not rendered.
    pub fn props_info(
        &mut self,
        identity: EditorObjectIdentity,
    ) -> Result<PropsInfoResponse, WorkerClientError> {
        if !self.supports_props() {
            return Err(invalid("Preview worker does not support props documents"));
        }
        let e = self.envelope()?;
        match self
            .exchange(
                &PreviewRequest::PropsInfo(PropsInfoRequest {
                    envelope: e.clone(),
                    identity: identity.clone(),
                }),
                &e,
                false,
            )?
            .0
        {
            PreviewResponse::PropsInfo(info) if info.identity == identity => Ok(info),
            _ => Err(invalid("Expected props info")),
        }
    }
    pub fn prepare_audio(
        &mut self,
        rate: u32,
    ) -> Result<PreparedAudioDescriptor, WorkerClientError> {
        let e = self.envelope()?;
        self.transport
            .set_request_timeout(std::time::Duration::from_secs(125));
        let r = self.exchange(
            &PreviewRequest::PrepareAudio(PrepareAudioRequest {
                envelope: e.clone(),
                output_sample_rate: rate,
            }),
            &e,
            false,
        );
        self.transport
            .set_request_timeout(std::time::Duration::from_secs(30));
        match r?.0 {
            PreviewResponse::PreparedAudio(a)
                if a.sample_rate == rate
                    && a.channels == 2
                    && a.sample_count.checked_mul(8) == Some(a.byte_count)
                    && a.byte_count <= MAX_PREPARED_AUDIO_BYTES =>
            {
                Ok(a)
            }
            _ => Err(invalid("Invalid prepared audio descriptor")),
        }
    }
    pub fn read_audio(
        &mut self,
        a: &PreparedAudioDescriptor,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, WorkerClientError> {
        let e = self.envelope()?;
        let req = ReadAudioRequest {
            envelope: e.clone(),
            artifact_id: a.artifact_id.clone(),
            offset,
            length,
        };
        req.validate(&self.identity, a.byte_count)
            .map_err(|e| invalid(e.to_string()))?;
        if !offset.is_multiple_of(8) || !length.is_multiple_of(8) {
            return Err(invalid("Unaligned PCM window"));
        }
        let (r, bytes) = self.exchange(&PreviewRequest::ReadAudio(req), &e, true)?;
        match r {
            PreviewResponse::AudioRead(r)
                if r.artifact_id == a.artifact_id
                    && r.offset == offset
                    && r.record.kind == BinaryRecordKind::AudioPcmF32Le
                    && r.record.offset == offset
                    && r.record.payload_len == length =>
            {
                Ok(bytes.unwrap())
            }
            _ => {
                let error = invalid("Audio destination/offset mismatch");
                self.stop(&error);
                Err(error)
            }
        }
    }
    pub fn release_audio(&mut self, artifact_id: String) -> Result<(), WorkerClientError> {
        let e = self.envelope()?;
        match self
            .exchange(
                &PreviewRequest::ReleaseAudio(ArtifactRequest {
                    envelope: e.clone(),
                    artifact_id,
                }),
                &e,
                false,
            )?
            .0
        {
            PreviewResponse::Ack(_) => Ok(()),
            _ => Err(invalid("Expected release acknowledgment")),
        }
    }
    pub fn shutdown(&mut self) {
        let Ok(e) = self.envelope() else {
            return;
        };
        let _ = self.exchange(&PreviewRequest::Shutdown(e.clone()), &e, false);
    }
}

fn invalid_editor_metadata(
    frame_index: usize,
    seek_serial: u64,
    video_width: u32,
    video_height: u32,
) -> EditorFrameMetadata {
    let digest = "0".repeat(64);
    EditorFrameMetadata {
        frame_index,
        seek_serial,
        video_width,
        video_height,
        editor_index_digest: digest.clone(),
        frame_geometry_digest: digest,
        status: EditorFrameStatus::Invalid,
        reason: Some("worker returned invalid selection metadata".into()),
        objects: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use studio_bootstrap::{ChildEnvironment, ProcessTreeManager};

    #[test]
    fn malformed_destinations_geometry_seek_and_bulk_reap_without_waiting_for_payload() {
        let script = format!(
            "{}/tests/malformed-preview-worker.py",
            env!("CARGO_MANIFEST_DIR")
        );
        for case in [
            "envelope",
            "destination",
            "geometry",
            "seek",
            "bulk",
            "truncated",
        ] {
            let manager = ProcessTreeManager::new();
            let identity = PreviewIdentity {
                project_id: "project".into(),
                open_session: "session".into(),
                source_revision: "rev".into(),
                worker_generation: 7,
            };
            let header = serde_json::to_string(
                &FrameHeader::new_straight_rgba("rev", 7, 1, 29, 2, 2).unwrap(),
            )
            .unwrap();
            let mut transport = WorkerClient::new("rev", 7);
            transport.set_request_timeout(Duration::from_secs(5));
            transport
                .spawn_worker(
                    studio_test_support::python(),
                    &[&script, "--case", case, "--header", &header],
                    None,
                    ChildEnvironment::default_allowlist(),
                    &manager,
                )
                .unwrap();
            let mut worker = PreviewWorkerClient::new(transport, identity);
            worker.negotiate().unwrap();
            worker.timeline().unwrap();
            assert!(worker.frame(29, 7, f64::NAN).is_err());
            assert_eq!(
                manager.active_count(),
                1,
                "caller scale rejection must not send I/O"
            );
            let started = Instant::now();
            assert!(worker.frame(29, 7, 1.).is_err(), "malformed {case}");
            // Allow bounded process-tree cleanup while staying below the request deadline.
            assert!(
                started.elapsed() < Duration::from_secs(4),
                "{case} waited for an invalid payload"
            );
            assert_eq!(manager.active_count(), 0, "{case} leaked worker");
        }
    }
    fn timing_identity() -> PreviewIdentity {
        PreviewIdentity {
            project_id: "p".into(),
            open_session: "s".into(),
            source_revision: "rev".into(),
            worker_generation: 7,
        }
    }

    fn timing_envelope(request_id: u64) -> PreviewEnvelope {
        PreviewEnvelope {
            contract_version: PREVIEW_CONTRACT_VERSION,
            identity: timing_identity(),
            request_id,
        }
    }

    /// A negotiated client whose worker answers with `responses` (after Hello) in order.
    fn scripted_worker(timing: bool, responses: Vec<PreviewResponse>) -> PreviewWorkerClient {
        scripted_worker_on("cpu", timing, responses)
    }

    /// [`scripted_worker`] with a worker that renders with `backend`.
    fn scripted_worker_on(
        backend: &str,
        timing: bool,
        responses: Vec<PreviewResponse>,
    ) -> PreviewWorkerClient {
        let extra: &[&str] = if timing {
            &[SET_TIMING_CAPABILITY]
        } else {
            &[]
        };
        scripted_worker_with(backend, extra, responses)
    }

    /// [`scripted_worker`] with a worker that also offers `extra` capabilities.
    fn scripted_worker_with(
        backend: &str,
        extra: &[&str],
        responses: Vec<PreviewResponse>,
    ) -> PreviewWorkerClient {
        let capabilities: Vec<String> = PREVIEW_CAPABILITIES
            .iter()
            .chain(extra)
            .map(|c| (*c).into())
            .collect();
        let hello = PreviewResponse::Hello(PreviewHelloResponse {
            contract_version: PREVIEW_CONTRACT_VERSION,
            supported_versions: vec![PREVIEW_CONTRACT_VERSION],
            identity: timing_identity(),
            request_id: 1,
            fframes_version: "1.3.0".into(),
            runtime_version: "0.2.0".into(),
            sdk_version: "sdk".into(),
            backend: backend.into(),
            capabilities,
            capability_gaps: vec![],
            max_frame_bytes: MAX_FRAME_PAYLOAD_BYTES,
            max_control_bytes: 1024 * 1024,
            max_preview_width: MAX_PREVIEW_WIDTH,
            max_preview_height: MAX_PREVIEW_HEIGHT,
            audio_sample_rates: vec![48_000],
            audio_sample_format: "f32le_stereo_interleaved".into(),
            backend_detail: None,
        });
        let mut framed = Vec::new();
        let mut bulk = Vec::new();
        for response in std::iter::once(hello).chain(responses) {
            if let PreviewResponse::ScaledFrame(frame) = &response {
                let record = serde_json::to_vec(&frame.record).unwrap();
                bulk.extend((record.len() as u32).to_be_bytes());
                bulk.extend(record);
                bulk.extend(vec![0; frame.record.payload_len]);
            }
            let bytes = serde_json::to_vec(&response).unwrap();
            framed.extend((bytes.len() as u32).to_be_bytes());
            framed.extend(bytes);
        }
        let mut transport = WorkerClient::new("rev", 7);
        transport.attach_pipes(
            std::io::sink(),
            std::io::Cursor::new(framed),
            std::io::Cursor::new(bulk),
        );
        let mut worker = PreviewWorkerClient::new(transport, timing_identity());
        worker.negotiate().unwrap();
        worker
    }

    fn applied(request_id: u64, timing_generation: u64) -> PreviewResponse {
        PreviewResponse::TimingApplied(TimingAppliedResponse {
            envelope: timing_envelope(request_id),
            timing_generation,
            total_frames: 30,
            orphan_clips: vec![],
            appended_scenes: vec![],
        })
    }

    fn laid_out(request_id: u64, timing_generation: u64) -> PreviewResponse {
        PreviewResponse::Timeline(PreviewTimelineResponse {
            envelope: timing_envelope(request_id),
            fps: 10,
            width: 32,
            height: 18,
            total_frames: 30,
            duration_seconds: 3.,
            scenes: vec![],
            audio_tracks: vec![],
            timing_generation,
        })
    }

    fn scaled_frame(request_id: u64, scale: f64) -> PreviewResponse {
        let even = |size: f64| ((size * scale / 2.).round() as u32 * 2).max(2);
        let header =
            FrameHeader::new_straight_rgba("rev", 7, request_id, 0, even(32.), even(18.)).unwrap();
        PreviewResponse::ScaledFrame(ScaledFrameResponse {
            envelope: timing_envelope(request_id),
            frame_index: 0,
            seek_serial: 9,
            scale,
            record: BinaryRecordHeader {
                kind: BinaryRecordKind::FrameRgba8,
                identity: timing_identity(),
                request_id,
                offset: 0,
                payload_len: header.payload_len,
            },
            header,
            render_duration_micros: 1,
            editor_metadata: None,
            backend_change: None,
        })
    }

    #[test]
    fn fractional_frame_scales_survive_json_round_trips() {
        for scale in (2..4096).map(|n| 1. / n as f64) {
            // Both the request and the response travel through JSON, including workers
            // built with serde_json's default floating-point parser.
            let worker_scale: f64 =
                serde_json::from_str(&serde_json::to_string(&scale).unwrap()).unwrap();
            let mut worker =
                scripted_worker(true, vec![laid_out(2, 0), scaled_frame(3, worker_scale)]);
            worker.timeline().unwrap();
            let frame = worker.frame(0, 9, scale).unwrap();
            assert_eq!(frame.response.scale, scale);
            assert!(!worker.transport.is_crashed());
        }
    }

    #[test]
    fn viewport_scales_accept_dimensions_from_json_rounding() {
        let PreviewResponse::Timeline(mut timeline) = laid_out(2, 0) else {
            unreachable!()
        };
        timeline.width = 1920;
        timeline.height = 1080;
        for scale in (1..1280)
            .map(|n| n as f64 / 1920.)
            .chain((1..720).map(|n| n as f64 / 1080.))
        {
            let decoded: f64 =
                serde_json::from_str(&serde_json::to_string(&scale).unwrap()).unwrap();
            let geometry = |scale: f64| {
                [1920., 1080.].map(|size| ((size * scale / 2.).round() as u32 * 2).max(2))
            };
            let [width, height] = geometry(decoded);
            assert!(
                frame_geometry_matches(&timeline, scale, (width, height)),
                "scale {scale}, decoded {decoded}"
            );
        }
        // This viewport used to be refused: JSON turns the expected 20x10 into 18x10.
        let scale = 19. / 1920.;
        let decoded = serde_json::from_str(&serde_json::to_string(&scale).unwrap()).unwrap();
        let mut frame = scaled_frame(3, decoded);
        if let PreviewResponse::ScaledFrame(frame) = &mut frame {
            frame.header = FrameHeader::new_straight_rgba("rev", 7, 3, 0, 18, 10).unwrap();
            frame.record.payload_len = frame.header.payload_len;
        }
        let mut worker = scripted_worker(true, vec![PreviewResponse::Timeline(timeline), frame]);
        worker.timeline().unwrap();
        assert_eq!(worker.frame(0, 9, scale).unwrap().response.scale, scale);
    }

    #[test]
    fn frame_scale_tolerance_does_not_accept_other_scales_or_geometry() {
        for scale in [f64::from_bits(0.5_f64.to_bits() + 5), 0.500000001, 0.25] {
            let mut worker = scripted_worker(true, vec![laid_out(2, 0), scaled_frame(3, scale)]);
            worker.timeline().unwrap();
            assert!(worker.frame(0, 9, 0.5).is_err());
            assert!(worker.transport.is_crashed());
        }
        // A near-identical scale still cannot excuse an incorrect pixel size.
        let mut frame = scaled_frame(3, 0.5);
        if let PreviewResponse::ScaledFrame(frame) = &mut frame {
            frame.header = FrameHeader::new_straight_rgba("rev", 7, 3, 0, 16, 6).unwrap();
            frame.record.payload_len = frame.header.payload_len;
        }
        let mut worker = scripted_worker(true, vec![laid_out(2, 0), frame]);
        worker.timeline().unwrap();
        assert!(worker.frame(0, 9, 0.5).is_err());
        assert!(worker.transport.is_crashed());
    }

    #[test]
    fn skia_workers_negotiate_and_a_frame_that_switches_backend_updates_backend_and_gaps() {
        for backend in ["skia-gpu", "skia-cpu"] {
            assert_eq!(
                scripted_worker_on(backend, false, vec![]).backend(),
                backend
            );
        }
        let mut frame = scaled_frame(3, 0.5);
        if let PreviewResponse::ScaledFrame(frame) = &mut frame {
            frame.backend_change = Some(BackendChange {
                backend: "cpu".into(),
                capability_gaps: vec!["shader_preview".into()],
                backend_detail: Some("skia-gpu failed, using the CPU renderer".into()),
            });
        }
        let mut worker = scripted_worker_on("skia-gpu", false, vec![laid_out(2, 0), frame]);
        assert!(worker.capability_gaps().is_empty());
        worker.timeline().unwrap();
        worker.frame(0, 9, 0.5).unwrap();
        assert_eq!(worker.backend(), "cpu");
        assert_eq!(worker.capability_gaps(), ["shader_preview".to_owned()]);
        assert_eq!(
            worker.backend_detail(),
            Some("skia-gpu failed, using the CPU renderer")
        );
        assert!(!worker.has_stopped(), "the worker keeps serving frames");
    }

    #[test]
    fn a_worker_reporting_an_unknown_backend_is_incompatible() {
        let hello = |backend: &str| {
            let mut transport = WorkerClient::new("rev", 7);
            let response = PreviewResponse::Hello(PreviewHelloResponse {
                contract_version: PREVIEW_CONTRACT_VERSION,
                supported_versions: vec![PREVIEW_CONTRACT_VERSION],
                identity: timing_identity(),
                request_id: 1,
                fframes_version: "1.3.0".into(),
                runtime_version: "0.2.0".into(),
                sdk_version: "sdk".into(),
                backend: backend.into(),
                capabilities: PREVIEW_CAPABILITIES.iter().map(|c| (*c).into()).collect(),
                capability_gaps: vec![],
                max_frame_bytes: MAX_FRAME_PAYLOAD_BYTES,
                max_control_bytes: 1024 * 1024,
                max_preview_width: MAX_PREVIEW_WIDTH,
                max_preview_height: MAX_PREVIEW_HEIGHT,
                audio_sample_rates: vec![48_000],
                audio_sample_format: "f32le_stereo_interleaved".into(),
                backend_detail: None,
            });
            let bytes = serde_json::to_vec(&response).unwrap();
            let mut framed = (bytes.len() as u32).to_be_bytes().to_vec();
            framed.extend(bytes);
            transport.attach_pipes(
                std::io::sink(),
                std::io::Cursor::new(framed),
                std::io::Cursor::new(Vec::new()),
            );
            PreviewWorkerClient::new(transport, timing_identity()).negotiate()
        };
        assert!(hello("skia-gpu").is_ok());
        assert!(hello("vulkan").is_err());
    }

    #[test]
    fn rejected_timing_keeps_the_connection_and_previous_layout_for_frames() {
        let mut worker = scripted_worker(
            true,
            vec![
                laid_out(2, 4),
                PreviewResponse::Error(PreviewError {
                    envelope: Some(timing_envelope(3)),
                    code: "INVALID_TIMING".into(),
                    message: "trim exceeds scene length".into(),
                }),
                scaled_frame(4, 0.5),
            ],
        );
        worker.timeline().unwrap();
        assert!(worker.set_timing(Some("{}".into()), 5).is_err());
        assert!(!worker.transport.is_crashed());
        assert_eq!(worker.timeline.as_ref().unwrap().timing_generation, 4);
        worker.frame(0, 9, 0.5).unwrap();
    }

    #[test]
    fn set_timing_returns_the_timeline_of_the_applied_generation() {
        let mut worker = scripted_worker(true, vec![applied(2, 5), laid_out(3, 5)]);
        assert!(worker.supports_timing());
        let timeline = worker.set_timing(Some("{}".into()), 5).unwrap();
        assert_eq!(timeline.timing_generation, 5);
        // The cached timeline (used for frame requests) is the new layout too.
        assert_eq!(worker.timeline.as_ref().unwrap().timing_generation, 5);
    }

    #[test]
    fn set_timing_rejects_stale_generations_and_unsupported_workers() {
        let mut stale_ack = scripted_worker(true, vec![applied(2, 4)]);
        assert!(stale_ack.set_timing(None, 5).is_err());

        let mut stale_layout = scripted_worker(true, vec![applied(2, 5), laid_out(3, 4)]);
        assert!(stale_layout.set_timing(None, 5).is_err());

        // A worker without the capability cannot parse the request: nothing is sent.
        let mut old = scripted_worker(false, vec![]);
        assert!(!old.supports_timing());
        assert!(old.set_timing(None, 1).is_err());
        assert_eq!(old.next_request, 1);

        let mut bounded = scripted_worker(true, vec![]);
        let oversized = "x".repeat(MAX_TIMING_DOC_BYTES + 1);
        assert!(bounded.set_timing(Some(oversized), 1).is_err());
        assert_eq!(bounded.next_request, 1);
    }

    fn props_identity() -> EditorObjectIdentity {
        EditorObjectIdentity {
            scene_instance_key: "intro".into(),
            component_key: "Title".into(),
            object_key: "headline".into(),
            repeat_key: "main".into(),
        }
    }

    #[test]
    fn suggested_stops_check_capability_batches_generation_and_untrusted_bounds() {
        let mut old = scripted_worker(false, vec![]);
        assert!(!old.supports_suggested_stops());
        assert!(old.suggest_stops(vec![0], 0, true).is_err());
        assert_eq!(
            old.next_request, 1,
            "old workers receive no unrecognized request"
        );
        let response = |generation| {
            PreviewResponse::SuggestedStops(SuggestStopsResponse {
                envelope: timing_envelope(3),
                timing_generation: generation,
                suggestions: StopSuggestions {
                    stops: vec![SuggestedSlideStop {
                        scene: "intro".into(),
                        origin: "bullets".into(),
                        seconds: 1.0,
                        label: Some("x".repeat(257)),
                        notes: None,
                    }],
                    truncated: false,
                },
            })
        };
        let mut worker = scripted_worker_with(
            "cpu",
            &[SUGGESTED_STOPS_CAPABILITY],
            vec![laid_out(2, 4), response(4)],
        );
        worker.timeline().unwrap();
        for frames in [vec![], vec![0; 17], vec![30]] {
            assert!(worker.suggest_stops(frames, 4, true).is_err());
        }
        assert!(worker.suggest_stops(vec![0], 3, true).is_err());
        assert_eq!(
            worker.next_request, 2,
            "invalid inputs never cross the transport"
        );
        let result = worker.suggest_stops(vec![0, 15], 4, true).unwrap();
        assert!(
            result.truncated && result.stops.is_empty(),
            "engine bounds worker-authored text again"
        );
        let mut stale = scripted_worker_with(
            "cpu",
            &[SUGGESTED_STOPS_CAPABILITY],
            vec![laid_out(2, 4), response(3)],
        );
        stale.timeline().unwrap();
        assert!(stale.suggest_stops(vec![0], 4, true).is_err());
        assert!(
            !stale.has_stopped(),
            "stale suggestion generation is discarded without killing the worker"
        );
    }

    #[test]
    fn set_props_checks_the_capability_size_and_acknowledged_generation() {
        let applied = |request_id, props_generation| {
            PreviewResponse::PropsApplied(PropsAppliedResponse {
                envelope: timing_envelope(request_id),
                props_generation,
            })
        };
        let mut worker = scripted_worker_with("cpu", &[SET_PROPS_CAPABILITY], vec![applied(2, 3)]);
        assert!(worker.supports_props());
        worker.set_props(Some("{}".into()), 3).unwrap();

        let mut stale = scripted_worker_with("cpu", &[SET_PROPS_CAPABILITY], vec![applied(2, 2)]);
        assert!(stale.set_props(None, 3).is_err());

        // A worker without the capability cannot parse the request: nothing is sent.
        let mut old = scripted_worker(true, vec![]);
        assert!(!old.supports_props());
        assert!(old.set_props(None, 1).is_err());
        assert!(old.props_info(props_identity()).is_err());
        assert_eq!(old.next_request, 1);

        let mut bounded = scripted_worker_with("cpu", &[SET_PROPS_CAPABILITY], vec![]);
        let oversized = "x".repeat(MAX_PROPS_DOC_BYTES + 1);
        assert!(bounded.set_props(Some(oversized), 1).is_err());
        assert_eq!(bounded.next_request, 1);
    }

    #[test]
    fn props_info_answers_only_for_the_object_asked_about() {
        let info = |request_id, identity| {
            PreviewResponse::PropsInfo(PropsInfoResponse {
                envelope: timing_envelope(request_id),
                identity,
                props_generation: 0,
                info: None,
            })
        };
        let mut worker = scripted_worker_with(
            "cpu",
            &[SET_PROPS_CAPABILITY],
            vec![info(2, props_identity())],
        );
        let answer = worker.props_info(props_identity()).unwrap();
        assert_eq!(answer.info, None, "not editable or not rendered");

        let other = EditorObjectIdentity {
            object_key: "subtitle".into(),
            ..props_identity()
        };
        let mut confused =
            scripted_worker_with("cpu", &[SET_PROPS_CAPABILITY], vec![info(2, other)]);
        assert!(confused.props_info(props_identity()).is_err());
    }

    #[test]
    fn a_refused_request_keeps_the_worker_and_a_broken_reply_stops_it_with_its_reason() {
        let refusal = PreviewResponse::Error(PreviewError {
            envelope: Some(timing_envelope(2)),
            code: "INVALID_TIMING".into(),
            message: "trim too long".into(),
        });
        let mut worker = scripted_worker(true, vec![refusal, laid_out(3, 0)]);
        let refused = worker.set_timing(Some("{}".into()), 5).unwrap_err();
        assert!(refused.to_string().contains("INVALID_TIMING"));
        assert!(!worker.transport.is_crashed());
        assert_eq!(worker.timeline().unwrap().timing_generation, 0);

        // A reply to another request: framing is untrusted, so the worker is stopped and
        // later requests say why instead of writing to its closed pipe.
        let mut broken = scripted_worker(true, vec![laid_out(9, 0), laid_out(3, 0)]);
        assert!(broken.timeline().is_err());
        assert!(broken.transport.is_crashed());
        let later = broken.timeline().unwrap_err().to_string();
        assert!(
            later.contains("stopped") && later.contains("mismatch"),
            "{later}"
        );
    }

    #[test]
    fn request_counter_exhaustion_does_not_wrap() {
        let transport = WorkerClient::new("rev", 7);
        let mut worker = PreviewWorkerClient::new(
            transport,
            PreviewIdentity {
                project_id: "p".into(),
                open_session: "s".into(),
                source_revision: "rev".into(),
                worker_generation: 7,
            },
        );
        worker.next_request = u64::MAX;
        assert!(worker.envelope().is_err());
        assert_eq!(worker.next_request, u64::MAX);
    }
}
