//! Immutable candidate capture and acceptance validation.
//!
//! A quiesced draft (proved by a [`CaptureTicket`]) is captured into content-addressed
//! checkpoint objects ([`capture_candidate`]): the draft is scanned, captured and
//! reloaded/verified, then scanned again; any difference fails the capture so a changing
//! tree is never validated. The resulting [`CapturedCandidate`] names the task, its
//! source base, the candidate revision and the manifest/object hashes.
//!
//! [`validate_candidate`] turns what an owned preview worker reports (through the
//! [`CandidateProbe`] trait) into an immutable [`ValidationReport`]: build identity,
//! diagnostics, coverage, representative frame hashes and prepared-audio identity. A
//! cached compiler success alone never passes: the report is `Passed` only when the
//! worker negotiated, the timeline is valid, no critical inspection error exists and
//! real frames and (checked) audio were produced. There is deliberately no pixel
//! difference or title-anchor requirement: an audio-only edit or an unchanged frame zero
//! is legitimate.
//!
//! Failure routing is deterministic ([`next_step`]): a source-attributable failure may
//! spend the single automatic repair of [`RepairBudget`]; environment failures never do.
use crate::{
    CandidateRevision, CaptureTicket, PreviewFrame, RepairBudget, TaskIdentity, TaskSourceBase,
    validate_preview_timeline,
};
use fframes_studio_protocol::{
    DiagnosticSeverity, InspectResponse, MAX_INSPECT_FRAMES, PreparedAudioDescriptor,
    PreviewTimelineResponse,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};
use studio_project::{
    Manifest, OpenProject, SourceRevision,
    checkpoint::Checkpoints,
    revision::{FileKind, SourceInventory},
};

/// Evenly spaced representative sample (initial maximum).
pub const MAX_SAMPLE_FRAMES: usize = 24;
/// Frames the validator renders and hashes (boundaries + playhead + sample).
pub const MAX_RENDERED_FRAMES: usize = 64;
/// Upper bound of a broadened (full-video) inspection; longer videos are strided and
/// reported as incomplete coverage rather than silently omitted.
pub const MAX_BROADENED_FRAMES: usize = 20_000;
pub const MAX_CHANGED_PATHS: usize = 256;
pub const MAX_REPORT_DIAGNOSTICS: usize = 128;
pub const MAX_FAILURE_PATHS: usize = 128;
pub const MAX_FAILURE_DIAGNOSTICS: usize = 64;
pub const MAX_COMPILER_OUTPUT_BYTES: usize = 16 * 1024;
pub const MAX_REPAIR_PROMPT_BYTES: usize = 24 * 1024;
const MAX_MESSAGE_BYTES: usize = 1024;
const MAX_ACTIVITY_RANGES: usize = 4096;
/// Peak below which a 10 ms bucket counts as silent (about -80 dBFS).
const SILENCE_PEAK: f32 = 1e-4;

// ---- capture ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CandidateError {
    #[error("the draft changed while it was being captured; retry after the writer is quiescent")]
    DraftChanged,
    #[error("candidate studio.json belongs to project {found}, not {expected}")]
    ProjectMismatch { expected: String, found: String },
    #[error("the draft is not a valid project: {0}")]
    InvalidDraft(String),
    #[error("the task source base is not available in history: {0}")]
    BaseMissing(String),
    #[error("candidate storage: {0}")]
    Storage(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    /// Same bytes, different executable bit.
    Mode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedPath {
    pub path: String,
    pub change: ChangeKind,
    pub kind: FileKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChangeSet {
    pub entries: Vec<ChangedPath>,
    /// Total changes, including entries beyond the bounded list.
    pub total: usize,
    pub truncated: bool,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// Diff of two verified inventories (both sorted by path).
    pub fn between(base: &SourceInventory, candidate: &SourceInventory) -> Self {
        let mut set = Self::default();
        let mut push = |path: &str, change: ChangeKind, kind: FileKind| {
            set.total += 1;
            if set.entries.len() < MAX_CHANGED_PATHS {
                set.entries.push(ChangedPath {
                    path: path.to_owned(),
                    change,
                    kind,
                });
            } else {
                set.truncated = true;
            }
        };
        let (mut a, mut b) = (
            base.files.iter().peekable(),
            candidate.files.iter().peekable(),
        );
        loop {
            match (a.peek(), b.peek()) {
                (None, None) => break,
                (Some(old), None) => {
                    push(old.path.as_str(), ChangeKind::Deleted, old.kind);
                    a.next();
                }
                (None, Some(new)) => {
                    push(new.path.as_str(), ChangeKind::Added, new.kind);
                    b.next();
                }
                (Some(old), Some(new)) => match old.path.cmp(&new.path) {
                    std::cmp::Ordering::Less => {
                        push(old.path.as_str(), ChangeKind::Deleted, old.kind);
                        a.next();
                    }
                    std::cmp::Ordering::Greater => {
                        push(new.path.as_str(), ChangeKind::Added, new.kind);
                        b.next();
                    }
                    std::cmp::Ordering::Equal => {
                        if old.sha256 != new.sha256 || old.size != new.size {
                            push(new.path.as_str(), ChangeKind::Modified, new.kind);
                        } else if old.executable != new.executable {
                            push(new.path.as_str(), ChangeKind::Mode, new.kind);
                        }
                        a.next();
                        b.next();
                    }
                },
            }
        }
        set
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRecord {
    pub path: String,
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}

/// Immutable bytes proposed by one task, retained by hash. The identities are the
/// capture's own: nothing here is the task base or an M1 checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapturedCandidate {
    identity: TaskIdentity,
    source_base: TaskSourceBase,
    candidate: CandidateRevision,
    manifest_sha256: String,
    objects: Vec<ObjectRecord>,
    changes: ChangeSet,
}

impl CapturedCandidate {
    pub fn identity(&self) -> &TaskIdentity {
        &self.identity
    }
    pub fn source_base(&self) -> &TaskSourceBase {
        &self.source_base
    }
    pub fn candidate(&self) -> &CandidateRevision {
        &self.candidate
    }
    /// SHA-256 of the immutable checkpoint manifest file.
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
    pub fn objects(&self) -> &[ObjectRecord] {
        &self.objects
    }
    pub fn changes(&self) -> &ChangeSet {
        &self.changes
    }
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(format!("{:x}", hash.finalize()));
        }
        hash.update(&buffer[..count]);
    }
}

/// Points of [`capture_candidate_observed`] where a test may mutate the draft to prove
/// that a changing tree fails capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturePhase {
    /// The draft was scanned and is about to be captured into objects.
    Scanned,
    /// Objects were captured and reloaded; the final verification scan is next.
    Captured,
}

/// Capture the quiesced draft as immutable checkpoint objects. Verifies twice (scan +
/// capture/reload, then a final scan) and retains manifest/object hashes. A tree that
/// changes at any point fails with [`CandidateError::DraftChanged`].
pub fn capture_candidate(
    ticket: &CaptureTicket,
    checkpoints: &Checkpoints,
) -> Result<CapturedCandidate, CandidateError> {
    capture_candidate_observed(ticket, checkpoints, &mut |_| {})
}

