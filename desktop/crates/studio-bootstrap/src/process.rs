use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("process spawn failed for '{command}': {source}")]
    SpawnFailed {
        command: String,
        #[source]
        source: std::io::Error,
    },
    #[error("process '{command}' (pid {pid}) failed assignment to job object")]
    JobAssignmentFailed { command: String, pid: u32 },
    #[error("process '{command}' (pid {pid}) timed out during graceful termination")]
    TerminationTimeout { command: String, pid: u32 },
    #[error(
        "process '{command}' (pid {pid}) left running members in its task process group after termination"
    )]
    GroupSurvivors { command: String, pid: u32 },
    #[error("process error: {0}")]
    Custom(String),
}

/// App-local environment configuration that never mutates global PATH
/// and starts strictly from an explicit allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildEnvironment {
    vars: BTreeMap<String, String>,
}

impl Default for ChildEnvironment {
    fn default() -> Self {
        Self::default_allowlist()
    }
}

impl ChildEnvironment {
    /// Constructs a clean environment with only allowlisted variables from current process.
    pub fn default_allowlist() -> Self {
        let mut vars = BTreeMap::new();

        #[cfg(unix)]
        const ALLOWED_KEYS: &[&str] = &[
            "HOME",
            "USER",
            "LOGNAME",
            "SHELL",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "TMPDIR",
            "PATH",
            "TERM",
            "XDG_RUNTIME_DIR",
            "XDG_DATA_DIRS",
            "XDG_CONFIG_DIRS",
            "LIBCLANG_PATH",
            "DEVELOPER_DIR",
            "SDKROOT",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ];

        #[cfg(windows)]
        const ALLOWED_KEYS: &[&str] = &[
            "SystemRoot",
            "SystemDrive",
            "WINDIR",
            // rustc locates the MSVC linker through the Visual Studio setup instances under
            // %ProgramData% (and vswhere under Program Files) when no developer prompt is set.
            "ProgramData",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "APPDATA",
            "LOCALAPPDATA",
            "USERPROFILE",
            "COMSPEC",
            "PATHEXT",
            "TEMP",
            "TMP",
            "PATH",
            "USERNAME",
            "INCLUDE",
            "LIB",
            "LIBPATH",
            "VCToolsInstallDir",
            "VSINSTALLDIR",
            "WindowsSdkDir",
            "WindowsSDKVersion",
            "UniversalCRTSdkDir",
            "UCRTVersion",
            "LIBCLANG_PATH",
        ];

        for &key in ALLOWED_KEYS {
            if let Ok(val) = std::env::var(key) {
                vars.insert(key.to_string(), val);
            }
        }

        Self { vars }
    }

    /// Empty environment without any host variables.
    pub fn empty() -> Self {
        Self {
            vars: BTreeMap::new(),
        }
    }

    pub fn set(&mut self, key: impl Into<String>, val: impl Into<String>) -> &mut Self {
        self.vars.insert(key.into(), val.into());
        self
    }

    pub fn remove(&mut self, key: &str) -> &mut Self {
        self.vars.remove(key);
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(|s| s.as_str())
    }

    /// Every variable in deterministic (name) order, unredacted: for fingerprinting the
    /// exact environment a child will receive, never for logging.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Prepend a path directory to PATH within this child environment,
    /// without modifying the host process PATH.
    pub fn prepend_path(&mut self, path: impl AsRef<Path>) -> &mut Self {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let path_key = if cfg!(windows) { "Path" } else { "PATH" };

        // Windows variable names are case-insensitive: fold every spelling (the host
        // allowlist reads `PATH`) into the one key written here, preferring that key's
        // value, so a second prepend extends the first instead of the host value.
        let current_path = if cfg!(windows) {
            let spellings: Vec<String> = self
                .vars
                .keys()
                .filter(|key| key.eq_ignore_ascii_case("PATH"))
                .cloned()
                .collect();
            let mut current = None;
            for key in spellings {
                let value = self.vars.remove(&key);
                if key == path_key || current.is_none() {
                    current = value.or(current);
                }
            }
            current.unwrap_or_default()
        } else {
            self.vars.get("PATH").cloned().unwrap_or_default()
        };
        let new_path = if current_path.is_empty() {
            path_str
        } else {
            #[cfg(unix)]
            {
                format!("{path_str}:{current_path}")
            }
            #[cfg(windows)]
            {
                format!("{path_str};{current_path}")
            }
        };

        self.vars.insert(path_key.to_string(), new_path);
        self
    }

    /// Apply the environment strictly to `Command` after clearing ambient environment.
    pub fn apply_to_command(&self, cmd: &mut Command) {
        cmd.env_clear();
        for (k, v) in &self.vars {
            cmd.env(k, v);
        }
    }

    /// Return a redacted copy of variables safe for logging/diagnostics.
    pub fn export_redacted(&self) -> BTreeMap<String, String> {
        let sensitive_fragments = ["TOKEN", "SECRET", "KEY", "PASS", "AUTH", "CREDENTIAL"];
        self.vars
            .iter()
            .map(|(k, v)| {
                let is_sensitive = sensitive_fragments
                    .iter()
                    .any(|frag| k.to_ascii_uppercase().contains(frag));
                if is_sensitive {
                    (k.clone(), "[REDACTED]".to_string())
                } else {
                    (k.clone(), v.clone())
                }
            })
            .collect()
    }
}

pub struct SpawnOptions {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub current_dir: Option<PathBuf>,
    pub env: ChildEnvironment,
    pub stdin: Stdio,
    pub stdout: Stdio,
    pub stderr: Stdio,
}

impl SpawnOptions {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
            env: ChildEnvironment::default_allowlist(),
            stdin: Stdio::null(),
            stdout: Stdio::piped(),
            stderr: Stdio::piped(),
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.args.push(arg.as_ref().to_string_lossy().to_string());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for a in args {
            self.arg(a);
        }
        self
    }

    pub fn current_dir(&mut self, dir: impl Into<PathBuf>) -> &mut Self {
        self.current_dir = Some(dir.into());
        self
    }

    pub fn stdin(&mut self, stdio: Stdio) -> &mut Self {
        self.stdin = stdio;
        self
    }

    pub fn stdout(&mut self, stdio: Stdio) -> &mut Self {
        self.stdout = stdio;
        self
    }

    pub fn stderr(&mut self, stdio: Stdio) -> &mut Self {
        self.stderr = stdio;
        self
    }
}

/// Members of a task process group (job object on Windows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupMembership {
    /// No live (non-zombie) member remains.
    Empty,
    /// Live members whose pids could be enumerated.
    Members(Vec<u32>),
    /// Live members exist but could not be enumerated on this platform.
    Present,
}

/// Verified outcome of terminating a task tree. `group_empty` covers the task
/// process group only; it cannot see helpers that escaped it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminationReport {
    /// SIGKILL (or job termination) was required after the graceful drain.
    pub forced: bool,
    pub direct_child_exited: bool,
    pub group_empty: bool,
    /// Live group members after termination when enumerable.
    pub remaining: Vec<u32>,
}
impl TerminationReport {
    /// Direct child gone and no member left in the owned group.
    pub fn verified(&self) -> bool {
        self.direct_child_exited && self.group_empty
    }
}

/// Outcome of terminating every tree owned by a [`ProcessTreeManager`] scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeTermination {
    pub children: Vec<TerminationReport>,
}
impl ScopeTermination {
    /// Every owned tree was terminated and its group verified empty.
    pub fn verified(&self) -> bool {
        self.children.iter().all(TerminationReport::verified)
    }

    /// One report for the whole scope: forced if any tree needed it, exited/empty only
    /// if every tree was. A scope with no owned trees is trivially clean.
    pub fn merged(&self) -> TerminationReport {
        TerminationReport {
            forced: self.children.iter().any(|c| c.forced),
            direct_child_exited: self.children.iter().all(|c| c.direct_child_exited),
            group_empty: self.children.iter().all(|c| c.group_empty),
            remaining: self
                .children
                .iter()
                .flat_map(|c| c.remaining.iter().copied())
                .collect(),
        }
    }
}

