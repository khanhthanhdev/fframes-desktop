//! Windows: NT handle-relative primitives.
//!
//! Every open is `NtCreateFile` with `OBJECT_ATTRIBUTES.RootDirectory` set to a held
//! directory handle, a single-component name, `OBJ_CASE_INSENSITIVE` (Win32 name
//! semantics) and `FILE_OPEN_REPARSE_POINT`, followed by a `FileAttributeTagInformation`
//! query: `FILE_OPEN_REPARSE_POINT` only opens the reparse point itself instead of its
//! target, so refusing links is that check, not the flag. Renames are
//! `NtSetInformationFile(FileRenameInformationEx)` on a handle to the source with
//! `RootDirectory` set to the target directory and no `FILE_RENAME_REPLACE_IF_EXISTS`.
//! No path-based Win32 call (`MoveFileExW`, `CreateFileW`, `DeleteFileW`) touches a live
//! name.
use std::{
    ffi::c_void,
    fs::File,
    io,
    mem::{MaybeUninit, offset_of, size_of},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Component, Path, PathBuf, Prefix},
    ptr::null_mut,
    sync::Arc,
};

use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FILE_BASIC_INFORMATION, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_ID_INFORMATION,
            FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION,
            FILE_STANDARD_INFORMATION, FILE_SYNCHRONOUS_IO_NONALERT, FileAttributeTagInformation,
            FileBasicInformation, FileDispositionInformationEx, FileIdInformation,
            FileRenameInformationEx, FileStandardInformation, NtCreateFile, NtQueryInformationFile,
            NtSetInformationFile, RtlSetProcessPlaceholderCompatibilityMode,
        },
    },
    Win32::{
        Foundation::{
            HANDLE, NTSTATUS, OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, UNICODE_STRING,
        },
        Storage::FileSystem::{
            DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_NORMAL,
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_FLAG_DELETE,
            FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, FILE_TRAVERSE, SYNCHRONIZE,
        },
        System::IO::IO_STATUS_BLOCK,
    },
};

use crate::{EntryKind, EntryStat, FileId, check_component, classify_reparse_tag, refusal};

const SHARE_ALL: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
const DIR_ACCESS: u32 = FILE_LIST_DIRECTORY
    | FILE_TRAVERSE
    | FILE_READ_ATTRIBUTES
    | FILE_ADD_FILE
    | FILE_ADD_SUBDIRECTORY;
/// `PHCM_EXPOSE_PLACEHOLDERS`: cloud placeholders report their real reparse tag to this
/// process instead of looking like ordinary files.
const PHCM_EXPOSE_PLACEHOLDERS: i8 = 2;

