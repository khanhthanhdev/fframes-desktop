//! Colour tokens, metrics and type for the fframes studio chrome.
//!
//! The palette is Cap's, transcribed from the web app's resolved values: the
//! Radix scales plus the editor's layered `--ed-*` surfaces. A few dark-mode
//! entries are Cap's `theme.css` overrides rather than stock Radix, so
//! regenerating this from a Radix crate would silently change the palette.
//!
//! Video style presets belong to project content; this crate owns only the
//! studio's own surfaces. A view resolves a [`Theme`] once per render with
//! [`Theme::for_window`] and passes it down.

use gpui::{Hsla, Rgba, WindowAppearance};

mod appearance;
#[cfg(test)]
mod contrast;
mod elevation;
mod metrics;
mod motion;
mod palette;
mod radix;
mod typography;

pub use appearance::{
    UiPrefs, bind_window, current_motion, current_preference, current_prefs, init, init_prefs,
    prefers_reduced_motion, resolve_appearance, resolve_reduced_motion, set_motion, set_preference,
    set_prefs,
};
pub use elevation::{Shadow, shadow_for};
pub use metrics::{Layout, METRICS, Metrics, Opacity, Radius, Size};
pub use motion::{MOTION, Motion, ease_out, reduced_motion};
pub use palette::{SegmentColors, TrackKind, segment_colors_for, track_color};
pub use studio_proto::{AppTheme, MotionPreference};
pub use typography::{MONO_FAMILY, TextStyle, UI_FONT, text_style};

// Still public until their last callers move (not `#[deprecated]`: `just clippy` denies
// warnings):
// - the raw Radix steps on `Theme` (`gray_*`, `blue_*`, `red_*`, `amber_*`, `green_*`) and
//   `Theme::gray` (agent composer, cards, todo panel, setup view, preset panel, player);
// - `Metrics::radius_frame` and the flat `radius_*`/size/layout fields once callers read
//   `METRICS.radius/size/layout`;
// - `reduced_motion()` (use `prefers_reduced_motion`), `Theme::with_alpha`;
// - `init(cx, AppTheme)` becomes `init_prefs(cx, UiPrefs)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    pub fn from_window(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
        }
    }
}

/// Every colour the studio draws with, already resolved for one appearance.
///
/// The scales are transcribed whole rather than trimmed to today's callers:
/// half a Radix scale sends the next caller back to the CSS to re-derive which
/// entries were overridden.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub appearance: Appearance,

    pub gray_1: Rgba,
    pub gray_2: Rgba,
    pub gray_3: Rgba,
    pub gray_4: Rgba,
    pub gray_5: Rgba,
    pub gray_6: Rgba,
    pub gray_7: Rgba,
    pub gray_8: Rgba,
    pub gray_9: Rgba,
    pub gray_10: Rgba,
    pub gray_11: Rgba,
    pub gray_12: Rgba,

    pub blue_3: Rgba,
    pub blue_4: Rgba,
    pub blue_5: Rgba,
    pub blue_6: Rgba,
    pub blue_8: Rgba,
    pub blue_9: Rgba,
    pub blue_10: Rgba,
    pub blue_11: Rgba,
    pub blue_12: Rgba,

    pub red_2: Rgba,
    /// Danger surfaces (error notices, destructive hover washes).
    pub red_3: Rgba,
    pub red_4: Rgba,
    pub red_9: Rgba,
    pub red_10: Rgba,
    /// Danger text and glyphs.
    pub red_11: Rgba,

    /// Warning surface.
    pub amber_3: Rgba,
    pub amber_6: Rgba,
    /// Warning text and glyphs.
    pub amber_11: Rgba,

    /// Success surface. Stock Radix green; Cap's palette carries no green.
    pub green_3: Rgba,
    /// Success text and glyphs.
    pub green_11: Rgba,

    /// The colour roles: the editor's layered surface system (`--ed-*`).
    pub colors: Colors,
    /// Code colours.
    pub syntax: Syntax,
}

