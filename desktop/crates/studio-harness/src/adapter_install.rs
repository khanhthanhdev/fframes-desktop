//! Explicit, pinned install of an npm-distributed ACP adapter into the app data folder.
//!
//! The install runs only when the caller asks for it. It installs into a `.tmp-*` sibling and
//! renames into place after the adapter entry resolved and a marker was written, so a killed
//! or failed install never leaves a usable half-installed adapter, and an incomplete folder
//! left in place is moved aside by the next install. The adapter starts through
//! `node <entry>`, never `npx` and never an npm shim.

use crate::redact_sensitive_string;
use std::{
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime},
};
use studio_bootstrap::{ChildEnvironment, ProcessTreeManager, SpawnOptions, TrackedChild};

mod claude_patch;

/// An npm package that provides an ACP adapter, pinned to an exact version together with its
/// whole dependency tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterPackage {
    pub name: &'static str,
    pub version: &'static str,
    /// The `bin` entry of the package that starts the adapter.
    pub bin: &'static str,
    /// The `package.json` that depends on exactly `name@version`.
    pub manifest: &'static str,
    /// The `package-lock.json` that `npm ci` installs from.
    pub lockfile: &'static str,
}

/// Versions checked against the npm registry on 2026-10-12. To change a version, edit the
/// dependency in `adapters/<profile>/package.json` and `version` here, then run
/// `npm install --package-lock-only --ignore-scripts` in that folder to regenerate the lock.
pub const CLAUDE_ADAPTER: AdapterPackage = AdapterPackage {
    name: "@agentclientprotocol/claude-agent-acp",
    version: "0.89.1",
    bin: "claude-agent-acp",
    manifest: include_str!("../adapters/claude/package.json"),
    lockfile: include_str!("../adapters/claude/package-lock.json"),
};
pub const CODEX_ADAPTER: AdapterPackage = AdapterPackage {
    name: "@agentclientprotocol/codex-acp",
    version: "2.2.2",
    bin: "codex-acp",
    manifest: include_str!("../adapters/codex/package.json"),
    lockfile: include_str!("../adapters/codex/package-lock.json"),
};

/// The adapter package a first-party profile can install.
pub fn package_for(profile_id: &str) -> Option<AdapterPackage> {
    match profile_id {
        "claude" => Some(CLAUDE_ADAPTER),
        "codex" => Some(CODEX_ADAPTER),
        _ => None,
    }
}

const MARKER: &str = ".fframes-adapter";
const MAX_OUTPUT: usize = 8 * 1024;
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// An installed adapter: start it as `node <entry>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAdapter {
    pub node: PathBuf,
    pub entry: PathBuf,
}

/// The runtimes an install needs, resolved by the caller.
#[derive(Debug, Clone)]
pub struct InstallTools {
    pub node: PathBuf,
    pub npm: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InstallError {
    #[error("install cancelled")]
    Cancelled,
    #[error("install timed out after {0} seconds")]
    TimedOut(u64),
    #[error("npm failed: {0}")]
    Npm(String),
    #[error("{0}")]
    Invalid(String),
    #[error("could not write the install folder: {0}")]
    Io(String),
}

fn io(error: std::io::Error) -> InstallError {
    InstallError::Io(error.to_string())
}

fn package_dir(adapters: &Path, package: &AdapterPackage) -> PathBuf {
    adapters.join(package.name.replace(['/', '@'], "_"))
}

fn final_dir(adapters: &Path, package: &AdapterPackage) -> PathBuf {
    let version = if claude_patch::applies(package) {
        format!("{}-{}", package.version, claude_patch::REVISION)
    } else {
        package.version.to_owned()
    };
    package_dir(adapters, package).join(version)
}

/// The entry file a package's `bin` names, from its installed `package.json`.
fn resolve_entry(root: &Path, package: &AdapterPackage) -> Result<PathBuf, InstallError> {
    let module = root.join("node_modules").join(package.name);
    let manifest = std::fs::read_to_string(module.join("package.json")).map_err(io)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest)
        .map_err(|e| InstallError::Invalid(format!("package.json is invalid: {e}")))?;
    let relative = match &manifest["bin"] {
        serde_json::Value::String(path) => Some(path.as_str()),
        serde_json::Value::Object(bins) => bins.get(package.bin).and_then(|v| v.as_str()),
        _ => None,
    }
    .ok_or_else(|| {
        InstallError::Invalid(format!("the package has no \"{}\" executable", package.bin))
    })?;
    let entry = module.join(relative);
    // A bin path must stay inside the package.
    if relative.split(['/', '\\']).any(|part| part == "..") || !entry.is_file() {
        return Err(InstallError::Invalid(format!(
            "the package entry \"{relative}\" is missing"
        )));
    }
    Ok(entry)
}

