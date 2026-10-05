//! Linux execution and recovery of file-set transactions (see the parent module docs).
use super::*;
use crate::journal::{ReplayedTransaction, TxStatus};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, io::Write as _, time::SystemTime};
use studio_project::publish::{Dir, EntryKind, EntryStat, FileId, fstat, hash_reader};

/// Moving a name an editor controls (displacing an original, taking a published file
/// down) is always one atomic `renameat2`: the link-based pair is never used for it,
/// whatever an older intent recorded as its mechanism.
const LIVE: NoClobber = NoClobber::Renameat2;

/// Internal failure of one step.
#[derive(Debug)]
enum Fail {
    Crash,
    Journal(String),
    /// An outside writer or unexpected state: roll back what is provably ours.
    Conflict(String),
    Io(String),
}

impl From<Fail> for Interrupt {
    fn from(fail: Fail) -> Self {
        match fail {
            Fail::Crash => Interrupt::Crash,
            Fail::Journal(m) => Interrupt::Journal(m),
            Fail::Conflict(m) | Fail::Io(m) => Interrupt::Journal(m),
        }
    }
}

fn io(context: &str, error: io::Error) -> Fail {
    Fail::Io(format!("{context}: {error}"))
}

/// What an operation's rollback ended in.
enum Rb {
    Fail(Fail),
    /// Unknown bytes or an unverifiable original: leave everything as it is.
    Foreign(String),
}
impl From<Fail> for Rb {
    fn from(f: Fail) -> Self {
        Rb::Fail(f)
    }
}

/// Files an operation's failure into the conflict list, or propagates a real failure.
fn settle(foreign: &mut Vec<OpConflict>, op: &FileOp, result: Result<(), Rb>) -> Result<(), Fail> {
    match result {
        Ok(()) => Ok(()),
        Err(Rb::Fail(fail)) => Err(fail),
        Err(Rb::Foreign(detail)) => {
            foreign.push(OpConflict {
                index: op.index,
                path: op.delta.path.clone(),
                detail,
            });
            Ok(())
        }
    }
}

#[derive(Debug)]
enum Seen {
    Absent,
    File { stat: EntryStat, sha256: String },
    Dir,
    Other(String),
}

fn see(dir: &Dir, name: &str) -> Seen {
    match dir.stat(name) {
        Ok(None) => Seen::Absent,
        Ok(Some(stat)) if stat.is_regular() => match dir.hash_regular(name) {
            Ok((stat, sha256)) => Seen::File { stat, sha256 },
            Err(e) => Seen::Other(format!("unreadable: {e}")),
        },
        Ok(Some(stat)) if stat.kind == EntryKind::Directory => Seen::Dir,
        Ok(Some(stat)) => Seen::Other(format!("{:?}", stat.kind)),
        Err(e) => Seen::Other(e.to_string()),
    }
}

