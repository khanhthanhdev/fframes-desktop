//! One CLI agent on Settings → Agent → CLI agents, after Oleafly's `AgentCard`: a card whose
//! header (logo slot, name, one line of status, the readiness badge and a chevron) opens
//! the details under a hairline: the command line tool and the adapter as labelled facts,
//! the agent's own sign-in command with a copy button, and the one next step with its
//! actions.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClipboardItem, ElementId, InteractiveElement, IntoElement, ParentElement,
    Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Transformation,
    Window, div, px, radians,
};
use studio_theme::{METRICS, MONO_FAMILY, TextStyle, Theme, text_style};

use crate::kit::{
    Badge, Handler, Icon, IconButton, IconButtonSize, Tone, focusable, icon, on_activate,
    selectable, setting_card,
};

/// Corner of the card, so the header's focus ring follows it.
const CARD_RADIUS: Pixels = px(METRICS.radius.card);
/// Corner of the sign-in command box.
const CONTROL_RADIUS: Pixels = px(METRICS.radius.control);

/// One labelled fact in the details: the adapter's executable, the protocol, …
pub struct AgentFact {
    pub label: SharedString,
    pub value: SharedString,
    /// A path or a command: monospaced, breaking anywhere.
    pub code: bool,
}

impl AgentFact {
    pub fn text(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            code: false,
        }
    }

    pub fn code(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            code: true,
        }
    }
}

#[derive(IntoElement)]
pub struct AgentCard {
    theme: Theme,
    id: SharedString,
    name: SharedString,
    summary: SharedString,
    badge: Option<(SharedString, Tone)>,
    in_use: bool,
    open: bool,
    on_toggle: Option<Handler>,
    facts: Vec<AgentFact>,
    sign_in: Option<SharedString>,
    content: Vec<AnyElement>,
    next_step: Option<SharedString>,
    actions: Vec<AnyElement>,
    footer: Vec<AnyElement>,
}

impl AgentCard {
    /// `id` names the card's elements (`agent-card-{id}`, …); `summary` is the one line
    /// under the name; `badge` the readiness on the right.
    pub fn new(
        theme: &Theme,
        id: impl Into<SharedString>,
        name: impl Into<SharedString>,
        summary: impl Into<SharedString>,
        badge: (impl Into<SharedString>, Tone),
    ) -> Self {
        Self {
            theme: *theme,
            id: id.into(),
            name: name.into(),
            summary: summary.into(),
            badge: Some((badge.0.into(), badge.1)),
            in_use: false,
            open: false,
            on_toggle: None,
            facts: Vec::new(),
            sign_in: None,
            content: Vec::new(),
            next_step: None,
            actions: Vec::new(),
            footer: Vec::new(),
        }
    }

    /// No readiness badge: a card that is not an agent (Register a custom agent).
    pub fn without_badge(mut self) -> Self {
        self.badge = None;
        self
    }

    /// Free content in the details, after the facts and the sign-in command (a form).
    pub fn content(mut self, element: impl IntoElement) -> Self {
        self.content.push(element.into_any_element());
        self
    }