/// Non-destructive snapshot of every tree a scope owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeObservation {
    /// Number of owned trees that are still alive (leader running or group non-empty).
    pub live_children: usize,
    /// Combined state of the owned trees; clean iff `live_children == 0`.
    pub termination: TerminationReport,
    /// Descendants that already left their task process group (Linux; only visible
    /// while the escaping parent is alive, so an empty list never proves containment).
    pub escaped: Vec<u32>,
}
impl ScopeObservation {
    pub fn is_clean(&self) -> bool {
        self.live_children == 0 && self.termination.verified() && self.escaped.is_empty()
    }
}

/// Whether an adapter's writers are known to stay inside the owned process group.
/// Only an explicit qualification of a specific adapter produces
/// [`WriterOwnership::ProcessGroupContained`]; escaped, background or unmodelled
/// writers are never claimed detectable, so everything else blocks candidate capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriterOwnership {
    /// Qualified: this adapter's writer descendants remain in the task process group.
    ProcessGroupContained { qualification: String },
    /// An escape from the task process group was observed.
    Detached,
    /// Not qualified; a surviving writer cannot be ruled out.
    Unknown,
}
impl WriterOwnership {
    pub fn is_qualified(&self) -> bool {
        matches!(self, Self::ProcessGroupContained { .. })
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::ProcessGroupContained { .. } => "process-group-contained",
            Self::Detached => "detached",
            Self::Unknown => "unknown",
        }
    }
}

const GROUP_VERIFY_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// One consistent look at the process table. `complete` is false whenever any part of
/// the enumeration could not be read, so an absent member can never be mistaken for a
/// vanished one.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Default)]
struct ProcScan {
    entries: Vec<ProcEntry>,
    complete: bool,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct ProcEntry {
    pid: u32,
    ppid: u32,
    pgrp: i32,
    start: u64,
    zombie: bool,
}

#[cfg(target_os = "linux")]
fn parse_proc_stat(pid: u32, stat: &str) -> Option<ProcEntry> {
    // The command name may contain spaces and parentheses; fields follow the last ')'.
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    let mut fields = rest.split_whitespace();
    let state = fields.next()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgrp = fields.next()?.parse().ok()?;
    // starttime is field 22 of the line, i.e. index 19 after the state field.
    let start = fields.nth(16)?.parse().ok()?;
    Some(ProcEntry {
        pid,
        ppid,
        pgrp,
        start,
        zombie: state == "Z" || state == "X",
    })
}

/// Snapshot of /proc. Any unreadable piece makes the scan incomplete.
#[cfg(target_os = "linux")]
fn scan_proc() -> ProcScan {
    let mut scan = ProcScan {
        entries: Vec::new(),
        complete: true,
    };
    let Ok(dir) = std::fs::read_dir("/proc") else {
        scan.complete = false;
        return scan;
    };
    for entry in dir {
        let Ok(entry) = entry else {
            scan.complete = false;
            continue;
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(stat) => match parse_proc_stat(pid, &stat) {
                Some(parsed) => scan.entries.push(parsed),
                None => scan.complete = false,
            },
            // The process exited between listing and reading: genuinely gone.
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ESRCH)) => {}
            Err(_) => scan.complete = false,
        }
    }
    scan
}

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
enum ScanVerdict {
    Members(Vec<u32>),
    /// Only zombies (or nothing) remain in the group.
    Quiet,
    /// The table was not fully readable: nothing can be concluded.
    Incomplete,
    /// The group id now belongs to an unrelated process family.
    Foreign,
}

#[cfg(target_os = "linux")]
fn classify_scan(scan: &ProcScan, pgid: i32, leader_start: Option<u64>) -> ScanVerdict {
    if !scan.complete {
        return ScanVerdict::Incomplete;
    }
    // A live-or-zombie process owning the group id that is not our leader means the id
    // was recycled; that group is not ours and is never signalled.
    if let (Some(expected), Some(leader)) = (
        leader_start,
        scan.entries.iter().find(|p| p.pid as i32 == pgid),
    ) && leader.start != expected
    {
        return ScanVerdict::Foreign;
    }
    let members: Vec<u32> = scan
        .entries
        .iter()
        .filter(|p| p.pgrp == pgid && !p.zombie)
        .map(|p| p.pid)
        .collect();
    if members.is_empty() {
        ScanVerdict::Quiet
    } else {
        ScanVerdict::Members(members)
    }
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    Exists,
    Gone,
    Unknown,
}

#[cfg(unix)]
fn probe_group(pgid: i32) -> Probe {
    // SAFETY: signal 0 only probes the group this tracker owns.
    if unsafe { libc::kill(-pgid, 0) } == 0 {
        Probe::Exists
    } else if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        Probe::Gone
    } else {
        Probe::Unknown
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Default)]
struct MacProcScan {
    entries: Vec<MacProcEntry>,
    complete: bool,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
struct MacProcEntry {
    pid: u32,
    pgrp: u32,
    start: (u64, u64),
    zombie: bool,
}

#[cfg(target_os = "macos")]
fn mac_process_start_time(pid: libc::pid_t) -> Option<(u64, u64)> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    // SAFETY: `info` points to a correctly-sized output buffer for PROC_PIDTBSDINFO.
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int,
        )
    };
    if read != std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int {
        return None;
    }
    // SAFETY: proc_pidinfo returned exactly the initialized structure size.
    let info = unsafe { info.assume_init() };
    Some((info.pbi_start_tvsec, info.pbi_start_tvusec))
}

#[cfg(target_os = "macos")]
fn mac_process_group_list_failed(count: libc::c_int, errno: libc::c_int) -> bool {
    count < 0 || (count == 0 && errno != 0)
}

#[cfg(target_os = "macos")]
fn mac_process_info_confirms_exit(read: libc::c_int, errno: libc::c_int) -> bool {
    read == 0 && errno == libc::ESRCH
}

/// Lists every process in a group, including zombies, via libproc. macOS does not
/// expose `/proc`, and `killpg(..., 0)` continues to report zombie-only groups as
/// present, so termination verification needs process state as well as the probe.
#[cfg(target_os = "macos")]
fn scan_process_group(pgid: i32) -> MacProcScan {
    let mut scan = MacProcScan {
        entries: Vec::new(),
        complete: true,
    };
    let mut capacity = 64usize;
    let pids = loop {
        let Some(bytes) = capacity.checked_mul(std::mem::size_of::<libc::pid_t>()) else {
            scan.complete = false;
            return scan;
        };
        let Ok(buffer_size) = libc::c_int::try_from(bytes) else {
            scan.complete = false;
            return scan;
        };
        let mut pids = vec![0 as libc::pid_t; capacity];
        // SAFETY: libproc writes at most `buffer_size` bytes to this allocated pid buffer.
        // SAFETY: `__error` returns this thread's errno storage.
        unsafe { *libc::__error() = 0 };
        let count = unsafe { libc::proc_listpgrppids(pgid, pids.as_mut_ptr().cast(), buffer_size) };
        let errno = unsafe { *libc::__error() };
        if mac_process_group_list_failed(count, errno) {
            scan.complete = false;
            return scan;
        }
        let count = count as usize;
        if count < capacity {
            pids.truncate(count);
            break pids;
        }
        if capacity >= 1_048_576 {
            scan.complete = false;
            return scan;
        }
        capacity *= 2;
    };

    for pid in pids {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        // A nonzero arg lets libproc read entries that have moved to zombproc but have
        // not yet been reaped by their parent.
        // SAFETY: `info` points to a correctly-sized output buffer for PROC_PIDTBSDINFO.
        // SAFETY: `__error` returns this thread's errno storage.
        unsafe { *libc::__error() = 0 };
        let read = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                1,
                info.as_mut_ptr().cast(),
                std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int,
            )
        };
        let errno = unsafe { *libc::__error() };
        if read == 0 {
            // libproc maps lookup failures to zero and leaves errno set. ESRCH means this
            // PID left the process table after enumeration; other failures make the scan
            // incomplete and cannot be counted as an exited process.
            if !mac_process_info_confirms_exit(read, errno) {
                scan.complete = false;
            }
            continue;
        }
        if read != std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int {
            scan.complete = false;
            continue;
        }
        // SAFETY: proc_pidinfo returned exactly the initialized structure size.
        let info = unsafe { info.assume_init() };
        scan.entries.push(MacProcEntry {
            pid: info.pbi_pid,
            pgrp: info.pbi_pgid,
            start: (info.pbi_start_tvsec, info.pbi_start_tvusec),
            zombie: info.pbi_status == libc::SZOMB,
        });
    }
    scan
}

