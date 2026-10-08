//! Durable file-set transactions: Apply and Undo of a validated task revision.
//!
//! A transaction publishes the difference between the current source and a validated
//! candidate as a set of create / replace / delete operations (a rename is a delete plus
//! a create; an executable-bit change is a replace with the same bytes). It is recorded in
//! the task journal ([`crate::journal::TaskJournal`]) as an immutable *intent* that names
//! the complete forward file set, the expected before/after states and every recovery
//! path, followed by synced per-operation *progress* and exactly one terminal event.
//!
//! # Publication protocol (per operation)
//!
//! 1. The after bytes are staged next to their destination under an exact app-generated
//!    name, with the right mode, and synced. The stage's inode is journalled *before*
//!    any byte is written so a crashed partial stage can be recognised as ours.
//! 2. The live original is verified (identity, bytes, mode) and *moved* with an atomic
//!    `renameat2(RENAME_NOREPLACE)` to a uniquely reserved recovery slot; the displaced
//!    file is verified again and the directory synced. A link-then-unlink pair is never
//!    used to move a live name (an editor saving between the two calls would lose its
//!    file); where `renameat2` is unavailable the Apply gate is `Blocked`.
//! 3. The staged file is published with a no-clobber rename. A destination that an
//!    outside writer recreated in between is never overwritten: every observed variant
//!    is retained and the transaction halts as a conflict.
//!
//! Directory identity is checked around each step, nothing follows a link, and the root
//! *pathname* is re-bound (no-follow walk from `/`) around publication and the final
//! inventory, so a root swapped for a link is a conflict.
//!
//! # Topology (file <-> directory)
//!
//! The schedule is durable in the intent, per operation: deletes come first (lowest
//! indexes); a create whose new directory occupies the name of a file the revision
//! deletes has `defer_stage` (it is staged and its directory created only after the
//! file was displaced); a create whose name is a directory the revision empties lists
//! it in `vacates` (removed, deepest first, after every displacement and before the
//! create publishes) and the displaced originals of those deletes use a `slot_dir`
//! outside the vacated tree. Rollback runs the exact inverse: take the file down,
//! recreate the vacated directories (with their recorded mode), remove the directories
//! the operation created, then restore the displaced originals.
//!
//! # Recovery
//!
//! Replay never rolls forward. For a transaction without a durable commit every
//! operation's state is *observed* (never assumed from progress records) and rolled
//! back only where the current bytes still equal the transaction's after bytes and the
//! displaced original is verified. A partially written stage is collected only when its
//! journalled inode is still the file and its bytes are a prefix of the expected bytes.
//! Unknown bytes are never overwritten: such an operation stays as it is, the
//! transaction ends as a conflict, and new mutation stays blocked until the user
//! resolves it. A committed transaction only needs its recovery slots collected.
use crate::{
    EngineError, candidate_validation::BuildIdentity, journal::TaskJournal, state::OpenSession,
};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "linux")]
use std::collections::BTreeSet;
use std::{collections::BTreeMap, io, path::Path};
use studio_project::{
    SourceInventory, SourceRevision, checkpoint::Checkpoints, revision::SourceFile,
};

#[cfg(target_os = "linux")]
pub use studio_project::publish::{FileId, NoClobber};

/// Stand-ins so the data model compiles everywhere; only Linux can execute.
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId {
    pub dev: u64,
    pub ino: u64,
}
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoClobber {
    Renameat2,
    Link,
}

/// Explicit format version of every event in the task journal.
pub const TASK_FORMAT: u32 = 2;
/// Most file operations one transaction may carry (the intent is a single journal line).
pub const MAX_FILE_OPS: usize = 2048;
const MAX_SUMMARY_CHARS: usize = 512;

// ---- data model ------------------------------------------------------------------------

/// Content and mode of one file as the inventory sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}

