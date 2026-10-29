//! `Toggle` -- `apps/desktop/src/components/Toggle.tsx`, a Kobalte `Switch`: a
//! `ctl_active` track that turns `accent` when on, with a `thumb` knob.

use gpui::{
    App, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, Toggled, Window, div, prelude::FluentBuilder,
    px,
};

use studio_theme::Theme;

use super::interact::{dim_disabled, focusable};

/// The track (`w-9 h-5 p-0.5`, 34 by 20 with a 2 px inset) and its 16 px knob.
const TRACK_W: f32 = 34.;
const TRACK_H: f32 = 20.;
const TRACK_INSET: f32 = 2.;
const KNOB: f32 = TRACK_H - 2. * TRACK_INSET;

#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    theme: Theme,
    checked: bool,
    disabled: bool,
    /// The accessible name.
    label: Option<SharedString>,
    on_click: Option<crate::kit::button::ClickHandler>,
}

/// The value after an activation, or `None` when disabled.
pub fn toggled(checked: bool, disabled: bool) -> Option<bool> {
    (!disabled).then_some(!checked)
}

impl Toggle {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, checked: bool) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            checked,
            disabled: false,
            label: None,
            on_click: None,
        }
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

    /// Called with the new value when a click, Enter or Space flips the toggle.
    pub fn on_change(self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        let next = toggled(self.checked, self.disabled);
        self.on_click(move |_, window, cx| {
            if let Some(value) = next {
                handler(value, window, cx);
            }
        })
    }

    /// The name a screen reader announces for the switch.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Toggle {
            id,
            theme,
            checked,
            disabled,
            label,
            on_click,
        } = self;
        let colors = theme.colors;

        let element = focusable(div().id(id), &theme, disabled)
            .role(Role::Switch)
            .aria_toggled(if checked {
                Toggled::True
            } else {
                Toggled::False
            })
            .when_some(label, |this, label| this.aria_label(label))
            .w(px(TRACK_W))
            .h(px(TRACK_H))
            .p(px(TRACK_INSET))
            .flex()
            .flex_row()
            .flex_shrink_0()
            .rounded_full()
            // `data-checked:translate-x-[calc(100%)]` on the thumb -- the same
            // end position a flex justification reaches, without a transform.
            .when(checked, |this| this.justify_end())
            .when(!disabled, |this| this.cursor_pointer())
            .bg(if checked {
                colors.accent
            } else {
                colors.ctl_active
            })
            .child(div().size(px(KNOB)).rounded_full().bg(colors.thumb));
        dim_disabled(element, disabled)
            .when_some(on_click.filter(|_| !disabled), |this, handler| {
                this.on_click(move |event, window, cx| handler(event, window, cx))
            })
    }
}
#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    struct Switches {
        changes: Log<(&'static str, bool)>,
    }

    impl Render for Switches {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let change = |name: &'static str| {
                let changes = self.changes.clone();
                move |value: bool, _: &mut Window, _: &mut App| {
                    changes.borrow_mut().push((name, value))
                }
            };
            div()
                .child(
                    Toggle::new(&theme, "locked", false)
                        .disabled(true)
                        .label("Locked")
                        .on_change(change("locked")),
                )
                .child(
                    Toggle::new(&theme, "loop", true)
                        .label("Loop")
                        .on_change(change("loop")),
                )
        }
    }

    #[gpui::test]
    fn space_flips_the_focused_switch_and_tab_skips_a_disabled_one(cx: &mut TestAppContext) {
        let changes = log();
        let shared = changes.clone();
        let (_, cx) = cx.add_window_view(|_, _| Switches { changes: shared });
        tab(cx);
        press(cx, "space");
        assert_eq!(*changes.borrow(), [("loop", false)]);
    }

    #[test]
    fn activation_flips_unless_disabled() {
        assert_eq!(toggled(false, false), Some(true));
        assert_eq!(toggled(true, false), Some(false));
        assert_eq!(toggled(true, true), None);
    }
}
