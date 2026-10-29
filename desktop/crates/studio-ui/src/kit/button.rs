//! The one button family: [`Button`] (a label with optional icons) and [`IconButton`] (a
//! glyph alone), after the editor's button (`routes/editor/ui.tsx:317-424`).
//!
//! Both take an [`Emphasis`]:
//!
//! | emphasis | rest | hover | pressed |
//! |---|---|---|---|
//! | `Ghost` (default) | nothing, `text_2` | `ctl_hover`, `text_1` | `ctl_active` |
//! | `Neutral` | `ctl`, `text_2` | `ctl_hover`, `text_1` | `ctl_active` |
//! | `Primary` | `accent`, `on_accent` | `accent_2` | `accent` |
//! | `Danger` | `danger_bg`, `danger` | `danger_hover` | `danger_active` |
//!
//! A selected button (`.pressed(true)`) shows `ctl_active` and `text_1`; a primary one
//! keeps its accent. Disabled buttons drop to `METRICS.opacity.disabled`.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, ElementId, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement,
    Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px, svg,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::interact::{BoundsHandler, States, bounds_probe, dim_disabled, focusable, with_tooltip};
use super::menu::OpenHandler;

/// The click handler every component takes. `cx.listener(..)` produces exactly
/// this shape for any window type, so components stay window-agnostic without
/// a generic parameter.
pub type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// How loudly a button asks to be pressed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Emphasis {
    /// A control with the subtle `ctl` fill.
    Neutral,
    /// No fill until hovered: toolbars and secondary actions.
    #[default]
    Ghost,
    /// The main action of a view, on the accent.
    Primary,
    /// A destructive action: danger text on the soft danger surface.
    Danger,
}

/// The fills of one emphasis, so the render bodies hold no per-emphasis branching.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Paint {
    rest: Option<Hsla>,
    hover: Hsla,
    active: Hsla,
    selected: Hsla,
    text: Hsla,
    hover_text: Hsla,
    selected_text: Hsla,
    disabled_text: Hsla,
}

impl Paint {
    fn of(theme: &Theme, emphasis: Emphasis) -> Self {
        let colors = theme.colors;
        let states = States::of(theme);
        let neutral = Self {
            rest: None,
            hover: states.hover,
            active: states.pressed,
            selected: states.selected,
            text: colors.text_2.into(),
            hover_text: colors.text_1.into(),
            selected_text: colors.text_1.into(),
            disabled_text: colors.text_3.into(),
        };
        match emphasis {
            Emphasis::Ghost => neutral,
            Emphasis::Neutral => Self {
                rest: Some(states.rest),
                ..neutral
            },
            Emphasis::Primary => {
                let (accent, text) = (Hsla::from(colors.accent), Hsla::from(colors.on_accent));
                Self {
                    rest: Some(accent),
                    hover: colors.accent_2.into(),
                    active: accent,
                    selected: accent,
                    text,
                    hover_text: text,
                    selected_text: text,
                    disabled_text: text,
                }
            }
            Emphasis::Danger => {
                let text = Hsla::from(colors.danger);
                Self {
                    rest: Some(colors.danger_bg.into()),
                    hover: colors.danger_hover.into(),
                    active: colors.danger_active.into(),
                    selected: colors.danger_active.into(),
                    text,
                    hover_text: text,
                    selected_text: text,
                    disabled_text: text,
                }
            }
        }
    }
}

/// A text button's height.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ButtonSize {
    /// 24 px, `Label` text: compact rows and inline actions.
    Sm,
    /// 28 px, `Body` text: the default.
    #[default]
    Md,
}

impl ButtonSize {
    fn height(self) -> f32 {
        match self {
            Self::Sm => METRICS.size.control_sm_h,
            Self::Md => METRICS.size.control_h,
        }
    }

    fn text(self) -> TextStyle {
        match self {
            Self::Sm => TextStyle::Label,
            Self::Md => TextStyle::Body,
        }
    }

    fn icon(self) -> f32 {
        match self {
            Self::Sm => METRICS.size.icon_sm,
            Self::Md => METRICS.size.icon_md,
        }
    }
}

