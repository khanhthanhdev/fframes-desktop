//! `SplitButton`: a primary action joined to the trigger of a menu of related choices, after
//! Oleafly's Recompile control (`CompileControls.tsx`, a `ButtonGroup` of two primary
//! buttons parted by a white rule at 25 %).
//!
//! The action segment is a Primary button with a glyph and a label (or the glyph alone, the
//! label then its tooltip); the menu segment is a chevron. Both are on the accent and meet
//! at the rule, so they read as one control. The caller owns the menu: the chevron reports
//! the whole control's bounds, which the menu opens against.

use std::{cell::Cell, rc::Rc};

use gpui::{
    App, Bounds, ClickEvent, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled,
    Window, canvas, div, prelude::FluentBuilder, px, svg,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::button::ClickHandler;
use super::icon::Icon;
use super::interact::{BoundsHandler, bounds_probe, dim_disabled, focusable, with_tooltip};
use super::menu::OpenHandler;

/// The rule between the segments: `on_accent` at 25 % (Oleafly's `border-white/25`).
const RULE_ALPHA: f32 = 0.25;

#[derive(IntoElement)]
pub struct SplitButton {
    id: ElementId,
    menu_id: ElementId,
    theme: Theme,
    label: SharedString,
    icon: Option<SharedString>,
    menu_label: SharedString,
    icon_only: bool,
    disabled: bool,
    menu_disabled: bool,
    open: bool,
    on_click: Option<ClickHandler>,
    on_open: Option<OpenHandler>,
    on_bounds: Option<BoundsHandler>,
}

impl SplitButton {
    /// `id` names the action segment, `menu_id` the chevron; `label` is the action's text
    /// and accessible name, `menu_label` the chevron's and its tooltip.
    pub fn new(
        theme: &Theme,
        id: impl Into<ElementId>,
        menu_id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        menu_label: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            menu_id: menu_id.into(),
            theme: *theme,
            label: label.into(),
            icon: None,
            menu_label: menu_label.into(),
            icon_only: false,
            disabled: false,
            menu_disabled: false,
            open: false,
            on_click: None,
            on_open: None,
            on_bounds: None,
        }
    }

    pub fn icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Shows only the glyph in the action segment; the label becomes its tooltip.
    pub fn icon_only(mut self, icon_only: bool) -> Self {
        self.icon_only = icon_only;
        self
    }

    /// The action cannot be pressed. The chevron keeps its own state, so a menu holding
    /// choices that do not need the action stays reachable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The chevron cannot be pressed.
    pub fn menu_disabled(mut self, disabled: bool) -> Self {
        self.menu_disabled = disabled;
        self
    }

    /// The menu is open: the chevron stays on the hover shade.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// The chevron was pressed; the handler receives the whole control's bounds, in window
    /// coordinates, to anchor the menu.
    pub fn on_open(
        mut self,
        handler: impl Fn(&Bounds<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open = Some(Box::new(handler));
        self
    }

    /// Reports the action segment's painted bounds on every prepaint.
    pub fn on_bounds(mut self, report: impl Fn(Bounds<Pixels>, &mut App) + 'static) -> Self {
        self.on_bounds = Some(Rc::new(report));
        self
    }
}

impl RenderOnce for SplitButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let SplitButton {
            id,
            menu_id,
            theme,
            label,
            icon,
            menu_label,
            icon_only,
            disabled,
            menu_disabled,
            open,
            on_click,
            on_open,
            on_bounds,
        } = self;
        let colors = theme.colors;
        let text = Hsla::from(colors.on_accent);
        let hover = Hsla::from(colors.accent_2);
        let height = METRICS.size.control_h;
        let radius = px(METRICS.radius.control);
        let measured = Rc::new(Cell::new(None::<Bounds<Pixels>>));

        let action = focusable(div().id(id), &theme, disabled)
            .role(Role::Button)
            .aria_label(label.clone())
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(METRICS.space_2))
            .h_full()
            .when(icon_only, |this| this.w(px(height)))
            .when(!icon_only, |this| this.px(px(METRICS.space_3)))
            .rounded_l(radius)
            .bg(colors.accent)
            .map(|this| dim_disabled(this, disabled))
            .when(!disabled, |this| {
                this.cursor_pointer().hover(move |style| style.bg(hover))
            })
            .children(icon.map(|icon| {
                svg()
                    .path(icon)
                    .size(px(METRICS.size.icon_md))
                    .flex_shrink_0()
                    .text_color(text)
            }))
            .when(!icon_only, |this| {
                this.child(
                    text_style(div(), TextStyle::Label)
                        .font_weight(FontWeight::MEDIUM)
                        .whitespace_nowrap()
                        .child(label.clone()),
                )
            })
            .map(|this| with_tooltip(this, &theme, icon_only.then_some(label)))
            .when_some(on_click.filter(|_| !disabled), |this, handler| {
                this.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .children(on_bounds.map(bounds_probe));

        let anchor = measured.clone();
        let chevron = focusable(div().id(menu_id), &theme, menu_disabled)
            .role(Role::Button)
            .aria_label(menu_label.clone())
            .flex()
            .items_center()
            .justify_center()
            .h_full()
            .w(px(METRICS.size.icon_button_md))
            .rounded_r(radius)
            .border_l(px(METRICS.size.divider))
            .border_color(text.opacity(RULE_ALPHA))
            .bg(if open { hover } else { colors.accent.into() })
            .map(|this| dim_disabled(this, menu_disabled))
            .when(!menu_disabled, |this| {
                this.cursor_pointer().hover(move |style| style.bg(hover))
            })
            .child(
                svg()
                    .path(Icon::ChevronDown.path())
                    .size(px(METRICS.size.icon_sm))
                    .text_color(text),
            )
            .map(|this| with_tooltip(this, &theme, Some(menu_label)))
            .when_some(on_open.filter(|_| !menu_disabled), |this, handler| {
                this.on_click(move |_, window, cx| {
                    if let Some(bounds) = anchor.get() {
                        handler(&bounds, window, cx);
                    }
                })
            });

        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(px(height))
            .text_color(text)
            .child(action)
            .child(chevron)
            .child(
                canvas(
                    move |bounds, _, _| measured.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
    }
}