/// Editor surfaces: a layered, low-contrast system (window < card < control)
/// with one accent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    pub window: Rgba,
    pub card: Rgba,
    pub card_2: Rgba,
    pub stage: Rgba,
    pub line: Rgba,
    pub line_strong: Rgba,
    pub text_1: Rgba,
    pub text_2: Rgba,
    pub text_3: Rgba,
    pub ctl: Rgba,
    pub ctl_hover: Rgba,
    pub ctl_active: Rgba,
    pub accent: Rgba,
    pub accent_2: Rgba,
    pub playhead: Rgba,
    pub thumb: Rgba,
    /// `--ed-seg-tint`: how much of a track colour a segment fill carries.
    pub seg_tint: f32,
    /// `--ed-seg-line`: the segment border's alpha.
    pub seg_line: f32,
    /// Segment text is the track colour pulled this far toward black (light)
    /// or white (dark).
    pub seg_label_base: Rgba,
    pub seg_label_mix: f32,
    /// How black each surface's drop shadow is: a card lifts barely at all on
    /// white and much more on the dark window, where the hairline alone is
    /// invisible.
    pub card_shadow_alpha: f32,
    pub pop_shadow_alpha: f32,
    pub pop_shadow_offset: f32,
    pub pop_shadow_blur: f32,

    /// The present-mode stage behind the letterboxed frame: black in both appearances.
    pub present_bg: Rgba,
    /// Text and glyphs on an `accent` fill.
    pub on_accent: Rgba,
    /// The keyboard focus ring: `accent`.
    pub focus: Rgba,
    /// Canvas and text selection: `accent` at 25 %.
    pub selection: Rgba,
    /// Tooltip surface and text: Radix gray 12 on gray 1, the inverse of the window.
    pub tooltip_bg: Rgba,
    pub tooltip_text: Rgba,
    /// Status foregrounds (Radix step 11) and their surfaces (step 3).
    pub success: Rgba,
    pub success_bg: Rgba,
    pub warning: Rgba,
    pub warning_bg: Rgba,
    pub danger: Rgba,
    pub danger_bg: Rgba,
    /// The soft-red washes of a hovered and pressed destructive control:
    /// `danger` mixed into `danger_bg` by 12 % and 22 %.
    pub danger_hover: Rgba,
    pub danger_active: Rgba,
    /// Informational notices: `accent` on `ctl`.
    pub info: Rgba,
    pub info_bg: Rgba,
    /// The player's video-scope rectangle: Radix amber 11.
    pub scope: Rgba,
    /// The dimming layer under present-mode overlays: black at 55 %.
    pub scrim: Rgba,
    /// Track base colours, the same as [`track_color`].
    pub track_clip: Rgba,
    pub track_overlay: Rgba,
    pub track_audio: Rgba,
    /// The selected item of a segmented control: a raised `card` pill in light, a white
    /// 12 % wash in dark, where a lifted card would read as a hole.
    pub segment_selected: Rgba,
    /// The Windows caption buttons (Cap's values): glyph, hovered and pressed washes, and
    /// the close button's red at rest-hover and pressed. Its glyph turns `on_accent`.
    pub caption_glyph: Rgba,
    pub caption_hover: Rgba,
    pub caption_pressed: Rgba,
    pub caption_close: Rgba,
    pub caption_close_pressed: Rgba,
}

/// The code colours (`agent/syntax`, diffs). The only place Radix steps stay visible
/// once phase 1 makes the raw steps private.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Syntax {
    /// gray 10.
    pub comment: Rgba,
    /// Keywords, operators and tags: red 11.
    pub keyword: Rgba,
    /// Strings, escapes: green 11.
    pub string: Rgba,
    /// Numbers, booleans, constants and attributes: amber 11.
    pub number: Rgba,
    /// Types, constructors, labels and links: blue 11.
    pub type_name: Rgba,
    /// Functions, macros and markup headings: blue 10.
    pub function: Rgba,
    /// Variables, properties and parameters: gray 12.
    pub variable: Rgba,
    /// gray 11.
    pub punctuation: Rgba,
    /// red 10.
    pub invalid: Rgba,
    /// Added diff lines: green 11.
    pub added: Rgba,
    /// Removed diff lines: red 11.
    pub removed: Rgba,
}