#[cfg(target_os = "macos")]
#[derive(Debug, PartialEq, Eq)]
enum MacScanVerdict {
    Members(Vec<u32>),
    Quiet,
    Incomplete,
    Foreign,
}

#[cfg(target_os = "macos")]
fn classify_mac_scan(
    scan: &MacProcScan,
    pgid: i32,
    leader_start: Option<(u64, u64)>,
) -> MacScanVerdict {
    if !scan.complete {
        return MacScanVerdict::Incomplete;
    }
    if let (Some(expected), Some(leader)) = (
        leader_start,
        scan.entries
            .iter()
            .find(|process| process.pid as i32 == pgid),
    ) && leader.start != expected
    {
        return MacScanVerdict::Foreign;
    }
    let members: Vec<u32> = scan
        .entries
        .iter()
        .filter(|process| process.pgrp as i32 == pgid && !process.zombie)
        .map(|process| process.pid)
        .collect();
    if members.is_empty() {
        MacScanVerdict::Quiet
    } else {
        MacScanVerdict::Members(members)
    }
}

#[cfg(target_os = "macos")]
fn mac_membership_with(
    pgid: i32,
    leader_start: Option<(u64, u64)>,
    probe: &mut dyn FnMut() -> Probe,
    scan: &mut dyn FnMut() -> MacProcScan,
    pause: &dyn Fn(),
) -> GroupMembership {
    match probe() {
        Probe::Gone => return GroupMembership::Empty,
        Probe::Unknown => return GroupMembership::Present,
        Probe::Exists => {}
    }
    let mut quiet = 0;
    for _ in 0..4 {
        match classify_mac_scan(&scan(), pgid, leader_start) {
            MacScanVerdict::Incomplete => return GroupMembership::Present,
            MacScanVerdict::Foreign => return GroupMembership::Empty,
            MacScanVerdict::Members(pids) => return GroupMembership::Members(pids),
            MacScanVerdict::Quiet => {
                quiet += 1;
                if quiet == 2 {
                    return GroupMembership::Empty;
                }
                pause();
            }
        }
    }
    GroupMembership::Present
}

/// Pure membership decision over injectable probe/scan sources.
///
/// A group is `Empty` only when the kernel says it is gone, or when two consecutive
/// complete scans show nothing but zombies. An incomplete scan is `Present`: a member
/// that merely could not be seen is never treated as having exited.
#[cfg(target_os = "linux")]
fn membership_with(
    pgid: i32,
    leader_start: Option<u64>,
    probe: &mut dyn FnMut() -> Probe,
    scan: &mut dyn FnMut() -> ProcScan,
    pause: &dyn Fn(),
) -> GroupMembership {
    match probe() {
        Probe::Gone => return GroupMembership::Empty,
        Probe::Unknown => return GroupMembership::Present,
        Probe::Exists => {}
    }
    let mut quiet = 0;
    for _ in 0..4 {
        match classify_scan(&scan(), pgid, leader_start) {
            ScanVerdict::Incomplete => return GroupMembership::Present,
            ScanVerdict::Foreign => return GroupMembership::Empty,
            ScanVerdict::Members(pids) => return GroupMembership::Members(pids),
            ScanVerdict::Quiet => {
                quiet += 1;
                if quiet == 2 {
                    return GroupMembership::Empty;
                }
                pause();
            }
        }
    }
    GroupMembership::Present
}

#[cfg(target_os = "linux")]
fn escaped_in_scan(
    scan: &ProcScan,
    root_pid: u32,
    pgid: i32,
    leader_start: Option<u64>,
) -> Vec<u32> {
    if !scan.complete {
        return Vec::new();
    }
    if let (Some(expected), Some(root)) = (
        leader_start,
        scan.entries.iter().find(|p| p.pid == root_pid),
    ) && root.start != expected
    {
        return Vec::new();
    }
    let mut descendants = vec![root_pid];
    let mut escaped = Vec::new();
    let mut index = 0;
    while index < descendants.len() {
        let parent = descendants[index];
        index += 1;
        for process in scan.entries.iter().filter(|p| p.ppid == parent) {
            if descendants.contains(&process.pid) {
                continue;
            }
            descendants.push(process.pid);
            if process.pgrp != pgid && !process.zombie {
                escaped.push(process.pid);
            }
        }
    }
    escaped
}

#[cfg(windows)]
fn job_has_active_processes(job: windows_sys::Win32::Foundation::HANDLE) -> bool {
    // SAFETY: queries a job object handle owned by the tracker.
    unsafe {
        let mut accounting: windows_sys::Win32::System::JobObjects::JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
        let mut ret_len = 0;
        let res = windows_sys::Win32::System::JobObjects::QueryInformationJobObject(
            job,
            windows_sys::Win32::System::JobObjects::JobObjectBasicAccountingInformation,
            &mut accounting as *mut _ as _,
            std::mem::size_of_val(&accounting) as u32,
            &mut ret_len,
        );
        // A failed query cannot prove the owned descendants exited.
        res == 0 || accounting.ActiveProcesses > 0
    }
}

pub struct TrackedChild {
    pub command: String,
    pub pid: u32,
    #[cfg(unix)]
    pgid: i32,
    /// Kernel start time of the leader; distinguishes our group from a recycled id.
    #[cfg(target_os = "linux")]
    leader_start: Option<u64>,
    /// Kernel start time of the leader; distinguishes our group from a recycled id.
    #[cfg(target_os = "macos")]
    leader_start: (u64, u64),
    child: Child,
    #[cfg(windows)]
    job_handle: windows_sys::Win32::Foundation::HANDLE,
    /// Set once the group was verified empty and the leader reaped. After this the
    /// group/job id may belong to someone else: it is never probed or signalled again.
    terminal: Option<TerminationReport>,
    os_calls: u32,
}
unsafe impl Send for TrackedChild {}
unsafe impl Sync for TrackedChild {}

impl TrackedChild {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Number of signals and group probes issued so far. Diagnostic: a tracker whose
    /// group is already verified empty must never increase it.
    pub fn os_calls(&self) -> u32 {
        self.os_calls
    }

