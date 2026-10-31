//! The "Review changes" buttons of the conversation panel. They only ask the shell to open the
//! review window; what happens there is up to [`changes::ReviewHost`](super::changes::ReviewHost).

use super::changes::ReviewTarget;
use super::*;
use crate::kit::ButtonSize;

impl ConversationPanel {
    /// The button on the change summary of the candidate that waits for review (the card
    /// makes it compact). Older change summaries in the transcript have none: their candidate
    /// is gone.
    pub(super) fn candidate_review_button(
        &mut self,
        card: &ChangeCard,
        cx: &mut Context<Self>,
    ) -> Option<Button> {
        let current = self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.task.as_ref().is_some_and(|task| {
                task.phase == TaskPhase::AwaitingReview
                    && task.review.is_some()
                    && task.changes.as_ref() == Some(card)
            })
        });
        current.then(|| {
            self.action_button(
                "agent-review-candidate",
                "Review changes",
                true,
                Emphasis::Neutral,
                cx,
                |this, _, cx| this.request_review(ReviewTarget::Candidate, cx),
            )
        })
    }

    /// The button in the pinned banner, which stays in view while the candidate awaits review
    /// even when its change card has scrolled out of the transcript.
    pub(super) fn banner_review_button(&mut self, cx: &mut Context<Self>) -> Button {
        self.action_button(
            "agent-review-banner",
            "Review changes",
            true,
            Emphasis::Neutral,
            cx,
            |this, _, cx| this.request_review(ReviewTarget::Candidate, cx),
        )
    }

    /// The button under an accepted edit in the history: compact (24 px), like the one on a
    /// change card, so it sits inside the history row.
    pub(super) fn revision_review_button(
        &mut self,
        revision: &str,
        cx: &mut Context<Self>,
    ) -> Button {
        let target = ReviewTarget::Revision(revision.to_owned());
        self.action_button(
            format!("agent-review-{revision}"),
            "Review changes",
            true,
            Emphasis::Neutral,
            cx,
            move |this, _, cx| this.request_review(target.clone(), cx),
        )
        .size(ButtonSize::Sm)
    }
}