fn nt_error(status: NTSTATUS) -> io::Error {
    // SAFETY: pure conversion.
    let code = unsafe { RtlNtStatusToDosError(status) };
    io::Error::from_raw_os_error(code as i32)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// `NtCreateFile` of `name` relative to `root` (or absolute when `root` is `None`),
/// never following a reparse point. Always synchronous.
fn nt_open(
    root: Option<&OwnedHandle>,
    name: &[u16],
    access: u32,
    share: u32,
    disposition: u32,
    options: u32,
) -> io::Result<OwnedHandle> {
    let bytes = u16::try_from(name.len() * 2)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name too long"))?;
    let object_name = UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes,
        Buffer: name.as_ptr().cast_mut(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: root.map_or(null_mut(), |h| h.as_raw_handle() as HANDLE),
        ObjectName: &object_name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut handle: HANDLE = null_mut();
    // SAFETY: zeroed IO_STATUS_BLOCK is a valid out-parameter.
    let mut iosb: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    // SAFETY: all pointers are valid for the call; `name` outlives it.
    let status = unsafe {
        NtCreateFile(
            &mut handle,
            access | SYNCHRONIZE,
            &attributes,
            &mut iosb,
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            share,
            disposition,
            options | FILE_OPEN_REPARSE_POINT | FILE_SYNCHRONOUS_IO_NONALERT,
            std::ptr::null(),
            0,
        )
    };
    if status < 0 {
        return Err(nt_error(status));
    }
    // SAFETY: NtCreateFile succeeded and returned a handle we own.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as _) })
}

fn query<T>(handle: &OwnedHandle, class: i32) -> io::Result<T> {
    let mut value = MaybeUninit::<T>::zeroed();
    // SAFETY: zeroed IO_STATUS_BLOCK is a valid out-parameter.
    let mut iosb: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    // SAFETY: `value` is a writable buffer of exactly the size passed.
    let status = unsafe {
        NtQueryInformationFile(
            handle.as_raw_handle() as HANDLE,
            &mut iosb,
            value.as_mut_ptr().cast::<c_void>(),
            size_of::<T>() as u32,
            class,
        )
    };
    if status < 0 {
        return Err(nt_error(status));
    }
    // SAFETY: the kernel filled the structure (and zeroed is valid for these POD types).
    Ok(unsafe { value.assume_init() })
}

fn set_information(handle: &OwnedHandle, buffer: &[u8], class: i32) -> io::Result<()> {
    // SAFETY: zeroed IO_STATUS_BLOCK is a valid out-parameter.
    let mut iosb: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
    // SAFETY: `buffer` is readable for its length and laid out for `class`.
    let status = unsafe {
        NtSetInformationFile(
            handle.as_raw_handle() as HANDLE,
            &mut iosb,
            buffer.as_ptr().cast::<c_void>(),
            buffer.len() as u32,
            class,
        )
    };
    if status < 0 {
        Err(nt_error(status))
    } else {
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AttributeTagInformation {
    file_attributes: u32,
    reparse_tag: u32,
}

/// Identity, kind, size, link count and times of an open handle.
pub(crate) fn stat_handle(handle: &OwnedHandle) -> io::Result<EntryStat> {
    let id: FILE_ID_INFORMATION = query(handle, FileIdInformation)?;
    let tag: AttributeTagInformation = query(handle, FileAttributeTagInformation)?;
    let standard: FILE_STANDARD_INFORMATION = query(handle, FileStandardInformation)?;
    let basic: FILE_BASIC_INFORMATION = query(handle, FileBasicInformation)?;
    let reparse = tag.file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    let recall = tag.file_attributes
        & (FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0;
    let kind = if reparse {
        classify_reparse_tag(tag.reparse_tag)
    } else if recall {
        EntryKind::CloudPlaceholder
    } else if standard.Directory {
        EntryKind::Directory
    } else {
        EntryKind::Regular
    };
    Ok(EntryStat {
        id: FileId {
            volume: id.VolumeSerialNumber,
            file: u128::from_le_bytes(id.FileId.Identifier),
        },
        kind,
        size: standard.EndOfFile as u64,
        nlink: u64::from(standard.NumberOfLinks),
        modified_ns: i128::from(basic.LastWriteTime) * 100,
        changed_ns: i128::from(basic.ChangeTime) * 100,
        reparse_tag: if reparse { tag.reparse_tag } else { 0 },
    })
}

/// The identity chain of a root pathname: the id of every directory from the volume
/// root down to the root, found by a no-reparse walk.
#[derive(Debug)]
struct RootFence {
    path: PathBuf,
    chain: Vec<FileId>,
}

fn open_dir_handle(
    root: Option<&OwnedHandle>,
    name: &[u16],
    shown: &str,
) -> io::Result<(OwnedHandle, FileId)> {
    let handle = nt_open(
        root,
        name,
        DIR_ACCESS,
        SHARE_ALL,
        FILE_OPEN,
        FILE_DIRECTORY_FILE,
    )?;
    let stat = stat_handle(&handle)?;
    if let Some(error) = refusal(shown, &stat, EntryKind::Directory) {
        return Err(error);
    }
    Ok((handle, stat.id))
}

/// Walks a canonical `\\?\X:\...` path from the volume root without following any
/// reparse point. UNC paths are not supported by the prototype.
fn walk_no_reparse(path: &Path) -> io::Result<(OwnedHandle, Vec<FileId>)> {
    let mut components = path.components();
    let drive = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimDisk(letter) | Prefix::Disk(letter) => letter as char,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("{} is not on a local drive letter", path.display()),
                ));
            }
        },
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not an absolute path", path.display()),
            ));
        }
    };
    let volume_root = format!("\\??\\{drive}:\\");
    let (mut current, id) = open_dir_handle(None, &wide(&volume_root), &volume_root)?;
    let mut chain = vec![id];
    for part in components {
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
        check_component(name)?;
        let (next, id) = open_dir_handle(Some(&current), &wide(name), name)?;
        current = next;
        chain.push(id);
    }
    Ok((current, chain))
}

