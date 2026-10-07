//! Bounded resident row store backed by an append-only JSONL conversation log.
//!
//! The file (`conversation.jsonl`) holds one `{"v":1,"row":<Row>}` line per persisted row
//! *version*; the latest line of an id wins on replay. Only the newest rows (bounded by
//! [`RowLimits`]) stay resident, older ones are paged back with [`RowStore::page_before`].
//!
//! Crash behaviour: a torn (unparsable) last line is ignored on open and truncated away by
//! the next flush; an unparsable interior line is skipped and never panics. The file is
//! compacted atomically (temp file + fsync + rename + directory fsync) once it outgrows
//! [`RowLimits::max_file_bytes`].
//!
//! `RowStore` is owned by a single actor thread: it is `Send` and has no interior locking.
use super::model::{
    MAX_HISTORY_PAGE, MAX_RESIDENT_ROW_BYTES, MAX_RESIDENT_ROWS, MAX_ROW_TEXT_BYTES, NoticeLevel,
    ROW_OVERHEAD_BYTES, Row, RowId, RowKind,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

const FORMAT_VERSION: u32 = 1;
const DEFAULT_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("Conversation log: {0}")]
    Io(String),
    #[error("Conversation log: could not encode a row: {0}")]
    Encode(String),
}

fn io_err(error: io::Error) -> LogError {
    LogError::Io(error.to_string())
}

#[derive(Debug, Clone, Copy)]
pub struct RowLimits {
    /// Most rows kept resident.
    pub max_rows: usize,
    /// Resident byte budget (sum of [`Row::estimated_bytes`]).
    pub max_bytes: usize,
    /// The file is compacted once a flush leaves it larger than this.
    pub max_file_bytes: u64,
}

impl Default for RowLimits {
    fn default() -> Self {
        Self {
            max_rows: MAX_RESIDENT_ROWS,
            max_bytes: MAX_RESIDENT_ROW_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

#[derive(Serialize)]
struct LineRef<'a> {
    v: u32,
    row: &'a Row,
}

#[derive(Deserialize)]
struct Line {
    v: u32,
    row: Row,
}

fn decode(bytes: &[u8]) -> Option<Row> {
    serde_json::from_slice::<Line>(bytes)
        .ok()
        .filter(|line| line.v == FORMAT_VERSION)
        .map(|line| line.row)
}

/// One physical line of the log.
struct RawLine<'a> {
    offset: u64,
    /// The line without its `\n`.
    bytes: &'a [u8],
    /// Bytes the line occupies in the file, including a `\n` when present.
    len: u64,
    terminated: bool,
    last: bool,
}

/// Streams the lines of `path`; `Ok(false)` when the file does not exist.
fn for_each_line(path: &Path, mut f: impl FnMut(RawLine<'_>)) -> io::Result<bool> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut reader = BufReader::new(file);
    let mut buf = Vec::new();
    let mut offset = 0u64;
    loop {
        buf.clear();
        let read = reader.read_until(b'\n', &mut buf)?;
        if read == 0 {
            return Ok(true);
        }
        let terminated = buf.last() == Some(&b'\n');
        let bytes = if terminated {
            &buf[..buf.len() - 1]
        } else {
            &buf[..]
        };
        let last = reader.fill_buf()?.is_empty();
        f(RawLine {
            offset,
            bytes,
            len: read as u64,
            terminated,
            last,
        });
        offset += read as u64;
    }
}

/// What the next flush must do to the file before appending: cut a torn tail and/or
/// terminate a final line that lacks its newline.
#[derive(Debug, Clone, Copy)]
struct Repair {
    len: u64,
    newline: bool,
}

pub struct RowStore {
    path: PathBuf,
    limits: RowLimits,
    rows: BTreeMap<RowId, Arc<Row>>,
    bytes: usize,
    /// Latest content of every id not yet appended to the file. Also holds replaced rows
    /// that are no longer resident.
    dirty: BTreeMap<RowId, Arc<Row>>,
    dirty_bytes: usize,
    /// Unsaved rows dropped from memory because persistence kept failing.
    lost: u64,
    next: u64,
    has_older: bool,
    file_len: u64,
    repair: Option<Repair>,
    line_buf: Vec<u8>,
}

impl RowStore {
    /// Creates the parent directory and replays the file. The resident window becomes the
    /// newest rows (latest version per id) within `limits`.
    pub fn open(path: PathBuf, limits: RowLimits) -> Result<Self, LogError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(io_err)?;
        }
        let mut store = Self {
            path: path.clone(),
            limits,
            rows: BTreeMap::new(),
            bytes: 0,
            dirty: BTreeMap::new(),
            dirty_bytes: 0,
            lost: 0,
            next: 1,
            has_older: false,
            file_len: 0,
            repair: None,
            line_buf: Vec::new(),
        };
        let mut max_id = 0u64;
        let mut repair = None;
        let existed = for_each_line(&path, |line| {
            let end = line.offset + line.bytes.len() as u64;
            if line.bytes.trim_ascii().is_empty() {
                if line.last && !line.terminated {
                    repair = Some(Repair {
                        len: end,
                        newline: true,
                    });
                }
                return;
            }
            match decode(line.bytes) {
                Some(row) => {
                    max_id = max_id.max(row.id.0);
                    store.place(Arc::new(row));
                    store.trim_to_limits();
                    if line.last && !line.terminated {
                        repair = Some(Repair {
                            len: end,
                            newline: true,
                        });
                    }
                }
                None if line.last => {
                    repair = Some(Repair {
                        len: line.offset,
                        newline: false,
                    });
                }
                None => {}
            }
        })
        .map_err(io_err)?;
        if existed {
            store.file_len = fs::metadata(&path).map_err(io_err)?.len();
        }
        store.next = max_id + 1;
        store.repair = repair;
        Ok(store)
    }

