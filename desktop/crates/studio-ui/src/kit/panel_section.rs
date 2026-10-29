//! `PanelSection`: a titled group of content in a panel, tab or settings page.
//!
//! The phase 3 `Section` of the design-system plan, named `PanelSection` while the older
//! `surface::Section` (the settings window's `<Section>`) still exists; phase 7 moves the
//! settings pages here, deletes `surface::Section`, `titled_group` and `text::section_title`,
//! and may rename this one to `Section`.
//!
//! The title is `Label` in `text_2` with an optional description under it and an optional
//! trailing element (a count, an action) at the end of the title row; the content follows
//! 8 px below. Sections stacked in a column sit 24 px apart ([`PanelSection::column`]).
//! [`PanelSection::card`] puts the content on a nested card (`card_2`), as settings pages do.

use gpui::{
    AnyElement, App, Div, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window,
    div, prelude::FluentBuilder, px,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::selectable;

#[derive(IntoElement)]
pub struct PanelSection {
    theme: Theme,
    title: SharedString,
    description: Option<SharedString>,
    trailing: Option<AnyElement>,
    children: Vec<AnyElement>,
    card: bool,
    grow: bool,
}

impl PanelSection {
    pub fn new(theme: &Theme, title: impl Into<SharedString>) -> Self {
        Self {
            theme: *theme,
            title: title.into(),
            description: None,
            trailing: None,
            children: Vec::new(),
            card: false,
            grow: false,
        }
    }

    /// Takes the height its column leaves and bounds the content to it, so a scrolling list
    /// inside scrolls instead of overflowing.
    pub fn grow(mut self) -> Self {
        self.grow = true;
        self
    }

    /// A column for stacking sections, 24 px apart.
    pub fn column() -> Div {
        div().flex().flex_col().gap(px(METRICS.space_6))
    }

    /// A line under the title (`Small`, `text_3`).
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// An element at the end of the title row: a count, an action.
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    /// Puts the content on a nested card (`card_2`, `line` border), as settings pages do.
    pub fn card(mut self) -> Self {
        self.card = true;
        self
    }

    pub fn child(mut self, child: impl IntoElement) -> Self {
        self.children.push(child.into_any_element());
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.children
            .extend(children.into_iter().map(IntoElement::into_any_element));
        self
    }
}

impl RenderOnce for PanelSection {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = self.theme.colors;
        let header = div()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(METRICS.space_2))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        text_style(div(), TextStyle::Label)
                            .text_color(colors.text_2)
                            .child(self.title),
                    )
                    .children(self.description.map(|description| {
                        text_style(div(), TextStyle::Small)
                            .text_color(colors.text_3)
                            .child(selectable(description))
                    })),
            )
            .children(
                self.trailing
                    .map(|trailing| div().flex().flex_none().items_center().child(trailing)),
            );

        let content = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .children(self.children)
            .when(self.grow, |this| this.flex_1().min_h_0())
            .when(self.card, |this| {
                this.p(px(METRICS.space_3))
                    .rounded(px(METRICS.radius.card))
                    .border_1()
                    .border_color(colors.line)
                    .bg(colors.card_2)
            });

        div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .when(self.grow, |this| this.flex_1().min_h_0())
            .child(header)
            .child(content)
    }
}
