//! Linux publication primitives for durable, no-clobber source mutation.
//!
//! Every operation is relative to a held directory descriptor and never follows a link.
//! Moving a name that an editor controls (displacing an original, taking a published
//! file down again) uses `renameat2(RENAME_NOREPLACE)` only: one atomic syscall, so a
//! destination or a source that an uncooperative editor replaced is never overwritten or
//! unlinked by mistake.
//!
//! [`NoClobber::Link`] (`linkat` then `unlinkat`) is **not** an equivalent. Between the
//! two calls an editor can save by renaming a new file over the source name, and the
//! `unlinkat` then deletes the editor's file while the link keeps the old inode alive,
//! so every later verification of the moved bytes still passes (see
//! `tests/publish.rs::link_then_unlink_loses_an_editor_save_between_the_two_syscalls`).
//! It is only safe where the *source* is a private, immutable name nobody else knows,
//! and the transaction layer never uses it to move a live source name:
//! [`probe_no_clobber`] qualifies `renameat2` alone and callers keep their Apply gate
//! blocked where it is unavailable.
//!
//! [`Dir::open_root`] binds the root *pathname* (not only its inode) with a no-follow
//! walk from `/`; [`Dir::check_root_binding`] re-walks it so a root renamed away and
//! replaced by a link is detected around each mutation and the final verification.
//!
//! These primitives cannot exclude a malicious writer that holds an open descriptor;
//! the transaction layer detects such races by re-verifying identity and hash around
//! each step and retains every observed variant.
use std::{
    ffi::CString,
    fs::File,
    io::{self, Read},
    os::fd::{AsRawFd, FromRawFd},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `(device, inode)` of a file or directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileId {
    pub dev: u64,
    pub ino: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Regular,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryStat {
    pub id: FileId,
    pub kind: EntryKind,
    pub size: u64,
    /// Permission bits including setuid/setgid/sticky (`st_mode & 0o7777`).
    pub mode: u32,
    pub nlink: u64,
    /// Last data modification (`st_mtim`), nanoseconds since the epoch.
    pub modified_ns: i128,
    /// Last inode change (`st_ctim`: writes, `chmod`, rename), nanoseconds since the epoch.
    pub changed_ns: i128,
}

impl EntryStat {
    pub fn is_regular(&self) -> bool {
        self.kind == EntryKind::Regular
    }
    pub fn executable(&self) -> bool {
        self.mode & 0o111 != 0
    }
}

/// How a staged file is published without replacing an existing destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoClobber {
    /// `renameat2(RENAME_NOREPLACE)`: atomic. The only mechanism that may move a name an
    /// editor controls.
    Renameat2,
    /// `linkat` then `unlinkat`: never replaces the destination, but is two syscalls: an
    /// editor saving by rename between them has its file unlinked. Only for sources that
    /// are private immutable names (never a live project file).
    Link,
}

impl NoClobber {
    pub fn label(self) -> &'static str {
        match self {
            Self::Renameat2 => "renameat2(RENAME_NOREPLACE)",
            Self::Link => "linkat+unlinkat",
        }
    }
}