    /// Whether the group was verified empty and this tracker is permanently inert.
    pub fn is_terminated(&self) -> bool {
        self.terminal.is_some()
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, ProcessError> {
        let status = self.child.try_wait()?;
        if status.is_some() && self.terminal.is_none() {
            // The leader is reaped; if nothing else is left, latch before the id can recycle.
            self.latch_if_finished(false);
        }
        Ok(status)
    }

    pub fn wait(&mut self) -> Result<ExitStatus, ProcessError> {
        let status = self.child.wait()?;
        if self.terminal.is_none() {
            self.latch_if_finished(false);
        }
        Ok(status)
    }

    pub fn is_alive(&mut self) -> bool {
        if self.terminal.is_some() {
            return false;
        }
        if matches!(self.child.try_wait(), Ok(None)) {
            return true;
        }
        self.group_membership() != GroupMembership::Empty
    }

    /// Gracefully terminate the child process and its entire process tree/group.
    /// Sends SIGTERM (or closes job on Windows), allows up to `drain_timeout` for exit,
    /// then forces SIGKILL if any processes in the tree remain. Fails when the group
    /// still has live members after forced termination; direct-child exit alone is
    /// never reported as success.
    pub fn terminate_gracefully(&mut self, drain_timeout: Duration) -> Result<(), ProcessError> {
        let report = self.terminate_verified(drain_timeout)?;
        self.require_empty(&report)
    }

    /// Same termination as [`Self::terminate_gracefully`] but returns the verified
    /// outcome so callers can record whether forced cleanup was needed and which
    /// group members remain. The verification covers only the task process group
    /// (job object on Windows): a helper that escaped it (for example through
    /// `setsid`) is invisible here. Every wait is bounded; an unverifiable group is
    /// reported as not empty rather than assumed gone. A tracker whose group was
    /// already verified empty returns the recorded report without any OS call.
    pub fn terminate_verified(
        &mut self,
        drain_timeout: Duration,
    ) -> Result<TerminationReport, ProcessError> {
        if let Some(report) = &self.terminal {
            return Ok(report.clone());
        }
        if self.group_membership() == GroupMembership::Empty && self.reap_leader() {
            return Ok(self.latch_report(false));
        }
        #[cfg(unix)]
        {
            self.signal_group(libc::SIGTERM);
            let start = Instant::now();
            while start.elapsed() < drain_timeout {
                if self.group_membership() == GroupMembership::Empty && self.reap_leader() {
                    return Ok(self.latch_report(false));
                }
                std::thread::sleep(POLL_INTERVAL.min(drain_timeout));
            }
            self.signal_group(libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let _ = drain_timeout;
            self.terminate_job();
        }
        self.await_group_empty();
        Ok(self.report(true))
    }

    /// Forcefully kill the entire process tree immediately and verify the group is empty.
    pub fn kill_forcefully(&mut self) -> Result<(), ProcessError> {
        if self.terminal.is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        self.signal_group(libc::SIGKILL);
        #[cfg(windows)]
        self.terminate_job();
        self.await_group_empty();
        let report = self.report(true);
        self.require_empty(&report)
    }

    /// Current membership of the task process group (job object on Windows).
    pub fn group_membership(&mut self) -> GroupMembership {
        if self.terminal.is_some() {
            return GroupMembership::Empty;
        }
        #[cfg(unix)]
        {
            self.os_calls += 1;
            #[cfg(target_os = "linux")]
            {
                let pgid = self.pgid;
                let start = self.leader_start;
                membership_with(
                    pgid,
                    start,
                    &mut || probe_group(pgid),
                    &mut scan_proc,
                    &|| std::thread::sleep(Duration::from_millis(3)),
                )
            }
            #[cfg(target_os = "macos")]
            {
                let pgid = self.pgid;
                let start = self.leader_start;
                mac_membership_with(
                    pgid,
                    Some(start),
                    &mut || probe_group(pgid),
                    &mut || scan_process_group(pgid),
                    &|| std::thread::sleep(Duration::from_millis(3)),
                )
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                match probe_group(self.pgid) {
                    Probe::Gone => GroupMembership::Empty,
                    _ => GroupMembership::Present,
                }
            }
        }
        #[cfg(windows)]
        {
            self.os_calls += 1;
            if job_has_active_processes(self.job_handle) {
                GroupMembership::Present
            } else {
                GroupMembership::Empty
            }
        }
    }

    /// Descendants of the direct child that are no longer in the task process group
    /// (Linux only; empty elsewhere). Detects an escape such as `setsid` only while
    /// the escaping parent is still alive: a double-forked helper is reparented to init
    /// and is not observable, so an empty answer never proves containment.
    pub fn escaped_descendants(&self) -> Vec<u32> {
        if self.terminal.is_some() {
            return Vec::new();
        }
        #[cfg(target_os = "linux")]
        {
            escaped_in_scan(&scan_proc(), self.pid, self.pgid, self.leader_start)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Vec::new()
        }
    }

    #[cfg(unix)]
    fn signal_group(&mut self, signal: i32) {
        if self.terminal.is_some() {
            return;
        }
        self.os_calls += 1;
        // SAFETY: signalling a process group this tracker created and still owns:
        // `terminal` is unset, so the group was not yet verified empty.
        unsafe {
            libc::kill(-self.pgid, signal);
        }
    }

    #[cfg(windows)]
    fn terminate_job(&mut self) {
        if self.terminal.is_some() {
            return;
        }
        self.os_calls += 1;
        // SAFETY: terminates the job object owned by this tracker.
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job_handle, 1);
        }
    }

    /// Reaps the direct child within a bounded time; false if it did not exit.
    fn reap_leader(&mut self) -> bool {
        let deadline = Instant::now() + GROUP_VERIFY_TIMEOUT;
        loop {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn await_group_empty(&mut self) {
        let deadline = Instant::now() + GROUP_VERIFY_TIMEOUT;
        loop {
            if self.group_membership() == GroupMembership::Empty {
                self.reap_leader();
                return;
            }
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn report(&mut self, forced: bool) -> TerminationReport {
        let membership = self.group_membership();
        let report = TerminationReport {
            forced,
            direct_child_exited: matches!(self.child.try_wait(), Ok(Some(_))),
            group_empty: membership == GroupMembership::Empty,
            remaining: match membership {
                GroupMembership::Members(pids) => pids,
                _ => Vec::new(),
            },
        };
        if report.verified() {
            self.terminal = Some(report.clone());
        }
        report
    }

    fn latch_report(&mut self, forced: bool) -> TerminationReport {
        let report = TerminationReport {
            forced,
            direct_child_exited: true,
            group_empty: true,
            remaining: Vec::new(),
        };
        self.terminal = Some(report.clone());
        report
    }

    /// Latches terminal emptiness when the leader is reaped and the group is empty.
    fn latch_if_finished(&mut self, forced: bool) {
        if matches!(self.child.try_wait(), Ok(Some(_)))
            && self.group_membership() == GroupMembership::Empty
        {
            self.latch_report(forced);
        }
    }

    fn require_empty(&self, report: &TerminationReport) -> Result<(), ProcessError> {
        if report.group_empty {
            Ok(())
        } else {
            Err(ProcessError::GroupSurvivors {
                command: self.command.clone(),
                pid: self.pid,
            })
        }
    }
}

#[cfg(unix)]
impl Drop for TrackedChild {
    fn drop(&mut self) {
        if self.terminal.is_some() {
            return;
        }
        if self.group_membership() != GroupMembership::Empty {
            self.signal_group(libc::SIGKILL);
            self.await_group_empty();
        }
    }
}

#[cfg(windows)]
impl Drop for TrackedChild {
    fn drop(&mut self) {
        if !self.job_handle.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.job_handle);
            }
        }
    }
}

/// Spawns a child process with process group ownership and app-local environment isolation.
pub fn spawn_tracked(opts: SpawnOptions) -> Result<TrackedChild, ProcessError> {
    let cmd_str = opts.program.to_string_lossy().to_string();
    let mut command = Command::new(&opts.program);
    command.args(&opts.args);

    if let Some(dir) = &opts.current_dir {
        command.current_dir(dir);
    }

    opts.env.apply_to_command(&mut command);
    command.stdin(opts.stdin);
    command.stdout(opts.stdout);
    command.stderr(opts.stderr);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Make the child the group leader of a new process group (setpgid(0, 0))
        command.process_group(0);

        let child = command
            .spawn()
            .map_err(|source| ProcessError::SpawnFailed {
                command: cmd_str.clone(),
                source,
            })?;

        let pid = child.id();
        let pgid = pid as i32;
        #[cfg(target_os = "linux")]
        let leader_start = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| parse_proc_stat(pid, &stat))
            .map(|entry| entry.start);
        #[cfg(target_os = "macos")]
        let leader_start = match mac_process_start_time(pid as libc::pid_t) {
            Some(start) => start,
            None => {
                // Keep the unreaped leader's pid reserved while cleaning up this just-created
                // group; without its start time, later group-id reuse cannot be distinguished.
                // SAFETY: this process group was created for this child and its leader is not
                // reaped until after the signal, so the numeric group id cannot be recycled.
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
                let mut child = child;
                let _ = child.wait();
                return Err(ProcessError::Custom(
                    "could not identify the spawned process before tracking its group".into(),
                ));
            }
        };