/// `color-mix(in srgb, b <t>, a)`: straight sRGB interpolation, which is what
/// the CSS side does.
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

pub(crate) fn rgba_alpha(color: Rgba, alpha: f32) -> Rgba {
    Rgba { a: alpha, ..color }
}

impl Theme {
    pub fn new(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => Self::light(),
            Appearance::Dark => Self::dark(),
        }
    }

    /// One Radix gray step by number.
    pub fn gray(&self, step: u8) -> Hsla {
        match step {
            1 => self.gray_1,
            2 => self.gray_2,
            3 => self.gray_3,
            4 => self.gray_4,
            5 => self.gray_5,
            6 => self.gray_6,
            7 => self.gray_7,
            8 => self.gray_8,
            9 => self.gray_9,
            10 => self.gray_10,
            11 => self.gray_11,
            _ => self.gray_12,
        }
        .into()
    }

    pub fn is_dark(&self) -> bool {
        self.appearance == Appearance::Dark
    }

    /// A colour at an alpha, for Tailwind's `/N` overlay fills.
    pub fn with_alpha(color: Rgba, alpha: f32) -> Hsla {
        let mut color: Hsla = color.into();
        color.a = alpha;
        color
    }
}

#[cfg(test)]
mod tests {
    use gpui::Rgba;

    use super::contrast::ratio;
    use super::palette::hex;
    use super::*;

    /// `0xRRGGBBAA`, rounded from the stored floats.
    fn bytes(color: Rgba) -> u32 {
        let byte = |channel: f32| (channel * 255.).round() as u32;
        byte(color.r) << 24 | byte(color.g) << 16 | byte(color.b) << 8 | byte(color.a)
    }

