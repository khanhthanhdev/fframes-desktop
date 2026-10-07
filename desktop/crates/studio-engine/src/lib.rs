//! Per-open-project state, isolated SDK builds, durable checkpoints and recovery.
pub mod agent_task;
pub mod app_paths;
pub mod build_materialization;
pub mod candidate_validation;
pub mod canvas_selection;
pub mod controller;
pub mod diagnostics;
pub mod edit_transaction;
pub mod journal;
pub mod playback_clock;
pub mod preset_state;
pub mod preview_state;
pub mod state;
pub mod store;
pub mod task_scope;
pub mod timeline;
pub use agent_task::{
    AgentTask, AgentTaskContext, AgentTaskId, AgentTaskManager, CandidateRevision, CaptureTicket,
    DraftSnapshot, DraftState, DraftStore, FinishedTask, MAX_AUTOMATIC_REPAIRS, PreparedDraft,
    QuiescenceBlock, QuiescenceEvidence, RepairBudget, RepairDecision, TaskError, TaskIdentity,
    TaskSourceBase, TaskState, TurnCompletion, WriterGeneration, WriterGoneEvidence,
    WriterObservation, evaluate_quiescence,
};
pub use canvas_selection::*;
pub use controller::{
    CompletionOutcome, Controller, DraftPreparationChoice, PresetMutation, PresetOutcome,
    Promotion, RecoveryStatus, UndoPreparation,
};
pub use edit_transaction::{
    ApplyGate, Boundary, ConflictReport, Fault, FileDelta, FileState, NoHooks, PlanError,
    PromotionError, TaskRevisionRecord, TransactionHooks, TransactionKind,
};
pub use playback_clock::*;
pub use preset_state::{
    PresetAction, PresetPlan, PresetProvenance, PresetRequest, PresetStateError, ProjectStyle,
};
pub use preview_state::*;
pub use state::*;
pub use task_scope::{
    CanvasTaskSelection, CanvasTaskSelectionKind, CompiledScope, SceneSourceCandidate,
    SceneSourceReference, SceneSourceResolution, ScopeSelection, ScopedScene,
    SourceMatchConfidence, StyleSnapshotIdentity, TaskScope, TaskScopeError, VideoPixelRect,
};
pub use timeline::*;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    Project(#[from] studio_project::ProjectError),
    #[error("Storage error: {0}; check disk space and permissions, preserve history and retry")]
    Io(#[from] std::io::Error),
    #[error(
        "History database: {0}; preserve studio.sqlite3 and journals, repair/rebuild the index"
    )]
    Sql(#[from] rusqlite::Error),
    #[error("History format: {0}; preserve history and repair from backup")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    State(#[from] StateError),
    #[error("{0}")]
    Task(#[from] TaskError),
    #[error("{0}")]
    Candidate(#[from] crate::candidate_validation::CandidateError),
    #[error("{0}")]
    TaskScope(#[from] TaskScopeError),
    #[error("{0}")]
    Promotion(#[from] PromotionError),
    #[error("{0}")]
    Preset(#[from] PresetStateError),
    #[error(
        "{what} was written by a newer Studio (format {found}, this version supports {supported}); update Studio. Source and history were not changed"
    )]
    NewerFormat {
        what: String,
        found: u32,
        supported: u32,
    },
    #[error("{0}")]
    Diagnostic(String),
}
