use std::path::PathBuf;
use studio_engine::{Controller, JobState};
use studio_project::{ProjectId, SourceRevision};

/// Bounded immutable presentation; rendering never scans files or queries the database.
#[derive(Clone)]
pub struct ProjectPresentation {
    pub id: ProjectId,
    pub name: String,
    pub root: PathBuf,
    pub source: SourceRevision,
    pub accepted: SourceRevision,
    pub job: String,
    pub interrupted: bool,
    pub draft: Option<PathBuf>,
    pub recovery_notice: Option<String>,
    pub checkpoint: PathBuf,
    pub files: Vec<String>,
    pub assets: Vec<String>,
    pub styles: Vec<String>,
    pub file_count: usize,
    pub history_available: bool,
    pub worker_available: bool,
}
impl ProjectPresentation {
    pub fn from_controller(controller: &Controller) -> Self {
        let project = &controller.project;
        let state = controller.state();
        let job = match state.job() {
            JobState::Idle => "Idle",
            JobState::Queued(_) => "Queued",
            JobState::Running(_) => "Running",
            JobState::CancelRequested(_) => "Cancelling",
            JobState::Succeeded(_) => "Succeeded",
            JobState::Failed(_, _) => "Failed",
            JobState::Interrupted(_) => "Interrupted",
        };
        Self {
            id: state.project().clone(),
            name: project.manifest.display.name.clone(),
            root: project.root.clone(),
            source: state.source().clone(),
            accepted: state.accepted().clone(),
            job: job.into(),
            interrupted: matches!(state.job(), JobState::Interrupted(_)),
            draft: controller
                .draft()
                .filter(|path| path.is_dir())
                .map(PathBuf::from),
            recovery_notice: controller.recovery_notice.clone(),
            checkpoint: controller.checkpoints.manifest_path(state.accepted()),
            files: project
                .inventory
                .files
                .iter()
                .take(200)
                .map(|f| f.path.as_str().to_owned())
                .collect(),
            assets: project
                .manifest
                .assets
                .iter()
                .take(200)
                .map(|p| p.as_str().to_owned())
                .collect(),
            styles: project
                .inventory
                .files
                .iter()
                .filter(|f| f.path.as_str().starts_with("style/"))
                .take(200)
                .map(|f| f.path.as_str().to_owned())
                .collect(),
            file_count: project.inventory.files.len(),
            history_available: controller.history_was_available,
            worker_available: project.worker_available,
        }
    }
}
