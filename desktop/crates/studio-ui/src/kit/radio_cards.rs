//! `RadioCards` -- the "radio button as a full-width bordered card" idiom the
//! cursor settings use twice (`ConfigSidebar.tsx:835-861` for Cursor Type,
//! `:919-945` for Cursor Movement Style) and the screenshot editor's background
//! popover uses again.
//!
//! It is one shape with two instances in the Solid app and no shared component,
//! which is exactly the kind of thing this library exists to hold: a card
//! (`rounded-lg border border-gray-3`, selected `border-blue-8 bg-blue-3/40`)
//! containing a `size-4` radio dot (`border-gray-7`, selected `border-blue-9
//! bg-blue-9`) and a title/description block.
//!
//! Every card is a tab stop; Enter or Space selects it.

use gpui::{
    App, ElementId, InteractiveElement, IntoElement, ParentElement, RenderOnce, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, svg,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::interact::{States, bounds_probe, dim_disabled, focusable};

#[derive(Debug, Clone)]
pub struct RadioCard {
    pub label: SharedString,
    pub description: Option<SharedString>,
    pub disabled: bool,
}

impl RadioCard {
    pub fn new(label: impl Into<SharedString>, description: Option<&str>) -> Self {
        Self {
            label: label.into(),
            description: description.map(SharedString::from),
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// The index arrives by reference, like the other index-dispatching components,
/// so `cx.listener(..)` builds the handler directly.
type SelectHandler = Box<dyn Fn(&usize, &mut Window, &mut App) + 'static>;
/// Receives a card's index and its bounds on every prepaint.
type OptionBoundsHandler = std::rc::Rc<dyn Fn(usize, gpui::Bounds<gpui::Pixels>, &mut App)>;

#[derive(IntoElement)]
pub struct RadioCards {
    id: ElementId,
    theme: Theme,
    items: Vec<RadioCard>,
    selected: Option<usize>,
    on_select: Option<SelectHandler>,
    on_option_bounds: Option<OptionBoundsHandler>,
}

/// The radio dot and the check inside it.
const DOT: f32 = 16.;
const CHECK: f32 = 12.;

impl RadioCards {
    /// Cards behind a `line` hairline; the selected one is outlined in `accent` on
    /// `ctl_active`, its dot filled with the accent.
    pub fn new(
        theme: &Theme,
        id: impl Into<ElementId>,
        items: Vec<RadioCard>,
        selected: Option<usize>,
    ) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            items,
            selected,
            on_select: None,
            on_option_bounds: None,
        }
    }

    pub fn on_select(mut self, handler: impl Fn(&usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }

    /// Reports each card's index and bounds, for controls that are measured.
    pub fn on_option_bounds(
        mut self,
        report: impl Fn(usize, gpui::Bounds<gpui::Pixels>, &mut App) + 'static,
    ) -> Self {
        self.on_option_bounds = Some(std::rc::Rc::new(report));
        self
    }
}

impl RenderOnce for RadioCards {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let RadioCards {
            id,
            theme,
            items,
            selected,
            on_select,
            on_option_bounds,
        } = self;
        let colors = theme.colors;
        let states = States::of(&theme);

        let prefix: SharedString = match &id {
            ElementId::Name(name) => name.clone(),
            other => SharedString::from(format!("{other:?}")),
        };
        let handler: Option<std::rc::Rc<SelectHandler>> = on_select.map(std::rc::Rc::new);

        div()
            .id(id)
            .role(Role::RadioGroup)
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .children(items.into_iter().enumerate().map(|(index, item)| {
                let handler = handler.clone();
                let checked = selected == Some(index);

                let card = focusable(
                    div().id(SharedString::from(format!("{prefix}-{index}"))),
                    &theme,
                    item.disabled,
                )
                .role(Role::RadioButton)
                .aria_selected(checked)
                .aria_label(item.label.clone())
                .relative()
                .children(on_option_bounds.clone().map(|report| {
                    bounds_probe(std::rc::Rc::new(move |bounds, cx: &mut App| {
                        report(index, bounds, cx)
                    }))
                }))
                .rounded(px(METRICS.radius.segment))
                .border_1()
                .border_color(if checked { colors.accent } else { colors.line })
                .when(checked, |this| this.bg(colors.info_bg))
                .when(!checked && !item.disabled, |this| {
                    this.cursor_pointer()
                        .hover(move |style| style.bg(states.hover))
                });
                dim_disabled(card, item.disabled)
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(METRICS.space_3))
                            .p(px(METRICS.space_3))
                            .child(
                                div()
                                    .mt(px(METRICS.space_1 / 2.))
                                    .size(px(DOT))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(if checked {
                                        colors.accent
                                    } else {
                                        colors.line_strong
                                    })
                                    .when(checked, |this| {
                                        this.bg(colors.accent).child(
                                            svg()
                                                .path("icons/check.svg")
                                                .size(px(CHECK))
                                                .text_color(colors.on_accent),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .min_w_0()
                                    .child(
                                        text_style(div(), TextStyle::Title)
                                            .text_color(colors.text_1)
                                            .child(item.label),
                                    )
                                    .children(item.description.map(|text| {
                                        text_style(div(), TextStyle::Label)
                                            .font_weight(TextStyle::Body.weight())
                                            .text_color(colors.text_2)
                                            .child(text)
                                    })),
                            ),
                    )
                    .when_some(handler.filter(|_| !item.disabled), |this, handler| {
                        this.on_click(move |_, window, cx| handler(&index, window, cx))
                    })
            }))
    }
}
