//! The deck's folder: built in a staging folder `.fframes-deck-<uuid>` next to the
//! destination and published with one atomic no-replace rename
//! ([`studio_project::publish`]), so an existing folder, even an empty one, is never
//! replaced. The staging folder names its process in `owner.json`; a later deck export
//! sweeps staging folders whose process is gone (a crash).
use std::{
    fs, io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime},
};

use serde::{Deserialize, Serialize};
use studio_project::publish::{Dir, NoClobber, probe_no_clobber};

/// Staging folders start with this.
pub const STAGING_PREFIX: &str = ".fframes-deck-";
pub const OWNER_FILE: &str = "owner.json";
/// How long a refused rename is retried: Windows refuses to rename a folder while a
/// scanner (Windows Defender) holds a file in it open, usually for well under a second.
pub const RENAME_PATIENCE: Duration = Duration::from_secs(10);
/// A staging folder without a readable owner file is swept once it is this old (a
/// crash between creating it and writing the file).
const UNOWNED_AGE: Duration = Duration::from_secs(10 * 60);

/// The process that builds a staging folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub pid: u32,
    /// When the process started, in a platform unit; `None` where it cannot be read.
    /// Tells a reused pid from the owner.
    pub started: Option<u64>,
}

impl Owner {
    pub fn current() -> Self {
        let pid = std::process::id();
        Self {
            pid,
            started: process_start(pid),
        }
    }

    /// Whether the owner still runs. Unsure answers are "yes": a folder is only swept
    /// when its owner is known to be gone.
    pub fn is_alive(&self) -> bool {
        if !process_exists(self.pid) {
            return false;
        }
        match (self.started, process_start(self.pid)) {
            (Some(recorded), Some(now)) => recorded == now,
            _ => true,
        }
    }
}

/// Where a deck is published: an existing parent folder and a new folder name in it.
#[derive(Debug)]
pub struct Destination {
    parent: PathBuf,
    name: String,
}

impl Destination {
    /// Checks that `path` names a folder that does not exist yet in an existing folder
    /// where an atomic no-replace rename works.
    pub fn prepare(path: &Path) -> Result<Self, String> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.starts_with(STAGING_PREFIX))
            .ok_or_else(|| "choose a folder name for the slide deck".to_owned())?
            .to_owned();
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()
            .map_err(|error| format!("the deck's parent folder is unavailable: {error}"))?;
        let destination = Self { parent, name };
        if fs::symlink_metadata(destination.path()).is_ok() {
            return Err(format!(
                "{} already exists; choose a new folder name",
                destination.path().display()
            ));
        }
        let dir = Dir::open_root(&destination.parent)
            .map_err(|error| format!("the deck's parent folder cannot be opened: {error}"))?;
        probe_no_clobber(&dir).map_err(|reason| {
            format!("slide decks cannot be exported into this folder: {reason}")
        })?;
        Ok(destination)
    }

    pub fn parent(&self) -> &Path {
        &self.parent
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> PathBuf {
        self.parent.join(&self.name)
    }
}

/// A staging folder; removed when dropped unless it was published.
#[derive(Debug)]
pub struct Staging {
    parent: PathBuf,
    name: String,
    published: bool,
}

impl Staging {
    /// Creates `.fframes-deck-<uuid>` in `parent` with this process's `owner.json`.
    pub fn create(parent: &Path) -> Result<Self, String> {
        let name = format!("{STAGING_PREFIX}{}", uuid::Uuid::new_v4().simple());
        let path = parent.join(&name);
        fs::create_dir(&path)
            .map_err(|error| format!("could not create the deck's staging folder: {error}"))?;
        let staging = Self {
            parent: parent.to_owned(),
            name,
            published: false,
        };
        let owner = serde_json::to_vec(&Owner::current()).expect("an owner serializes");
        fs::write(path.join(OWNER_FILE), owner)
            .map_err(|error| format!("could not write the deck's owner file: {error}"))?;
        Ok(staging)
    }

