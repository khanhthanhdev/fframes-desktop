//! Slides edits of the open project: stops, their titles and notes, and the write of
//! `slides.json` at the end of each gesture. Undo and redo are the timeline's: one
//! history holds the timing and slides steps in order.
//!
//! Like a timing edit, a slides edit is not a source transaction: it writes no journal
//! record and never invalidates the build (the build revision leaves the slides document
//! out). It has the same gates: an agent task, a blocked publication gate or an
//! unresolved earlier mutation refuses it.
use super::Controller;
use crate::{
    EngineError,
    edit_transaction::{ApplyGate, Boundary, Fault, PromotionError},
    slides::{
        SLIDES, SlideStop, SlidesCommand, SlidesDoc, SlidesEditing, SlidesSnapshot, StopAt,
        resolve::{self, ResolvedStop, SlidesProblem},
    },
    timing::{
        file::{self, WriteError},
        history::RunTimeDoc,
    },
};
use studio_project::revision::SLIDES_FILE;

/// Shown when the live file changed under the session.
const CHANGED_OUTSIDE: &str = "slides.json changed outside the studio; the slides were reloaded and their undo history cleared";
/// Shown when an agent edit published a new document.
const PUBLISHED_BY_AGENT: &str = "An agent edit published a new slides.json; the slides were reloaded and their undo history cleared";

impl Controller {
    /// Whether the slides can be edited now, with the reason when they cannot.
    pub fn slides_mutation(&mut self) -> SlidesEditing {
        if let Err(error) = self.ensure_mutable() {
            return SlidesEditing::Disabled(error.to_string());
        }
        if let ApplyGate::Blocked(reason) = self.apply_gate() {
            return SlidesEditing::Disabled(format!("Source publication is blocked: {reason}"));
        }
        if self.agent.active().is_some() {
            return SlidesEditing::Disabled(
                "An agent task is running; finish or cancel it before editing slides".into(),
            );
        }
        match self.slides.doc() {
            Ok(_) => SlidesEditing::Enabled,
            Err(error) => SlidesEditing::Disabled(format!(
                "{error}; fix or remove {SLIDES_FILE} to edit slides"
            )),
        }
    }

    pub(super) fn require_slides_mutation(&mut self) -> Result<(), EngineError> {
        match self.slides_mutation() {
            SlidesEditing::Enabled => Ok(()),
            SlidesEditing::Disabled(reason) => Err(EngineError::Diagnostic(reason)),
        }
    }

    /// The stops at video frames of the timeline the person sees and what did not
    /// resolve; `Err` says why nothing can resolve now.
    pub fn resolved_slides(&self) -> Result<(Vec<ResolvedStop>, Vec<SlidesProblem>), String> {
        let doc = self.slides.doc()?;
        let timeline = self
            .slides
            .layout(self.state().generation(), self.timing.generation())?;
        Ok(resolve::resolve(doc, timeline))
    }

    pub fn slides_snapshot(&mut self) -> SlidesSnapshot {
        let (stops, problems, unavailable) = match self.resolved_slides() {
            Ok((stops, problems)) => (stops, problems, None),
            Err(reason) => (Vec::new(), Vec::new(), Some(reason)),
        };
        SlidesSnapshot {
            generation: self.slides.generation(),
            doc: self.slides.doc().ok().cloned(),
            stops,
            problems,
            unavailable,
            editing: self.slides_mutation(),
            notice: self.slides.notice().map(str::to_owned),
        }
    }