/// Checkouts may turn the lock's line endings into CRLF.
fn same_lockfile(a: &str, b: &str) -> bool {
    a.lines().eq(b.lines())
}

/// The installed adapter, if a complete install of exactly this version, from exactly this
/// lockfile, exists. An install from an older lock of the same version is replaced.
pub fn find_installed(
    adapters: &Path,
    package: &AdapterPackage,
    node: &Path,
) -> Option<InstalledAdapter> {
    let root = final_dir(adapters, package);
    let marker = std::fs::read_to_string(root.join(MARKER)).ok()?;
    if marker.trim() != format!("{}@{}", package.name, package.version) {
        return None;
    }
    let lockfile = std::fs::read_to_string(root.join("package-lock.json")).ok()?;
    if !same_lockfile(&lockfile, package.lockfile) {
        return None;
    }
    if !claude_patch::installed(&root, package) {
        return None;
    }
    let entry = resolve_entry(&root, package).ok()?;
    Some(InstalledAdapter {
        node: node.to_owned(),
        entry,
    })
}

/// Bounded tail of a child's output, read on a thread so a full pipe never blocks it.
struct Output {
    tail: Arc<parking_lot::Mutex<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
}

fn drain(stream: Option<impl Read + Send + 'static>) -> Output {
    let tail = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let reader = stream.map(|mut stream| {
        let sink = Arc::clone(&tail);
        std::thread::spawn(move || {
            let mut chunk = [0_u8; 1024];
            while let Ok(read) = stream.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                let mut sink = sink.lock();
                sink.extend_from_slice(&chunk[..read]);
                let excess = sink.len().saturating_sub(MAX_OUTPUT);
                sink.drain(..excess);
            }
        })
    });
    Output { tail, reader }
}

/// How long the readers may take to reach the end of npm's output once npm is gone. A helper
/// that inherited the pipes and outlived npm would keep them open forever.
const READER_WAIT: Duration = Duration::from_secs(2);

/// What npm printed: stderr, or stdout when stderr is empty. Waits (bounded) for both readers
/// so the text is complete.
fn finish_output(stderr: Output, stdout: Output) -> String {
    let deadline = Instant::now() + READER_WAIT;
    let texts = [stderr, stdout].map(|output| {
        if let Some(reader) = output.reader {
            while !reader.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if reader.is_finished() {
                drop(reader.join());
            }
        }
        let text = String::from_utf8_lossy(&output.tail.lock()).into_owned();
        redact_sensitive_string(text.trim())
    });
    texts
        .into_iter()
        .find(|text| !text.is_empty())
        .unwrap_or_default()
}

/// The end of npm's debug log of this run. npm writes it into the cache folder, which is
/// deleted after the install, so the "complete log" path npm prints would point nowhere.
fn npm_log_tail(cache: &Path) -> Option<String> {
    const LOG_TAIL: u64 = 4 * 1024;
    let log = std::fs::read_dir(cache.join("_logs"))
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
        // Log names start with a timestamp, so the newest sorts last.
        .max()?;
    let mut file = std::fs::File::open(log).ok()?;
    let start = file.metadata().ok()?.len().saturating_sub(LOG_TAIL);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    // Skip the line the seek cut in half.
    let text = if start > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    Some(redact_sensitive_string(text.trim())).filter(|text| !text.is_empty())
}

