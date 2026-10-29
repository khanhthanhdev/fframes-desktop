//! The one outcome vocabulary: how a finished task reads in the transcript and in History.

use studio_engine::agent::OutcomeKind;

use crate::kit::Tone;

/// The word for an outcome and its tone. Waiting on the user is a warning, a failure or a
/// conflict is danger, an outcome that changed nothing is neutral.
pub fn outcome_style(kind: OutcomeKind) -> (&'static str, Tone) {
    match kind {
        OutcomeKind::Accepted => ("Accepted", Tone::Success),
        OutcomeKind::Undone => ("Undone", Tone::Neutral),
        OutcomeKind::AwaitingReview => ("In review", Tone::Warning),
        OutcomeKind::Conflict => ("Conflict", Tone::Danger),
        OutcomeKind::Failed => ("Failed", Tone::Danger),
        OutcomeKind::Cancelled => ("Stopped", Tone::Neutral),
        OutcomeKind::Interrupted => ("Interrupted", Tone::Warning),
        OutcomeKind::Discarded => ("Discarded", Tone::Neutral),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_outcome_has_one_word_and_tone() {
        assert_eq!(
            outcome_style(OutcomeKind::Accepted),
            ("Accepted", Tone::Success)
        );
        assert_eq!(outcome_style(OutcomeKind::Failed), ("Failed", Tone::Danger));
        assert_eq!(
            outcome_style(OutcomeKind::Conflict),
            ("Conflict", Tone::Danger)
        );
        assert_eq!(
            outcome_style(OutcomeKind::AwaitingReview),
            ("In review", Tone::Warning)
        );
        assert_eq!(
            outcome_style(OutcomeKind::Cancelled),
            ("Stopped", Tone::Neutral)
        );
    }
}