    /// Next row id; monotonic and never reused.
    pub fn next_id(&mut self) -> RowId {
        let id = RowId(self.next);
        self.next += 1;
        id
    }

    /// Monotonic sequence boundary for session restoration tracking.
    pub fn sequence_boundary(&self) -> usize {
        self.next as usize
    }

    /// Inserts a new id or replaces an existing id's content in place (the position is
    /// defined by the id, not by call order). Replacing an id that was already evicted
    /// from the resident window is accepted: the new version is persisted by the next
    /// flush but the row is not made resident again. The row is marked dirty.
    pub fn upsert(&mut self, row: Arc<Row>) {
        let row = self.clamp(row);
        let id = row.id;
        if id.0 >= self.next {
            self.next = id.0 + 1;
        }
        self.dirty_insert(Arc::clone(&row));
        self.place(row);
        self.enforce_limits();
    }

    pub fn get(&self, id: RowId) -> Option<&Arc<Row>> {
        self.rows.get(&id)
    }

    /// Resident rows ordered by id ascending (cheap `Arc` clones).
    pub fn resident(&self) -> Vec<Arc<Row>> {
        self.rows.values().cloned().collect()
    }

    pub fn resident_len(&self) -> usize {
        self.rows.len()
    }

    pub fn resident_bytes(&self) -> usize {
        self.bytes
    }

    /// Whether rows older than the first resident row exist (evicted, found beyond the
    /// window on replay, and not dropped by compaction).
    pub fn has_older(&self) -> bool {
        self.has_older
    }

    pub fn file_bytes(&self) -> u64 {
        self.file_len
    }

    /// Appends one line per dirty id (its latest content only), syncs the file, repairs a
    /// torn tail, and compacts when the file exceeds [`RowLimits::max_file_bytes`]. On an
    /// error the rows stay dirty and the next flush retries.
    pub fn flush(&mut self) -> Result<(), LogError> {
        if !self.dirty.is_empty() || self.repair.is_some() {
            self.append_dirty()?;
        }
        if self.file_len > self.limits.max_file_bytes {
            self.compact()?;
        }
        Ok(())
    }

    /// Rows with id < `before`: latest version per id, ascending, the newest `limit`
    /// (clamped to `1..=MAX_HISTORY_PAGE`). Only flushed rows are read from the file;
    /// resident (and not yet flushed) rows with an older id come from memory and win over
    /// the file.
    pub fn page_before(&self, before: RowId, limit: usize) -> Result<Vec<Arc<Row>>, LogError> {
        self.page_job(before, limit).run()
    }

    /// Captures what a page needs so the file read can run on another thread.
    pub fn page_job(&self, before: RowId, limit: usize) -> PageJob {
        let limit = limit.clamp(1, MAX_HISTORY_PAGE);
        let newer = self
            .dirty
            .range(..before)
            .rev()
            .take(limit)
            .chain(self.rows.range(..before).rev().take(limit))
            .map(|(id, row)| (*id, Arc::clone(row)))
            .collect();
        PageJob {
            path: self.path.clone(),
            before,
            limit,
            newer,
        }
    }

