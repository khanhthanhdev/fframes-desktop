//! `SettingRow`: one setting on a settings page, the first-run setup, Create or Export.
//!
//! Oleafly's setting row (`rounded-lg border bg-card p-3`): a flat card 12 px in, an optional
//! leading icon, the label (`BodyMedium`, `text_1`) and its description (`Small`, `text_2`)
//! on the left, the control on the right. When the row is too narrow for both, the control
//! wraps below the text; [`SettingRow::below`] puts one under the text always, for a control
//! too wide to sit beside it. [`SettingRow::link`] makes the whole row a link with Oleafly's
//! external-link mark at its end. Rows stack 8 px apart in a
//! [`PanelSection`](super::PanelSection); [`setting_note`] is the muted note row between
//! them.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    Rgba, Role, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::interact::{Handler, States, focusable};
use super::{Card, Icon, icon, selectable};

/// The narrowest the label column gets before the control wraps below it.
const TEXT_MIN_W: f32 = 200.;
/// A note's fill: Oleafly's `bg-muted/30`.
const NOTE_FILL_ALPHA: f32 = 0.3;

/// The surface of a setting row, for content that is not a label and a control (a status
/// line, a group of actions): the flat card, 12 px in. As Oleafly's `bg-card` on its muted
/// settings page, the card is lighter than the page: the document white in light, a grey
/// between `card` and `card_2` in dark.
pub fn setting_card(theme: &Theme) -> Div {
    let colors = theme.colors;
    let fill: gpui::Hsla = if theme.is_dark() {
        Theme::with_alpha(colors.card_2, DARK_CARD_ALPHA)
    } else {
        colors.window.into()
    };
    Card::raised(theme).bg(fill).p(px(METRICS.space_3))
}

/// How much of `card_2` a setting card lays over the dark page.
const DARK_CARD_ALPHA: f32 = 0.5;

/// A note between setting rows, Oleafly's `SettingsNote`: an `Info` icon and `Small` text in
/// `text_2` on a muted fill behind a hairline, at the row's radius.
pub fn setting_note(theme: &Theme, text: impl Into<SharedString>) -> Div {
    let colors = theme.colors;
    text_style(div(), TextStyle::Small)
        .flex()
        .items_start()
        .gap(px(METRICS.space_2))
        .px(px(METRICS.space_3))
        .py(px(METRICS.space_2))
        .rounded(px(METRICS.radius.card))
        .border(px(METRICS.size.divider))
        .border_color(colors.line)
        .bg(Rgba {
            a: NOTE_FILL_ALPHA,
            ..colors.card_2
        })
        .text_color(colors.text_2)
        // The 14 px icon sits on the first line's centre (16 px line).
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(px(TextStyle::Small.line_height()))
                .child(icon(Icon::Info, METRICS.size.icon_sm, colors.text_2)),
        )
        .child(div().flex_1().min_w_0().child(selectable(text.into())))
}

#[derive(IntoElement)]
pub struct SettingRow {
    theme: Theme,
    label: SharedString,
    description: Option<SharedString>,
    icon: Option<Icon>,
    control: Option<AnyElement>,
    below: Option<AnyElement>,
    link: Option<(ElementId, Handler)>,
}

impl SettingRow {
    pub fn new(theme: &Theme, label: impl Into<SharedString>) -> Self {
        Self {
            theme: *theme,
            label: label.into(),
            description: None,
            icon: None,
            control: None,
            below: None,
            link: None,
        }
    }

    /// A line under the label.
    pub fn description(mut self, text: impl Into<SharedString>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// A 16 px icon before the label, in `text_2`.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The control at the row's right end.
    pub fn control(mut self, control: impl IntoElement) -> Self {
        self.control = Some(control.into_any_element());
        self
    }

    /// A control under the label, for one too wide to sit beside it.
    pub fn below(mut self, element: impl IntoElement) -> Self {
        self.below = Some(element.into_any_element());
        self
    }

    /// Makes the whole row a link (`id`) that runs `open`: a tab stop that Enter and Space
    /// open, the `ctl` fill under the pointer, and the external-link mark at its end when
    /// it has no control.
    pub fn link(
        mut self,
        id: impl Into<ElementId>,
        open: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.link = Some((id.into(), Rc::new(open)));
        self
    }
}

impl RenderOnce for SettingRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let colors = theme.colors;
        let label = self.label;
        // A link's text is its name; selecting it would fight the click.
        let linked = self.link.is_some();
        let text_of = |text: SharedString| -> AnyElement {
            if linked {
                text.into_any_element()
            } else {
                selectable(text).into_any_element()
            }
        };
        // The text grows from its minimum; a control that does not fit beside it wraps.
        let text = div()
            .flex()
            .flex_col()
            .flex_1()
            .flex_basis(px(TEXT_MIN_W))
            .min_w_0()
            .child(
                text_style(div(), TextStyle::BodyMedium)
                    .text_color(colors.text_1)
                    .child(text_of(label.clone())),
            )
            .children(self.description.map(|description| {
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_2)
                    .child(text_of(description))
            }));
        let control = self.control.or_else(|| {
            linked.then(|| {
                icon(Icon::ExternalLink, METRICS.size.icon_md, colors.text_2).into_any_element()
            })
        });
        let row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(METRICS.space_3))
            .children(self.icon.map(|glyph| {
                div().flex().flex_none().items_center().child(icon(
                    glyph,
                    METRICS.size.icon_md,
                    colors.text_2,
                ))
            }))
            .child(text)
            .children(control.map(|control| div().flex().flex_none().child(control)));
        let card = setting_card(&theme)
            .flex()
            .flex_col()
            .gap(px(METRICS.space_3))
            .child(row)
            // In a row of its own, so the control keeps its width instead of stretching.
            .children(self.below.map(|below| div().flex().child(below)));
        match self.link {
            None => card.into_any_element(),
            Some((id, open)) => {
                let hover = States::of(&theme).hover;
                focusable(card.id(id), &theme, false)
                    .role(Role::Link)
                    .aria_label(label)
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .on_click(move |_, window, cx| open(window, cx))
                    .into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    struct Links(Log<&'static str>);

    impl Render for Links {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let opened = self.0.clone();
            div().child(
                SettingRow::new(&theme, "Documentation")
                    .icon(Icon::BookOpen)
                    .link("docs", move |_, _| opened.borrow_mut().push("docs")),
            )
        }
    }

    #[gpui::test]
    fn a_link_row_is_a_tab_stop_that_enter_opens(cx: &mut TestAppContext) {
        let opened = log();
        let shared = opened.clone();
        let (_, cx) = cx.add_window_view(|_, _| Links(shared));
        tab(cx);
        press(cx, "enter");
        assert_eq!(*opened.borrow(), ["docs"]);
    }
}
