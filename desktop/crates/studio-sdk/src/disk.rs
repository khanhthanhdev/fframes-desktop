use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiskSpaceError {
    #[error("could not determine free disk space for {path}: {reason}")]
    Query { path: String, reason: String },
    #[error("not enough disk space at {path}: need {required} bytes, have {available} bytes")]
    Insufficient {
        path: String,
        required: u64,
        available: u64,
    },
}

pub fn available_bytes(path: &Path) -> Result<u64, DiskSpaceError> {
    let path = path.canonicalize().map_err(|error| DiskSpaceError::Query {
        path: path.display().to_string(),
        reason: error.to_string(),
    })?;

    #[cfg(unix)]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let path_c =
            CString::new(path.as_os_str().as_bytes()).map_err(|error| DiskSpaceError::Query {
                path: path.display().to_string(),
                reason: error.to_string(),
            })?;
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `path_c` is NUL-terminated and `stats` points to writable storage.
        let result = unsafe { libc::statvfs(path_c.as_ptr(), stats.as_mut_ptr()) };
        if result != 0 {
            return Err(DiskSpaceError::Query {
                path: path.display().to_string(),
                reason: std::io::Error::last_os_error().to_string(),
            });
        }
        // SAFETY: statvfs initialized the structure on success.
        let stats = unsafe { stats.assume_init() };
        let available = u128::from(stats.f_bavail).saturating_mul(u128::from(stats.f_frsize));
        Ok(u64::try_from(available).unwrap_or(u64::MAX))
    }

    #[cfg(windows)]
    {
        use std::{ffi::OsStr, os::windows::ffi::OsStrExt};
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let mut available = 0u64;
        let path_w: Vec<u16> = OsStr::new(&path).encode_wide().chain([0]).collect();
        // SAFETY: `path_w` is NUL-terminated and the out pointer is valid.
        let result = unsafe {
            GetDiskFreeSpaceExW(
                path_w.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if result == 0 {
            return Err(DiskSpaceError::Query {
                path: path.display().to_string(),
                reason: std::io::Error::last_os_error().to_string(),
            });
        }
        Ok(available)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(DiskSpaceError::Query {
            path: path.display().to_string(),
            reason: "disk-space queries are unsupported on this platform".into(),
        })
    }
}

pub fn ensure_available(path: &Path, required: u64) -> Result<(), DiskSpaceError> {
    let available = available_bytes(path)?;
    if available < required {
        return Err(DiskSpaceError::Insufficient {
            path: path.display().to_string(),
            required,
            available,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_space_is_observed_from_an_existing_directory() {
        let directory = tempfile::tempdir().unwrap();
        assert!(available_bytes(directory.path()).unwrap() > 0);
        ensure_available(directory.path(), 1).unwrap();
    }

    #[test]
    fn required_space_check_reports_a_shortage_without_mutating_storage() {
        let directory = tempfile::tempdir().unwrap();
        let error = ensure_available(directory.path(), u64::MAX).unwrap_err();
        assert!(matches!(error, DiskSpaceError::Insufficient { .. }));
    }
}
