//! Animation durations and easing.

use std::time::Duration;

/// Every duration the studio animates with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Motion {
    /// Hover and press feedback.
    pub fast: Duration,
    /// Opening and folding: collapsibles, popovers.
    pub normal: Duration,
    /// Larger moves across the window.
    pub slow: Duration,
    /// How long the pointer rests before a tooltip shows.
    pub tooltip_delay: Duration,
    /// One turn of a spinner.
    pub spin_period: Duration,
    /// Half a caret blink: on this long, then off this long.
    pub caret_blink: Duration,
    /// The bounds of the streaming text veil's fade (agent markdown).
    pub veil_min: Duration,
    pub veil_max: Duration,
}

pub const MOTION: Motion = Motion {
    fast: Duration::from_millis(150),
    normal: Duration::from_millis(200),
    slow: Duration::from_millis(320),
    tooltip_delay: Duration::from_millis(300),
    spin_period: Duration::from_millis(1100),
    caret_blink: Duration::from_millis(500),
    veil_min: Duration::from_millis(120),
    veil_max: Duration::from_millis(400),
};

/// Cubic ease-out over `t` in 0..=1 (clamped): fast start, soft landing. Everything that
/// opens or folds uses it.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}

/// Reduced-motion preference without an app context: always `false`. Use
/// [`prefers_reduced_motion`](crate::prefers_reduced_motion), which reads the user's
/// preference; this one is removed once its callers move.
pub const fn reduced_motion() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_matches_the_phase_1_table() {
        let ms = |d: Duration| d.as_millis();
        assert_eq!(
            [MOTION.fast, MOTION.normal, MOTION.slow].map(ms),
            [150, 200, 320]
        );
        assert_eq!(
            [MOTION.tooltip_delay, MOTION.spin_period, MOTION.caret_blink].map(ms),
            [300, 1100, 500]
        );
        assert_eq!([MOTION.veil_min, MOTION.veil_max].map(ms), [120, 400]);
        assert!(MOTION.fast < MOTION.normal && MOTION.normal < MOTION.slow);
    }

    #[test]
    fn ease_out_starts_fast_and_lands_at_one() {
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(1.), 1.);
        assert_eq!(ease_out(-1.), 0.);
        assert_eq!(ease_out(2.), 1.);
        assert!(ease_out(0.5) > 0.5);
        let mut last = 0.;
        for step in 1..=20 {
            let value = ease_out(step as f32 / 20.);
            assert!(value >= last);
            last = value;
        }
    }
}
