use crate::{EngineError, ProjectState, edit_transaction::TaskRevisionRecord};
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

/// Schema version of the history database. Version 2 adds the task-revision projection.
pub const SCHEMA_VERSION: u32 = 2;

/// Owned by the background controller/serialized command queue, never the GPUI render thread.
pub struct Store {
    connection: Connection,
    read_only: bool,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self, EngineError> {
        let existed = path.exists();
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: u32 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(EngineError::NewerFormat {
                what: format!("{} (history database)", path.display()),
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        if version < SCHEMA_VERSION {
            if existed {
                backup_database(&connection, path)?;
            }
            // Additive only: the original table and any unknown tables are untouched.
            let tx = connection.transaction()?;
            tx.execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, location TEXT NOT NULL, record TEXT NOT NULL, opened INTEGER NOT NULL, recent INTEGER NOT NULL DEFAULT 1);
                 CREATE TABLE IF NOT EXISTS task_revisions (project TEXT NOT NULL, tx TEXT NOT NULL, position INTEGER NOT NULL, record TEXT NOT NULL, PRIMARY KEY (project, tx));
                 PRAGMA user_version={SCHEMA_VERSION};"
            ))?;
            tx.commit()?;
        }
        // Self-heal an additive projection table that was dropped or never created.
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_revisions (project TEXT NOT NULL, tx TEXT NOT NULL, position INTEGER NOT NULL, record TEXT NOT NULL, PRIMARY KEY (project, tx));",
        )?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self {
            connection,
            read_only: false,
        })
    }
    /// Opens any existing database for inspection only: no migration, no backup, no
    /// write. Works for databases written by a newer Studio.
    pub fn open_read_only(path: &Path) -> Result<Self, EngineError> {
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(5))?;
        Ok(Self {
            connection,
            read_only: true,
        })
    }
    /// `PRAGMA user_version` of the open database.
    pub fn version(&self) -> Result<u32, EngineError> {
        Ok(self
            .connection
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }
    fn writable(&self) -> Result<(), EngineError> {
        if self.read_only {
            return Err(EngineError::Diagnostic(
                "History database is open read-only for diagnostics; nothing was changed".into(),
            ));
        }
        Ok(())
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
        self.writable()?;
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
        self.writable()?;
        self.connection.execute(
            "UPDATE projects SET recent=0 WHERE id=?",
            [String::from(id.clone())],
        )?;
        Ok(())
    }
    /// Committed task revisions of `project` in commit order. The projection of the
    /// task journal, which stays the source of truth.
    pub fn task_revisions(
        &self,
        project: &studio_project::ProjectId,
    ) -> Result<Vec<TaskRevisionRecord>, EngineError> {
        let mut statement = self
            .connection
            .prepare("SELECT record FROM task_revisions WHERE project=? ORDER BY position")?;
        let rows =
            statement.query_map([String::from(project.clone())], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    /// Replaces the whole projection of `project` (rebuild from the journal).
    pub fn replace_task_revisions(
        &mut self,
        project: &studio_project::ProjectId,
        records: &[TaskRevisionRecord],
    ) -> Result<(), EngineError> {
        self.writable()?;
        let tx = self.connection.transaction()?;
        let id = String::from(project.clone());
        tx.execute("DELETE FROM task_revisions WHERE project=?", [&id])?;
        for (position, record) in records.iter().enumerate() {
            tx.execute(
                "INSERT INTO task_revisions(project,tx,position,record) VALUES(?1,?2,?3,?4)",
                params![
                    id,
                    record.id,
                    position as i64,
                    serde_json::to_string(record)?
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// Consistent copy of the whole database, including any WAL state, made with SQLite's
/// online backup API and synced before the schema is touched.
fn backup_database(connection: &Connection, path: &Path) -> Result<PathBuf, EngineError> {
    let backup = path.with_extension(format!("backup-{}", uuid::Uuid::new_v4()));
    {
        let mut target = Connection::open(&backup)?;
        target.pragma_update(None, "synchronous", "FULL")?;
        let copy = rusqlite::backup::Backup::new(connection, &mut target)?;
        copy.run_to_completion(256, Duration::from_millis(5), None)?;
    }
    std::fs::File::open(&backup)?.sync_all()?;
    studio_project::lifecycle::sync_directory(path.parent().unwrap())?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy_database(path: &Path) -> Connection {
        let original = Connection::open(path).unwrap();
        original
            .execute_batch(
                "CREATE TABLE retained (value TEXT); INSERT INTO retained VALUES ('old data');",
            )
            .unwrap();
        original
    }

    fn only_backup(dir: &Path, database: &Path) -> PathBuf {
        let mut backups: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p != database && p.to_string_lossy().contains("backup-"))
            .collect();
        assert_eq!(backups.len(), 1, "{backups:?}");
        backups.remove(0)
    }

    #[test]
    fn migration_preserves_a_durable_backup_and_unknown_tables() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("studio.sqlite3");
        drop(legacy_database(&path));
        let store = Store::open(&path).unwrap();
        assert_eq!(store.version().unwrap(), SCHEMA_VERSION);
        assert_eq!(
            store
                .connection
                .query_row("SELECT value FROM retained", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "old data"
        );
        let backup = Connection::open(only_backup(temp.path(), &path)).unwrap();
        assert_eq!(
            backup
                .query_row("SELECT value FROM retained", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "old data"
        );
        // The backup is a pre-migration copy: it has no projection table.
        assert!(
            backup
                .query_row("SELECT count(*) FROM task_revisions", [], |r| r
                    .get::<_, i64>(0))
                .is_err()
        );
        drop(store);
        assert!(Store::open(&path).unwrap().recents().unwrap().is_empty());
        // Reopening a current database neither migrates nor backs up again.
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn the_backup_includes_committed_wal_state() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("studio.sqlite3");
        let original = Connection::open(&path).unwrap();
        original
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))
            .unwrap();
        original
            .execute_batch(
                "CREATE TABLE retained (value TEXT); INSERT INTO retained VALUES ('in the wal');",
            )
            .unwrap();
        // Rows exist only in the -wal file while this connection stays open.
        assert!(temp.path().join("studio.sqlite3-wal").exists());
        let store = Store::open(&path).unwrap();
        let backup = Connection::open(only_backup(temp.path(), &path)).unwrap();
        assert_eq!(
            backup
                .query_row("SELECT value FROM retained", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "in the wal"
        );
        drop((store, original));
    }

    #[test]
    fn version_one_databases_gain_the_projection_and_keep_their_records() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("studio.sqlite3");
        let v1 = Connection::open(&path).unwrap();
        v1.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY, location TEXT NOT NULL, record TEXT NOT NULL, opened INTEGER NOT NULL, recent INTEGER NOT NULL DEFAULT 1); INSERT INTO projects VALUES ('p','/x','{}',1,1); PRAGMA user_version=1;").unwrap();
        drop(v1);
        let store = Store::open(&path).unwrap();
        assert_eq!(store.version().unwrap(), 2);
        assert_eq!(
            store
                .connection
                .query_row("SELECT record FROM projects WHERE id='p'", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "{}"
        );
        only_backup(temp.path(), &path);
    }

    #[test]
    fn a_newer_database_is_refused_for_writing_but_readable_for_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("studio.sqlite3");
        let newer = legacy_database(&path);
        newer.pragma_update(None, "user_version", 99).unwrap();
        drop(newer);
        let before = std::fs::read(&path).unwrap();
        let error = Store::open(&path).err().unwrap();
        assert!(
            matches!(
                error,
                EngineError::NewerFormat {
                    found: 99,
                    supported: SCHEMA_VERSION,
                    ..
                }
            ),
            "{error}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let mut diagnostic = Store::open_read_only(&path).unwrap();
        assert_eq!(diagnostic.version().unwrap(), 99);
        let id = studio_project::ProjectId::try_from("project-1".to_owned()).unwrap();
        assert!(diagnostic.replace_task_revisions(&id, &[]).is_err());
        assert!(diagnostic.remove_recent(&id).is_err());
        drop(diagnostic);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }
}