/// Scan, capture into checkpoint objects, reload by hash, then scan again. Any
/// difference is [`CandidateError::DraftChanged`]: a changing tree is never captured.
fn capture_verified(
    draft: &Path,
    checkpoints: &Checkpoints,
    observe: &mut dyn FnMut(CapturePhase),
) -> Result<(SourceRevision, SourceInventory), CandidateError> {
    let before =
        SourceInventory::scan(draft).map_err(|e| CandidateError::InvalidDraft(e.to_string()))?;
    observe(CapturePhase::Scanned);
    let revision = checkpoints.capture(draft).map_err(|e| {
        let text = e.to_string();
        // A file that vanished or shrank mid-capture is also a changing tree: compare
        // against what was scanned before deciding this is a storage problem.
        let moved = text.contains("changed while checkpointing")
            || SourceInventory::scan(draft).is_ok_and(|now| now.revision != before.revision);
        if moved {
            CandidateError::DraftChanged
        } else {
            CandidateError::Storage(text)
        }
    })?;
    if revision != before.revision {
        return Err(CandidateError::DraftChanged);
    }
    // Second verification: reload the manifest and every object by hash.
    let inventory = checkpoints
        .load(&revision)
        .map_err(|e| CandidateError::Storage(e.to_string()))?;
    observe(CapturePhase::Captured);
    let after =
        SourceInventory::scan(draft).map_err(|e| CandidateError::InvalidDraft(e.to_string()))?;
    if after.revision != revision || inventory.revision != revision {
        return Err(CandidateError::DraftChanged);
    }
    Ok((revision, inventory))
}

/// Capture a labelled immutable snapshot of `draft` (verified twice) without any task
/// ticket: used by read-only project tools while the writer gate is free.
pub fn capture_draft_revision(
    checkpoints: &Checkpoints,
    draft: &Path,
) -> Result<SourceRevision, CandidateError> {
    capture_verified(draft, checkpoints, &mut |_| {}).map(|(revision, _)| revision)
}

pub fn capture_candidate_observed(
    ticket: &CaptureTicket,
    checkpoints: &Checkpoints,
    observe: &mut dyn FnMut(CapturePhase),
) -> Result<CapturedCandidate, CandidateError> {
    capture_tree(
        ticket.identity(),
        ticket.source_base(),
        ticket.draft(),
        checkpoints,
        observe,
    )
}

/// Captures an Undo candidate: the current source with an inverse delta applied in a
/// private tree. There is no agent writer, ticket or lease; the identity is the Undo's
/// own (fresh task id, current session, a freshly allocated generation) and the source
/// base is the current source the inverse delta was formed from. The result is validated
/// through the same Stage 2 pipeline as any candidate.
pub fn capture_undo_candidate(
    identity: &TaskIdentity,
    source_base: &TaskSourceBase,
    tree: &Path,
    checkpoints: &Checkpoints,
) -> Result<CapturedCandidate, CandidateError> {
    capture_tree(identity, source_base, tree, checkpoints, &mut |_| {})
}

fn capture_tree(
    identity: &TaskIdentity,
    source_base: &TaskSourceBase,
    tree: &Path,
    checkpoints: &Checkpoints,
    observe: &mut dyn FnMut(CapturePhase),
) -> Result<CapturedCandidate, CandidateError> {
    let (revision, inventory) = capture_verified(tree, checkpoints, observe)?;
    let manifest_file = checkpoints.manifest_path(&revision);
    let manifest_sha256 =
        sha256_file(&manifest_file).map_err(|e| CandidateError::Storage(e.to_string()))?;
    let manifest_entry = inventory
        .files
        .iter()
        .find(|f| f.path.as_str() == "studio.json")
        .ok_or_else(|| CandidateError::InvalidDraft("studio.json is missing".into()))?;
    let manifest_bytes = std::fs::read(
        checkpoints
            .root
            .join("objects")
            .join(&manifest_entry.sha256),
    )
    .map_err(|e| CandidateError::Storage(e.to_string()))?;
    let manifest = Manifest::parse(&manifest_bytes)
        .map_err(|e| CandidateError::InvalidDraft(e.to_string()))?;
    if manifest.project_id != identity.project {
        return Err(CandidateError::ProjectMismatch {
            expected: String::from(identity.project.clone()),
            found: String::from(manifest.project_id),
        });
    }
    let base = checkpoints
        .load(source_base.revision())
        .map_err(|e| CandidateError::BaseMissing(e.to_string()))?;
    Ok(CapturedCandidate {
        identity: identity.clone(),
        source_base: source_base.clone(),
        candidate: CandidateRevision::new(revision),
        manifest_sha256,
        objects: inventory
            .files
            .iter()
            .map(|f| ObjectRecord {
                path: f.path.as_str().to_owned(),
                sha256: f.sha256.clone(),
                size: f.size,
                executable: f.executable,
            })
            .collect(),
        changes: ChangeSet::between(&base, &inventory),
    })
}

/// A private, immutable materialization of a captured revision, removed on drop. Build
/// requests keep it alive while compiling.
#[derive(Debug)]
pub struct RestoredRevision {
    pub project: OpenProject,
    root: PathBuf,
}

