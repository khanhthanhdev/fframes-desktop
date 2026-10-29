//! Cap's minimap: a bar the length of the video with a chip for the visible part. A
//! press outside the chip centres the view there, the chip pans, its edges zoom. It is
//! hidden and takes no clicks while the whole video is visible.
use gpui::{Hsla, ParentElement, Styled, div, prelude::FluentBuilder, px, relative};
use studio_engine::{TimelineModel, TimelineTransform};
use studio_theme::{METRICS, Theme, TrackKind, track_color};

/// Whether part of the video is outside the view, so the minimap shows.
pub(super) fn zoomed_in(transform: &TimelineTransform, model: &TimelineModel) -> bool {
    model.duration() - transform.zoom > 0.01
}

/// The chip's left edge and width inside a bar `bar_width` wide.
pub(super) fn minimap_chip(
    transform: &TimelineTransform,
    total: f64,
    bar_width: f32,
) -> (f32, f32) {
    let total = total.max(0.001);
    let bar_width = bar_width.max(1.);
    let chip_width = ((transform.zoom * bar_width as f64 / total) as f32)
        .clamp(METRICS.layout.minimap_min_chip_w.min(bar_width), bar_width);
    let max_position = (total - transform.zoom).max(0.001);
    let chip_left = ((transform.position / max_position).clamp(0., 1.) as f32)
        * (bar_width - chip_width).max(0.);
    (chip_left, chip_width)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MinimapDragKind {
    Move,
    Left,
    Right,
}

#[derive(Debug, Clone)]
pub(super) struct MinimapDrag {
    pub kind: MinimapDragKind,
    start_x: f32,
    start: TimelineTransform,
    seconds_per_pixel: f64,
    move_scale: f64,
}

impl MinimapDrag {
    /// A press at window `x` on a bar at `bar_left`, `bar_width` wide. A press outside
    /// the chip centres the view on that time first. `None` while nothing is hidden.
    pub fn begin(
        x: f32,
        bar_left: f32,
        bar_width: f32,
        transform: &mut TimelineTransform,
        model: &TimelineModel,
    ) -> Option<Self> {
        let total = model.duration();
        if !x.is_finite()
            || !bar_left.is_finite()
            || !bar_width.is_finite()
            || bar_width <= 0.
            || !zoomed_in(transform, model)
        {
            return None;
        }
        let (chip_left, chip_width) = minimap_chip(transform, total, bar_width);
        let offset = x - bar_left;
        // The grab width of the chip's resize edges.
        let handle = METRICS.layout.minimap_handle_w;
        let kind = if offset < chip_left || offset > chip_left + chip_width {
            transform.set_position(
                offset as f64 / bar_width as f64 * total - transform.zoom / 2.,
                model,
            );
            MinimapDragKind::Move
        } else if offset < chip_left + handle {
            MinimapDragKind::Left
        } else if offset > chip_left + chip_width - handle {
            MinimapDragKind::Right
        } else {
            MinimapDragKind::Move
        };
        let (_, chip_width) = minimap_chip(transform, total, bar_width);
        Some(Self {
            kind,
            start_x: x,
            start: transform.clone(),
            seconds_per_pixel: total / bar_width as f64,
            move_scale: (total - transform.zoom) / (bar_width - chip_width).max(1.) as f64,
        })
    }

    /// The view for the pointer at window `x`: panned, or zoomed about the opposite edge.
    pub fn update(&self, x: f32, model: &TimelineModel) -> TimelineTransform {
        let mut transform = self.start.clone();
        if !x.is_finite() {
            return transform;
        }
        let delta = (x - self.start_x) as f64;
        match self.kind {
            MinimapDragKind::Move => {
                transform.set_position(self.start.position + delta * self.move_scale, model);
            }
            MinimapDragKind::Left => transform.update_zoom(
                self.start.zoom - delta * self.seconds_per_pixel,
                self.start.position + self.start.zoom,
                model,
            ),
            MinimapDragKind::Right => transform.update_zoom(
                self.start.zoom + delta * self.seconds_per_pixel,
                self.start.position,
                model,
            ),
        }
        transform
    }
}

/// The bar, `bar_width` wide, with the clip boundaries (seconds) marked on it.
pub(super) fn render_minimap(
    theme: &Theme,
    transform: &TimelineTransform,
    model: &TimelineModel,
    boundaries: impl IntoIterator<Item = f64>,
    bar_width: f32,
) -> gpui::Div {
    let total = model.duration().max(0.001);
    let (chip_left, chip_width) = minimap_chip(transform, total, bar_width);
    let chip = Hsla::from(theme.colors.text_3);
    let layout = METRICS.layout;
    let (radius, muted) = (px(layout.minimap_radius), METRICS.opacity.muted_media);
    let mut bar = div()
        .relative()
        .w_full()
        .h(px(layout.minimap_bar_h))
        .overflow_hidden()
        .rounded(radius)
        .bg(theme.colors.ctl_active);
    for offset in boundaries {
        if offset <= 0. || offset >= total {
            continue;
        }
        bar = bar.child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(relative((offset / total) as f32))
                .w(px(METRICS.size.divider))
                .bg(Hsla::from(track_color(TrackKind::Clip)).opacity(muted)),
        );
    }
    div()
        .w(px(bar_width))
        .h(px(layout.minimap_h))
        .flex()
        .items_center()
        .when(!zoomed_in(transform, model), |minimap| minimap.invisible())
        .child(
            bar.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(chip_left))
                    .w(px(chip_width))
                    .rounded(radius)
                    .bg(chip.opacity(muted)),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::model::tests::report;

    /// 10 s at 10 fps, 1000 px wide, showing `zoom` seconds from `position`.
    fn view(zoom: f64, position: f64) -> (TimelineModel, TimelineTransform) {
        let model = TimelineModel::new(report(10, &[("intro", 0, 100)])).unwrap();
        let mut transform = TimelineTransform::default();
        transform.resize(1_000., &model);
        transform.update_zoom(zoom, 0., &model);
        transform.set_position(position, &model);
        (model, transform)
    }

    #[test]
    fn the_chip_is_the_visible_share_of_the_bar() {
        let (model, fit) = view(10., 0.);
        assert_eq!(minimap_chip(&fit, model.duration(), 78.), (0., 78.));
        assert!(!zoomed_in(&fit, &model));
        let (model, half) = view(5., 5.);
        assert_eq!(minimap_chip(&half, model.duration(), 78.), (39., 39.));
        // Never narrower than the minimum chip.
        let (model, close) = view(0.1, 0.);
        assert_eq!(
            minimap_chip(&close, model.duration(), 78.).1,
            METRICS.layout.minimap_min_chip_w
        );
    }

    #[test]
    fn a_press_outside_the_chip_centres_the_view_there() {
        let (model, mut fit) = view(10., 0.);
        assert!(MinimapDrag::begin(139., 100., 78., &mut fit, &model).is_none());

        let (model, mut transform) = view(2., 0.);
        // The bar's middle is 5 s; the 2 s view is centred on it.
        let drag = MinimapDrag::begin(139., 100., 78., &mut transform, &model).unwrap();
        assert_eq!(drag.kind, MinimapDragKind::Move);
        assert_eq!(transform.position, 4.);
        // Dragging on pans by the bar's scale and stops at the end.
        let panned = drag.update(149., &model);
        assert!(panned.position > 4.);
        assert_eq!(drag.update(10_000., &model).position, 8.);
        assert_eq!(drag.update(f32::NAN, &model), transform);
    }

    #[test]
    fn the_chip_edges_zoom_about_the_opposite_edge() {
        let (model, mut transform) = view(5., 5.);
        // Chip [39, 78): its left edge.
        let left = MinimapDrag::begin(141., 100., 78., &mut transform, &model).unwrap();
        assert_eq!(left.kind, MinimapDragKind::Left);
        let wider = left.update(141. - 7.8, &model);
        assert!((wider.zoom - 6.).abs() < 1e-5 && (wider.position - 4.).abs() < 1e-5);

        let (model, mut transform) = view(5., 0.);
        // Chip [0, 39): its right edge keeps the left edge where it is.
        let right = MinimapDrag::begin(136., 100., 78., &mut transform, &model).unwrap();
        assert_eq!(right.kind, MinimapDragKind::Right);
        let narrower = right.update(136. - 7.8, &model);
        assert!((narrower.zoom - 4.).abs() < 1e-5 && narrower.position == 0.);
    }
}