/// A text button's horizontal padding: Cap's `px-[7px]`.
const BUTTON_PAD_X: f32 = 7.;
/// The trailing chevron of a menu trigger.
const CHEVRON: f32 = 10.;
/// Space between a button's icons and its label.
const BUTTON_GAP: f32 = 6.;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    theme: Theme,
    emphasis: Emphasis,
    size: ButtonSize,
    label: Option<SharedString>,
    left_icon: Option<SharedString>,
    right_icon: Option<SharedString>,
    /// Pushes the right icon to the end, the way select-style triggers park their chevron.
    right_icon_end: bool,
    width: Option<Pixels>,
    full_width: bool,
    disabled: bool,
    /// Selected or expanded.
    pressed: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
    on_open: Option<OpenHandler>,
    on_bounds: Option<BoundsHandler>,
}

impl Button {
    pub fn new(theme: &Theme, id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            emphasis: Emphasis::default(),
            size: ButtonSize::default(),
            label: None,
            left_icon: None,
            right_icon: None,
            right_icon_end: false,
            width: None,
            full_width: false,
            disabled: false,
            pressed: false,
            tooltip: None,
            on_click: None,
            on_open: None,
            on_bounds: None,
        }
    }

    pub fn emphasis(mut self, emphasis: Emphasis) -> Self {
        self.emphasis = emphasis;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn left_icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.left_icon = Some(icon.into());
        self
    }

    pub fn right_icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.right_icon = Some(icon.into());
        self
    }

    pub fn right_icon_end(mut self, end: bool) -> Self {
        self.right_icon_end = end;
        self
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Stretches across its parent.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    /// Shows only the icon; the label becomes the tooltip (unless one is set), which also
    /// names the button.
    pub fn icon_only(mut self) -> Self {
        if let Some(label) = self.label.take() {
            self.tooltip.get_or_insert(label);
        }
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
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
        handler: impl Fn(&gpui::Bounds<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open = Some(Box::new(handler));
        self
    }

    /// Reports the painted bounds, in window coordinates, on every prepaint.
    pub fn on_bounds(mut self, report: impl Fn(gpui::Bounds<Pixels>, &mut App) + 'static) -> Self {
        self.on_bounds = Some(Rc::new(report));
        self
    }
}

/// The group a button's glyphs hover with: gpui svgs do not inherit a parent's text
/// colour, so they follow the hover through a group rather than the cascade.
fn hover_group(prefix: &str, id: &ElementId) -> SharedString {
    match id {
        ElementId::Name(name) => SharedString::from(format!("{prefix}-{name}")),
        other => SharedString::from(format!("{prefix}-{other:?}")),
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Button {
            id,
            theme,
            emphasis,
            size,
            label,
            left_icon,
            right_icon,
            right_icon_end,
            width,
            full_width,
            disabled,
            pressed,
            tooltip,
            on_click,
            on_open,
            on_bounds,
        } = self;
        let paint = Paint::of(&theme, emphasis);
        let foreground = if disabled {
            paint.disabled_text
        } else if pressed {
            paint.selected_text
        } else {
            paint.text
        };
        let chevron = if disabled || emphasis != Emphasis::Ghost && emphasis != Emphasis::Neutral {
            foreground
        } else {
            Hsla::from(theme.colors.text_3)
        };
        let group = hover_group("button", &id);
        let icon_hover = (!disabled && !pressed).then_some(paint.hover_text);
        let name = label.clone().or_else(|| tooltip.clone());
        let height = px(size.height());

        let element = focusable(div().id(id), &theme, disabled)
            .role(Role::Button)
            .when_some(name, |this, name| this.aria_label(name))
            .group(group.clone())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .px(px(BUTTON_PAD_X))
            .gap(px(BUTTON_GAP))
            .h(height)
            .min_w(height)
            .rounded(px(METRICS.radius.control))
            .flex_shrink_0()
            .when(right_icon_end, |this| this.justify_between())
            .when_some(width, |this, width| this.w(width))
            .when(full_width, |this| this.w_full());
        dim_disabled(text_style(element, size.text()), disabled)
            .font_weight(FontWeight::MEDIUM)
            .text_color(foreground)
            .when_some(paint.rest, |this, bg| this.bg(bg))
            .when(pressed, |this| this.bg(paint.selected))
            .when(!disabled && !pressed, |this| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(paint.hover).text_color(paint.hover_text))
                    .active(move |style| style.bg(paint.active))
            })
            .children(left_icon.map(|icon| {
                let group = group.clone();
                svg()
                    .path(icon)
                    .size(px(size.icon()))
                    .flex_shrink_0()
                    .text_color(foreground)
                    .when_some(icon_hover, |this, color| {
                        this.group_hover(group, move |style| style.text_color(color))
                    })
            }))
            .children(label.map(|label| div().truncate().child(label)))
            .children(right_icon.map(|icon| {
                svg()
                    .path(icon)
                    .size(px(CHEVRON))
                    .flex_shrink_0()
                    .when(right_icon_end, |this| this.ml_auto())
                    .text_color(chevron)
            }))
            .map(|this| with_tooltip(this, &theme, tooltip))
            .when_some(on_click.filter(|_| !disabled), |this, handler| {
                this.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .when_some(on_open.filter(|_| !disabled), crate::kit::Menu::trigger)
            .children(on_bounds.map(bounds_probe))
    }
}

/// The square icon buttons.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IconButtonSize {
    /// 24 px box, 14 px glyph.
    Sm,
    /// 28 px box, 16 px glyph.
    #[default]
    Md,
    /// 32 px box, 16 px glyph.
    Lg,
}

