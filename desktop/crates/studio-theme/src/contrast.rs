//! WCAG contrast helpers, used by tests.

use gpui::Rgba;

fn linear(channel: f32) -> f32 {
    if channel <= 0.03928 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

pub fn luminance(color: Rgba) -> f32 {
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

/// Composites `foreground` (with its alpha) over the opaque `background`.
pub fn composite(foreground: Rgba, background: Rgba) -> Rgba {
    let a = foreground.a;
    Rgba {
        r: foreground.r * a + background.r * (1. - a),
        g: foreground.g * a + background.g * (1. - a),
        b: foreground.b * a + background.b * (1. - a),
        a: 1.,
    }
}

/// Contrast ratio of `foreground` painted over `background`.
pub fn ratio(foreground: Rgba, background: Rgba) -> f32 {
    let (a, b) = (
        luminance(composite(foreground, background)),
        luminance(background),
    );
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// CIELAB lightness L* (0 to 100) of `foreground` painted over `background`.
pub fn lightness(foreground: Rgba, background: Rgba) -> f32 {
    let y = luminance(composite(foreground, background));
    if y > 216. / 24389. {
        116. * y.cbrt() - 16.
    } else {
        y * 24389. / 27.
    }
}