impl From<&SourceFile> for FileState {
    fn from(file: &SourceFile) -> Self {
        Self {
            sha256: file.sha256.clone(),
            size: file.size,
            executable: file.executable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpKind {
    Create,
    /// New bytes and/or a changed executable bit.
    Replace,
    Delete,
}

/// One changed path with the immutable before/after objects (content hashes into the
/// checkpoint object store) and, once planned, the exact permission bits on both sides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDelta {
    pub path: String,
    pub before: Option<FileState>,
    pub after: Option<FileState>,
    /// Exact permission bits (`st_mode & 0o7777`) the original had when the plan was
    /// made. `None` for a create, and for legacy records that only knew the executable
    /// bit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_mode: Option<u32>,
    /// Exact permission bits the published file carries. `None` for a delete, and for
    /// legacy records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_mode: Option<u32>,
}

impl FileDelta {
    pub fn kind(&self) -> OpKind {
        match (&self.before, &self.after) {
            (None, _) => OpKind::Create,
            (Some(_), None) => OpKind::Delete,
            (Some(_), Some(_)) => OpKind::Replace,
        }
    }

    /// The same change run backwards, exact permission bits included: restoring the
    /// original reproduces the bits it had, and the guard on the other side is the bits
    /// the edit left.
    pub fn inverse(&self) -> Self {
        Self {
            path: self.path.clone(),
            before: self.after.clone(),
            after: self.before.clone(),
            before_mode: self.after_mode,
            after_mode: self.before_mode,
        }
    }
}

/// Forward delta from `from` to `to` (both sorted inventories): additions, deletions and
/// every path whose bytes or executable bit differ. Permission bits are not known from
/// an inventory; planning fills them in from the live files.
pub fn deltas_between(from: &SourceInventory, to: &SourceInventory) -> Vec<FileDelta> {
    let old: BTreeMap<&str, &SourceFile> =
        from.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let new: BTreeMap<&str, &SourceFile> = to.files.iter().map(|f| (f.path.as_str(), f)).collect();
    let delta = |path: &str, before: Option<FileState>, after: Option<FileState>| FileDelta {
        path: path.to_owned(),
        before,
        after,
        before_mode: None,
        after_mode: None,
    };
    let mut deltas = Vec::new();
    for (path, before) in &old {
        match new.get(path) {
            None => deltas.push(delta(path, Some((*before).into()), None)),
            Some(after) => {
                let (b, a): (FileState, FileState) = ((*before).into(), (*after).into());
                if a != b {
                    deltas.push(delta(path, Some(b), Some(a)));
                }
            }
        }
    }
    for (path, after) in &new {
        if !old.contains_key(path) {
            deltas.push(delta(path, None, Some((*after).into())));
        }
    }
    deltas.sort_by(|a, b| a.path.cmp(&b.path));
    deltas
}

/// The inverse of a whole delta set.
pub fn invert(deltas: &[FileDelta]) -> Vec<FileDelta> {
    deltas.iter().map(FileDelta::inverse).collect()
}

/// A touched path whose current state is not what the edit left behind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathConflict {
    pub path: String,
    pub expected: Option<FileState>,
    pub found: Option<FileState>,
    pub reason: String,
}

/// Undo's only precondition: on every path the original edit touched the current
/// source must still hold exactly the edit's after state (bytes and executable bit).
/// Paths outside the edit are never examined, so unrelated changes are preserved.
pub fn touched_conflicts(deltas: &[FileDelta], current: &SourceInventory) -> Vec<PathConflict> {
    let files: BTreeMap<&str, &SourceFile> =
        current.files.iter().map(|f| (f.path.as_str(), f)).collect();
    deltas
        .iter()
        .filter_map(|delta| {
            let found: Option<FileState> = files.get(delta.path.as_str()).map(|f| (*f).into());
            if found == delta.after {
                return None;
            }
            let reason = match (&delta.after, &found) {
                (Some(_), None) => "was deleted after the edit",
                (None, Some(_)) => "was recreated after the edit",
                (Some(a), Some(f)) if a.sha256 == f.sha256 => "had its executable bit changed",
                _ => "was modified after the edit",
            };
            Some(PathConflict {
                path: delta.path.clone(),
                expected: delta.after.clone(),
                found,
                reason: reason.into(),
            })
        })
        .collect()
}

/// Undo's guard on exact permission bits: files whose content is still the edit's but
/// whose permission bits differ from the bits the edit left (a `chmod` since). Legacy
/// records that never knew the bits are not examined. Only touches the filesystem to
/// `lstat` the recorded paths.
#[cfg(target_os = "linux")]
pub fn mode_conflicts(root: &Path, deltas: &[FileDelta]) -> Vec<PathConflict> {
    use studio_project::publish::Dir;
    let Ok(root_dir) = Dir::open_root(root) else {
        return Vec::new();
    };
    deltas
        .iter()
        .filter_map(|delta| {
            let expected = delta.after_mode?;
            let after = delta.after.as_ref()?;
            let (dir, name) = split_path(&delta.path);
            let stat = root_dir.open_relative(dir).ok()?.stat(name).ok()??;
            (stat.is_regular() && stat.mode != expected).then(|| PathConflict {
                path: delta.path.clone(),
                expected: Some(after.clone()),
                found: Some(after.clone()),
                reason: format!(
                    "had its permission bits changed after the edit ({:04o}; the edit left {expected:04o})",
                    stat.mode
                ),
            })
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
pub fn mode_conflicts(_root: &Path, _deltas: &[FileDelta]) -> Vec<PathConflict> {
    Vec::new()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
    Apply,
    Undo,
    /// A Studio-originated preset snapshot mutation (see [`crate::preset_state`]). It
    /// shares the durable file-set protocol but is never accepted task history: replay
    /// keeps it out of [`crate::journal::TaskReplay::committed`].
    Preset,
}

/// A validated task revision: what one accepted agent edit (or Undo) did to the source.
///
/// This is *not* a saved checkpoint. Checkpoints (`ProjectState::accepted`) prove bytes
/// only; a task revision names the task, its source base, the saved history it started
/// from, the validated candidate, the revision that was actually published, the
/// before/after objects of every changed path, and the validation evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRevisionRecord {
    /// Transaction id (32 lowercase hex digits); also the history identity of this entry.
    pub id: String,
    pub kind: TransactionKind,
    pub task: String,
    pub generation: u64,
    pub session: OpenSession,
    /// The source the task was captured from (for Undo: the source it was formed from).
    pub task_base: SourceRevision,
    /// Accepted task-history head when the task started.
    pub prior_history: Option<String>,
    /// The M1 saved checkpoint when the task started; informational.
    pub prior_checkpoint: SourceRevision,
    /// The validated candidate.
    pub candidate: SourceRevision,
    /// The source revision that publication produced (equals `candidate`).
    pub published: SourceRevision,
    /// The revision this entry reverses, for Undo.
    pub undoes: Option<String>,
    pub prompt_summary: String,
    pub changes: Vec<FileDelta>,
    /// Directories this revision created (relative, shallowest first); Undo removes the
    /// ones that are empty again.
    #[serde(default)]
    pub created_dirs: Vec<String>,
    /// SDK / build key of the validated candidate.
    pub build: Option<BuildIdentity>,
    pub validation_report_sha256: String,
    pub committed_unix: u64,
    /// Present exactly on [`TransactionKind::Preset`] records: what the preset mutation
    /// was. Older records never carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<crate::preset_state::PresetProvenance>,
}

impl TaskRevisionRecord {
    pub fn changed_paths(&self) -> impl Iterator<Item = &str> {
        self.changes.iter().map(|c| c.path.as_str())
    }
}

/// Bounded one-line summary of a brief.
pub fn prompt_summary(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(MAX_SUMMARY_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line,
    }
}

/// One planned operation with everything needed to execute, verify and roll it back.
///
/// The exact permission bits (observed before, published after) live in `delta`
/// (`before_mode` / `after_mode`), which is also what the committed record keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOp {
    pub index: usize,
    #[serde(flatten)]
    pub delta: FileDelta,
    pub kind: OpKind,
    /// Parent directory relative to the project root (`""` for the root).
    pub dir: String,
    pub name: String,
    /// Identity of the destination observed during preflight.
    pub before_id: Option<FileId>,
    /// Directories (relative, shallowest first) that do not exist yet.
    pub new_dirs: Vec<String>,
    /// File -> directory: a new directory of this create occupies the name of a file the
    /// revision deletes, so the stage (and its directory) is only made after every
    /// displacement.
    #[serde(default)]
    pub defer_stage: bool,
    /// Directory -> file: the directories (deepest first) this create removes, once every
    /// delete beneath them was displaced, before it publishes.
    #[serde(default)]
    pub vacates: Vec<VacatedDir>,
    /// Where the displaced original lives when it must not stay in the op's own
    /// directory (that directory is vacated by the revision). `None`: the op's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_dir: Option<String>,
    pub mechanism: NoClobber,
    pub recovery: RecoveryPaths,
}

impl FileOp {
    /// Permission bits observed on the original during planning.
    pub fn before_mode(&self) -> Option<u32> {
        self.delta.before_mode
    }
    /// Permission bits the published file must carry.
    pub fn after_mode(&self) -> Option<u32> {
        self.delta.after_mode
    }
    /// The directory (relative to the root) that holds the displaced original.
    pub fn original_dir(&self) -> &str {
        self.slot_dir.as_deref().unwrap_or(&self.dir)
    }
}

/// A directory a revision removes (it becomes empty once its files are displaced) with
/// what rollback needs to put it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VacatedDir {
    pub path: String,
    pub id: FileId,
    /// Permission bits of the directory when planned.
    pub mode: u32,
}

/// Exact names of this operation's transaction files. The stage and rollback slots live
/// in the destination directory; the displaced original in [`FileOp::original_dir`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryPaths {
    pub stage: String,
    pub original: String,
    pub rollback: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirIdentity {
    pub path: String,
    pub id: FileId,
}

/// The immutable plan journalled before any source byte changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionIntent {
    pub format: u32,
    pub id: String,
    pub kind: TransactionKind,
    /// The source revision the operations were planned against.
    pub base: SourceRevision,
    /// The inventory that must result: the validated candidate.
    pub expected: SourceRevision,
    pub ops: Vec<FileOp>,
    pub dirs: Vec<DirIdentity>,
    /// Directories to remove after the commit if they are empty (Undo of a creation).
    #[serde(default)]
    pub remove_dirs: Vec<String>,
    /// The record that will be committed once the final inventory verifies.
    pub record: TaskRevisionRecord,
}

