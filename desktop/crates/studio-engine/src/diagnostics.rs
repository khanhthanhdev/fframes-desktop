//! Read-only inspection of a project's local history.
//!
//! Used when [`crate::Controller::open`] refuses to open because the history was written
//! by a newer Studio (or is damaged): nothing here creates, migrates, truncates,
//! quarantines or locks anything, so source and history stay exactly as they were.
use crate::{
    EngineError,
    app_paths::AppPaths,
    journal::TaskJournal,
    store::{Record, SCHEMA_VERSION, Store},
};
use std::path::Path;

#[derive(Debug, Clone)]
pub enum TaskJournalDiagnostics {
    Absent,
    Readable {
        transactions: usize,
        committed: usize,
        /// Transactions without a terminal event; recovery rolls these back on open.
        pending: Vec<String>,
        /// Conflicted transactions awaiting resolution.
        unresolved: Vec<String>,
        /// Bytes of an incomplete last line that open would quarantine.
        ignored_tail_bytes: usize,
    },
    /// Written by a newer Studio (`found` is its format number).
    Newer {
        found: u32,
    },
    Unreadable(String),
}

#[derive(Debug, Clone)]
pub struct HistoryDiagnostics {
    pub project: String,
    pub database_version: Option<u32>,
    pub supported_database_version: u32,
    pub database_newer: bool,
    pub project_record: Result<Option<Record>, String>,
    pub task_journal: TaskJournalDiagnostics,
}

/// Inspects the history of the project at `root` without changing anything.
pub fn diagnose_history(root: &Path, paths: &AppPaths) -> Result<HistoryDiagnostics, EngineError> {
    let project = studio_project::open_for_recovery(root)?;
    let id = project.manifest.project_id.clone();
    let database = paths.database();
    let (database_version, project_record) = if database.exists() {
        let store = Store::open_read_only(&database)?;
        let record = store.get(&id).map_err(|e| e.to_string());
        (Some(store.version()?), record)
    } else {
        (None, Ok(None))
    };
    let journal = paths.project(&id).join("tasks.jsonl");
    let task_journal = match TaskJournal::inspect(&journal) {
        Ok(replay) if replay.transactions.is_empty() && !journal.exists() => {
            TaskJournalDiagnostics::Absent
        }
        Ok(replay) => TaskJournalDiagnostics::Readable {
            committed: replay.committed().len(),
            pending: replay.pending().iter().map(|t| t.id.clone()).collect(),
            unresolved: replay
                .unresolved_conflicts()
                .into_iter()
                .map(|c| c.transaction)
                .collect(),
            ignored_tail_bytes: replay.ignored_tail_bytes,
            transactions: replay.transactions.len(),
        },
        Err(EngineError::NewerFormat { found, .. }) => TaskJournalDiagnostics::Newer { found },
        Err(other) => TaskJournalDiagnostics::Unreadable(other.to_string()),
    };
    Ok(HistoryDiagnostics {
        project: String::from(id),
        database_version,
        supported_database_version: SCHEMA_VERSION,
        database_newer: database_version.is_some_and(|v| v > SCHEMA_VERSION),
        project_record,
        task_journal,
    })
}