        Ok(TrackedChild {
            command: cmd_str,
            pid,
            pgid,
            #[cfg(target_os = "linux")]
            leader_start,
            #[cfg(target_os = "macos")]
            leader_start,
            child,
            terminal: None,
            os_calls: 0,
        })
    }

    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
        use windows_sys::Win32::System::JobObjects::*;
        use windows_sys::Win32::System::Threading::*;

        // Create a Job Object configured to kill all processes on close
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return Err(ProcessError::Custom(
                    "failed to create Windows JobObject".into(),
                ));
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let res = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if res == 0 {
                CloseHandle(job);
                return Err(ProcessError::Custom(
                    "failed to configure JobObject limit".into(),
                ));
            }

            // Spawn suspended so it cannot execute or break away before assignment, and
            // without a console window: the studio is a GUI process, so every console child
            // (cargo, the worker, .cmd adapters) would otherwise open its own.
            command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
            let child = command.spawn().map_err(|source| {
                CloseHandle(job);
                ProcessError::SpawnFailed {
                    command: cmd_str.clone(),
                    source,
                }
            })?;

            let pid = child.id();
            let proc_handle = child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
            let assign_res = AssignProcessToJobObject(job, proc_handle);
            if assign_res == 0 {
                let _ = TerminateProcess(proc_handle, 1);
                CloseHandle(job);
                return Err(ProcessError::JobAssignmentFailed {
                    command: cmd_str,
                    pid,
                });
            }

            // Query Toolhelp to find the primary thread of the child process
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                let _ = TerminateProcess(proc_handle, 1);
                CloseHandle(job);
                return Err(ProcessError::Custom(
                    "failed to take thread snapshot for primary thread resolution".into(),
                ));
            }

            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut target_tid = 0;
            if Thread32First(snapshot, &mut entry) != 0 {
                loop {
                    if entry.th32OwnerProcessID == pid {
                        target_tid = entry.th32ThreadID;
                        break;
                    }
                    if Thread32Next(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snapshot);

            if target_tid == 0 {
                let _ = TerminateProcess(proc_handle, 1);
                CloseHandle(job);
                return Err(ProcessError::Custom(
                    "could not locate primary thread for child process".into(),
                ));
            }

            let thread_handle = OpenThread(THREAD_SUSPEND_RESUME, 0, target_tid);
            if thread_handle.is_null() {
                let _ = TerminateProcess(proc_handle, 1);
                CloseHandle(job);
                return Err(ProcessError::Custom(
                    "failed to open primary thread for child process".into(),
                ));
            }

            let resume_res = ResumeThread(thread_handle);
            CloseHandle(thread_handle);
            if resume_res == u32::MAX {
                let _ = TerminateProcess(proc_handle, 1);
                CloseHandle(job);
                return Err(ProcessError::Custom(
                    "ResumeThread failed on child primary thread".into(),
                ));
            }

            Ok(TrackedChild {
                command: cmd_str,
                pid,
                child,
                job_handle: job,
                terminal: None,
                os_calls: 0,
            })
        }
    }
}

type ChildList = Arc<Mutex<Vec<Arc<Mutex<TrackedChild>>>>>;

/// Global/app process tree manager to ensure all spawned children are cleaned up.
#[derive(Clone, Default)]
pub struct ProcessTreeManager {
    children: ChildList,
    stopped: Arc<AtomicBool>,
    ancestors: Vec<Arc<AtomicBool>>,
    parents: Vec<ChildList>,
    lifecycle: Arc<Mutex<()>>,
}

impl ProcessTreeManager {
    pub fn new() -> Self {
        Self {
            children: Arc::new(Mutex::new(Vec::new())),
            stopped: Arc::new(AtomicBool::new(false)),
            ancestors: Vec::new(),
            parents: Vec::new(),
            lifecycle: Arc::new(Mutex::new(())),
        }
    }

    /// A terminal child scope cancels its descendants, not siblings. The root retains
    /// every process for app-wide shutdown, including nested scopes.
    pub fn sub_manager(&self) -> Self {
        let mut ancestors = self.ancestors.clone();
        ancestors.push(self.stopped.clone());
        let mut parents = self.parents.clone();
        parents.push(self.children.clone());
        Self {
            children: Arc::new(Mutex::new(Vec::new())),
            stopped: Arc::new(AtomicBool::new(false)),
            ancestors,
            parents,
            lifecycle: Arc::clone(&self.lifecycle),
        }
    }

    pub fn spawn(&self, opts: SpawnOptions) -> Result<Arc<Mutex<TrackedChild>>, ProcessError> {
        // Serialize spawn/publication with terminal shutdown so no child can be
        // created after the shutdown sweep, including by a background installer.
        let _lifecycle = self.lifecycle.lock();
        let mut list = self.children.lock();
        if self.is_shutdown() {
            return Err(ProcessError::Custom(
                "Process owner is shutting down".into(),
            ));
        }
        let tracked = spawn_tracked(opts)?;
        let arc_child = Arc::new(Mutex::new(tracked));
        // Prune exited children whose groups/jobs are also dead
        list.retain(|c| {
            if let Some(mut lock) = c.try_lock() {
                lock.is_alive()
            } else {
                true
            }
        });
        list.push(Arc::clone(&arc_child));
        for parent in &self.parents {
            let mut parent_list = parent.lock();
            parent_list.retain(|c| {
                if let Some(mut lock) = c.try_lock() {
                    lock.is_alive()
                } else {
                    true
                }
            });
            parent_list.push(Arc::clone(&arc_child));
        }
        Ok(arc_child)
    }

    pub fn terminate_all(&self, drain_timeout: Duration) {
        let _lifecycle = self.lifecycle.lock();
        self.terminate_all_locked(drain_timeout);
    }

    fn terminate_all_locked(&self, drain_timeout: Duration) -> Vec<TerminationReport> {
        let list = {
            let mut l = self.children.lock();
            std::mem::take(&mut *l)
        };
        let mut survivors = Vec::new();
        let mut terminated = Vec::new();
        let mut reports = Vec::new();
        for child_arc in list {
            let mut child = child_arc.lock();
            match child.terminate_verified(drain_timeout) {
                Ok(report) => reports.push(report),
                Err(_) => reports.push(TerminationReport {
                    forced: true,
                    direct_child_exited: false,
                    group_empty: false,
                    remaining: Vec::new(),
                }),
            }
            if child.is_alive() {
                survivors.push(Arc::clone(&child_arc));
            } else {
                terminated.push(Arc::clone(&child_arc));
            }
        }
        for parent in &self.parents {
            parent
                .lock()
                .retain(|child| !terminated.iter().any(|done| Arc::ptr_eq(child, done)));
        }
        // Preserve ownership if cleanup fails; active_count must not claim success.
        self.children.lock().extend(survivors);
        reports
    }

    /// Terminal cancellation shared by all clones; unlike terminate_all, rejects future spawns.
    pub fn shutdown(&self, drain_timeout: Duration) {
        let _ = self.shutdown_verified(drain_timeout);
    }

    /// Seals the scope against any future spawn (including from clones held by background
    /// threads), then terminates and verifies every owned tree. The returned report lists
    /// one verified outcome per owned child; an unverifiable child is reported not empty.
    pub fn shutdown_verified(&self, drain_timeout: Duration) -> ScopeTermination {
        let _lifecycle = self.lifecycle.lock();
        self.stopped.store(true, Ordering::Release);
        ScopeTermination {
            children: self.terminate_all_locked(drain_timeout),
        }
    }

    /// Seals the scope against future spawns without terminating anything.
    pub fn seal(&self) {
        let _lifecycle = self.lifecycle.lock();
        self.stopped.store(true, Ordering::Release);
    }

    /// Observes every owned tree without signalling or reaping it. Escapes are read
    /// while their parents are still alive, so call this before any termination.
    /// Serialized with spawn and shutdown: with the scope sealed the answer cannot go
    /// stale through a new spawn.
    pub fn observe(&self) -> ScopeObservation {
        let _lifecycle = self.lifecycle.lock();
        let list: Vec<_> = self.children.lock().iter().cloned().collect();
        let mut observation = ScopeObservation {
            live_children: 0,
            termination: TerminationReport {
                forced: false,
                direct_child_exited: true,
                group_empty: true,
                remaining: Vec::new(),
            },
            escaped: Vec::new(),
        };
        for child_arc in list {
            let mut child = child_arc.lock();
            if child.is_terminated() {
                continue;
            }
            observation.escaped.extend(child.escaped_descendants());
            let exited = matches!(child.child_mut().try_wait(), Ok(Some(_)));
            let membership = child.group_membership();
            if exited && membership == GroupMembership::Empty {
                continue;
            }
            observation.live_children += 1;
            observation.termination.direct_child_exited &= exited;
            match membership {
                GroupMembership::Empty => {}
                GroupMembership::Members(pids) => {
                    observation.termination.group_empty = false;
                    observation.termination.remaining.extend(pids);
                }
                GroupMembership::Present => observation.termination.group_empty = false,
            }
        }
        observation
    }

    pub fn is_shutdown(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
            || self
                .ancestors
                .iter()
                .any(|flag| flag.load(Ordering::Acquire))
    }

