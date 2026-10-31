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

/// The keycaps of one keystroke in gpui's syntax (`secondary-shift-z`), modifiers first:
/// `["Ctrl", "Shift", "Z"]`, or the glyphs on macOS. Named keys are spelled out (`Space`,
/// `PageDown`, `Esc`) and the arrows are arrows. `None` when gpui cannot parse `keys`.
pub fn keystroke_caps(keys: &str) -> Option<Vec<SharedString>> {
    let keystroke = gpui::Keystroke::parse(keys).ok()?;
    let modifiers = keystroke.modifiers;
    let mut caps: Vec<SharedString> = [
        (modifiers.control, "ctrl"),
        (modifiers.alt, "alt"),
        (modifiers.shift, "shift"),
        (modifiers.platform, "cmd"),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .map(|(_, name)| kbd_symbol(name))
    .collect();
    let key = match keystroke.key.as_str() {
        "space" => "Space".to_owned(),
        "escape" => "Esc".to_owned(),
        "enter" => "Enter".to_owned(),
        "backspace" => "Backspace".to_owned(),
        "delete" => "Delete".to_owned(),
        "home" => "Home".to_owned(),
        "end" => "End".to_owned(),
        "pageup" => "PageUp".to_owned(),
        "pagedown" => "PageDown".to_owned(),
        "tab" => "Tab".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        other => other.to_uppercase(),
    };
    caps.push(key.into());
    Some(caps)
}

/// A keycap, Oleafly's `Kbd`: a borderless 20 px chip on the quiet `ctl` fill with `Label`
/// text in `text_2`, or, inside a tooltip, a smaller `Caption` chip in the same colours.
#[derive(IntoElement)]
pub struct KbdChip {
    theme: Theme,
    key: SharedString,
    in_tooltip: bool,
}

/// The standalone chip's least side (`h-5 min-w-5`), and the paddings of both looks.
const CHIP_MIN: f32 = 20.;
const CHIP_PAD_X: f32 = 4.;
const TOOLTIP_PAD_X: f32 = 4.;
const PAD_Y: f32 = 2.;

impl KbdChip {
    pub fn new(theme: &Theme, key: impl AsRef<str>) -> Self {
        Self {
            theme: *theme,
            key: kbd_symbol(key.as_ref()),
            in_tooltip: false,
        }
    }

    /// A chip showing `cap` as it is, for caps [`keystroke_caps`] already named.
    pub fn literal(theme: &Theme, cap: impl Into<SharedString>) -> Self {
        Self {
            theme: *theme,
            key: cap.into(),
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
            .rounded(px(METRICS.radius.item))
            .bg(colors.ctl)
            .text_color(colors.text_2);
        if in_tooltip {
            text_style(chip, TextStyle::Caption)
                .px(px(TOOLTIP_PAD_X))
                .child(key)
        } else {
            text_style(chip, TextStyle::Label)
                .px(px(CHIP_PAD_X))
                .min_w(px(CHIP_MIN))
                .h(px(CHIP_MIN))
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
    #[cfg(not(target_os = "macos"))]
    fn a_keystroke_becomes_its_modifiers_then_its_key() {
        let caps = |keys| keystroke_caps(keys).unwrap();
        assert_eq!(caps("secondary-shift-z"), ["Ctrl", "Shift", "Z"]);
        assert_eq!(caps("pagedown"), ["PageDown"]);
        assert_eq!(caps("left"), ["←"]);
        assert_eq!(caps("f5"), ["F5"]);
        assert_eq!(caps("shift-="), ["Shift", "="]);
    }

    #[test]
    fn the_mapping_is_case_insensitive() {
        assert_eq!(kbd_symbol("Shift"), kbd_symbol("shift"));
        assert_eq!(kbd_symbol("COMMAND"), kbd_symbol("cmd"));
    }
}
