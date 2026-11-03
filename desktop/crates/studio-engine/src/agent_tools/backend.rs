//! The production [`ToolBackend`]: read-only project tools over immutable revisions.
//!
//! * `Fixed` bindings answer for one immutable (candidate) revision.
//! * `Draft` bindings first take the [`WriterGate`], capture a labelled immutable
//!   snapshot of the task's stable draft into checkpoint objects (the capture verifies
//!   the tree twice), or return `busy` when the tree is changing. A changing tree is
//!   never compiled.
//! * Builds go through the shared [`BuildService`] (equal key => one compile) and run in
//!   tool-owned preview workers with their own process scope: they never use the
//!   displayed worker or the thumbnail lane. At most [`TOOL_WORKERS`] workers live.
//! * Images are app-owned PNG artifacts ([`ArtifactStore`]) with ids, hashes, expiry and
//!   size limits; nothing is returned inline.
//! * `build_status` only observes; it never captures, compiles or launches Cargo.
//!
//! Tool builds are explicitly unvalidated: only the acceptance validator qualifies a task.
use super::{
    ARTIFACT_TTL, ArtifactRef, BoundRevision, MAX_IMAGE_ARTIFACT_BYTES, RevisionInfo,
    RevisionLabel, TOOL_WORKERS, TaskLiveness, ToolBackend, ToolBinding, ToolCall, ToolError,
    ToolErrorCode, ToolReply, ToolRequest,
};
use crate::services::{
    build_service::{
        BuildError, BuildKey, BuildService, CompileEnvironment, CompileRequest, Subscriber,
        SubscriberKind,
    },
    preview_worker_client::PreviewWorkerClient,
    worker_client::allocate_worker_generation,
    worker_project::launch_preview_worker,
};
use crate::{
    AgentTaskId, TaskIdentity,
    candidate_validation::{
        CandidateError, RestoredRevision, capture_draft_revision, restore_revision,
    },
};
use fframes_studio_protocol::{
    EditorFrameStatus, EditorSourceAnchor, MAX_PREVIEW_HEIGHT, MAX_PREVIEW_WIDTH, PreviewIdentity,
    PreviewTimelineResponse,
};
use parking_lot::Mutex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    any::Any,
    collections::{HashMap, VecDeque},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use studio_bootstrap::ProcessTreeManager;
use studio_project::{
    ProjectId, ProjectPath, SourceRevision,
    checkpoint::Checkpoints,
    revision::FileKind,
    source_index::{
        MAX_INDEX_FILE_BYTES, MAX_INDEX_RUST_FILES, MAX_INDEX_TOTAL_BYTES, SourceAnchor,
        SourceIndex, SourceIndexInput,
    },
};
use studio_sdk::CompatibilityManifest;

const MAX_ARTIFACT_STORE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SELECTION_CONTEXT_OBJECTS: usize = 128;
const MAX_OPEN_REVISIONS: usize = 4;
const MAX_CONTEXT_FILES: usize = 400;
/// Snapshot identities a task may assert again (see [`RevisionHistory`]).
pub const MAX_KNOWN_REVISIONS: usize = 64;
/// Identities `build_status` lists. Independent of (and smaller than) the retention bound,
/// so the reply size depends on neither the bound nor how many snapshots a task captured.
pub const STATUS_KNOWN_REVISIONS: usize = 16;
/// Strip width bound: keeps a 24-frame strip a few megabytes of PNG at most.
const MAX_STRIP_WIDTH: u32 = 4096;

fn tool_error(code: ToolErrorCode, message: impl Into<String>) -> ToolError {
    ToolError::new(code, message)
}

fn source_lookup_error(error: studio_project::source_index::SourceIndexError) -> ToolError {
    use studio_project::source_index::SourceIndexError;
    match error {
        SourceIndexError::InvalidAnchor(message) => tool_error(ToolErrorCode::NotFound, message),
        SourceIndexError::HashMismatch(path) => tool_error(
            ToolErrorCode::StaleRevision,
            format!("immutable source bytes changed while looking up {path}"),
        ),
        SourceIndexError::InvalidDigest(path) | SourceIndexError::SizeMismatch(path) => tool_error(
            ToolErrorCode::Unavailable,
            format!("invalid immutable source record for {path}"),
        ),
        SourceIndexError::Cancelled => {
            tool_error(ToolErrorCode::Cancelled, "source lookup cancelled")
        }
    }
}

fn source_parse_diagnostic(index: &SourceIndex, path: &ProjectPath) -> Option<String> {
    index
        .diagnostics
        .iter()
        .find(|diagnostic| &diagnostic.path == path)
        .map(|diagnostic| diagnostic.message.clone())
}

