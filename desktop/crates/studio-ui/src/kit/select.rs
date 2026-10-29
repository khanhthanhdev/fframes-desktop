//! `Select`'s trigger -- the closed half of the control [`crate::kit::Menu`]
//! opens: a borderless `ctl` pill with the value and a chevron.
//!
//! The chevron does not rotate: this gpui rev has no transform.

use gpui::{
    App, Bounds, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, svg,
};

use super::interact::{States, dim_disabled, focusable};
use super::menu::OpenHandler;
use studio_theme::{METRICS, TextStyle, Theme, text_style};

/// The trigger's horizontal padding and its chevron.
const PAD_X: f32 = 10.;
const CHEVRON: f32 = 12.;

#[derive(IntoElement)]
pub struct Select {
    id: ElementId,
    theme: Theme,
    label: SharedString,
    /// No fill until hovered: the composer's model picker.
    ghost: bool,
    stretch: bool,
    disabled: bool,
    on_click: Option<crate::kit::button::ClickHandler>,
    on_open: Option<OpenHandler>,
}

impl Select {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            label: label.into(),
            ghost: false,
            stretch: false,
            disabled: false,
            on_click: None,
            on_open: None,
        }
    }

    /// No resting fill: a trigger inside a toolbar.
    pub fn ghost(mut self) -> Self {
        self.ghost = true;
        self
    }

    /// The label takes the remaining width and the chevron is pushed to the end.
    pub fn stretch_label(mut self) -> Self {
        self.stretch = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    pub fn on_open(
        mut self,
        handler: impl Fn(&Bounds<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Select {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Select {
            id,
            theme,
            label,
            ghost,
            stretch,
            disabled,
            on_click,
            on_open,
        } = self;
        let colors = theme.colors;
        let states = States::of(&theme);

        let element = focusable(div().id(id), &theme, disabled)
            .role(Role::ComboBox)
            .aria_label(label.clone())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(METRICS.space_2))
            .px(px(PAD_X))
            .h(px(METRICS.size.select_h))
            .rounded(px(METRICS.radius.control))
            .when(!ghost, |this| this.bg(states.rest))
            .when(!disabled, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(states.hover))
            });
        dim_disabled(text_style(element, TextStyle::Body), disabled)
            .text_color(colors.text_1)
            .map(|this| {
                if stretch {
                    this.child(div().flex_1().truncate().child(label))
                } else {
                    this.child(label)
                }
            })
            .child(
                svg()
                    .path("icons/chevron-down.svg")
                    .size(px(CHEVRON))
                    .flex_shrink_0()
                    .text_color(colors.text_3),
            )
            .when_some(on_click.filter(|_| !disabled), |this, handler| {
                this.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .when_some(on_open.filter(|_| !disabled), crate::kit::Menu::trigger)
    }
}
#[cfg(test)]
mod tests {
    use gpui::{Context, ParentElement, Render, TestAppContext};

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    struct Pickers {
        opened: Log<&'static str>,
    }

    impl Render for Pickers {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let open = |name: &'static str| {
                let opened = self.opened.clone();
                move |_: &ClickEvent, _: &mut Window, _: &mut App| opened.borrow_mut().push(name)
            };
            div()
                .child(
                    Select::new(&theme, "locked", "Locked")
                        .disabled(true)
                        .on_click(open("locked")),
                )
                .child(Select::new(&theme, "fps", "30 fps").on_click(open("fps")))
        }
    }

    #[gpui::test]
    fn enter_opens_the_focused_select_and_tab_skips_a_disabled_one(cx: &mut TestAppContext) {
        let opened = log();
        let shared = opened.clone();
        let (_, cx) = cx.add_window_view(|_, _| Pickers { opened: shared });
        tab(cx);
        press(cx, "enter");
        assert_eq!(*opened.borrow(), ["fps"]);
    }
}