    /// The latest version of `id` wherever it lives: resident, unsaved, or in the file.
    /// `None` when the id was never stored (or is unreadable).
    pub fn latest(&self, id: RowId) -> Option<Arc<Row>> {
        if let Some(row) = self.rows.get(&id).or_else(|| self.dirty.get(&id)) {
            return Some(Arc::clone(row));
        }
        let mut found = None;
        let _ = for_each_line(&self.path, |line| {
            if let Some(row) = decode(line.bytes)
                && row.id == id
            {
                found = Some(Arc::new(row));
            }
        });
        found
    }

    /// Makes `row` resident unless its id was already evicted. Returns whether it is
    /// resident now.
    fn place(&mut self, row: Arc<Row>) -> bool {
        let id = row.id;
        if let Some(old) = self.rows.get(&id) {
            self.bytes -= old.estimated_bytes();
        } else if self.has_older
            && self
                .rows
                .first_key_value()
                .is_some_and(|(first, _)| id < *first)
        {
            return false;
        }
        self.bytes += row.estimated_bytes();
        self.rows.insert(id, row);
        true
    }

    fn over_limits(&self) -> bool {
        self.rows.len() > 1
            && (self.rows.len() > self.limits.max_rows || self.bytes > self.limits.max_bytes)
    }

    fn evict_oldest(&mut self) {
        if let Some((_, row)) = self.rows.pop_first() {
            self.bytes -= row.estimated_bytes();
            self.has_older = true;
        }
    }

    /// Eviction of rows that are already persisted (replay).
    fn trim_to_limits(&mut self) {
        while self.over_limits() {
            self.evict_oldest();
        }
    }

    /// Eviction after an upsert: a dirty row is flushed first. If persistence fails the
    /// bound still holds: the oldest unsaved row is dropped from memory and counted as
    /// lost ([`RowStore::take_lost`]) instead of letting the window grow without limit.
    fn enforce_limits(&mut self) {
        while self.over_limits() {
            let Some(oldest) = self.rows.first_key_value().map(|(id, _)| *id) else {
                break;
            };
            if self.dirty.contains_key(&oldest) && self.flush().is_err() {
                self.drop_unsaved(oldest);
            }
            self.evict_oldest();
        }
        // Replaced versions of rows that already left the window are unsaved too.
        while self.dirty.len() > self.limits.max_rows || self.dirty_bytes > self.limits.max_bytes {
            if self.flush().is_ok() {
                break;
            }
            let Some(&oldest) = self.dirty.keys().next() else {
                break;
            };
            self.drop_unsaved(oldest);
        }
    }

    fn dirty_insert(&mut self, row: Arc<Row>) {
        self.dirty_bytes += row.estimated_bytes();
        if let Some(old) = self.dirty.insert(row.id, row) {
            self.dirty_bytes -= old.estimated_bytes();
        }
    }

    fn drop_unsaved(&mut self, id: RowId) {
        if let Some(old) = self.dirty.remove(&id) {
            self.dirty_bytes -= old.estimated_bytes();
            self.lost += 1;
        }
    }

    /// Rows that could not be saved and were dropped from memory to keep the bound since
    /// the last call. Non-zero means the conversation is no longer being retained.
    pub fn take_lost(&mut self) -> u64 {
        std::mem::take(&mut self.lost)
    }

    /// The most a single resident row may weigh: larger rows are replaced by a notice.
    fn row_cap(&self) -> usize {
        (self.limits.max_bytes / 4).max(MAX_ROW_TEXT_BYTES + 8 * ROW_OVERHEAD_BYTES)
    }

    fn clamp(&self, row: Arc<Row>) -> Arc<Row> {
        let size = row.estimated_bytes();
        if size <= self.row_cap() {
            return row;
        }
        Arc::new(Row {
            id: row.id,
            task: row.task.clone(),
            at_unix: row.at_unix,
            kind: RowKind::Notice {
                level: NoticeLevel::Warning,
                text: format!(
                    "A conversation row of {size} bytes exceeded the per-row limit ({} bytes) and was not kept.",
                    self.row_cap()
                ),
            },
        })
    }

