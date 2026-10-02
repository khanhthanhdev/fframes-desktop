use crate::{EngineError, store::Record};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
};
use studio_project::lifecycle::sync_directory;

const MAX_ENTRY_BYTES: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    version: u32,
    sequence: u64,
    transaction: String,
    event: Event,
}
#[derive(Serialize, Deserialize)]
enum Event {
    Intent(Box<Record>),
    Commit,
}

pub struct Journal {
    path: PathBuf,
    file: File,
    sequence: u64,
    failed: bool,
}
pub struct Replay {
    pub committed: Option<Record>,
    pub pending: Option<Record>,
    pub quarantined_tail: Option<PathBuf>,
}
impl Journal {
    pub fn open(path: PathBuf) -> Result<(Self, Replay), EngineError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;
        let mut reader = BufReader::new(file.try_clone()?);
        let mut sequence = 0;
        let mut offset = 0;
        let mut pending: Option<(String, Record)> = None;
        let mut committed = None;
        let mut quarantined_tail = None;
        loop {
            let mut line = Vec::new();
            let count = reader
                .by_ref()
                .take(MAX_ENTRY_BYTES as u64 + 1)
                .read_until(b'\n', &mut line)?;
            if count == 0 {
                break;
            }
            if count > MAX_ENTRY_BYTES {
                return Err(EngineError::Diagnostic(format!(
                    "{}: journal entry exceeds 1 MiB at byte {offset}; preserve and repair history",
                    path.display()
                )));
            }
            if line.last() != Some(&b'\n') {
                let quarantine = path.with_extension(format!("tail-{}", uuid::Uuid::new_v4()));
                studio_project::lifecycle::atomic_write(&quarantine, &line)?;
                file.set_len(offset)?;
                file.sync_all()?;
                quarantined_tail = Some(quarantine);
                break;
            }
            let entry: Entry = serde_json::from_slice(&line).map_err(|e| EngineError::Diagnostic(format!("{}: malformed interior journal at byte {offset}: {e}; preserve and repair the journal", path.display())))?;
            if entry.version != 1 || entry.sequence != sequence + 1 {
                return Err(EngineError::Diagnostic(format!(
                    "{}: unsupported journal version or non-monotonic sequence; preserve and repair history",
                    path.display()
                )));
            }
            sequence = entry.sequence;
            offset += count as u64;
            match entry.event {
                Event::Intent(record) => pending = Some((entry.transaction, *record)),
                Event::Commit => {
                    let (transaction, record) = pending.take().ok_or_else(|| {
                        EngineError::Diagnostic(
                            "Journal commit without intent; repair history".into(),
                        )
                    })?;
                    if transaction != entry.transaction {
                        return Err(EngineError::Diagnostic(
                            "Journal transaction mismatch; repair history".into(),
                        ));
                    }
                    committed = Some(record);
                }
            }
        }
        sync_directory(path.parent().unwrap())?;
        Ok((
            Self {
                path,
                file,
                sequence,
                failed: false,
            },
            Replay {
                committed,
                pending: pending.map(|(_, r)| r),
                quarantined_tail,
            },
        ))
    }
    pub fn intent(&mut self, record: &Record) -> Result<String, EngineError> {
        let transaction = uuid::Uuid::new_v4().to_string();
        self.append(&transaction, Event::Intent(Box::new(record.clone())))?;
        Ok(transaction)
    }
    pub fn commit(&mut self, transaction: &str) -> Result<(), EngineError> {
        self.append(transaction, Event::Commit)
    }
    fn append(&mut self, transaction: &str, event: Event) -> Result<(), EngineError> {
        if self.failed {
            return Err(EngineError::Diagnostic("Journal write failed; repair disk/permissions, then close and reopen to recover before retrying".into()));
        }
        let next = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| EngineError::Diagnostic("Journal sequence exhausted".into()))?;
        let mut bytes = serde_json::to_vec(&Entry {
            version: 1,
            sequence: next,
            transaction: transaction.into(),
            event,
        })?;
        bytes.push(b'\n');
        if bytes.len() > MAX_ENTRY_BYTES {
            return Err(EngineError::Diagnostic(
                "Journal entry exceeds 1 MiB; shorten project metadata".into(),
            ));
        }
        let mut write = || -> Result<(), EngineError> {
            self.file.write_all(&bytes)?;
            self.file.sync_all()?;
            sync_directory(self.path.parent().unwrap())?;
            Ok(())
        };
        if let Err(error) = write() {
            // Continuing after a partial write would turn a recoverable tail into
            // interior corruption, or reuse a sequence whose sync result was unknown.
            self.failed = true;
            return Err(error);
        }
        self.sequence = next;
        Ok(())
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn truncated_tail_is_quarantined_but_interior_corruption_stops_replay() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("journal");
        fs::write(&path, b"{truncated").unwrap();
        let (_, replay) = Journal::open(path.clone()).unwrap();
        assert!(replay.quarantined_tail.unwrap().is_file());
        assert_eq!(fs::read(&path).unwrap(), b"");
        fs::write(&path, b"{bad}\n{tail").unwrap();
        assert!(Journal::open(path.clone()).is_err());
        assert_eq!(fs::read(path).unwrap(), b"{bad}\n{tail");
    }

    #[test]
    fn oversized_entry_is_refused_without_truncating_history() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("journal");
        let bytes = vec![b'x'; MAX_ENTRY_BYTES + 1];
        fs::write(&path, &bytes).unwrap();
        assert!(
            Journal::open(path.clone())
                .err()
                .unwrap()
                .to_string()
                .contains("exceeds")
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn disk_full_writer_refuses_further_appends_until_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut journal, _) = Journal::open(tmp.path().join("journal")).unwrap();
        journal.file = OpenOptions::new().write(true).open("/dev/full").unwrap();
        assert!(journal.commit("test").is_err());
        assert!(
            journal
                .commit("retry")
                .unwrap_err()
                .to_string()
                .contains("close and reopen")
        );
        assert_eq!(journal.sequence, 0);
        assert_eq!(fs::read(journal.path()).unwrap(), b"");
    }
}