    pub fn path(&self) -> PathBuf {
        self.parent.join(&self.name)
    }

    /// Renames the staging folder to `destination` without replacing anything there,
    /// retrying refusals of a busy folder for [`RENAME_PATIENCE`]. The owner file is
    /// removed first. An existing destination is `AlreadyExists` and stays untouched.
    pub fn publish(&mut self, destination: &Destination) -> io::Result<()> {
        self.publish_with_patience(destination, RENAME_PATIENCE)
    }

    fn publish_with_patience(
        &mut self,
        destination: &Destination,
        patience: Duration,
    ) -> io::Result<()> {
        let source = Dir::open_root(&self.parent)?;
        let target = Dir::open_root(destination.parent())?;
        let _ = fs::remove_file(self.path().join(OWNER_FILE));
        let started = Instant::now();
        loop {
            match source.rename_noreplace(
                NoClobber::Renameat2,
                &self.name,
                &target,
                destination.name(),
            ) {
                Ok(()) => {
                    self.published = true;
                    let _ = target.sync();
                    return Ok(());
                }
                Err(error) if is_transient(&error) && started.elapsed() < patience => {
                    thread::sleep(Duration::from_millis(100));
                }
                Err(error) => {
                    // Still ours: name it again so a crash before cleanup is swept.
                    let owner = serde_json::to_vec(&Owner::current()).expect("an owner");
                    let _ = fs::write(self.path().join(OWNER_FILE), owner);
                    return Err(error);
                }
            }
        }
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(self.path());
        }
    }
}

fn is_transient(error: &io::Error) -> bool {
    // Sharing and lock violations, which std does not classify.
    let windows_busy = cfg!(windows) && matches!(error.raw_os_error(), Some(5 | 32 | 33));
    windows_busy || error.kind() == io::ErrorKind::ResourceBusy
}

/// Removes staging folders in `parent` whose owner is gone; returns how many.
pub fn sweep_orphans(parent: &Path) -> usize {
    let Ok(entries) = fs::read_dir(parent) else {
        return 0;
    };
    let mut swept = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(STAGING_PREFIX) {
            continue;
        }
        // Never follows a link out of the parent.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let path = entry.path();
        let orphan = match fs::read(path.join(OWNER_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Owner>(&bytes).ok())
        {
            Some(owner) => !owner.is_alive(),
            None => entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                .is_some_and(|age| age > UNOWNED_AGE),
        };
        if orphan && fs::remove_dir_all(&path).is_ok() {
            swept += 1;
        }
    }
    swept
}

#[cfg(windows)]
fn process_exists(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, STILL_ACTIVE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: a query open; the handle is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // Only "no such process" is gone; access denied is a running process.
        return unsafe { GetLastError() } != ERROR_INVALID_PARAMETER;
    }
    let mut code = 0u32;
    // SAFETY: `handle` is valid until closed.
    let read = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe { CloseHandle(handle) };
    read == 0 || code == STILL_ACTIVE as u32
}

#[cfg(windows)]
fn process_start(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: a query open; the handle is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: `handle` is valid and the out pointers live on this stack.
    let read =
        unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) };
    unsafe { CloseHandle(handle) };
    (read != 0)
        .then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The start time in clock ticks since boot (field 22 of `/proc/<pid>/stat`).