fn component(name: &str) -> io::Result<CString> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} is not a single path component"),
        ));
    }
    CString::new(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

fn check(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

// `libc::stat` field widths differ per platform (e.g. `st_nlink` is `u16` on macOS), so
// these casts are no-ops on some targets and required on others.
#[allow(clippy::unnecessary_cast)]
fn stat_of(raw: &libc::stat) -> EntryStat {
    let kind = match raw.st_mode & libc::S_IFMT {
        libc::S_IFREG => EntryKind::Regular,
        libc::S_IFDIR => EntryKind::Directory,
        libc::S_IFLNK => EntryKind::Symlink,
        _ => EntryKind::Other,
    };
    EntryStat {
        id: FileId {
            dev: raw.st_dev as u64,
            ino: raw.st_ino as u64,
        },
        kind,
        size: raw.st_size as u64,
        mode: raw.st_mode as u32 & 0o7777,
        nlink: raw.st_nlink as u64,
        modified_ns: i128::from(raw.st_mtime) * 1_000_000_000 + i128::from(raw.st_mtime_nsec),
        changed_ns: i128::from(raw.st_ctime) * 1_000_000_000 + i128::from(raw.st_ctime_nsec),
    }
}

/// `fstat` of an open descriptor.
pub fn fstat(file: &File) -> io::Result<EntryStat> {
    // SAFETY: zeroed `stat` is a valid out-parameter and the descriptor is open.
    let mut raw: libc::stat = unsafe { std::mem::zeroed() };
    check(unsafe { libc::fstat(file.as_raw_fd(), &mut raw) })?;
    Ok(stat_of(&raw))
}

/// SHA-256 and length of everything readable from `reader`.
pub fn hash_reader(reader: &mut impl Read) -> io::Result<(String, u64)> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        size += count as u64;
        digest.update(&buffer[..count]);
    }
    Ok((format!("{:x}", digest.finalize()), size))
}

/// The identity chain of a root pathname: the `(device, inode)` of every directory from
/// `/` down to the root, found by a no-follow walk. Re-walking it later shows whether
/// the pathname still leads to the very directory that was opened (a root renamed away
/// and replaced by a link, or any ancestor swapped, changes the chain).
#[derive(Debug)]
struct RootFence {
    path: PathBuf,
    chain: Vec<FileId>,
}

