//! The light and dark palettes, and the timeline's track colours.

use gpui::{Rgba, rgb, rgba};

use crate::radix::{self, Radix};
use crate::{Appearance, Colors, Syntax, Theme, mix, rgba_alpha};

/// Opaque colour from `0xRRGGBB`.
pub const fn hex(value: u32) -> Rgba {
    Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.,
        g: ((value >> 8) & 0xff) as f32 / 255.,
        b: (value & 0xff) as f32 / 255.,
        a: 1.,
    }
}

/// The roles derived from a Radix step or another role, the same in both appearances'
/// rule (phase 1 table in `plans/261027-2045-studio-design-system/phase-01-tokens.md`).
struct Derived {
    present_bg: Rgba,
    on_accent: Rgba,
    focus: Rgba,
    selection: Rgba,
    success: Rgba,
    success_bg: Rgba,
    warning: Rgba,
    warning_bg: Rgba,
    danger: Rgba,
    danger_bg: Rgba,
    danger_hover: Rgba,
    danger_active: Rgba,
    info: Rgba,
    info_bg: Rgba,
    scope: Rgba,
    scrim: Rgba,
    track_clip: Rgba,
    track_overlay: Rgba,
    track_audio: Rgba,
}

fn derived(r: &Radix, accent: Rgba) -> Derived {
    Derived {
        present_bg: hex(0x000000),
        on_accent: hex(0xffffff),
        focus: accent,
        // The accent at 25 %.
        selection: rgba_alpha(accent, 0x40 as f32 / 255.),
        success: r.green_11,
        success_bg: r.green_3,
        warning: r.amber_11,
        warning_bg: r.amber_3,
        danger: r.red_11,
        danger_bg: r.red_3,
        danger_hover: mix(r.red_3, r.red_11, 0.12),
        danger_active: mix(r.red_3, r.red_11, 0.22),
        info: accent,
        // Oleafly's `bg-primary/10`: Info badges, notices and selected radio cards.
        info_bg: rgba_alpha(accent, 0.10),
        scope: r.amber_11,
        scrim: Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.55,
        },
        track_clip: track_color(TrackKind::Clip),
        track_overlay: track_color(TrackKind::Overlay),
        track_audio: track_color(TrackKind::Audio),
    }
}

impl Colors {
    pub fn light() -> Self {
        // Control fills are translucent so they show on every surface, `card_2` included.
        let (accent, ctl) = (rgb(0x2563eb), rgba(0x0000000f));
        let d = derived(&radix::LIGHT, accent);
        Self {
            window: rgb(0xffffff),
            card: rgb(0xfafafa),
            card_2: rgb(0xf1f1f1),
            sidebar: rgb(0xf7f7f7),
            popover: rgb(0xfafafa),
            stage: rgb(0xf7f7f7),
            line: rgb(0xdedede),
            line_strong: rgb(0xa1a1a1),
            text_1: rgb(0x0a0a0a),
            // A little stronger than Oleafly's #737373 to retain 4.5:1 on nested cards.
            text_2: rgb(0x6b6b6b),
            text_3: rgb(0x737373),
            ctl,
            ctl_hover: rgba(0x0000001a),
            ctl_active: rgba(0x00000024),
            accent,
            accent_2: rgb(0x1d4ed8),
            playhead: rgb(0xff3b30),
            thumb: rgb(0xffffff),
            seg_tint: 0.13,
            seg_line: 0.34,
            seg_label_base: rgb(0x000000),
            seg_label_mix: 0.58,
            card_shadow_alpha: 0.,
            pop_shadow_alpha: 0.15,
            pop_shadow_offset: 4.,
            pop_shadow_blur: 12.,
            present_bg: d.present_bg,
            on_accent: d.on_accent,
            focus: d.focus,
            selection: d.selection,
            tooltip_bg: rgb(0xfafafa),
            tooltip_text: rgb(0x0a0a0a),
            success: d.success,
            success_bg: d.success_bg,
            warning: d.warning,
            warning_bg: d.warning_bg,
            danger: d.danger,
            danger_bg: d.danger_bg,
            danger_hover: d.danger_hover,
            danger_active: d.danger_active,
            info: d.info,
            info_bg: d.info_bg,
            scope: d.scope,
            scrim: d.scrim,
            track_clip: d.track_clip,
            track_overlay: d.track_overlay,
            track_audio: d.track_audio,
            segment_selected: rgb(0xffffff),
            caption_glyph: rgb(0x12161f),
            caption_hover: rgba(0x0000000d),
            caption_pressed: rgba(0x00000008),
            caption_close: rgb(0xc42b1c),
            caption_close_pressed: rgba(0xc42b1ce6),
        }
    }

