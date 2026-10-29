//! `Kbd` -- one keycap chip for the three the Solid app grew independently:
//! the one inside `Tooltip`'s bubble, onboarding's "physical key" look, and
//! `settings/hotkeys.tsx`'s `HotkeyText` squares. All three re-implement the
//! macOS-glyph-versus-Windows-label mapping; that lives here once.

use gpui::{App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

/// `kbdSymbolModifier(key, os)`: macOS renders the glyphs, every other OS
/// renders the word. Non-modifier keys pass through with the `Key` prefix
/// stripped, which is what `HotkeyText` does with a `KeyboardEvent.code`.
pub fn kbd_symbol(key: &str) -> SharedString {
    let mac = cfg!(target_os = "macos");
    let symbol = match key.to_ascii_lowercase().as_str() {
        "meta" | "cmd" | "command" | "super" => {
            if mac {
                "⌘"
            } else {
                "Win"
            }
        }
        "ctrl" | "control" => {
            if mac {
                "⌃"
            } else {
                "Ctrl"
            }
        }
        "shift" => {
            if mac {
                "⇧"
            } else {
                "Shift"
            }
        }
        "alt" | "option" => {
            if mac {
                "⌥"
            } else {
                "Alt"
            }
        }
        "enter" | "return" => "↩",
        "escape" | "esc" => "esc",
        "backspace" => "⌫",
        "space" => "␣",
        _ => return SharedString::from(key.strip_prefix("Key").unwrap_or(key).to_string()),
    };
    SharedString::from(symbol)
}

/// A keycap: a square chip on `ctl` behind a `line_strong` hairline, or, inside a
/// tooltip, a small chip in the tooltip's inverse colours.
#[derive(IntoElement)]
pub struct KbdChip {
    theme: Theme,
    key: SharedString,
    in_tooltip: bool,
}

/// The standalone chip's least side, and the paddings of both looks.
const CHIP_MIN: f32 = 24.;
const CHIP_PAD_X: f32 = 4.;
const TOOLTIP_PAD_X: f32 = 5.;
const PAD_Y: f32 = 2.;

impl KbdChip {
    pub fn new(theme: &Theme, key: impl AsRef<str>) -> Self {
        Self {
            theme: *theme,
            key: kbd_symbol(key.as_ref()),
            in_tooltip: false,
        }
    }

    /// The small chip inside a tooltip bubble.
    pub fn in_tooltip(mut self) -> Self {
        self.in_tooltip = true;
        self
    }
}

impl RenderOnce for KbdChip {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let KbdChip {
            theme,
            key,
            in_tooltip,
        } = self;
        let colors = theme.colors;
        let chip = div()
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .py(px(PAD_Y))
            .rounded(px(METRICS.radius.item));
        if in_tooltip {
            text_style(chip, TextStyle::Caption)
                .px(px(TOOLTIP_PAD_X))
                .bg(colors.tooltip_text)
                .text_color(colors.tooltip_bg)
                .child(key)
        } else {
            text_style(chip, TextStyle::Small)
                .px(px(CHIP_PAD_X))
                .min_w(px(CHIP_MIN))
                .h(px(CHIP_MIN))
                .bg(colors.ctl)
                .border_1()
                .border_color(colors.line_strong)
                .text_color(colors.text_1)
                .child(key)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn modifiers_render_as_glyphs_on_macos() {
        assert_eq!(kbd_symbol("meta").as_ref(), "⌘");
        assert_eq!(kbd_symbol("ctrl").as_ref(), "⌃");
        assert_eq!(kbd_symbol("shift").as_ref(), "⇧");
        assert_eq!(kbd_symbol("alt").as_ref(), "⌥");
    }

    #[test]
    fn a_key_code_loses_its_prefix() {
        assert_eq!(kbd_symbol("KeyA").as_ref(), "A");
        assert_eq!(kbd_symbol("F5").as_ref(), "F5");
    }

    #[test]
    fn the_mapping_is_case_insensitive() {
        assert_eq!(kbd_symbol("Shift"), kbd_symbol("shift"));
        assert_eq!(kbd_symbol("COMMAND"), kbd_symbol("cmd"));
    }
}