/// npm's output for an error, with the end of its debug log in place of the log's path.
fn npm_detail(output: &str, cache: &Path) -> String {
    let mut detail: Vec<&str> = output
        .lines()
        .filter(|line| !line.contains("A complete log of this run can be found in"))
        .collect();
    let log = npm_log_tail(cache);
    if let Some(log) = &log {
        detail.extend(["npm log (end):", log]);
    }
    detail.join("\n").trim().to_owned()
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        ".tmp-{}-{}-{nanos}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// A `.tmp-*` folder younger than this may belong to an install that is still running.
const STALE_AFTER: Duration = Duration::from_secs(3600);

/// Only a folder positively known to be older than [`STALE_AFTER`] is stale. An unreadable
/// time, or one in the future (a clock that moved back), says nothing about its age.
fn is_stale(modified: std::io::Result<SystemTime>, now: SystemTime) -> bool {
    modified
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age > STALE_AFTER)
}

/// Removes `.tmp-*` folders left by an install that was killed, except those of the run named
/// `current`.
fn sweep_stale(parent: &Path, current: &str) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(".tmp-") || name.starts_with(current) {
            continue;
        }
        if is_stale(entry.metadata().and_then(|m| m.modified()), now) {
            // Temporary folders only: one that cannot be removed now never blocks an
            // install, and the next sweep tries again.
            drop(std::fs::remove_dir_all(entry.path()));
        }
    }
}

/// Installs `package` under `<adapters>/<package>/<version>`. Returns at once when that exact
/// version is already installed. `cancel` kills the process tree; so does `timeout`.
pub fn install_adapter(
    package: &AdapterPackage,
    adapters: &Path,
    tools: &InstallTools,
    processes: &ProcessTreeManager,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<InstalledAdapter, InstallError> {
    // npm, and the adapter started from the installed entry, get a path without the verbatim prefix.
    let adapters = &studio_bootstrap::external_path(adapters);
    if let Some(installed) = find_installed(adapters, package, &tools.node) {
        return Ok(installed);
    }
    let parent = package_dir(adapters, package);
    std::fs::create_dir_all(&parent).map_err(io)?;
    let name = temp_name();
    sweep_stale(&parent, &name);
    let staging = parent.join(&name);
    let cache = parent.join(format!("{name}-cache"));
    let outcome = run_install(package, &staging, &cache, tools, processes, cancel, timeout);
    // Temporary folders: whatever cannot be removed now is swept by a later install.
    drop(std::fs::remove_dir_all(&cache));
    let result =
        outcome.and_then(|()| move_into_place(&staging, adapters, package, &tools.node, cancel));
    drop(std::fs::remove_dir_all(&staging));
    result
}

/// How long a refused rename is retried. Windows refuses to rename a folder while a scanner
/// (Windows Defender) holds a file in it open, usually for well under a second.
const RENAME_PATIENCE: Duration = Duration::from_secs(10);

fn is_transient(error: &std::io::Error) -> bool {
    // Sharing and lock violations, which std does not classify.
    let windows_busy = cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33));
    windows_busy
        || matches!(
            error.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ResourceBusy
        )
}

/// Moves a folder that is not a complete install out of the way, to a `.tmp-*` sibling that is
/// removed now or by a later sweep.
fn set_aside(folder: &Path) -> std::io::Result<()> {
    let Some(parent) = folder.parent() else {
        return Err(std::io::Error::other("the install folder has no parent"));
    };
    let aside = parent.join(format!("{}-incomplete", temp_name()));
    std::fs::rename(folder, &aside)?;
    drop(std::fs::remove_dir_all(&aside));
    Ok(())
}

