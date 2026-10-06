//! Studio-originated preset mutations (Apply, Reapply, Reset, override edits).
//!
//! Like Apply and Undo this runs under `&mut Controller`, the project mutation gate, and
//! publishes only through the durable file-set transaction of
//! [`crate::edit_transaction`] (no-follow, no-clobber, journalled, recoverable). It is *not*
//! an agent task: no task exists, no validation report is forged, the accepted task
//! history, its Undo and the saved checkpoint are untouched, and the committed journal
//! record is a distinct [`TransactionKind::Preset`] entry. The source is fenced by the
//! revision the caller captured; a changed source refuses before anything is written.
use super::Controller;
use crate::{
    EngineError, PresetProvenance, PresetRequest, StateError,
    edit_transaction::{
        ApplyGate, Boundary, PlanError, PromotionError, RecordSeed, TaskRevisionRecord,
        TransactionKind, plan as plan_transaction, prompt_summary,
    },
    preset_state::{self, FileChange, MUTATION_QUALIFIED, ProjectStyle},
};
use std::{fs, io::Write, path::Path};
use studio_project::SourceRevision;

/// Whether the controls that change the project's preset snapshot may be used now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetMutation {
    Enabled,
    /// Disabled with the reason to show next to the controls.
    Disabled(String),
}

impl PresetMutation {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled)
    }
}

/// What a preset mutation did to the source.
#[derive(Debug, Clone)]
pub struct PresetOutcome {
    /// The durable preset record; `None` when the project already held exactly the
    /// requested snapshot (nothing was written and nothing was invalidated).
    pub record: Option<Box<TaskRevisionRecord>>,
    /// The source revision after the mutation (unchanged when `record` is `None`).
    pub source: SourceRevision,
    /// Overrides kept but not applied, and non-fatal observations.
    pub notes: Vec<String>,
}

impl Controller {
    /// Whether preset mutation is available, with an actionable reason when it is not.
    /// Only the qualified platform may publish; a blocked publication gate, an
    /// unresolved or suspended earlier mutation and a running agent task also disable it.
    pub fn preset_mutation(&mut self) -> PresetMutation {
        if !MUTATION_QUALIFIED {
            return PresetMutation::Disabled(
                "Preset changes are enabled only on Linux x86_64, where no-clobber publication and crash recovery were qualified; this platform stays read-only until its probes pass".into(),
            );
        }
        if let Err(error) = self.ensure_mutable() {
            return PresetMutation::Disabled(error.to_string());
        }
        if let ApplyGate::Blocked(reason) = self.apply_gate() {
            return PresetMutation::Disabled(format!("Source publication is blocked: {reason}"));
        }
        if self.agent.active().is_some() {
            return PresetMutation::Disabled(
                "An agent task is running; finish or cancel it before changing the preset".into(),
            );
        }
        PresetMutation::Enabled
    }

    /// The style snapshot stored in the project's portable files, read fresh.
    pub fn project_style(&self) -> Option<ProjectStyle> {
        preset_state::read_project_style(&self.project.root, &self.project.inventory)
    }

    /// Applies, reapplies or resets a preset, or edits one project override, as one
    /// source-fenced durable revision. `expected` is the source revision the caller's
    /// view (and its displayed preview) was built from.
    ///
    /// After a commit every derived identity (build, candidate, install authorization,
    /// running operations) is invalidated by the source rescan; the caller then asks
    /// for an ordinary revision-safe preview of the new source.
    pub fn apply_preset(
        &mut self,
        expected: &SourceRevision,
        request: PresetRequest<'_>,
    ) -> Result<PresetOutcome, EngineError> {
        self.ensure_mutable()?;
        if let PresetMutation::Disabled(reason) = self.preset_mutation() {
            return Err(PromotionError::GateBlocked(reason).into());
        }
        self.reconcile()?;
        let current = self.project.inventory.clone();
        if &current.revision != expected {
            return Err(PromotionError::SourceChanged {
                expected: expected.as_str()[..12].to_owned(),
                found: current.revision.as_str()[..12].to_owned(),
            }
            .into());
        }
        let plan = preset_state::plan(&self.project.root, &current, &request)?;
        let unchanged = |notes: Vec<String>| PresetOutcome {
            record: None,
            source: current.revision.clone(),
            notes,
        };
        if plan.changes.is_empty() {
            // Nothing to publish; a manifest reference left stale by an interrupted
            // earlier application is still repaired.
            let mut notes = plan.provenance.notes;
            if let Some((id, hash)) = &plan.reference {
                self.sync_manifest_reference(id, hash, &mut notes)?;
            }
            let mut outcome = unchanged(notes);
            outcome.source = self.record.state.source().clone();
            return Ok(outcome);
        }
        // The current source and the candidate as immutable objects.
        let base = self.checkpoints.capture(&self.project.root)?;
        if base != current.revision {
            return Err(StateError::StaleResult.into());
        }
        let draft = self
            .checkpoints
            .draft(&base, &format!("preset-{}", uuid::Uuid::new_v4().simple()))?;
        let formed = write_changes(&draft, &plan.changes)
            .map_err(EngineError::from)
            .and_then(|()| self.checkpoints.capture(&draft).map_err(EngineError::from));
        let _ = fs::remove_dir_all(&draft);
        let candidate = self.checkpoints.load(&formed?)?;
        let seed = RecordSeed {
            kind: TransactionKind::Preset,
            task: "studio-preset".into(),
            generation: 0,
            session: self.record.state.session().clone(),
            task_base: current.revision.clone(),
            prior_history: self.record.state.task_history().head_id(),
            prior_checkpoint: self.record.state.accepted().clone(),
            undoes: None,
            prompt_summary: prompt_summary(&plan.summary),
            build: None,
            validation_report_sha256: String::new(),
            remove_dirs: Vec::new(),
            reverses: Vec::new(),
            preset: Some(plan.provenance.clone()),
        };
        let intent = match plan_transaction(
            &self.project.root,
            &current,
            &candidate,
            seed,
            self.mechanism_override,
        ) {
            Ok(intent) => intent,
            Err(PlanError::NoChange) => return Ok(unchanged(plan.provenance.notes)),
            Err(error) => return Err(PromotionError::from(error).into()),
        };
        let record = self.run_intent(&intent)?;
        self.preset_after_commit(record, plan.provenance, plan.reference)
    }