    pub fn dark() -> Self {
        let (accent, ctl) = (rgb(0x2563eb), rgba(0xffffff14));
        let d = derived(&radix::DARK, accent);
        Self {
            window: rgb(0x0a0a0a),
            card: rgb(0x171717),
            card_2: rgb(0x262626),
            sidebar: rgb(0x171717),
            popover: rgb(0x121212),
            stage: rgb(0x171717),
            line: rgba(0xffffff1a),
            line_strong: rgba(0xffffff26),
            text_1: rgb(0xfafafa),
            text_2: rgb(0xa1a1a1),
            text_3: rgb(0x8b8b8b),
            ctl,
            ctl_hover: rgba(0xffffff1f),
            ctl_active: rgba(0xffffff29),
            accent,
            accent_2: rgb(0x1d4ed8),
            playhead: rgb(0xff453a),
            thumb: rgb(0xe8e8ea),
            seg_tint: 0.20,
            seg_line: 0.42,
            seg_label_base: rgb(0xffffff),
            seg_label_mix: 0.55,
            card_shadow_alpha: 0.,
            pop_shadow_alpha: 0.30,
            pop_shadow_offset: 4.,
            pop_shadow_blur: 12.,
            present_bg: d.present_bg,
            on_accent: d.on_accent,
            focus: d.focus,
            selection: d.selection,
            tooltip_bg: rgb(0x121212),
            tooltip_text: rgb(0xfafafa),
            success: d.success,
            success_bg: d.success_bg,
            warning: d.warning,
            warning_bg: d.warning_bg,
            danger: d.danger,
            danger_bg: d.danger_bg,
            danger_hover: d.danger_hover,
            danger_active: d.danger_active,
            info: d.info,
            info_bg: d.info_bg,
            scope: d.scope,
            scrim: d.scrim,
            track_clip: d.track_clip,
            track_overlay: d.track_overlay,
            track_audio: d.track_audio,
            segment_selected: rgb(0x383838),
            caption_glyph: rgb(0xffffff),
            caption_hover: rgba(0xffffff0d),
            caption_pressed: rgba(0xe9e9e908),
            caption_close: rgb(0xc42b1c),
            caption_close_pressed: rgba(0xc42b1ce6),
        }
    }
}

impl Syntax {
    /// The code colours for one appearance's Radix steps; the mapping is the one
    /// `studio-ui`'s `agent/syntax::token_color` paints with.
    fn from_radix(r: &Radix) -> Self {
        Self {
            comment: r.gray_10,
            keyword: r.red_11,
            string: r.green_11,
            number: r.amber_11,
            type_name: r.blue_11,
            function: r.blue_10,
            variable: r.gray_12,
            punctuation: r.gray_11,
            invalid: r.red_10,
            added: r.green_11,
            removed: r.red_11,
        }
    }

    pub fn light() -> Self {
        Self::from_radix(&radix::LIGHT)
    }

    pub fn dark() -> Self {
        Self::from_radix(&radix::DARK)
    }
}

impl Theme {
    pub fn light() -> Self {
        let colors = Colors::light();
        Self::from_parts(Appearance::Light, colors, Syntax::light(), &radix::LIGHT)
    }

    pub fn dark() -> Self {
        let colors = Colors::dark();
        Self::from_parts(Appearance::Dark, colors, Syntax::dark(), &radix::DARK)
    }