/// Renames the complete install in `staging` into its final folder. A folder already there
/// that is not a complete install (left by an older failure, or emptied by an antivirus
/// quarantine) is moved aside first, so it can never block the install for good. A cancel
/// that arrives after npm finished is still honoured here.
fn move_into_place(
    staging: &Path,
    adapters: &Path,
    package: &AdapterPackage,
    node: &Path,
    cancel: &AtomicBool,
) -> Result<InstalledAdapter, InstallError> {
    let final_root = final_dir(adapters, package);
    let deadline = Instant::now() + RENAME_PATIENCE;
    let mut backoff = Duration::from_millis(50);
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(InstallError::Cancelled);
        }
        let refused = match std::fs::rename(staging, &final_root) {
            Ok(()) => {
                return find_installed(adapters, package, node)
                    .ok_or_else(|| InstallError::Invalid("the install did not complete".into()));
            }
            Err(error) => error,
        };
        // A concurrent installer won the rename; its complete install is just as good.
        if let Some(installed) = find_installed(adapters, package, node) {
            return Ok(installed);
        }
        let blocker = if final_root.symlink_metadata().is_ok() {
            match set_aside(&final_root) {
                Ok(()) => None,
                // Someone else moved it already.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => Some(error),
            }
        } else {
            Some(refused)
        };
        match blocker {
            None if Instant::now() < deadline => {}
            Some(error) if is_transient(&error) && Instant::now() < deadline => {
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(1));
            }
            None => {
                return Err(InstallError::Invalid(
                    "the install folder kept changing while the install was moved into place"
                        .into(),
                ));
            }
            Some(error) => {
                return Err(InstallError::Io(format!(
                    "{}: {error}",
                    final_root.display()
                )));
            }
        }
    }
}

/// Stops npm's process tree. A tree that could not be stopped may still be writing into the
/// staging folder, so the failure is part of the error, never dropped.
fn stop(child: &parking_lot::Mutex<TrackedChild>) -> Option<String> {
    child
        .lock()
        .terminate_gracefully(Duration::from_secs(2))
        .err()
        .map(|error| format!("npm could not be stopped: {error}"))
}

