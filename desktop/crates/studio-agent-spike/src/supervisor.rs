use crate::driver::redact_sensitive_string;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use studio_bootstrap::{
    ChildEnvironment, ProcessError, ProcessTreeManager, SpawnOptions, TrackedChild,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("process error: {0}")]
    Process(#[from] ProcessError),
    #[error("json protocol error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("task cancelled by user")]
    Cancelled,
    #[error("supervisor error: {0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualificationStatus {
    Passed {
        provider: String,
        adapter_version: String,
        duration_ms: u64,
    },
    NotRun {
        provider: String,
        reason: String,
    },
    Failed {
        provider: String,
        stage: String,
        error: String,
    },
}

pub struct AgentSupervisor {
    process_tree: ProcessTreeManager,
}

impl AgentSupervisor {
    pub fn new(process_tree: ProcessTreeManager) -> Self {
        Self { process_tree }
    }

    /// Spawns an agent adapter child in an isolated environment with process group ownership.
    pub fn spawn_adapter(
        &self,
        executable: &Path,
        args: &[String],
        working_dir: &Path,
        custom_env: Option<ChildEnvironment>,
    ) -> Result<Arc<Mutex<TrackedChild>>, SupervisorError> {
        let env = custom_env.unwrap_or_else(ChildEnvironment::default_allowlist);
        let mut opts = SpawnOptions::new(executable);
        opts.args(args);
        opts.current_dir(working_dir);
        opts.env = env;
        opts.stdin = Stdio::piped();
        opts.stdout = Stdio::piped();
        opts.stderr = Stdio::piped();

        let child = self.process_tree.spawn(opts)?;
        Ok(child)
    }

    /// Cancels an active task and terminates its child process group.
    pub fn cancel_task(child_arc: &Arc<Mutex<TrackedChild>>) -> Result<(), SupervisorError> {
        let mut child = child_arc.lock();
        child.terminate_gracefully(Duration::from_millis(300))?;
        Ok(())
    }

    /// Reads bounded stderr from a child (up to max_bytes), redacting any secret tokens.
    pub fn drain_bounded_stderr(
        mut stderr_reader: impl Read,
        max_bytes: usize,
    ) -> Result<String, SupervisorError> {
        let mut buffer = vec![0u8; max_bytes];
        let mut total_read = 0;

        while total_read < max_bytes {
            match stderr_reader.read(&mut buffer[total_read..]) {
                Ok(0) => break,
                Ok(n) => total_read += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(SupervisorError::Io(e)),
            }
        }

        let raw_str = String::from_utf8_lossy(&buffer[..total_read]);
        Ok(redact_sensitive_string(&raw_str))
    }

    /// Evaluates qualification when no real provider credentials are available.
    pub fn record_unauthenticated_status(provider: &str) -> QualificationStatus {
        QualificationStatus::NotRun {
            provider: provider.to_string(),
            reason: "credentials unavailable".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drain_bounded_stderr_redacts_content() {
        let raw_stderr = b"Error authenticating with token=sk-ant-secret12345 in session\n";
        let drained = AgentSupervisor::drain_bounded_stderr(&raw_stderr[..], 1024).unwrap();

        assert!(!drained.contains("sk-ant-secret12345"));
        assert!(drained.contains("[REDACTED]"));
    }

    #[test]
    #[cfg(unix)]
    fn test_spawn_and_cancel_adapter() {
        let manager = ProcessTreeManager::new();
        let supervisor = AgentSupervisor::new(manager.clone());

        let tmp = tempfile::tempdir().unwrap();
        let child = supervisor
            .spawn_adapter(Path::new("sleep"), &["30".into()], tmp.path(), None)
            .expect("spawns adapter");

        assert_eq!(manager.active_count(), 1);

        AgentSupervisor::cancel_task(&child).expect("cancels task");
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn test_unauthenticated_qualification_records_not_run() {
        let status = AgentSupervisor::record_unauthenticated_status("anthropic-acp");
        assert!(matches!(
            status,
            QualificationStatus::NotRun { ref reason, .. } if reason == "credentials unavailable"
        ));
    }
}
