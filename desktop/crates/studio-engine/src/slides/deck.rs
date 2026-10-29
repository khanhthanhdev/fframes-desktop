//! Present mode's navigator: where the deck holds and what Next and Previous do.
//!
//! Pure: it knows only the hold frames, frame 0 and every resolved stop. Slide `n` plays
//! from hold `n - 1` to hold `n` and then holds there until the presenter advances.
use super::resolve::ResolvedStop;

/// What the player does for a navigation key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeckStep {
    /// Play from the current frame and hold at this frame.
    PlayTo(usize),
    /// Seek to this frame and hold there.
    HoldAt(usize),
    /// Nothing to do: the deck is at its end (Next) or its start (Previous).
    Stay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deck {
    /// Frame 0, then every stop before `total_frames`, sorted and without duplicates.
    holds: Vec<usize>,
}

impl Deck {
    /// The deck of `stops` (resolved, sorted by video frame) in a video of `total_frames`.
    /// The video's last frame is always the final hold.
    pub fn new(stops: &[ResolvedStop], total_frames: usize) -> Self {
        let mut holds = vec![0];
        holds.extend(
            stops
                .iter()
                .map(|stop| stop.video_frame)
                .filter(|frame| *frame > 0 && *frame < total_frames),
        );
        if let Some(last) = total_frames.checked_sub(1) {
            holds.push(last);
        }
        holds.sort_unstable();
        holds.dedup();
        Self { holds }
    }

    /// How many slides the deck has: one per stop after frame 0.
    pub fn slides(&self) -> usize {
        self.holds.len().saturating_sub(1)
    }

    /// The hold frames, frame 0 first.
    pub fn holds(&self) -> &[usize] {
        &self.holds
    }

    /// Next: while playing, jump to the stop being played to and hold; while held, play
    /// to the next stop.
    pub fn next(&self, position: usize, playing: bool) -> DeckStep {
        match self.holds.iter().find(|hold| **hold > position) {
            Some(&hold) if playing => DeckStep::HoldAt(hold),
            Some(&hold) => DeckStep::PlayTo(hold),
            None => DeckStep::Stay,
        }
    }

    /// Previous: hold at the stop before `position`.
    pub fn previous(&self, position: usize) -> DeckStep {
        match self.holds.iter().rev().find(|hold| **hold < position) {
            Some(&hold) => DeckStep::HoldAt(hold),
            None => DeckStep::Stay,
        }
    }

    /// The hold before slide `slide` (1-based), where the slide starts; `None` for a slide
    /// the deck does not have.
    pub fn start_of(&self, slide: usize) -> Option<usize> {
        (1..=self.slides())
            .contains(&slide)
            .then(|| self.holds[slide - 1])
    }

    /// The hold frame nearest to `position`; of two equally near, the earlier.
    pub fn nearest(&self, position: usize) -> usize {
        self.holds
            .iter()
            .copied()
            .min_by_key(|hold| hold.abs_diff(position))
            .unwrap_or(0)
    }

    /// The 1-based slide `position` is in: the slide that plays up to the first stop at or
    /// after it (a held stop counts for the slide that ended there).
    pub fn slide_at(&self, position: usize) -> usize {
        let before = self.holds[1..]
            .iter()
            .filter(|hold| **hold < position)
            .count();
        (before + 1).min(self.slides().max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slides::StopSource;

    fn deck(frames: &[usize], total: usize) -> Deck {
        let stops: Vec<_> = frames
            .iter()
            .map(|&video_frame| ResolvedStop {
                video_frame,
                title: None,
                notes: None,
                source: StopSource::Default,
            })
            .collect();
        Deck::new(&stops, total)
    }

    #[test]
    fn next_from_the_start_plays_to_the_first_stop() {
        let deck = deck(&[30, 60, 89], 90);
        assert_eq!(deck.holds(), &[0, 30, 60, 89]);
        assert_eq!(deck.slides(), 3);
        assert_eq!(deck.next(0, false), DeckStep::PlayTo(30));
        assert_eq!(deck.next(30, false), DeckStep::PlayTo(60));
    }

    #[test]
    fn next_while_playing_holds_at_the_target() {
        let deck = deck(&[30, 60, 89], 90);
        assert_eq!(deck.next(12, true), DeckStep::HoldAt(30));
        assert_eq!(deck.next(45, true), DeckStep::HoldAt(60));
    }

    #[test]
    fn previous_holds_at_the_stop_before() {
        let deck = deck(&[30, 60, 89], 90);
        assert_eq!(deck.previous(60), DeckStep::HoldAt(30));
        // From the first stop back to the start, and no further.
        assert_eq!(deck.previous(30), DeckStep::HoldAt(0));
        assert_eq!(deck.previous(0), DeckStep::Stay);
        // While playing slide 2, back to where it started.
        assert_eq!(deck.previous(45), DeckStep::HoldAt(30));
    }

    #[test]
    fn next_at_the_last_stop_stays() {
        let deck = deck(&[30, 60, 89], 90);
        assert_eq!(deck.next(89, false), DeckStep::Stay);
        assert_eq!(deck.next(90, false), DeckStep::Stay);
    }

    #[test]
    fn nearest_follows_changed_stops() {
        let before = deck(&[30, 60, 89], 90);
        assert_eq!(before.nearest(60), 60);
        // A trim moved the stops: the held frame snaps to the closest new one.
        let after = deck(&[25, 50, 79], 80);
        assert_eq!(after.nearest(60), 50);
        assert_eq!(after.nearest(65), 79);
        // Ties go to the earlier stop.
        assert_eq!(deck(&[10, 20], 30).nearest(15), 10);
        // Past a shortened video, the last frame.
        assert_eq!(after.nearest(200), 79);
    }

    #[test]
    fn slides_count_from_one_and_start_at_the_hold_before() {
        let deck = deck(&[30, 60, 89], 90);
        assert_eq!(deck.slide_at(0), 1);
        assert_eq!(deck.slide_at(30), 1);
        assert_eq!(deck.slide_at(31), 2);
        assert_eq!(deck.slide_at(89), 3);
        assert_eq!(deck.start_of(1), Some(0));
        assert_eq!(deck.start_of(3), Some(60));
        assert_eq!(deck.start_of(0), None);
        assert_eq!(deck.start_of(4), None);
    }

    #[test]
    fn stops_outside_the_video_are_ignored_and_the_end_is_implicit() {
        let deck = deck(&[0, 30, 30, 120], 90);
        assert_eq!(deck.holds(), &[0, 30, 89]);
        let empty = super::Deck::new(&[], 0);
        assert_eq!(empty.holds(), &[0]);
        assert_eq!(empty.slides(), 0);
        assert_eq!(empty.next(0, false), DeckStep::Stay);
        assert_eq!(empty.slide_at(0), 1);
    }
}