impl IconButtonSize {
    /// The box side and the glyph size, in pixels.
    pub const fn box_and_glyph(self) -> (f32, f32) {
        let size = METRICS.size;
        match self {
            Self::Sm => (size.icon_button_sm, size.icon_sm),
            Self::Md => (size.icon_button_md, size.icon_md),
            Self::Lg => (size.icon_button_lg, size.icon_md),
        }
    }
}

/// An icon-only button. The label is the accessible name; without one the tooltip names it.
#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    theme: Theme,
    icon: SharedString,
    emphasis: Emphasis,
    size: IconButtonSize,
    round: bool,
    inverted: bool,
    pressed: bool,
    disabled: bool,
    label: Option<SharedString>,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
    on_bounds: Option<BoundsHandler>,
}

impl IconButton {
    pub fn new(theme: &Theme, id: impl Into<ElementId>, icon: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            theme: *theme,
            icon: icon.into(),
            emphasis: Emphasis::default(),
            size: IconButtonSize::default(),
            round: false,
            inverted: false,
            pressed: false,
            disabled: false,
            label: None,
            tooltip: None,
            on_click: None,
            on_bounds: None,
        }
    }

    pub fn emphasis(mut self, emphasis: Emphasis) -> Self {
        self.emphasis = emphasis;
        self
    }

    pub fn size(mut self, size: IconButtonSize) -> Self {
        self.size = size;
        self
    }

    /// A circle rather than a rounded square: the composer's send and stop.
    pub fn round(mut self) -> Self {
        self.round = true;
        self
    }

    /// The transport's play disc: a `card` glyph on `text_1`, round.
    pub fn inverted(mut self) -> Self {
        self.inverted = true;
        self.round = true;
        self
    }

    /// Selected: `ctl_active` behind a `text_1` glyph.
    pub fn pressed(mut self, pressed: bool) -> Self {
        self.pressed = pressed;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The name a screen reader announces for the button.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Shows `text` on hover after the standard delay; it also names the button
    /// when no [`label`](Self::label) does.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// Reports the painted bounds, in window coordinates, on every prepaint.
    pub fn on_bounds(mut self, report: impl Fn(gpui::Bounds<Pixels>, &mut App) + 'static) -> Self {
        self.on_bounds = Some(Rc::new(report));
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    fn paint(&self) -> Paint {
        if self.inverted {
            let colors = self.theme.colors;
            let glyph = Hsla::from(colors.card);
            return Paint {
                rest: Some(colors.text_1.into()),
                hover: colors.text_2.into(),
                active: colors.text_2.into(),
                selected: colors.text_1.into(),
                text: glyph,
                hover_text: glyph,
                selected_text: glyph,
                disabled_text: glyph,
            };
        }
        Paint::of(&self.theme, self.emphasis)
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let paint = self.paint();
        let IconButton {
            id,
            theme,
            icon,
            emphasis: _,
            size,
            round,
            inverted: _,
            pressed,
            disabled,
            label,
            tooltip,
            on_click,
            on_bounds,
        } = self;
        let name = label.or_else(|| tooltip.clone());
        let group = hover_group("icon-button", &id);
        let glyph_hover = (!disabled && !pressed).then_some(paint.hover_text);
        let (side, glyph) = size.box_and_glyph();

        let element = focusable(div().id(id), &theme, disabled)
            .group(group.clone())
            .role(Role::Button)
            .when_some(name, |this, name| this.aria_label(name))
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .size(px(side))
            .flex_shrink_0()
            .map(|this| {
                if round {
                    this.rounded_full()
                } else {
                    this.rounded(px(METRICS.radius.control))
                }
            })
            .when_some(paint.rest, |this, bg| this.bg(bg))
            .when(pressed, |this| this.bg(paint.selected));
        dim_disabled(element, disabled)
            .when(!disabled && !pressed, |this| {
                this.when(on_click.is_some(), |this| this.cursor_pointer())
                    .hover(move |style| style.bg(paint.hover))
                    .active(move |style| style.bg(paint.active))
            })
            .child(
                svg()
                    .path(icon)
                    .size(px(glyph))
                    .text_color(if pressed {
                        paint.selected_text
                    } else {
                        paint.text
                    })
                    .when_some(glyph_hover, |this, color| {
                        this.group_hover(group, move |style| style.text_color(color))
                    }),
            )
            .when_some(on_click.filter(|_| !disabled), |this, handler| {
                this.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .map(|this| with_tooltip(this, &theme, tooltip))
            .children(on_bounds.map(bounds_probe))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Bounds, Context, Render, TestAppContext, VisualTestContext};

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    type Seen = Rc<Cell<Option<Bounds<Pixels>>>>;

    /// A disabled button, an 80px one reporting its bounds, a third and an icon button,
    /// each logging its clicks.
    struct Row {
        clicks: Log<&'static str>,
        seen: Seen,
    }

    impl Render for Row {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let click = |name: &'static str| {
                let clicks = self.clicks.clone();
                move |_: &ClickEvent, _: &mut Window, _: &mut App| clicks.borrow_mut().push(name)
            };
            let seen = self.seen.clone();
            div()
                .flex()
                .w(px(400.))
                .child(
                    Button::new(&theme, "off")
                        .label("Off")
                        .disabled(true)
                        .on_click(click("off")),
                )
                .child(
                    Button::new(&theme, "on")
                        .label("On")
                        .width(px(80.))
                        .on_bounds(move |bounds, _| seen.set(Some(bounds)))
                        .on_click(click("on")),
                )
                .child(
                    Button::new(&theme, "go")
                        .label("Go")
                        .emphasis(Emphasis::Primary)
                        .on_click(click("go")),
                )
                .child(
                    IconButton::new(&theme, "icon", "icons/x.svg")
                        .label("Close")
                        .on_click(click("icon")),
                )
        }
    }

    fn row(cx: &mut TestAppContext) -> (Log<&'static str>, Seen, &mut VisualTestContext) {
        let clicks = log();
        let seen = Seen::default();
        let (shared, bounds) = (clicks.clone(), seen.clone());
        let (_, cx) = cx.add_window_view(|_, _| Row {
            clicks: shared,
            seen: bounds,
        });
        cx.run_until_parked();
        (clicks, seen, cx)
    }

    #[gpui::test]
    fn tab_skips_a_disabled_button_and_enter_or_space_activates(cx: &mut TestAppContext) {
        let (clicks, _, cx) = row(cx);
        tab(cx);
        press(cx, "enter");
        tab(cx);
        press(cx, "space");
        tab(cx);
        press(cx, "enter");
        assert_eq!(*clicks.borrow(), ["on", "go", "icon"]);
    }

    #[gpui::test]
    fn the_reported_bounds_are_the_buttons_own(cx: &mut TestAppContext) {
        let (_, seen, _) = row(cx);
        let bounds = seen.get().expect("painted");
        assert_eq!(
            (f32::from(bounds.size.width), f32::from(bounds.size.height)),
            (80., 28.)
        );
    }

    #[test]
    fn icon_only_moves_the_label_into_the_tooltip() {
        let theme = Theme::light();
        let button = Button::new(&theme, "a").label("Open…").icon_only();
        assert_eq!(button.label, None);
        assert_eq!(button.tooltip.as_deref(), Some("Open…"));

        let button = Button::new(&theme, "b")
            .label("Open…")
            .tooltip("Open a project")
            .icon_only();
        assert_eq!(button.label, None);
        assert_eq!(button.tooltip.as_deref(), Some("Open a project"));
    }

    #[test]
    fn emphases_paint_from_the_roles() {
        for theme in [Theme::light(), Theme::dark()] {
            let c = theme.colors;
            let hsla = Hsla::from;

            let ghost = Paint::of(&theme, Emphasis::Ghost);
            assert_eq!(ghost.rest, None);
            assert_eq!(ghost.hover, hsla(c.ctl_hover));
            assert_eq!(ghost.active, hsla(c.ctl_active));
            assert_eq!(ghost.selected, hsla(c.ctl_active));
            assert_eq!(
                (ghost.text, ghost.hover_text),
                (hsla(c.text_2), hsla(c.text_1))
            );

            let neutral = Paint::of(&theme, Emphasis::Neutral);
            assert_eq!(neutral.rest, Some(hsla(c.ctl)));
            assert_eq!(neutral.hover, hsla(c.ctl_hover));

            let primary = Paint::of(&theme, Emphasis::Primary);
            assert_eq!(primary.rest, Some(hsla(c.accent)));
            assert_eq!(primary.hover, hsla(c.accent_2));
            assert_eq!(primary.text, hsla(c.on_accent));

            let danger = Paint::of(&theme, Emphasis::Danger);
            assert_eq!(danger.rest, Some(hsla(c.danger_bg)));
            assert_eq!(danger.hover, hsla(c.danger_hover));
            assert_eq!(danger.active, hsla(c.danger_active));
            assert_eq!(
                (danger.text, danger.hover_text),
                (hsla(c.danger), hsla(c.danger))
            );
        }
    }

    #[test]
    fn sizes_follow_the_metrics() {
        assert_eq!(ButtonSize::Sm.height(), 24.);
        assert_eq!(ButtonSize::Md.height(), 28.);
        assert_eq!(IconButtonSize::Sm.box_and_glyph(), (24., 14.));
        assert_eq!(IconButtonSize::Md.box_and_glyph(), (28., 16.));
        assert_eq!(IconButtonSize::Lg.box_and_glyph(), (32., 16.));
    }

    #[test]
    fn the_inverted_disc_is_a_card_glyph_on_text_1() {
        let theme = Theme::dark();
        let button = IconButton::new(&theme, "play", "icons/x.svg").inverted();
        let paint = button.paint();
        assert_eq!(paint.rest, Some(Hsla::from(theme.colors.text_1)));
        assert_eq!(paint.text, Hsla::from(theme.colors.card));
        assert!(button.round);
    }

    struct Probed {
        seen: Seen,
    }

    impl Render for Probed {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::for_window(window, cx);
            let seen = self.seen.clone();
            div().w(px(300.)).child(
                IconButton::new(&theme, "probed", "icons/x.svg")
                    .size(IconButtonSize::Sm)
                    .on_bounds(move |bounds, _| seen.set(Some(bounds))),
            )
        }
    }

    #[gpui::test]
    fn an_icon_buttons_reported_bounds_are_its_own(cx: &mut TestAppContext) {
        let seen = Seen::default();
        let shared = seen.clone();
        let (_, cx) = cx.add_window_view(|_, _| Probed { seen: shared });
        cx.run_until_parked();
        let bounds = seen.get().expect("painted");
        assert_eq!(f32::from(bounds.size.width), 24.);
    }

    #[test]
    fn an_icon_button_carries_its_accessible_name_and_tooltip() {
        let theme = Theme::light();
        let button = IconButton::new(&theme, "close", "icons/x.svg")
            .label("Close")
            .tooltip("Close the panel");
        assert_eq!(button.label.as_deref(), Some("Close"));
        assert_eq!(button.tooltip.as_deref(), Some("Close the panel"));
    }
}
