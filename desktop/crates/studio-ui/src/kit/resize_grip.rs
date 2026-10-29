//! `ResizeGrip`: the handle that drags a panel's edge (the sidebar's width, the timeline's
//! height).
//!
//! A 4×36 bar with round ends in `line`, `line_strong` while hovered or dragged, centred in a
//! 14 px wide hit area that runs the length of the edge. The caller positions the grip (it
//! fills its parent along the edge) and keeps the drag state, as `shell/layout.rs` does today:
//!
//! - `on_drag_start(position)` runs on the left-button press, with the pointer's coordinate
//!   along the grip's axis; record it with the size at that moment and set `dragging(true)`.
//! - While `dragging`, the grip follows the pointer through the whole window, past its edges
//!   where element listeners stop firing: `on_drag(position)` runs on every move.
//! - `on_drag_end` runs on release, or on a move without the button (a release outside the
//!   window never arrives); set `dragging(false)` then.
//!
//! `Axis::Horizontal` drags left and right (a vertical bar, column-resize cursor);
//! `Axis::Vertical` drags up and down (a horizontal bar, row-resize cursor).

use std::rc::Rc;

use gpui::{
    App, Axis, CursorStyle, DispatchPhase, ElementId, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, RenderOnce,
    SharedString, Styled, Window, canvas, div, prelude::FluentBuilder, px,
};

use studio_theme::{METRICS, Theme};

type PositionHandler = Rc<dyn Fn(Pixels, &mut Window, &mut App)>;
type EndHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The pointer's coordinate along `axis`.
pub(crate) fn along(axis: Axis, position: Point<Pixels>) -> Pixels {
    match axis {
        Axis::Horizontal => position.x,
        Axis::Vertical => position.y,
    }
}

#[derive(IntoElement)]
pub struct ResizeGrip {
    id: ElementId,
    theme: Theme,
    axis: Axis,
    dragging: bool,
    on_drag_start: Option<PositionHandler>,
    on_drag: Option<PositionHandler>,
    on_drag_end: Option<EndHandler>,
}

impl ResizeGrip {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, axis: Axis) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            axis,
            dragging: false,
            on_drag_start: None,
            on_drag: None,
            on_drag_end: None,
        }
    }

    /// Whether a drag that started on this grip is in progress.
    pub fn dragging(mut self, dragging: bool) -> Self {
        self.dragging = dragging;
        self
    }

    pub fn on_drag_start(
        mut self,
        handler: impl Fn(Pixels, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_drag_start = Some(Rc::new(handler));
        self
    }

    pub fn on_drag(mut self, handler: impl Fn(Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.on_drag = Some(Rc::new(handler));
        self
    }

    pub fn on_drag_end(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_drag_end = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ResizeGrip {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ResizeGrip {
            id,
            theme,
            axis,
            dragging,
            on_drag_start,
            on_drag,
            on_drag_end,
        } = self;
        let colors = theme.colors;
        let size = METRICS.size;
        let group = SharedString::from(format!("resize-grip-{id}"));
        let (bar_w, bar_h) = match axis {
            Axis::Horizontal => (size.grip_w, size.grip_h),
            Axis::Vertical => (size.grip_h, size.grip_w),
        };

        let tracker = dragging.then(|| {
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let moved = on_drag.clone();
                    let ended = on_drag_end.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture {
                            return;
                        }
                        if event.pressed_button != Some(MouseButton::Left) {
                            if let Some(ended) = &ended {
                                ended(window, cx);
                            }
                            return;
                        }
                        if let Some(moved) = &moved {
                            moved(along(axis, event.position), window, cx);
                        }
                    });
                    let released = on_drag_end.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture
                            && event.button == MouseButton::Left
                            && let Some(released) = &released
                        {
                            released(window, cx);
                        }
                    });
                },
            )
            .absolute()
            .size_0()
        });

        div()
            .id(id)
            .group(group.clone())
            .flex()
            .items_center()
            .justify_center()
            .map(|this| match axis {
                Axis::Horizontal => this
                    .w(px(size.grip_hit))
                    .h_full()
                    .cursor(CursorStyle::ResizeColumn),
                Axis::Vertical => this
                    .h(px(size.grip_hit))
                    .w_full()
                    .cursor(CursorStyle::ResizeRow),
            })
            .when_some(on_drag_start, |this, handler| {
                this.on_mouse_down(
                    MouseButton::Left,
                    move |event: &MouseDownEvent, window, cx| {
                        handler(along(axis, event.position), window, cx);
                        cx.stop_propagation();
                    },
                )
            })
            .child(
                div()
                    .w(px(bar_w))
                    .h(px(bar_h))
                    .rounded(px(size.grip_w / 2.))
                    .bg(if dragging {
                        colors.line_strong
                    } else {
                        colors.line
                    })
                    .group_hover(group, move |style| style.bg(colors.line_strong)),
            )
            .children(tracker)
    }
}

#[cfg(test)]
mod tests {
    use gpui::point;

    use super::*;

    #[test]
    fn a_grip_reads_the_coordinate_along_its_axis() {
        let position = point(px(10.), px(20.));
        assert_eq!(along(Axis::Horizontal, position), px(10.));
        assert_eq!(along(Axis::Vertical, position), px(20.));
    }
}