/// Experiment hook only: opens a directory by absolute NT path (for example
/// `\??\UNC\localhost\C$\Users`) with no root binding and without refusing a reparse
/// point on that directory itself (so a cloud sync root can be entered to test the
/// per-file refusal). Every name below it is still checked.
pub fn open_unbound(nt_path: &str) -> io::Result<Dir> {
    let handle = nt_open(
        None,
        &wide(nt_path),
        DIR_ACCESS,
        SHARE_ALL,
        FILE_OPEN,
        FILE_DIRECTORY_FILE,
    )?;
    let id = stat_handle(&handle)?.id;
    Ok(Dir {
        handle,
        id,
        fence: None,
    })
}

/// A directory held open by handle. All names are single components resolved relative
/// to it, and every open refuses reparse points.
#[derive(Debug)]
pub struct Dir {
    handle: OwnedHandle,
    id: FileId,
    fence: Option<Arc<RootFence>>,
}

impl Dir {
    /// Opens `path` as the root handle: canonicalized once, then walked from the volume
    /// root one no-reparse component at a time, recording every directory's id.
    ///
    /// Also switches this process to `PHCM_EXPOSE_PLACEHOLDERS`, so cloud placeholders
    /// show their reparse tag and are refused instead of looking like plain files.
    pub fn open_root(path: &Path) -> io::Result<Self> {
        // SAFETY: plain process-wide setting.
        unsafe { RtlSetProcessPlaceholderCompatibilityMode(PHCM_EXPOSE_PLACEHOLDERS) };
        let path = std::fs::canonicalize(path)?;
        let (handle, chain) = walk_no_reparse(&path)?;
        let id = *chain.last().expect("the chain has the volume root");
        Ok(Self {
            handle,
            id,
            fence: Some(Arc::new(RootFence { path, chain })),
        })
    }

