//! Timing edits of the open project: the timeline's split, trim and move, undo and redo,
//! and the write of `timeline.json` at the end of each gesture.
//!
//! A timing edit is not a source transaction: it writes no journal record and never
//! invalidates the build (the build revision leaves the timing document out). It has
//! the preset rule's gates all the same: an agent task, a blocked publication gate or an
//! unresolved earlier mutation refuses it.
use super::Controller;
use crate::{
    EngineError, OperationTag,
    edit_transaction::{ApplyGate, Boundary, Fault, PromotionError},
    timing::{
        TimelineCommand, TimelineSnapshot, TimingDoc,
        doc::code_scenes,
        edits::{self, ResolvedClip},
        file::{self, WriteError},
        history::RunTimeDoc,
    },
};
use fframes_studio_protocol::PreviewTimelineResponse;
use studio_project::revision::TIMING_FILE;

/// Shown when the live file changed under the session.
const CHANGED_OUTSIDE: &str = "timeline.json changed outside the studio; the timeline was reloaded and its undo history cleared";
/// Shown when an agent edit published a new document.
const PUBLISHED_BY_AGENT: &str = "An agent edit published a new timeline.json; the timeline was reloaded and its undo history cleared";

/// Whether Studio may change the project's timing (`timeline.json`) now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimingEditing {
    Enabled,
    /// Disabled with the reason to show next to the controls.
    Disabled(String),
}

impl Controller {
    /// Whether the timeline can be edited now, with the reason when it cannot.
    pub fn timing_mutation(&mut self) -> TimingEditing {
        if let Err(error) = self.ensure_mutable() {
            return TimingEditing::Disabled(error.to_string());
        }
        if let ApplyGate::Blocked(reason) = self.apply_gate() {
            return TimingEditing::Disabled(format!("Source publication is blocked: {reason}"));
        }
        if self.agent.active().is_some() {
            return TimingEditing::Disabled(
                "An agent task is running; finish or cancel it before editing the timeline".into(),
            );
        }
        if let Err(error) = self.timing.doc() {
            return TimingEditing::Disabled(format!(
                "{error}; fix or remove {TIMING_FILE} to edit the timeline"
            ));
        }
        match self.timing.scenes(self.state().generation()) {
            None => TimingEditing::Disabled(
                "Build the preview to edit the timeline of the current source".into(),
            ),
            Some(Err(reason)) => TimingEditing::Disabled(reason),
            Some(Ok(_)) => TimingEditing::Enabled,
        }
    }

    fn require_timing_mutation(&mut self) -> Result<(), EngineError> {
        match self.timing_mutation() {
            TimingEditing::Enabled => Ok(()),
            TimingEditing::Disabled(reason) => Err(EngineError::Diagnostic(reason)),
        }
    }

    /// Records the scenes a ready preview reported, when it previews the current build:
    /// one built from it, or the adopted preview of the promotion that published it.
    pub fn learn_timeline(&mut self, tag: &OperationTag, timeline: &PreviewTimelineResponse) {
        let state = self.state();
        let promoted = state.promotion().is_some_and(|authorization| {
            authorization.tag() == tag && authorization.is_current(state)
        });
        if state.is_current_build(tag) || promoted {
            let generation = self.state().generation();
            self.timing.learn_scenes(generation, code_scenes(timeline));
            // The layout the person sees, which the slide stops resolve against.
            self.slides.learn_layout(generation, timeline);
        }
    }

