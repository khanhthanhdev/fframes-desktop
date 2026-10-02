use crate::{EngineError, ProjectState};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub location: PathBuf,
    pub name: String,
    pub state: ProjectState,
    pub draft: Option<PathBuf>,
    pub sdk_path: Option<PathBuf>,
}

/// Owned by the background controller/serialized command queue, never the GPUI render thread.
pub struct Store {
    connection: Connection,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self, EngineError> {
        let existed = path.exists();
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > 1 {
            return Err(EngineError::Diagnostic("History database is newer than Studio; update Studio. Source and history were not changed".into()));
        }
        if version == 0 {
            if existed {
                let backup = path.with_extension(format!("backup-{}", uuid::Uuid::new_v4()));
                std::fs::copy(path, &backup)?;
                std::fs::File::open(&backup)?.sync_all()?;
                studio_project::lifecycle::sync_directory(path.parent().unwrap())?;
            }
            let tx = connection.transaction()?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, location TEXT NOT NULL, record TEXT NOT NULL, opened INTEGER NOT NULL, recent INTEGER NOT NULL DEFAULT 1); PRAGMA user_version=1;")?;
            tx.commit()?;
        }
        connection.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self { connection })
    }
    pub fn get(&self, id: &studio_project::ProjectId) -> Result<Option<Record>, EngineError> {
        let json: Option<String> = self
            .connection
            .query_row(
                "SELECT record FROM projects WHERE id=?",
                [String::from(id.clone())],
                |r| r.get(0),
            )
            .optional()?;
        json.map(|j| serde_json::from_str(&j).map_err(EngineError::from))
            .transpose()
    }
    pub fn save(&mut self, record: &Record) -> Result<(), EngineError> {
        let tx = self.connection.transaction()?;
        tx.execute("INSERT INTO projects(id,location,record,opened,recent) VALUES(?1,?2,?3,unixepoch(),1) ON CONFLICT(id) DO UPDATE SET location=excluded.location,record=excluded.record,opened=excluded.opened,recent=1", params![String::from(record.state.project().clone()), record.location.to_string_lossy(), serde_json::to_string(record)?])?;
        tx.commit()?;
        Ok(())
    }
    pub fn recents(&self) -> Result<Vec<Record>, EngineError> {
        let mut statement = self.connection.prepare(
            "SELECT record FROM projects WHERE recent=1 ORDER BY opened DESC, id LIMIT 50",
        )?;
        let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    /// Removes only the recent-list entry, never history, projects or assets.
    pub fn remove_recent(&mut self, id: &studio_project::ProjectId) -> Result<(), EngineError> {
        self.connection.execute(
            "UPDATE projects SET recent=0 WHERE id=?",
            [String::from(id.clone())],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_preserves_a_durable_backup_and_unknown_tables() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("studio.sqlite3");
        let original = Connection::open(&path).unwrap();
        original
            .execute_batch(
                "CREATE TABLE retained (value TEXT); INSERT INTO retained VALUES ('old data');",
            )
            .unwrap();
        drop(original);
        let bytes = std::fs::read(&path).unwrap();
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |r| r.get::<_, u32>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT value FROM retained", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "old data"
        );
        let backup = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p != &path)
            .unwrap();
        assert_eq!(std::fs::read(backup).unwrap(), bytes);
        drop(store);
        assert!(Store::open(&path).unwrap().recents().unwrap().is_empty());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
    }
}
