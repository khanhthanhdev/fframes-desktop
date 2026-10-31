//! The agent panel's own sizes, after Oleafly's ACP panel (`docs/desktop/oleafly-design-system.md`,
//! "Agent panel"). They are denser than the kit's global controls on purpose: the panel's frame,
//! transcript and activity read these, and global buttons and fields keep their sizes.

use studio_theme::METRICS;

/// The header row with the panel's tabs and its icon buttons.
pub mod header {
    use super::METRICS;

    /// Oleafly's `h-9` assistant header.
    pub const H: f32 = 36.;
    pub const PAD_X: f32 = METRICS.space_2;
    /// Between the tabs and the action group.
    pub const GAP: f32 = 6.;
    /// Between the icon buttons on the right.
    pub const ACTION_GAP: f32 = 2.;
}

/// The status strip under the header: the task or the agent in one line.
pub mod status {
    use super::METRICS;

    pub const PAD_X: f32 = METRICS.space_3;
    pub const PAD_Y: f32 = 6.;
    pub const GAP: f32 = 6.;
}

/// The scrolling conversation.
pub mod transcript {
    use super::METRICS;

    /// Inset of the rows from the panel's sides, and from its top and bottom.
    pub const INSET_X: f32 = METRICS.space_4;
    pub const INSET_Y: f32 = METRICS.space_3;
    /// Space under every row.
    pub const ROW_GAP: f32 = METRICS.space_3;
}

/// The two voices: the user's bubble and the agent's framed answer, with their footer.
pub mod message {
    use super::METRICS;

    /// The widest a user bubble grows, as a share of the row.
    pub const BUBBLE_MAX_W: f32 = 0.85;
    pub const PAD_X: f32 = METRICS.space_3;
    pub const PAD_Y: f32 = METRICS.space_2;
    /// Between a bubble and its footer, and between the footer's time and Copy.
    pub const FOOTER_GAP: f32 = 2.;
    /// The footer's height, always reserved so hover and focus never move a row.
    pub const FOOTER_H: f32 = 28.;
    /// The light answer frame's hairline: `line` at 60 %.
    pub const FRAME_LINE_ALPHA: f32 = 0.6;
    /// How long Copy shows its check after copying.
    pub const COPIED_MS: u64 = 1500;
}

/// Runs of tool calls and reasoning.
pub mod activity {
    use super::METRICS;

    /// A summary line and a step row.
    pub const ROW_H: f32 = 24.;
    pub const ICON: f32 = METRICS.size.icon_sm;
    /// The chevron of a summary or a step.
    pub const DISCLOSURE: f32 = 12.;
    /// Between a glyph and its text.
    pub const GAP: f32 = METRICS.space_2;
    /// Between the chevron and the summary.
    pub const SUMMARY_GAP: f32 = 6.;
    /// The steps of an open run sit this far in from its summary.
    pub const NESTED_INSET: f32 = 10.;
    /// Open reasoning: its rule's margin, the text's inset from the rule, the tallest it grows
    /// before it scrolls, and the space above and below it.
    pub const REASONING_RULE_MARGIN: f32 = 7.;
    pub const REASONING_INSET: f32 = METRICS.space_3;
    pub const REASONING_MAX_H: f32 = 224.;
    pub const REASONING_PAD_Y: f32 = METRICS.space_1;
}

/// The new-chat, setup and connecting states in the empty transcript.
pub mod home {
    use super::METRICS;

    pub const GAP: f32 = 20.;
    pub const PAD_Y: f32 = METRICS.space_6;
    /// The text column's widest (Oleafly's `max-w-2xl`; the sidebar is narrower, so the
    /// column follows the panel).
    pub const MAX_W: f32 = 672.;
    /// Between the heading and the line under it.
    pub const TITLE_GAP: f32 = METRICS.space_2;
}
