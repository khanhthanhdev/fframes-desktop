//! Strict M2 consumer using the same supervised bounded transport as the legacy client.
use crate::worker_client::{WorkerClient, WorkerClientError};
use fframes_studio_protocol::*;
use studio_engine::{PreviewFrame, validate_preview_timeline};

pub struct PreviewWorkerClient {
    transport: WorkerClient,
    identity: PreviewIdentity,
    next_request: u64,
    hello: Option<PreviewHelloResponse>,
    timeline: Option<PreviewTimelineResponse>,
}
fn invalid(message: impl Into<String>) -> WorkerClientError {
    WorkerClientError::Other(message.into())
}

impl PreviewWorkerClient {
    pub fn new(transport: WorkerClient, identity: PreviewIdentity) -> Self {
        Self {
            transport,
            identity,
            next_request: 0,
            hello: None,
            timeline: None,
        }
    }
    pub fn identity(&self) -> &PreviewIdentity {
        &self.identity
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
    ) -> Result<std::sync::Arc<studio_engine::PreparedAudioSource>, String> {
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
        let mut remaining = audio.byte_count;
        while remaining > 0 {
            if cancelled() {
                return Err("PCM validation cancelled".into());
            }
            let length = remaining.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..length])
                .map_err(|e| e.to_string())?;
            if buffer[..length]
                .as_chunks::<4>()
                .0
                .iter()
                .any(|b| !f32::from_le_bytes(*b).is_finite())
            {
                return Err("Prepared PCM contains nonfinite samples".into());
            }
            hash.update(&buffer[..length]);
            remaining -= length as u64;
        }
        if format!("{:x}", hash.finalize()) != audio.sha256.to_lowercase() {
            return Err("Prepared PCM checksum mismatch".into());
        }
        Ok(std::sync::Arc::new(
            studio_engine::PreparedAudioSource::new(file, audio.clone(), lease),
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
        let dimensions = match (request, &self.timeline) {
            (PreviewRequest::ScaledFrame(req), Some(t)) => {
                let scaled = |n: usize| {
                    if req.scale == 1. {
                        n as u32
                    } else {
                        ((n as f64 * req.scale / 2.).round() as u32 * 2).max(2)
                    }
                };
                Some((scaled(t.width), scaled(t.height)))
            }
            _ => None,
        };
        let result = self.transport.with_deadline(|t| {
            t.write_message(request)?;
            let response: PreviewResponse = t.read_message()?;
            let envelope = match &response {
                PreviewResponse::Timeline(r) => Some(&r.envelope),
                PreviewResponse::ScaledFrame(r) => Some(&r.envelope),
                PreviewResponse::Inspect(r) => Some(&r.envelope),
                PreviewResponse::PreparedAudio(r) => Some(&r.envelope),
                PreviewResponse::AudioRead(r) => Some(&r.envelope),
                PreviewResponse::Ack(r) => Some(&r.envelope),
                PreviewResponse::Error(r) => r.envelope.as_ref(),
                PreviewResponse::Hello(_) => None,
            };
            if envelope != Some(e) {
                return Err(invalid("Preview response identity/request mismatch"));
            }
            if let PreviewResponse::Error(error) = &response {
                return Err(invalid(format!("{}: {}", error.code, error.message)));
            }
            match (request, &response) {
                (PreviewRequest::ScaledFrame(req), PreviewResponse::ScaledFrame(r)) => {
                    r.header.validate()?;
                    if r.frame_index != req.frame_index
                        || r.seek_serial != req.seek_serial
                        || r.scale != req.scale
                        || dimensions != Some((r.header.width, r.header.height))
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
                        return Err(invalid(
                            "Frame geometry/seek/scale mismatch before payload allocation",
                        ));
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
                | (PreviewRequest::Shutdown(_), PreviewResponse::Ack(_)) => (),
                _ => return Err(invalid("Unexpected typed preview destination")),
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
        if result.is_err() {
            let _ = self.transport.force_crash();
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
                PreviewResponse::Hello(h) if h.identity == *id && h.request_id == e.request_id
                    && h.contract_version == PREVIEW_CONTRACT_VERSION
                    && h.supported_versions.contains(&PREVIEW_CONTRACT_VERSION)
                    && PREVIEW_CAPABILITIES.iter().all(|s| h.capabilities.iter().any(|c| c == s))
                    && h.backend == "cpu" && h.max_frame_bytes <= MAX_FRAME_PAYLOAD_BYTES
                    && h.max_control_bytes <= 1024 * 1024 && h.max_preview_width == MAX_PREVIEW_WIDTH
                    && h.max_preview_height == MAX_PREVIEW_HEIGHT
                    && h.audio_sample_format == "f32le_stereo_interleaved" => Ok(h),
                _ => Err(invalid("Incompatible preview SDK/worker bridge; source unchanged. Select an M2-capable SDK and explicit preview entry.")),
            }
        });
        match result {
            Ok(h) => {
                self.hello = Some(h.clone());
                Ok(h)
            }
            Err(e) => {
                let _ = self.transport.force_crash();
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
                Ok(f)
            }
            _ => {
                let _ = self.transport.force_crash();
                Err(invalid("Preview frame geometry/seek/scale mismatch"))
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
                let _ = self.transport.force_crash();
                Err(invalid("Audio destination/offset mismatch"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
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
            transport.set_request_timeout(Duration::from_secs(3));
            transport
                .spawn_worker(
                    Path::new(if cfg!(windows) { "python" } else { "python3" }),
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
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{case} waited for an invalid payload"
            );
            assert_eq!(manager.active_count(), 0, "{case} leaked worker");
        }
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