impl RestoredRevision {
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for RestoredRevision {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Restore `revision` (already present in `checkpoints`) into a private tree and open it
/// as a project. Objects are verified by the checkpoint store.
pub fn restore_revision(
    checkpoints: &Checkpoints,
    revision: &SourceRevision,
) -> Result<RestoredRevision, CandidateError> {
    let name = format!("candidate-{}", uuid::Uuid::new_v4().simple());
    let root = checkpoints
        .draft(revision, &name)
        .map_err(|e| CandidateError::Storage(e.to_string()))?;
    let guard_root = root.clone();
    match studio_project::open(&root) {
        Ok(project) => Ok(RestoredRevision { project, root }),
        Err(e) => {
            let _ = std::fs::remove_dir_all(guard_root);
            Err(CandidateError::InvalidDraft(e.to_string()))
        }
    }
}

// ---- report types ----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStage {
    Capture,
    Compile,
    Worker,
    Timeline,
    Inspection,
    Render,
    Audio,
    /// Required coverage exceeds what the bounded validator can inspect or render.
    Coverage,
}

/// Whether the agent could plausibly fix the failure by editing the draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Source,
    /// SDK, cancellation, cache or I/O problems: a repair turn cannot help.
    Environment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportDiagnostic {
    pub stage: ValidationStage,
    pub severity: DiagnosticSeverity,
    pub frame: Option<usize>,
    pub key: String,
    pub message: String,
}

/// Build identity of the compile that produced the validated bytes (from the shared
/// compile service's key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildIdentity {
    pub key_digest: String,
    pub source_revision: String,
    pub sdk_id: String,
    pub compatibility_digest: String,
    pub toolchain: String,
    pub target_triple: String,
    pub package: String,
    pub worker_target: String,
    pub profile: String,
    pub backend: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BroadenReason {
    /// Rust, Cargo, style or configuration changed: shared code may affect any scene.
    SharedSourceChanged,
    /// An existing file was deleted or modified (assets may be shared between scenes).
    ExistingFilesChanged,
    ChangeListTruncated,
    /// Added/changed files of an unclassified kind (fonts, data): their users are unknown.
    OtherFilesChanged,
    /// The compiled timeline names no scenes, or its scenes do not tile the video, so a
    /// scene-targeted sample cannot be trusted to reach every part of it.
    ScenesUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub total_frames: usize,
    pub boundary_frames: Vec<usize>,
    pub playhead: usize,
    pub sample_frames: Vec<usize>,
    pub rendered_frames: Vec<usize>,
    pub inspected_frames: usize,
    pub inspection_batches: usize,
    pub broadened: Option<BroadenReason>,
    /// False when the video was longer than the broadened bound or rendering was capped.
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameArtifact {
    pub frame: usize,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackCheck {
    pub file: String,
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// Non-silent PCM was found inside the track's window.
    pub audible: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioReport {
    pub artifact_id: String,
    pub sha256: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub sample_count: u64,
    pub silent: bool,
    pub peak: f32,
    pub clipped_samples: u64,
    pub tracks: Vec<TrackCheck>,
    /// True only when the streaming scan checked every active PCM bucket against exactly
    /// the track windows of the compiled timeline. False is never a pass.
    pub placement_verified: bool,
    /// False when the bounded activity map overflowed: `tracks[].audible` is then
    /// incomplete (never used for acceptance).
    pub audibility_complete: bool,
    /// Names of the local checks that passed.
    pub checks_passed: Vec<String>,
}

/// Bounded context given to the repair turn and to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureContext {
    pub stage: ValidationStage,
    pub kind: FailureKind,
    pub summary: String,
    /// Tail of the compiler/worker output (at most 16 KiB).
    pub output_tail: String,
    pub diagnostics: Vec<ReportDiagnostic>,
    pub changed_paths: Vec<String>,
    pub candidate: String,
    pub source_base: String,
    pub build_key: Option<String>,
    pub repair_count: u32,
}

impl FailureContext {
    /// Deterministic, bounded text for the automatic repair turn.
    pub fn repair_prompt(&self) -> String {
        let mut text = format!(
            "The previous edit failed validation at the {:?} stage (automatic repair attempt {} of 1).\n\nSummary: {}\n",
            self.stage,
            self.repair_count + 1,
            self.summary
        );
        if !self.changed_paths.is_empty() {
            text.push_str("\nChanged paths:\n");
            for path in &self.changed_paths {
                text.push_str("- ");
                text.push_str(path);
                text.push('\n');
            }
        }
        if !self.diagnostics.is_empty() {
            text.push_str("\nDiagnostics:\n");
            for d in &self.diagnostics {
                text.push_str(&format!(
                    "- [{:?}] frame {}: {}: {}\n",
                    d.severity,
                    d.frame.map_or_else(|| "-".to_owned(), |f| f.to_string()),
                    d.key,
                    d.message
                ));
            }
        }
        if !self.output_tail.is_empty() {
            text.push_str("\nOutput:\n");
            text.push_str(&self.output_tail);
            text.push('\n');
        }
        text.push_str("\nFix the problem in the working directory and finish the turn. Do not change the project id or SDK pin.\n");
        bound_tail(&text, MAX_REPAIR_PROMPT_BYTES, false)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Passed,
    Failed,
}

/// Local audio checks every passing report must have passed.
const REQUIRED_AUDIO_CHECKS: [&str; 8] = [
    "revision",
    "geometry",
    "duration",
    "pcm_length",
    "finite",
    "silent_flag",
    "track_bounds",
    "track_placement",
];

/// The immutable, revision-bound acceptance record of one candidate.
///
/// Only [`validate_candidate`] can construct one, and it is deliberately **not**
/// `Deserialize`: a report read back from storage or a wire is data for display, never
/// an acceptance proof. [`next_step`] and the controller re-check the evidence anyway.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationReport {
    task: TaskIdentity,
    source_base: TaskSourceBase,
    candidate: CandidateRevision,
    manifest_sha256: String,
    build: Option<BuildIdentity>,
    verdict: Verdict,
    /// Display buffer (bounded); acceptance never reads it.
    diagnostics: Vec<ReportDiagnostic>,
    diagnostics_total: usize,
    /// First error-severity diagnostics, retained regardless of the display bound.
    errors: Vec<ReportDiagnostic>,
    errors_total: usize,
    warnings: Vec<String>,
    capability_gaps: Vec<String>,
    coverage: Option<Coverage>,
    frames: Vec<FrameArtifact>,
    audio: Option<AudioReport>,
    changes: ChangeSet,
    repair_count: u32,
    failure: Option<FailureContext>,
}

impl ValidationReport {
    pub fn task(&self) -> &TaskIdentity {
        &self.task
    }
    pub fn source_base(&self) -> &TaskSourceBase {
        &self.source_base
    }
    pub fn candidate(&self) -> &CandidateRevision {
        &self.candidate
    }
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
    pub fn build(&self) -> Option<&BuildIdentity> {
        self.build.as_ref()
    }
    pub fn verdict(&self) -> Verdict {
        self.verdict
    }
    pub fn passed(&self) -> bool {
        self.verdict == Verdict::Passed
    }
    pub fn diagnostics(&self) -> &[ReportDiagnostic] {
        &self.diagnostics
    }
    pub fn diagnostics_total(&self) -> usize {
        self.diagnostics_total
    }
    /// Error-severity diagnostics (bounded list) kept independently of the display
    /// buffer, so a late critical error is never hidden by earlier warnings.
    pub fn errors(&self) -> &[ReportDiagnostic] {
        &self.errors
    }
    /// Count of every error-severity diagnostic seen, including beyond [`Self::errors`].
    pub fn errors_total(&self) -> usize {
        self.errors_total
    }
    /// Visible warnings (unsupported shaders, clipping, silence, partial coverage...).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub fn coverage(&self) -> Option<&Coverage> {
        self.coverage.as_ref()
    }
    pub fn frames(&self) -> &[FrameArtifact] {
        &self.frames
    }
    pub fn audio(&self) -> Option<&AudioReport> {
        self.audio.as_ref()
    }
    pub fn changes(&self) -> &ChangeSet {
        &self.changes
    }
    pub fn repair_count(&self) -> u32 {
        self.repair_count
    }
    /// Capabilities the preview worker declared it lacks (for example `shader_preview`).
    pub fn capability_gaps(&self) -> &[String] {
        &self.capability_gaps
    }
    pub fn failure(&self) -> Option<&FailureContext> {
        self.failure.as_ref()
    }

    /// Why this report cannot be accepted as complete evidence for its candidate, if it
    /// cannot: not passed, a failure recorded, an error seen, a build that does not name
    /// the candidate, incomplete required coverage, missing/mismatched frames or unproven
    /// audio checks. `None` means every acceptance requirement is evidenced.
    pub fn acceptance_gap(&self) -> Option<String> {
        if self.verdict != Verdict::Passed {
            return Some("the verdict is not Passed".into());
        }
        if self.failure.is_some() {
            return Some("a failure is recorded".into());
        }
        if self.errors_total > 0 || !self.errors.is_empty() {
            return Some(format!(
                "{} error diagnostic(s) were reported",
                self.errors_total
            ));
        }
        let Some(build) = &self.build else {
            return Some("no build identity".into());
        };
        let candidate = self.candidate.revision().as_str();
        if build.source_revision != candidate || build.backend != VALIDATED_BACKEND {
            return Some("the build identity does not name this candidate".into());
        }
        let Some(coverage) = &self.coverage else {
            return Some("no coverage".into());
        };
        if !coverage.complete {
            return Some("required coverage is incomplete".into());
        }
        if coverage.total_frames == 0
            || coverage.rendered_frames.is_empty()
            || self.frames.len() != coverage.rendered_frames.len()
            || self
                .frames
                .iter()
                .zip(&coverage.rendered_frames)
                .any(|(f, want)| {
                    f.frame != *want
                        || f.width == 0
                        || f.height == 0
                        || f.bytes != u64::from(f.width) * u64::from(f.height) * 4
                        || f.sha256.is_empty()
                })
        {
            return Some("the representative frames are missing or do not match the plan".into());
        }
        let Some(audio) = &self.audio else {
            return Some("no prepared-audio evidence".into());
        };
        if !audio.placement_verified {
            return Some("audio track placement was not verified".into());
        }
        if let Some(missing) = REQUIRED_AUDIO_CHECKS
            .iter()
            .find(|name| !audio.checks_passed.iter().any(|c| c == *name))
        {
            return Some(format!("the audio check `{missing}` did not pass"));
        }
        None
    }
}

// ---- probe -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeError {
    pub kind: FailureKind,
    pub message: String,
}

impl ProbeError {
    pub fn source(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Source,
            message: message.into(),
        }
    }
    pub fn environment(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Environment,
            message: message.into(),
        }
    }
}

