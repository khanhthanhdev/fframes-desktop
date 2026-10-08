use studio_project::{OpenProject, SourceRevision, checkpoint::copy_draft};

use crate::{Controller, EngineError, TaskRevisionRecord};

/// One immutable, app-owned project tree selected for rendering.
///
/// The temporary directory outlives compilation and rendering when held by the job;
/// callers must not substitute the mutable checkout after this capture succeeds.
pub struct FrozenExportSource {
    root: tempfile::TempDir,
    project: OpenProject,
    revision: SourceRevision,
    task_revision: Option<String>,
    label: String,
    live_source_warning: Option<String>,
}

impl FrozenExportSource {
    pub fn project(&self) -> &OpenProject {
        &self.project
    }

    pub fn revision(&self) -> &SourceRevision {
        &self.revision
    }

    pub fn task_revision(&self) -> Option<&str> {
        self.task_revision.as_deref()
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn live_source_warning(&self) -> Option<&str> {
        self.live_source_warning.as_deref()
    }

    pub fn root(&self) -> &std::path::Path {
        &self.project.root
    }

    pub fn retained_directory(&self) -> &tempfile::TempDir {
        &self.root
    }
}

impl Controller {
    /// Freezes the latest validated published task revision, or the current saved
    /// revision after the preview build has succeeded. This deliberately does not use
    /// `ProjectState::accepted()` when a newer task revision is published.
    pub fn freeze_export_source(&self) -> Result<FrozenExportSource, EngineError> {
        let state = self.state();
        let (revision, task_revision, label) = match state.task_history().head() {
            Some(record) if record.published == *state.source() => (
                record.published.clone(),
                Some(record.id.clone()),
                format!("Published edit · {}", record.prompt_summary),
            ),
            Some(record) => {
                return Err(EngineError::Diagnostic(format!(
                    "latest validated edit {} is not the current project source; reconcile the project before exporting",
                    record.id
                )));
            }
            None if state.source() == state.accepted() && state.built() == Some(state.source()) => {
                (
                    state.source().clone(),
                    None,
                    "Saved project · preview build verified".into(),
                )
            }
            None if state.source() != state.accepted() => {
                return Err(EngineError::Diagnostic(
                    "project has unsaved, unvalidated source; build and save or publish a validated edit before exporting".into(),
                ));
            }
            None => {
                return Err(EngineError::Diagnostic(
                    "build the current saved revision in preview before exporting".into(),
                ));
            }
        };

        let live_source_warning = match studio_project::revision::SourceInventory::scan(
            &self.project.root,
        ) {
            Ok(inventory) if inventory.revision == revision => None,
            Ok(inventory) => {
                return Err(EngineError::Diagnostic(format!(
                    "live source {} differs from the selected export revision {}; restore or explicitly select a validated revision",
                    inventory.revision, revision
                )));
            }
            Err(error) if task_revision.is_some() => {
                // The durable task journal and checkpoint still identify the accepted
                // bytes; a damaged live checkout must not destroy that recovery path.
                Some(error.to_string())
            }
            Err(error) => {
                return Err(EngineError::Diagnostic(format!(
                    "cannot verify saved source for export: {error}"
                )));
            }
        };

        let temp_root = tempfile::Builder::new()
            .prefix("fframes-export-source-")
            .tempdir()?;
        let recovered = self
            .checkpoints
            .draft(&revision, &format!("export-{}", uuid::Uuid::new_v4()))?;
        let frozen_path = temp_root.path().join("source");
        copy_draft(&recovered, &frozen_path)?;
        let project = studio_project::open(&frozen_path)?;
        if project.inventory.revision != revision {
            return Err(EngineError::Diagnostic(
                "frozen export source does not match its recorded revision".into(),
            ));
        }
        Ok(FrozenExportSource {
            root: temp_root,
            project,
            revision,
            task_revision,
            label,
            live_source_warning,
        })
    }

    /// The available validated task revisions, newest first, for a future explicit
    /// revision picker. Every record remains immutable in the task journal.
    pub fn export_revisions(&self) -> Vec<TaskRevisionRecord> {
        self.state()
            .task_history()
            .entries()
            .iter()
            .rev()
            .cloned()
            .collect()
    }
}
