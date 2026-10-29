//! `Chip`: a compact token for context, attached files and suggestions (the agent composer).
//!
//! `chip_h` (28) high with `item` corners on a `ctl` fill: an optional 14 px icon, a `Label`
//! that truncates at the chip's maximum width, and with [`Chip::removable`] a 16 px remove
//! button at the end. The button's space is always reserved; it shows while the chip is hovered
//! or the button has keyboard focus. [`Chip::tone`] colours the icon only.

use std::rc::Rc;

use gpui::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, Rgba, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::badge::Tone;
use super::icon::{Icon, icon};
use super::interact::{Handler, focusable, on_activate};

/// The widest a chip grows before its label truncates.
const CHIP_MAX_W: f32 = 220.;
/// The remove button's side.
const REMOVE_SIZE: f32 = 16.;
/// The cross inside it.
const REMOVE_ICON: f32 = 12.;
/// The space between a chip's parts.
const CHIP_GAP: f32 = 6.;

/// The icon's colour: `text_2` for neutral, the status colours otherwise.
pub(crate) fn chip_icon_color(theme: &Theme, tone: Tone) -> Rgba {
    let colors = theme.colors;
    match tone {
        Tone::Neutral => colors.text_2,
        Tone::Info => colors.info,
        Tone::Success => colors.success,
        Tone::Warning => colors.warning,
        Tone::Danger => colors.danger,
    }
}

#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    theme: Theme,
    label: SharedString,
    icon: Option<Icon>,
    tone: Tone,
    on_remove: Option<Handler>,
}

impl Chip {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            label: label.into(),
            icon: None,
            tone: Tone::Neutral,
            on_remove: None,
        }
    }

    pub fn icon(mut self, glyph: Icon) -> Self {
        self.icon = Some(glyph);
        self
    }

    /// Colours the icon; the fill and label stay neutral.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Adds a remove button (id `"{id}-remove"`) that runs `on_remove` on click, Enter or Space.
    pub fn removable(mut self, on_remove: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_remove = Some(Rc::new(on_remove));
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Chip {
            id,
            theme,
            label,
            icon: glyph,
            tone,
            on_remove,
        } = self;
        let colors = theme.colors;
        let group = SharedString::from(format!("chip-{id}"));

        let remove = on_remove.map(|handler| {
            let button = focusable(
                div().id(ElementId::Name(format!("{id}-remove").into())),
                &theme,
                false,
            );
            on_activate(button, false, Some(handler))
                .role(Role::Button)
                .aria_label(SharedString::from(format!("Remove {label}")))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(REMOVE_SIZE))
                .rounded(px(METRICS.radius.item))
                .cursor_pointer()
                .opacity(0.)
                .group_hover(group.clone(), |style| style.opacity(1.))
                .focus(|style| style.opacity(1.))
                .hover(move |style| style.bg(colors.ctl_hover))
                .child(icon(Icon::X, REMOVE_ICON, colors.text_2))
        });

        div()
            .id(id)
            .group(group)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(CHIP_GAP))
            .h(px(METRICS.size.chip_h))
            .max_w(px(CHIP_MAX_W))
            .px(px(METRICS.space_2))
            .rounded(px(METRICS.radius.item))
            .bg(colors.ctl)
            .children(
                glyph.map(|glyph| icon(glyph, METRICS.size.icon_sm, chip_icon_color(&theme, tone))),
            )
            .child(
                text_style(div(), TextStyle::Label)
                    .min_w_0()
                    .truncate()
                    .text_color(colors.text_1)
                    .child(label),
            )
            .children(remove)
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};
    use studio_theme::Appearance;

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    #[test]
    fn tone_colours_the_icon_from_the_roles() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let theme = Theme::new(appearance);
            let c = theme.colors;
            assert_eq!(chip_icon_color(&theme, Tone::Neutral), c.text_2);
            assert_eq!(chip_icon_color(&theme, Tone::Success), c.success);
            assert_eq!(chip_icon_color(&theme, Tone::Danger), c.danger);
        }
    }

    struct Chips {
        removed: Log<()>,
    }

    impl Render for Chips {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let removed = self.removed.clone();
            Chip::new(&theme, "file", "main.rs")
                .icon(Icon::File)
                .removable(move |_, _| removed.borrow_mut().push(()))
        }
    }

    #[gpui::test]
    fn the_remove_button_works_from_the_keyboard(cx: &mut TestAppContext) {
        let removed = log();
        let shared = removed.clone();
        let (_, cx) = cx.add_window_view(|_, _| Chips { removed: shared });
        tab(cx);
        press(cx, "enter");
        assert_eq!(removed.borrow().len(), 1);
    }
}
