//! Timeline and slide stop edits on their way to the engine. The engine runs one command
//! at a time and the shell drops what arrives while one runs, so edits wait here in
//! order: a drag sends a trim (or a stop move) per pointer move, and one replaces a queued
//! one of the same clip edge (or stop). Both documents share one queue, as they share one
//! undo history in the engine: an undo runs after every edit made before it.

use gpui::{
    Context, DispatchPhase, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, Styled, canvas,
};
use studio_engine::engine::backend::Command;
use studio_engine::slides::SlidesCommand;
use studio_engine::timing::TimelineCommand;

use super::StudioShell;
use crate::timeline::TimelineControls;

/// An edit of a run-time document the timeline changes: the timing or the slide stops.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum DocEdit {
    Timing(TimelineCommand),
    Slides(SlidesCommand),
}

impl DocEdit {
    fn command(&self) -> Command {
        match self {
            DocEdit::Timing(command) => Command::Timeline(command.clone()),
            DocEdit::Slides(command) => Command::Slides(command.clone()),
        }
    }

    /// Whether this edit opens (`end` false) or closes (`end` true) a gesture.
    fn is_gesture(&self, end: bool) -> bool {
        match self {
            DocEdit::Timing(command) => {
                *command
                    == if end {
                        TimelineCommand::EndGesture
                    } else {
                        TimelineCommand::BeginGesture
                    }
            }
            DocEdit::Slides(command) => {
                *command
                    == if end {
                        SlidesCommand::EndGesture
                    } else {
                        SlidesCommand::BeginGesture
                    }
            }
        }
    }

    /// A step of a drag: a trim or a stop move.
    fn is_drag_step(&self) -> bool {
        matches!(
            self,
            DocEdit::Timing(TimelineCommand::Trim { .. })
                | DocEdit::Slides(SlidesCommand::MoveStop { .. })
        )
    }
}

/// Whether `edit` makes the queued edit `queued` pointless: a later trim of the same clip
/// edge, or a later move of the same stop.
fn replaces(edit: &DocEdit, queued: Option<&DocEdit>) -> bool {
    match (edit, queued) {
        (
            DocEdit::Timing(TimelineCommand::Trim { clip, edge, .. }),
            Some(DocEdit::Timing(TimelineCommand::Trim {
                clip: queued_clip,
                edge: queued_edge,
                ..
            })),
        ) => clip == queued_clip && edge == queued_edge,
        (
            DocEdit::Slides(SlidesCommand::MoveStop { stop, .. }),
            Some(DocEdit::Slides(SlidesCommand::MoveStop {
                stop: queued_stop, ..
            })),
        ) => stop == queued_stop,
        _ => false,
    }
}

impl StudioShell {
    /// Queues `command` and sends it when the engine is free.
    pub(super) fn timeline_edit(&mut self, command: TimelineCommand, cx: &mut Context<Self>) {
        self.doc_edit(DocEdit::Timing(command), cx);
    }

    /// Queues a slide stop edit behind the timeline edits and sends it when the engine
    /// is free.
    pub(super) fn slides_edit(&mut self, command: SlidesCommand, cx: &mut Context<Self>) {
        self.doc_edit(DocEdit::Slides(command), cx);
    }

    fn doc_edit(&mut self, edit: DocEdit, cx: &mut Context<Self>) {
        if replaces(&edit, self.timeline_queue.back()) {
            self.timeline_queue.pop_back();
        }
        self.timeline_queue.push_back(edit);
        self.flush_timeline_edits(cx);
    }

    /// Sends the oldest queued edit when no command is running.
    pub(super) fn flush_timeline_edits(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(edit) = self.timeline_queue.pop_front() else {
            return;
        };
        self.dispatch(edit.command(), cx);
        self.timeline_in_flight = self.in_flight.map(|(id, _)| (id, edit));
    }

    /// The engine answered the edit it was sent. A refused `BeginGesture` never opened its
    /// gesture, so the gesture's trims or stop moves are dropped (outside a gesture each
    /// would be written and undone on its own) and a drag still under way is let go.
    pub(super) fn timeline_edit_finished(
        &mut self,
        edit: DocEdit,
        refused: bool,
        cx: &mut Context<Self>,
    ) {
        if !edit.is_gesture(false) || !refused {
            return;
        }
        let same = |queued: &DocEdit| {
            matches!(
                (&edit, queued),
                (DocEdit::Timing(_), DocEdit::Timing(_)) | (DocEdit::Slides(_), DocEdit::Slides(_))
            )
        };
        match self
            .timeline_queue
            .iter()
            .position(|queued| same(queued) && queued.is_gesture(true))
        {
            Some(end) => {
                self.timeline_queue.drain(..=end);
            }
            None => {
                self.timeline_queue
                    .retain(|queued| !(same(queued) && queued.is_drag_step()));
                self.timeline
                    .update(cx, |timeline, cx| timeline.cancel_edit_drag(cx));
            }
        }
    }