    fn append_dirty(&mut self) -> Result<(), LogError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(io_err)?;
        let pending = self.repair;
        let base_len = match pending {
            Some(repair) => {
                file.set_len(repair.len).map_err(io_err)?;
                repair.len
            }
            None => self.file_len,
        };
        let mut writer = BufWriter::new(file);
        let mut written = 0u64;
        let result = (|| -> Result<(), LogError> {
            if pending.is_some_and(|repair| repair.newline) {
                writer.write_all(b"\n").map_err(io_err)?;
                written += 1;
            }
            for row in self.dirty.values() {
                self.line_buf.clear();
                serde_json::to_writer(
                    &mut self.line_buf,
                    &LineRef {
                        v: FORMAT_VERSION,
                        row,
                    },
                )
                .map_err(|e| LogError::Encode(e.to_string()))?;
                self.line_buf.push(b'\n');
                writer.write_all(&self.line_buf).map_err(io_err)?;
                written += self.line_buf.len() as u64;
            }
            writer.flush().map_err(io_err)?;
            writer.get_ref().sync_data().map_err(io_err)
        })();
        match result {
            Ok(()) => {
                self.file_len = base_len + written;
                self.repair = None;
                self.dirty.clear();
                self.dirty_bytes = 0;
                Ok(())
            }
            Err(error) => {
                // Part of the batch may have reached the file: cut it on the next attempt.
                self.repair = Some(Repair {
                    len: base_len,
                    newline: pending.is_some_and(|repair| repair.newline),
                });
                Err(error)
            }
        }
    }

    /// Rewrites the file with only the latest version of the newest ids whose serialized
    /// size fits in a quarter of `max_file_bytes` (always at least the newest row).
    fn compact(&mut self) -> Result<(), LogError> {
        let mut index: BTreeMap<RowId, (u64, u64)> = BTreeMap::new();
        for_each_line(&self.path, |line| {
            if let Some(row) = decode(line.bytes) {
                index.insert(
                    row.id,
                    (line.offset, line.len + u64::from(!line.terminated)),
                );
            }
        })
        .map_err(io_err)?;

        let budget = self.limits.max_file_bytes / 4;
        let mut total = 0u64;
        let mut kept: BTreeSet<RowId> = BTreeSet::new();
        for (id, (_, size)) in index.iter().rev() {
            if !kept.is_empty() && total + size > budget {
                break;
            }
            total += size;
            kept.insert(*id);
        }

        let dir = match self.path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let mut tmp_name = self.path.file_name().unwrap_or_default().to_os_string();
        tmp_name.push(".compact");
        let tmp_path = dir.join(tmp_name);
        let written = match self.write_compacted(&index, &kept, &tmp_path) {
            Ok(written) => written,
            Err(error) => {
                let _ = fs::remove_file(&tmp_path);
                return Err(error);
            }
        };
        self.file_len = written;
        self.has_older = match self.rows.first_key_value() {
            Some((first, _)) => kept.first().is_some_and(|oldest| oldest < first),
            None => !kept.is_empty(),
        };
        // The rename already happened; make it durable.
        #[cfg(unix)]
        File::open(&dir)
            .and_then(|dir| dir.sync_all())
            .map_err(io_err)?;
        Ok(())
    }

    fn write_compacted(
        &self,
        index: &BTreeMap<RowId, (u64, u64)>,
        kept: &BTreeSet<RowId>,
        tmp_path: &Path,
    ) -> Result<u64, LogError> {
        let mut source = File::open(&self.path).map_err(io_err)?;
        let mut writer = BufWriter::new(File::create(tmp_path).map_err(io_err)?);
        let mut written = 0u64;
        let mut buf = Vec::new();
        for id in kept {
            let (offset, size) = index[id];
            buf.clear();
            source.seek(SeekFrom::Start(offset)).map_err(io_err)?;
            (&mut source)
                .take(size)
                .read_to_end(&mut buf)
                .map_err(io_err)?;
            if buf.last() != Some(&b'\n') {
                buf.push(b'\n');
            }
            writer.write_all(&buf).map_err(io_err)?;
            written += buf.len() as u64;
        }
        writer.flush().map_err(io_err)?;
        writer.get_ref().sync_all().map_err(io_err)?;
        drop(writer);
        fs::rename(tmp_path, &self.path).map_err(io_err)?;
        Ok(written)
    }
}

/// One history page whose file read can run off the actor thread: everything held in
/// memory is captured up front, only the log file is read in [`PageJob::run`].
pub struct PageJob {
    path: PathBuf,
    before: RowId,
    limit: usize,
    newer: Vec<(RowId, Arc<Row>)>,
}

