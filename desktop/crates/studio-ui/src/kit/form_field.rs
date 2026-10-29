//! `FormField`: a label and its control, in a row or stacked.
//!
//! The phase 3 `Field` of the design-system plan, named `FormField` while the older
//! `field::Field` and `field::Subfield` (the config sidebar's containers) still exist. It is
//! meant to replace them, `text::field_row`, `surface::SettingRow` and the props panel's
//! `field_row`: phases 5 and 7 move those callers here and delete the old items.
//!
//! - [`FormField::row`]: label left, control right, at least `field_row_h` (34) high.
//! - [`FormField::stacked`]: label above the control, 4 px apart.
//!
//! Both take a `hint` (`Small`, `text_3`) or an `error` (`Small`, `danger`, shown instead of the
//! hint) under them, and a [`FormField::reset`] action: a small reset button beside the label,
//! shown while the pointer is over the field or the button has keyboard focus.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::icon::{Icon, icon};
use super::interact::{Handler, focusable, on_activate, with_tooltip};

/// The label column of a row, so a column of rows starts its controls at one x.
const ROW_LABEL_MIN_W: f32 = 96.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    Row,
    Stacked,
}

#[derive(IntoElement)]
pub struct FormField {
    theme: Theme,
    layout: Layout,
    label: SharedString,
    control: AnyElement,
    hint: Option<SharedString>,
    error: Option<SharedString>,
    on_reset: Option<Handler>,
    reset_id: Option<SharedString>,
}

impl FormField {
    fn new(
        theme: &Theme,
        layout: Layout,
        label: impl Into<SharedString>,
        control: impl IntoElement,
    ) -> Self {
        Self {
            theme: *theme,
            layout,
            label: label.into(),
            control: control.into_any_element(),
            hint: None,
            error: None,
            on_reset: None,
            reset_id: None,
        }
    }

    /// Label left, control right, at least `field_row_h` high.
    pub fn row(theme: &Theme, label: impl Into<SharedString>, control: impl IntoElement) -> Self {
        Self::new(theme, Layout::Row, label, control)
    }

    /// Label above, control below, 4 px apart.
    pub fn stacked(
        theme: &Theme,
        label: impl Into<SharedString>,
        control: impl IntoElement,
    ) -> Self {
        Self::new(theme, Layout::Stacked, label, control)
    }

    /// A note under the field (`Small`, `text_3`).
    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// What is wrong with the value (`Small`, `danger`); replaces the hint while set.
    pub fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Shows a reset button beside the label while the field is hovered. Its id is
    /// `"{label}-reset"`.
    pub fn reset(mut self, on_reset: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_reset = Some(Rc::new(on_reset));
        self
    }

    /// The reset button's element id, for a field whose reset already had one.
    pub fn reset_id(mut self, id: impl Into<SharedString>) -> Self {
        self.reset_id = Some(id.into());
        self
    }
}

impl RenderOnce for FormField {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let FormField {
            theme,
            layout,
            label,
            control,
            hint,
            error,
            on_reset,
            reset_id,
        } = self;
        let colors = theme.colors;
        let group = SharedString::from(format!("form-field-{label}"));

        let reset = on_reset.map(|handler| {
            let id = ElementId::Name(
                reset_id.unwrap_or_else(|| SharedString::from(format!("{label}-reset"))),
            );
            let button = focusable(div().id(id), &theme, false);
            let button = with_tooltip(button, &theme, Some("Reset to default".into()));
            on_activate(button, false, Some(handler))
                .role(Role::Button)
                .aria_label(SharedString::from("Reset to default"))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(METRICS.size.icon_button_sm))
                .rounded(px(METRICS.radius.control))
                .cursor_pointer()
                .opacity(0.)
                .group_hover(group.clone(), |style| style.opacity(1.))
                .focus(|style| style.opacity(1.))
                .hover(move |style| style.bg(colors.ctl_hover))
                .child(icon(Icon::Undo, METRICS.size.icon_sm, colors.text_2))
        });

        let label_row = div()
            .flex()
            .items_center()
            .gap(px(METRICS.space_1))
            .child(
                text_style(div(), TextStyle::Body)
                    .text_color(colors.text_1)
                    .child(label),
            )
            .children(reset);

        let note = match (error, hint) {
            (Some(error), _) => Some((error, colors.danger)),
            (None, Some(hint)) => Some((hint, colors.text_3)),
            (None, None) => None,
        }
        .map(|(text, color)| {
            text_style(div(), TextStyle::Small)
                .text_color(color)
                .child(text)
        });

        let field = div().group(group).flex().flex_col();
        match layout {
            Layout::Row => field
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(METRICS.space_2))
                        .min_h(px(METRICS.size.field_row_h))
                        .child(label_row.flex_none().min_w(px(ROW_LABEL_MIN_W)))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .items_center()
                                .justify_end()
                                .child(control),
                        ),
                )
                .children(note),
            Layout::Stacked => field
                .gap(px(METRICS.space_1))
                .child(label_row)
                .child(control)
                .children(note),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    struct Fields {
        resets: Log<&'static str>,
    }

    impl Render for Fields {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let resets = self.resets.clone();
            let control = || div().w(px(40.)).h(px(10.));
            div()
                .w(px(300.))
                .child(
                    div().debug_selector(|| "row".into()).child(
                        FormField::row(&theme, "Opacity", control())
                            .reset(move |_, _| resets.borrow_mut().push("opacity")),
                    ),
                )
                .child(
                    div()
                        .debug_selector(|| "stacked".into())
                        .child(FormField::stacked(&theme, "Notes", control()).hint("Optional")),
                )
        }
    }

    #[gpui::test]
    fn the_reset_button_is_a_tab_stop_that_runs_the_callback(cx: &mut TestAppContext) {
        let resets = log();
        let shared = resets.clone();
        let (_, cx) = cx.add_window_view(|_, _| Fields { resets: shared });
        tab(cx);
        press(cx, "enter");
        press(cx, "space");
        assert_eq!(*resets.borrow(), ["opacity", "opacity"]);
    }

    #[gpui::test]
    fn a_row_is_a_field_row_high(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Fields { resets: log() });
        cx.run_until_parked();
        let row = cx.debug_bounds("row").expect("row");
        assert_eq!(f32::from(row.size.height), METRICS.size.field_row_h);
        let stacked = cx.debug_bounds("stacked").expect("stacked");
        // Label row, gap, control, gap, hint.
        let expected = TextStyle::Body.line_height()
            + METRICS.space_1
            + 10.
            + METRICS.space_1
            + TextStyle::Small.line_height();
        assert_eq!(f32::from(stacked.size.height), expected);
    }
}