    pub fn active_count(&self) -> usize {
        let mut list = self.children.lock();
        list.retain(|c| {
            if let Some(mut lock) = c.try_lock() {
                lock.is_alive()
            } else {
                true
            }
        });
        list.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::sync::Barrier;

    #[test]
    fn test_child_environment_allowlist_isolation() {
        let env = ChildEnvironment::default_allowlist();
        assert!(env.get("PATH").is_some() || env.get("Path").is_some());

        let mut cmd = Command::new("sh");
        env.apply_to_command(&mut cmd);
        // Ensure no panics, env applied
    }

    #[test]
    fn test_child_environment_prepend_path() {
        let mut env = ChildEnvironment::empty();
        env.set("PATH", "/usr/bin");
        env.prepend_path("/custom/bin");

        #[cfg(unix)]
        assert_eq!(env.get("PATH"), Some("/custom/bin:/usr/bin"));
    }

    /// The host allowlist stores `PATH`; on Windows every prepend must build on the last
    /// one (toolchain, then FFmpeg DLLs) and leave a single spelling for the child.
    #[cfg(windows)]
    #[test]
    fn test_child_environment_repeated_prepend_keeps_every_entry_on_windows() {
        let mut env = ChildEnvironment::empty();
        env.set("PATH", r"C:\host");
        env.prepend_path(r"C:\sdk\toolchain\bin");
        env.prepend_path(r"C:\sdk\ffmpeg\bin");

        assert_eq!(
            env.get("Path"),
            Some(r"C:\sdk\ffmpeg\bin;C:\sdk\toolchain\bin;C:\host")
        );
        assert_eq!(
            env.iter()
                .filter(|(key, _)| key.eq_ignore_ascii_case("PATH"))
                .count(),
            1
        );
    }

    #[test]
    fn test_child_environment_redaction() {
        let mut env = ChildEnvironment::empty();
        env.set("NORMAL_VAR", "value123");
        env.set("API_KEY", "super_secret_key");
        env.set("MY_TOKEN", "bearer_secret");

        let redacted = env.export_redacted();
        assert_eq!(redacted.get("NORMAL_VAR"), Some(&"value123".to_string()));
        assert_eq!(redacted.get("API_KEY"), Some(&"[REDACTED]".to_string()));
        assert_eq!(redacted.get("MY_TOKEN"), Some(&"[REDACTED]".to_string()));
    }

    #[test]
    #[cfg(unix)]
    fn test_tracked_child_graceful_termination() {
        let mut opts = SpawnOptions::new("sleep");
        opts.arg("30");
        let mut child = spawn_tracked(opts).expect("spawn sleep");
        assert!(child.pid() > 0);

        // Terminate within 500ms
        let start = Instant::now();
        child
            .terminate_gracefully(Duration::from_millis(500))
            .expect("terminate gracefully");
        assert!(start.elapsed() < Duration::from_secs(2));

        let status = child.try_wait().expect("wait status");
        assert!(status.is_some());
    }

    #[test]
    #[cfg(unix)]
    fn test_process_tree_manager_cleanup() {
        let manager = ProcessTreeManager::new();
        let mut opts1 = SpawnOptions::new("sleep");
        opts1.arg("30");
        let _child1 = manager.spawn(opts1).expect("spawn child 1");

        let mut opts2 = SpawnOptions::new("sleep");
        opts2.arg("30");
        let _child2 = manager.spawn(opts2).expect("spawn child 2");

        assert_eq!(manager.active_count(), 2);
        manager.terminate_all(Duration::from_millis(200));
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn terminal_shutdown_reaps_and_prevents_background_clone_from_spawning() {
        let manager = ProcessTreeManager::new();
        let background = manager.clone();
        let mut options = SpawnOptions::new("sleep");
        options.arg("30");
        let child = background.spawn(options).unwrap();
        manager.shutdown(Duration::ZERO);
        assert!(child.lock().try_wait().unwrap().is_some());
        assert!(background.spawn(SpawnOptions::new("sleep")).is_err());
        assert_eq!(background.active_count(), 0);
    }

    #[test]
    #[cfg(unix)]
    fn concurrent_scoped_spawn_and_parent_shutdown_never_leaves_a_live_child() {
        for _ in 0..32 {
            let parent = ProcessTreeManager::new();
            let scoped = parent.sub_manager();
            let barrier = Arc::new(Barrier::new(2));
            let spawn_barrier = Arc::clone(&barrier);
            let spawn_thread = std::thread::spawn(move || {
                let mut options = SpawnOptions::new("sleep");
                options.arg("30");
                spawn_barrier.wait();
                scoped.spawn(options)
            });

            barrier.wait();
            std::thread::sleep(Duration::from_micros(100));
            parent.shutdown(Duration::ZERO);
            let spawned = spawn_thread.join().expect("spawn thread panicked");

            // Inspect the first shutdown's outcome before cleanup: a second sweep
            // could hide a child published after the first sweep returned.
            let child_exited = spawned
                .as_ref()
                .map(|child| child.lock().try_wait().expect("wait status").is_some())
                .unwrap_or(true);
            let tracked = parent.active_count();
            // Always clean up before assertions so a failed expectation cannot leak a process.
            parent.shutdown(Duration::ZERO);

            assert!(child_exited, "successful concurrent spawn remained alive");
            assert_eq!(tracked, 0, "parent retained a live concurrent child");
        }
    }

    #[test]
    #[cfg(unix)]
    fn sub_manager_scopes_cleanup_while_parent_retains_shutdown_ownership() {
        let parent = ProcessTreeManager::new();
        let mut p_opts = SpawnOptions::new("sleep");
        p_opts.arg("30");
        let parent_child = parent.spawn(p_opts).unwrap();

        let sub = parent.sub_manager();
        let mut s_opts = SpawnOptions::new("sleep");
        s_opts.arg("30");
        let sub_child = sub.spawn(s_opts).unwrap();

        // Sub-manager terminates only its own scoped child
        sub.terminate_all(Duration::from_millis(100));
        let sub_exited = sub_child.lock().try_wait().unwrap().is_some();
        let parent_survived = parent_child.lock().try_wait().unwrap().is_none();
        let sub_count = sub.active_count();
        let parent_count = parent.active_count();

        // Parent shutdown terminates remaining children and cancels sub
        parent.shutdown(Duration::ZERO);
        let parent_exited = parent_child.lock().try_wait().unwrap().is_some();

        assert!(sub_exited);
        assert!(parent_survived);
        assert_eq!(sub_count, 0);
        assert_eq!(parent_count, 1);
        assert!(parent_exited);
        assert!(sub.is_shutdown());
        assert!(sub.spawn(SpawnOptions::new("sleep")).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn nested_scope_cleanup_preserves_unrelated_children() {
        let parent = ProcessTreeManager::new();
        let sibling = parent.sub_manager();
        let nested = parent.sub_manager().sub_manager();

        let mut sibling_options = SpawnOptions::new("sleep");
        sibling_options.arg("30");
        let sibling_child = sibling.spawn(sibling_options).unwrap();
        let mut nested_options = SpawnOptions::new("sleep");
        nested_options.arg("30");
        let nested_child = nested.spawn(nested_options).unwrap();

        nested.terminate_all(Duration::ZERO);
        let nested_exited = nested_child.lock().try_wait().unwrap().is_some();
        let sibling_survived = sibling_child.lock().try_wait().unwrap().is_none();
        let parent_count = parent.active_count();

        // Clean up both scopes before asserting observations.
        parent.shutdown(Duration::ZERO);
        let sibling_exited = sibling_child.lock().try_wait().unwrap().is_some();

        assert!(nested_exited);
        assert!(sibling_survived);
        assert_eq!(parent_count, 1);
        assert!(sibling_exited);
    }

    #[test]
    #[cfg(unix)]
    fn terminal_operation_scope_reaps_nested_children_without_cancelling_displayed_scope() {
        let root = ProcessTreeManager::new();
        let operation = root.sub_manager();
        let nested = operation.sub_manager();
        let displayed = root.sub_manager();
        let mut opts = SpawnOptions::new("sleep");
        opts.arg("30");
        let child = nested.spawn(opts).unwrap();
        operation.shutdown(Duration::ZERO);
        assert!(child.lock().try_wait().unwrap().is_some());
        assert!(nested.is_shutdown());
        assert!(nested.spawn(SpawnOptions::new("sleep")).is_err());
        assert!(!displayed.is_shutdown());
        let mut opts = SpawnOptions::new("sleep");
        opts.arg("30");
        let live = displayed.spawn(opts).unwrap();
        root.shutdown(Duration::ZERO);
        assert!(live.lock().try_wait().unwrap().is_some());
        assert!(displayed.is_shutdown());
        assert_eq!(root.active_count(), 0);
    }

    #[cfg(unix)]
    fn shell(script: &str) -> TrackedChild {
        let mut opts = SpawnOptions::new("sh");
        opts.arg("-c").arg(script);
        spawn_tracked(opts).expect("spawn shell")
    }

    #[cfg(unix)]
    fn read_pid_line(child: &mut TrackedChild) -> i32 {
        use std::io::{BufRead, BufReader};
        let stdout = child.child_mut().stdout.take().expect("piped stdout");
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        line.trim().parse().expect("helper pid line")
    }

    #[cfg(unix)]
    fn pid_alive(pid: i32) -> bool {
        // A zombie still answers signal 0 but cannot write; /proc distinguishes it.
        // SAFETY: signal 0 probes existence only.
        let exists = unsafe { libc::kill(pid, 0) } == 0;
        exists
            && std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|s| !s.rsplit(") ").next().unwrap_or("").starts_with('Z'))
                .unwrap_or(false)
    }

    #[cfg(target_os = "linux")]
    fn wait_for_escaped_descendant(mut observe: impl FnMut() -> Vec<u32>, helper: u32) -> Vec<u32> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let escaped = observe();
            if escaped.contains(&helper) {
                return escaped;
            }
            assert!(
                Instant::now() < deadline,
                "setsid helper {helper} never left its parent's process group"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    #[cfg(unix)]
    fn forced_termination_verifies_term_ignoring_group_members_are_gone() {
        let mut child = shell("trap '' TERM; sleep 30 & echo $!; wait");
        let helper = read_pid_line(&mut child);
        let report = child
            .terminate_verified(Duration::from_millis(100))
            .expect("terminates");
        assert!(report.forced, "TERM-ignoring tree needs a forced kill");
        assert!(report.verified(), "{report:?}");
        assert!(report.remaining.is_empty());
        assert!(!pid_alive(helper));
    }

    #[test]
    #[cfg(unix)]
    fn dead_leader_with_surviving_group_member_is_not_reported_clean() {
        let mut child = shell("sleep 30 & echo $!; exit 0");
        let helper = read_pid_line(&mut child);
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        // The direct child has exited but a group member is alive.
        let membership = child.group_membership();
        assert!(matches!(
            &membership,
            GroupMembership::Members(pids) if pids.contains(&(helper as u32))
        ));
        assert!(child.is_alive());
        let report = child
            .terminate_verified(Duration::from_millis(500))
            .expect("terminates");
        assert!(report.verified(), "{report:?}");
        assert!(!pid_alive(helper));
        assert!(!child.is_alive());
    }

    #[test]
    #[cfg(unix)]
    fn setsid_escaped_helper_survives_group_verification_and_is_observable_only_while_parented() {
        let mut child = shell("setsid sleep 30 & echo $!; wait");
        let helper = read_pid_line(&mut child);
        // Linux can see the escape while the helper is still a descendant.
        #[cfg(target_os = "linux")]
        assert_eq!(
            wait_for_escaped_descendant(|| child.escaped_descendants(), helper as u32),
            vec![helper as u32]
        );
        let report = child
            .terminate_verified(Duration::from_millis(500))
            .expect("terminates");
        let survived = pid_alive(helper);
        // Clean up only the known test-owned helper before asserting.
        // SAFETY: helper is the pid this test spawned and read from the shell.
        unsafe {
            libc::kill(helper, libc::SIGKILL);
        }
        assert!(
            report.verified(),
            "group verification only covers the group: {report:?}"
        );
        assert!(survived, "setsid helper escapes the process group");
    }

    #[test]
    fn writer_ownership_is_qualified_only_for_process_group_containment() {
        assert!(
            WriterOwnership::ProcessGroupContained {
                qualification: "fixture".into()
            }
            .is_qualified()
        );
        assert!(!WriterOwnership::Detached.is_qualified());
        assert!(!WriterOwnership::Unknown.is_qualified());
    }

    #[cfg(target_os = "linux")]
    fn stat_line(pid: u32, comm: &str, state: &str, ppid: u32, pgrp: i32, start: u64) -> String {
        // pid (comm) state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt
        // utime stime cutime cstime priority nice threads itrealvalue starttime ...
        format!(
            "{pid} ({comm}) {state} {ppid} {pgrp} 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {start} 0 0"
        )
    }

    #[cfg(target_os = "linux")]
    fn entry(pid: u32, ppid: u32, pgrp: i32, start: u64, zombie: bool) -> ProcEntry {
        ProcEntry {
            pid,
            ppid,
            pgrp,
            start,
            zombie,
        }
    }

    #[cfg(target_os = "linux")]
    fn complete(entries: Vec<ProcEntry>) -> ProcScan {
        ProcScan {
            entries,
            complete: true,
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn proc_stat_parser_handles_spaces_parentheses_and_reads_start_time() {
        let entry = parse_proc_stat(7, &stat_line(7, "we ird) name", "S", 1, 42, 9001)).unwrap();
        assert_eq!(
            (entry.ppid, entry.pgrp, entry.start, entry.zombie),
            (1, 42, 9001, false)
        );
        assert!(
            parse_proc_stat(8, &stat_line(8, "x", "Z", 1, 9, 5))
                .unwrap()
                .zombie
        );
        // A truncated line is a parse failure, which makes a scan incomplete.
        assert!(parse_proc_stat(8, "8 (x) Z 1 9 9").is_none());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn unreadable_scan_is_unverified_present_never_empty() {
        let mut probes = 0;
        let membership = membership_with(
            500,
            Some(1),
            &mut || {
                probes += 1;
                Probe::Exists
            },
            // Injected scan failure: the group is known to exist but cannot be enumerated.
            &mut || ProcScan {
                entries: vec![entry(1, 0, 1, 1, false)],
                complete: false,
            },
            &|| {},
        );
        assert_eq!(membership, GroupMembership::Present);
        // The same scan that merely lacks the group still cannot clear it.
        assert_eq!(
            classify_scan(
                &ProcScan {
                    entries: vec![],
                    complete: false
                },
                500,
                None
            ),
            ScanVerdict::Incomplete
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn zombie_only_group_needs_two_quiet_scans_and_a_fork_between_them_is_seen() {
        // Scan 1 sees only a zombie leader; before scan 2 a live member appears
        // (a fork racing the enumeration). The group is not empty.
        let mut scans = vec![
            complete(vec![entry(500, 1, 500, 77, true)]),
            complete(vec![
                entry(500, 1, 500, 77, true),
                entry(501, 500, 500, 90, false),
            ]),
        ]
        .into_iter();
        let membership = membership_with(
            500,
            Some(77),
            &mut || Probe::Exists,
            &mut || scans.next().expect("no more than two scans"),
            &|| {},
        );
        assert_eq!(membership, GroupMembership::Members(vec![501]));

        // Two consecutive quiet scans clear it.
        let quiet = complete(vec![entry(500, 1, 500, 77, true)]);
        let membership = membership_with(
            500,
            Some(77),
            &mut || Probe::Exists,
            &mut || quiet.clone(),
            &|| {},
        );
        assert_eq!(membership, GroupMembership::Empty);
        // Kernel says gone: empty without scanning at all.
        let membership = membership_with(
            500,
            Some(77),
            &mut || Probe::Gone,
            &mut || panic!("no scan needed"),
            &|| {},
        );
        assert_eq!(membership, GroupMembership::Empty);
        let membership = membership_with(
            500,
            Some(77),
            &mut || Probe::Unknown,
            &mut || panic!("no scan needed"),
            &|| {},
        );
        assert_eq!(membership, GroupMembership::Present);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_recycled_group_id_is_foreign_and_never_ours() {
        // Same numeric id, different leader start time: an unrelated process family.
        let recycled = complete(vec![entry(500, 1, 500, 99_999, false)]);
        assert_eq!(
            classify_scan(&recycled, 500, Some(77)),
            ScanVerdict::Foreign
        );
        let membership = membership_with(
            500,
            Some(77),
            &mut || Probe::Exists,
            &mut || recycled.clone(),
            &|| {},
        );
        assert_eq!(membership, GroupMembership::Empty);
        assert!(escaped_in_scan(&recycled, 500, 500, Some(77)).is_empty());
        // Our own leader is recognised.
        let ours = complete(vec![entry(500, 1, 500, 77, false)]);
        assert_eq!(
            classify_scan(&ours, 500, Some(77)),
            ScanVerdict::Members(vec![500])
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn escape_detection_follows_descendants_and_ignores_incomplete_scans() {
        let scan = complete(vec![
            entry(500, 1, 500, 77, false),
            entry(501, 500, 500, 78, false),
            entry(502, 501, 700, 79, false),
            entry(503, 501, 700, 80, true),
        ]);
        assert_eq!(escaped_in_scan(&scan, 500, 500, Some(77)), vec![502]);
        let mut partial = scan;
        partial.complete = false;
        assert!(escaped_in_scan(&partial, 500, 500, Some(77)).is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn a_verified_terminated_tracker_is_inert_and_never_probes_or_signals_again() {
        let mut child = shell("sleep 30 & wait");
        let report = child
            .terminate_verified(Duration::from_millis(200))
            .unwrap();
        assert!(report.verified());
        assert!(child.is_terminated());
        let calls = child.os_calls();
        assert!(calls > 0);
        // Everything below runs while the numeric group id could already belong to
        // someone else. None of it may touch the OS again.
        assert_eq!(
            child.terminate_verified(Duration::from_millis(50)).unwrap(),
            report
        );
        child
            .terminate_gracefully(Duration::from_millis(50))
            .unwrap();
        child.kill_forcefully().unwrap();
        assert!(!child.is_alive());
        assert_eq!(child.group_membership(), GroupMembership::Empty);
        assert!(child.escaped_descendants().is_empty());
        assert!(child.try_wait().unwrap().is_some());
        assert_eq!(
            child.os_calls(),
            calls,
            "no probe or signal after the latch"
        );
        drop(child);
    }

    #[test]
    #[cfg(unix)]
    fn naturally_exited_group_latches_when_observed_and_is_never_signalled() {
        let mut child = shell("exit 0");
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.is_alive() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(child.try_wait().unwrap().is_some());
        assert!(child.is_terminated());
        let calls = child.os_calls();
        child.kill_forcefully().unwrap();
        assert_eq!(child.os_calls(), calls);
    }

    #[test]
    #[cfg(unix)]
    fn termination_is_bounded_even_when_the_leader_ignores_term() {
        let mut child = shell("trap '' TERM; while :; do sleep 1; done");
        let started = Instant::now();
        let report = child
            .terminate_verified(Duration::from_millis(100))
            .unwrap();
        assert!(report.forced && report.verified(), "{report:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    #[cfg(unix)]
    fn verified_scope_shutdown_seals_first_and_reports_every_tree() {
        let scope = ProcessTreeManager::new().sub_manager();
        let background = scope.clone();
        let mut options = SpawnOptions::new("sh");
        options.arg("-c").arg("sleep 30 & wait");
        let child = scope.spawn(options).unwrap();
        let report = scope.shutdown_verified(Duration::from_millis(200));
        assert_eq!(report.children.len(), 1);
        assert!(report.verified(), "{report:?}");
        assert!(child.lock().is_terminated());
        // A clone held elsewhere can no longer spawn into the sealed scope.
        assert!(background.spawn(SpawnOptions::new("sleep")).is_err());
        assert!(background.is_shutdown());
        // Sealing alone terminates nothing.
        let other = ProcessTreeManager::new();
        let mut options = SpawnOptions::new("sleep");
        options.arg("30");
        let live = other.spawn(options).unwrap();
        other.seal();
        assert!(other.spawn(SpawnOptions::new("sleep")).is_err());
        assert!(live.lock().try_wait().unwrap().is_none());
        other.shutdown(Duration::ZERO);
    }

    #[test]
    #[cfg(unix)]
    fn scope_observation_reports_live_members_without_terminating_anything() {
        let scope = ProcessTreeManager::new().sub_manager();
        assert!(scope.observe().is_clean(), "an empty scope is clean");
        let mut options = SpawnOptions::new("sleep");
        options.arg("30");
        let child = scope.spawn(options).unwrap();
        let observed = scope.observe();
        assert_eq!(observed.live_children, 1);
        assert!(!observed.is_clean());
        assert!(!observed.termination.group_empty);
        assert!(!observed.termination.direct_child_exited);
        assert_eq!(observed.termination.remaining, vec![child.lock().pid()]);
        assert!(
            child.lock().try_wait().unwrap().is_none(),
            "observing never signals or reaps"
        );
        let merged = scope.shutdown_verified(Duration::from_millis(200)).merged();
        assert!(
            merged.verified() && merged.remaining.is_empty(),
            "{merged:?}"
        );
        assert!(scope.observe().is_clean());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn scope_observation_sees_a_setsid_escape_before_the_kill() {
        let scope = ProcessTreeManager::new().sub_manager();
        let mut options = SpawnOptions::new("sh");
        options.arg("-c").arg("setsid sleep 30 & echo $!; wait");
        let child = scope.spawn(options).unwrap();
        let helper = {
            use std::io::{BufRead, BufReader};
            let stdout = child
                .lock()
                .child_mut()
                .stdout
                .take()
                .expect("piped stdout");
            let mut line = String::new();
            BufReader::new(stdout).read_line(&mut line).unwrap();
            line.trim().parse::<u32>().expect("helper pid line")
        };
        let observed = wait_for_escaped_descendant(|| scope.observe().escaped, helper);
        assert_eq!(observed, vec![helper]);
        assert!(!scope.observe().is_clean());
        let termination = scope.shutdown_verified(Duration::from_millis(500));
        // SAFETY: helper is the pid this test spawned and read from the shell.
        unsafe {
            libc::kill(helper as i32, libc::SIGKILL);
        }
        assert!(
            termination.verified(),
            "group verification alone cannot see the escape: {termination:?}"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_membership_ignores_zombies_but_requires_two_complete_scans() {
        let zombie_group = MacProcScan {
            entries: vec![MacProcEntry {
                pid: 500,
                pgrp: 500,
                start: (10, 20),
                zombie: true,
            }],
            complete: true,
        };
        let mut scans = 0;
        let membership = mac_membership_with(
            500,
            Some((10, 20)),
            &mut || Probe::Exists,
            &mut || {
                scans += 1;
                zombie_group.clone()
            },
            &|| {},
        );
        assert_eq!(membership, GroupMembership::Empty);
        assert_eq!(scans, 2, "one quiet snapshot is not enough to prove empty");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_membership_preserves_live_and_unverifiable_groups() {
        let live_group = MacProcScan {
            entries: vec![MacProcEntry {
                pid: 501,
                pgrp: 500,
                start: (10, 21),
                zombie: false,
            }],
            complete: true,
        };
        assert_eq!(
            mac_membership_with(
                500,
                Some((10, 20)),
                &mut || Probe::Exists,
                &mut || live_group.clone(),
                &|| {},
            ),
            GroupMembership::Members(vec![501])
        );

        let incomplete = MacProcScan {
            entries: Vec::new(),
            complete: false,
        };
        assert_eq!(
            mac_membership_with(
                500,
                Some((10, 20)),
                &mut || Probe::Exists,
                &mut || incomplete.clone(),
                &|| {},
            ),
            GroupMembership::Present
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_membership_does_not_claim_a_recycled_group_id() {
        let recycled = MacProcScan {
            entries: vec![MacProcEntry {
                pid: 500,
                pgrp: 500,
                start: (99, 1),
                zombie: false,
            }],
            complete: true,
        };
        assert_eq!(
            mac_membership_with(
                500,
                Some((10, 20)),
                &mut || Probe::Exists,
                &mut || recycled.clone(),
                &|| {},
            ),
            GroupMembership::Empty
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_libproc_zero_returns_are_disambiguated_by_errno() {
        assert!(!mac_process_group_list_failed(0, 0));
        assert!(mac_process_group_list_failed(0, libc::EACCES));
        assert!(mac_process_group_list_failed(-1, 0));
        assert!(mac_process_info_confirms_exit(0, libc::ESRCH));
        assert!(!mac_process_info_confirms_exit(0, libc::EACCES));
        assert!(!mac_process_info_confirms_exit(0, 0));
    }

    #[test]
    fn merged_termination_is_clean_only_if_every_tree_is() {
        let clean = TerminationReport {
            forced: false,
            direct_child_exited: true,
            group_empty: true,
            remaining: vec![],
        };
        assert!(ScopeTermination { children: vec![] }.merged().verified());
        let dirty = TerminationReport {
            forced: true,
            direct_child_exited: true,
            group_empty: false,
            remaining: vec![9],
        };
        let merged = ScopeTermination {
            children: vec![clean, dirty],
        }
        .merged();
        assert!(merged.forced && !merged.verified());
        assert_eq!(merged.remaining, vec![9]);
    }
}
