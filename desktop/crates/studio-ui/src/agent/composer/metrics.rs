//! Sizes and the pure decisions behind the input's scrolling, selection and caret.
//!
//! Nothing here touches gpui state, so every function is tested without a window.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// Input text size in pixels: the `Body` style.
pub const INPUT_TEXT_SIZE: f32 = studio_theme::TextStyle::Body.size();
/// Input line height in pixels.
pub const INPUT_LINE_HEIGHT: f32 = 20.0;
/// The most lines the text area grows to before it scrolls.
pub const INPUT_MAX_LINES: usize = 8;
/// The tallest the text area grows before it scrolls, in pixels.
pub const INPUT_MAX_HEIGHT: f32 = INPUT_MAX_LINES as f32 * INPUT_LINE_HEIGHT;
/// Caret blink half-period: about 500 ms on, 500 ms off.
pub const CARET_BLINK_MS: u64 = 500;
/// The caret's width.
pub const CARET_WIDTH: f32 = 2.0;
/// The narrowest width text is wrapped at.
pub const MIN_WRAP_WIDTH: f32 = 20.0;
/// The width text is measured at while the layout offers no definite one.
pub const FALLBACK_WIDTH: f32 = 320.0;
/// How far inside the text bounds a pointer outside them is clamped.
pub const HIT_INSET: f32 = 0.5;
/// Drag-selection autoscroll runs at a 60 fps cadence.
pub const DRAG_SCROLL_FRAME_MS: u64 = 16;

/// Edge length of an attachment thumbnail in the strip.
pub const STRIP_THUMB: f32 = 56.0;
/// Gap between strip chips.
pub const STRIP_GAP: f32 = 8.0;
/// Space above the strip.
pub const STRIP_PAD_TOP: f32 = 12.0;
/// Space left and right of the strip.
pub const STRIP_PAD_X: f32 = 12.0;

/// Caret blink phase for the time since the last keystroke or caret move.
///
/// The caret is solid through the first half-period, so typing never blinks it
/// (each keystroke restarts the phase), and alternates after that.
pub fn caret_visible(ms_since_activity: u64) -> bool {
    (ms_since_activity / CARET_BLINK_MS).is_multiple_of(2)
}

/// Content height for a wrapped-line count.
pub fn input_content_height(wrapped_lines: usize) -> f32 {
    wrapped_lines.max(1) as f32 * INPUT_LINE_HEIGHT
}

/// The farthest the content can scroll.
pub fn input_max_scroll(content_height: f32, viewport_height: f32) -> f32 {
    (content_height - viewport_height).max(0.0)
}

/// Whether content is hidden above and below the viewport, as `(top, bottom)`.
pub fn input_overflow_edges(
    content_height: f32,
    viewport_height: f32,
    scroll_top: f32,
) -> (bool, bool) {
    let max_scroll = input_max_scroll(content_height, viewport_height);
    if max_scroll <= 1.0 {
        return (false, false);
    }
    (scroll_top > 1.0, scroll_top < max_scroll - 1.0)
}

/// Applies gpui's wheel delta to a top-origin scroll offset.
///
/// A positive delta scrolls toward the start, as gpui's lists and divs do.
pub fn input_scroll_offset(
    current: f32,
    delta_y: f32,
    content_height: f32,
    viewport_height: f32,
) -> f32 {
    (current - delta_y).clamp(0.0, input_max_scroll(content_height, viewport_height))
}

/// Moves the viewport the least that shows the whole caret row.
pub fn input_scroll_offset_for_cursor(
    current: f32,
    cursor_top: f32,
    cursor_height: f32,
    content_height: f32,
    viewport_height: f32,
) -> f32 {
    let mut next = current;
    if cursor_top < next {
        next = cursor_top;
    } else if cursor_top + cursor_height > next + viewport_height {
        next = cursor_top + cursor_height - viewport_height;
    }
    next.clamp(0.0, input_max_scroll(content_height, viewport_height))
}

/// How a press selects: multi-click drags keep the initial unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressIntent {
    Word,
    Line,
    ExtendSelection,
    PlaceCaret,
}

/// The selection unit a press asks for.
pub fn press_intent(click_count: usize, shift: bool) -> PressIntent {
    if click_count >= 3 {
        PressIntent::Line
    } else if click_count == 2 {
        PressIntent::Word
    } else if shift {
        PressIntent::ExtendSelection
    } else {
        PressIntent::PlaceCaret
    }
}

/// The word around `offset`, or the empty range at it when none is there.
pub fn word_range(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    text.split_word_bound_indices()
        .find(|(at, word)| *at <= offset && offset < at + word.len())
        .map(|(at, word)| at..at + word.len())
        .unwrap_or(offset..offset)
}

/// Per-frame scroll while a selection drag sits outside the viewport.
///
/// Speed grows with distance and is capped at one text row per frame, so
/// crossing the edge never jumps.
pub fn input_drag_scroll_delta(
    pointer_y: f32,
    viewport_top: f32,
    viewport_bottom: f32,
    line_height: f32,
) -> f32 {
    let distance = if pointer_y < viewport_top {
        pointer_y - viewport_top
    } else if pointer_y > viewport_bottom {
        pointer_y - viewport_bottom
    } else {
        return 0.0;
    };
    distance.signum() * (distance.abs() * 0.2).clamp(1.0, line_height)
}

/// Height the wrapping chip strip takes for `count` chips in `inner_width`.
///
/// Zero when there are none. As many chips per row as fit between the side
/// insets, with [`STRIP_GAP`] between them.
pub fn attachment_strip_height(count: usize, inner_width: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let usable = (inner_width - 2.0 * STRIP_PAD_X).max(STRIP_THUMB);
    let per_row = (((usable + STRIP_GAP) / (STRIP_THUMB + STRIP_GAP)).floor() as usize).max(1);
    let rows = count.div_ceil(per_row);
    STRIP_PAD_TOP + rows as f32 * STRIP_THUMB + (rows - 1) as f32 * STRIP_GAP
}