impl PageJob {
    pub fn run(self) -> Result<Vec<Arc<Row>>, LogError> {
        let Self {
            path,
            before,
            limit,
            newer,
        } = self;
        let mut page: BTreeMap<RowId, Arc<Row>> = BTreeMap::new();
        for_each_line(&path, |line| {
            let Some(row) = decode(line.bytes) else {
                return;
            };
            if row.id >= before {
                return;
            }
            // A later version of an id already popped for being too old is popped again.
            page.insert(row.id, Arc::new(row));
            if page.len() > limit {
                page.pop_first();
            }
        })
        .map_err(io_err)?;
        for (id, row) in newer {
            page.insert(id, row);
        }
        while page.len() > limit {
            page.pop_first();
        }
        Ok(page.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_workflow::model::{NoticeLevel, RowKind};

    fn row(id: u64, text: &str) -> Arc<Row> {
        Arc::new(Row {
            id: RowId(id),
            task: None,
            at_unix: 1_700_000_000 + id,
            kind: RowKind::Notice {
                level: NoticeLevel::Info,
                text: text.to_owned(),
            },
        })
    }

    fn text(row: &Row) -> &str {
        match &row.kind {
            RowKind::Notice { text, .. } => text,
            other => panic!("unexpected row kind {other:?}"),
        }
    }

    fn ids(rows: &[Arc<Row>]) -> Vec<u64> {
        rows.iter().map(|r| r.id.0).collect()
    }

    fn limits(max_rows: usize) -> RowLimits {
        RowLimits {
            max_rows,
            max_bytes: usize::MAX,
            max_file_bytes: u64::MAX,
        }
    }

    fn lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn open(dir: &tempfile::TempDir, limits: RowLimits) -> RowStore {
        RowStore::open(dir.path().join("sub").join("conversation.jsonl"), limits).unwrap()
    }

    fn log_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("sub").join("conversation.jsonl")
    }

    #[test]
    fn ids_are_monotonic_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, limits(100));
        assert_eq!(store.next_id(), RowId(1));
        let id2 = store.next_id();
        let id3 = store.next_id();
        assert_eq!((id2, id3), (RowId(2), RowId(3)));
        store.upsert(row(1, "a"));
        store.upsert(row(2, "b"));
        store.upsert(row(3, "c"));
        store.flush().unwrap();
        drop(store);