impl TransactionIntent {
    #[cfg(target_os = "linux")]
    fn dir_id(&self, path: &str) -> Option<FileId> {
        self.dirs.iter().find(|d| d.path == path).map(|d| d.id)
    }

    /// Whether the revision itself removes (and, on rollback, recreates) `path`.
    #[cfg(target_os = "linux")]
    fn is_vacated(&self, path: &str) -> bool {
        self.ops
            .iter()
            .any(|op| op.vacates.iter().any(|v| v.path == path))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// The stage file exists (empty); `detail` is its `dev:ino`, journalled before any
    /// byte is written so a crashed partial stage can be recognised as ours.
    StageCreated,
    Staged,
    DirCreated,
    /// A directory the revision empties was removed (directory -> file).
    DirRemoved,
    Displaced,
    Published,
    RolledBack,
    Retained,
    Cleaned,
}

/// A durable boundary reached by one operation (or the whole transaction).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub op: Option<usize>,
    pub step: Step,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpConflict {
    pub index: usize,
    pub path: String,
    pub detail: String,
}

/// A file kept on disk because it may hold bytes nobody else has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedVariant {
    /// Path relative to the project root.
    pub path: String,
    pub role: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
}

/// Why a transaction ended in conflict, with every variant it retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictReport {
    pub transaction: String,
    pub reason: String,
    pub ops: Vec<OpConflict>,
    pub variants: Vec<RetainedVariant>,
}

impl std::fmt::Display for ConflictReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)?;
        for op in &self.ops {
            write!(f, "; {}: {}", op.path, op.detail)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskEvent {
    Intent(Box<TransactionIntent>),
    Progress(Progress),
    Commit(Box<TaskRevisionRecord>),
    RolledBack { reason: String },
    Conflict(Box<ConflictReport>),
    Resolved { note: String },
}

impl TaskEvent {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Intent(_) => "intent",
            Self::Progress(_) => "progress",
            Self::Commit(_) => "commit",
            Self::RolledBack { .. } => "rolled_back",
            Self::Conflict(_) => "conflict",
            Self::Resolved { .. } => "resolved",
        }
    }
}

// ---- fault injection -------------------------------------------------------------------

