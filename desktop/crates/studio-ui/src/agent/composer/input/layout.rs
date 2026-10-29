//! Shaping the content into wrapped lines, and keeping the caret in view.

use super::*;

/// Everything that changes how the content shapes. Scrolling, selection and
/// caret blinking are not part of it, so they reuse the retained layout.
#[derive(Clone, PartialEq)]
pub(super) struct InputLayoutKey {
    width: Pixels,
    font: gpui::Font,
    font_size: Pixels,
    color: gpui::Hsla,
    placeholder_color: gpui::Hsla,
    marked_range: Option<Range<usize>>,
    placeholder: SharedString,
}

impl ComposerInput {
    /// Shapes the text at a width, stores the measured layout and returns the
    /// content height. Called from the element's measured-layout closure.
    pub(super) fn layout_text(
        &mut self,
        width: Pixels,
        style: &TextStyle,
        window: &mut Window,
    ) -> f32 {
        let font_size = style.font_size.to_pixels(window.rem_size());
        let placeholder_color: gpui::Hsla = self.theme.colors.text_3.into();
        let key = InputLayoutKey {
            width,
            font: style.font(),
            font_size,
            color: style.color,
            placeholder_color,
            marked_range: self.marked_range.clone(),
            placeholder: self.placeholder.clone(),
        };
        if !self.needs_measure && self.last_layout_key.as_ref() == Some(&key) {
            return self.content_height;
        }
        #[cfg(test)]
        {
            self.layout_rebuilds += 1;
        }
        let is_placeholder = self.content.is_empty();
        let display: SharedString = if is_placeholder {
            self.placeholder.clone()
        } else {
            SharedString::from(self.content.clone())
        };
        let color = if is_placeholder {
            placeholder_color
        } else {
            style.color
        };
        self.line_height = px(INPUT_LINE_HEIGHT);

        let run_for = |len: usize, underline: bool| TextRun {
            len,
            font: style.font(),
            color,
            background_color: None,
            underline: underline.then_some(UnderlineStyle {
                color: Some(color),
                thickness: px(studio_theme::METRICS.size.divider),
                wavy: false,
            }),
            strikethrough: None,
        };
        let marked = if is_placeholder {
            None
        } else {
            self.marked_range.clone()
        };

        let mut lines = Vec::new();
        let mut line_starts = Vec::new();
        let mut at = 0usize;
        #[cfg(test)]
        let mut all_runs = Vec::new();
        for text in display.split('\n') {
            let end = at + text.len();
            let mut cuts = vec![at, end];
            if let Some(range) = &marked {
                cuts.extend([range.start.clamp(at, end), range.end.clamp(at, end)]);
            }
            cuts.sort_unstable();
            cuts.dedup();
            let runs: Vec<TextRun> = cuts
                .windows(2)
                .map(|pair| {
                    let underline = marked
                        .as_ref()
                        .is_some_and(|range| range.start <= pair[0] && pair[1] <= range.end);
                    run_for(pair[1] - pair[0], underline)
                })
                .collect();
            #[cfg(test)]
            all_runs.extend(runs.iter().cloned());
            if let Ok(shaped) = window.text_system().shape_text(
                text.to_string().into(),
                font_size,
                &runs,
                Some(width.max(px(MIN_WRAP_WIDTH))),
                None,
            ) {
                for line in shaped {
                    line_starts.push(at);
                    lines.push(line);
                }
            }
            at = end + 1;
        }
        #[cfg(test)]
        {
            self.last_runs = all_runs;
        }
        if line_starts.is_empty() {
            line_starts.push(0);
        }
        let content_height: f32 = lines
            .iter()
            .map(|line| f32::from(line.size(self.line_height).height))
            .sum();

        self.display_is_placeholder = is_placeholder;
        self.last_layout_key = Some(key);
        self.last_lines = lines;
        self.line_starts = line_starts;
        self.content_height = content_height.max(INPUT_LINE_HEIGHT);
        self.last_width = f32::from(width);
        self.needs_measure = false;
        self.content_height
    }

    /// Keeps the caret visible when the content is taller than the element.
    /// Returns whether the scroll offset moved.
    pub(super) fn clamp_scroll(&mut self, element_height: f32) -> bool {
        let previous = self.scroll_top;
        if self.follow_cursor
            && let Some(cursor) = self.cursor_point()
        {
            self.scroll_top = input_scroll_offset_for_cursor(
                self.scroll_top,
                f32::from(cursor.y),
                f32::from(self.line_height),
                self.content_height,
                element_height,
            );
        }
        self.scroll_top = self
            .scroll_top
            .clamp(0.0, input_max_scroll(self.content_height, element_height));
        self.scroll_top != previous
    }
}
