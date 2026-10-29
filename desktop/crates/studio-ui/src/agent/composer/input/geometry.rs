//! Where text sits: caret points and hit testing against the shaped lines.
//!
//! Points are content-local: x runs right from the input's left edge and y runs
//! down from the top of the content, ignoring the scroll offset.

use super::*;

impl ComposerInput {
    /// The point of a byte index, with the caret on the downstream side of a
    /// soft wrap.
    pub(super) fn point_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        self.point_for_index_with_affinity(index, CaretAffinity::Downstream)
    }

    pub(super) fn cursor_point(&self) -> Option<Point<Pixels>> {
        self.point_for_index_with_affinity(self.cursor_offset(), self.caret_affinity)
    }

    /// The point of a byte index on the given side of a soft wrap.
    ///
    /// A wrap boundary is the end of one visual row and the start of the next;
    /// downstream puts the caret at the start of the lower row.
    pub(super) fn point_for_index_with_affinity(
        &self,
        index: usize,
        affinity: CaretAffinity,
    ) -> Option<Point<Pixels>> {
        for (line_ix, line) in self.last_lines.iter().enumerate() {
            let line_start = *self.line_starts.get(line_ix)?;
            let line_len = line.len();
            if index < line_start {
                continue;
            }
            if index <= line_start + line_len {
                let relative_index = index - line_start;
                let local = if affinity == CaretAffinity::Downstream {
                    line.wrap_boundaries()
                        .iter()
                        .position(|boundary| {
                            line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index
                                == relative_index
                        })
                        .map(|row| point(Pixels::ZERO, self.line_height * (row + 1)))
                        .or_else(|| line.position_for_index(relative_index, self.line_height))?
                } else {
                    line.position_for_index(relative_index, self.line_height)?
                };
                let y_offset: f32 = self
                    .last_lines
                    .iter()
                    .take(line_ix)
                    .map(|l| f32::from(l.size(self.line_height).height))
                    .sum();
                return Some(point(local.x, local.y + px(y_offset)));
            }
        }
        None
    }

    /// The byte index and visual side closest to a content-local point.
    pub(super) fn caret_for_point(&self, position: Point<Pixels>) -> (usize, CaretAffinity) {
        if self.display_is_placeholder {
            return (0, CaretAffinity::Downstream);
        }
        let mut y = f32::from(position.y);
        if y < 0.0 {
            return (0, CaretAffinity::Downstream);
        }
        for (line_ix, line) in self.last_lines.iter().enumerate() {
            let height = f32::from(line.size(self.line_height).height);
            let line_start = self.line_starts.get(line_ix).copied().unwrap_or(0);
            if y < height || line_ix + 1 == self.last_lines.len() {
                let local = point(position.x, px(y.min(height - 1.0).max(0.0)));
                let ix = line
                    .closest_index_for_position(local, self.line_height)
                    .unwrap_or_else(|ix| ix);
                let affinity = line
                    .wrap_boundaries()
                    .iter()
                    .position(|boundary| {
                        line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index == ix
                    })
                    .filter(|row| local.y < self.line_height * (row + 1))
                    .map_or(CaretAffinity::Downstream, |_| CaretAffinity::Upstream);
                let mut raw = (line_start + ix).min(self.content.len());
                // CRLF is one newline grapheme. A click past the text on its
                // row must land before the CR, never between its two bytes.
                if raw > 0
                    && self.content.as_bytes().get(raw) == Some(&b'\n')
                    && self.content.as_bytes()[raw - 1] == b'\r'
                {
                    raw -= 1;
                }
                return (raw, affinity);
            }
            y -= height;
        }
        (self.content.len(), CaretAffinity::Upstream)
    }

    pub(super) fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        self.caret_for_mouse_position(position).0
    }

    pub(super) fn caret_for_mouse_position(
        &self,
        position: Point<Pixels>,
    ) -> (usize, CaretAffinity) {
        let Some(bounds) = self.last_bounds else {
            return (0, CaretAffinity::Downstream);
        };
        let local = point(
            position.x - bounds.left(),
            position.y - bounds.top() + px(self.scroll_top),
        );
        self.caret_for_point(local)
    }
}