#[cfg(target_os = "linux")]
fn process_start(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may hold spaces and parentheses; fields follow the last ')'.
    let after = &stat[stat.rfind(')')? + 1..];
    after.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn process_start(_pid: u32) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exited_pid() -> u32 {
        let mut child = if cfg!(windows) {
            std::process::Command::new("cmd")
                .args(["/C", "exit"])
                .spawn()
                .unwrap()
        } else {
            std::process::Command::new("true").spawn().unwrap()
        };
        let pid = child.id();
        child.wait().unwrap();
        pid
    }

    fn staging_with(parent: &Path, name: &str, owner: Option<Owner>) -> PathBuf {
        let path = parent.join(format!("{STAGING_PREFIX}{name}"));
        fs::create_dir(&path).unwrap();
        fs::write(path.join("deck.mp4"), b"partial").unwrap();
        if let Some(owner) = owner {
            fs::write(path.join(OWNER_FILE), serde_json::to_vec(&owner).unwrap()).unwrap();
        }
        path
    }

    #[test]
    fn this_process_is_alive_and_an_exited_one_is_not() {
        assert!(Owner::current().is_alive());
        let gone = Owner {
            pid: exited_pid(),
            started: None,
        };
        assert!(!gone.is_alive());
    }

    #[test]
    fn a_staging_folder_of_a_dead_owner_is_swept_and_a_live_ones_kept() {
        let temp = tempfile::tempdir().unwrap();
        let dead = staging_with(
            temp.path(),
            "dead",
            Some(Owner {
                pid: exited_pid(),
                started: Some(1),
            }),
        );
        let live = staging_with(temp.path(), "live", Some(Owner::current()));
        // Fresh and without an owner file: it may be a folder being created right now.
        let unowned = staging_with(temp.path(), "unowned", None);
        let other = temp.path().join("Slides");
        fs::create_dir(&other).unwrap();
        let reused = Owner::current().started.map(|started| {
            staging_with(
                temp.path(),
                "reused",
                Some(Owner {
                    pid: std::process::id(),
                    started: Some(started.wrapping_add(1)),
                }),
            )
        });

        let expected = 1 + usize::from(reused.is_some());
        assert_eq!(sweep_orphans(temp.path()), expected);
        assert!(!dead.exists());
        assert!(live.join("deck.mp4").exists());
        assert!(unowned.exists());
        assert!(other.exists(), "folders of other names are never touched");
        if let Some(reused) = reused {
            assert!(!reused.exists(), "a reused pid is not the owner");
        }
    }

    #[test]
    fn publishing_refuses_an_existing_empty_folder_and_leaves_it_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let chosen = temp.path().join("Slides");
        let destination = Destination::prepare(&chosen).unwrap();
        fs::create_dir(&chosen).unwrap();
        assert!(
            Destination::prepare(&chosen)
                .unwrap_err()
                .contains("already exists")
        );

        let mut staging = Staging::create(destination.parent()).unwrap();
        fs::write(staging.path().join("index.html"), b"deck").unwrap();
        let error = staging
            .publish_with_patience(&destination, Duration::ZERO)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "{error}");
        assert_eq!(fs::read_dir(&chosen).unwrap().count(), 0, "still empty");
        let staged = staging.path();
        assert!(staged.join("index.html").exists());
        assert!(
            staged.join(OWNER_FILE).exists(),
            "named again for the sweep"
        );
        drop(staging);
        assert!(!staged.exists(), "a failed publication removes its staging");
        assert!(chosen.exists());
    }

    #[test]
    fn a_published_folder_has_the_files_and_no_owner_file() {
        let temp = tempfile::tempdir().unwrap();
        let chosen = temp.path().join("Slides");
        let destination = Destination::prepare(&chosen).unwrap();
        let mut staging = Staging::create(destination.parent()).unwrap();
        fs::write(staging.path().join("index.html"), b"deck").unwrap();
        staging.publish(&destination).unwrap();
        drop(staging);
        assert_eq!(fs::read(chosen.join("index.html")).unwrap(), b"deck");
        assert!(!chosen.join(OWNER_FILE).exists());
        let leftovers = fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(STAGING_PREFIX)
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn a_destination_needs_a_new_name_in_an_existing_folder() {
        let temp = tempfile::tempdir().unwrap();
        assert!(Destination::prepare(&temp.path().join("missing/Slides")).is_err());
        assert!(Destination::prepare(&temp.path().join(".fframes-deck-x")).is_err());
        let file = temp.path().join("taken");
        fs::write(&file, b"").unwrap();
        assert!(Destination::prepare(&file).is_err());
    }
}
