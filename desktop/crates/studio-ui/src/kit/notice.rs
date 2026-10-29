//! Inline notice: a message with a tone icon and surface, optional actions, collapsible
//! details and a dismiss button. Screen readers announce it as an alert.

use std::{rc::Rc, time::Instant};

use gpui::{
    AnyElement, App, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    Rgba, Role, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::badge::{Tone, tone_color, tone_surface};
use super::button::{IconButton, IconButtonSize};
use super::collapsible::{Collapsible, CollapsibleState, Disclosure};
use super::icon::{Icon, icon};
use super::selectable;
use super::surface::Card;

/// Tallest the expanded details grow before they scroll, so a long error
/// never pushes the rest of the window down.
pub const DETAILS_MAX_H: f32 = 160.;

/// Glyph for a tone.
pub fn tone_icon(tone: Tone) -> Icon {
    match tone {
        Tone::Neutral | Tone::Info => Icon::Info,
        Tone::Success => Icon::CircleCheck,
        Tone::Warning | Tone::Danger => Icon::AlertTriangle,
    }
}

/// Fill of the notice card: the status surfaces, the plain card otherwise.
pub fn notice_surface(theme: &Theme, tone: Tone) -> Rgba {
    match tone {
        Tone::Neutral | Tone::Info => theme.colors.card,
        tone => tone_surface(theme, tone),
    }
}

type DismissHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Notice {
    id: ElementId,
    tone: Tone,
    title: SharedString,
    body: Option<SharedString>,
    details: Option<SharedString>,
    actions: Vec<AnyElement>,
    on_dismiss: Option<DismissHandler>,
}

impl Notice {
    pub fn new(id: impl Into<ElementId>, tone: Tone, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            tone,
            title: title.into(),
            body: None,
            details: None,
            actions: Vec::new(),
            on_dismiss: None,
        }
    }

    pub fn body(mut self, text: impl Into<SharedString>) -> Self {
        self.body = Some(text.into());
        self
    }

    /// Longer text shown under a collapsible "Details" header.
    pub fn details(mut self, text: impl Into<SharedString>) -> Self {
        self.details = Some(text.into());
        self
    }

    pub fn action(mut self, button: impl IntoElement) -> Self {
        self.actions.push(button.into_any_element());
        self
    }

    pub fn on_dismiss(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Notice {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = Theme::for_window(window, cx);
        let colors = theme.colors;
        let accent = tone_color(&theme, self.tone);

        let details = self.details.map(|text| {
            // The entity, not a clone of its value: `CollapsibleState` keeps
            // its open flag in a `Cell`, so a clone would flip a copy.
            let entity =
                window.use_keyed_state((self.id.clone(), "details".to_string()), cx, |_, _| {
                    CollapsibleState::new(false)
                });
            let state = entity.read(cx);
            let (height, animating) = state.height_for(Instant::now());
            if animating {
                window.request_animation_frame();
            }
            let (open, measure) = (state.is_open(), state.measure_cell());
            div()
                .flex()
                .flex_col()
                .child(
                    Disclosure::new((self.id.clone(), "details-header".to_string()), open)
                        .child(
                            text_style(div(), TextStyle::Label)
                                .text_color(colors.text_2)
                                .child("Details"),
                        )
                        .on_toggle(move |open, window, cx| {
                            entity.read(cx).set_open(*open);
                            window.refresh();
                        }),
                )
                .child(
                    div().debug_selector(|| "notice-details".into()).child(
                        Collapsible::new(height, measure).content(
                            text_style(
                                div().id((self.id.clone(), "details-text".to_string())),
                                TextStyle::Small,
                            )
                            .mt(px(METRICS.space_1))
                            .max_h(px(DETAILS_MAX_H))
                            .overflow_y_scroll()
                            .text_color(colors.text_2)
                            .child(selectable(text)),
                        ),
                    ),
                )
        });

        let dismiss = self.on_dismiss.map(|handler| {
            IconButton::new(
                &theme,
                (self.id.clone(), "dismiss".to_string()),
                Icon::X.path(),
            )
            .size(IconButtonSize::Sm)
            .label("Dismiss")
            .on_click(move |_, window, cx| handler(window, cx))
        });

        let body = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(METRICS.space_1))
            .child(
                text_style(div(), TextStyle::BodyStrong)
                    .text_color(colors.text_1)
                    .child(selectable(self.title)),
            )
            .children(self.body.map(|body| {
                text_style(div(), TextStyle::Body)
                    .text_color(colors.text_2)
                    .child(selectable(body))
            }))
            .children(details)
            .children((!self.actions.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(METRICS.space_2))
                    .pt(px(METRICS.space_1))
                    .children(self.actions)
            }));

        Card::raised(&theme)
            .id(self.id)
            .role(Role::Alert)
            .flex()
            .w_full()
            .overflow_hidden()
            .bg(notice_surface(&theme, self.tone))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(METRICS.space_2))
                    .p(px(METRICS.space_3))
                    .child(icon(
                        tone_icon(self.tone),
                        METRICS.size.icon_md,
                        Hsla::from(accent),
                    ))
                    .child(body)
                    .children(dismiss),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Render, TestAppContext};
    use studio_theme::Appearance;

    use super::*;
    use crate::kit::test_support::{Log, log, press, tab};

    #[test]
    fn surfaces_and_icons_follow_tone() {
        let theme = Theme::new(Appearance::Light);
        assert_eq!(notice_surface(&theme, Tone::Info), theme.colors.card);
        assert_eq!(notice_surface(&theme, Tone::Neutral), theme.colors.card);
        assert_eq!(
            notice_surface(&theme, Tone::Success),
            theme.colors.success_bg
        );
        assert_eq!(notice_surface(&theme, Tone::Danger), theme.colors.danger_bg);
        assert_eq!(tone_icon(Tone::Info), Icon::Info);
        assert_eq!(tone_icon(Tone::Success), Icon::CircleCheck);
        assert_eq!(tone_icon(Tone::Warning), Icon::AlertTriangle);
    }

    struct Alerting {
        dismissed: Log<()>,
    }

    impl Render for Alerting {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let dismissed = self.dismissed.clone();
            Notice::new("build-failed", Tone::Danger, "Build failed")
                .body("The scene did not compile.")
                .details("error[E0425]: cannot find value `frame`")
                .on_dismiss(move |_, _| dismissed.borrow_mut().push(()))
        }
    }

    /// The details' clipped height once any transition has settled.
    fn settled_details_height(cx: &mut gpui::VisualTestContext) -> f32 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        f32::from(
            cx.debug_bounds("notice-details")
                .expect("details drawn")
                .size
                .height,
        )
    }

    #[gpui::test]
    fn enter_on_the_details_header_opens_and_closes_them(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Alerting { dismissed: log() });
        assert_eq!(settled_details_height(cx), 0.);
        tab(cx);
        press(cx, "enter");
        assert!(settled_details_height(cx) > 0.);
        press(cx, "enter");
        assert_eq!(settled_details_height(cx), 0.);
    }

    #[gpui::test]
    fn the_details_header_then_dismiss_take_focus_in_order(cx: &mut TestAppContext) {
        let dismissed = log();
        let shared = dismissed.clone();
        let (_, cx) = cx.add_window_view(|_, _| Alerting { dismissed: shared });
        tab(cx);
        press(cx, "enter");
        assert!(
            dismissed.borrow().is_empty(),
            "the first stop is the details header"
        );
        tab(cx);
        press(cx, "enter");
        assert_eq!(dismissed.borrow().len(), 1);
    }
}
