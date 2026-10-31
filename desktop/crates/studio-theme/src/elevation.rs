//! Box shadows per appearance.

use gpui::{BoxShadow, hsla, point, px};
use smallvec::{SmallVec, smallvec};

use crate::Appearance;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shadow {
    Card,
    Pop,
    Preview,
    RaisedPill,
    Thumb,
}

fn black(alpha: f32, y: f32, blur: f32, spread: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset: false,
    }
}

pub fn shadow_for(appearance: Appearance, kind: Shadow) -> SmallVec<[BoxShadow; 2]> {
    let dark = appearance == Appearance::Dark;
    let pick = |light: f32, dark_value: f32| if dark { dark_value } else { light };
    match kind {
        Shadow::Card => SmallVec::new(),
        Shadow::Pop => smallvec![black(pick(0.15, 0.30), 4., 12., -2.)],
        Shadow::Preview => smallvec![black(0.35, 12., 32., -8.)],
        // The dark pill is a wash on the trough (`segment_selected`), with no lift.
        Shadow::RaisedPill if dark => SmallVec::new(),
        Shadow::RaisedPill => smallvec![black(0.12, 1., 2., 0.), black(0.06, 0., 0., 0.5)],
        Shadow::Thumb => smallvec![black(0.25, 1., 3., 0.)],
    }
}

#[cfg(test)]
mod tests {
    use gpui::Pixels;

    use super::*;

    #[test]
    fn shadows_follow_the_spec_table() {
        let px = |value: Pixels| f32::from(value);
        let card = shadow_for(Appearance::Dark, Shadow::Card);
        assert!(card.is_empty());
        let pop_light = shadow_for(Appearance::Light, Shadow::Pop);
        let pop_dark = shadow_for(Appearance::Dark, Shadow::Pop);
        assert_eq!(
            (px(pop_light[0].offset.y), px(pop_light[0].blur_radius)),
            (4., 12.)
        );
        assert_eq!(
            (px(pop_dark[0].offset.y), px(pop_dark[0].blur_radius)),
            (4., 12.)
        );
        assert_eq!(px(pop_dark[0].spread_radius), -2.);
        assert_eq!(shadow_for(Appearance::Light, Shadow::RaisedPill).len(), 2);
        assert!(shadow_for(Appearance::Dark, Shadow::RaisedPill).is_empty());
    }
}