    fn from_parts(appearance: Appearance, colors: Colors, syntax: Syntax, r: &Radix) -> Self {
        Self {
            appearance,
            colors,
            syntax,

            gray_1: r.gray_1,
            gray_2: r.gray_2,
            gray_3: r.gray_3,
            gray_4: r.gray_4,
            gray_5: r.gray_5,
            gray_6: r.gray_6,
            gray_7: r.gray_7,
            gray_8: r.gray_8,
            gray_9: r.gray_9,
            gray_10: r.gray_10,
            gray_11: r.gray_11,
            gray_12: r.gray_12,

            blue_3: r.blue_3,
            blue_4: r.blue_4,
            blue_5: r.blue_5,
            blue_6: r.blue_6,
            blue_8: r.blue_8,
            blue_9: r.blue_9,
            blue_10: r.blue_10,
            blue_11: r.blue_11,
            blue_12: r.blue_12,

            red_2: r.red_2,
            red_3: r.red_3,
            red_4: r.red_4,
            red_9: r.red_9,
            red_10: r.red_10,
            red_11: r.red_11,

            amber_3: r.amber_3,
            amber_6: r.amber_6,
            amber_11: r.amber_11,

            green_3: r.green_3,
            green_11: r.green_11,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Clip,
    Overlay,
    Audio,
}

/// Base colour of a track kind (Tailwind 500: blue, violet, green).
pub const fn track_color(kind: TrackKind) -> Rgba {
    match kind {
        TrackKind::Clip => hex(0x3b82f6),
        TrackKind::Overlay => hex(0x8b5cf6),
        TrackKind::Audio => hex(0x22c55e),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentColors {
    pub fill: Rgba,
    pub border: Rgba,
    pub label: Rgba,
}

/// Segment colours for an appearance. The track colour is mixed into `card`:
/// fill 13 % light / 20 % dark (a selected segment adds 10 points), border
/// 34 % / 42 %; the label mixes the track colour toward black (light) or
/// white (dark) by 58 % / 55 %. Every result is opaque.
pub fn segment_colors_for(
    appearance: Appearance,
    kind: TrackKind,
    selected: bool,
) -> SegmentColors {
    let (palette, fill_mix, border_mix, label_target, label_mix) = match appearance {
        Appearance::Light => (Colors::light(), 0.13, 0.34, hex(0x000000), 0.58),
        Appearance::Dark => (Colors::dark(), 0.20, 0.42, hex(0xffffff), 0.55),
    };
    let track = track_color(kind);
    let opaque = |color: Rgba| Rgba { a: 1., ..color };
    SegmentColors {
        fill: opaque(mix(
            palette.card,
            track,
            fill_mix + if selected { 0.10 } else { 0. },
        )),
        border: opaque(mix(palette.card, track, border_mix)),
        label: opaque(mix(track, label_target, label_mix)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0xRRGGBBAA`, rounded from the stored floats.
    fn bytes(color: Rgba) -> u32 {
        let byte = |channel: f32| (channel * 255.).round() as u32;
        byte(color.r) << 24 | byte(color.g) << 16 | byte(color.b) << 8 | byte(color.a)
    }

    fn assert_close(actual: Rgba, expected: u32, name: &str) {
        let (r, g, b) = (
            (expected >> 16) & 0xff,
            (expected >> 8) & 0xff,
            expected & 0xff,
        );
        let got = bytes(actual) >> 8;
        let delta = |a: u32, b: u32| a.abs_diff(b);
        assert!(
            delta((got >> 16) & 0xff, r) <= 1
                && delta((got >> 8) & 0xff, g) <= 1
                && delta(got & 0xff, b) <= 1,
            "{name}: got {got:06x}, want {expected:06x}"
        );
    }

    #[test]
    fn segment_colors_follow_mix_rule() {
        // Clip blue 3b82f6 mixed into card; expected bytes computed from the rule
        // (fill 13 %/20 %, +10 points selected, border 34 %/42 %, label 58 %/55 %).
        let light = segment_colors_for(Appearance::Light, TrackKind::Clip, false);
        assert_close(light.fill, 0xe1eaf9, "light fill");
        assert_close(light.border, 0xb9d1f9, "light border");
        assert_close(light.label, 0x193767, "light label");
        let light_selected = segment_colors_for(Appearance::Light, TrackKind::Clip, true);
        assert_close(light_selected.fill, 0xcedef9, "light selected fill");
        assert_eq!(light_selected.border, light.border);

        let dark = segment_colors_for(Appearance::Dark, TrackKind::Audio, false);
        assert_close(dark.fill, 0x193a25, "dark fill");
        assert_close(dark.border, 0x1c6035, "dark border");
        assert_close(dark.label, 0x9ce5b7, "dark label");
        let dark_selected = segment_colors_for(Appearance::Dark, TrackKind::Audio, true);
        assert_close(dark_selected.fill, 0x1a4b2c, "dark selected fill");
        for color in [light.fill, light.border, light.label, dark.fill] {
            assert_eq!(color.a, 1.);
        }
    }
}
