//! Spike S3: no-clobber publication primitives for Windows and macOS.
//!
//! The same surface as the Linux `studio-project::publish` module, reduced to what the
//! five guarantees need:
//!
//! - every operation is relative to a held directory handle ([`Dir`]) and takes a single
//!   path component, so a path swap above the handle cannot redirect it;
//! - no operation follows a link (symlink, junction, any reparse point), and cloud-file
//!   placeholders are refused;
//! - moving a name uses one atomic no-replace rename: `renameatx_np(RENAME_EXCL)` on
//!   macOS, `NtSetInformationFile(FileRenameInformationEx)` with `RootDirectory` and no
//!   replace flag on Windows;
//! - [`Dir::open_root`] binds the root *pathname* by a no-follow walk from the volume
//!   root and [`Dir::check_root_binding`] re-walks it;
//! - a source that was replaced between verification and the move is detected
//!   ([`Dir::rename_noreplace_checked`]).
use std::{
    fmt,
    io::{self, Read},
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::Dir;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Dir, open_unbound};

/// A volume identifier plus a 128-bit file identifier.
///
/// Windows: `FILE_ID_INFORMATION` (`VolumeSerialNumber`, `FILE_ID_128`). macOS:
/// `(st_dev, st_ino)` widened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId {
    pub volume: u64,
    pub file: u128,
}

impl fmt::Display for FileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}:{:032x}", self.volume, self.file)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Regular,
    Directory,
    /// Symlink, or on Windows a junction (mount point) or symlink reparse point.
    Link,
    /// A cloud-files placeholder (OneDrive and other Cloud Files API providers).
    CloudPlaceholder,
    /// Anything else: FIFO, socket, device, or a reparse point of another kind.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryStat {
    pub id: FileId,
    pub kind: EntryKind,
    pub size: u64,
    pub nlink: u64,
    /// Last data write (`st_mtim` / `LastWriteTime`), nanoseconds since the platform epoch.
    pub modified_ns: i128,
    /// Last metadata change (`st_ctim` / `ChangeTime`), nanoseconds since the platform epoch.
    pub changed_ns: i128,
    /// The raw reparse tag on Windows (0 when the entry is no reparse point); 0 on macOS.
    pub reparse_tag: u32,
}

impl EntryStat {
    pub fn is_regular(&self) -> bool {
        self.kind == EntryKind::Regular
    }
}

/// Why an entry cannot be opened or moved by the publication layer, or `None` when its
/// kind is the one wanted.
pub(crate) fn refusal(name: &str, stat: &EntryStat, wanted: EntryKind) -> Option<io::Error> {
    if stat.kind == wanted {
        return None;
    }
    let what = match stat.kind {
        EntryKind::Link => "a link (symlink or junction); links are never followed".to_owned(),
        EntryKind::CloudPlaceholder => format!(
            "a cloud-files placeholder (reparse tag {:#010x}, e.g. OneDrive Files On-Demand); \
             project files are never changed inside a cloud-synced folder because the sync \
             provider can replace or dehydrate them underneath the transaction. Move the \
             project to a local folder",
            stat.reparse_tag
        ),
        EntryKind::Other if stat.reparse_tag != 0 => format!(
            "a reparse point of an unsupported kind (tag {:#010x})",
            stat.reparse_tag
        ),
        EntryKind::Other => "a special file".to_owned(),
        EntryKind::Regular => "a regular file".to_owned(),
        EntryKind::Directory => "a directory".to_owned(),
    };
    Some(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{name:?} is {what}"),
    ))
}

/// Whether a Windows reparse tag belongs to the Cloud Files API (`IO_REPARSE_TAG_CLOUD`
/// and `IO_REPARSE_TAG_CLOUD_1` .. `IO_REPARSE_TAG_CLOUD_F`, which differ only in bits
/// 12..15).
pub fn is_cloud_reparse_tag(tag: u32) -> bool {
    const IO_REPARSE_TAG_CLOUD: u32 = 0x9000_001A;
    const CLOUD_MASK: u32 = 0x0000_F000;
    tag & !CLOUD_MASK == IO_REPARSE_TAG_CLOUD
}

/// Classifies a Windows reparse point by its tag.
pub fn classify_reparse_tag(tag: u32) -> EntryKind {
    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
    if is_cloud_reparse_tag(tag) {
        EntryKind::CloudPlaceholder
    } else if tag == IO_REPARSE_TAG_SYMLINK || tag == IO_REPARSE_TAG_MOUNT_POINT {
        EntryKind::Link
    } else {
        EntryKind::Other
    }
}