    /// The clips as they play now; empty until the current build's scenes are known.
    fn timeline_clips(&self) -> Vec<ResolvedClip> {
        match (
            self.timing.doc(),
            self.timing.scenes(self.state().generation()),
        ) {
            (Ok(doc), Some(Ok(scenes))) => edits::resolve(doc, scenes)
                .map(|(clips, _)| clips)
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    pub fn timeline_snapshot(&mut self) -> TimelineSnapshot {
        TimelineSnapshot {
            generation: self.timing.generation(),
            timing_json: self.timing.timing_json(),
            clips: self.timeline_clips(),
            // One history for the timing and slides documents.
            can_undo: self.run_time_history.can_undo() && !self.run_time_history.in_gesture(),
            can_redo: self.run_time_history.can_redo() && !self.run_time_history.in_gesture(),
            editing: self.timing_mutation(),
            notice: self.timing.notice().map(str::to_owned),
        }
    }

    /// The timing document and its generation, for a preview or an export.
    pub fn timing_for_preview(&self) -> (Option<String>, u64) {
        (self.timing.timing_json(), self.timing.generation())
    }

    /// The document an export plays: `None` (the code order) unless it has clips, so a
    /// project whose worker cannot export timing still exports without an edit. An
    /// invalid document refuses the export with its reason.
    pub fn timing_for_export(&self) -> Result<Option<String>, String> {
        let doc = self
            .timing
            .doc()
            .map_err(|error| format!("{error}; fix or remove {TIMING_FILE} to export"))?;
        Ok((!doc.clips.is_empty()).then(|| doc.to_json()))
    }

    /// Applies one timeline command. Edits outside a gesture are written at once; inside
    /// one they only preview, and the gesture's end writes them as one undo step.
    pub fn timeline_command(&mut self, command: TimelineCommand) -> Result<(), EngineError> {
        match command {
            TimelineCommand::Split { clip, at } => {
                self.edit_timeline(|clips| edits::split(clips, clip, at))
            }
            TimelineCommand::Trim { clip, edge, frames } => {
                self.edit_timeline(|clips| edits::trim(clips, clip, edge, frames))
            }
            TimelineCommand::Move { clip, to } => {
                self.edit_timeline(|clips| edits::move_clip(clips, clip, to))
            }
            TimelineCommand::BeginGesture => {
                self.require_timing_mutation()?;
                self.refuse_other_gesture(true)?;
                let doc = self.timing.doc().map_err(EngineError::Diagnostic)?.clone();
                self.run_time_history
                    .begin_gesture(&RunTimeDoc::Timing(doc));
                Ok(())
            }
            TimelineCommand::EndGesture => self.end_timeline_gesture(),
            TimelineCommand::Undo | TimelineCommand::Redo => {
                self.undo_run_time(command == TimelineCommand::Redo)
            }
        }
    }

    /// Undoes (or redoes) the latest step of the shared history, a timing or a slides
    /// gesture, and writes the document it changes.
    fn undo_run_time(&mut self, redo: bool) -> Result<(), EngineError> {
        let next = if redo {
            self.run_time_history.peek_redo()
        } else {
            self.run_time_history.peek_undo()
        };
        let slides = next.is_some_and(RunTimeDoc::is_slides);
        if slides {
            self.require_slides_mutation()?;
        } else {
            self.require_timing_mutation()?;
        }
        if self.run_time_history.in_gesture() {
            return Err(EngineError::Diagnostic(
                "Finish the drag before undoing a timeline edit".into(),
            ));
        }
        let current = if slides {
            RunTimeDoc::Slides(self.slides.doc().map_err(EngineError::Diagnostic)?.clone())
        } else {
            RunTimeDoc::Timing(self.timing.doc().map_err(EngineError::Diagnostic)?.clone())
        };
        let next = if redo {
            self.run_time_history.redo(&current)
        } else {
            self.run_time_history.undo(&current)
        };
        match next {
            None => Err(EngineError::Diagnostic(
                match (redo, self.timing.notice().or(self.slides.notice())) {
                    (_, Some(notice)) => notice.to_owned(),
                    (false, None) => "There is no timeline edit to undo".into(),
                    (true, None) => "There is no timeline edit to redo".into(),
                },
            )),
            Some(RunTimeDoc::Timing(doc)) => {
                self.timing.set_doc(doc);
                self.write_timing()
            }
            Some(RunTimeDoc::Slides(doc)) => {
                self.slides.set_doc(doc);
                self.write_slides()
            }
        }
    }

    /// Refuses an edit of the timing (`timing`) or the slides document while a drag of
    /// the other one runs: its steps would fall into that drag's undo step.
    pub(super) fn refuse_other_gesture(&self, timing: bool) -> Result<(), EngineError> {
        match self.run_time_history.gesture() {
            Some(start) if start.is_timing() != timing => Err(EngineError::Diagnostic(
                "Finish the drag before making another edit".into(),
            )),
            _ => Ok(()),
        }
    }

    /// Reloads the timing document after an outside change and forgets its undo steps.
    fn reload_timing(&mut self, reason: String) {
        let root = self.project.root.clone();
        self.timing.reload(&root, reason);
        self.run_time_history.clear_where(RunTimeDoc::is_timing);
    }

    fn edit_timeline(
        &mut self,
        edit: impl FnOnce(&mut Vec<ResolvedClip>) -> Result<(), String>,
    ) -> Result<(), EngineError> {
        self.require_timing_mutation()?;
        self.refuse_other_gesture(true)?;
        if self.run_time_history.gesture_aborted() {
            return Err(EngineError::Diagnostic(CHANGED_OUTSIDE.into()));
        }
        let generation = self.state().generation();
        let Some(Ok(scenes)) = self.timing.scenes(generation) else {
            unreachable!("timing_mutation requires the scenes");
        };
        let doc = self.timing.doc().map_err(EngineError::Diagnostic)?.clone();
        let (mut clips, orphans) = edits::resolve(&doc, scenes).map_err(EngineError::Diagnostic)?;
        edit(&mut clips).map_err(EngineError::Diagnostic)?;
        let next = edits::to_doc(&clips, &orphans);
        if next == doc {
            return Ok(());
        }
        self.run_time_history.record(&RunTimeDoc::Timing(doc));
        self.timing.set_doc(next);
        if self.run_time_history.in_gesture() {
            Ok(())
        } else {
            self.write_timing()
        }
    }

    fn end_timeline_gesture(&mut self) -> Result<(), EngineError> {
        if !self
            .run_time_history
            .gesture()
            .is_some_and(RunTimeDoc::is_timing)
        {
            return Ok(());
        }
        if let TimingEditing::Disabled(reason) = self.timing_mutation() {
            // Nothing of the gesture is written: the file stands, and the session
            // goes back to it.
            self.run_time_history
                .end_gesture(&RunTimeDoc::Timing(TimingDoc::default()));
            self.reload_timing(reason.clone());
            return Err(EngineError::Diagnostic(reason));
        }
        let doc = self.timing.doc().map_err(EngineError::Diagnostic)?.clone();
        if self.run_time_history.end_gesture(&RunTimeDoc::Timing(doc)) {
            self.write_timing()
        } else {
            Ok(())
        }
    }

    /// Writes the session's document to `timeline.json` and rescans; the rescan sees a
    /// timing-only change, which keeps the build.
    fn write_timing(&mut self) -> Result<(), EngineError> {
        let json = self
            .timing
            .doc()
            .map_err(EngineError::Diagnostic)?
            .to_json();
        let root = self.project.root.clone();
        let hooks = self.hooks.clone();
        let mut crashed = false;
        let written = file::write(&root, &json, self.timing.known(), &mut |step| match hooks
            .at(&Boundary::TimingWrite(step))
        {
            Ok(()) => Ok(()),
            Err(Fault::Crash) => {
                crashed = true;
                Err(std::io::Error::other("interrupted at an injected boundary"))
            }
            Err(Fault::Io(kind)) => Err(std::io::Error::new(kind, "injected")),
        });
        match written {
            Ok(known) => {
                self.timing.set_known(known);
                self.reconcile()
            }
            // The process "died" here: nothing is repaired until the project reopens.
            Err(_) if crashed => {
                self.recovery_status.suspended = Some("interrupted at an injected boundary".into());
                Err(PromotionError::Crashed.into())
            }
            Err(error) => {
                if matches!(error, WriteError::Io(_)) {
                    // An interrupted write may have moved the live file aside.
                    let _ = file::recover(&root);
                }
                let reason = error.to_string();
                self.reload_timing(reason.clone());
                let _ = self.reconcile();
                Err(EngineError::Diagnostic(reason))
            }
        }
    }

    /// Reloads the document when the scan found a `timeline.json` the session did not
    /// write; its own writes are recognised by their hash.
    pub(super) fn observe_timing_file(&mut self) {
        let live = self
            .project
            .inventory
            .timing_file()
            .map(|file| file.sha256.clone());
        if !self.timing.is_known(live.as_deref()) {
            let state = self.state();
            let published = state
                .task_history()
                .head()
                .is_some_and(|record| record.published == *state.source());
            let reason = if published {
                PUBLISHED_BY_AGENT
            } else {
                CHANGED_OUTSIDE
            };
            self.reload_timing(reason.into());
        }
    }
}
