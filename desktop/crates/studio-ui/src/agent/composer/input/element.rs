//! The element that measures, paints and wires the input method for the text.

use super::*;

/// Paints the shaped lines, the selection and the caret, and reports the
/// measured height so the input grows with its content up to `max_content_height`.
pub struct ComposerTextElement {
    pub(super) input: Entity<ComposerInput>,
    pub(super) max_content_height: f32,
}

pub struct ComposerTextPrepaint {
    cursor: Option<PaintQuad>,
    selection_quads: Vec<PaintQuad>,
}

impl IntoElement for ComposerTextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ComposerTextElement {
    type RequestLayoutState = ();
    type PrepaintState = ComposerTextPrepaint;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        let input = self.input.clone();
        let text_style = window.text_style();
        let max_content = self.max_content_height;
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, cx| {
                let width = known.width.unwrap_or(match available.width {
                    gpui::AvailableSpace::Definite(width) => width,
                    _ => px(FALLBACK_WIDTH),
                });
                let content_height =
                    input.update(cx, |input, _| input.layout_text(width, &text_style, window));
                size(width, px(content_height.min(max_content)))
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        self.input.update(cx, |input, cx| {
            // Intrinsic measurement may try several widths in one layout. Only
            // publish the resolved geometry, once: notifying for each
            // provisional width starts an endless measure and notify loop.
            input.layout_text(bounds.size.width, &text_style, window);
            let layout = (bounds.size.width, input.content_height);
            let layout_changed = input.last_notified_layout != Some(layout);
            input.last_notified_layout = Some(layout);
            let scrolled = input.clamp_scroll(f32::from(bounds.size.height));
            input.last_bounds = Some(bounds);
            if scrolled || layout_changed {
                cx.emit(ComposerInputEvent::ViewportChanged);
            }
        });
        let input = self.input.read(cx);
        let origin = point(bounds.left(), bounds.top() - px(input.scroll_top));
        let caret_color: gpui::Hsla = input.theme.colors.accent.into();
        let selection_color: gpui::Hsla = input.theme.colors.selection.into();
        let line_height = input.line_height;

        let mut selection_quads = Vec::new();
        let mut cursor = None;
        if input.selected_range.is_empty() || input.display_is_placeholder {
            let caret_at = input.cursor_point().unwrap_or_default();
            cursor = Some(fill(
                Bounds::new(
                    point(origin.x + caret_at.x, origin.y + caret_at.y),
                    size(px(CARET_WIDTH), line_height),
                ),
                caret_color,
            ));
        } else if let (Some(start), Some(end)) = (
            input.point_for_index(input.selected_range.start),
            input.point_for_index_with_affinity(input.selected_range.end, CaretAffinity::Upstream),
        ) {
            if start.y == end.y {
                selection_quads.push(fill(
                    Bounds::from_corners(
                        point(origin.x + start.x, origin.y + start.y),
                        point(origin.x + end.x, origin.y + start.y + line_height),
                    ),
                    selection_color,
                ));
            } else {
                // First visual row, full middle rows, last visual row.
                selection_quads.push(fill(
                    Bounds::from_corners(
                        point(origin.x + start.x, origin.y + start.y),
                        point(bounds.right(), origin.y + start.y + line_height),
                    ),
                    selection_color,
                ));
                if end.y > start.y + line_height {
                    selection_quads.push(fill(
                        Bounds::from_corners(
                            point(origin.x, origin.y + start.y + line_height),
                            point(bounds.right(), origin.y + end.y),
                        ),
                        selection_color,
                    ));
                }
                selection_quads.push(fill(
                    Bounds::from_corners(
                        point(origin.x, origin.y + end.y),
                        point(origin.x + end.x, origin.y + end.y + line_height),
                    ),
                    selection_color,
                ));
            }
        }
        ComposerTextPrepaint {
            cursor,
            selection_quads,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let input = self.input.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                input.update(cx, |input, cx| input.on_mouse_move(event, cx));
            }
        });

        // WrappedLine is not Clone: take the shaped lines out of the entity for
        // painting, then put them back for mouse mapping.
        let (lines, line_height, scroll) = self.input.update(cx, |input, _| {
            (
                std::mem::take(&mut input.last_lines),
                input.line_height,
                input.scroll_top,
            )
        });

        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in prepaint.selection_quads.drain(..) {
                window.paint_quad(quad);
            }
            let mut y = bounds.top() - px(scroll);
            for line in &lines {
                let height = line.size(line_height).height;
                let _ = line.paint(
                    point(bounds.left(), y),
                    line_height,
                    gpui::TextAlign::Left,
                    Some(bounds),
                    window,
                    cx,
                );
                y += height;
            }
            // The caret shows only when this input is focused in an active
            // window, and only in the "on" blink phase.
            if self
                .input
                .update(cx, |input, cx| input.caret_shown(window, cx))
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        self.input.update(cx, |input, _| {
            input.last_lines = lines;
        });
    }
}