        let mut store = open(&dir, limits(100));
        assert_eq!(store.next_id(), RowId(4));
        assert_eq!(store.next_id(), RowId(5));
    }

    #[test]
    fn empty_log_starts_at_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, limits(100));
        assert_eq!(store.next_id(), RowId(1));
        assert!(!store.has_older());
        assert_eq!(store.resident_len(), 0);
        store.flush().unwrap();
        assert_eq!(store.file_bytes(), 0);
    }

    #[test]
    fn upsert_replaces_in_place_and_reopen_shows_latest() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, limits(100));
        store.upsert(row(1, "one"));
        store.upsert(row(2, "two"));
        store.upsert(row(3, "three"));
        store.flush().unwrap();
        store.upsert(row(2, "two v2"));
        assert_eq!(ids(&store.resident()), vec![1, 2, 3]);
        assert_eq!(text(store.get(RowId(2)).unwrap()), "two v2");
        store.flush().unwrap();
        assert_eq!(lines(&log_path(&dir)).len(), 4);
        drop(store);

        let store = open(&dir, limits(100));
        let resident = store.resident();
        assert_eq!(ids(&resident), vec![1, 2, 3]);
        assert_eq!(text(&resident[1]), "two v2");
        assert!(!store.has_older());
    }

    #[test]
    fn resident_bytes_tracks_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, limits(100));
        store.upsert(row(1, "aaaa"));
        assert_eq!(store.resident_bytes(), row(1, "aaaa").estimated_bytes());
        store.upsert(row(1, "aaaaaaaa"));
        assert_eq!(store.resident_bytes(), row(1, "aaaaaaaa").estimated_bytes());
        assert_eq!(store.resident_len(), 1);
    }

    #[test]
    fn evicts_by_row_count_keeping_newest() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, limits(3));
        for id in 1..=6 {
            store.upsert(row(id, "x"));
        }
        assert_eq!(ids(&store.resident()), vec![4, 5, 6]);
        assert_eq!(store.resident_len(), 3);
        assert!(store.has_older());
        assert!(store.get(RowId(1)).is_none());
        // Evicted dirty rows were flushed on the way out.
        assert!(lines(&log_path(&dir)).len() >= 3);
        store.flush().unwrap();
        drop(store);

        let store = open(&dir, limits(3));
        assert_eq!(ids(&store.resident()), vec![4, 5, 6]);
        assert!(store.has_older());
    }

    #[test]
    fn evicts_by_bytes_keeping_newest() {
        let dir = tempfile::tempdir().unwrap();
        let unit = row(1, "0123456789").estimated_bytes();
        let mut store = open(
            &dir,
            RowLimits {
                max_rows: 100,
                max_bytes: unit * 3 + 1,
                max_file_bytes: u64::MAX,
            },
        );
        for id in 1..=5 {
            store.upsert(row(id, "0123456789"));
        }
        assert_eq!(ids(&store.resident()), vec![3, 4, 5]);
        assert_eq!(store.resident_bytes(), unit * 3);
        assert!(store.has_older());

        // One oversized newest row evicts everything else but is never evicted itself.
        store.upsert(row(6, &"y".repeat(unit * 10)));
        assert_eq!(ids(&store.resident()), vec![6]);
        assert!(store.has_older());
    }

    #[test]
    fn page_before_returns_older_rows_ascending() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, limits(3));
        for id in 1..=10 {
            store.upsert(row(id, &format!("row {id}")));
        }
        store.flush().unwrap();
        assert_eq!(ids(&store.resident()), vec![8, 9, 10]);

        let page = store.page_before(RowId(8), 2).unwrap();
        assert_eq!(ids(&page), vec![6, 7]);
        assert_eq!(text(&page[0]), "row 6");

        assert_eq!(
            ids(&store.page_before(RowId(5), 100).unwrap()),
            vec![1, 2, 3, 4]
        );
        // Spans file and resident rows.
        assert_eq!(
            ids(&store.page_before(RowId(10), 5).unwrap()),
            vec![5, 6, 7, 8, 9]
        );
        // Limit 0 is clamped to 1.
        assert_eq!(ids(&store.page_before(RowId(8), 0).unwrap()), vec![7]);
        assert!(store.page_before(RowId(1), 10).unwrap().is_empty());
    }

    #[test]
    fn page_before_clamps_limit_and_prefers_resident_versions() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, limits(3));
        for id in 1..=150 {
            store.upsert(row(id, "x"));
        }
        store.flush().unwrap();
        let page = store.page_before(RowId(151), 1_000).unwrap();
        assert_eq!(page.len(), MAX_HISTORY_PAGE);
        assert_eq!(page.first().unwrap().id.0, 51);
        assert_eq!(page.last().unwrap().id.0, 150);

        // Unflushed resident version wins over the file's.
        store.upsert(row(149, "fresh"));
        let page = store.page_before(RowId(150), 1).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(text(&page[0]), "fresh");
    }

    #[test]
    fn page_before_returns_latest_version_of_evicted_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, limits(2));
        for id in 1..=4 {
            store.upsert(row(id, "old"));
        }
        // Row 1 was evicted: replacing it persists but does not make it resident.
        store.upsert(row(1, "new"));
        assert_eq!(ids(&store.resident()), vec![3, 4]);
        store.flush().unwrap();
        let page = store.page_before(RowId(3), 10).unwrap();
        assert_eq!(ids(&page), vec![1, 2]);
        assert_eq!(text(&page[0]), "new");
    }

    #[test]
    fn torn_last_line_is_ignored_then_repaired_by_flush() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(&dir);
        let mut store = open(&dir, limits(100));
        store.upsert(row(1, "a"));
        store.upsert(row(2, "b"));
        store.flush().unwrap();
        drop(store);
        let good_len = fs::metadata(&path).unwrap().len();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(br#"{"v":1,"row":{"id":3,"ta"#).unwrap();
        drop(file);

        let mut store = open(&dir, limits(100));
        assert_eq!(ids(&store.resident()), vec![1, 2]);
        assert_eq!(store.next_id(), RowId(3));
        assert!(store.file_bytes() > good_len);
        store.upsert(row(3, "c"));
        store.flush().unwrap();
        drop(store);

        let all = lines(&path);
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|l| decode(l.as_bytes()).is_some()));
        assert!(fs::read_to_string(&path).unwrap().ends_with('\n'));
        let store = open(&dir, limits(100));
        assert_eq!(ids(&store.resident()), vec![1, 2, 3]);
    }

    #[test]
    fn torn_tail_is_cut_even_when_nothing_else_is_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(&dir);
        let mut store = open(&dir, limits(100));
        store.upsert(row(1, "a"));
        store.flush().unwrap();
        let good_len = store.file_bytes();
        drop(store);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"{\"v\":1,\"row\"").unwrap();
        drop(file);

        let mut store = open(&dir, limits(100));
        store.flush().unwrap();
        assert_eq!(store.file_bytes(), good_len);
        assert_eq!(fs::metadata(&path).unwrap().len(), good_len);
    }

    #[test]
    fn valid_last_line_without_newline_is_kept_and_terminated() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(&dir);
        let mut store = open(&dir, limits(100));
        store.upsert(row(1, "a"));
        store.upsert(row(2, "b"));
        store.flush().unwrap();
        drop(store);
        let content = fs::read_to_string(&path).unwrap();
        fs::write(&path, content.trim_end_matches('\n')).unwrap();

        let mut store = open(&dir, limits(100));
        assert_eq!(ids(&store.resident()), vec![1, 2]);
        store.upsert(row(3, "c"));
        store.flush().unwrap();
        drop(store);
        assert_eq!(lines(&path).len(), 3);
        let store = open(&dir, limits(100));
        assert_eq!(ids(&store.resident()), vec![1, 2, 3]);
    }

    #[test]
    fn interior_garbage_line_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(&dir);
        let mut store = open(&dir, limits(100));
        store.upsert(row(1, "a"));
        store.upsert(row(2, "b"));
        store.flush().unwrap();
        drop(store);
        let mut all = lines(&path);
        all.insert(1, "this is not json".to_owned());
        all.insert(2, r#"{"v":1,"row":{"id":"nope"}}"#.to_owned());
        all.insert(3, r#"{"v":7,"row":null}"#.to_owned());
        all.insert(4, String::new());
        fs::write(&path, all.join("\n") + "\n").unwrap();

        let mut store = open(&dir, limits(100));
        assert_eq!(ids(&store.resident()), vec![1, 2]);
        assert_eq!(store.next_id(), RowId(3));
        // A page read skips the garbage too.
        assert_eq!(ids(&store.page_before(RowId(3), 10).unwrap()), vec![1, 2]);
    }

    #[test]
    fn compaction_shrinks_file_and_keeps_newest_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(&dir);
        let max_file_bytes = 8_000;
        let mut store = open(
            &dir,
            RowLimits {
                max_rows: 3,
                max_bytes: usize::MAX,
                max_file_bytes,
            },
        );
        let payload = "p".repeat(50);
        for id in 1..=100 {
            store.upsert(row(id, &payload));
            if id % 5 == 0 {
                store.flush().unwrap();
                assert!(
                    store.file_bytes() <= max_file_bytes,
                    "file {} bytes after flush at id {id}",
                    store.file_bytes()
                );
            }
        }
        store.flush().unwrap();
        let actual = fs::metadata(&path).unwrap().len();
        assert_eq!(actual, store.file_bytes());
        assert!(actual <= max_file_bytes);
        assert!(!path.with_file_name("conversation.jsonl.compact").exists());

        assert_eq!(ids(&store.resident()), vec![98, 99, 100]);
        assert!(store.has_older());
        let page = store.page_before(RowId(98), 100).unwrap();
        assert!(!page.is_empty());
        assert!(page.iter().all(|r| r.id.0 >= 50), "dropped rows are gone");
        assert_eq!(page.last().unwrap().id.0, 97);
        assert!(page.windows(2).all(|w| w[0].id < w[1].id));
        drop(store);

        let mut store = open(&dir, limits(1000));
        assert_eq!(store.resident().last().unwrap().id.0, 100);
        assert_eq!(store.next_id(), RowId(101));
        assert!(store.resident().iter().all(|r| r.id.0 >= 50));
    }

    #[test]
    fn compaction_without_dropped_older_rows_clears_has_older() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(
            &dir,
            RowLimits {
                max_rows: 1000,
                max_bytes: usize::MAX,
                max_file_bytes: 2_000,
            },
        );
        // Rewriting one row over and over bloats the file, not the row set.
        for version in 0..60 {
            store.upsert(row(1, &format!("version {version}")));
            store.flush().unwrap();
        }
        assert!(store.file_bytes() <= 2_000);
        assert!(!store.has_older());
        assert_eq!(text(store.get(RowId(1)).unwrap()), "version 59");
        drop(store);
        let store = open(&dir, limits(10));
        assert_eq!(text(store.get(RowId(1)).unwrap()), "version 59");
    }

    #[test]
    fn flush_writes_only_dirty_ids() {
        let dir = tempfile::tempdir().unwrap();
        let path = log_path(&dir);
        let mut store = open(&dir, limits(100));
        store.upsert(row(1, "a"));
        store.upsert(row(2, "b"));
        store.upsert(row(3, "c"));
        store.flush().unwrap();
        assert_eq!(lines(&path).len(), 3);
        let size = store.file_bytes();
        assert_eq!(size, fs::metadata(&path).unwrap().len());

        // Nothing dirty: nothing written.
        store.flush().unwrap();
        assert_eq!(lines(&path).len(), 3);
        assert_eq!(store.file_bytes(), size);

        // Several intermediate versions of one id collapse to a single line.
        store.upsert(row(2, "b1"));
        store.upsert(row(2, "b2"));
        store.upsert(row(2, "b3"));
        store.upsert(row(3, "c1"));
        store.flush().unwrap();
        let all = lines(&path);
        assert_eq!(all.len(), 5);
        assert!(all[3].contains("b3"));
        assert!(
            !all.iter()
                .any(|l| l.contains("\"b1\"") || l.contains("\"b2\""))
        );
        assert_eq!(store.file_bytes(), fs::metadata(&path).unwrap().len());
    }

    #[test]
    fn default_limits_match_model_constants() {
        let limits = RowLimits::default();
        assert_eq!(limits.max_rows, MAX_RESIDENT_ROWS);
        assert_eq!(limits.max_bytes, MAX_RESIDENT_ROW_BYTES);
        assert_eq!(limits.max_file_bytes, 16 * 1024 * 1024);
    }

    #[test]
    fn a_log_that_cannot_be_written_keeps_the_resident_and_unsaved_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, limits(8));
        store.upsert(row(1, "seed"));
        store.flush().unwrap();
        // The log can no longer be appended to (a directory sits where the file was).
        fs::remove_file(log_path(&dir)).unwrap();
        fs::create_dir(log_path(&dir)).unwrap();
        for id in 2..=500 {
            store.upsert(row(id, "streamed while the disk is failing"));
            assert!(store.resident_len() <= 8, "resident window grew");
            assert!(store.dirty.len() <= 8, "unsaved rows grew");
        }
        assert!(
            store.take_lost() > 400,
            "dropped rows are counted, not hidden"
        );
        assert_eq!(store.take_lost(), 0);
        assert_eq!(ids(&store.resident()), (493..=500).collect::<Vec<_>>());
        // Dropped unsaved versions of evicted ids do not accumulate either.
        for _ in 0..1000 {
            store.upsert(row(2, "late update of an evicted row"));
        }
        assert!(store.dirty.len() <= 8);
        // Once the disk works again the retained rows persist.
        fs::remove_dir(log_path(&dir)).unwrap();
        store.flush().unwrap();
        assert!(store.dirty.is_empty());
    }

    #[test]
    fn byte_budgets_are_hard_even_when_the_log_cannot_be_written() {
        let dir = tempfile::tempdir().unwrap();
        let unit = row(1, &"z".repeat(1000)).estimated_bytes();
        let mut store = open(
            &dir,
            RowLimits {
                max_rows: 10_000,
                max_bytes: unit * 10,
                max_file_bytes: u64::MAX,
            },
        );
        store.flush().unwrap();
        fs::create_dir(log_path(&dir)).unwrap();
        for id in 1..=300 {
            store.upsert(row(id, &"z".repeat(1000)));
            assert!(store.resident_bytes() <= unit * 10);
            assert!(store.dirty_bytes <= unit * 10);
        }
        assert!(store.take_lost() > 0);
    }

    #[test]
    fn an_oversized_row_is_replaced_by_a_notice_and_never_exceeds_the_row_cap() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, RowLimits::default());
        let cap = store.row_cap();
        store.upsert(row(1, &"y".repeat(cap + 1)));
        let kept = store.get(RowId(1)).unwrap();
        assert!(kept.estimated_bytes() <= cap);
        assert!(text(kept).contains("exceeded the per-row limit"));
        store.upsert(row(2, &"y".repeat(1000)));
        assert_eq!(text(store.get(RowId(2)).unwrap()).len(), 1000);
    }

    #[test]
    fn the_latest_version_of_an_evicted_row_is_found_in_memory_or_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir, limits(3));
        for id in 1..=6 {
            store.upsert(row(id, "v1"));
        }
        store.flush().unwrap();
        assert!(store.get(RowId(1)).is_none());
        store.upsert(row(1, "v2"));
        assert_eq!(
            text(&store.latest(RowId(1)).unwrap()),
            "v2",
            "unsaved version"
        );
        store.flush().unwrap();
        assert_eq!(
            text(&store.latest(RowId(1)).unwrap()),
            "v2",
            "persisted version"
        );
        assert!(store.latest(RowId(99)).is_none());
    }

    #[test]
    fn row_store_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<RowStore>();
    }
}