/// Statistics of prepared PCM, in sample frames, gathered in one streaming pass.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PcmStats {
    pub sample_frames: u64,
    pub non_finite: u64,
    pub peak: f32,
    pub clipped: u64,
    /// Merged ranges `[start, end)` of 10 ms buckets whose peak exceeds the silence floor
    /// (bounded; only used to tell which tracks are audible).
    pub active: Vec<(u64, u64)>,
    /// More ranges existed than are retained: per-track audibility is incomplete.
    pub activity_truncated: bool,
    /// The merged track windows the scan checked every active bucket against. Placement
    /// is only verified when these equal the windows the compiled timeline implies
    /// ([`placement_windows`]); a scan that was given none cannot vouch for placement.
    pub checked_windows: Vec<(u64, u64)>,
    /// First active `[start, end)` bucket outside every checked window, retained
    /// regardless of how fragmented the activity map became (sticky).
    pub first_outside: Option<(u64, u64)>,
}

/// Sorted, merged `[start, end)` sample-frame windows of the compiled audio tracks at
/// `sample_rate`, padded by 100 ms on each side for fades and rounding.
pub fn placement_windows(timeline: &PreviewTimelineResponse, sample_rate: u32) -> Vec<(u64, u64)> {
    let rate = f64::from(sample_rate);
    merge_windows(
        timeline
            .audio_tracks
            .iter()
            .filter(|t| t.end_seconds > t.start_seconds)
            .map(|t| {
                (
                    ((t.start_seconds - 0.1).max(0.) * rate) as u64,
                    ((t.end_seconds + 0.1) * rate) as u64,
                )
            })
            .collect(),
    )
}

