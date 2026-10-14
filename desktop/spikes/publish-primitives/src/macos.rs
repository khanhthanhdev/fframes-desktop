//! macOS: descriptor-relative primitives (`openat(O_NOFOLLOW)`, `fstatat`,
//! `renameatx_np(RENAME_EXCL)`).
//!
//! The rename moves whatever is at the source *name* in one syscall (like Linux
//! `renameat2`); a source replaced just before it is detected afterwards by the moved
//! file's identity and moved back ([`Dir::rename_noreplace_checked`]).
use std::{
    ffi::CString,
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use crate::{EntryKind, EntryStat, FileId, check_component, refusal};

fn cstr(name: &str) -> io::Result<CString> {
    check_component(name)?;
    CString::new(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

fn check(result: libc::c_int) -> io::Result<libc::c_int> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

#[allow(clippy::unnecessary_cast)]
fn stat_of(raw: &libc::stat) -> EntryStat {
    let kind = match raw.st_mode & libc::S_IFMT {
        libc::S_IFREG => EntryKind::Regular,
        libc::S_IFDIR => EntryKind::Directory,
        libc::S_IFLNK => EntryKind::Link,
        _ => EntryKind::Other,
    };
    EntryStat {
        id: FileId {
            volume: raw.st_dev as u64,
            file: raw.st_ino as u128,
        },
        kind,
        size: raw.st_size as u64,
        nlink: raw.st_nlink as u64,
        modified_ns: i128::from(raw.st_mtime) * 1_000_000_000 + i128::from(raw.st_mtime_nsec),
        changed_ns: i128::from(raw.st_ctime) * 1_000_000_000 + i128::from(raw.st_ctime_nsec),
        reparse_tag: 0,
    }
}

fn fstat(file: &File) -> io::Result<EntryStat> {
    // SAFETY: zeroed `stat` is a valid out-parameter and the descriptor is open.
    let mut raw: libc::stat = unsafe { std::mem::zeroed() };
    check(unsafe { libc::fstat(file.as_raw_fd(), &mut raw) })?;
    Ok(stat_of(&raw))
}

fn openat(dir: &File, name: &CString, flags: libc::c_int) -> io::Result<File> {
    // SAFETY: valid directory descriptor and NUL-terminated component.
    let fd = check(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600 as libc::c_uint,
        )
    })?;
    // SAFETY: openat returned a new descriptor we own.
    Ok(unsafe { File::from_raw_fd(fd) })
}

#[derive(Debug)]
struct RootFence {
    path: PathBuf,
    chain: Vec<FileId>,
}

fn walk_no_follow(path: &Path) -> io::Result<(File, Vec<FileId>)> {
    let root = CString::new("/").expect("no NUL");
    // SAFETY: absolute path, NUL-terminated.
    let fd = check(unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })?;
    // SAFETY: open returned a new descriptor we own.
    let mut current = unsafe { File::from_raw_fd(fd) };
    let mut chain = vec![fstat(&current)?.id];
    for part in path.components() {
        let name = match part {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "path is not valid Unicode")
            })?,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a canonical absolute path", path.display()),
                ));
            }
        };
        current = openat(&current, &cstr(name)?, libc::O_RDONLY | libc::O_DIRECTORY)?;
        chain.push(fstat(&current)?.id);
    }
    Ok((current, chain))
}

/// A directory held open by descriptor; every name is a single component resolved
/// relative to it, and no open follows a symlink.
#[derive(Debug)]
pub struct Dir {
    file: File,
    id: FileId,
    fence: Option<Arc<RootFence>>,
}