/// A durable boundary. Tests inject faults at each one; production uses [`NoHooks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Boundary {
    /// Before / after a journal append of `event` (`after == false`: not yet written).
    Append {
        event: &'static str,
        op: Option<usize>,
        step: Option<Step>,
        after: bool,
    },
    DirCreated(usize),
    /// A directory the revision empties was removed (directory -> file).
    DirRemoved(usize),
    /// Rollback recreated a directory the revision had removed.
    DirRestored(usize),
    /// The stage exists and its inode is journalled; no byte is written yet.
    StageCreated(usize),
    /// The after bytes are written but not yet synced.
    StageWritten(usize),
    StageSynced(usize),
    BeforeDisplace(usize),
    AfterDisplace(usize),
    DisplacedVerified(usize),
    BeforePublish(usize),
    AfterPublish(usize),
    PublishVerified(usize),
    FinalInventory,
    InventoryVerified,
    /// Recovery / rollback steps of one operation.
    RollbackBegin(usize),
    RollbackMoved(usize),
    RollbackRestored(usize),
    Cleanup(usize),
    /// Controller-side: SQLite history record and task-revision projection.
    Database {
        after: bool,
    },
    Projection {
        after: bool,
    },
    /// Controller-side: the manifest's preset reference is replaced after a preset
    /// file set committed.
    Manifest {
        after: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// The process dies here: nothing after this point runs, nothing is cleaned up.
    Crash,
    /// The operation at this boundary fails with an I/O error (for example disk full).
    Io(io::ErrorKind),
}

pub trait TransactionHooks: Send + Sync {
    fn at(&self, boundary: &Boundary) -> Result<(), Fault>;
}

pub struct NoHooks;
impl TransactionHooks for NoHooks {
    fn at(&self, _: &Boundary) -> Result<(), Fault> {
        Ok(())
    }
}
impl<F: Fn(&Boundary) -> Result<(), Fault> + Send + Sync> TransactionHooks for F {
    fn at(&self, boundary: &Boundary) -> Result<(), Fault> {
        self(boundary)
    }
}

// ---- errors ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("the candidate does not change the project source")]
    NoChange,
    #[error("the candidate changes {0} files; at most {MAX_FILE_OPS} are supported per revision")]
    TooManyChanges(usize),
    #[error("{path}: {reason}")]
    Conflict { path: String, reason: String },
    #[error("publication is blocked: {0}")]
    GateBlocked(String),
    #[error("{0}")]
    Io(String),
}

/// Why promotion (Apply or Undo) did not produce an accepted task revision.
#[derive(Debug, thiserror::Error)]
pub enum PromotionError {
    #[error("Apply is blocked: {0}; the retained draft can still be reviewed or exported")]
    GateBlocked(String),
    #[error(
        "a previous source mutation is unresolved ({0}); resolve it before applying or undoing"
    )]
    Unresolved(String),
    #[error(
        "the project source changed after the task started ({expected} -> {found}); the candidate is preserved, start a new task from the current source"
    )]
    SourceChanged { expected: String, found: String },
    #[error(
        "the accepted task history changed after the task started; the candidate is preserved, start a new task from the current source"
    )]
    HistoryChanged,
    #[error(
        "the saved checkpoint history changed after the task started; the candidate is preserved, start a new task from the current source"
    )]
    SavedHistoryChanged,
    #[error("this Undo preparation is stale: {0}")]
    StalePreparation(String),
    #[error("publication was declined before it began; nothing was written")]
    Declined,
    #[error("{0}")]
    Plan(#[from] PlanError),
    #[error("publication halted on a conflict and left every observed variant in place: {0}")]
    Conflict(Box<ConflictReport>),
    #[error("publication was rolled back: {0}")]
    RolledBack(String),
    #[error("history journal failure: {0}; mutation is suspended until the project is reopened")]
    Journal(String),
    #[error("the process was interrupted at an injected boundary")]
    Crashed,
    #[error("the validation report does not authorize this candidate: {0}")]
    Unauthorized(String),
    #[error("Undo is unavailable: {0}")]
    UndoUnavailable(String),
    #[error("Undo conflicts with later changes to {}", .0.iter().map(|c| c.path.as_str()).collect::<Vec<_>>().join(", "))]
    UndoConflict(Vec<PathConflict>),
}

/// What an execution reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Committed(Box<TaskRevisionRecord>),
    RolledBack(String),
    Conflicted(Box<ConflictReport>),
}

/// Reasons execution stops without a terminal outcome. Nothing is cleaned up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Interrupt {
    /// Injected process death (tests).
    Crash,
    /// The journal cannot be appended: mutation is suspended and all variants stay for
    /// recovery on the next open.
    Journal(String),
}

/// What replay did to unfinished transactions on open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub rolled_back: Vec<String>,
    pub conflicts: Vec<ConflictReport>,
    pub cleaned: Vec<String>,
    /// The filesystem under the project root may have changed.
    pub touched_source: bool,
}

// ---- gate ------------------------------------------------------------------------------

/// Whether this platform and filesystem can publish without clobbering. Apply and Undo
/// are available only when this is `Ready`; a source scan is never a fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyGate {
    Ready(NoClobber),
    Blocked(String),
}

impl ApplyGate {
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }
}

#[cfg(target_os = "linux")]
pub fn probe_apply_gate(root: &Path) -> ApplyGate {
    match studio_project::publish::Dir::open_root(root) {
        Err(e) => ApplyGate::Blocked(format!("{}: {e}", root.display())),
        Ok(dir) => match studio_project::publish::probe_no_clobber(&dir) {
            Ok(mechanism) => ApplyGate::Ready(mechanism),
            Err(reason) => ApplyGate::Blocked(reason),
        },
    }
}

#[cfg(not(target_os = "linux"))]
pub fn probe_apply_gate(_root: &Path) -> ApplyGate {
    ApplyGate::Blocked(
        "no-clobber source publication is only proven on Linux; this platform's Apply stays blocked until its primitives are qualified".into(),
    )
}

// ---- planning --------------------------------------------------------------------------

