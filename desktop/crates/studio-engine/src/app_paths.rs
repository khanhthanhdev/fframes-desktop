use crate::EngineError;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data: PathBuf,
}
impl AppPaths {
    pub fn system() -> Result<Self, EngineError> {
        #[cfg(target_os = "linux")]
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
        #[cfg(target_os = "macos")]
        let base =
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
        #[cfg(windows)]
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let base = base.filter(|p| p.is_absolute()).ok_or_else(|| EngineError::Diagnostic("Platform app-data directory unavailable; configure HOME/XDG_DATA_HOME or LOCALAPPDATA".into()))?;
        Self::new(base.join("fframes-studio"))
    }
    pub fn new(data: impl Into<PathBuf>) -> Result<Self, EngineError> {
        let data = data.into();
        fs::create_dir_all(&data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&data, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            data: fs::canonicalize(data)?,
        })
    }
    pub fn database(&self) -> PathBuf {
        self.data.join("studio.sqlite3")
    }
    pub fn project(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.data.join("projects").join(String::from(id.clone()))
    }
    pub fn builds(&self) -> PathBuf {
        self.data.join("builds")
    }
    /// Per-project agent workspace root (outside portable source and Git).
    pub fn agent(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.project(id).join("agent")
    }
    /// Stable agent working directory: the same path for every task of the project.
    pub fn agent_draft(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.agent(id).join("draft")
    }
    /// Durable draft ownership marker, kept beside (never inside) the draft.
    pub fn agent_draft_state(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.agent(id).join("draft-state.json")
    }
    /// Retained failed drafts are moved here before the stable draft is refreshed.
    pub fn agent_archive(&self, id: &studio_project::ProjectId) -> PathBuf {
        self.agent(id).join("archive")
    }
    pub fn contains_source(&self, source: &Path) -> bool {
        self.data.starts_with(source) || source.starts_with(&self.data)
    }
}
