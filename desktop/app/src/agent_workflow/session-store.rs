//! Versioned durable session manifest and bounded handoff context storage.
//!
//! Stores opaque native session IDs, provider/profile identity, project/draft identity,
//! last source and accepted revisions, capability snapshots, and transcript boundaries
//! using owner-only atomic storage. Redacts opaque session IDs for telemetry/UI.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use studio_agent_spike::driver::AgentCapabilityInfo;
use studio_engine::{TaskScope, app_paths::AppPaths};
use studio_project::ProjectId;

pub const SESSION_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const CONTEXT_SCHEMA_VERSION: &str = "m6-context/1";
pub const MAX_CONTEXT_ENVELOPE_BYTES: usize = 64 * 1024; // 64 KiB

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionManifestState {
    Ready,
    Ended,
    Unsafe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionManifest {
    pub schema_version: u32,
    pub provider_id: String,
    /// Opaque native driver session ID (stored securely, redacted in UI/logs).
    pub wire_session_id: Option<String>,
    pub project_id: String,
    pub draft_path: PathBuf,
    pub launch_digest: String,
    pub last_source_revision: String,
    pub last_accepted_revision: String,
    pub context_schema: String,
    pub capability_snapshot: Option<AgentCapabilityInfo>,
    pub transcript_boundary: usize,
    pub state: SessionManifestState,
    pub created_at: String,
    pub updated_at: String,
}

impl SessionManifest {
    pub fn is_resumable(&self) -> bool {
        self.schema_version == SESSION_MANIFEST_SCHEMA_VERSION
            && self.state == SessionManifestState::Ready
            && self.wire_session_id.is_some()
    }

    /// Redacted view of the session manifest for safe logging and UI display.
    pub fn redacted_id(&self) -> String {
        match &self.wire_session_id {
            Some(id) if id.len() > 12 => format!("{}…{}", &id[..6], &id[id.len() - 4..]),
            Some(id) => format!("{id}…"),
            None => "none".to_string(),
        }
    }
}

/// Bounded context transferred between provider sessions on handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundedHandoffContext {
    pub schema: String,
    pub source_revision: String,
    pub draft_revision: Option<String>,
    pub brief: String,
    pub outcome_summary: Option<String>,
    pub scope: Option<TaskScope>,
    pub style_snapshot: Option<String>,
    pub diagnostics: Vec<String>,
    pub omissions: Vec<String>,
}

impl BoundedHandoffContext {
    pub fn new(
        source_revision: String,
        draft_revision: Option<String>,
        brief: String,
        outcome_summary: Option<String>,
        scope: Option<TaskScope>,
        style_snapshot: Option<String>,
        diagnostics: Vec<String>,
    ) -> Self {
        let mut omissions = Vec::new();
        let mut ctx = Self {
            schema: CONTEXT_SCHEMA_VERSION.to_string(),
            source_revision,
            draft_revision,
            brief,
            outcome_summary,
            scope,
            style_snapshot,
            diagnostics,
            omissions: Vec::new(),
        };

        for diagnostic in &mut ctx.diagnostics {
            truncate_utf8(diagnostic, 4096);
        }
        if ctx.diagnostics.len() > 32 {
            ctx.diagnostics.truncate(32);
            omissions.push("diagnostics_truncated".to_string());
        }
        if let Some(outcome) = &mut ctx.outcome_summary {
            truncate_utf8(outcome, 4096);
        }
        truncate_utf8(&mut ctx.brief, 16 * 1024);
        if serialized_len(&ctx) > MAX_CONTEXT_ENVELOPE_BYTES {
            ctx.scope = None;
            omissions.push("scope_omitted".to_string());
        }
        if serialized_len(&ctx) > MAX_CONTEXT_ENVELOPE_BYTES {
            ctx.style_snapshot = None;
            omissions.push("style_snapshot_omitted".to_string());
        }
        if serialized_len(&ctx) > MAX_CONTEXT_ENVELOPE_BYTES {
            if let Some(outcome) = &mut ctx.outcome_summary {
                truncate_utf8(outcome, 1024);
            }
            omissions.push("outcome_summary_truncated".to_string());
        }
        while serialized_len(&ctx) > MAX_CONTEXT_ENVELOPE_BYTES && !ctx.diagnostics.is_empty() {
            ctx.diagnostics.pop();
            if !omissions.iter().any(|item| item == "diagnostics_truncated") {
                omissions.push("diagnostics_truncated".to_string());
            }
        }
        if serialized_len(&ctx) > MAX_CONTEXT_ENVELOPE_BYTES {
            truncate_utf8(&mut ctx.brief, 1024);
            omissions.push("brief_truncated".to_string());
        }
        if serialized_len(&ctx) > MAX_CONTEXT_ENVELOPE_BYTES {
            truncate_utf8(&mut ctx.source_revision, 128);
            if let Some(revision) = &mut ctx.draft_revision {
                truncate_utf8(revision, 128);
            }
            omissions.push("revision_identity_truncated".to_string());
        }
        ctx.omissions = omissions;
        ctx
    }

    pub fn to_json_envelope(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

fn serialized_len(context: &BoundedHandoffContext) -> usize {
    serde_json::to_vec(context).map_or(usize::MAX, |bytes| bytes.len())
}

fn truncate_utf8(text: &mut String, max_bytes: usize) {
    if text.len() > max_bytes {
        let mut end = max_bytes;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

pub struct SessionStore;

impl SessionStore {
    pub fn save(
        paths: &AppPaths,
        project_id: &ProjectId,
        manifest: &SessionManifest,
    ) -> Result<(), String> {
        let path = paths.agent_session_manifest(project_id);
        let bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|e| format!("cannot serialize session manifest: {e}"))?;
        write_atomic(&path, &bytes)
    }

    pub fn load(
        paths: &AppPaths,
        project_id: &ProjectId,
    ) -> Result<Option<SessionManifest>, String> {
        let path = paths.agent_session_manifest(project_id);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let manifest: SessionManifest = serde_json::from_str(&text)
                    .map_err(|e| format!("cannot parse session manifest: {e}"))?;
                if manifest.schema_version > SESSION_MANIFEST_SCHEMA_VERSION {
                    return Ok(None);
                }
                Ok(Some(manifest))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot read session manifest: {e}")),
        }
    }

    pub fn clear(paths: &AppPaths, project_id: &ProjectId) -> Result<(), String> {
        let path = paths.agent_session_manifest(project_id);
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot clear session manifest: {e}")),
        }
    }
}

static TEMPORARY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::Ordering;

    let parent = path.parent().ok_or_else(|| "invalid path".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| format!("cannot create directory: {e}"))?;

    let temporary = path.with_extension(format!(
        "manifest.{}.{}.tmp",
        std::process::id(),
        TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|e| format!("cannot write session manifest: {e}"))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("cannot write session manifest: {e}"))?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(|e| format!("cannot save session manifest: {e}"))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}
