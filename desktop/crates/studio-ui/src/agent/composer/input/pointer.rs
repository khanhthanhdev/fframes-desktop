//! Mouse selection, drag autoscroll and the wheel.

use super::*;

impl ComposerInput {
    pub(super) fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let intent = press_intent(event.click_count, event.modifiers.shift);
        self.is_selecting = true;
        self.drag_position = Some(event.position);
        self.drag_unit = None;
        self.drag_generation = self.drag_generation.wrapping_add(1);
        self.drag_autoscroll_active = false;
        match intent {
            PressIntent::Word | PressIntent::Line => {
                let index = self.index_for_mouse_position(event.position);
                let range = self.selection_unit(intent, index);
                self.move_to(range.start, cx);
                self.select_to(range.end, cx);
                self.drag_unit = Some((intent, range));
            }
            PressIntent::ExtendSelection => {
                let (index, affinity) = self.caret_for_mouse_position(event.position);
                self.select_to(index, cx);
                self.caret_affinity = affinity;
            }
            PressIntent::PlaceCaret => {
                let (index, affinity) = self.caret_for_mouse_position(event.position);
                self.move_to(index, cx);
                self.caret_affinity = affinity;
            }
        }
    }

    pub(super) fn selection_unit(&self, intent: PressIntent, index: usize) -> Range<usize> {
        if intent == PressIntent::Line {
            let mut range = self.line_range_at(index);
            if range.end < self.content.len() {
                range.end += 1;
            }
            range
        } else {
            word_range(&self.content, index)
        }
    }

    pub(super) fn drag_select_to(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some((intent, anchor)) = self.drag_unit.clone() {
            let range = self.selection_unit(intent, index);
            if range.start < anchor.start {
                self.move_to(anchor.end, cx);
                self.select_to(range.start, cx);
            } else {
                self.move_to(anchor.start, cx);
                self.select_to(range.end.max(anchor.end), cx);
            }
        } else {
            self.select_to(index, cx);
        }
    }

    pub(super) fn drag_select_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (index, affinity) = self.caret_for_mouse_position(position);
        self.drag_select_to(index, cx);
        if self.drag_unit.is_none() {
            self.caret_affinity = affinity;
        }
    }

    pub(super) fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
        self.drag_position = None;
        self.drag_generation = self.drag_generation.wrapping_add(1);
        self.drag_autoscroll_active = false;
    }

    pub(super) fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.drag_position = Some(event.position);
            let position = self.drag_selection_position(event.position);
            self.drag_select_at(position, cx);
            if self.drag_scroll_delta(event.position) != 0.0 && !self.drag_autoscroll_active {
                self.start_drag_autoscroll(cx);
            }
        }
    }

    pub(super) fn start_drag_autoscroll(&mut self, cx: &mut Context<Self>) {
        self.drag_autoscroll_active = true;
        let generation = self.drag_generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(DRAG_SCROLL_FRAME_MS))
                    .await;
                let keep_running = this
                    .update(cx, |input, cx| input.drag_autoscroll_tick(generation, cx))
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn drag_selection_position(&self, position: Point<Pixels>) -> Point<Pixels> {
        let Some(bounds) = self.last_bounds else {
            return position;
        };
        point(
            position
                .x
                .clamp(bounds.left(), bounds.right() - px(HIT_INSET)),
            position
                .y
                .clamp(bounds.top(), bounds.bottom() - px(HIT_INSET)),
        )
    }

    pub(super) fn drag_scroll_delta(&self, position: Point<Pixels>) -> f32 {
        let Some(bounds) = self.last_bounds else {
            return 0.0;
        };
        input_drag_scroll_delta(
            f32::from(position.y),
            f32::from(bounds.top()),
            f32::from(bounds.bottom()),
            f32::from(self.line_height),
        )
    }

    pub(super) fn drag_autoscroll_tick(&mut self, generation: u64, cx: &mut Context<Self>) -> bool {
        if !self.is_selecting || self.drag_generation != generation {
            return false;
        }
        let (Some(position), Some(bounds)) = (self.drag_position, self.last_bounds) else {
            self.drag_autoscroll_active = false;
            return false;
        };
        let delta = self.drag_scroll_delta(position);
        if delta == 0.0 {
            self.drag_autoscroll_active = false;
            return false;
        }
        let next = (self.scroll_top + delta).clamp(
            0.0,
            input_max_scroll(self.content_height, f32::from(bounds.size.height)),
        );
        if next == self.scroll_top {
            self.drag_autoscroll_active = false;
            return false;
        }
        self.scroll_top = next;
        let edge_position = self.drag_selection_position(position);
        self.drag_select_at(edge_position, cx);
        // Selection motion normally resumes caret following. During an edge
        // drag the autoscroll loop owns the viewport instead.
        self.follow_cursor = false;
        true
    }

    pub(super) fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.last_bounds else {
            return;
        };
        let viewport_height = f32::from(bounds.size.height);
        let delta_y = f32::from(event.delta.pixel_delta(self.line_height).y);
        let next = input_scroll_offset(
            self.scroll_top,
            delta_y,
            self.content_height,
            viewport_height,
        );
        if next == self.scroll_top {
            // Overscroll guard: when the input itself is scrollable (content
            // taller than the viewport), swallow the wheel event even at the
            // scroll boundary so it never chains into the outer transcript
            // list (the native equivalent of `overscroll-behavior: contain`).
            if delta_y != 0.0 && input_max_scroll(self.content_height, viewport_height) > 0.0 {
                cx.stop_propagation();
            }
            return;
        }
        self.scroll_top = next;
        self.follow_cursor = false;
        cx.stop_propagation();
        cx.emit(ComposerInputEvent::ViewportChanged);
        cx.notify();
    }
}
