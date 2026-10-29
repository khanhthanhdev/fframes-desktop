//! Snapshot undo and redo of the timing document (Cap's editor history): every step
//! stores the whole document, and a gesture (a drag) collects its changes into one step.
//! The props document uses the same history. The timing and slides documents share one
//! history of [`RunTimeDoc`] steps, so undo goes back through the latest gesture of
//! either in order.
use super::doc::TimingDoc;
use crate::slides::SlidesDoc;

/// Undo steps kept; the oldest go first.
pub const MAX_HISTORY: usize = 100;

/// A step of the shared timing and slides history: the whole document it restores.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTimeDoc {
    Timing(TimingDoc),
    Slides(SlidesDoc),
}

impl RunTimeDoc {
    pub fn is_timing(&self) -> bool {
        matches!(self, Self::Timing(_))
    }
    pub fn is_slides(&self) -> bool {
        matches!(self, Self::Slides(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Gesture<D> {
    None,
    /// The document when the gesture began.
    Active(D),
    /// The document changed outside the studio during the gesture: its remaining
    /// edits are refused until it ends. Keeps the document it began with.
    Aborted(D),
}

#[derive(Debug, Clone)]
pub struct History<D = TimingDoc> {
    undo: Vec<D>,
    redo: Vec<D>,
    gesture: Gesture<D>,
}

impl<D> Default for History<D> {
    fn default() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            gesture: Gesture::None,
        }
    }
}

impl<D: Clone + PartialEq> History<D> {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn in_gesture(&self) -> bool {
        self.gesture != Gesture::None
    }
    pub fn gesture_aborted(&self) -> bool {
        matches!(self.gesture, Gesture::Aborted(_))
    }
    /// The document the running gesture (active or aborted) began with.
    pub fn gesture(&self) -> Option<&D> {
        match &self.gesture {
            Gesture::None => None,
            Gesture::Active(start) | Gesture::Aborted(start) => Some(start),
        }
    }
    /// The document the next undo goes back to.
    pub fn peek_undo(&self) -> Option<&D> {
        self.undo.last()
    }
    /// The document the next redo goes forward to.
    pub fn peek_redo(&self) -> Option<&D> {
        self.redo.last()
    }

    /// Starts collecting changes into one step. A second begin keeps the first start.
    pub fn begin_gesture(&mut self, current: &D) {
        if self.gesture == Gesture::None {
            self.gesture = Gesture::Active(current.clone());
        }
    }

    /// Ends the gesture; true when it changed the document and became one undo step.
    pub fn end_gesture(&mut self, current: &D) -> bool {
        match std::mem::replace(&mut self.gesture, Gesture::None) {
            Gesture::Active(start) if start != *current => {
                self.push(start);
                true
            }
            _ => false,
        }
    }

    /// Records `before` as an undo step for a change, unless a gesture collects it.
    pub fn record(&mut self, before: &D) {
        if self.gesture == Gesture::None {
            self.push(before.clone());
        }
    }

    fn push(&mut self, before: D) {
        self.undo.push(before);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The document to go back to, remembering `current` for redo.
    pub fn undo(&mut self, current: &D) -> Option<D> {
        let previous = self.undo.pop()?;
        self.redo.push(current.clone());
        Some(previous)
    }

    pub fn redo(&mut self, current: &D) -> Option<D> {
        let next = self.redo.pop()?;
        self.undo.push(current.clone());
        Some(next)
    }

    /// Forgets every step; a running gesture is aborted, not ended.
    pub fn clear(&mut self) {
        self.clear_where(|_| true);
    }

    /// Forgets the steps `matches` selects, keeping the others in order; a running
    /// gesture that began on a selected document is aborted, not ended.
    pub fn clear_where(&mut self, matches: impl Fn(&D) -> bool) {
        self.undo.retain(|step| !matches(step));
        self.redo.retain(|step| !matches(step));
        if let Gesture::Active(start) = &self.gesture
            && matches(start)
        {
            let Gesture::Active(start) = std::mem::replace(&mut self.gesture, Gesture::None) else {
                unreachable!("just matched");
            };
            self.gesture = Gesture::Aborted(start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timing::doc::TimingClip;

    fn doc(trim: usize) -> TimingDoc {
        TimingDoc {
            clips: vec![TimingClip {
                key: "intro".into(),
                trim_start_frames: trim,
                trim_end_frames: 0,
            }],
            ..TimingDoc::default()
        }
    }

    #[test]
    fn a_gesture_is_one_step() {
        let mut history = History::default();
        history.begin_gesture(&doc(0));
        for trim in 1..=60 {
            history.record(&doc(trim - 1));
        }
        assert!(!history.can_undo());
        assert!(history.end_gesture(&doc(60)));
        assert_eq!(history.undo(&doc(60)), Some(doc(0)));
        assert!(!history.can_undo());
        assert_eq!(history.redo(&doc(0)), Some(doc(60)));
    }

    #[test]
    fn an_unchanged_gesture_leaves_no_step_and_new_changes_drop_redo() {
        let mut history = History::default();
        history.begin_gesture(&doc(0));
        assert!(!history.end_gesture(&doc(0)));
        history.record(&doc(0));
        history.record(&doc(1));
        assert_eq!(history.undo(&doc(2)), Some(doc(1)));
        assert!(history.can_redo());
        history.record(&doc(1));
        assert!(!history.can_redo());
    }

    #[test]
    fn clearing_aborts_a_gesture_until_it_ends() {
        let mut history = History::default();
        history.record(&doc(0));
        history.begin_gesture(&doc(1));
        history.clear();
        assert!(!history.can_undo());
        assert!(history.gesture_aborted());
        assert!(!history.end_gesture(&doc(5)));
        assert!(!history.in_gesture());
    }

    #[test]
    fn a_shared_history_keeps_both_documents_in_order_and_clears_one() {
        let slides = |stops: usize| {
            RunTimeDoc::Slides(SlidesDoc {
                version: 1,
                stops: vec![
                    crate::slides::SlideStop {
                        scene: "intro".into(),
                        at: crate::slides::StopAt::End,
                        title: None,
                        notes: None,
                    };
                    stops
                ],
            })
        };
        let mut history = History::default();
        history.record(&RunTimeDoc::Timing(doc(0)));
        history.record(&slides(0));
        history.record(&RunTimeDoc::Timing(doc(1)));
        assert_eq!(history.peek_undo(), Some(&RunTimeDoc::Timing(doc(1))));

        // A slides drag that an outside write interrupts: the timing steps stay.
        history.begin_gesture(&slides(1));
        history.clear_where(RunTimeDoc::is_slides);
        assert!(history.gesture_aborted());
        assert_eq!(history.gesture(), Some(&slides(1)));
        assert!(!history.end_gesture(&slides(3)));
        assert_eq!(
            history.undo(&RunTimeDoc::Timing(doc(2))),
            Some(RunTimeDoc::Timing(doc(1)))
        );
        assert_eq!(history.peek_undo(), Some(&RunTimeDoc::Timing(doc(0))));
        assert_eq!(history.peek_redo(), Some(&RunTimeDoc::Timing(doc(2))));

        // A timing gesture is not aborted by clearing the slides.
        history.begin_gesture(&RunTimeDoc::Timing(doc(5)));
        history.clear_where(RunTimeDoc::is_slides);
        assert!(!history.gesture_aborted());
    }

    #[test]
    fn history_is_bounded() {
        let mut history = History::default();
        for trim in 0..MAX_HISTORY + 10 {
            history.record(&doc(trim));
        }
        let mut steps = 0;
        let mut current = doc(MAX_HISTORY + 10);
        while let Some(previous) = history.undo(&current) {
            current = previous;
            steps += 1;
        }
        assert_eq!(steps, MAX_HISTORY);
        assert_eq!(current, doc(10));
    }
}