/// Walks `path` (absolute, already canonical) from `/` without following any link and
/// returns the final directory plus the id of every directory on the way.
fn walk_no_follow(path: &Path) -> io::Result<(File, Vec<FileId>)> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Component;
    let mut current = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open("/")?;
    let mut chain = vec![fstat(&current)?.id];
    for part in path.components() {
        let name = match part {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a canonical absolute path", path.display()),
                ));
            }
        };
        let name = CString::new(std::os::unix::ffi::OsStrExt::as_bytes(name))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        // SAFETY: valid directory descriptor and NUL-terminated component.
        let fd = check(unsafe {
            libc::openat(
                current.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?;
        // SAFETY: openat returned a new descriptor we own.
        current = unsafe { File::from_raw_fd(fd) };
        chain.push(fstat(&current)?.id);
    }
    Ok((current, chain))
}

/// A directory held open by descriptor. All names are single components resolved
/// relative to it, so a path swap above it cannot redirect an operation, and every
/// open refuses symlinks.
#[derive(Debug)]
pub struct Dir {
    file: File,
    id: FileId,
    /// Set on a root handle (and its clones): the pathname binding to re-verify.
    fence: Option<Arc<RootFence>>,
}

impl Dir {
    /// Opens `path` as the root handle. The path is canonicalized once and then walked
    /// from `/` one no-follow component at a time, recording the identity of every
    /// directory on the way (see [`Self::check_root_binding`]).
    pub fn open_root(path: &Path) -> io::Result<Self> {
        let path = std::fs::canonicalize(path)?;
        let (file, chain) = walk_no_follow(&path)?;
        let id = fstat(&file)?.id;
        Ok(Self {
            file,
            id,
            fence: Some(Arc::new(RootFence { path, chain })),
        })
    }

    /// Whether the pathname this root handle was opened by still resolves, without
    /// following any link, to the same directory through the same ancestors. `Err`
    /// describes what changed. A handle that is not a root has nothing to check.
    pub fn check_root_binding(&self) -> Result<(), String> {
        let Some(fence) = &self.fence else {
            return Ok(());
        };
        match walk_no_follow(&fence.path) {
            Ok((_, chain)) if chain == fence.chain => Ok(()),
            Ok(_) => Err(format!(
                "{} no longer leads to the opened directory (it or an ancestor was replaced)",
                fence.path.display()
            )),
            Err(e) => Err(format!(
                "{} can no longer be resolved without following a link ({e}); it was moved, removed or replaced by a link",
                fence.path.display()
            )),
        }
    }

    /// Descends `relative` (slash separated, empty = this directory) one no-follow
    /// component at a time.
    pub fn open_relative(&self, relative: &str) -> io::Result<Self> {
        let mut current = self.try_clone()?;
        if relative.is_empty() {
            return Ok(current);
        }
        for part in relative.split('/') {
            current = current.open_child(part)?;
        }
        Ok(current)
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            id: self.id,
            fence: self.fence.clone(),
        })
    }

    pub fn open_child(&self, name: &str) -> io::Result<Self> {
        let name = component(name)?;
        // SAFETY: valid directory descriptor and NUL-terminated component.
        let fd = check(unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?;
        // SAFETY: openat returned a new descriptor we own.
        let file = unsafe { File::from_raw_fd(fd) };
        let id = fstat(&file)?.id;
        Ok(Self {
            file,
            id,
            fence: None,
        })
    }

    pub fn id(&self) -> FileId {
        self.id
    }

    /// `fstat` of the directory itself (its permission bits, for example).
    pub fn stat_self(&self) -> io::Result<EntryStat> {
        fstat(&self.file)
    }

    /// `fchmod` of the directory itself (so the umask never changes a restored mode).
    pub fn chmod(&self, mode: u32) -> io::Result<()> {
        // SAFETY: the descriptor is open.
        check(unsafe { libc::fchmod(self.file.as_raw_fd(), mode as libc::mode_t) }).map(drop)
    }

    /// Whether `relative` below `root` still resolves (without following links) to this
    /// very directory. A swapped directory or link makes this false.
    pub fn is_still_at(&self, root: &Dir, relative: &str) -> bool {
        root.open_relative(relative)
            .is_ok_and(|current| current.id == self.id)
    }

    /// `lstat` of a name; `None` if it does not exist.
    pub fn stat(&self, name: &str) -> io::Result<Option<EntryStat>> {
        let name = component(name)?;
        // SAFETY: zeroed `stat` is a valid out-parameter.
        let mut raw: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: valid directory descriptor and NUL-terminated component.
        let result = unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                &mut raw,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            };
        }
        Ok(Some(stat_of(&raw)))
    }

    /// Opens a regular file read-only without following links. The returned stat is the
    /// descriptor's own (`fstat`), so it describes exactly the inode that was opened.
    pub fn open_regular(&self, name: &str) -> io::Result<(File, EntryStat)> {
        let name = component(name)?;
        // SAFETY: valid directory descriptor and NUL-terminated component.
        let fd = check(unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        })?;
        // SAFETY: openat returned a new descriptor we own.
        let file = unsafe { File::from_raw_fd(fd) };
        let stat = fstat(&file)?;
        if !stat.is_regular() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name:?} is not a regular file"),
            ));
        }
        Ok((file, stat))
    }

    /// Hash of a regular file's current bytes together with its identity and mode.
    ///
    /// The descriptor is `fstat`ed before and after the read: a concurrent in-place
    /// write (even one that keeps the length), a `chmod` or a replaced inode makes the
    /// result an error instead of a hash that describes no single state of the file.
    pub fn hash_regular(&self, name: &str) -> io::Result<(EntryStat, String)> {
        self.hash_regular_observed(name, || {})
    }

    /// [`Self::hash_regular`] with a seam: `after_read` runs once all bytes were read and
    /// before the final `fstat`, so a test can interleave a concurrent writer exactly
    /// there.
    pub fn hash_regular_observed(
        &self,
        name: &str,
        after_read: impl FnOnce(),
    ) -> io::Result<(EntryStat, String)> {
        let (mut file, stat) = self.open_regular(name)?;
        let (hash, size) = hash_reader(&mut file)?;
        after_read();
        let after = fstat(&file)?;
        if size != after.size
            || after.id != stat.id
            || after.mode != stat.mode
            || after.modified_ns != stat.modified_ns
            || after.changed_ns != stat.changed_ns
        {
            return Err(io::Error::other("file changed while it was being read"));
        }
        Ok((EntryStat { size, ..stat }, hash))
    }

    /// Creates a new regular file (`O_EXCL`, no link following) with `mode` before any
    /// bytes are written. The mode is re-applied with `fchmod` so the umask never
    /// changes it.
    pub fn create_new(&self, name: &str, mode: u32) -> io::Result<File> {
        let cname = component(name)?;
        // SAFETY: valid directory descriptor and NUL-terminated component.
        let fd = check(unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                cname.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            )
        })?;
        // SAFETY: openat returned a new descriptor we own.
        let file = unsafe { File::from_raw_fd(fd) };
        // SAFETY: the descriptor is open.
        check(unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) })?;
        Ok(file)
    }

    pub fn mkdir(&self, name: &str, mode: u32) -> io::Result<()> {
        let name = component(name)?;
        // SAFETY: valid directory descriptor and NUL-terminated component.
        check(unsafe { libc::mkdirat(self.file.as_raw_fd(), name.as_ptr(), mode as libc::mode_t) })
            .map(drop)
    }

    /// Removes an empty directory.
    pub fn rmdir(&self, name: &str) -> io::Result<()> {
        let name = component(name)?;
        // SAFETY: valid directory descriptor and NUL-terminated component.
        check(unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) })
            .map(drop)
    }

    pub fn unlink(&self, name: &str) -> io::Result<()> {
        let name = component(name)?;
        // SAFETY: valid directory descriptor and NUL-terminated component.
        check(unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) }).map(drop)
    }

    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_all()
    }

    /// Moves `from` (in this directory) to `to` in `target` without replacing an
    /// existing `to`. An existing destination is `ErrorKind::AlreadyExists` and leaves
    /// both names untouched.
    ///
    /// `NoClobber::Link` must only be used when `from` is a private name no editor can
    /// touch: see the module documentation.
    pub fn rename_noreplace(
        &self,
        mechanism: NoClobber,
        from: &str,
        target: &Dir,
        to: &str,
    ) -> io::Result<()> {
        self.rename_noreplace_observed(mechanism, from, target, to, || {})
    }

    /// [`Self::rename_noreplace`] with a seam for the window between the two syscalls of
    /// the link-based mechanism: `between` runs after `linkat` and before `unlinkat`
    /// (both names exist). `renameat2` has no such window and never calls it.
    pub fn rename_noreplace_observed(
        &self,
        mechanism: NoClobber,
        from: &str,
        target: &Dir,
        to: &str,
        between: impl FnOnce(),
    ) -> io::Result<()> {
        let from_c = component(from)?;
        let to_c = component(to)?;
        match mechanism {
            NoClobber::Renameat2 => {
                // SAFETY: valid descriptors and NUL-terminated components; flag 1 is
                // RENAME_NOREPLACE.
                let result = unsafe {
                    libc::syscall(
                        libc::SYS_renameat2,
                        self.file.as_raw_fd(),
                        from_c.as_ptr(),
                        target.file.as_raw_fd(),
                        to_c.as_ptr(),
                        1 as libc::c_uint,
                    )
                };
                if result < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            }
            NoClobber::Link => {
                self.link_noreplace(from, target, to)?;
                between();
                // SAFETY: valid descriptor and NUL-terminated component.
                check(unsafe { libc::unlinkat(self.file.as_raw_fd(), from_c.as_ptr(), 0) })
                    .map(drop)
            }
        }
    }

    /// `linkat` without following a symlink source; an existing `to` is
    /// `ErrorKind::AlreadyExists`. The first half of the link-based move.
    pub fn link_noreplace(&self, from: &str, target: &Dir, to: &str) -> io::Result<()> {
        let from = component(from)?;
        let to = component(to)?;
        // SAFETY: valid descriptors and NUL-terminated components. Without
        // AT_SYMLINK_FOLLOW a symlink source is linked as itself.
        check(unsafe {
            libc::linkat(
                self.file.as_raw_fd(),
                from.as_ptr(),
                target.file.as_raw_fd(),
                to.as_ptr(),
                0,
            )
        })
        .map(drop)
    }
}