/// One error message from the parts that have something to say.
fn join_detail(parts: impl IntoIterator<Item = Option<String>>) -> String {
    parts
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn run_install(
    package: &AdapterPackage,
    staging: &Path,
    cache: &Path,
    tools: &InstallTools,
    processes: &ProcessTreeManager,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<(), InstallError> {
    std::fs::create_dir_all(staging).map_err(io)?;
    std::fs::write(staging.join("package.json"), package.manifest).map_err(io)?;
    std::fs::write(staging.join("package-lock.json"), package.lockfile).map_err(io)?;

    let mut env = ChildEnvironment::default_allowlist();
    // Runtime folders first, without inheriting the ambient PATH. Linux npm wrappers
    // also need system tools such as bash, dirname and basename.
    let mut dirs: Vec<PathBuf> = [&tools.node, &tools.npm]
        .iter()
        .filter_map(|tool| tool.parent().map(Path::to_owned))
        .collect();
    dirs.dedup();
    #[cfg(target_os = "linux")]
    for dir in [PathBuf::from("/usr/bin"), PathBuf::from("/bin")] {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    let path = std::env::join_paths(dirs)
        .map_err(|_| InstallError::Invalid("a runtime folder is not a valid path".into()))?;
    env.remove("PATH");
    env.remove("Path");
    env.set(
        if cfg!(windows) { "Path" } else { "PATH" },
        path.to_string_lossy(),
    );
    env.set("npm_config_cache", cache.to_string_lossy());
    env.set("npm_config_update_notifier", "false");

    let mut options = SpawnOptions::new(&tools.npm);
    options
        // `ci` installs exactly the lock and fails when it disagrees with the manifest.
        .arg("ci")
        .arg("--prefix")
        .arg(staging)
        .args(["--no-audit", "--no-fund", "--ignore-scripts"]);
    options.env = env;
    let child = processes
        .spawn(options)
        .map_err(|e| InstallError::Npm(format!("could not start npm: {e}")))?;
    let (stdout, stderr) = {
        let mut guard = child.lock();
        (
            drain(guard.child_mut().stdout.take()),
            drain(guard.child_mut().stderr.take()),
        )
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
        if cancel.load(Ordering::SeqCst) {
            let stopped = stop(&child);
            finish_output(stderr, stdout);
            return Err(match stopped {
                None => InstallError::Cancelled,
                Some(failure) => InstallError::Npm(format!("install cancelled, but {failure}")),
            });
        }
        // The lock is released before `stop` takes it again.
        let polled = child.lock().try_wait();
        match polled {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let stopped = stop(&child);
                let output = npm_detail(&finish_output(stderr, stdout), cache);
                // The variant without text when there is nothing to add.
                if stopped.is_none() && output.is_empty() {
                    return Err(InstallError::TimedOut(timeout.as_secs()));
                }
                return Err(InstallError::Npm(join_detail([
                    Some(format!("timed out after {} seconds", timeout.as_secs())),
                    stopped,
                    Some(output),
                ])));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let stopped = stop(&child);
                let output = npm_detail(&finish_output(stderr, stdout), cache);
                return Err(InstallError::Npm(join_detail([
                    Some(format!("could not wait for npm: {error}")),
                    stopped,
                    Some(output),
                ])));
            }
        }
    };
    let output = finish_output(stderr, stdout);
    if !status.success() {
        let detail = npm_detail(&output, cache);
        return Err(InstallError::Npm(if detail.is_empty() {
            format!("exit status {status}")
        } else {
            detail
        }));
    }
    resolve_entry(staging, package)?;
    claude_patch::apply(staging, package)?;
    std::fs::write(
        staging.join(MARKER),
        format!("{}@{}\n", package.name, package.version),
    )
    .map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAKE: AdapterPackage = AdapterPackage {
        name: "fake-adapter",
        version: "1.0.0",
        bin: "fake-acp",
        manifest: "{\"private\":true,\"dependencies\":{\"fake-adapter\":\"1.0.0\"}}\n",
        lockfile: "{\"lockfileVersion\":3}\n",
    };

    /// A stand-in for npm that lays out the package like `npm ci --prefix <dir>`. It refuses
    /// any other call, and a call without the lockfile next to the manifest.
    fn fake_npm(dir: &Path, body: &str) -> PathBuf {
        #[cfg(windows)]
        let (path, script) = (
            dir.join("npm.cmd"),
            format!(
                "@echo off\r\n\
                 if not \"%1\"==\"ci\" (echo unexpected npm call 1>&2& exit /b 9)\r\n\
                 if not exist \"%~3\\package-lock.json\" (echo no lockfile 1>&2& exit /b 9)\r\n\
                 {body}\r\n"
            ),
        );
        // Linux version managers can wrap npm in a shell script that needs system tools.
        #[cfg(not(windows))]
        let (path, script) = {
            let header = if cfg!(target_os = "linux") {
                "#!/usr/bin/env bash\n"
            } else {
                "#!/bin/sh\nPATH=\"$PATH:/usr/bin:/bin\"\n"
            };
            (
                dir.join("npm"),
                format!(
                    "{header}\
                 [ \"$1\" = ci ] || {{ echo unexpected npm call >&2; exit 9; }}\n\
                 [ -f \"$3/package-lock.json\" ] || {{ echo no lockfile >&2; exit 9; }}\n\
                 {body}\n"
                ),
            )
        };
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            // A sibling test that forks while this script is open for writing holds it until
            // its child execs, and running the script then fails with ETXTBSY. Run it once
            // (without `ci` it exits at once): after one start no writer can be left.
            for _ in 0..100 {
                let started = std::process::Command::new(&path)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                match started {
                    Err(error) if error.raw_os_error() == Some(26) => {
                        std::thread::sleep(std::time::Duration::from_millis(20))
                    }
                    _ => break,
                }
            }
        }
        path
    }

    #[cfg(windows)]
    const SUCCESS: &str = "mkdir \"%~3\\node_modules\\fake-adapter\\dist\"\r\n\
        echo {\"name\":\"fake-adapter\",\"bin\":{\"fake-acp\":\"dist/index.js\"}}>\"%~3\\node_modules\\fake-adapter\\package.json\"\r\n\
        echo x>\"%~3\\node_modules\\fake-adapter\\dist\\index.js\"";
    #[cfg(not(windows))]
    const SUCCESS: &str = "d=\"$3/node_modules/fake-adapter\"; mkdir -p \"$d/dist\"; \
        echo '{\"name\":\"fake-adapter\",\"bin\":{\"fake-acp\":\"dist/index.js\"}}' > \"$d/package.json\"; \
        echo x > \"$d/dist/index.js\"";

    #[cfg(windows)]
    const FAILURE: &str = "echo registry unreachable 1>&2\r\nexit /b 1";
    #[cfg(not(windows))]
    const FAILURE: &str = "echo registry unreachable >&2; exit 1";

    #[cfg(windows)]
    const HANG: &str = r"%SystemRoot%\System32\ping.exe -n 30 127.0.0.1 >nul";
    #[cfg(not(windows))]
    const HANG: &str = "sleep 30";

    /// Redirects an `echo` to stderr.
    #[cfg(windows)]
    const ERR: &str = " 1>&2";
    #[cfg(not(windows))]
    const ERR: &str = " >&2";

    struct Fixture {
        _dir: tempfile::TempDir,
        adapters: PathBuf,
        tools: InstallTools,
    }

    fn fixture(body: &str) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let npm = fake_npm(&bin, body);
        let node = bin.join(if cfg!(windows) { "node.exe" } else { "node" });
        std::fs::write(&node, b"").unwrap();
        Fixture {
            adapters: dir.path().join("adapters"),
            tools: InstallTools { node, npm },
            _dir: dir,
        }
    }

    fn install(f: &Fixture, cancel: &AtomicBool) -> Result<InstalledAdapter, InstallError> {
        install_adapter(
            &FAKE,
            &f.adapters,
            &f.tools,
            &ProcessTreeManager::new(),
            cancel,
            Duration::from_secs(20),
        )
    }

    fn leftovers(f: &Fixture) -> Vec<String> {
        std::fs::read_dir(package_dir(&f.adapters, &FAKE))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with(".tmp-"))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn successful_install_is_complete_and_launches_through_node() {
        let f = fixture(SUCCESS);
        let installed = install(&f, &AtomicBool::new(false)).unwrap();
        assert_eq!(installed.node, f.tools.node);
        assert!(installed.entry.ends_with("index.js") && installed.entry.is_file());
        assert!(leftovers(&f).is_empty());
        assert_eq!(
            find_installed(&f.adapters, &FAKE, &f.tools.node),
            Some(installed)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn npm_wrapper_gets_runtime_and_system_paths_without_ambient_path() {
        let f = fixture(&format!(
            "[ \"$(basename \"$(dirname \"$0\")\")\" = bin ] || exit 9; \
             printf '%s' \"$PATH\" > \"$3/install-path\"; {SUCCESS}"
        ));
        let ambient_path = std::env::var_os("PATH");
        install(&f, &AtomicBool::new(false)).unwrap();
        let path =
            std::fs::read_to_string(final_dir(&f.adapters, &FAKE).join("install-path")).unwrap();
        assert_eq!(
            std::env::split_paths(&path).collect::<Vec<_>>(),
            vec![
                f.tools.node.parent().unwrap().to_owned(),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ]
        );
        assert_eq!(std::env::var_os("PATH"), ambient_path);
        assert!(leftovers(&f).is_empty());
    }

    /// A canonicalized app data folder (`\\?\C:\...`) reaches npm and the installed entry
    /// as a plain path: npm's `realpath` never ends on a verbatim one.
    #[cfg(windows)]
    #[test]
    fn a_verbatim_adapters_folder_reaches_npm_as_a_plain_path() {
        let mut f = fixture(&format!("echo %~3>\"%~dp0prefix.txt\"\r\n{SUCCESS}"));
        std::fs::create_dir_all(&f.adapters).unwrap();
        f.adapters = std::fs::canonicalize(&f.adapters).unwrap();
        assert!(f.adapters.to_string_lossy().starts_with(r"\\?\"));
        let installed = install(&f, &AtomicBool::new(false)).unwrap();
        let prefix = std::fs::read_to_string(f.tools.npm.with_file_name("prefix.txt")).unwrap();
        assert!(!prefix.starts_with(r"\\?\"), "{prefix}");
        assert!(!installed.entry.to_string_lossy().starts_with(r"\\?\"));
        assert!(installed.entry.is_file());
    }

    #[test]
    fn failed_install_leaves_nothing_usable_and_reports_npm_output() {
        let f = fixture(FAILURE);
        let error = install(&f, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(&error, InstallError::Npm(m) if m.contains("registry unreachable")));
        assert!(find_installed(&f.adapters, &FAKE, &f.tools.node).is_none());
        assert!(leftovers(&f).is_empty());
    }

    #[test]
    fn cancel_kills_the_install_and_leaves_nothing_usable() {
        let f = fixture(HANG);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        assert_eq!(install(&f, &cancel), Err(InstallError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(15));
        assert!(find_installed(&f.adapters, &FAKE, &f.tools.node).is_none());
        assert!(leftovers(&f).is_empty());
    }

    #[test]
    fn an_unmarked_folder_is_not_an_install() {
        let f = fixture(SUCCESS);
        let root = final_dir(&f.adapters, &FAKE);
        std::fs::create_dir_all(root.join("node_modules/fake-adapter/dist")).unwrap();
        assert!(find_installed(&f.adapters, &FAKE, &f.tools.node).is_none());
    }

    #[test]
    fn install_replaces_an_unmarked_existing_folder() {
        let f = fixture(SUCCESS);
        let root = final_dir(&f.adapters, &FAKE);
        std::fs::create_dir_all(root.join("node_modules/fake-adapter/dist")).unwrap();
        std::fs::write(root.join("node_modules/fake-adapter/dist/old.js"), b"//").unwrap();
        let installed = install(&f, &AtomicBool::new(false)).unwrap();
        assert!(installed.entry.is_file());
        assert!(!root.join("node_modules/fake-adapter/dist/old.js").exists());
        assert!(leftovers(&f).is_empty());
    }

    #[test]
    fn install_replaces_a_marked_folder_whose_entry_was_quarantined() {
        let f = fixture(SUCCESS);
        install(&f, &AtomicBool::new(false)).unwrap();
        let entry = find_installed(&f.adapters, &FAKE, &f.tools.node)
            .unwrap()
            .entry;
        std::fs::remove_file(&entry).unwrap();
        assert!(find_installed(&f.adapters, &FAKE, &f.tools.node).is_none());
        let installed = install(&f, &AtomicBool::new(false)).unwrap();
        assert_eq!(installed.entry, entry);
        assert!(entry.is_file());
        assert!(leftovers(&f).is_empty());
    }

    #[test]
    fn a_cancel_after_npm_finished_is_honoured_before_the_rename() {
        let f = fixture(SUCCESS);
        let parent = package_dir(&f.adapters, &FAKE);
        let staging = parent.join(temp_name());
        let module = staging.join("node_modules/fake-adapter");
        std::fs::create_dir_all(module.join("dist")).unwrap();
        std::fs::write(
            module.join("package.json"),
            r#"{"bin":{"fake-acp":"dist/index.js"}}"#,
        )
        .unwrap();
        std::fs::write(module.join("dist/index.js"), b"//").unwrap();
        std::fs::write(staging.join("package-lock.json"), FAKE.lockfile).unwrap();
        std::fs::write(staging.join(MARKER), "fake-adapter@1.0.0\n").unwrap();
        let cancelled = move_into_place(
            &staging,
            &f.adapters,
            &FAKE,
            &f.tools.node,
            &AtomicBool::new(true),
        );
        assert_eq!(cancelled, Err(InstallError::Cancelled));
        assert!(find_installed(&f.adapters, &FAKE, &f.tools.node).is_none());
        // The same staging folder installs when nobody cancels.
        assert!(
            move_into_place(
                &staging,
                &f.adapters,
                &FAKE,
                &f.tools.node,
                &AtomicBool::new(false)
            )
            .is_ok()
        );
    }

    #[test]
    fn only_folders_positively_older_than_the_threshold_are_stale() {
        let now = SystemTime::now();
        let hours = |h: u64| Duration::from_secs(h * 3600);
        assert!(is_stale(Ok(now - hours(2)), now));
        assert!(!is_stale(Ok(now - Duration::from_secs(60)), now));
        // A time in the future or one that cannot be read says nothing about the age.
        assert!(!is_stale(Ok(now + hours(2)), now));
        assert!(!is_stale(Err(std::io::Error::other("no time")), now));
    }

    #[test]
    fn the_sweep_keeps_young_folders_and_the_current_run() {
        let dir = tempfile::tempdir().unwrap();
        let young = dir.path().join(".tmp-young");
        let current = dir.path().join(".tmp-current-cache");
        let other = dir.path().join("1.0.0");
        for folder in [&young, &current, &other] {
            std::fs::create_dir_all(folder).unwrap();
        }
        sweep_stale(dir.path(), ".tmp-current");
        assert!(young.is_dir() && current.is_dir() && other.is_dir());
    }

    #[test]
    fn a_timeout_keeps_what_npm_printed() {
        let f = fixture(&format!("echo still resolving{ERR}\n{HANG}"));
        let started = Instant::now();
        let error = install_adapter(
            &FAKE,
            &f.adapters,
            &f.tools,
            &ProcessTreeManager::new(),
            &AtomicBool::new(false),
            Duration::from_secs(4),
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(15));
        assert!(
            matches!(&error, InstallError::Npm(m) if m.contains("timed out after 4 seconds")
                && m.contains("still resolving")),
            "{error}"
        );
        assert!(leftovers(&f).is_empty());
    }

    #[test]
    fn a_failure_reports_the_end_of_the_npm_log_instead_of_its_deleted_path() {
        #[cfg(windows)]
        let body = "mkdir \"%npm_config_cache%\\_logs\"\r\n\
            echo 12 verbose stack Error: tarball corrupted>\"%npm_config_cache%\\_logs\\2026-debug-0.log\"\r\n\
            echo npm error A complete log of this run can be found in: %npm_config_cache%\\_logs\\2026-debug-0.log 1>&2\r\n\
            exit /b 1";
        #[cfg(not(windows))]
        let body = "mkdir -p \"$npm_config_cache/_logs\"; \
            echo '12 verbose stack Error: tarball corrupted' > \"$npm_config_cache/_logs/2026-debug-0.log\"; \
            echo \"npm error A complete log of this run can be found in: $npm_config_cache/_logs/2026-debug-0.log\" >&2; \
            exit 1";
        let f = fixture(body);
        let error = install(&f, &AtomicBool::new(false)).unwrap_err();
        let InstallError::Npm(message) = &error else {
            panic!("{error}");
        };
        assert!(message.contains("tarball corrupted"), "{message}");
        assert!(!message.contains("A complete log"), "{message}");
        assert!(leftovers(&f).is_empty());
    }

    #[test]
    fn concurrent_installers_both_succeed_with_one_result() {
        let f = Arc::new(fixture(SUCCESS));
        let handles: Vec<_> = (0..3)
            .map(|_| {
                let f = Arc::clone(&f);
                std::thread::spawn(move || install(&f, &AtomicBool::new(false)))
            })
            .collect();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        assert!(find_installed(&f.adapters, &FAKE, &f.tools.node).is_some());
        assert!(leftovers(&f).is_empty());
    }

    #[test]
    fn an_install_from_another_lockfile_is_replaced() {
        let f = fixture(SUCCESS);
        install(&f, &AtomicBool::new(false)).unwrap();
        let relocked = AdapterPackage {
            lockfile: "{\"lockfileVersion\":3,\"packages\":{}}\n",
            ..FAKE
        };
        assert!(find_installed(&f.adapters, &relocked, &f.tools.node).is_none());
        let crlf = AdapterPackage {
            lockfile: "{\"lockfileVersion\":3}\r\n",
            ..FAKE
        };
        assert!(find_installed(&f.adapters, &crlf, &f.tools.node).is_some());
    }

    #[test]
    fn pinned_packages_match_their_manifest_and_lockfile() {
        for package in [CLAUDE_ADAPTER, CODEX_ADAPTER] {
            assert!(
                package
                    .version
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.')
            );
            let manifest: serde_json::Value = serde_json::from_str(package.manifest).unwrap();
            let lock: serde_json::Value = serde_json::from_str(package.lockfile).unwrap();
            let packages = lock["packages"].as_object().unwrap();
            assert_eq!(manifest["dependencies"][package.name], package.version);
            assert_eq!(packages[""]["dependencies"][package.name], package.version);
            assert_eq!(
                packages[&format!("node_modules/{}", package.name)]["version"],
                package.version
            );
            for (path, entry) in packages.iter().filter(|(path, _)| !path.is_empty()) {
                assert!(
                    entry["integrity"].is_string() && entry["resolved"].is_string(),
                    "{path} in the {} lock is not pinned",
                    package.name
                );
            }
        }
        assert_eq!(package_for("claude"), Some(CLAUDE_ADAPTER));
        assert_eq!(package_for("pi"), None);
    }
}
