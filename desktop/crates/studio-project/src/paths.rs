use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::ProjectError;

/// UTF-8 slash-separated relative name with exactly one portable representation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProjectPath(String);
impl TryFrom<String> for ProjectPath {
    type Error = ProjectError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.len() > 4096
            || value.contains(['\\', ':', '\0', '<', '>', '"', '|', '?', '*'])
            || value.split('/').any(|c| {
                c.is_empty()
                    || c.len() > 255
                    || c.ends_with([' ', '.'])
                    || reserved_name(c)
                    || c == "."
                    || c == ".."
                    || c.chars().any(char::is_control)
            })
        {
            return Err(ProjectError::new(
                &value,
                "path",
                "invalid portable relative path",
                "Use a contained slash-separated relative path",
            ));
        }
        Ok(Self(value))
    }
}

fn reserved_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|n| {
                matches!(
                    n,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
}

impl From<ProjectPath> for String {
    fn from(value: ProjectPath) -> Self {
        value.0
    }
}
impl ProjectPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Validate existing components without following links, then verify canonical containment.
    pub fn resolve_existing(&self, root: &Path) -> Result<PathBuf, ProjectError> {
        let root = fs::canonicalize(root).map_err(|e| io_error(root, e))?;
        let mut path = root.clone();
        for component in self.0.split('/') {
            path.push(component);
            let meta = fs::symlink_metadata(&path).map_err(|e| io_error(&path, e))?;
            if is_link(&meta) {
                return Err(link_error(&path));
            }
        }
        let resolved = fs::canonicalize(&path).map_err(|e| io_error(&path, e))?;
        if !resolved.starts_with(root) {
            return Err(link_error(&path));
        }
        Ok(resolved)
    }
    /// Unix opens each component relative to a held directory descriptor with O_NOFOLLOW.
    /// This also guards replacements between enumeration and reading/copying.
    pub fn open_file(&self, root: &Path) -> Result<File, ProjectError> {
        self.resolve_existing(root)?;
        #[cfg(unix)]
        {
            use std::{
                ffi::CString,
                os::fd::{AsRawFd, FromRawFd},
            };
            let root = fs::canonicalize(root).map_err(|e| io_error(root, e))?;
            let mut parent = File::open(&root).map_err(|e| io_error(&root, e))?;
            let parts: Vec<_> = self.0.split('/').collect();
            for (i, part) in parts.iter().enumerate() {
                let name = CString::new(*part).expect("validated NUL-free path");
                let flags = libc::O_RDONLY
                    | libc::O_CLOEXEC
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | if i + 1 < parts.len() {
                        libc::O_DIRECTORY
                    } else {
                        0
                    };
                // SAFETY: the directory fd and NUL-terminated component live through openat.
                let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
                if fd < 0 {
                    let e = std::io::Error::last_os_error();
                    return Err(
                        if matches!(e.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR)) {
                            link_error(&root.join(&self.0))
                        } else {
                            io_error(&root.join(&self.0), e)
                        },
                    );
                }
                // SAFETY: openat returned a new owned descriptor.
                parent = unsafe { File::from_raw_fd(fd) };
            }
            if !parent.metadata().map_err(|e| io_error(&root, e))?.is_file() {
                return Err(io_error(&root.join(&self.0), "not a regular file"));
            }
            Ok(parent)
        }
        #[cfg(not(unix))]
        {
            let path = self.resolve_existing(root)?;
            let mut options = fs::OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.custom_flags(0x00200000);
            }
            let file = options.open(&path).map_err(|e| io_error(&path, e))?;
            let meta = file.metadata().map_err(|e| io_error(&path, e))?;
            if is_link(&meta) {
                return Err(link_error(&path));
            }
            self.resolve_existing(root)?;
            if !meta.is_file() {
                return Err(io_error(&path, "not a regular file"));
            }
            Ok(file)
        }
    }
}
pub(crate) fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.is_symlink()
    }
}
pub(crate) fn io_error(path: &Path, error: impl ToString) -> ProjectError {
    ProjectError::new(
        path,
        "path",
        error,
        "Check file availability and permissions",
    )
}
pub(crate) fn link_error(path: &Path) -> ProjectError {
    ProjectError::new(
        path,
        "path",
        "unsupported link",
        "Copy the linked content into the project as ordinary files",
    )
}

/// Prefix of every file the edit-transaction layer creates inside the project tree.
pub const TX_PREFIX: &str = ".fframes-tx-";

/// The purpose of one app-generated transaction file. Each belongs to exactly one
/// transaction (`tx`, 32 lowercase hex digits) and one operation index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxRole {
    /// The after bytes, staged next to their destination.
    Stage,
    /// The displaced original.
    Original,
    /// Our own published file, moved aside while rolling back.
    Rollback,
    /// A foreign variant retained after an outside writer interfered.
    Variant(u32),
}
impl TxRole {
    fn suffix(self) -> String {
        match self {
            Self::Stage => "stage".into(),
            Self::Original => "orig".into(),
            Self::Rollback => "undo".into(),
            Self::Variant(n) => format!("var{n}"),
        }
    }
}

/// The exact name of a transaction file: `.fframes-tx-<tx>-<index>.<role>`.
pub fn transaction_file_name(tx: &str, index: usize, role: TxRole) -> String {
    debug_assert!(is_transaction_id(tx));
    format!("{TX_PREFIX}{tx}-{index}.{}", role.suffix())
}

/// A transaction id as embedded in file names: exactly 32 lowercase hex digits.
pub fn is_transaction_id(tx: &str) -> bool {
    tx.len() == 32
        && tx
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `name` (one path component) is exactly a name the transaction layer
/// generates. Only these names are excluded from the source inventory (and only when
/// they are regular files); any other file, including one that merely starts with the
/// prefix, is user content.
pub fn is_transaction_internal_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(TX_PREFIX) else {
        return false;
    };
    let Some((tx, tail)) = rest.split_at_checked(32) else {
        return false;
    };
    let Some(tail) = tail.strip_prefix('-') else {
        return false;
    };
    let Some((index, role)) = tail.split_once('.') else {
        return false;
    };
    let digits = |s: &str| {
        !s.is_empty()
            && s.len() <= 9
            && s.bytes().all(|b| b.is_ascii_digit())
            && (s == "0" || !s.starts_with('0'))
    };
    is_transaction_id(tx)
        && digits(index)
        && match role {
            "stage" | "orig" | "undo" => true,
            _ => role.strip_prefix("var").is_some_and(digits),
        }
}
