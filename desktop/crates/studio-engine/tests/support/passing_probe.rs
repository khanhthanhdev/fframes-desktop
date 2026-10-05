//! A minimal deterministic probe that makes `validate_candidate` produce a genuinely
//! passing report, for tests that need a task to reach `CandidateReady` the only way the
//! engine allows: through report application. Include with
//! `#[path = "support/passing_probe.rs"] mod passing_probe;`.
#![allow(dead_code)]

use fframes_studio_protocol::*;
use studio_engine::{
    PreviewFrame,
    candidate_validation::{
        AudioProbe, BuildIdentity, BuildOutcome, CandidateProbe, CapturedCandidate, PcmStats,
        ProbeError, ValidationReport, validate_candidate,
    },
};

pub const TOTAL_FRAMES: usize = 30;

fn identity(revision: &str) -> PreviewIdentity {
    PreviewIdentity {
        project_id: "p".into(),
        open_session: "s".into(),
        source_revision: revision.into(),
        worker_generation: 1,
    }
}

fn envelope(revision: &str) -> PreviewEnvelope {
    PreviewEnvelope {
        contract_version: PREVIEW_CONTRACT_VERSION,
        identity: identity(revision),
        request_id: 1,
    }
}

pub struct PassingProbe {
    revision: String,
}

impl PassingProbe {
    pub fn new(revision: &str) -> Self {
        Self {
            revision: revision.into(),
        }
    }
}

impl CandidateProbe for PassingProbe {
    fn timeline(&mut self) -> Result<PreviewTimelineResponse, ProbeError> {
        let scene = |i: usize, s: usize, e: usize| PreviewSceneInfo {
            instance_id: format!("scene-{i}"),
            index: i,
            name: format!("s{i}"),
            full_name: format!("s{i}"),
            start_frame: s,
            end_frame: e,
            start_seconds: s as f32 / 30.,
            end_seconds: e as f32 / 30.,
        };
        Ok(PreviewTimelineResponse {
            envelope: envelope(&self.revision),
            fps: 30,
            width: 4,
            height: 2,
            total_frames: TOTAL_FRAMES,
            duration_seconds: 1.,
            scenes: vec![scene(0, 0, 15), scene(1, 15, 30)],
            audio_tracks: vec![],
        })
    }
    fn inspect(&mut self, _frames: &[usize]) -> Result<InspectResponse, ProbeError> {
        Ok(InspectResponse {
            envelope: envelope(&self.revision),
            diagnostics: vec![],
            truncated: false,
        })
    }
    fn render(&mut self, index: usize) -> Result<PreviewFrame, ProbeError> {
        Ok(PreviewFrame {
            response: ScaledFrameResponse {
                envelope: envelope(&self.revision),
                frame_index: index,
                seek_serial: 0,
                scale: 1.,
                header: FrameHeader::new_straight_rgba(&self.revision, 1, 1, index, 4, 2).unwrap(),
                render_duration_micros: 1,
                record: BinaryRecordHeader {
                    kind: BinaryRecordKind::FrameRgba8,
                    identity: identity(&self.revision),
                    request_id: 1,
                    offset: 0,
                    payload_len: 32,
                },
            },
            pixels: vec![7; 32],
        })
    }
    fn audio(&mut self) -> Result<AudioProbe, ProbeError> {
        let samples = (TOTAL_FRAMES as u64 * 48_000).div_ceil(30);
        Ok(AudioProbe {
            descriptor: PreparedAudioDescriptor {
                envelope: envelope(&self.revision),
                artifact_id: "pcm-abc".into(),
                sample_rate: 48_000,
                channels: 2,
                sample_count: samples,
                byte_count: samples * 8,
                sha256: "a".repeat(64),
                silent: true,
            },
            stats: PcmStats {
                sample_frames: samples,
                ..Default::default()
            },
        })
    }
}

/// A build identity naming exactly `captured`.
pub fn build_identity(captured: &CapturedCandidate) -> BuildIdentity {
    BuildIdentity {
        key_digest: "k".repeat(64),
        source_revision: captured.candidate().revision().as_str().into(),
        sdk_id: "sdk".into(),
        compatibility_digest: "d".repeat(64),
        toolchain: "1.98.1".into(),
        target_triple: "x86_64-unknown-linux-gnu".into(),
        package: "video".into(),
        worker_target: "worker".into(),
        profile: "debug".into(),
        backend: "cpu".into(),
    }
}

/// A report that genuinely passes for `captured`.
pub fn passing_report(captured: &CapturedCandidate) -> ValidationReport {
    let mut probe = PassingProbe::new(captured.candidate().revision().as_str());
    let report = validate_candidate(
        captured,
        BuildOutcome::Built(build_identity(captured)),
        0,
        0,
        Some(&mut probe),
    );
    assert!(report.passed(), "{:?}", report.failure());
    report
}