    /// Whether the pathname this root was opened by still resolves, without following
    /// any reparse point, to the same directory through the same ancestors.
    pub fn check_root_binding(&self) -> Result<(), String> {
        let Some(fence) = &self.fence else {
            return Ok(());
        };
        match walk_no_reparse(&fence.path) {
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
            handle: self.handle.try_clone()?,
            id: self.id,
            fence: self.fence.clone(),
        })
    }

    pub fn open_child(&self, name: &str) -> io::Result<Self> {
        check_component(name)?;
        let (handle, id) = open_dir_handle(Some(&self.handle), &wide(name), name)?;
        Ok(Self {
            handle,
            id,
            fence: None,
        })
    }

    /// Descends `relative` (slash separated, empty = this directory).
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

    /// Metadata of a name without following it; `None` if it does not exist.
    pub fn stat(&self, name: &str) -> io::Result<Option<EntryStat>> {
        check_component(name)?;
        match nt_open(
            Some(&self.handle),
            &wide(name),
            FILE_READ_ATTRIBUTES,
            SHARE_ALL,
            FILE_OPEN,
            0,
        ) {
            Ok(handle) => stat_handle(&handle).map(Some),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Opens a regular file read-only; links, placeholders and directories are refused.
    pub fn open_regular(&self, name: &str) -> io::Result<(File, EntryStat)> {
        check_component(name)?;
        let handle = nt_open(
            Some(&self.handle),
            &wide(name),
            FILE_GENERIC_READ,
            SHARE_ALL,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE,
        )?;
        let stat = stat_handle(&handle)?;
        if let Some(error) = refusal(name, &stat, EntryKind::Regular) {
            return Err(error);
        }
        Ok((File::from(handle), stat))
    }

    /// Creates a new regular file; any existing entry (a dangling link included) is
    /// `AlreadyExists`. NTFS reports `FILE_CREATE` onto an existing directory or
    /// directory link as access denied, so a failed create onto a name that exists is
    /// reported as `AlreadyExists` too (the create itself changed nothing either way).
    pub fn create_new(&self, name: &str) -> io::Result<File> {
        check_component(name)?;
        match nt_open(
            Some(&self.handle),
            &wide(name),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            SHARE_ALL,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE,
        ) {
            Ok(handle) => Ok(File::from(handle)),
            Err(e) if e.kind() != io::ErrorKind::AlreadyExists && self.stat(name)?.is_some() => {
                Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("{name:?} exists ({e})"),
                ))
            }
            Err(e) => Err(e),
        }
    }

    pub fn mkdir(&self, name: &str) -> io::Result<()> {
        check_component(name)?;
        nt_open(
            Some(&self.handle),
            &wide(name),
            FILE_LIST_DIRECTORY,
            SHARE_ALL,
            FILE_CREATE,
            FILE_DIRECTORY_FILE,
        )
        .map(drop)
    }

    /// Removes a name (a link is removed as itself) with POSIX semantics: the name is
    /// gone when the call returns even if another handle is still open.
    pub fn unlink(&self, name: &str) -> io::Result<()> {
        check_component(name)?;
        let handle = nt_open(
            Some(&self.handle),
            &wide(name),
            DELETE,
            SHARE_ALL,
            FILE_OPEN,
            0,
        )?;
        let flags = FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS;
        set_information(&handle, &flags.to_ne_bytes(), FileDispositionInformationEx)
    }

    /// Moves `from` (in this directory) to `to` in `target` without replacing an existing
    /// `to` (`AlreadyExists`, both names untouched).
    pub fn rename_noreplace(&self, from: &str, target: &Dir, to: &str) -> io::Result<()> {
        self.rename_inner(from, target, to, None, || {})
    }

    /// [`Self::rename_noreplace`] that moves `from` only while it is still the file
    /// `expected` (the one the caller verified). The check runs on the very handle that
    /// is renamed, which is opened without `FILE_SHARE_DELETE`: between the check and the
    /// rename nobody can rename, replace or delete the source name.
    pub fn rename_noreplace_checked(
        &self,
        from: &str,
        target: &Dir,
        to: &str,
        expected: FileId,
    ) -> io::Result<()> {
        self.rename_inner(from, target, to, Some(expected), || {})
    }

    /// [`Self::rename_noreplace`] with a seam: `between` runs after the source handle was
    /// opened and before the rename (the window the two-call NT sequence has).
    pub fn rename_noreplace_observed(
        &self,
        from: &str,
        target: &Dir,
        to: &str,
        between: impl FnOnce(),
    ) -> io::Result<()> {
        self.rename_inner(from, target, to, None, between)
    }

    fn rename_inner(
        &self,
        from: &str,
        target: &Dir,
        to: &str,
        expected: Option<FileId>,
        between: impl FnOnce(),
    ) -> io::Result<()> {
        check_component(from)?;
        check_component(to)?;
        // No FILE_SHARE_DELETE: while this handle is open the source name cannot be
        // renamed, replaced (rename-over) or deleted by anyone else. Writers already
        // holding the file without FILE_SHARE_DELETE make this open fail with a sharing
        // violation instead.
        let source = nt_open(
            Some(&self.handle),
            &wide(from),
            DELETE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            0,
        )?;
        let stat = stat_handle(&source)?;
        if matches!(stat.kind, EntryKind::CloudPlaceholder | EntryKind::Other) {
            return Err(refusal(from, &stat, EntryKind::Regular).expect("refused kind"));
        }
        if let Some(expected) = expected
            && stat.id != expected
        {
            return Err(io::Error::other(format!(
                "{from:?} was replaced: it is {} now, not the verified {expected}",
                stat.id
            )));
        }
        between();
        let name = wide(to);
        let name_offset = offset_of!(FILE_RENAME_INFORMATION, FileName);
        let length = name_offset + name.len() * 2;
        // u64 storage keeps the structure 8-byte aligned.
        let mut storage = vec![0u64; length.div_ceil(8)];
        // SAFETY: `storage` holds at least `length` bytes, aligned for the structure.
        unsafe {
            let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
            // Flags = 0: no FILE_RENAME_REPLACE_IF_EXISTS.
            (*info).Anonymous.Flags = 0;
            (*info).RootDirectory = target.handle.as_raw_handle() as HANDLE;
            (*info).FileNameLength = (name.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                storage
                    .as_mut_ptr()
                    .cast::<u8>()
                    .add(name_offset)
                    .cast::<u16>(),
                name.len(),
            );
        }
        // SAFETY: reinterpreting initialized u64 storage as bytes.
        let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), length) };
        set_information(&source, bytes, FileRenameInformationEx)
    }
}