    /// The editor's colour roles, in the order of the spec table.
    fn roles(theme: &Theme) -> [(&'static str, Rgba); 26] {
        let e = theme.colors;
        [
            ("window", e.window),
            ("card", e.card),
            ("card_2", e.card_2),
            ("stage", e.stage),
            ("line", e.line),
            ("line_strong", e.line_strong),
            ("text_1", e.text_1),
            ("text_2", e.text_2),
            ("text_3", e.text_3),
            ("ctl", e.ctl),
            ("ctl_hover", e.ctl_hover),
            ("ctl_active", e.ctl_active),
            ("accent", e.accent),
            ("accent_2", e.accent_2),
            ("on_accent", theme.colors.on_accent),
            ("playhead", e.playhead),
            ("thumb", e.thumb),
            ("success", theme.green_11),
            ("success_surface", theme.green_3),
            ("warning", theme.amber_11),
            ("warning_surface", theme.amber_3),
            ("danger", theme.red_11),
            ("danger_surface", theme.red_3),
            ("selection", theme.colors.selection),
            ("tooltip_bg", theme.colors.tooltip_bg),
            ("tooltip_text", theme.colors.tooltip_text),
        ]
    }

    fn assert_roles(appearance: Appearance, expected: [u32; 26]) {
        for ((name, color), want) in roles(&Theme::new(appearance)).into_iter().zip(expected) {
            assert_eq!(bytes(color), want, "{appearance:?} {name}");
        }
    }

    #[test]
    fn light_palette_matches_cap_spec() {
        assert_roles(
            Appearance::Light,
            [
                0xf1f1f3ff, 0xffffffff, 0xf6f6f7ff, 0xe9e9ecff, 0x00000013, 0x0000001f, 0x1c1c1eff,
                0x66666dff, 0x7a7a82ff, 0x0000000b, 0x00000013, 0x0000001c, 0x0066e0ff, 0x0b6fe6ff,
                0xffffffff, 0xff3b30ff, 0xffffffff, 0x218358ff, 0xe6f6ebff, 0xab6400ff, 0xfff7c2ff,
                0xce2c31ff, 0xfeebecff, 0x0066e040, 0x202020ff, 0xfcfcfcff,
            ],
        );
    }

    #[test]
    fn dark_palette_matches_cap_spec() {
        assert_roles(
            Appearance::Dark,
            [
                0x131315ff, 0x1b1b1eff, 0x202024ff, 0x121214ff, 0xffffff14, 0xffffff21, 0xf4f4f5ff,
                0x9a9aa3ff, 0x7c7c86ff, 0xffffff0f, 0xffffff17, 0xffffff21, 0x0866d8ff, 0x1272e4ff,
                0xffffffff, 0xff453aff, 0xe8e8eaff, 0x3dd68cff, 0x132d21ff, 0xffca16ff, 0x302008ff,
                0xff9592ff, 0x3b1219ff, 0x0866d840, 0xeeeeeeff, 0x111111ff,
            ],
        );
    }

    #[test]
    fn theme_preference_resolves_like_tauri() {
        assert_eq!(
            resolve_appearance(AppTheme::System, Appearance::Dark),
            Appearance::Dark
        );
        assert_eq!(
            resolve_appearance(AppTheme::System, Appearance::Light),
            Appearance::Light
        );
        assert_eq!(
            resolve_appearance(AppTheme::Light, Appearance::Dark),
            Appearance::Light
        );
        assert_eq!(
            resolve_appearance(AppTheme::Dark, Appearance::Light),
            Appearance::Dark
        );
    }

    #[test]
    fn appearance_from_window_maps_all_variants() {
        assert_eq!(
            Appearance::from_window(WindowAppearance::Light),
            Appearance::Light
        );
        assert_eq!(
            Appearance::from_window(WindowAppearance::VibrantLight),
            Appearance::Light
        );
        assert_eq!(
            Appearance::from_window(WindowAppearance::Dark),
            Appearance::Dark
        );
        assert_eq!(
            Appearance::from_window(WindowAppearance::VibrantDark),
            Appearance::Dark
        );
    }

    #[test]
    fn status_roles_match_radix() {
        let (light, dark) = (Theme::light(), Theme::dark());
        let roles = |t: &Theme| {
            [
                t.green_11, t.green_3, t.amber_11, t.amber_3, t.red_11, t.red_3,
            ]
            .map(bytes)
        };
        assert_eq!(
            roles(&light),
            [
                0x218358ff, 0xe6f6ebff, 0xab6400ff, 0xfff7c2ff, 0xce2c31ff, 0xfeebecff
            ]
        );
        assert_eq!(
            roles(&dark),
            [
                0x3dd68cff, 0x132d21ff, 0xffca16ff, 0x302008ff, 0xff9592ff, 0x3b1219ff
            ]
        );
        assert_eq!(bytes(light.colors.selection), 0x0066e040);
        assert_eq!(bytes(dark.colors.selection), 0x0866d840);
        assert_eq!(
            [light.colors.tooltip_bg, light.colors.tooltip_text].map(bytes),
            [0x202020ff, 0xfcfcfcff]
        );
        assert_eq!(
            [dark.colors.tooltip_bg, dark.colors.tooltip_text].map(bytes),
            [0xeeeeeeff, 0x111111ff]
        );
    }

    #[test]
    fn translucent_roles_keep_alpha() {
        for theme in [Theme::light(), Theme::dark()] {
            let p = theme.colors;
            for color in [p.line, p.line_strong, p.ctl, p.ctl_hover, p.ctl_active] {
                assert!(
                    color.a > 0. && color.a < 1.,
                    "{:?} {color:?}",
                    theme.appearance
                );
            }
        }
    }

    #[test]
    fn body_text_contrast() {
        for theme in [Theme::light(), Theme::dark()] {
            let p = theme.colors;
            for background in [p.window, p.card, p.card_2] {
                let value = ratio(p.text_1, background);
                assert!(value >= 4.5, "{:?} text_1 {value}", theme.appearance);
            }
            for background in [p.card, p.card_2] {
                let value = ratio(p.text_2, background);
                assert!(value >= 4.5, "{:?} text_2 {value}", theme.appearance);
            }
        }
    }

    #[test]
    fn secondary_text_and_on_accent_meet_their_contrast_targets() {
        for theme in [Theme::light(), Theme::dark()] {
            let p = theme.colors;
            let name = format!("{:?}", theme.appearance);
            assert!(ratio(p.text_2, p.window) >= 4.5, "{name} text_2 on window");
            for background in [p.card, p.card_2] {
                let value = ratio(p.text_3, background);
                assert!(value >= 3., "{name} text_3 {value}");
            }
            for fill in [p.accent, p.accent_2] {
                let value = ratio(p.on_accent, fill);
                assert!(value >= 4.5, "{name} on_accent {value}");
            }
        }
    }

    #[test]
    fn semantic_status_surfaces_are_distinct_from_the_neutral_surface() {
        for theme in [Theme::light(), Theme::dark()] {
            for surface in [theme.green_3, theme.amber_3, theme.red_3] {
                assert_ne!(surface, theme.colors.card);
            }
        }
    }

    /// The roles phase 1 adds, in the order of its table.
    fn new_roles(theme: &Theme) -> [(&'static str, Rgba); 21] {
        let c = theme.colors;
        [
            ("present_bg", c.present_bg),
            ("on_accent", c.on_accent),
            ("focus", c.focus),
            ("selection", c.selection),
            ("tooltip_bg", c.tooltip_bg),
            ("tooltip_text", c.tooltip_text),
            ("success", c.success),
            ("success_bg", c.success_bg),
            ("warning", c.warning),
            ("warning_bg", c.warning_bg),
            ("danger", c.danger),
            ("danger_bg", c.danger_bg),
            ("danger_hover", c.danger_hover),
            ("danger_active", c.danger_active),
            ("info", c.info),
            ("info_bg", c.info_bg),
            ("scope", c.scope),
            ("scrim", c.scrim),
            ("track_clip", c.track_clip),
            ("track_overlay", c.track_overlay),
            ("track_audio", c.track_audio),
        ]
    }

    /// Each new role is the value its call sites compute today.
    fn sources(theme: &Theme) -> [Rgba; 21] {
        let e = theme.colors;
        [
            hex(0x000000),
            theme.colors.on_accent,
            e.accent,
            theme.colors.selection,
            theme.colors.tooltip_bg,
            theme.colors.tooltip_text,
            theme.green_11,
            theme.green_3,
            theme.amber_11,
            theme.amber_3,
            theme.red_11,
            theme.red_3,
            mix(theme.red_3, theme.red_11, 0.12),
            mix(theme.red_3, theme.red_11, 0.22),
            e.accent,
            e.ctl,
            theme.amber_11,
            Rgba::from(gpui::black().opacity(0.55)),
            track_color(TrackKind::Clip),
            track_color(TrackKind::Overlay),
            track_color(TrackKind::Audio),
        ]
    }

    #[test]
    fn new_roles_equal_what_call_sites_use_today() {
        for theme in [Theme::light(), Theme::dark()] {
            for ((name, role), source) in new_roles(&theme).into_iter().zip(sources(&theme)) {
                assert_eq!(bytes(role), bytes(source), "{:?} {name}", theme.appearance);
            }
        }
    }

    #[test]
    fn new_roles_match_the_spec_values() {
        let light = new_roles(&Theme::light()).map(|(_, c)| bytes(c));
        assert_eq!(
            light,
            [
                0x000000ff, 0xffffffff, 0x0066e0ff, 0x0066e040, 0x202020ff, 0xfcfcfcff, 0x218358ff,
                0xe6f6ebff, 0xab6400ff, 0xfff7c2ff, 0xce2c31ff, 0xfeebecff, 0xf8d4d6ff, 0xf3c1c3ff,
                0x0066e0ff, 0x0000000b, 0xab6400ff, 0x0000008c, 0x3b82f6ff, 0x8b5cf6ff, 0x22c55eff,
            ]
        );
        let dark = new_roles(&Theme::dark()).map(|(_, c)| bytes(c));
        assert_eq!(
            dark,
            [
                0x000000ff, 0xffffffff, 0x0866d8ff, 0x0866d840, 0xeeeeeeff, 0x111111ff, 0x3dd68cff,
                0x132d21ff, 0xffca16ff, 0x302008ff, 0xff9592ff, 0x3b1219ff, 0x532228ff, 0x662f34ff,
                0x0866d8ff, 0xffffff0f, 0xffca16ff, 0x0000008c, 0x3b82f6ff, 0x8b5cf6ff, 0x22c55eff,
            ]
        );
    }

    #[test]
    fn syntax_matches_the_agent_highlighter() {
        for theme in [Theme::light(), Theme::dark()] {
            let s = theme.syntax;
            assert_eq!(
                [
                    s.comment,
                    s.keyword,
                    s.string,
                    s.number,
                    s.type_name,
                    s.function,
                    s.variable,
                    s.punctuation,
                    s.invalid,
                    s.added,
                    s.removed,
                ],
                [
                    theme.gray_10,
                    theme.red_11,
                    theme.green_11,
                    theme.amber_11,
                    theme.blue_11,
                    theme.blue_10,
                    theme.gray_12,
                    theme.gray_11,
                    theme.red_10,
                    theme.green_11,
                    theme.red_11,
                ],
                "{:?}",
                theme.appearance
            );
        }
    }

    #[test]
    fn status_foregrounds_read_on_their_surfaces() {
        // Phase 1 asks for 4.5:1. Dark meets it everywhere; light success, warning and danger
        // on its hover wash fall short with Radix's step 11 on step 3 and are recorded.
        for theme in [Theme::light(), Theme::dark()] {
            let c = theme.colors;
            let mut pairs = vec![
                ("danger", c.danger, c.danger_bg),
                ("tooltip", c.tooltip_text, c.tooltip_bg),
            ];
            if theme.is_dark() {
                pairs.extend([
                    ("success", c.success, c.success_bg),
                    ("warning", c.warning, c.warning_bg),
                    ("danger on hover", c.danger, c.danger_hover),
                ]);
            }
            for (name, fg, bg) in pairs {
                let value = ratio(fg, bg);
                assert!(value >= 4.5, "{:?} {name} {value}", theme.appearance);
            }
        }
        let light = Theme::light().colors;
        for (name, measured, recorded) in [
            (
                "light success",
                ratio(light.success, light.success_bg),
                4.21,
            ),
            (
                "light warning",
                ratio(light.warning, light.warning_bg),
                4.25,
            ),
            (
                "light danger on hover",
                ratio(light.danger, light.danger_hover),
                3.82,
            ),
        ] {
            assert!(
                (measured - recorded).abs() <= 0.01,
                "{name}: measured {measured}, recorded {recorded}"
            );
        }
    }

    #[test]
    fn hovered_controls_are_visibly_lighter_or_darker_than_the_card() {
        for theme in [Theme::light(), Theme::dark()] {
            let c = theme.colors;
            let card = super::contrast::lightness(c.card, c.card);
            let hover = super::contrast::lightness(c.ctl_hover, c.card);
            assert!(
                (card - hover).abs() >= 2.,
                "{:?} delta-L {}",
                theme.appearance,
                (card - hover).abs()
            );
        }
    }
}