impl Dir {
    /// Opens `path` as the root: canonicalized once (macOS temp dirs live under the
    /// `/var` -> `/private/var` link), then walked from `/` without following links.
    pub fn open_root(path: &Path) -> io::Result<Self> {
        let path = std::fs::canonicalize(path)?;
        let (file, chain) = walk_no_follow(&path)?;
        let id = *chain.last().expect("the chain has /");
        Ok(Self {
            file,
            id,
            fence: Some(Arc::new(RootFence { path, chain })),
        })
    }

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

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            id: self.id,
            fence: self.fence.clone(),
        })
    }

    pub fn open_child(&self, name: &str) -> io::Result<Self> {
        let file = openat(&self.file, &cstr(name)?, libc::O_RDONLY | libc::O_DIRECTORY)?;
        let id = fstat(&file)?.id;
        Ok(Self {
            file,
            id,
            fence: None,
        })
    }

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

    pub fn id(&self) -> FileId {
        self.id
    }

    pub fn is_still_at(&self, root: &Dir, relative: &str) -> bool {
        root.open_relative(relative)
            .is_ok_and(|current| current.id == self.id)
    }

    pub fn stat(&self, name: &str) -> io::Result<Option<EntryStat>> {
        let name = cstr(name)?;
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

    pub fn open_regular(&self, name: &str) -> io::Result<(File, EntryStat)> {
        let file = openat(&self.file, &cstr(name)?, libc::O_RDONLY | libc::O_NONBLOCK)?;
        let stat = fstat(&file)?;
        if let Some(error) = refusal(name, &stat, EntryKind::Regular) {
            return Err(error);
        }
        Ok((file, stat))
    }

    pub fn create_new(&self, name: &str) -> io::Result<File> {
        openat(
            &self.file,
            &cstr(name)?,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )
    }

    pub fn mkdir(&self, name: &str) -> io::Result<()> {
        let name = cstr(name)?;
        // SAFETY: valid directory descriptor and NUL-terminated component.
        check(unsafe { libc::mkdirat(self.file.as_raw_fd(), name.as_ptr(), 0o755) }).map(drop)
    }

    pub fn unlink(&self, name: &str) -> io::Result<()> {
        let name = cstr(name)?;
        // SAFETY: valid directory descriptor and NUL-terminated component.
        check(unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) }).map(drop)
    }

    /// `renameatx_np(RENAME_EXCL)`: one atomic syscall; an existing `to` is
    /// `AlreadyExists` (`EEXIST`) and leaves both names untouched.
    pub fn rename_noreplace(&self, from: &str, target: &Dir, to: &str) -> io::Result<()> {
        let from = cstr(from)?;
        let to = cstr(to)?;
        // SAFETY: valid descriptors and NUL-terminated components.
        check(unsafe {
            libc::renameatx_np(
                self.file.as_raw_fd(),
                from.as_ptr(),
                target.file.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        })
        .map(drop)
    }

    /// Moves `from` only if the moved file is `expected`. The rename is by name, so a
    /// replaced source is detected after the move by the identity of `to`, and moved
    /// back no-replace; if the source name was taken again meanwhile, the replacement
    /// stays at `to` and the error says so (never lost, never overwritten).
    pub fn rename_noreplace_checked(
        &self,
        from: &str,
        target: &Dir,
        to: &str,
        expected: FileId,
    ) -> io::Result<()> {
        self.rename_noreplace(from, target, to)?;
        let moved = target
            .stat(to)?
            .ok_or_else(|| io::Error::other(format!("{to:?} vanished right after the rename")))?;
        if moved.id == expected {
            return Ok(());
        }
        let back = target.rename_noreplace(to, self, from);
        Err(io::Error::other(format!(
            "{from:?} was replaced: it was {}, not the verified {expected}; {}",
            moved.id,
            match back {
                Ok(()) => "it was moved back".to_owned(),
                Err(e) => format!("it is kept at {to:?} ({e})"),
            }
        )))
    }

    /// [`Self::rename_noreplace`]; the single syscall has no window, so `between` is
    /// never called.
    pub fn rename_noreplace_observed(
        &self,
        from: &str,
        target: &Dir,
        to: &str,
        _between: impl FnOnce(),
    ) -> io::Result<()> {
        self.rename_noreplace(from, target, to)
    }
}