fn is_unsupported(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENOSYS | libc::EINVAL | libc::EOPNOTSUPP | libc::EPERM)
    )
}

/// Proves on the filesystem of `dir` that moving a name an editor controls never
/// replaces an existing name: a `renameat2(RENAME_NOREPLACE)` onto an existing file must
/// fail with `AlreadyExists` leaving both files byte-identical, and onto a free name it
/// must move the inode. Only `renameat2` qualifies: the link-based pair is two syscalls
/// and can unlink an editor's replacement (see the module documentation), so there is no
/// fallback; the caller keeps Apply blocked on `Err`. The probe files carry exact
/// app-generated names and are removed again.
pub fn probe_no_clobber(dir: &Dir) -> Result<NoClobber, String> {
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let names: Vec<String> = (0..3)
        .map(|i| format!("{}{tag}-{i}.stage", crate::paths::TX_PREFIX))
        .collect();
    let result = probe_with(dir, NoClobber::Renameat2, &names);
    for name in &names {
        let _ = dir.unlink(name);
    }
    let _ = dir.sync();
    match result {
        Ok(()) => Ok(NoClobber::Renameat2),
        Err(reason) => Err(format!(
            "this filesystem cannot move project files with an atomic no-replace rename ({}: {reason}); a link-then-unlink fallback could delete an editor's replacement saved between its two calls and is never used, so Apply stays blocked",
            NoClobber::Renameat2.label()
        )),
    }
}

