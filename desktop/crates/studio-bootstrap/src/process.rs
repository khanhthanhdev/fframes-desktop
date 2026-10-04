use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;
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
            "WINDIR",
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

    /// Prepend a path directory to PATH within this child environment,
    /// without modifying the host process PATH.
    pub fn prepend_path(&mut self, path: impl AsRef<Path>) -> &mut Self {
        let path_str = path.as_ref().to_string_lossy().to_string();
        let path_key = if cfg!(windows) { "Path" } else { "PATH" };

        let current_path = self
            .vars
            .get("PATH")
            .or_else(|| self.vars.get("Path"))
            .cloned()
            .unwrap_or_default();
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

#[cfg(unix)]
fn is_process_group_alive(pgid: i32) -> bool {
    let ret = unsafe { libc::kill(-pgid, 0) };
    if ret == 0 {
        true
    } else {
        let err = std::io::Error::last_os_error().raw_os_error();
        err != Some(libc::ESRCH)
    }
}

pub struct TrackedChild {
    pub command: String,
    pub pid: u32,
    #[cfg(unix)]
    pgid: i32,
    child: Child,
    #[cfg(windows)]
    job_handle: windows_sys::Win32::Foundation::HANDLE,
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

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, ProcessError> {
        Ok(self.child.try_wait()?)
    }

    pub fn wait(&mut self) -> Result<ExitStatus, ProcessError> {
        Ok(self.child.wait()?)
    }

    pub fn is_alive(&mut self) -> bool {
        #[cfg(unix)]
        {
            if matches!(self.child.try_wait(), Ok(None)) {
                return true;
            }
            is_process_group_alive(self.pgid)
        }
        #[cfg(windows)]
        {
            if matches!(self.child.try_wait(), Ok(None)) {
                return true;
            }
            unsafe {
                let mut accounting: windows_sys::Win32::System::JobObjects::JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                let mut ret_len = 0;
                let res = windows_sys::Win32::System::JobObjects::QueryInformationJobObject(
                    self.job_handle,
                    windows_sys::Win32::System::JobObjects::JobObjectBasicAccountingInformation,
                    &mut accounting as *mut _ as _,
                    std::mem::size_of_val(&accounting) as u32,
                    &mut ret_len,
                );
                // A failed query cannot prove the owned descendants exited.
                res == 0 || accounting.ActiveProcesses > 0
            }
        }
    }

    /// Gracefully terminate the child process and its entire process tree/group.
    /// Sends SIGTERM (or closes job on Windows), allows up to `drain_timeout` for exit,
    /// then forces SIGKILL if any processes in the tree remain.
    pub fn terminate_gracefully(&mut self, _drain_timeout: Duration) -> Result<(), ProcessError> {
        #[cfg(unix)]
        {
            let child_exited = matches!(self.child.try_wait(), Ok(Some(_)));
            if child_exited && !is_process_group_alive(self.pgid) {
                return Ok(());
            }

            // Send SIGTERM to the process group
            unsafe {
                libc::kill(-self.pgid, libc::SIGTERM);
            }

            let start = Instant::now();
            let poll_interval = Duration::from_millis(20);
            while start.elapsed() < _drain_timeout {
                let _ = self.child.try_wait();
                if !is_process_group_alive(self.pgid) {
                    let _ = self.child.wait();
                    return Ok(());
                }
                std::thread::sleep(poll_interval);
            }

            // Still alive after drain_timeout: send SIGKILL to the whole group
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
            let _ = self.child.wait();
            Ok(())
        }

        #[cfg(windows)]
        {
            unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job_handle, 1);
            }
            let _ = self.child.wait();
            Ok(())
        }
    }

    /// Forcefully kill the entire process tree immediately.
    pub fn kill_forcefully(&mut self) -> Result<(), ProcessError> {
        #[cfg(unix)]
        {
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
            let _ = self.child.wait();
            Ok(())
        }

        #[cfg(windows)]
        {
            unsafe {
                windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job_handle, 1);
            }
            let _ = self.child.wait();
            Ok(())
        }
    }
}

#[cfg(unix)]
impl Drop for TrackedChild {
    fn drop(&mut self) {
        if is_process_group_alive(self.pgid) {
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
            let _ = self.child.wait();
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

        Ok(TrackedChild {
            command: cmd_str,
            pid,
            pgid,
            child,
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

            // Spawn suspended so it cannot execute or break away before assignment
            command.creation_flags(CREATE_SUSPENDED);
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

    fn terminate_all_locked(&self, drain_timeout: Duration) {
        let list = {
            let mut l = self.children.lock();
            std::mem::take(&mut *l)
        };
        let mut survivors = Vec::new();
        let mut terminated = Vec::new();
        for child_arc in list {
            let mut child = child_arc.lock();
            let _ = child.terminate_gracefully(drain_timeout);
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
    }

    /// Terminal cancellation shared by all clones; unlike terminate_all, rejects future spawns.
    pub fn shutdown(&self, drain_timeout: Duration) {
        let _lifecycle = self.lifecycle.lock();
        self.stopped.store(true, Ordering::Release);
        self.terminate_all_locked(drain_timeout);
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
}