/// Everything a record needs besides the file set.
#[derive(Debug, Clone)]
pub struct RecordSeed {
    pub kind: TransactionKind,
    pub task: String,
    pub generation: u64,
    pub session: OpenSession,
    pub task_base: SourceRevision,
    pub prior_history: Option<String>,
    pub prior_checkpoint: SourceRevision,
    pub undoes: Option<String>,
    pub prompt_summary: String,
    pub build: Option<BuildIdentity>,
    pub validation_report_sha256: String,
    /// Directories to remove once empty after the commit.
    pub remove_dirs: Vec<String>,
    /// For Undo: the forward changes being reversed. They carry the exact permission
    /// bits the edit saw and left: the plan refuses when a touched file's bits are no
    /// longer the ones the edit left, and restores the original's exact bits.
    pub reverses: Vec<FileDelta>,
    /// Provenance of a Studio preset mutation.
    pub preset: Option<crate::preset_state::PresetProvenance>,
}

#[cfg(target_os = "linux")]
fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

#[cfg(target_os = "linux")]
fn conflict(path: &str, reason: impl Into<String>) -> PlanError {
    PlanError::Conflict {
        path: path.to_owned(),
        reason: reason.into(),
    }
}

#[cfg(target_os = "linux")]
fn exec_mode(base: u32) -> u32 {
    base | ((base & 0o444) >> 2)
}

#[cfg(target_os = "linux")]
fn plain_mode(base: u32) -> u32 {
    base & !0o111
}

/// The mode a published file must carry. An unchanged executable status keeps the
/// original's exact permission bits (a content-only replacement of a `0744` file stays
/// `0744`); a flip adds or removes the executable bits following the readable classes;
/// new files get the portable defaults.
#[cfg(target_os = "linux")]
fn after_mode(before_mode: Option<u32>, executable: bool) -> u32 {
    match before_mode {
        Some(mode) if (mode & 0o111 != 0) == executable => mode,
        Some(mode) if executable => exec_mode(mode),
        Some(mode) => plain_mode(mode),
        None if executable => 0o755,
        None => 0o644,
    }
}

/// Why the link-based pair is refused as a gate mechanism (see the module docs).
pub const LINK_REFUSED: &str = "linkat+unlinkat cannot move a live project file safely: an editor saving by rename between the two calls would lose its file; an atomic renameat2(RENAME_NOREPLACE) is required";

