//! Per-open-project state, isolated SDK builds, durable checkpoints and recovery.
pub mod app_paths;
pub mod build_materialization;
pub mod controller;
pub mod journal;
pub mod playback_clock;
pub mod preview_state;
pub mod state;
pub mod store;
pub mod timeline;
pub use controller::Controller;
pub use playback_clock::*;
pub use preview_state::*;
pub use state::*;
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
    Diagnostic(String),
}