fn merge_windows(mut windows: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    windows.retain(|(start, end)| start < end);
    windows.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(windows.len());
    for (start, end) in windows {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

#[derive(Debug, Clone)]
pub struct AudioProbe {
    pub descriptor: PreparedAudioDescriptor,
    pub stats: PcmStats,
}

/// The worker-facing operations validation needs. The app implements it over an owned
/// preview worker; tests use deterministic fakes.
pub trait CandidateProbe {
    fn timeline(&mut self) -> Result<PreviewTimelineResponse, ProbeError>;
    /// Inspect at most [`MAX_INSPECT_FRAMES`] frames.
    fn inspect(&mut self, frames: &[usize]) -> Result<InspectResponse, ProbeError>;
    fn render(&mut self, frame: usize) -> Result<PreviewFrame, ProbeError>;
    /// Prepare the mix and scan its PCM locally.
    fn audio(&mut self) -> Result<AudioProbe, ProbeError>;
    /// Capabilities the worker's hello declared unavailable. Defaults to none.
    fn capability_gaps(&self) -> Vec<String> {
        Vec::new()
    }
    /// True once the run was stopped (Stop, project close, shutdown); polled between
    /// worker requests. Defaults to never.
    fn cancelled(&self) -> bool {
        false
    }
}

/// Stream prepared PCM (`f32le` stereo interleaved) once and gather [`PcmStats`]. Every
/// active 10 ms bucket is checked against `windows` (sample-frame ranges, see
/// [`placement_windows`]) while streaming, so audible content outside the compiled
/// tracks is recorded even when the bounded activity map overflows.
pub fn scan_pcm(
    mut reader: impl Read,
    sample_rate: u32,
    windows: &[(u64, u64)],
    cancelled: &dyn Fn() -> bool,
) -> std::io::Result<PcmStats> {
    let bucket = (sample_rate as u64 / 100).max(1);
    let windows = merge_windows(windows.to_vec());
    let mut stats = PcmStats {
        checked_windows: windows.clone(),
        ..PcmStats::default()
    };
    let mut buffer = vec![0u8; 64 * 1024];
    let mut carry = 0usize;
    let mut bucket_peak = 0f32;
    let mut bucket_fill = 0u64;
    let mut bucket_start = 0u64;
    let mut window = 0usize;
    let mut flush = |stats: &mut PcmStats, start: u64, end: u64, peak: f32| {
        if peak <= SILENCE_PEAK {
            return;
        }
        // Buckets arrive in order: skip windows that ended before this one.
        while window < windows.len() && windows[window].1 <= start {
            window += 1;
        }
        let inside = windows
            .get(window)
            .is_some_and(|&(ws, we)| ws <= start && end <= we);
        if !inside && stats.first_outside.is_none() {
            stats.first_outside = Some((start, end));
        }
        if let Some(last) = stats.active.last_mut()
            && last.1 == start
        {
            last.1 = end;
        } else if stats.active.len() >= MAX_ACTIVITY_RANGES {
            stats.activity_truncated = true;
        } else {
            stats.active.push((start, end));
        }
    };
    loop {
        if cancelled() {
            return Err(std::io::Error::other("PCM scan cancelled"));
        }
        let read = reader.read(&mut buffer[carry..])?;
        if read == 0 {
            break;
        }
        let available = carry + read;
        let whole = available - available % 8;
        for frame in buffer[..whole].as_chunks::<8>().0 {
            let l = f32::from_le_bytes(frame[0..4].try_into().expect("4 bytes"));
            let r = f32::from_le_bytes(frame[4..8].try_into().expect("4 bytes"));
            let mut peak = 0f32;
            for v in [l, r] {
                if !v.is_finite() {
                    stats.non_finite += 1;
                    continue;
                }
                let a = v.abs();
                peak = peak.max(a);
                if a >= 1.0 {
                    stats.clipped += 1;
                }
            }
            stats.peak = stats.peak.max(peak);
            bucket_peak = bucket_peak.max(peak);
            bucket_fill += 1;
            stats.sample_frames += 1;
            if bucket_fill == bucket {
                flush(
                    &mut stats,
                    bucket_start,
                    bucket_start + bucket_fill,
                    bucket_peak,
                );
                bucket_start += bucket_fill;
                bucket_fill = 0;
                bucket_peak = 0.;
            }
        }
        buffer.copy_within(whole..available, 0);
        carry = available - whole;
    }
    if bucket_fill > 0 {
        flush(
            &mut stats,
            bucket_start,
            bucket_start + bucket_fill,
            bucket_peak,
        );
    }
    Ok(stats)
}

// ---- coverage --------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveragePlan {
    pub boundary_frames: Vec<usize>,
    pub sample_frames: Vec<usize>,
    pub rendered_frames: Vec<usize>,
    pub inspect_frames: Vec<usize>,
    pub broadened: Option<BroadenReason>,
    pub complete: bool,
}

/// Why the targeted (boundary + playhead + sample) coverage must widen to the full video.
pub fn broadening(changes: &ChangeSet) -> Option<BroadenReason> {
    if changes.truncated {
        return Some(BroadenReason::ChangeListTruncated);
    }
    if changes.entries.iter().any(|c| {
        matches!(
            c.kind,
            FileKind::Rust | FileKind::Cargo | FileKind::Style | FileKind::Configuration
        )
    }) {
        return Some(BroadenReason::SharedSourceChanged);
    }
    // An existing non-instruction file that was deleted or modified may be shared by
    // scenes that were not edited.
    if changes.entries.iter().any(|c| {
        matches!(c.change, ChangeKind::Deleted | ChangeKind::Modified)
            && !matches!(c.kind, FileKind::Instructions)
    }) {
        return Some(BroadenReason::ExistingFilesChanged);
    }
    if changes
        .entries
        .iter()
        .any(|c| matches!(c.kind, FileKind::Other))
    {
        return Some(BroadenReason::OtherFilesChanged);
    }
    None
}

/// True when the scenes' frame ranges cover `0..total` without a gap.
fn scenes_tile(timeline: &PreviewTimelineResponse) -> bool {
    let total = timeline.total_frames;
    let mut ranges: Vec<(usize, usize)> = timeline
        .scenes
        .iter()
        .map(|s| (s.start_frame, s.end_frame.min(total)))
        .filter(|(start, end)| start < end)
        .collect();
    ranges.sort_unstable();
    let mut cursor = 0;
    for (start, end) in ranges {
        if start > cursor {
            return false;
        }
        cursor = cursor.max(end);
    }
    cursor >= total
}

/// `count` evenly spaced indexes in `0..total` including both ends.
pub fn evenly_spaced(total: usize, count: usize) -> Vec<usize> {
    if total == 0 || count == 0 {
        return vec![];
    }
    let n = count.min(total);
    if n == 1 {
        return vec![0];
    }
    let mut frames: Vec<usize> = (0..n).map(|i| i * (total - 1) / (n - 1)).collect();
    frames.dedup();
    frames
}

pub fn plan_coverage(
    timeline: &PreviewTimelineResponse,
    playhead: usize,
    changes: &ChangeSet,
) -> CoveragePlan {
    let total = timeline.total_frames;
    if total == 0 {
        return CoveragePlan {
            boundary_frames: vec![],
            sample_frames: vec![],
            rendered_frames: vec![],
            inspect_frames: vec![],
            broadened: None,
            complete: true,
        };
    }
    let playhead = playhead.min(total - 1);
    let mut boundaries = vec![0, total - 1];
    for scene in &timeline.scenes {
        if scene.start_frame < total {
            boundaries.push(scene.start_frame);
        }
        if scene.end_frame > 0 && scene.end_frame <= total {
            boundaries.push(scene.end_frame - 1);
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let sample = evenly_spaced(total, MAX_SAMPLE_FRAMES);
    // Render priority: start/end/playhead, scene boundaries, then the sample.
    let mut priority: Vec<usize> = vec![0, total - 1, playhead];
    priority.extend(boundaries.iter().copied());
    priority.extend(sample.iter().copied());
    let mut seen = std::collections::BTreeSet::new();
    let mut rendered: Vec<usize> = priority.into_iter().filter(|f| seen.insert(*f)).collect();
    // Only the start/end/playhead and scene-boundary frames are *required*; the sample is
    // best-effort and is what the render cap trims first.
    let required = std::iter::once(playhead)
        .chain(boundaries.iter().copied())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let mut complete = required <= MAX_RENDERED_FRAMES;
    rendered.truncate(MAX_RENDERED_FRAMES);
    rendered.sort_unstable();
    let broadened = broadening(changes).or_else(|| {
        // Targeted coverage cannot be established without scenes that tile the video.
        (!scenes_tile(timeline)).then_some(BroadenReason::ScenesUnavailable)
    });
    let inspect_frames = if broadened.is_some() {
        if total <= MAX_BROADENED_FRAMES {
            (0..total).collect()
        } else {
            complete = false;
            let mut frames = evenly_spaced(total, MAX_BROADENED_FRAMES);
            frames.extend(boundaries.iter().copied());
            frames.push(playhead);
            frames.sort_unstable();
            frames.dedup();
            frames
        }
    } else {
        let mut frames: Vec<usize> = boundaries.clone();
        frames.extend(sample.iter().copied());
        frames.push(playhead);
        frames.sort_unstable();
        frames.dedup();
        frames
    };
    CoveragePlan {
        boundary_frames: boundaries,
        sample_frames: sample,
        rendered_frames: rendered,
        inspect_frames,
        broadened,
        complete,
    }
}

// ---- validation ------------------------------------------------------------------------

/// What the compile/launch step produced before the probe could run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildOutcome {
    Built(BuildIdentity),
    Failed {
        kind: FailureKind,
        stage: ValidationStage,
        /// Compiler, materialization or worker negotiation output.
        output: String,
        build: Option<BuildIdentity>,
    },
}

/// Keep at most `max` bytes at a char boundary: the tail (compiler errors come last) or
/// the head.
pub fn bound_tail(text: &str, max: usize, tail: bool) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    if tail {
        let mut start = text.len() - max;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text[start..].to_owned()
    } else {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_owned()
    }
}

/// Collects diagnostics. The bounded `diagnostics` list is a *display* buffer; the
/// sticky `errors`/`errors_total`/`first_inspection_error` facts are recorded before any
/// display truncation and are what acceptance reads.
struct Collector {
    diagnostics: Vec<ReportDiagnostic>,
    total: usize,
    /// First [`MAX_FAILURE_DIAGNOSTICS`] error-severity diagnostics, whenever they arrive.
    errors: Vec<ReportDiagnostic>,
    errors_total: usize,
    first_inspection_error: Option<ReportDiagnostic>,
    warnings: Vec<String>,
    gaps: Vec<String>,
}

impl Collector {
    fn new() -> Self {
        Self {
            diagnostics: vec![],
            total: 0,
            errors: vec![],
            errors_total: 0,
            first_inspection_error: None,
            warnings: vec![],
            gaps: vec![],
        }
    }
    fn push(&mut self, diagnostic: ReportDiagnostic) {
        self.total += 1;
        if diagnostic.severity == DiagnosticSeverity::Error {
            self.errors_total += 1;
            if diagnostic.stage == ValidationStage::Inspection
                && self.first_inspection_error.is_none()
            {
                self.first_inspection_error = Some(diagnostic.clone());
            }
            if self.errors.len() < MAX_FAILURE_DIAGNOSTICS {
                self.errors.push(diagnostic.clone());
            }
        }
        if self.diagnostics.len() < MAX_REPORT_DIAGNOSTICS {
            self.diagnostics.push(diagnostic);
        }
    }
    fn note(
        &mut self,
        stage: ValidationStage,
        severity: DiagnosticSeverity,
        frame: Option<usize>,
        key: &str,
        message: impl Into<String>,
    ) {
        self.push(ReportDiagnostic {
            stage,
            severity,
            frame,
            key: key.to_owned(),
            message: bound_tail(&message.into(), MAX_MESSAGE_BYTES, false),
        });
    }
    fn warn(&mut self, message: impl Into<String>) {
        let message = bound_tail(&message.into(), MAX_MESSAGE_BYTES, false);
        if !self.warnings.contains(&message) && self.warnings.len() < MAX_REPORT_DIAGNOSTICS {
            self.warnings.push(message);
        }
    }
}

/// A user Stop or shutdown is never the candidate's fault.
fn cancelled_failure(stage: ValidationStage) -> Failure {
    Failure {
        stage,
        kind: FailureKind::Environment,
        summary: "validation was cancelled".into(),
        output: String::new(),
    }
}

struct Failure {
    stage: ValidationStage,
    kind: FailureKind,
    summary: String,
    output: String,
}

fn is_unsupported_shader(key: &str, message: &str) -> bool {
    let text = format!("{key} {message}").to_lowercase();
    text.contains("shader")
        && (text.contains("unsupported")
            || text.contains("not supported")
            || text.contains("fallback")
            || text.contains("cpu"))
}

/// The only backend the owned preview worker validates.
pub const VALIDATED_BACKEND: &str = "cpu";

/// A compile that does not name exactly the captured candidate (or the validated
/// backend) proves nothing about it.
fn build_identity_defect(captured: &CapturedCandidate, identity: &BuildIdentity) -> Option<String> {
    let candidate = captured.candidate.revision().as_str();
    if identity.source_revision != candidate {
        return Some(format!(
            "the build names source revision {} but the candidate is {candidate}",
            identity.source_revision
        ));
    }
    if identity.backend != VALIDATED_BACKEND {
        return Some(format!(
            "the build used backend `{}` but candidates are validated on `{VALIDATED_BACKEND}`",
            identity.backend
        ));
    }
    None
}

/// Run the acceptance checks. `playhead` is the user's current playhead; `repair_used` is
/// [`RepairBudget::used`]. The report never passes without a probe, a build that names
/// the candidate, complete required coverage and real frames and audio.
pub fn validate_candidate(
    captured: &CapturedCandidate,
    build: BuildOutcome,
    playhead: usize,
    repair_used: u32,
    probe: Option<&mut dyn CandidateProbe>,
) -> ValidationReport {
    let mut collector = Collector::new();
    let mut coverage = None;
    let mut frames = vec![];
    let mut audio = None;
    let (identity, failure) = match build {
        BuildOutcome::Failed {
            kind,
            stage,
            output,
            build,
        } => (
            build,
            Some(Failure {
                stage,
                kind,
                summary: format!("{stage:?} failed"),
                output,
            }),
        ),
        BuildOutcome::Built(identity) => {
            let failure = if let Some(defect) = build_identity_defect(captured, &identity) {
                Some(Failure {
                    stage: ValidationStage::Compile,
                    kind: FailureKind::Environment,
                    summary: defect,
                    output: String::new(),
                })
            } else {
                match probe {
                    Some(probe) => run_probe(
                        captured,
                        probe,
                        playhead,
                        &mut collector,
                        &mut coverage,
                        &mut frames,
                        &mut audio,
                    ),
                    None => Some(Failure {
                        stage: ValidationStage::Worker,
                        kind: FailureKind::Environment,
                        summary: "no preview worker was available to validate the candidate".into(),
                        output: String::new(),
                    }),
                }
            };
            (Some(identity), failure)
        }
    };
    let context_for = |f: Failure, collector: &Collector| FailureContext {
        stage: f.stage,
        kind: f.kind,
        summary: bound_tail(&f.summary, MAX_MESSAGE_BYTES * 2, false),
        output_tail: bound_tail(&f.output, MAX_COMPILER_OUTPUT_BYTES, true),
        diagnostics: collector.errors.clone(),
        changed_paths: captured
            .changes
            .entries
            .iter()
            .take(MAX_FAILURE_PATHS)
            .map(|c| c.path.clone())
            .collect(),
        candidate: captured.candidate.revision().as_str().to_owned(),
        source_base: captured.source_base.revision().as_str().to_owned(),
        build_key: identity.as_ref().map(|b| b.key_digest.clone()),
        repair_count: repair_used,
    };
    let failure_context = failure.map(|f| context_for(f, &collector));
    let mut report = ValidationReport {
        task: captured.identity.clone(),
        source_base: captured.source_base.clone(),
        candidate: captured.candidate.clone(),
        manifest_sha256: captured.manifest_sha256.clone(),
        build: identity.clone(),
        verdict: if failure_context.is_some() {
            Verdict::Failed
        } else {
            Verdict::Passed
        },
        errors: collector.errors.clone(),
        errors_total: collector.errors_total,
        diagnostics: std::mem::take(&mut collector.diagnostics),
        diagnostics_total: collector.total,
        warnings: std::mem::take(&mut collector.warnings),
        capability_gaps: std::mem::take(&mut collector.gaps),
        coverage,
        frames,
        audio,
        changes: captured.changes.clone(),
        repair_count: repair_used,
        failure: failure_context,
    };
    // Last line of defence: whatever the probe claimed, a passing report must carry
    // complete evidence for exactly this candidate.
    if report.verdict == Verdict::Passed
        && let Some(gap) = report.acceptance_gap()
    {
        report.failure = Some(context_for(
            Failure {
                stage: ValidationStage::Worker,
                kind: FailureKind::Environment,
                summary: format!("validation evidence is incomplete: {gap}"),
                output: String::new(),
            },
            &collector,
        ));
        report.verdict = Verdict::Failed;
    }
    report
}

fn run_probe(
    captured: &CapturedCandidate,
    probe: &mut dyn CandidateProbe,
    playhead: usize,
    collector: &mut Collector,
    coverage_out: &mut Option<Coverage>,
    frames_out: &mut Vec<FrameArtifact>,
    audio_out: &mut Option<AudioReport>,
) -> Option<Failure> {
    let revision = captured.candidate.revision().as_str();
    let fail = |stage, error: ProbeError, what: &str| Failure {
        stage,
        kind: error.kind,
        summary: format!(
            "{what}: {}",
            bound_tail(&error.message, MAX_MESSAGE_BYTES, false)
        ),
        output: error.message,
    };
    let timeline = match probe.timeline() {
        Ok(t) => t,
        Err(e) => {
            return Some(fail(
                ValidationStage::Timeline,
                e,
                "the compiled timeline is unavailable",
            ));
        }
    };
    if timeline.envelope.identity.source_revision != revision {
        return Some(Failure {
            stage: ValidationStage::Timeline,
            kind: FailureKind::Environment,
            summary: "the timeline was produced for a different revision than the candidate".into(),
            output: String::new(),
        });
    }
    if let Err(message) = validate_preview_timeline(&timeline) {
        collector.note(
            ValidationStage::Timeline,
            DiagnosticSeverity::Error,
            None,
            "timeline_invalid",
            &message,
        );
        return Some(Failure {
            stage: ValidationStage::Timeline,
            kind: FailureKind::Source,
            summary: message.clone(),
            output: message,
        });
    }
    if timeline.total_frames == 0 {
        collector.note(
            ValidationStage::Timeline,
            DiagnosticSeverity::Error,
            None,
            "timeline_empty",
            "the compiled video has no frames",
        );
        return Some(Failure {
            stage: ValidationStage::Timeline,
            kind: FailureKind::Source,
            summary: "the compiled video has no frames".into(),
            output: String::new(),
        });
    }
    for gap in probe.capability_gaps().into_iter().take(16) {
        let gap = bound_tail(&gap, 128, false);
        if gap.to_lowercase().contains("shader") {
            collector.warn(format!(
                "Unsupported shaders: the CPU preview worker reports the capability gap `{gap}`; shader effects in this video cannot be shown faithfully."
            ));
        }
        collector.note(
            ValidationStage::Worker,
            DiagnosticSeverity::Warning,
            None,
            "capability_gap",
            format!("the preview worker declares `{gap}` unavailable"),
        );
        collector.gaps.push(gap);
    }
    let plan = plan_coverage(&timeline, playhead, &captured.changes);
    let playhead = playhead.min(timeline.total_frames - 1);
    // Inspection in bounded batches.
    let mut batches = 0;
    for batch in plan.inspect_frames.chunks(MAX_INSPECT_FRAMES) {
        if probe.cancelled() {
            return Some(cancelled_failure(ValidationStage::Inspection));
        }
        let response = match probe.inspect(batch) {
            Ok(r) => r,
            Err(e) => return Some(fail(ValidationStage::Inspection, e, "inspection failed")),
        };
        batches += 1;
        if response.truncated {
            collector.note(
                ValidationStage::Inspection,
                DiagnosticSeverity::Error,
                None,
                "inspection_truncated",
                "inspection output was truncated; critical findings cannot be ruled out",
            );
        }
        for d in response.diagnostics {
            if is_unsupported_shader(&d.key, &d.message) {
                collector.warn(format!(
                    "Unsupported shader at frame {}: {} ({}); the CPU preview cannot show it faithfully.",
                    d.frame, d.message, d.key
                ));
            }
            collector.note(
                ValidationStage::Inspection,
                d.severity,
                Some(d.frame),
                &d.key,
                d.message,
            );
        }
    }
    *coverage_out = Some(Coverage {
        total_frames: timeline.total_frames,
        boundary_frames: plan.boundary_frames.clone(),
        playhead,
        sample_frames: plan.sample_frames.clone(),
        rendered_frames: plan.rendered_frames.clone(),
        inspected_frames: plan.inspect_frames.len(),
        inspection_batches: batches,
        broadened: plan.broadened,
        complete: plan.complete,
    });
    if !plan.complete {
        collector.warn(
            "Coverage is partial: the video is longer than the bounded full inspection or more frames were representative than the render cap allows.",
        );
    }
    if let Some(first) = collector.first_inspection_error.as_ref() {
        let summary = format!(
            "critical inspection error at frame {}: {}",
            first
                .frame
                .map_or_else(|| "-".to_owned(), |f| f.to_string()),
            first.message
        );
        return Some(Failure {
            stage: ValidationStage::Inspection,
            kind: FailureKind::Source,
            summary,
            output: String::new(),
        });
    }
    // Representative frames.
    for frame in &plan.rendered_frames {
        if probe.cancelled() {
            return Some(cancelled_failure(ValidationStage::Render));
        }
        let rendered = match probe.render(*frame) {
            Ok(f) => f,
            Err(e) => {
                collector.note(
                    ValidationStage::Render,
                    DiagnosticSeverity::Error,
                    Some(*frame),
                    "render_failed",
                    &e.message,
                );
                return Some(fail(
                    ValidationStage::Render,
                    e,
                    &format!("rendering frame {frame} failed"),
                ));
            }
        };
        let header = &rendered.response.header;
        let expected = u64::from(header.width) * u64::from(header.height) * 4;
        if header.width == 0
            || header.height == 0
            || rendered.pixels.len() as u64 != expected
            || rendered.response.frame_index != *frame
        {
            let message = format!("frame {frame} has invalid geometry or index");
            collector.note(
                ValidationStage::Render,
                DiagnosticSeverity::Error,
                Some(*frame),
                "render_invalid",
                &message,
            );
            return Some(Failure {
                stage: ValidationStage::Render,
                kind: FailureKind::Source,
                summary: message.clone(),
                output: message,
            });
        }
        if let Err(message) = rendered.validate(&timeline.envelope.identity) {
            let message =
                format!("frame {frame} does not belong to the candidate timeline: {message}");
            collector.note(
                ValidationStage::Render,
                DiagnosticSeverity::Error,
                Some(*frame),
                "render_identity",
                &message,
            );
            return Some(Failure {
                stage: ValidationStage::Render,
                kind: FailureKind::Environment,
                summary: message.clone(),
                output: message,
            });
        }
        frames_out.push(FrameArtifact {
            frame: *frame,
            width: header.width,
            height: header.height,
            bytes: rendered.pixels.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&rendered.pixels)),
        });
    }
    // Prepared audio and local PCM/timeline checks.
    if probe.cancelled() {
        return Some(cancelled_failure(ValidationStage::Audio));
    }
    let probed = match probe.audio() {
        Ok(a) => a,
        Err(e) => {
            collector.note(
                ValidationStage::Audio,
                DiagnosticSeverity::Error,
                None,
                "audio_unavailable",
                &e.message,
            );
            return Some(fail(ValidationStage::Audio, e, "audio preparation failed"));
        }
    };
    let (report, errors) = check_audio(&timeline, &probed, revision);
    for (key, message) in &errors {
        collector.note(
            ValidationStage::Audio,
            DiagnosticSeverity::Error,
            None,
            key,
            message,
        );
    }
    for (severity, key, message) in audio_diagnostics(&report) {
        let warn = severity != DiagnosticSeverity::Info;
        collector.note(ValidationStage::Audio, severity, None, &key, &message);
        if warn {
            collector.warn(message);
        }
    }
    let placement_unverified = !report.placement_verified;
    if placement_unverified {
        let message = "Track placement was not verified: the PCM scan did not check the compiled audio track windows, so audible content outside them cannot be ruled out.";
        collector.note(
            ValidationStage::Audio,
            DiagnosticSeverity::Error,
            None,
            "audio_placement_unverified",
            message,
        );
        collector.warn(message);
    }
    *audio_out = Some(report);
    if let Some((_, message)) = errors.first() {
        return Some(Failure {
            stage: ValidationStage::Audio,
            kind: FailureKind::Source,
            summary: message.clone(),
            output: errors
                .iter()
                .map(|(_, m)| m.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }
    if placement_unverified {
        return Some(Failure {
            stage: ValidationStage::Audio,
            kind: FailureKind::Environment,
            summary: "audio track placement could not be verified".into(),
            output: String::new(),
        });
    }
    // Every other check passed on the frames that were examined, but a change whose
    // required coverage exceeds the bounded validator is retained with this limitation
    // instead of being accepted.
    if !plan.complete {
        return Some(Failure {
            stage: ValidationStage::Coverage,
            kind: FailureKind::Environment,
            summary: format!(
                "bounded validation limitation: this change requires more coverage than the validator supports ({} frames; at most {MAX_RENDERED_FRAMES} representative frames are rendered and at most {MAX_BROADENED_FRAMES} frames are inspected). The candidate is retained for review, not accepted.",
                timeline.total_frames
            ),
            output: String::new(),
        });
    }
    None
}

fn audio_diagnostics(report: &AudioReport) -> Vec<(DiagnosticSeverity, String, String)> {
    let mut out = vec![];
    if report.clipped_samples > 0 {
        out.push((
            DiagnosticSeverity::Warning,
            "audio_clipping".to_owned(),
            format!(
                "{} samples reach or exceed full scale (peak {:.3})",
                report.clipped_samples, report.peak
            ),
        ));
    }
    if !report.silent && report.peak <= SILENCE_PEAK {
        out.push((
            DiagnosticSeverity::Warning,
            "audio_silent_mix".to_owned(),
            "the mix declares audio tracks but the prepared PCM is silent".to_owned(),
        ));
    }
    for track in report
        .tracks
        .iter()
        .filter(|t| !t.audible && t.end_seconds > t.start_seconds)
        .filter(|_| report.audibility_complete)
    {
        if report.peak > SILENCE_PEAK {
            out.push((
                DiagnosticSeverity::Warning,
                "audio_silent_track".to_owned(),
                format!(
                    "track {} is silent inside its window {:.2}s-{:.2}s",
                    track.file, track.start_seconds, track.end_seconds
                ),
            ));
        }
    }
    out
}

/// Bounded local checks of the prepared PCM against the compiled timeline. Returns the
/// report and the failing checks (`(key, message)`).
pub fn check_audio(
    timeline: &PreviewTimelineResponse,
    probed: &AudioProbe,
    candidate_revision: &str,
) -> (AudioReport, Vec<(String, String)>) {
    let d = &probed.descriptor;
    let stats = &probed.stats;
    let mut errors: Vec<(String, String)> = vec![];
    let mut passed: Vec<String> = vec![];
    let mut check = |name: &str, ok: bool, message: String| {
        if ok {
            passed.push(name.to_owned());
        } else {
            errors.push((format!("audio_{name}"), message));
        }
    };
    check(
        "revision",
        d.envelope.identity.source_revision == candidate_revision
            && timeline.envelope.identity == d.envelope.identity,
        "prepared audio was produced for a different revision/worker than the candidate timeline"
            .into(),
    );
    check(
        "geometry",
        (8000..=192_000).contains(&d.sample_rate) && d.channels == 2,
        format!(
            "unsupported audio geometry {} Hz / {} channels",
            d.sample_rate, d.channels
        ),
    );
    let expected = (timeline.total_frames as u128 * u128::from(d.sample_rate))
        .div_ceil(timeline.fps.max(1) as u128);
    check(
        "duration",
        u128::from(d.sample_count) == expected
            && d.sample_count.checked_mul(8) == Some(d.byte_count),
        format!(
            "prepared audio has {} samples; the compiled duration needs {expected}",
            d.sample_count
        ),
    );
    check(
        "pcm_length",
        stats.sample_frames == d.sample_count,
        format!(
            "scanned PCM has {} sample frames, descriptor says {}",
            stats.sample_frames, d.sample_count
        ),
    );
    check(
        "finite",
        stats.non_finite == 0,
        format!(
            "prepared PCM contains {} non-finite samples",
            stats.non_finite
        ),
    );
    check(
        "silent_flag",
        d.silent == timeline.audio_tracks.is_empty(),
        "the silent flag disagrees with the compiled audio tracks".into(),
    );
    let duration = timeline.total_frames as f64 / timeline.fps.max(1) as f64;
    let tolerance = 1.0 / timeline.fps.max(1) as f64 + 0.05;
    let rate = f64::from(d.sample_rate);
    let in_bounds = timeline.audio_tracks.iter().all(|t| {
        t.start_seconds >= 0.
            && t.end_seconds >= t.start_seconds
            && t.start_seconds <= duration + tolerance
    });
    check(
        "track_bounds",
        in_bounds,
        "an audio track starts beyond the compiled duration".into(),
    );
    // Placement is evidenced by the streaming scan, which checked every active bucket
    // against these windows; it never depends on the bounded activity map. A scan that
    // checked different windows (or none) cannot vouch for placement: not a pass.
    let windows = placement_windows(timeline, d.sample_rate);
    let placement_verified = stats.checked_windows == windows;
    if placement_verified {
        check(
            "track_placement",
            stats.first_outside.is_none(),
            match stats.first_outside {
                Some((start, end)) => format!(
                    "prepared PCM has audible content at {:.2}s-{:.2}s, outside every compiled audio track window",
                    start as f64 / rate,
                    end as f64 / rate
                ),
                None => String::new(),
            },
        );
    }
    let tracks = timeline
        .audio_tracks
        .iter()
        .map(|t| {
            let s = (t.start_seconds * rate) as u64;
            let e = (t.end_seconds * rate) as u64;
            TrackCheck {
                file: bound_tail(&t.file, 256, false),
                start_seconds: t.start_seconds,
                end_seconds: t.end_seconds,
                audible: stats.active.iter().any(|&(a, b)| a < e && b > s),
            }
        })
        .collect();
    (
        AudioReport {
            artifact_id: d.artifact_id.clone(),
            sha256: d.sha256.clone(),
            sample_rate: d.sample_rate,
            channels: d.channels,
            sample_count: d.sample_count,
            silent: d.silent,
            peak: stats.peak,
            clipped_samples: stats.clipped,
            tracks,
            placement_verified,
            audibility_complete: !stats.activity_truncated,
            checks_passed: passed,
        },
        errors,
    )
}

// ---- routing ---------------------------------------------------------------------------

/// What the transaction does next with a validation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextStep {
    /// The candidate is validated and may be handed to promotion.
    Accept,
    /// Spend the single automatic repair: reopen the stable draft with this context,
    /// reap the writer again and capture a new immutable candidate.
    Repair {
        attempt: u32,
        context: FailureContext,
    },
    /// Retain the draft for user review; no automatic repair applies.
    Retain {
        reason: String,
        context: Option<FailureContext>,
    },
}

/// Deterministic repair routing over the Stage 1 [`RepairBudget`] (one attempt maximum).
pub fn next_step(report: &ValidationReport, budget: &RepairBudget) -> NextStep {
    let Some(context) = report.failure() else {
        // No recorded failure is not acceptance: the report must be a passing one with
        // complete evidence for exactly its candidate.
        return match report.acceptance_gap() {
            None => NextStep::Accept,
            Some(gap) => NextStep::Retain {
                reason: format!("the validation report is not acceptance evidence: {gap}"),
                context: None,
            },
        };
    };
    match context.kind {
        FailureKind::Environment => NextStep::Retain {
            reason: format!("validation could not complete: {}", context.summary),
            context: Some(context.clone()),
        },
        FailureKind::Source if budget.remaining() > 0 => NextStep::Repair {
            attempt: budget.used() + 1,
            context: context.clone(),
        },
        FailureKind::Source => NextStep::Retain {
            reason: format!(
                "validation failed after the automatic repair attempt: {}",
                context.summary
            ),
            context: Some(context.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Controller, EngineError, QuiescenceEvidence, TaskError, TaskState, TurnCompletion,
        WriterObservation, app_paths::AppPaths, build_materialization::sdk_pin,
    };
    use studio_bootstrap::WriterOwnership;
    use studio_sdk::CompatibilityManifest;

    fn qualified() -> WriterOwnership {
        WriterOwnership::ProcessGroupContained {
            qualification: "test-adapter".into(),
        }
    }

    /// A controller whose task is `Validating` a captured, unchanged candidate.
    fn validating() -> (
        tempfile::TempDir,
        Controller,
        TaskIdentity,
        CapturedCandidate,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("video");
        let paths = AppPaths::new(temp.path().join("history")).unwrap();
        studio_project::create(
            &root,
            "Video",
            sdk_pin(&CompatibilityManifest::default_linux_x64()),
            "1.1.0",
            "0.1.0",
        )
        .unwrap();
        let mut controller = Controller::open(&root, &paths).unwrap();
        let context = controller.begin_agent_task("edit").unwrap();
        let id = context.identity.clone();
        let writer = controller.agent_writer_started(&id, qualified()).unwrap();
        controller
            .agent_task_transition(&id, TaskState::Editing)
            .unwrap();
        controller
            .agent_task_transition(&id, TaskState::Quiescing)
            .unwrap();
        let ticket = controller
            .agent_complete_quiescence(
                &id,
                &QuiescenceEvidence {
                    identity: id.clone(),
                    provider_session: None,
                    writer,
                    completion: TurnCompletion::EndTurn,
                    cancel_requested: false,
                    unresolved_requests: 0,
                    observed: WriterObservation {
                        ownership: qualified(),
                        escaped_pids: vec![],
                    },
                },
            )
            .unwrap();
        let captured = controller.agent_capture_candidate(ticket).unwrap();
        (temp, controller, id, captured)
    }

    fn failed_report(captured: &CapturedCandidate) -> ValidationReport {
        validate_candidate(
            captured,
            BuildOutcome::Failed {
                kind: FailureKind::Source,
                stage: ValidationStage::Compile,
                output: "error".into(),
                build: None,
            },
            0,
            0,
            None,
        )
    }

    #[test]
    fn a_report_for_another_manifest_or_candidate_never_advances_the_task() {
        let (_temp, mut controller, id, captured) = validating();
        let report = failed_report(&captured);

        let mut wrong_manifest = report.clone();
        wrong_manifest.manifest_sha256 = "0".repeat(64);
        assert!(matches!(
            controller.agent_apply_validation(&id, &wrong_manifest),
            Err(EngineError::Task(TaskError::ReportMismatch))
        ));
        let mut wrong_candidate = report.clone();
        wrong_candidate.candidate = CandidateRevision::new(
            studio_project::SourceRevision::try_from("9".repeat(64)).unwrap(),
        );
        assert!(matches!(
            controller.agent_apply_validation(&id, &wrong_candidate),
            Err(EngineError::Task(TaskError::ReportMismatch))
        ));
        assert_eq!(
            controller.agent_task().unwrap().state(),
            TaskState::Validating
        );
        // The untampered report routes normally (a source failure spends the repair).
        assert!(matches!(
            controller.agent_apply_validation(&id, &report).unwrap(),
            NextStep::Repair { .. }
        ));
    }

    #[test]
    fn a_failed_report_forged_into_a_pass_has_no_evidence_and_is_not_accepted() {
        let (_temp, mut controller, id, captured) = validating();
        let budget = RepairBudget::automatic();
        let mut forged = failed_report(&captured);
        forged.failure = None;
        // Still verdict Failed with no recorded failure: not acceptance.
        assert!(matches!(
            next_step(&forged, &budget),
            NextStep::Retain { .. }
        ));
        // Verdict flipped as well, but no build/coverage/frames/audio exist.
        forged.verdict = Verdict::Passed;
        assert!(forged.acceptance_gap().is_some());
        assert!(matches!(
            next_step(&forged, &budget),
            NextStep::Retain { .. }
        ));
        assert!(matches!(
            controller.agent_apply_validation(&id, &forged).unwrap(),
            NextStep::Retain { .. }
        ));
        assert_ne!(
            controller.agent_task().map(|t| t.state()),
            Some(TaskState::CandidateReady)
        );
    }
}