    /// Applies one slides command. Edits outside a gesture are written at once; inside
    /// one they only preview, and the gesture's end writes them as one undo step.
    pub fn slides_command(&mut self, command: SlidesCommand) -> Result<(), EngineError> {
        match command {
            SlidesCommand::SetSlides(doc) => self.edit_slides(|current| {
                *current = doc;
                Ok(())
            }),
            SlidesCommand::AddStop(stop) => self.edit_slides(|doc| {
                doc.stops.push(stop);
                Ok(())
            }),
            SlidesCommand::AcceptSuggestions {
                identity,
                timing_generation,
                props_generation,
                stops,
            } => {
                let timeline = self
                    .slides
                    .layout(self.state().generation(), self.timing.generation())
                    .map_err(EngineError::Diagnostic)?
                    .clone();
                if timeline.envelope.identity != identity
                    || timeline.timing_generation != timing_generation
                    || self.props.generation() != props_generation
                {
                    return Err(EngineError::Diagnostic(
                        "Scan again: the preview, timeline or props changed".into(),
                    ));
                }
                self.edit_slides(|doc| {
                    let mut occupied: std::collections::BTreeSet<_> =
                        resolve::resolve(doc, &timeline)
                            .0
                            .into_iter()
                            .filter(|stop| matches!(stop.source, resolve::StopSource::Explicit(_)))
                            .map(|stop| stop.video_frame)
                            .collect();
                    for stop in stops {
                        let candidate = SlidesDoc {
                            stops: vec![stop.clone()],
                            ..SlidesDoc::default()
                        };
                        candidate.validate()?;
                        let resolved = resolve::resolve(&candidate, &timeline)
                            .0
                            .into_iter()
                            .find(|stop| stop.source == resolve::StopSource::Explicit(0))
                            .ok_or_else(|| {
                                "A suggestion no longer resolves; scan again".to_owned()
                            })?;
                        if occupied.insert(resolved.video_frame) {
                            doc.stops.push(stop);
                        }
                    }
                    Ok(())
                })
            }
            SlidesCommand::MoveStop { stop, scene, at } => self.edit_slides(|doc| {
                let found = stop_mut(doc, stop)?;
                found.scene = scene;
                found.at = at;
                Ok(())
            }),
            SlidesCommand::RemoveStop { stop } => self.edit_slides(|doc| {
                stop_mut(doc, stop)?;
                doc.stops.remove(stop);
                Ok(())
            }),
            SlidesCommand::SetStopText { stop, title, notes } => self.edit_slides(|doc| {
                let found = stop_mut(doc, stop)?;
                found.title = title.filter(|text| !text.is_empty());
                found.notes = notes.filter(|text| !text.is_empty());
                Ok(())
            }),
            SlidesCommand::MaterializeDefaultStops => {
                let timeline = self
                    .slides
                    .layout(self.state().generation(), self.timing.generation())
                    .map_err(EngineError::Diagnostic)?;
                let mut clips: Vec<_> = timeline.scenes.iter().collect();
                clips.sort_by_key(|scene| scene.start_frame);
                let stops = clips
                    .into_iter()
                    .map(|scene| {
                        scene
                            .clip
                            .as_ref()
                            .map(|clip| SlideStop {
                                scene: clip.timing_key.clone(),
                                at: StopAt::End,
                                title: None,
                                notes: None,
                            })
                            .ok_or_else(|| resolve::WORKER_TOO_OLD.to_owned())
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(EngineError::Diagnostic)?;
                self.edit_slides(|doc| {
                    if !doc.stops.is_empty() {
                        return Err(format!("{SLIDES_FILE} already lists its stops"));
                    }
                    doc.stops = stops;
                    Ok(())
                })
            }
            SlidesCommand::BeginGesture => {
                self.require_slides_mutation()?;
                self.refuse_other_gesture(false)?;
                let doc = self.slides.doc().map_err(EngineError::Diagnostic)?.clone();
                self.run_time_history
                    .begin_gesture(&RunTimeDoc::Slides(doc));
                Ok(())
            }
            SlidesCommand::EndGesture => self.end_slides_gesture(),
        }
    }

    fn edit_slides(
        &mut self,
        edit: impl FnOnce(&mut SlidesDoc) -> Result<(), String>,
    ) -> Result<(), EngineError> {
        self.require_slides_mutation()?;
        self.refuse_other_gesture(false)?;
        if self.run_time_history.gesture_aborted() {
            return Err(EngineError::Diagnostic(CHANGED_OUTSIDE.into()));
        }
        let doc = self.slides.doc().map_err(EngineError::Diagnostic)?.clone();
        let mut next = doc.clone();
        edit(&mut next).map_err(EngineError::Diagnostic)?;
        next.validate()
            .and_then(|()| next.check_size())
            .map_err(|error| EngineError::Diagnostic(format!("{SLIDES_FILE}: {error}")))?;
        if next == doc {
            return Ok(());
        }
        self.run_time_history.record(&RunTimeDoc::Slides(doc));
        self.slides.set_doc(next);
        if self.run_time_history.in_gesture() {
            Ok(())
        } else {
            self.write_slides()
        }
    }

    fn end_slides_gesture(&mut self) -> Result<(), EngineError> {
        if !self
            .run_time_history
            .gesture()
            .is_some_and(RunTimeDoc::is_slides)
        {
            return Ok(());
        }
        if let SlidesEditing::Disabled(reason) = self.slides_mutation() {
            // Nothing of the gesture is written: the file stands, and the session goes
            // back to it.
            self.run_time_history
                .end_gesture(&RunTimeDoc::Slides(SlidesDoc::default()));
            self.reload_slides(reason.clone());
            return Err(EngineError::Diagnostic(reason));
        }
        let doc = self.slides.doc().map_err(EngineError::Diagnostic)?.clone();
        if self.run_time_history.end_gesture(&RunTimeDoc::Slides(doc)) {
            self.write_slides()
        } else {
            Ok(())
        }
    }

    /// Writes the session's document to `slides.json` and rescans; the rescan sees a
    /// run-time-only change, which keeps the build.
    pub(super) fn write_slides(&mut self) -> Result<(), EngineError> {
        let json = self
            .slides
            .doc()
            .map_err(EngineError::Diagnostic)?
            .to_json();
        let root = self.project.root.clone();
        let hooks = self.hooks.clone();
        let mut crashed = false;
        let written =
            file::write_doc(
                &root,
                &SLIDES,
                &json,
                self.slides.known(),
                &mut |step| match hooks.at(&Boundary::SlidesWrite(step)) {
                    Ok(()) => Ok(()),
                    Err(Fault::Crash) => {
                        crashed = true;
                        Err(std::io::Error::other("interrupted at an injected boundary"))
                    }
                    Err(Fault::Io(kind)) => Err(std::io::Error::new(kind, "injected")),
                },
            );
        match written {
            Ok(known) => {
                self.slides.set_known(known);
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
                    let _ = file::recover_doc(&root, &SLIDES);
                }
                let reason = error.to_string();
                self.reload_slides(reason.clone());
                let _ = self.reconcile();
                Err(EngineError::Diagnostic(reason))
            }
        }
    }

    /// Reloads the slides document after an outside change and forgets its undo steps;
    /// the timing steps stay.
    fn reload_slides(&mut self, reason: String) {
        let root = self.project.root.clone();
        self.slides.reload(&root, reason);
        self.run_time_history.clear_where(RunTimeDoc::is_slides);
    }

    /// Reloads the document when the scan found a `slides.json` the session did not
    /// write; its own writes are recognised by their hash.
    pub(super) fn observe_slides_file(&mut self) {
        let live = self
            .project
            .inventory
            .slides_file()
            .map(|file| file.sha256.clone());
        if !self.slides.is_known(live.as_deref()) {
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
            self.reload_slides(reason.into());
        }
    }
}

fn stop_mut(doc: &mut SlidesDoc, stop: usize) -> Result<&mut SlideStop, String> {
    let count = doc.stops.len();
    doc.stops
        .get_mut(stop)
        .ok_or_else(|| format!("there is no stop {} ({count} stops)", stop + 1))
}