    /// The agent new tasks start with.
    pub fn in_use(mut self, in_use: bool) -> Self {
        self.in_use = in_use;
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    pub fn facts(mut self, facts: impl IntoIterator<Item = AgentFact>) -> Self {
        self.facts.extend(facts);
        self
    }

    /// The command that signs the agent in, in its own tool.
    pub fn sign_in(mut self, command: impl Into<SharedString>) -> Self {
        self.sign_in = Some(command.into());
        self
    }

    /// What to do next, above the actions.
    pub fn next_step(mut self, text: impl Into<SharedString>) -> Self {
        self.next_step = Some(text.into());
        self
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// Lines under everything else (the last launch's containment, a hint).
    pub fn footer(mut self, element: impl IntoElement) -> Self {
        self.footer.push(element.into_any_element());
        self
    }
}

fn overline(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    let text: SharedString = text.into();
    text_style(div(), TextStyle::Overline)
        .text_color(theme.colors.text_3)
        .child(SharedString::from(text.to_uppercase()))
}

impl RenderOnce for AgentCard {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let colors = theme.colors;
        let id = self.id.clone();
        let chevron = icon(Icon::ChevronRight, METRICS.size.icon_sm, colors.text_2)
            .with_transformation(Transformation::rotate(radians(if self.open {
                std::f32::consts::FRAC_PI_2
            } else {
                0.
            })));
        let mut title = div().flex().items_center().gap(px(METRICS.space_2)).child(
            text_style(div(), TextStyle::BodyMedium)
                .text_color(colors.text_1)
                .child(self.name.clone()),
        );
        if self.in_use {
            title = title.child(Badge::new("In use").tone(Tone::Info));
        }
        let header = div()
            .id(ElementId::Name(format!("agent-card-{id}").into()))
            .role(Role::Button)
            .aria_label(format!("{} details", self.name))
            .flex()
            .items_start()
            .gap(px(METRICS.space_2))
            .p(px(METRICS.space_3))
            .rounded(CARD_RADIUS)
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_1))
                    .child(title)
                    .child(
                        text_style(div(), TextStyle::Small)
                            .text_color(colors.text_2)
                            .child(self.summary.clone()),
                    ),
            )
            .children(self.badge.map(|(text, tone)| Badge::new(text).tone(tone)))
            .child(chevron);
        let header = on_activate(focusable(header, &theme, false), false, self.on_toggle);
        let mut card = setting_card(&theme).p_0().flex().flex_col().child(header);
        if !self.open {
            return card;
        }
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_4))
            .px(px(METRICS.space_4))
            .py(px(METRICS.space_4))
            .border_t(px(METRICS.size.divider))
            .border_color(colors.line);
        if !self.facts.is_empty() {
            let facts = self.facts.into_iter().map(|fact| {
                let value = text_style(div(), TextStyle::Small).text_color(colors.text_1);
                let value = if fact.code {
                    value.font_family(MONO_FAMILY)
                } else {
                    value
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_1))
                    .child(overline(&theme, fact.label))
                    .child(value.child(selectable(fact.value)))
            });
            body = body.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x(px(METRICS.space_6))
                    .gap_y(px(METRICS.space_3))
                    .children(facts),
            );
        }
        if let Some(command) = self.sign_in {
            let copied = command.clone();
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_2))
                    .child(overline(&theme, "Sign-in command"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(METRICS.space_2))
                            .px(px(METRICS.space_3))
                            .py(px(METRICS.space_2))
                            .rounded(CONTROL_RADIUS)
                            .border(px(METRICS.size.divider))
                            .border_color(colors.line)
                            .bg(colors.window)
                            .child(
                                text_style(div(), TextStyle::Small)
                                    .flex_1()
                                    .min_w_0()
                                    .font_family(MONO_FAMILY)
                                    .text_color(colors.text_1)
                                    .child(selectable(command)),
                            )
                            .child(
                                IconButton::new(
                                    &theme,
                                    ElementId::Name(format!("agent-copy-sign-in-{id}").into()),
                                    Icon::Copy.path(),
                                )
                                .size(IconButtonSize::Sm)
                                .label("Copy sign-in command")
                                .tooltip("Copy sign-in command")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copied.to_string(),
                                    ));
                                }),
                            ),
                    ),
            );
        }
        body = body.children(self.content);
        if self.next_step.is_some() || !self.actions.is_empty() {
            let mut next = div()
                .flex()
                .flex_col()
                .gap(px(METRICS.space_2))
                .pt(px(METRICS.space_4))
                .border_t(px(METRICS.size.divider))
                .border_color(colors.line)
                .child(overline(&theme, "Next step"));
            if let Some(step) = self.next_step {
                next = next.child(
                    text_style(div(), TextStyle::Small)
                        .text_color(colors.text_1)
                        .child(selectable(step)),
                );
            }
            if !self.actions.is_empty() {
                next = next.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(METRICS.space_2))
                        .children(self.actions),
                );
            }
            body = body.child(next);
        }
        if !self.footer.is_empty() {
            body = body.child(
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_2)
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_1))
                    .children(self.footer),
            );
        }
        card = card.child(body);
        card
    }
}