/// A single path component: no separators, no `.`/`..`, no NUL. On Windows also no `:`
/// (alternate data streams), no characters Win32 cannot name, and no trailing dot or
/// space (NT would create a name Win32 tools cannot open).
pub(crate) fn check_component(name: &str) -> io::Result<()> {
    let invalid = name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\0'])
        || (cfg!(windows)
            && (name.contains(['\\', ':', '*', '?', '"', '<', '>', '|'])
                || name.chars().any(|c| (c as u32) < 0x20)
                || name.ends_with(['.', ' '])));
    if invalid {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} is not a single path component"),
        ))
    } else {
        Ok(())
    }
}

/// Proves on the filesystem of `dir` that a no-replace rename refuses an existing
/// destination (leaving both files intact) and moves the inode onto a free name. The
/// caller keeps mutation blocked on `Err` (FAT, network shares, anything without the
/// primitive). Probe files are removed again.
pub fn probe_no_clobber(dir: &Dir) -> Result<(), String> {
    let tag = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let names: Vec<String> = (0..3)
        .map(|i| format!(".fframes-probe-{tag}-{i}.stage"))
        .collect();
    let result = probe_with(dir, &names);
    for name in &names {
        let _ = dir.unlink(name);
    }
    result.map_err(|reason| {
        format!("this filesystem cannot move files with an atomic no-replace rename: {reason}")
    })
}

fn probe_with(dir: &Dir, names: &[String]) -> Result<(), String> {
    use std::io::Write;
    let describe = |step: &str, error: io::Error| format!("{step}: {error}");
    let write = |name: &str, bytes: &[u8]| -> Result<(), String> {
        let mut file = dir.create_new(name).map_err(|e| describe("create", e))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| describe("write", e))
    };
    let read = |name: &str| -> Result<Vec<u8>, String> {
        let (mut file, _) = dir.open_regular(name).map_err(|e| describe("reopen", e))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| describe("reread", e))?;
        Ok(bytes)
    };
    write(&names[0], b"staged")?;
    write(&names[1], b"existing")?;
    let staged = dir
        .stat(&names[0])
        .map_err(|e| describe("stat", e))?
        .ok_or("probe file vanished")?
        .id;
    match dir.rename_noreplace(&names[0], dir, &names[1]) {
        Ok(()) => return Err("an existing destination was replaced".into()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(describe("refusing to replace", e)),
    }
    if read(&names[0])? != b"staged" || read(&names[1])? != b"existing" {
        return Err("a refused rename still changed a file".into());
    }
    dir.rename_noreplace(&names[0], dir, &names[2])
        .map_err(|e| describe("rename onto a free name", e))?;
    let moved = dir
        .stat(&names[2])
        .map_err(|e| describe("stat moved", e))?
        .ok_or("moved file is missing")?;
    let source_left = dir
        .stat(&names[0])
        .map_err(|e| describe("stat source", e))?
        .is_some();
    if source_left || moved.id != staged || read(&names[2])? != b"staged" {
        return Err("a rename onto a free name did not move the file".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_tags_are_classified_as_placeholders_and_nothing_else_is() {
        // IO_REPARSE_TAG_CLOUD and CLOUD_1 .. CLOUD_F.
        for n in 0..=0xFu32 {
            let tag = 0x9000_001A | (n << 12);
            assert!(is_cloud_reparse_tag(tag), "{tag:#x}");
            assert_eq!(classify_reparse_tag(tag), EntryKind::CloudPlaceholder);
        }
        assert_eq!(classify_reparse_tag(0xA000_0003), EntryKind::Link); // mount point
        assert_eq!(classify_reparse_tag(0xA000_000C), EntryKind::Link); // symlink
        for other in [
            0x8000_0013u32, // dedup
            0x8000_0017,    // WOF (compressed system files)
            0x8000_001B,    // AppExecLink
            0x8000_0023,    // AF_UNIX socket
            0x9000_101B,    // near miss: not the cloud low word
            0x1000_001A,    // near miss: other high bits
            0,
        ] {
            assert!(!is_cloud_reparse_tag(other), "{other:#x}");
            assert_eq!(classify_reparse_tag(other), EntryKind::Other, "{other:#x}");
        }
    }

    #[test]
    fn components_must_be_single_plain_names() {
        for good in ["a", "a.txt", ".fframes-tx-1.stage", "con-tent"] {
            check_component(good).unwrap();
        }
        let mut bad = vec!["", ".", "..", "a/b", "a\0b"];
        if cfg!(windows) {
            bad.extend(["a\\b", "file:stream", "a.", "a ", "x*", "c:"]);
        }
        for name in bad {
            assert!(check_component(name).is_err(), "{name:?}");
        }
    }
}
