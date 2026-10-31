//! Everything that waits for the user (permission requests, questions, the outside-file
//! prompt, held and refused messages) and the transcript's errors, as one card: the kit
//! `Notice`'s tone icon and surface with room for content and the actions on the right.
//! An open permission request and an agent question each have their own presentation
//! ([`ActionCard::permission`], [`ActionCard::question`]); every other card is a notice.

use gpui::{
    AnyElement, App, ClipboardItem, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::CARD_RADIUS;
use crate::kit::{
    Button, ButtonSize, Card, Emphasis, Icon, Tone, icon, notice_surface, selectable, tone_color,
    tone_icon,
};

/// The permission request after Oleafly's `PermissionCard` in its `AiChrome`: a 2 px ring
/// at a 14 px corner around the 10 px card, 14 px inside, 12 px between the parts.
///
/// The ring is the accent, drawn still. Oleafly's moving six-colour spectrum has no colours
/// in the theme yet, and animating it would redraw the whole window every frame for as long
/// as a request is open; a still ring is also what reduced motion asks for.
const PERMISSION_RING: f32 = 2.;
const PERMISSION_OUTER_RADIUS: f32 = 14.;
const PERMISSION_PAD: f32 = 14.;
const PERMISSION_GAP: f32 = 12.;
/// The 20 px shield in its 28 px slot, 10 px before the title.
const PERMISSION_GLYPH: f32 = 20.;
const PERMISSION_GLYPH_SLOT: f32 = 28.;
const PERMISSION_HEAD_GAP: f32 = 10.;
/// Space between the hairline and the actions under it.
const PERMISSION_ACTIONS_TOP: f32 = 10.;

/// How a card presents itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Presentation {
    /// The tone's icon and soft surface: errors, validation, held and refused messages.
    Notice,
    /// An open permission request: ringed, the shield, the actions under a hairline.
    Permission,
    /// An agent question: a neutral card in 12 px type.
    Question,
}

/// `Warning` when the agent is blocked on the user, `Info` when it only informs, `Danger`
/// for errors. Title `BodyStrong`, body `Small` `text_2` that wraps and never truncates,
/// then any content (radio cards, a field), then the actions right-aligned and wrapping.
#[derive(IntoElement)]
pub struct ActionCard {
    theme: Theme,
    id: ElementId,
    tone: Tone,
    presentation: Presentation,
    title: SharedString,
    body: Option<SharedString>,
    content: Vec<AnyElement>,
    actions: Vec<AnyElement>,
    copy: Option<SharedString>,
}

impl ActionCard {
    pub fn new(
        theme: &Theme,
        id: impl Into<ElementId>,
        tone: Tone,
        title: impl Into<SharedString>,
    ) -> Self {
        Self {
            theme: *theme,
            id: id.into(),
            tone,
            presentation: Presentation::Notice,
            title: title.into(),
            body: None,
            content: Vec::new(),
            actions: Vec::new(),
            copy: None,
        }
    }

    /// Presents an open permission request, the one card that stops the agent until the
    /// user picks an option: a 2 px accent ring, the shield in the tone's colour, title
    /// `BodyStrong` over the `Small` body, and the actions under a hairline. Use it for
    /// nothing else, so the ring keeps meaning "the agent waits for your permission".
    pub fn permission(mut self) -> Self {
        self.presentation = Presentation::Permission;
        self
    }

    /// Presents an agent question: the neutral card (`card` fill, `line` border, no tone
    /// wash), 12 px inside, a `Label` title after the tone's icon.
    pub fn question(mut self) -> Self {
        self.presentation = Presentation::Question;
        self
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// An element under the body: the options of a question, a text field.
    pub fn child(mut self, element: impl IntoElement) -> Self {
        self.content.push(element.into_any_element());
        self
    }

    /// A button in the right-aligned row at the bottom; the primary one goes last.
    pub fn action(mut self, button: impl IntoElement) -> Self {
        self.actions.push(button.into_any_element());
        self
    }

    /// A Copy button in the title row that puts `text` on the clipboard (errors).
    pub fn copy(mut self, text: impl Into<SharedString>) -> Self {
        self.copy = Some(text.into());
        self
    }
}

impl RenderOnce for ActionCard {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ActionCard {
            theme,
            id,
            tone,
            presentation,
            title,
            body,
            content,
            actions,
            copy,
        } = self;
        let colors = theme.colors;
        let accent = tone_color(&theme, tone);
        let copy = copy.map(|text| {
            Button::new(
                &theme,
                ElementId::from(SharedString::from(format!("{id}-copy"))),
            )
            .emphasis(Emphasis::Ghost)
            .size(ButtonSize::Sm)
            .label("Copy")
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()))
            })
        });
        let body = body.map(|body| {
            text_style(div(), TextStyle::Small)
                .text_color(colors.text_2)
                .child(selectable(body))
        });
        let action_row = (!actions.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(px(METRICS.space_2))
                .children(actions)
        });

        if presentation == Presentation::Permission {
            let head = div()
                .flex()
                .items_start()
                .gap(px(PERMISSION_HEAD_GAP))
                .child(
                    div()
                        .size(px(PERMISSION_GLYPH_SLOT))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(Icon::ShieldQuestion, PERMISSION_GLYPH, accent)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(METRICS.space_1))
                        .child(
                            text_style(div(), TextStyle::BodyStrong)
                                .text_color(colors.text_1)
                                .child(selectable(title)),
                        )
                        .children(body),
                )
                .children(copy);
            let card = div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(PERMISSION_GAP))
                .p(px(PERMISSION_PAD))
                .rounded(CARD_RADIUS)
                .bg(colors.card)
                .child(head)
                .children(content)
                .children(action_row.map(|row| {
                    row.border_t_1()
                        .border_color(colors.line)
                        .pt(px(PERMISSION_ACTIONS_TOP))
                }));
            let outer_radius = px(PERMISSION_OUTER_RADIUS);
            return div()
                .id(id)
                .role(Role::Alert)
                .w_full()
                .flex_none()
                .rounded(outer_radius)
                .bg(colors.accent)
                .p(px(PERMISSION_RING))
                .child(card)
                .into_any_element();
        }

        let (title_style, surface) = match presentation {
            Presentation::Question => (TextStyle::Label, colors.card),
            _ => (TextStyle::BodyStrong, notice_surface(&theme, tone)),
        };
        let column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(METRICS.space_2))
            .p(px(METRICS.space_3))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(METRICS.space_2))
                    .child(
                        div()
                            .h(px(title_style.line_height()))
                            .flex()
                            .items_center()
                            .child(icon(tone_icon(tone), METRICS.size.icon_sm, accent)),
                    )
                    .child(
                        text_style(div().flex_1().min_w_0(), title_style)
                            .text_color(colors.text_1)
                            .child(selectable(title)),
                    )
                    .children(copy),
            )
            .children(body)
            .children(content)
            .children(action_row);
        Card::raised(&theme)
            .id(id)
            .role(Role::Alert)
            .flex()
            .w_full()
            .overflow_hidden()
            .bg(surface)
            .child(column)
            .into_any_element()
    }
}