/// Builds the immutable intent: verifies every destination and parent without following
/// links, checks the before states, proves no-clobber publication in each destination
/// directory and reserves every recovery name. Nothing is changed (apart from the
/// probe files, which are removed again).
///
/// `forced` simulates a filesystem where only that mechanism exists: `Link` is refused
/// ([`LINK_REFUSED`]) because it can never move a live name.
#[cfg(target_os = "linux")]
pub fn plan(
    root: &Path,
    current: &SourceInventory,
    candidate: &SourceInventory,
    seed: RecordSeed,
    forced: Option<NoClobber>,
) -> Result<TransactionIntent, PlanError> {
    use studio_project::{
        paths::{TxRole, is_transaction_internal_name, transaction_file_name},
        publish::{Dir, EntryKind, probe_no_clobber},
    };
    if forced == Some(NoClobber::Link) {
        return Err(PlanError::GateBlocked(LINK_REFUSED.into()));
    }
    let mut deltas = deltas_between(current, candidate);
    if deltas.is_empty() {
        return Err(PlanError::NoChange);
    }
    if deltas.len() > MAX_FILE_OPS {
        return Err(PlanError::TooManyChanges(deltas.len()));
    }
    // Deletions first (lowest indexes) so a file can become a directory and a directory
    // a file; then creates; then replacements.
    deltas.sort_by(|a, b| {
        (
            a.kind() as u8 != OpKind::Delete as u8,
            a.kind() as u8,
            &a.path,
        )
            .cmp(&(
                b.kind() as u8 != OpKind::Delete as u8,
                b.kind() as u8,
                &b.path,
            ))
    });
    let deleted: BTreeSet<String> = deltas
        .iter()
        .filter(|d| d.kind() == OpKind::Delete)
        .map(|d| d.path.clone())
        .collect();
    // Directory -> file: a created path that holds files today must be emptied by this
    // very revision. `vacated_by[path]` lists the directories to remove, deepest first.
    let mut vacated_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for d in deltas.iter().filter(|d| d.kind() == OpKind::Create) {
        let prefix = format!("{}/", d.path);
        let under: Vec<&str> = current
            .files
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| p.starts_with(&prefix))
            .collect();
        if under.is_empty() {
            continue;
        }
        if let Some(left) = under.iter().copied().find(|p| !deleted.contains(*p)) {
            return Err(conflict(
                &d.path,
                format!("is a directory that still holds {left}"),
            ));
        }
        let mut set: BTreeSet<String> = BTreeSet::new();
        set.insert(d.path.clone());
        for file in &under {
            let mut parent = split_path(file).0;
            while parent.len() > d.path.len() {
                set.insert(parent.to_owned());
                parent = split_path(parent).0;
            }
        }
        let mut ordered: Vec<String> = set.into_iter().collect();
        ordered.sort_by(|a, b| {
            b.matches('/')
                .count()
                .cmp(&a.matches('/').count())
                .then_with(|| a.cmp(b))
        });
        vacated_by.insert(d.path.clone(), ordered);
    }
    let root_dir =
        Dir::open_root(root).map_err(|e| PlanError::Io(format!("{}: {e}", root.display())))?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let mut dirs: BTreeMap<String, FileId> = BTreeMap::new();
    dirs.insert(String::new(), root_dir.id());
    let mut probed: BTreeMap<String, NoClobber> = BTreeMap::new();
    let mut ops = Vec::with_capacity(deltas.len());
    for (index, mut delta) in deltas.into_iter().enumerate() {
        let (dir_rel, name) = split_path(&delta.path);
        if is_transaction_internal_name(name) {
            return Err(conflict(&delta.path, "reserved transaction file name"));
        }
        let kind = delta.kind();
        // Walk the parent without following links.
        let mut handle = root_dir
            .try_clone()
            .map_err(|e| PlanError::Io(e.to_string()))?;
        let mut so_far = String::new();
        let mut new_dirs = Vec::new();
        let mut last_existing = String::new();
        let mut blocked = false;
        if !dir_rel.is_empty() {
            for component in dir_rel.split('/') {
                if !so_far.is_empty() {
                    so_far.push('/');
                }
                so_far.push_str(component);
                if !new_dirs.is_empty() {
                    new_dirs.push(so_far.clone());
                    continue;
                }
                match handle
                    .stat(component)
                    .map_err(|e| conflict(&delta.path, e.to_string()))?
                {
                    None => new_dirs.push(so_far.clone()),
                    Some(stat) if stat.kind == EntryKind::Directory => {
                        handle = handle
                            .open_child(component)
                            .map_err(|e| conflict(&delta.path, format!("{so_far}: {e}")))?;
                        dirs.insert(so_far.clone(), handle.id());
                        last_existing.clone_from(&so_far);
                    }
                    Some(stat) if stat.is_regular() && deleted.contains(so_far.as_str()) => {
                        // File -> directory: the file goes first, its directory later.
                        blocked = true;
                        new_dirs.push(so_far.clone());
                    }
                    Some(_) => {
                        return Err(conflict(
                            &delta.path,
                            format!("parent {so_far} is a file, link or special file"),
                        ));
                    }
                }
            }
        }
        if !new_dirs.is_empty() && kind != OpKind::Create {
            return Err(conflict(&delta.path, "its directory does not exist"));
        }
        let mut vacates = Vec::new();
        let (before_id, before_mode) = match kind {
            OpKind::Create => {
                if new_dirs.is_empty() {
                    match handle
                        .stat(name)
                        .map_err(|e| conflict(&delta.path, e.to_string()))?
                    {
                        None => (),
                        Some(stat)
                            if stat.kind == EntryKind::Directory
                                && vacated_by.contains_key(&delta.path) =>
                        {
                            // Directory -> file: only a directory whose files this very
                            // revision deletes; an unrelated (even empty) directory is
                            // the user's and stays a conflict. The directories are
                            // recreated on rollback.
                            let tops = vacated_by[&delta.path].clone();
                            for path in tops {
                                let dir = root_dir
                                    .open_relative(&path)
                                    .map_err(|e| conflict(&delta.path, format!("{path}: {e}")))?;
                                let stat = dir
                                    .stat_self()
                                    .map_err(|e| conflict(&delta.path, format!("{path}: {e}")))?;
                                vacates.push(VacatedDir {
                                    path,
                                    id: dir.id(),
                                    mode: stat.mode,
                                });
                            }
                        }
                        Some(_) => {
                            return Err(conflict(&delta.path, "already exists in the project"));
                        }
                    }
                }
                (None, None)
            }
            OpKind::Replace | OpKind::Delete => {
                let before = delta.before.as_ref().expect("replace/delete has a before");
                let (stat, hash) = handle
                    .hash_regular(name)
                    .map_err(|e| conflict(&delta.path, format!("destination: {e}")))?;
                if hash != before.sha256
                    || stat.size != before.size
                    || stat.executable() != before.executable
                {
                    return Err(conflict(
                        &delta.path,
                        "changed outside the task since it was scanned",
                    ));
                }
                (Some(stat.id), Some(stat.mode))
            }
        };
        // The original of a delete beneath a directory this revision removes cannot stay
        // in that directory: its slot lives in the removed directory's parent.
        let slot_dir = (kind == OpKind::Delete)
            .then(|| {
                vacated_by
                    .keys()
                    .find(|top| delta.path.starts_with(&format!("{top}/")))
                    .map(|top| split_path(top).0.to_owned())
            })
            .flatten();
        // Prove atomic no-replace publication once per destination directory (the
        // nearest existing one for new directories).
        let key = if new_dirs.is_empty() {
            dir_rel.to_owned()
        } else {
            last_existing.clone()
        };
        let mechanism = match probed.get(&key) {
            Some(m) => *m,
            None => {
                let dir = root_dir
                    .open_relative(&key)
                    .map_err(|e| PlanError::Io(e.to_string()))?;
                let m = probe_no_clobber(&dir).map_err(PlanError::GateBlocked)?;
                probed.insert(key, m);
                m
            }
        };
        // An Undo restores the exact bits the edit saw, and only when the file still has
        // the bits the edit left (a permission change since is not ours to overwrite).
        let reverse = seed.reverses.iter().find(|r| r.path == delta.path);
        if let (Some(expected), Some(observed)) = (reverse.and_then(|r| r.after_mode), before_mode)
            && expected != observed
        {
            return Err(conflict(
                &delta.path,
                format!(
                    "its permission bits changed after the edit ({observed:04o}; the edit left {expected:04o})"
                ),
            ));
        }
        let published_mode = delta.after.as_ref().map(|after| {
            reverse
                .and_then(|r| r.before_mode)
                .filter(|mode| (mode & 0o111 != 0) == after.executable)
                .unwrap_or_else(|| after_mode(before_mode, after.executable))
        });
        delta.before_mode = before_mode;
        delta.after_mode = published_mode;
        ops.push(FileOp {
            index,
            kind,
            dir: dir_rel.to_owned(),
            name: name.to_owned(),
            before_id,
            new_dirs,
            defer_stage: blocked,
            vacates,
            slot_dir,
            mechanism,
            recovery: RecoveryPaths {
                stage: transaction_file_name(&id, index, TxRole::Stage),
                original: transaction_file_name(&id, index, TxRole::Original),
                rollback: transaction_file_name(&id, index, TxRole::Rollback),
            },
            delta,
        });
    }
    let changes: Vec<FileDelta> = ops.iter().map(|op| op.delta.clone()).collect();
    let created_dirs: Vec<String> = ops
        .iter()
        .flat_map(|op| op.new_dirs.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let record = TaskRevisionRecord {
        id: id.clone(),
        kind: seed.kind,
        task: seed.task,
        generation: seed.generation,
        session: seed.session,
        task_base: seed.task_base,
        prior_history: seed.prior_history,
        prior_checkpoint: seed.prior_checkpoint,
        candidate: candidate.revision.clone(),
        published: candidate.revision.clone(),
        undoes: seed.undoes,
        prompt_summary: seed.prompt_summary,
        changes,
        created_dirs,
        build: seed.build,
        validation_report_sha256: seed.validation_report_sha256,
        committed_unix: 0,
        preset: seed.preset,
    };
    Ok(TransactionIntent {
        format: TASK_FORMAT,
        id,
        kind: seed.kind,
        base: current.revision.clone(),
        expected: candidate.revision.clone(),
        ops,
        dirs: dirs
            .into_iter()
            .map(|(path, id)| DirIdentity { path, id })
            .collect(),
        remove_dirs: seed.remove_dirs,
        record,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn plan(
    _root: &Path,
    _current: &SourceInventory,
    _candidate: &SourceInventory,
    _seed: RecordSeed,
    _forced: Option<NoClobber>,
) -> Result<TransactionIntent, PlanError> {
    match probe_apply_gate(Path::new(".")) {
        ApplyGate::Blocked(reason) => Err(PlanError::GateBlocked(reason)),
        ApplyGate::Ready(_) => unreachable!("only Linux can publish"),
    }
}

// ---- execution (Linux) -----------------------------------------------------------------

/// Executes and recovers transactions against one project root.
pub struct Executor<'a> {
    #[cfg(target_os = "linux")]
    inner: linux::Machine<'a>,
    #[cfg(not(target_os = "linux"))]
    _marker: std::marker::PhantomData<&'a ()>,
}

#[cfg(target_os = "linux")]
impl<'a> Executor<'a> {
    pub fn new(
        root: &'a Path,
        checkpoints: &'a Checkpoints,
        journal: &'a mut TaskJournal,
        hooks: &'a dyn TransactionHooks,
    ) -> Result<Self, EngineError> {
        Ok(Self {
            inner: linux::Machine::new(root, checkpoints, journal, hooks)?,
        })
    }

    /// Runs the whole sequence for `intent`: journal the intent, stage, publish every
    /// operation, verify the final inventory equals the validated candidate and append
    /// the durable commit. Any conflict or failure before the commit rolls the
    /// completed operations back (or halts as a conflict where that is unsafe).
    pub fn execute(&mut self, intent: &TransactionIntent) -> Result<Outcome, Interrupt> {
        self.inner.execute(intent)
    }

    /// Recovers every unfinished transaction of a replayed journal.
    pub fn recover(
        &mut self,
        transactions: &[crate::journal::ReplayedTransaction],
    ) -> Result<RecoveryReport, Interrupt> {
        self.inner.recover(transactions)
    }
}

#[cfg(not(target_os = "linux"))]
impl<'a> Executor<'a> {
    pub fn new(
        _root: &'a Path,
        _checkpoints: &'a Checkpoints,
        _journal: &'a mut TaskJournal,
        _hooks: &'a dyn TransactionHooks,
    ) -> Result<Self, EngineError> {
        // Opening a project must keep working everywhere; only mutation is unsupported.
        Ok(Self {
            _marker: std::marker::PhantomData,
        })
    }
    pub fn execute(&mut self, _: &TransactionIntent) -> Result<Outcome, Interrupt> {
        Err(Interrupt::Journal("unsupported platform".into()))
    }
    pub fn recover(
        &mut self,
        _: &[crate::journal::ReplayedTransaction],
    ) -> Result<RecoveryReport, Interrupt> {
        Ok(RecoveryReport::default())
    }
}

#[cfg(target_os = "linux")]
mod linux;

#[cfg(test)]
mod tests {
    use super::*;
    use studio_project::{ProjectPath, revision::FileKind};

    fn hash(c: char) -> String {
        c.to_string().repeat(64)
    }

    fn file(path: &str, content: char, size: u64, executable: bool) -> SourceFile {
        SourceFile {
            path: ProjectPath::try_from(path.to_owned()).unwrap(),
            kind: FileKind::Other,
            size,
            sha256: hash(content),
            executable,
        }
    }

    fn inventory(files: Vec<SourceFile>) -> SourceInventory {
        let mut files = files;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        SourceInventory {
            version: 1,
            revision: SourceRevision::try_from(hash('0')).unwrap(),
            files,
        }
    }

    #[test]
    fn forward_delta_names_creates_replacements_deletes_and_mode_changes() {
        let from = inventory(vec![
            file("a.txt", 'a', 1, false),
            file("b.txt", 'b', 1, false),
            file("run.sh", 'c', 1, false),
            file("same.txt", 'd', 1, false),
        ]);
        let to = inventory(vec![
            file("a.txt", 'e', 2, false),
            file("n/new.txt", 'f', 1, false),
            file("run.sh", 'c', 1, true),
            file("same.txt", 'd', 1, false),
        ]);
        let deltas = deltas_between(&from, &to);
        let summary: Vec<_> = deltas.iter().map(|d| (d.path.as_str(), d.kind())).collect();
        assert_eq!(
            summary,
            vec![
                ("a.txt", OpKind::Replace),
                ("b.txt", OpKind::Delete),
                ("n/new.txt", OpKind::Create),
                ("run.sh", OpKind::Replace),
            ]
        );
        let mode = deltas.iter().find(|d| d.path == "run.sh").unwrap();
        assert_eq!(
            mode.before.as_ref().unwrap().sha256,
            mode.after.as_ref().unwrap().sha256
        );
        assert!(!mode.before.as_ref().unwrap().executable);
        assert!(mode.after.as_ref().unwrap().executable);
    }

    #[test]
    fn inverse_delta_swaps_states_and_round_trips() {
        let from = inventory(vec![
            file("a.txt", 'a', 1, false),
            file("b.txt", 'b', 1, true),
        ]);
        let to = inventory(vec![
            file("a.txt", 'c', 3, false),
            file("c.txt", 'd', 1, false),
        ]);
        let forward = deltas_between(&from, &to);
        let inverse = invert(&forward);
        assert_eq!(inverse, deltas_between(&to, &from));
        assert_eq!(invert(&inverse), forward);
        let kinds: Vec<_> = inverse
            .iter()
            .map(|d| (d.path.as_str(), d.kind()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("a.txt", OpKind::Replace),
                ("b.txt", OpKind::Create),
                ("c.txt", OpKind::Delete)
            ]
        );
    }

    #[test]
    fn undo_conflicts_only_on_touched_paths_and_names_the_reason() {
        let before = inventory(vec![
            file("a.txt", 'a', 1, false),
            file("b.txt", 'b', 1, false),
        ]);
        let after = inventory(vec![
            file("a.txt", 'c', 1, false),
            file("b.txt", 'b', 1, false),
            file("new.txt", 'd', 1, false),
        ]);
        let edit = deltas_between(&before, &after);
        // Unrelated edit to b.txt and an extra file: preserved, no conflict.
        let unrelated = inventory(vec![
            file("a.txt", 'c', 1, false),
            file("b.txt", 'z', 9, false),
            file("extra.txt", 'e', 1, false),
            file("new.txt", 'd', 1, false),
        ]);
        assert!(touched_conflicts(&edit, &unrelated).is_empty());
        // a.txt changed after the edit, new.txt deleted, mode flipped.
        let touched = inventory(vec![file("a.txt", 'q', 1, false)]);
        let conflicts = touched_conflicts(&edit, &touched);
        let reasons: Vec<_> = conflicts
            .iter()
            .map(|c| (c.path.as_str(), c.reason.as_str()))
            .collect();
        assert_eq!(
            reasons,
            vec![
                ("a.txt", "was modified after the edit"),
                ("new.txt", "was deleted after the edit"),
            ]
        );
        let mode = inventory(vec![
            file("a.txt", 'c', 1, true),
            file("new.txt", 'd', 1, false),
        ]);
        assert_eq!(
            touched_conflicts(&edit, &mode)[0].reason,
            "had its executable bit changed"
        );
    }

    #[test]
    fn published_modes_keep_permissions_and_follow_the_executable_state() {
        assert_eq!(after_mode(Some(0o600), true), 0o700);
        assert_eq!(after_mode(Some(0o644), true), 0o755);
        assert_eq!(after_mode(Some(0o755), false), 0o644);
        assert_eq!(after_mode(Some(0o640), false), 0o640);
        assert_eq!(after_mode(None, true), 0o755);
        assert_eq!(after_mode(None, false), 0o644);
    }

    #[test]
    fn an_unchanged_executable_status_keeps_the_full_mode() {
        // Executable already (any x bit counts) and staying executable: untouched.
        assert_eq!(after_mode(Some(0o744), true), 0o744);
        assert_eq!(after_mode(Some(0o641), true), 0o641);
        assert_eq!(after_mode(Some(0o100), true), 0o100);
        assert_eq!(after_mode(Some(0o111), true), 0o111);
        // Not executable and staying so: untouched (including odd read bits).
        assert_eq!(after_mode(Some(0o640), false), 0o640);
        assert_eq!(after_mode(Some(0o604), false), 0o604);
        assert_eq!(after_mode(Some(0o000), false), 0o000);
        // A flip only adds / removes the executable bits.
        assert_eq!(after_mode(Some(0o741), false), 0o640);
        assert_eq!(after_mode(Some(0o111), false), 0o000);
        assert_eq!(after_mode(Some(0o640), true), 0o750);
    }

    #[test]
    fn records_without_permission_bits_read_with_defaults_and_stay_invertible() {
        let legacy =
            r#"{"path":"a.txt","before":{"sha256":"aa","size":1,"executable":false},"after":null}"#;
        let delta: FileDelta = serde_json::from_str(legacy).unwrap();
        assert_eq!((delta.before_mode, delta.after_mode), (None, None));
        assert_eq!(delta.inverse().inverse(), delta);
        // New records name both sides and swap them in the inverse.
        let exact = FileDelta {
            before_mode: Some(0o741),
            after_mode: Some(0o640),
            ..delta
        };
        let inverse = exact.inverse();
        assert_eq!(
            (inverse.before_mode, inverse.after_mode),
            (Some(0o640), Some(0o741))
        );
        let json = serde_json::to_string(&exact).unwrap();
        assert_eq!(serde_json::from_str::<FileDelta>(&json).unwrap(), exact);
        // Absent bits are not written at all.
        assert!(
            !serde_json::to_string(&delta_without_modes())
                .unwrap()
                .contains("_mode")
        );
    }

    fn delta_without_modes() -> FileDelta {
        serde_json::from_str(r#"{"path":"a.txt","before":null,"after":null}"#).unwrap()
    }

    #[test]
    fn summaries_are_one_bounded_line() {
        assert_eq!(prompt_summary("  make\n\nit   red "), "make it red");
        let long = "word ".repeat(400);
        let summary = prompt_summary(&long);
        assert_eq!(summary.chars().count(), MAX_SUMMARY_CHARS + 1);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn event_labels_are_stable() {
        assert_eq!(
            TaskEvent::Resolved {
                note: String::new()
            }
            .label(),
            "resolved"
        );
        assert_eq!(
            TaskEvent::RolledBack {
                reason: String::new()
            }
            .label(),
            "rolled_back"
        );
    }
}