fn unresolved_style_bindings(names: &[String], token_filter: Option<&str>) -> Vec<Value> {
    names
        .iter()
        .filter(|token| token_filter.is_none_or(|filter| token.as_str() == filter))
        .map(|token| json!({"token": token, "status": "no_active_preset"}))
        .collect()
}

fn retain_source_index(
    indexes: &mut VecDeque<Arc<SourceIndex>>,
    index: Arc<SourceIndex>,
) -> Arc<SourceIndex> {
    if let Some(existing) = indexes
        .iter()
        .find(|existing| existing.revision == index.revision)
    {
        return existing.clone();
    }
    indexes.push_back(index.clone());
    while indexes.len() > 4
        || indexes
            .iter()
            .map(|entry| entry.indexed_bytes)
            .sum::<usize>()
            > 32 * 1024 * 1024
    {
        indexes.pop_front();
    }
    index
}

struct SourceLookupRequest<'a> {
    path: &'a ProjectPath,
    symbol: &'a str,
    marker: Option<&'a str>,
    object_identity: Option<&'a fframes_studio_protocol::EditorObjectIdentity>,
    registered_style_tokens: Option<&'a [String]>,
}

struct SelectionContextRequest<'a> {
    frame_index: usize,
    seek_serial: u64,
    wanted: Option<&'a fframes_studio_protocol::EditorObjectIdentity>,
    expected_geometry_digest: Option<&'a str>,
}

fn build_error(error: BuildError) -> ToolError {
    match error {
        BuildError::Failed(message) => tool_error(ToolErrorCode::BuildFailed, message),
        BuildError::Cancelled => tool_error(ToolErrorCode::Cancelled, "build cancelled"),
        BuildError::CacheFull { .. } => tool_error(ToolErrorCode::Busy, error.to_string()),
        other => tool_error(ToolErrorCode::Unavailable, other.to_string()),
    }
}

mod artifacts;
mod captions;
mod project;
mod writer_gate;

pub use artifacts::*;
pub use project::*;
pub use writer_gate::*;

#[cfg(test)]
mod source_index_cache_tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn index(revision: usize) -> Arc<SourceIndex> {
        let revision: SourceRevision = format!("{revision:064x}").try_into().unwrap();
        Arc::new(SourceIndex::build(revision, vec![], &|| false).unwrap())
    }

    #[test]
    fn source_indexes_retain_four_latest_revisions_and_reuse_matching_revision() {
        let mut indexes = VecDeque::new();
        for revision in 0..5 {
            retain_source_index(&mut indexes, index(revision));
        }

        assert_eq!(indexes.len(), 4);
        assert_eq!(
            indexes
                .iter()
                .map(|entry| entry.revision.as_str().to_owned())
                .collect::<Vec<_>>(),
            (1..5)
                .map(|revision| format!("{revision:064x}"))
                .collect::<Vec<_>>()
        );
        let existing = indexes.back().unwrap().clone();
        let reused = retain_source_index(&mut indexes, index(4));
        assert!(Arc::ptr_eq(&existing, &reused));
        assert_eq!(indexes.len(), 4);
    }

    #[test]
    fn malformed_indexed_source_keeps_its_parse_diagnostic() {
        let path = ProjectPath::try_from("src/broken.rs".to_owned()).unwrap();
        let bytes = b"fn broken( {".to_vec();
        let source = SourceIndexInput {
            path: path.clone(),
            kind: FileKind::Rust,
            expected_sha256: format!("{:x}", Sha256::digest(&bytes)),
            expected_size: bytes.len() as u64,
            bytes,
        };
        let revision: SourceRevision = "a".repeat(64).try_into().unwrap();
        let index = SourceIndex::build(revision, vec![source], &|| false).unwrap();

        assert!(
            source_parse_diagnostic(&index, &path)
                .unwrap()
                .contains("syntax unavailable")
        );
    }

    #[test]
    fn no_preset_binding_results_respect_the_exact_token_filter() {
        let names = vec!["color.text".into(), "color.accent".into()];
        assert_eq!(
            unresolved_style_bindings(&names, Some("color.accent")),
            vec![json!({"token": "color.accent", "status": "no_active_preset"})]
        );
        assert_eq!(
            unresolved_style_bindings(&names, Some("missing")),
            Vec::<Value>::new()
        );
        assert_eq!(unresolved_style_bindings(&names, None).len(), 2);
    }

    #[test]
    fn source_index_cancellation_keeps_its_tool_error_code() {
        assert_eq!(
            source_lookup_error(studio_project::source_index::SourceIndexError::Cancelled).code,
            ToolErrorCode::Cancelled
        );
    }
}