    /// Hands the slide stops of the latest presentation to the timeline's markers and the
    /// Slides tab, with the layout they resolved against. Runs when the shell renders,
    /// which every new presentation (a command's, a refresh's, a gated preview's) makes
    /// it do; the views re-render only when what they show changed.
    pub(super) fn sync_slides(&mut self, cx: &mut Context<Self>) {
        let slides = self.presentation.slides.clone();
        let layout = self.timeline.update(cx, |timeline, cx| {
            timeline.set_slides(slides.clone(), cx);
            timeline.stop_layout()
        });
        let scan = self
            .preview
            .as_ref()
            .and_then(|preview| preview.stop_scan())
            .filter(|scan| {
                self.preview_state.displayed() == Some(&scan.timeline.envelope.identity)
                    && self.presentation.timeline.as_ref().is_some_and(|timeline| {
                        timeline.generation == scan.timeline.timing_generation
                    })
                    && self
                        .presentation
                        .props
                        .as_ref()
                        .is_some_and(|props| props.generation == scan.props_generation)
            });
        self.slides_panel.update(cx, |panel, cx| {
            panel.set_slides(slides, layout, cx);
            panel.set_scan(scan, cx);
        });
    }

    /// Undo or redo from the keyboard, sent only when the engine has a step to take or
    /// edits are still on their way to it, so a stray Ctrl+Z says nothing.
    pub(super) fn timeline_history(&mut self, command: TimelineCommand, cx: &mut Context<Self>) {
        // A presentation shows no timeline to undo on.
        if self.presenting.is_some() {
            return;
        }
        let step = self.presentation.timeline.as_ref().is_some_and(|timeline| {
            if command == TimelineCommand::Undo {
                timeline.can_undo
            } else {
                timeline.can_redo
            }
        });
        let pending = !self.timeline_queue.is_empty() || self.timeline_in_flight.is_some();
        if !self.timeline.read(cx).edit_dragging() && (step || pending) {
            self.timeline_edit(command, cx);
        }
    }

    /// The timeline's controls changed: the header draws undo and redo, the transport
    /// draws zoom, Fit, Split and Cycle. Only the section that shows a change renders.
    pub(super) fn timeline_controls_changed(
        &mut self,
        controls: TimelineControls,
        cx: &mut Context<Self>,
    ) {
        let old = std::mem::replace(&mut self.timeline_controls, controls);
        let Some(sections) = &self.sections else {
            cx.notify();
            return;
        };
        if (old.editable, old.can_undo, old.can_redo)
            != (controls.editable, controls.can_undo, controls.can_redo)
        {
            sections.header.update(cx, |_, cx| cx.notify());
        }
        sections.player.update(cx, |_, cx| cx.notify());
    }

    /// While a clip is dragged, follows the pointer through the whole window, including
    /// past its edges, where element listeners stop firing.
    pub(super) fn render_edit_tracking(&self) -> gpui::AnyElement {
        let timeline = self.timeline.clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let moved = timeline.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    moved.update(cx, |timeline, cx| {
                        // A release outside the window never arrives; a move without the
                        // button ends the drag instead.
                        if event.pressed_button == Some(MouseButton::Left) {
                            let x = f32::from(event.position.x);
                            timeline.edit_drag_move(x, event.modifiers.shift, cx);
                        } else {
                            timeline.end_edit_drag(cx);
                        }
                    });
                });
                let released = timeline.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        released.update(cx, |timeline, cx| timeline.end_edit_drag(cx));
                    }
                });
            },
        )
        .absolute()
        .size_0()
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_engine::slides::StopAt;
    use studio_engine::timing::TrimEdge;

    fn trim(clip: usize, frames: usize) -> DocEdit {
        DocEdit::Timing(TimelineCommand::Trim {
            clip,
            edge: TrimEdge::End,
            frames,
        })
    }

    fn move_stop(stop: usize, at: usize) -> DocEdit {
        DocEdit::Slides(SlidesCommand::MoveStop {
            stop,
            scene: "intro".into(),
            at: StopAt::Frame(at),
        })
    }

    #[test]
    fn a_drag_step_replaces_only_a_queued_step_of_the_same_clip_edge_or_stop() {
        assert!(replaces(&trim(1, 5), Some(&trim(1, 4))));
        assert!(!replaces(&trim(1, 5), Some(&trim(2, 4))));
        assert!(replaces(&move_stop(0, 9), Some(&move_stop(0, 8))));
        assert!(!replaces(&move_stop(0, 9), Some(&move_stop(1, 8))));
        // A stop move never swallows a trim, nor the other way round: undo would
        // otherwise skip one of them.
        assert!(!replaces(&move_stop(0, 9), Some(&trim(0, 8))));
        assert!(!replaces(&trim(0, 9), Some(&move_stop(0, 8))));
        assert!(!replaces(
            &DocEdit::Slides(SlidesCommand::EndGesture),
            Some(&DocEdit::Slides(SlidesCommand::EndGesture))
        ));
        assert!(DocEdit::Slides(SlidesCommand::BeginGesture).is_gesture(false));
        assert!(DocEdit::Timing(TimelineCommand::EndGesture).is_gesture(true));
        assert!(!move_stop(0, 1).is_gesture(false));
    }
}