    /// Repairs the manifest's preset reference from the stored snapshot, for a project
    /// whose application was interrupted between its commit and the manifest write.
    /// Best effort and silent when nothing is stale, blocked or unreadable.
    pub(super) fn heal_manifest_reference(&mut self) {
        let Some((id, hash)) =
            preset_state::stored_reference(&self.project.root, &self.project.inventory)
        else {
            return;
        };
        if preset_state::manifest_with_reference(&self.project.manifest, &id, &hash).is_none() {
            return;
        }
        if self.preset_mutation().is_enabled() {
            let mut notes = Vec::new();
            if self.sync_manifest_reference(&id, &hash, &mut notes).is_ok() && !notes.is_empty() {
                self.recovery_notice = Some(notes.join(" "));
            }
        }
    }

    /// Replaces `studio.json`'s preset reference with the project's own same-directory
    /// atomic manifest replacement (never a displace/publish pair: a manifest that is
    /// briefly absent would make the project unopenable, and so unrecoverable). Other
    /// manifest fields are taken from the file as it is now. A failed write is a note;
    /// the reference is repaired on the next open or application.
    fn sync_manifest_reference(
        &mut self,
        id: &str,
        hash: &str,
        notes: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        if let Err(error) = self.reconcile() {
            notes.push(format!("manifest reference not updated: {error}"));
            return Ok(());
        }
        let Some(next) = preset_state::manifest_with_reference(&self.project.manifest, id, hash)
        else {
            return Ok(());
        };
        if let Some(fault) = self.controller_boundary(Boundary::Manifest { after: false })? {
            notes.push(format!("manifest reference not updated: {fault}"));
            return Ok(());
        }
        let written =
            preset_state::verify_manifest_unchanged(&self.project.root, &self.project.inventory)
                .map_err(EngineError::from)
                .and_then(|()| {
                    studio_project::lifecycle::write_manifest(&self.project.root, &next)
                        .map_err(EngineError::from)
                });
        match written {
            Ok(()) => {
                self.controller_boundary(Boundary::Manifest { after: true })?;
                if let Err(error) = self.reconcile() {
                    notes.push(format!(
                        "source rescan after the manifest update failed: {error}"
                    ));
                }
            }
            Err(error) => notes.push(format!("manifest reference not updated: {error}")),
        }
        Ok(())
    }

    /// The commit is durable. The source is rescanned (which invalidates the build,
    /// candidate, install authorization and running operations of the old source) and
    /// the lifecycle record persisted. Task history and the saved checkpoint are not
    /// touched.
    fn preset_after_commit(
        &mut self,
        record: TaskRevisionRecord,
        provenance: PresetProvenance,
        reference: Option<(String, String)>,
    ) -> Result<PresetOutcome, EngineError> {
        let mut notes = provenance.notes;
        if let Err(error) = self.reconcile() {
            notes.push(format!("source rescan after publication failed: {error}"));
        }
        if let Some(fault) = self.controller_boundary(Boundary::Database { after: false })? {
            self.projection_stale = true;
            notes.push(format!("history database not updated: {fault}"));
        } else {
            if let Err(error) = self.persist(self.record.clone()) {
                self.projection_stale = true;
                notes.push(format!("history database not updated: {error}"));
            }
            self.controller_boundary(Boundary::Database { after: true })?;
        }
        if let Some((id, hash)) = &reference {
            self.sync_manifest_reference(id, hash, &mut notes)?;
            if self.record.state.source() != &record.published {
                // The manifest replacement is a further source revision after the file set.
                self.persist(self.record.clone()).ok();
            }
        }
        Ok(PresetOutcome {
            record: Some(Box::new(record)),
            source: self.record.state.source().clone(),
            notes,
        })
    }
}

/// Writes the after bytes into a draft copy of the current source.
fn write_changes(draft: &Path, changes: &[FileChange]) -> std::io::Result<()> {
    for change in changes {
        let target = draft.join(change.path.as_str());
        match &change.bytes {
            Some(bytes) => {
                fs::create_dir_all(target.parent().expect("draft paths have parents"))?;
                match fs::remove_file(&target) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e),
                }
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)?;
                file.write_all(bytes)?;
                file.sync_all()?;
            }
            None => {
                match fs::remove_file(&target) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e),
                }
                // The inventory has no directories: an emptied one must go.
                let mut parent = target.parent();
                while let Some(dir) = parent
                    && dir != draft
                    && dir.starts_with(draft)
                    && fs::remove_dir(dir).is_ok()
                {
                    parent = dir.parent();
                }
            }
        }
    }
    Ok(())
}
