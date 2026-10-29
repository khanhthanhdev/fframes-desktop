//! Stick-to-bottom. The fork's `ListState` in tail mode follows new rows until the reader
//! scrolls up by any amount, and follows again only once they are back at the very end.
//! New content re-pins the list as well while the reader is within
//! [`STICK_THRESHOLD_PX`] of the end, so a small nudge does not stop streaming text from
//! staying in view.

use gpui::ListState;

/// Rows measured and painted beyond the viewport, above and below.
pub const OVERDRAW_PX: f32 = 320.;
/// How far above the end the reader is before the jump-to-latest pill shows.
pub const JUMP_DISTANCE_PX: f32 = 320.;
/// How far above the end the reader may be for new content to pin the list again.
pub const STICK_THRESHOLD_PX: f32 = 70.;

/// How far the viewport's bottom is above the end of the measured content, in pixels.
pub fn distance_from_bottom(list: &ListState) -> f32 {
    let end = list.max_offset_for_scrollbar().y;
    let top = -list.scroll_px_offset_for_scrollbar().y;
    f32::from(end - top).max(0.)
}

/// Whether new content pins the list to its end: it already follows, or the reader is
/// within [`STICK_THRESHOLD_PX`] of it.
pub fn sticks(following: bool, distance_from_bottom: f32) -> bool {
    following || distance_from_bottom <= STICK_THRESHOLD_PX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_content_pins_the_list_unless_the_reader_scrolled_up_past_the_threshold() {
        assert!(sticks(true, 500.));
        assert!(sticks(false, 0.));
        assert!(sticks(false, STICK_THRESHOLD_PX));
        assert!(!sticks(false, STICK_THRESHOLD_PX + 1.));
    }
}