/// Proves one specific mechanism on the filesystem of `dir` (see [`probe_no_clobber`]).
pub fn probe_mechanism(dir: &Dir, mechanism: NoClobber) -> Result<(), String> {
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let names: Vec<String> = (0..3)
        .map(|i| format!("{}{tag}-{i}.stage", crate::paths::TX_PREFIX))
        .collect();
    let result = probe_with(dir, mechanism, &names);
    for name in &names {
        let _ = dir.unlink(name);
    }
    let _ = dir.sync();
    result.map_err(|reason| format!("{}: {reason}", mechanism.label()))
}

fn probe_with(dir: &Dir, mechanism: NoClobber, names: &[String]) -> Result<(), String> {
    use std::io::Write;
    let describe = |step: &str, error: io::Error| format!("{step}: {error}");
    let write = |name: &str, bytes: &[u8]| -> Result<(), String> {
        let mut file = dir
            .create_new(name, 0o600)
            .map_err(|e| describe("create probe", e))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| describe("write probe", e))
    };
    write(&names[0], b"staged")?;
    write(&names[1], b"existing")?;
    let staged_id = dir
        .stat(&names[0])
        .map_err(|e| describe("stat probe", e))?
        .ok_or("probe file vanished")?
        .id;
    match dir.rename_noreplace(mechanism, &names[0], dir, &names[1]) {
        Ok(()) => return Err("an existing destination was replaced".into()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) if is_unsupported(&e) => return Err(format!("not supported ({e})")),
        Err(e) => return Err(describe("refusing to replace", e)),
    }
    let read = |name: &str| -> Result<Vec<u8>, String> {
        let (mut file, _) = dir.open_regular(name).map_err(|e| describe("reopen", e))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| describe("reread", e))?;
        Ok(bytes)
    };
    if read(&names[0])? != b"staged" || read(&names[1])? != b"existing" {
        return Err("a refused publication still changed a file".into());
    }
    dir.rename_noreplace(mechanism, &names[0], dir, &names[2])
        .map_err(|e| describe("publish onto a free name", e))?;
    let moved = dir
        .stat(&names[2])
        .map_err(|e| describe("stat published", e))?
        .ok_or("published file is missing")?;
    if dir
        .stat(&names[0])
        .map_err(|e| describe("stat source", e))?
        .is_some()
        || moved.id != staged_id
        || read(&names[2])? != b"staged"
    {
        return Err("publication onto a free name did not move the staged inode".into());
    }
    Ok(())
}