impl Seen {
    fn is_absent(&self) -> bool {
        matches!(self, Seen::Absent)
    }
    fn is_dir(&self) -> bool {
        matches!(self, Seen::Dir)
    }
    /// Same bytes, size and (when known) permission bits.
    fn is(&self, state: &FileState, mode: Option<u32>) -> bool {
        match self {
            Seen::File { stat, sha256 } => {
                *sha256 == state.sha256
                    && stat.size == state.size
                    && mode.is_none_or(|m| stat.mode == m)
            }
            _ => false,
        }
    }
    fn describe(&self) -> String {
        match self {
            Seen::Absent => "absent".into(),
            Seen::File { stat, sha256 } => {
                format!("{} bytes, sha256 {}", stat.size, &sha256[..12])
            }
            Seen::Dir => "a directory".into(),
            Seen::Other(what) => what.clone(),
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `dev:ino` as journalled in the `StageCreated` progress detail.
fn encode_id(id: FileId) -> String {
    format!("{}:{}", id.dev, id.ino)
}

fn decode_id(detail: &str) -> Option<FileId> {
    let (dev, ino) = detail.split_once(':')?;
    Some(FileId {
        dev: dev.parse().ok()?,
        ino: ino.parse().ok()?,
    })
}

/// Hashes the first `limit` bytes written and discards the rest.
struct PrefixHasher {
    limit: u64,
    seen: u64,
    digest: Sha256,
}

impl io::Write for PrefixHasher {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let take = (self.limit - self.seen).min(buf.len() as u64) as usize;
        self.digest.update(&buf[..take]);
        self.seen += take as u64;
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) struct Machine<'a> {
    root_path: &'a Path,
    root: Dir,
    checkpoints: &'a Checkpoints,
    journal: &'a mut TaskJournal,
    hooks: &'a dyn TransactionHooks,
    /// Inode of every stage this transaction created (in memory while executing,
    /// replayed from the journalled `StageCreated` progress on recovery).
    stage_ids: HashMap<usize, FileId>,
}

impl<'a> Machine<'a> {
    pub(super) fn new(
        root_path: &'a Path,
        checkpoints: &'a Checkpoints,
        journal: &'a mut TaskJournal,
        hooks: &'a dyn TransactionHooks,
    ) -> Result<Self, EngineError> {
        let root = Dir::open_root(root_path)?;
        Ok(Self {
            root_path,
            root,
            checkpoints,
            journal,
            hooks,
            stage_ids: HashMap::new(),
        })
    }

    fn hit(&self, boundary: Boundary) -> Result<(), Fail> {
        match self.hooks.at(&boundary) {
            Ok(()) => Ok(()),
            Err(Fault::Crash) => Err(Fail::Crash),
            Err(Fault::Io(kind)) => Err(Fail::Io(format!("injected {kind:?} at {boundary:?}"))),
        }
    }

    fn append(
        &mut self,
        tx: &str,
        event: TaskEvent,
        op: Option<usize>,
        step: Option<Step>,
    ) -> Result<(), Fail> {
        let label = event.label();
        let boundary = |after| Boundary::Append {
            event: label,
            op,
            step,
            after,
        };
        match self.hooks.at(&boundary(false)) {
            Ok(()) => (),
            Err(Fault::Crash) => return Err(Fail::Crash),
            Err(Fault::Io(kind)) => {
                // A failed append makes the journal refuse everything until reopened.
                self.journal.mark_failed();
                return Err(Fail::Journal(format!("injected {kind:?} while appending")));
            }
        }
        self.journal
            .append(tx, &event)
            .map_err(|e| Fail::Journal(e.to_string()))?;
        // After the write only a crash is meaningful: the event is durable.
        match self.hooks.at(&boundary(true)) {
            Err(Fault::Crash) => Err(Fail::Crash),
            _ => Ok(()),
        }
    }

    fn progress(
        &mut self,
        tx: &str,
        op: Option<usize>,
        step: Step,
        detail: Option<String>,
    ) -> Result<(), Fail> {
        self.append(
            tx,
            TaskEvent::Progress(Progress { op, step, detail }),
            op,
            Some(step),
        )
    }

    /// The root pathname must still lead, without following a link, to the directory
    /// this machine holds. Checked around every mutation and the final verification: a
    /// root renamed away and replaced by a link is a conflict, never a commit.
    fn fence(&self) -> Result<(), Fail> {
        self.root
            .check_root_binding()
            .map_err(|reason| Fail::Conflict(format!("project folder: {reason}")))
    }

    /// `rel` (relative to the root), verified against the identity the intent recorded
    /// (directories created by the transaction have none).
    fn open_dir(&self, intent: &TransactionIntent, rel: &str) -> Result<Dir, Fail> {
        let dir = self
            .root
            .open_relative(rel)
            .map_err(|e| Fail::Conflict(format!("{rel}: directory unavailable: {e}")))?;
        if let Some(expected) = intent.dir_id(rel)
            && dir.id() != expected
        {
            return Err(Fail::Conflict(format!(
                "directory {rel:?} was replaced outside the task"
            )));
        }
        Ok(dir)
    }

    /// The destination directory of `op`.
    fn dir_of(&self, intent: &TransactionIntent, op: &FileOp) -> Result<Dir, Fail> {
        self.open_dir(intent, &op.dir)
    }

    /// The directory holding `op`'s displaced original.
    fn slot_of(&self, intent: &TransactionIntent, op: &FileOp) -> Result<Dir, Fail> {
        self.open_dir(intent, op.original_dir())
    }

    fn dir_unmoved(&self, dir: &Dir, op: &FileOp) -> Result<(), Fail> {
        self.fence()?;
        if dir.is_still_at(&self.root, &op.dir) {
            Ok(())
        } else {
            Err(Fail::Conflict(format!(
                "directory {:?} changed identity during publication",
                op.dir
            )))
        }
    }

    // ---- forward ----------------------------------------------------------------

    pub(super) fn execute(&mut self, intent: &TransactionIntent) -> Result<Outcome, Interrupt> {
        self.stage_ids.clear();
        match self.forward(intent) {
            Ok(record) => {
                // The commit is durable; collecting recovery slots is best effort.
                match self.cleanup(intent) {
                    Ok(()) | Err(Fail::Io(_) | Fail::Conflict(_)) => (),
                    Err(Fail::Crash) => return Err(Interrupt::Crash),
                    Err(Fail::Journal(_)) => (),
                }
                Ok(Outcome::Committed(Box::new(record)))
            }
            Err(Fail::Crash) => Err(Interrupt::Crash),
            Err(Fail::Journal(m)) => Err(Interrupt::Journal(m)),
            Err(Fail::Conflict(reason) | Fail::Io(reason)) => self
                .rollback_all(intent, &reason, &[])
                .map_err(Interrupt::from),
        }
    }

    fn forward(&mut self, intent: &TransactionIntent) -> Result<TaskRevisionRecord, Fail> {
        let tx = intent.id.as_str();
        self.append(tx, TaskEvent::Intent(Box::new(intent.clone())), None, None)?;
        self.fence()?;
        // Everything that can be staged without changing topology is staged first; a
        // create whose directory still holds a file the revision deletes waits for it.
        for op in &intent.ops {
            if op.delta.after.is_some() && !op.defer_stage {
                self.stage(intent, op)?;
            }
        }
        for op in &intent.ops {
            match op.kind {
                OpKind::Create => {
                    if op.defer_stage {
                        self.stage(intent, op)?;
                    }
                    self.vacate(intent, op)?;
                    self.publish(intent, op)?;
                }
                OpKind::Replace => {
                    self.displace(intent, op)?;
                    self.publish(intent, op)?;
                }
                OpKind::Delete => self.displace(intent, op)?,
            }
        }
        self.hit(Boundary::FinalInventory)?;
        self.fence()?;
        let scanned = SourceInventory::scan(self.root_path)
            .map_err(|e| Fail::Conflict(format!("final inventory: {e}")))?;
        self.fence()?;
        if scanned.revision != intent.expected {
            return Err(Fail::Conflict(format!(
                "the published source ({}) is not the validated candidate ({}); something outside the task changed the project during publication",
                &scanned.revision.as_str()[..12],
                &intent.expected.as_str()[..12]
            )));
        }
        // Displaced originals must still be what was verified: a write through a
        // descriptor an editor kept open would otherwise vanish with the slot.
        for op in &intent.ops {
            if let (Some(before), OpKind::Replace | OpKind::Delete) = (&op.delta.before, op.kind) {
                let slot_dir = self.slot_of(intent, op)?;
                let slot = see(&slot_dir, &op.recovery.original);
                if !slot.is(before, op.before_mode()) {
                    return Err(Fail::Conflict(format!(
                        "{}: the displaced original changed after displacement ({})",
                        op.delta.path,
                        slot.describe()
                    )));
                }
            }
        }
        self.hit(Boundary::InventoryVerified)?;
        self.fence()?;
        let mut record = intent.record.clone();
        record.committed_unix = now();
        self.append(tx, TaskEvent::Commit(Box::new(record.clone())), None, None)?;
        Ok(record)
    }

    fn ensure_dirs(&mut self, intent: &TransactionIntent, op: &FileOp) -> Result<(), Fail> {
        for new in &op.new_dirs {
            let (parent, name) = split_path(new);
            let parent_dir = self
                .root
                .open_relative(parent)
                .map_err(|e| io("parent directory", e))?;
            match parent_dir.mkdir(name, 0o755) {
                Ok(()) => {
                    parent_dir.sync().map_err(|e| io("sync directory", e))?;
                    self.hit(Boundary::DirCreated(op.index))?;
                    self.progress(
                        &intent.id,
                        Some(op.index),
                        Step::DirCreated,
                        Some(new.clone()),
                    )?;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let kind = parent_dir.stat(name).ok().flatten().map(|s| s.kind);
                    if kind != Some(EntryKind::Directory) {
                        return Err(Fail::Conflict(format!("{new}: not a directory")));
                    }
                }
                Err(e) => return Err(io(new, e)),
            }
        }
        Ok(())
    }

    fn stage(&mut self, intent: &TransactionIntent, op: &FileOp) -> Result<(), Fail> {
        let after = op.delta.after.as_ref().expect("staged op has an after");
        self.ensure_dirs(intent, op)?;
        let dir = self.dir_of(intent, op)?;
        let mode = op.after_mode().unwrap_or(0o644);
        let mut file = dir
            .create_new(&op.recovery.stage, mode)
            .map_err(|e| io(&op.recovery.stage, e))?;
        // The inode is durable before any byte is written, so a crash that leaves a
        // partial stage can tell it from a stranger's file.
        let id = fstat(&file).map_err(|e| io("stage stat", e))?.id;
        self.stage_ids.insert(op.index, id);
        self.progress(
            &intent.id,
            Some(op.index),
            Step::StageCreated,
            Some(encode_id(id)),
        )?;
        self.hit(Boundary::StageCreated(op.index))?;
        let written = self
            .checkpoints
            .copy_object(&after.sha256, after.size, &mut file)
            .map_err(|e| Fail::Io(e.to_string()))
            .and_then(|()| file.flush().map_err(|e| io("stage flush", e)));
        if let Err(fail) = written {
            // We created this exact file a moment ago and hold its descriptor: a
            // failed copy (full disk, rotted object) is ours to remove.
            if dir.stat(&op.recovery.stage).ok().flatten().map(|s| s.id) == Some(id) {
                let _ = dir.unlink(&op.recovery.stage);
            }
            return Err(fail);
        }
        self.hit(Boundary::StageWritten(op.index))?;
        file.sync_all().map_err(|e| io("stage sync", e))?;
        drop(file);
        dir.sync().map_err(|e| io("stage directory sync", e))?;
        self.hit(Boundary::StageSynced(op.index))?;
        self.progress(&intent.id, Some(op.index), Step::Staged, None)
    }

    /// Directory -> file: removes the directories this create replaces (deepest first)
    /// once every delete beneath them was displaced. A directory that still holds
    /// anything else is a conflict and is never emptied by force.
    fn vacate(&mut self, intent: &TransactionIntent, op: &FileOp) -> Result<(), Fail> {
        for vacated in &op.vacates {
            let (parent, name) = split_path(&vacated.path);
            let parent_dir = self.open_dir(intent, parent)?;
            let current = parent_dir.open_child(name).map_err(|e| {
                Fail::Conflict(format!("{}: directory unavailable: {e}", vacated.path))
            })?;
            if current.id() != vacated.id {
                return Err(Fail::Conflict(format!(
                    "directory {:?} was replaced outside the task",
                    vacated.path
                )));
            }
            self.fence()?;
            match parent_dir.rmdir(name) {
                Ok(()) => (),
                Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => {
                    return Err(Fail::Conflict(format!(
                        "{}: the directory still holds files the task did not displace",
                        vacated.path
                    )));
                }
                Err(e) => return Err(io(&vacated.path, e)),
            }
            parent_dir.sync().map_err(|e| io("sync directory", e))?;
            self.hit(Boundary::DirRemoved(op.index))?;
            self.progress(
                &intent.id,
                Some(op.index),
                Step::DirRemoved,
                Some(vacated.path.clone()),
            )?;
        }
        Ok(())
    }

    fn displace(&mut self, intent: &TransactionIntent, op: &FileOp) -> Result<(), Fail> {
        let before = op.delta.before.as_ref().expect("displaced op has a before");
        let dir = self.dir_of(intent, op)?;
        let slot_dir = self.slot_of(intent, op)?;
        self.hit(Boundary::BeforeDisplace(op.index))?;
        let live = see(&dir, &op.name);
        let identity_ok = matches!(&live, Seen::File { stat, .. }
            if Some(stat.id) == op.before_id);
        if !live.is(before, op.before_mode()) || !identity_ok {
            return Err(Fail::Conflict(format!(
                "{}: the file changed outside the task before it was displaced ({})",
                op.delta.path,
                live.describe()
            )));
        }
        self.dir_unmoved(&dir, op)?;
        // One atomic syscall: the live name is never linked-then-unlinked.
        dir.rename_noreplace(LIVE, &op.name, &slot_dir, &op.recovery.original)
            .map_err(|e| match e.kind() {
                io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound => Fail::Conflict(format!(
                    "{}: could not displace the original: {e}",
                    op.delta.path
                )),
                _ => io(&op.delta.path, e),
            })?;
        self.hit(Boundary::AfterDisplace(op.index))?;
        // Verify exactly what was moved. A write that raced the displacement (an
        // editor saving by rename, or an open descriptor) shows here.
        let moved = see(&slot_dir, &op.recovery.original);
        let verified = matches!(&moved, Seen::File { stat, .. } if Some(stat.id) == op.before_id)
            && moved.is(before, op.before_mode());
        if !verified {
            // Put whatever we moved back where the user had it, if that is still
            // possible (the slot is a private name); either way nothing is deleted.
            let _ = slot_dir.rename_noreplace(op.mechanism, &op.recovery.original, &dir, &op.name);
            let _ = dir.sync();
            let _ = slot_dir.sync();
            return Err(Fail::Conflict(format!(
                "{}: the displaced file is not the verified original ({}); every variant was retained",
                op.delta.path,
                moved.describe()
            )));
        }
        dir.sync().map_err(|e| io("displacement sync", e))?;
        slot_dir
            .sync()
            .map_err(|e| io("displacement slot sync", e))?;
        self.hit(Boundary::DisplacedVerified(op.index))?;
        self.progress(&intent.id, Some(op.index), Step::Displaced, None)
    }

    fn publish(&mut self, intent: &TransactionIntent, op: &FileOp) -> Result<(), Fail> {
        let after = op.delta.after.as_ref().expect("published op has an after");
        let dir = self.dir_of(intent, op)?;
        self.hit(Boundary::BeforePublish(op.index))?;
        self.dir_unmoved(&dir, op)?;
        // The source is our private stage name, so even a link-based legacy mechanism
        // cannot lose anyone's file here.
        dir.rename_noreplace(op.mechanism, &op.recovery.stage, &dir, &op.name)
            .map_err(|e| match e.kind() {
                io::ErrorKind::AlreadyExists => Fail::Conflict(format!(
                    "{}: an outside writer created the destination during publication; its file was left untouched",
                    op.delta.path
                )),
                io::ErrorKind::NotFound => Fail::Conflict(format!(
                    "{}: the staged file vanished before publication",
                    op.delta.path
                )),
                _ => io(&op.delta.path, e),
            })?;
        self.hit(Boundary::AfterPublish(op.index))?;
        self.dir_unmoved(&dir, op)?;
        let published = see(&dir, &op.name);
        let staged_id = self.stage_ids.get(&op.index).copied();
        let same_inode = matches!(&published, Seen::File { stat, .. }
            if staged_id.is_none_or(|id| id == stat.id));
        if !published.is(after, op.after_mode()) || !same_inode {
            return Err(Fail::Conflict(format!(
                "{}: the published file is not the staged bytes ({})",
                op.delta.path,
                published.describe()
            )));
        }
        dir.sync().map_err(|e| io("publication sync", e))?;
        self.hit(Boundary::PublishVerified(op.index))?;
        self.progress(&intent.id, Some(op.index), Step::Published, None)
    }

    /// Collects the recovery slots of a committed transaction. A slot whose bytes
    /// are no longer the verified original is kept and recorded.
    fn cleanup(&mut self, intent: &TransactionIntent) -> Result<(), Fail> {
        for op in &intent.ops {
            self.hit(Boundary::Cleanup(op.index))?;
            if let Some(before) = &op.delta.before
                && let Ok(slot_dir) = self.slot_of(intent, op)
            {
                let slot = see(&slot_dir, &op.recovery.original);
                if slot.is(before, op.before_mode()) {
                    let _ = slot_dir.unlink(&op.recovery.original);
                } else if !slot.is_absent() {
                    self.progress(
                        &intent.id,
                        Some(op.index),
                        Step::Retained,
                        Some(format!(
                            "{} kept: {}",
                            op.recovery.original,
                            slot.describe()
                        )),
                    )?;
                }
                let _ = slot_dir.sync();
            }
            if let Some(after) = &op.delta.after
                && let Ok(dir) = self.dir_of(intent, op)
            {
                for leftover in [&op.recovery.stage, &op.recovery.rollback] {
                    // Only our own bytes are collected.
                    if see(&dir, leftover).is(after, None) {
                        let _ = dir.unlink(leftover);
                    }
                }
                let _ = dir.sync();
            }
        }
        // Directories an undone revision created go once they are empty again.
        let mut dirs: Vec<&String> = intent.remove_dirs.iter().collect();
        dirs.sort();
        for dir in dirs.into_iter().rev() {
            let (parent, name) = split_path(dir);
            if let Ok(parent_dir) = self.root.open_relative(parent)
                && parent_dir.rmdir(name).is_ok()
            {
                let _ = parent_dir.sync();
            }
        }
        self.progress(&intent.id, None, Step::Cleaned, None)
    }

    // ---- rollback / recovery ----------------------------------------------------

    /// Rolls every operation back where that is provably safe and ends the
    /// transaction as `RolledBack` or, if any operation holds bytes we must not
    /// touch, as a conflict that retains every variant.
    ///
    /// Operations run in reverse index order, which is the inverse of the forward
    /// schedule: after one operation is undone, the directories it removed come back
    /// and the directories it created go (when empty), so the operations before it find
    /// the topology they left. `progress` is what a replayed journal recorded
    /// (`StageCreated` identities); a live rollback passes `&[]` and uses memory.
    fn rollback_all(
        &mut self,
        intent: &TransactionIntent,
        reason: &str,
        progress: &[Progress],
    ) -> Result<Outcome, Fail> {
        for p in progress {
            if p.step == Step::StageCreated
                && let (Some(op), Some(detail)) = (p.op, &p.detail)
                && let Some(id) = decode_id(detail)
            {
                self.stage_ids.entry(op).or_insert(id);
            }
        }
        let mut foreign: Vec<OpConflict> = Vec::new();
        for op in intent.ops.iter().rev() {
            self.hit(Boundary::RollbackBegin(op.index))?;
            let result = self.rollback_op(intent, op);
            settle(&mut foreign, op, result)?;
            let result = self.restore_vacated(op);
            settle(&mut foreign, op, result)?;
            self.remove_op_dirs(op);
        }
        foreign.reverse();
        // Directories the transaction created go if they are empty again, whatever
        // else happened (a non-empty one is simply left alone).
        self.remove_new_dirs(intent);
        if foreign.is_empty() {
            self.append(
                &intent.id,
                TaskEvent::RolledBack {
                    reason: reason.to_owned(),
                },
                None,
                None,
            )?;
            return Ok(Outcome::RolledBack(reason.to_owned()));
        }
        let variants = self.collect_variants(intent, &foreign);
        let report = ConflictReport {
            transaction: intent.id.clone(),
            reason: reason.to_owned(),
            ops: foreign,
            variants,
        };
        self.append(
            &intent.id,
            TaskEvent::Conflict(Box::new(report.clone())),
            None,
            None,
        )?;
        Ok(Outcome::Conflicted(Box::new(report)))
    }

    /// Removes the directories `op` created, deepest first; only empty ones go.
    fn remove_op_dirs(&self, op: &FileOp) {
        for new in op.new_dirs.iter().rev() {
            let (parent, name) = split_path(new);
            if let Ok(parent_dir) = self.root.open_relative(parent)
                && parent_dir.rmdir(name).is_ok()
            {
                let _ = parent_dir.sync();
            }
        }
    }

    fn remove_new_dirs(&self, intent: &TransactionIntent) {
        let mut dirs: BTreeSet<&String> = BTreeSet::new();
        for op in &intent.ops {
            dirs.extend(op.new_dirs.iter());
        }
        // Deepest first; only empty directories go.
        for new in dirs.into_iter().rev() {
            let (parent, name) = split_path(new);
            if let Ok(parent_dir) = self.root.open_relative(parent)
                && parent_dir.rmdir(name).is_ok()
            {
                let _ = parent_dir.sync();
            }
        }
    }

    /// Recreates (shallowest first, with their recorded mode) the directories `op`
    /// removed, if they are not there.
    fn restore_vacated(&mut self, op: &FileOp) -> Result<(), Rb> {
        for vacated in op.vacates.iter().rev() {
            let (parent, name) = split_path(&vacated.path);
            let parent_dir = self.root.open_relative(parent).map_err(|e| {
                Rb::Foreign(format!(
                    "{}: its parent directory is unavailable ({e})",
                    vacated.path
                ))
            })?;
            match parent_dir.stat(name) {
                Ok(None) => {
                    parent_dir
                        .mkdir(name, vacated.mode)
                        .map_err(|e| Rb::Fail(io(&vacated.path, e)))?;
                    parent_dir
                        .open_child(name)
                        .and_then(|dir| dir.chmod(vacated.mode))
                        .map_err(|e| Rb::Fail(io(&vacated.path, e)))?;
                    parent_dir
                        .sync()
                        .map_err(|e| Rb::Fail(io("sync directory", e)))?;
                    self.hit(Boundary::DirRestored(op.index))?;
                }
                Ok(Some(stat)) if stat.kind == EntryKind::Directory => (),
                Ok(Some(_)) | Err(_) => {
                    return Err(Rb::Foreign(format!(
                        "{}: something other than the removed directory occupies its name; the displaced files were retained",
                        vacated.path
                    )));
                }
            }
        }
        Ok(())
    }

    fn collect_variants(
        &self,
        intent: &TransactionIntent,
        foreign: &[OpConflict],
    ) -> Vec<RetainedVariant> {
        let mut variants = Vec::new();
        for conflict in foreign {
            let Some(op) = intent.ops.get(conflict.index) else {
                continue;
            };
            let listing = [
                ("destination", op.dir.as_str(), op.name.clone()),
                ("stage", op.dir.as_str(), op.recovery.stage.clone()),
                ("original", op.original_dir(), op.recovery.original.clone()),
                ("rollback", op.dir.as_str(), op.recovery.rollback.clone()),
            ];
            for (role, dir_rel, name) in listing {
                let Ok(dir) = self.root.open_relative(dir_rel) else {
                    continue;
                };
                let relative = if dir_rel.is_empty() {
                    name.clone()
                } else {
                    format!("{dir_rel}/{name}")
                };
                match see(&dir, &name) {
                    Seen::Absent => (),
                    Seen::File { stat, sha256 } => variants.push(RetainedVariant {
                        path: relative,
                        role: role.into(),
                        sha256: Some(sha256),
                        size: Some(stat.size),
                    }),
                    Seen::Dir | Seen::Other(_) => variants.push(RetainedVariant {
                        path: relative,
                        role: role.into(),
                        sha256: None,
                        size: None,
                    }),
                }
            }
        }
        variants
    }

    /// Removes `name` only if it is provably one of our own files.
    ///
    /// - The file must be the inode this transaction created (`expected`, journalled
    ///   before any byte was written); a replaced inode is never ours, whatever its
    ///   bytes.
    /// - Its bytes must be the complete after bytes, or - for a stage file that a crash
    ///   or a full disk left half written - a *prefix* of them, hashed through the open
    ///   descriptor against the same prefix of the checkpoint object. Without a
    ///   journalled identity only an empty stage qualifies (a crash between creating
    ///   the file and journalling its inode).
    ///
    /// Anything else is retained and turns the transaction into a conflict.
    fn discard_ours(
        &self,
        dir: &Dir,
        name: &str,
        ours: &FileState,
        expected: Option<FileId>,
        stage: bool,
    ) -> Result<(), Rb> {
        match dir.stat(name) {
            Ok(None) => return Ok(()),
            Ok(Some(stat)) if stat.is_regular() => (),
            Ok(Some(stat)) => {
                return Err(Rb::Foreign(format!(
                    "{name} is {:?}, not a file the task staged; retained",
                    stat.kind
                )));
            }
            Err(e) => return Err(Rb::Foreign(format!("{name}: {e}; retained"))),
        }
        let (mut file, stat) = dir
            .open_regular(name)
            .map_err(|e| Rb::Foreign(format!("{name}: {e}; retained")))?;
        if let Some(id) = expected
            && stat.id != id
        {
            return Err(Rb::Foreign(format!(
                "{name} is not the file the task created (its inode changed); retained"
            )));
        }
        let (hash, size) = hash_reader(&mut file)
            .map_err(|e| Rb::Foreign(format!("{name}: unreadable ({e}); retained")))?;
        let complete = size == ours.size && hash == ours.sha256;
        let prefix = stage && size < ours.size && (expected.is_some() || size == 0) && {
            let mut hasher = PrefixHasher {
                limit: size,
                seen: 0,
                digest: Sha256::new(),
            };
            self.checkpoints
                .copy_object(&ours.sha256, ours.size, &mut hasher)
                .is_ok()
                && format!("{:x}", hasher.digest.finalize()) == hash
        };
        if !(complete || prefix) {
            return Err(Rb::Foreign(format!(
                "{name} is neither the staged bytes nor a verified prefix of them ({size} bytes, sha256 {}); retained",
                &hash[..12]
            )));
        }
        // The name must still be the inode that was verified.
        if dir.stat(name).ok().flatten().map(|s| s.id) != Some(stat.id) {
            return Err(Rb::Foreign(format!(
                "{name} changed while it was being verified; retained"
            )));
        }
        dir.unlink(name).map_err(|e| io(name, e))?;
        Ok(())
    }

    fn rollback_op(&mut self, intent: &TransactionIntent, op: &FileOp) -> Result<(), Rb> {
        let dir = match self.root.open_relative(&op.dir) {
            Ok(dir) => dir,
            // A directory the transaction never got to create holds nothing of ours:
            // absent, or (file -> directory) still occupied by the file the revision
            // has not displaced yet.
            Err(e)
                if op.kind == OpKind::Create
                    && (e.kind() == io::ErrorKind::NotFound
                        || (!op.new_dirs.is_empty()
                            && e.kind() == io::ErrorKind::NotADirectory)) =>
            {
                return Ok(());
            }
            Err(e) => return Err(Rb::Foreign(format!("directory unavailable: {e}"))),
        };
        // A directory the revision itself removed and rollback recreated has a new
        // identity by design.
        let identity = |rel: &str, dir: &Dir| match intent.dir_id(rel) {
            Some(expected) if dir.id() != expected && !intent.is_vacated(rel) => Err(Rb::Foreign(
                "the directory was replaced outside the task".into(),
            )),
            _ => Ok(()),
        };
        identity(&op.dir, &dir)?;
        let slot_dir = if op.original_dir() == op.dir {
            dir.try_clone()
                .map_err(|e| Rb::Fail(io("clone directory", e)))?
        } else {
            let slot_dir = self
                .root
                .open_relative(op.original_dir())
                .map_err(|e| Rb::Foreign(format!("original slot directory unavailable: {e}")))?;
            identity(op.original_dir(), &slot_dir)?;
            slot_dir
        };
        let live = see(&dir, &op.name);
        let original = see(&slot_dir, &op.recovery.original);
        let before = op.delta.before.as_ref();
        let after = op.delta.after.as_ref();
        let stage_id = self.stage_ids.get(&op.index).copied();
        match op.kind {
            OpKind::Create => {
                let after = after.expect("create has an after");
                // Nothing published yet: the name is free, or (directory -> file) still
                // the directory the revision has not removed.
                let unpublished = live.is_absent() || (live.is_dir() && !op.vacates.is_empty());
                if unpublished {
                    self.discard_ours(&dir, &op.recovery.stage, after, stage_id, true)?;
                    self.discard_ours(&dir, &op.recovery.rollback, after, stage_id, false)?;
                } else if live.is(after, op.after_mode()) {
                    self.take_down_published(&dir, op, after)?;
                    self.discard_ours(&dir, &op.recovery.stage, after, stage_id, true)?;
                } else {
                    return Err(Rb::Foreign(format!(
                        "{} holds bytes the task did not write ({})",
                        op.delta.path,
                        live.describe()
                    )));
                }
            }
            OpKind::Replace | OpKind::Delete => {
                let before = before.expect("replace/delete has a before");
                let original_ok = original.is(before, op.before_mode());
                if live.is(before, op.before_mode()) && original.is_absent() {
                    // Never touched.
                } else if live.is_absent() && original_ok {
                    self.restore_original(&dir, &slot_dir, op)?;
                } else if let (Some(after), true) = (after, original_ok) {
                    if live.is(after, op.after_mode()) {
                        self.take_down_published(&dir, op, after)?;
                        self.restore_original(&dir, &slot_dir, op)?;
                    } else {
                        return Err(Rb::Foreign(format!(
                            "{} holds bytes the task did not write ({}); the displaced original was retained",
                            op.delta.path,
                            live.describe()
                        )));
                    }
                } else {
                    return Err(Rb::Foreign(format!(
                        "{}: current {} and displaced original {} cannot be verified; nothing was overwritten",
                        op.delta.path,
                        live.describe(),
                        original.describe()
                    )));
                }
                if let Some(after) = after {
                    self.discard_ours(&dir, &op.recovery.stage, after, stage_id, true)?;
                    self.discard_ours(&dir, &op.recovery.rollback, after, stage_id, false)?;
                }
            }
        }
        let _ = dir.sync();
        let _ = slot_dir.sync();
        self.progress(&intent.id, Some(op.index), Step::RolledBack, None)?;
        Ok(())
    }

    /// Moves our published file aside (never deleting in place), verifies what was
    /// moved is still exactly our bytes and our inode, then discards it. If an outside
    /// writer got there first the moved file is put back.
    fn take_down_published(&mut self, dir: &Dir, op: &FileOp, after: &FileState) -> Result<(), Rb> {
        // The source is the live name: one atomic renameat2, never link + unlink.
        dir.rename_noreplace(LIVE, &op.name, dir, &op.recovery.rollback)
            .map_err(|e| match e.kind() {
                io::ErrorKind::AlreadyExists | io::ErrorKind::NotFound => Rb::Foreign(format!(
                    "{}: could not move the published file aside: {e}",
                    op.delta.path
                )),
                _ => Rb::Fail(io(&op.delta.path, e)),
            })?;
        self.hit(Boundary::RollbackMoved(op.index))?;
        let moved = see(dir, &op.recovery.rollback);
        let identity_ok = self
            .stage_ids
            .get(&op.index)
            .is_none_or(|id| matches!(&moved, Seen::File { stat, .. } if stat.id == *id));
        if !moved.is(after, None) || !identity_ok {
            let _ = dir.rename_noreplace(op.mechanism, &op.recovery.rollback, dir, &op.name);
            return Err(Rb::Foreign(format!(
                "{}: the published file changed during rollback ({}); it was kept",
                op.delta.path,
                moved.describe()
            )));
        }
        dir.unlink(&op.recovery.rollback)
            .map_err(|e| Rb::Fail(io(&op.recovery.rollback, e)))?;
        Ok(())
    }

    /// The source is our private slot, so the operation's mechanism is safe here.
    fn restore_original(&mut self, dir: &Dir, slot_dir: &Dir, op: &FileOp) -> Result<(), Rb> {
        slot_dir
            .rename_noreplace(op.mechanism, &op.recovery.original, dir, &op.name)
            .map_err(|e| match e.kind() {
                io::ErrorKind::AlreadyExists => Rb::Foreign(format!(
                    "{}: an outside writer created the destination; the displaced original was retained",
                    op.delta.path
                )),
                _ => Rb::Fail(io(&op.delta.path, e)),
            })?;
        self.hit(Boundary::RollbackRestored(op.index))?;
        Ok(())
    }

    /// Replays unfinished transactions on open.
    pub(super) fn recover(
        &mut self,
        transactions: &[ReplayedTransaction],
    ) -> Result<RecoveryReport, Interrupt> {
        let mut report = RecoveryReport::default();
        for tx in transactions {
            match &tx.status {
                TxStatus::Pending => {
                    report.touched_source = true;
                    self.stage_ids.clear();
                    match self.rollback_all(
                        &tx.intent,
                        "interrupted before its commit was durable",
                        &tx.progress,
                    ) {
                        Ok(Outcome::RolledBack(_)) => report.rolled_back.push(tx.id.clone()),
                        Ok(Outcome::Conflicted(conflict)) => report.conflicts.push(*conflict),
                        Ok(Outcome::Committed(_)) => unreachable!("rollback never commits"),
                        Err(fail) => return Err(fail.into()),
                    }
                }
                TxStatus::Committed(_) => {
                    let cleaned = tx
                        .progress
                        .iter()
                        .any(|p| p.step == Step::Cleaned && p.op.is_none());
                    if !cleaned {
                        report.touched_source = true;
                        self.cleanup(&tx.intent).map_err(Interrupt::from)?;
                        report.cleaned.push(tx.id.clone());
                    }
                }
                TxStatus::Conflicted(conflict) => report.conflicts.push((**conflict).clone()),
                TxStatus::RolledBack(_) | TxStatus::Resolved(_) => (),
            }
        }
        Ok(report)
    }
}
