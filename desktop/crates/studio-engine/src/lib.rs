//! Per-open-project state, isolated SDK builds, durable checkpoints and recovery.
pub mod app_paths;
pub mod build_materialization;
pub mod controller;
pub mod journal;
pub mod state;
pub mod store;
pub use controller::Controller;
pub use state::*;

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
