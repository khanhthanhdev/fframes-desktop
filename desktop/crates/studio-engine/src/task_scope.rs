//! Immutable, revision-bound scope captured when a brief is submitted.

use fframes_studio_protocol::{PreviewIdentity, PreviewTimelineResponse};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};
use studio_project::revision::{FileKind, SourceFile};

use crate::timeline::TimelineSelection;

const MAX_SCOPE_SCENES: usize = 128;
const MAX_SCOPE_BOUNDARIES: usize = 256;
const MAX_SOURCE_SCENES: usize = 16;
const MAX_SOURCE_CANDIDATES_PER_SCENE: usize = 4;
const MAX_SOURCE_FILES: usize = 256;
const MAX_SOURCE_FILE_BYTES: u64 = 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScopeSelection {
    WholeProject,
    Scene {
        instance_id: String,
        name: String,
        full_name: String,
    },
    FrameRange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedScene {
    pub instance_id: String,
    pub name: String,
    pub full_name: String,
    pub start_frame: usize,
    pub end_frame: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMatchConfidence {
    ExactIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneSourceCandidate {
    pub path: String,
    pub symbol: String,
    pub sha256: String,
    pub confidence: SourceMatchConfidence,
    pub match_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneSourceResolution {
    Unique,
    Ambiguous,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneSourceReference {
    pub instance_id: String,
    pub resolution: SceneSourceResolution,
    pub candidate_count: usize,
    pub candidates: Vec<SceneSourceCandidate>,
}

/// Compiled context is absent only for legacy whole-project task entry points that do
/// not have a displayed preview. Scoped UI submissions always carry it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledScope {
    pub preview: PreviewIdentity,
    pub fps: usize,
    pub total_frames: usize,
    pub start_frame: usize,
    pub end_frame: usize,
    /// Intersecting scene instances plus immediate neighbours, bounded for prompts.
    pub scenes: Vec<ScopedScene>,
    /// Required selected and adjacent scene-boundary frame indexes.
    pub boundary_frames: Vec<usize>,
    pub scene_context_truncated: bool,
}

/// Project/source-bound selection frozen at submit time. Frame ranges are half-open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskScope {
    pub project_id: String,
    pub source_revision: String,
    pub selection: ScopeSelection,
    pub compiled: Option<CompiledScope>,
    /// Exact, bounded path/name matches. These are hints, never source spans or
    /// declarations proved by a parser.
    pub scene_sources: Vec<SceneSourceReference>,
    pub scene_source_search_truncated: bool,
    pub style_snapshot: Option<StyleSnapshotIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskScopeError {
    #[error("scope project/source identity does not match the submitted project")]
    Identity,
    #[error("scope has an invalid compiled timebase or frame range")]
    Range,
    #[error("selected scene instance is not present in the captured timeline")]
    MissingScene,
    #[error("compiled timeline is invalid: {0}")]
    Timeline(String),
    #[error("project Rust sources changed during bounded scene lookup")]
    SourceChanged,
}

impl TaskScope {
    /// Legacy task path: exact project/source identity, with no compiled selection.
    pub fn whole_project(
        project_id: impl Into<String>,
        source_revision: impl Into<String>,
    ) -> Self {
        Self {
            project_id: project_id.into(),
            source_revision: source_revision.into(),
            selection: ScopeSelection::WholeProject,
            compiled: None,
            scene_sources: Vec::new(),
            scene_source_search_truncated: false,
            style_snapshot: None,
        }
    }

    /// Freeze the current timeline selection against the identity that produced it.
    pub fn from_timeline(
        timeline: &PreviewTimelineResponse,
        selected: &TimelineSelection,
    ) -> Result<Self, TaskScopeError> {
        crate::validate_preview_timeline(timeline).map_err(TaskScopeError::Timeline)?;
        let preview = &timeline.envelope.identity;
        if preview.project_id.is_empty()
            || preview.source_revision.len() != 64
            || !preview
                .source_revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || timeline.fps == 0
        {
            return Err(TaskScopeError::Identity);
        }

        let selected_scene = match selected.scene_id.as_deref() {
            Some(id) => Some(
                timeline
                    .scenes
                    .iter()
                    .find(|scene| scene.instance_id == id)
                    .ok_or(TaskScopeError::MissingScene)?,
            ),
            None => None,
        };
        let (selection, start_frame, end_frame) = if let Some(scene) = selected_scene {
            (
                ScopeSelection::Scene {
                    instance_id: scene.instance_id.clone(),
                    name: scene.name.clone(),
                    full_name: scene.full_name.clone(),
                },
                scene.start_frame,
                scene.end_frame,
            )
        } else if let Some(range) = &selected.range {
            (
                ScopeSelection::FrameRange,
                range.start.min(range.end),
                range.start.max(range.end),
            )
        } else {
            (ScopeSelection::WholeProject, 0, timeline.total_frames)
        };

        if start_frame > end_frame || end_frame > timeline.total_frames {
            return Err(TaskScopeError::Range);
        }
        if timeline.scenes.iter().any(|scene| {
            [
                scene.instance_id.as_str(),
                scene.name.as_str(),
                scene.full_name.as_str(),
            ]
            .iter()
            .any(|value| value.len() > 256 || value.chars().any(char::is_control))
        }) {
            return Err(TaskScopeError::Timeline(
                "scene names exceed the bounded plain-text context".into(),
            ));
        }

        let mut included = Vec::new();
        for (index, scene) in timeline.scenes.iter().enumerate() {
            if matches!(&selection, ScopeSelection::WholeProject)
                || (start_frame < end_frame
                    && scene.start_frame < end_frame
                    && start_frame < scene.end_frame)
                || matches!(
                    &selection,
                    ScopeSelection::Scene { instance_id, .. }
                        if scene.instance_id == *instance_id
                )
            {
                included.push(index);
            }
        }
        if let ScopeSelection::Scene { instance_id, .. } = &selection
            && !included
                .iter()
                .any(|index| timeline.scenes[*index].instance_id == *instance_id)
        {
            return Err(TaskScopeError::MissingScene);
        }

        // Include one adjacent scene on either side to expose transitions without
        // pretending the selection isolates shared scene code.
        if !matches!(&selection, ScopeSelection::WholeProject) && !included.is_empty() {
            let first = *included.first().expect("checked non-empty");
            let last = *included.last().expect("checked non-empty");
            if first > 0 {
                included.push(first - 1);
            }
            if last + 1 < timeline.scenes.len() {
                included.push(last + 1);
            }
            included.sort_unstable();
            included.dedup();
        }

        let mut scene_context_truncated = included.len() > MAX_SCOPE_SCENES;
        included.truncate(MAX_SCOPE_SCENES);
        let scenes: Vec<_> = included
            .iter()
            .map(|index| &timeline.scenes[*index])
            .map(|scene| ScopedScene {
                instance_id: scene.instance_id.clone(),
                name: scene.name.clone(),
                full_name: scene.full_name.clone(),
                start_frame: scene.start_frame,
                end_frame: scene.end_frame,
            })
            .collect();

        let mut boundaries = BTreeSet::new();
        if start_frame < end_frame {
            boundaries.insert(start_frame);
            boundaries.insert(end_frame - 1);
        }
        for scene in &scenes {
            for boundary in [scene.start_frame, scene.end_frame] {
                if boundary > 0 {
                    boundaries.insert(boundary - 1);
                }
                if boundary < timeline.total_frames {
                    boundaries.insert(boundary);
                }
            }
        }
        if boundaries.len() > MAX_SCOPE_BOUNDARIES {
            scene_context_truncated = true;
            let required = if start_frame < end_frame {
                [Some(start_frame), Some(end_frame - 1)]
                    .into_iter()
                    .flatten()
                    .collect::<BTreeSet<_>>()
            } else {
                BTreeSet::new()
            };
            let mut bounded = required.clone();
            bounded.extend(
                boundaries
                    .into_iter()
                    .filter(|frame| !required.contains(frame))
                    .take(MAX_SCOPE_BOUNDARIES - required.len()),
            );
            boundaries = bounded;
        }

        let scope = Self {
            project_id: preview.project_id.clone(),
            source_revision: preview.source_revision.clone(),
            selection,
            compiled: Some(CompiledScope {
                preview: preview.clone(),
                fps: timeline.fps,
                total_frames: timeline.total_frames,
                start_frame,
                end_frame,
                scenes,
                boundary_frames: boundaries.into_iter().collect(),
                scene_context_truncated,
            }),
            scene_sources: Vec::new(),
            scene_source_search_truncated: false,
            style_snapshot: None,
        };
        scope.validate()?;
        Ok(scope)
    }

    pub fn validate(&self) -> Result<(), TaskScopeError> {
        if self.project_id.is_empty()
            || self.source_revision.len() != 64
            || !self
                .source_revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(TaskScopeError::Identity);
        }
        let Some(compiled) = &self.compiled else {
            if !matches!(self.selection, ScopeSelection::WholeProject) {
                return Err(TaskScopeError::Identity);
            }
            if !self.scene_sources.is_empty() || self.scene_source_search_truncated {
                return Err(TaskScopeError::Identity);
            }
            return Ok(());
        };
        if compiled.preview.project_id != self.project_id
            || compiled.preview.source_revision != self.source_revision
            || compiled.preview.open_session.is_empty()
            // Generation 0 is a legitimate identity: a preview adopted from a promotion
            // carries it. Staleness is full-identity equality, never "non-zero".
            || compiled.fps == 0
            || compiled.start_frame > compiled.end_frame
            || compiled.end_frame > compiled.total_frames
            || compiled.scenes.len() > MAX_SCOPE_SCENES
            || compiled.boundary_frames.len() > MAX_SCOPE_BOUNDARIES
            || compiled
                .boundary_frames
                .iter()
                .any(|frame| *frame >= compiled.total_frames)
        {
            return Err(TaskScopeError::Range);
        }
        if let ScopeSelection::Scene { instance_id, .. } = &self.selection
            && !compiled.scenes.iter().any(|scene| {
                &scene.instance_id == instance_id
                    && scene.start_frame == compiled.start_frame
                    && scene.end_frame == compiled.end_frame
            })
        {
            return Err(TaskScopeError::MissingScene);
        }
        if matches!(&self.selection, ScopeSelection::WholeProject)
            && (compiled.start_frame != 0 || compiled.end_frame != compiled.total_frames)
        {
            return Err(TaskScopeError::Range);
        }
        if self.scene_sources.len() > MAX_SOURCE_SCENES
            || self.scene_sources.iter().any(|reference| {
                !compiled
                    .scenes
                    .iter()
                    .any(|scene| scene.instance_id == reference.instance_id)
                    || reference.candidates.len() > MAX_SOURCE_CANDIDATES_PER_SCENE
                    || reference.candidate_count < reference.candidates.len()
                    || match reference.resolution {
                        SceneSourceResolution::Missing => reference.candidate_count != 0,
                        SceneSourceResolution::Unique => reference.candidate_count != 1,
                        SceneSourceResolution::Ambiguous => reference.candidate_count <= 1,
                    }
                    || reference.candidates.iter().any(|candidate| {
                        !safe_source_path(&candidate.path)
                            || !is_rust_identifier(&candidate.symbol)
                            || !is_sha256(&candidate.sha256)
                            || candidate.match_count == 0
                    })
            })
        {
            return Err(TaskScopeError::Identity);
        }
        if let Some(style) = &self.style_snapshot
            && (style.preset_id.is_empty()
                || style.preset_id.len() > 128
                || !is_sha256(&style.preset_hash)
                || !is_sha256(&style.resolved_tokens_hash))
        {
            return Err(TaskScopeError::Identity);
        }
        Ok(())
    }

    /// Find bounded exact Rust identifier matches for the frozen compiled scenes.
    /// Every file is read only from the captured inventory and re-hashed before its
    /// relative path can be exposed in the task prompt.
    pub fn resolve_scene_sources(
        &mut self,
        root: &Path,
        source_files: &[SourceFile],
    ) -> Result<(), TaskScopeError> {
        self.scene_sources.clear();
        self.scene_source_search_truncated = false;
        let Some(compiled) = &self.compiled else {
            return Ok(());
        };

        let scenes: Vec<_> = compiled.scenes.iter().take(MAX_SOURCE_SCENES).collect();
        self.scene_source_search_truncated = compiled.scenes.len() > scenes.len();
        if scenes.is_empty() {
            return Ok(());
        }

        let scene_symbols: Vec<_> = scenes.iter().map(|scene| rust_symbols(scene)).collect();
        let symbols: BTreeSet<String> = scene_symbols.iter().flatten().cloned().collect();
        let canonical_root = root
            .canonicalize()
            .map_err(|_| TaskScopeError::SourceChanged)?;
        let mut rust_files: Vec<_> = source_files
            .iter()
            .filter(|file| file.kind == FileKind::Rust)
            .collect();
        rust_files.sort_by(|left, right| left.path.as_str().cmp(right.path.as_str()));
        if rust_files.len() > MAX_SOURCE_FILES {
            self.scene_source_search_truncated = true;
        }

        let mut total_bytes = 0_u64;
        let mut documents = Vec::new();
        for source in rust_files.into_iter().take(MAX_SOURCE_FILES) {
            if source.size > MAX_SOURCE_FILE_BYTES
                || total_bytes.saturating_add(source.size) > MAX_SOURCE_BYTES
            {
                self.scene_source_search_truncated = true;
                continue;
            }
            let relative = source.path.as_str();
            if !safe_source_path(relative) {
                return Err(TaskScopeError::SourceChanged);
            }
            let path = root.join(relative);
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| TaskScopeError::SourceChanged)?;
            if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                return Err(TaskScopeError::SourceChanged);
            }
            let canonical_path = path
                .canonicalize()
                .map_err(|_| TaskScopeError::SourceChanged)?;
            if !canonical_path.starts_with(&canonical_root) {
                return Err(TaskScopeError::SourceChanged);
            }
            let bytes = fs::read(&canonical_path).map_err(|_| TaskScopeError::SourceChanged)?;
            if bytes.len() as u64 != source.size
                || format!("{:x}", Sha256::digest(&bytes)) != source.sha256
            {
                return Err(TaskScopeError::SourceChanged);
            }
            total_bytes = total_bytes.saturating_add(source.size);
            let Ok(text) = std::str::from_utf8(&bytes) else {
                self.scene_source_search_truncated = true;
                continue;
            };
            let mut counts = BTreeMap::<String, usize>::new();
            for token in text.split(|ch: char| !(ch == '_' || ch.is_ascii_alphanumeric())) {
                if symbols.contains(token) {
                    let count = counts.entry(token.to_owned()).or_default();
                    *count = count.saturating_add(1);
                }
            }
            documents.push((relative.to_owned(), source.sha256.clone(), counts));
        }

        for (scene, symbols) in scenes.into_iter().zip(scene_symbols) {
            let mut candidates = Vec::new();
            for (path, sha256, counts) in &documents {
                for symbol in &symbols {
                    if let Some(match_count) = counts.get(symbol) {
                        candidates.push(SceneSourceCandidate {
                            path: path.clone(),
                            symbol: symbol.clone(),
                            sha256: sha256.clone(),
                            confidence: SourceMatchConfidence::ExactIdentifier,
                            match_count: *match_count,
                        });
                    }
                }
            }
            candidates.sort_by(|left, right| {
                left.path
                    .cmp(&right.path)
                    .then_with(|| left.symbol.cmp(&right.symbol))
            });
            let candidate_count = candidates.len();
            let resolution = match candidate_count {
                0 => SceneSourceResolution::Missing,
                1 => SceneSourceResolution::Unique,
                _ => SceneSourceResolution::Ambiguous,
            };
            candidates.truncate(MAX_SOURCE_CANDIDATES_PER_SCENE);
            self.scene_sources.push(SceneSourceReference {
                instance_id: scene.instance_id.clone(),
                resolution,
                candidate_count,
                candidates,
            });
        }
        Ok(())
    }

    pub fn is_current_preview(&self, current: &PreviewIdentity) -> bool {
        self.compiled
            .as_ref()
            .is_some_and(|scope| &scope.preview == current)
    }

    pub fn label(&self) -> String {
        let Some(compiled) = &self.compiled else {
            return "Whole project".into();
        };
        match &self.selection {
            ScopeSelection::WholeProject => {
                format!("Whole project · [0..{})", compiled.total_frames)
            }
            ScopeSelection::Scene { name, .. } => format!(
                "Scene {name} · [{}..{})",
                compiled.start_frame, compiled.end_frame
            ),
            ScopeSelection::FrameRange => {
                format!("Frames [{}..{})", compiled.start_frame, compiled.end_frame)
            }
        }
    }

    pub fn prompt_context(&self) -> String {
        let Some(compiled) = &self.compiled else {
            return "Requested scope: whole project (legacy submission; no displayed compiled timeline was attached).".into();
        };
        let mut text = format!(
            "Requested scope: {} at {} fps, source {} (preview worker generation {}).\n",
            self.label(),
            compiled.fps,
            &self.source_revision[..12],
            compiled.preview.worker_generation
        );
        if !compiled.scenes.is_empty() {
            text.push_str(
                "Compiled scene context (selected/overlapping and adjacent instances):\n",
            );
            for scene in &compiled.scenes {
                text.push_str(&format!(
                    "- {} ({}) [{}..{})\n",
                    scene.name, scene.instance_id, scene.start_frame, scene.end_frame
                ));
            }
        }
        if compiled.scene_context_truncated {
            text.push_str("Scene context is truncated; treat scope boundaries as incomplete.\n");
        }
        if !self.scene_sources.is_empty() {
            text.push_str("Best-effort exact scene-name source candidates (no AST or source spans; verify before editing):\n");
            for reference in &self.scene_sources {
                match reference.resolution {
                    SceneSourceResolution::Missing => text.push_str(&format!(
                        "- {}: no exact Rust identifier match found.\n",
                        reference.instance_id
                    )),
                    SceneSourceResolution::Unique | SceneSourceResolution::Ambiguous => {
                        text.push_str(&format!(
                            "- {}: {:?} ({} candidate(s)).\n",
                            reference.instance_id, reference.resolution, reference.candidate_count
                        ));
                        for candidate in &reference.candidates {
                            text.push_str(&format!(
                                "  - {}::{} sha256={} confidence=exact_identifier occurrences={}\n",
                                candidate.path,
                                candidate.symbol,
                                &candidate.sha256[..12],
                                candidate.match_count
                            ));
                        }
                    }
                }
            }
        } else {
            text.push_str(
                "No compiled scene source candidates were available for this submission.\n",
            );
        }
        if self.scene_source_search_truncated {
            text.push_str(
                "Scene source search was bounded/truncated; candidates may be incomplete.\n",
            );
        }
        if !compiled.boundary_frames.is_empty() {
            text.push_str(&format!(
                "Required selected/adjacent boundary frames: {}\n",
                compiled
                    .boundary_frames
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        text.push_str("Scope narrows attention only; shared Rust, Cargo, style, media, configuration, or uncertain changes require broader validation.\n");
        match &self.style_snapshot {
            Some(style) => text.push_str(&format!(
                "Active style snapshot: preset {} (preset hash {}, resolved tokens hash {}).\n",
                style.preset_id,
                &style.preset_hash[..12],
                &style.resolved_tokens_hash[..12]
            )),
            None => text.push_str("No active style snapshot was attached to this task context.\n"),
        }
        text
    }
}

fn rust_symbols(scene: &ScopedScene) -> BTreeSet<String> {
    let mut symbols = BTreeSet::new();
    if is_rust_identifier(&scene.name) {
        symbols.insert(scene.name.clone());
    }
    let full_name = scene.full_name.trim_start_matches('<');
    let leaf = full_name
        .rsplit("::")
        .next()
        .unwrap_or(full_name)
        .split(['<', ' ', '>'])
        .next()
        .unwrap_or_default();
    if is_rust_identifier(leaf) {
        symbols.insert(leaf.to_owned());
    }
    symbols
}

fn is_rust_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn safe_source_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('/')
        && !value.chars().any(|ch| matches!(ch, '\\' | ':' | '\0'))
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// An optional frozen style reference. The Phase 3 preset implementation supplies the
/// resolved-token digest; absence must be represented honestly, never inferred.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleSnapshotIdentity {
    pub preset_id: String,
    pub preset_hash: String,
    pub resolved_tokens_hash: String,
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
