//! Test-only helpers shared by the studio crates. Used only as a dev-dependency.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::OnceLock,
};

/// The engine's integration-test directory (`desktop/crates/studio-engine/tests`). It holds the
/// fixtures every studio crate's tests share: the scripted ACP agent, the fake preview worker
/// and the fake compiler under `support/`.
pub const ENGINE_TESTS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../studio-engine/tests");

/// The Python interpreter for test fixtures, resolved once per test binary.
///
/// Candidates, in order: the `PYTHON` environment variable; the interpreter under
/// `$pythonLocation` (set by `actions/setup-python`); the `sys.executable` reported by the
/// Windows launcher `py -3`; `python3`; `python`. The first one that runs and reports an
/// absolute `sys.executable` wins, as that path (this skips the Windows Store `python`
/// alias).
///
/// # Panics
///
/// When no candidate works, with the list of candidates that were tried.
pub fn python() -> &'static Path {
    static PYTHON: OnceLock<PathBuf> = OnceLock::new();
    PYTHON.get_or_init(resolve_python)
}

fn resolve_python() -> PathBuf {
    let mut tried = Vec::new();
    let mut candidates = Vec::new();
    if let Some(value) = std::env::var_os("PYTHON").filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(value));
    }
    if let Some(location) = std::env::var_os("pythonLocation").filter(|v| !v.is_empty()) {
        let location = PathBuf::from(location);
        candidates.push(if cfg!(windows) {
            location.join("python.exe")
        } else {
            location.join("bin").join("python3")
        });
    }
    match launcher_executable() {
        Some(path) => candidates.push(path),
        None => tried.push("py -3 (no usable sys.executable)".to_owned()),
    }
    candidates.push(PathBuf::from("python3"));
    candidates.push(PathBuf::from("python"));
    for candidate in candidates {
        // Its own absolute path: commands such as MCP servers must be absolute.
        if let Some(path) = executable(&candidate) {
            return path;
        }
        tried.push(candidate.display().to_string());
    }
    panic!(
        "no working Python interpreter for the test fixtures; tried: {}. \
         Set PYTHON to an interpreter that runs `-c \"import sys\"`.",
        tried.join(", ")
    );
}

/// The `sys.executable` of `interpreter`, if it runs.
fn executable(interpreter: &Path) -> Option<PathBuf> {
    let output = Command::new(interpreter)
        .args(["-c", "import sys; print(sys.executable)"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let path = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(path.trim());
    (output.status.success() && path.is_absolute()).then_some(path)
}

/// `sys.executable` as reported by the Windows `py` launcher, if it is installed.
fn launcher_executable() -> Option<PathBuf> {
    let output = Command::new("py")
        .args(["-3", "-c", "import sys; print(sys.executable)"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_resolved_interpreter_runs_python() {
        let python = python();
        assert_eq!(executable(python).as_deref(), Some(python));
        // Resolved once: every call returns the same path.
        assert!(std::ptr::eq(python, super::python()));
    }
}
