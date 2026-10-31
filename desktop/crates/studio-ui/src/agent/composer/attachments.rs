//! Attachment chips above the input.
//!
//! The composer validates nothing on disk and reads no file. The owner turns an
//! [`AttachRequest`](super::AttachRequest) into [`AttachmentDraft`]s after its
//! own checks (project confinement, the deny list, image capability and size),
//! and the chips only show what it accepted.

use std::{rc::Rc, sync::Arc};

use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, Window, div, img, prelude::*, px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::metrics::{
    CONTROL_CORNER, FILE_CHIP_GAP, FILE_CHIP_NAME_MAX_W, FILE_CHIP_PAD_LEFT, FILE_CHIP_PAD_RIGHT,
    STRIP_GAP, STRIP_PAD_X, STRIP_THUMB,
};
use crate::kit::{Chip, Emphasis, Icon, IconButton, IconButtonSize, Tone, icon};

/// Identifies a chip. Ids only grow within one composer, so a stale id from a
/// removed chip never names a newer one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AttachmentId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    File,
    Image,
}

/// What the owner hands the composer to show as a chip.
#[derive(Clone)]
pub struct AttachmentDraft {
    pub kind: AttachmentKind,
    /// The file name, or a description of the image.
    pub label: SharedString,
    /// The pixels of an image chip's thumbnail.
    pub image: Option<Arc<gpui::Image>>,
}

impl AttachmentDraft {
    pub fn file(label: impl Into<SharedString>) -> Self {
        Self {
            kind: AttachmentKind::File,
            label: label.into(),
            image: None,
        }
    }

    pub fn image(label: impl Into<SharedString>, image: Arc<gpui::Image>) -> Self {
        Self {
            kind: AttachmentKind::Image,
            label: label.into(),
            image: Some(image),
        }
    }
}

/// The timeline selection that goes with the next message. The owner sets it from the
/// timeline; it is changed there, not removed from the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextChip {
    pub label: SharedString,
    /// The selection cannot be sent; the label says why.
    pub problem: bool,
}

/// A chip: a draft with its id.
#[derive(Clone)]
pub struct Attachment {
    pub id: AttachmentId,
    pub draft: AttachmentDraft,
}

/// The chips on a draft message, in the order they were added.
#[derive(Clone, Default)]
pub struct Attachments {
    items: Vec<Attachment>,
    next_id: u64,
}

impl Attachments {
    pub fn add(&mut self, draft: AttachmentDraft) -> AttachmentId {
        let id = AttachmentId(self.next_id);
        self.next_id += 1;
        self.items.push(Attachment { id, draft });
        id
    }

    /// Removes a chip. Returns whether it was there.
    pub fn remove(&mut self, id: AttachmentId) -> bool {
        let before = self.items.len();
        self.items.retain(|item| item.id != id);
        self.items.len() != before
    }

    pub fn items(&self) -> &[Attachment] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Takes every chip, leaving the strip empty. Ids keep growing.
    pub fn take(&mut self) -> Vec<Attachment> {
        std::mem::take(&mut self.items)
    }

    /// Puts chips back under their own ids, as when the owner refuses a send.
    pub fn restore(&mut self, items: Vec<Attachment>) {
        for item in &items {
            self.next_id = self.next_id.max(item.id.0 + 1);
        }
        let mut merged = items;
        merged.append(&mut self.items);
        merged.sort_by_key(|item| item.id);
        self.items = merged;
    }
}

/// Called with the id of the chip whose remove button was pressed.
pub type RemoveHandler = Rc<dyn Fn(AttachmentId, &mut Window, &mut App)>;

/// The wrapping strip of chips, or nothing when there are none.
pub fn strip(
    attachments: &Attachments,
    context: Option<&ContextChip>,
    theme: &Theme,
    on_remove: RemoveHandler,
) -> Option<impl IntoElement> {
    if attachments.is_empty() && context.is_none() {
        return None;
    }
    let chips = attachments
        .items()
        .iter()
        .map(|attachment| chip(attachment, theme, on_remove.clone()))
        .collect::<Vec<_>>();
    Some(
        div()
            .id("composer-attachments")
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(STRIP_GAP))
            .px(px(STRIP_PAD_X))
            .children(context.map(|context| context_chip(context, theme)))
            .children(chips),
    )
}

/// The timeline selection as a chip; its full label is the tooltip.
fn context_chip(context: &ContextChip, theme: &Theme) -> impl IntoElement {
    let (glyph, tone) = if context.problem {
        (Icon::AlertTriangle, Tone::Danger)
    } else {
        (Icon::Layers, Tone::Info)
    };
    let (tip_theme, tip) = (*theme, context.label.clone());
    div()
        .id("composer-context")
        .tooltip_show_delay(crate::kit::TOOLTIP_SHOW_DELAY)
        .tooltip(move |_, cx| crate::kit::Tooltip::new(&tip_theme, tip.clone()).view(cx))
        .child(
            Chip::new(theme, "composer-context-chip", context.label.clone())
                .icon(glyph)
                .tone(tone),
        )
}

fn chip(attachment: &Attachment, theme: &Theme, on_remove: RemoveHandler) -> AnyElement {
    let id = attachment.id;
    let label = attachment.draft.label.clone();
    match (&attachment.draft.kind, &attachment.draft.image) {
        (AttachmentKind::Image, Some(image)) => {
            let group = SharedString::from(format!("attachment-{}", id.0));
            let remove =
                IconButton::new(theme, ("attachment-remove", id.0 as usize), Icon::X.path())
                    .size(IconButtonSize::Sm)
                    .emphasis(Emphasis::Neutral)
                    .round()
                    .label(format!("Remove {label}"))
                    .tooltip("Remove")
                    .on_click(move |_, window, cx| on_remove(id, window, cx));
            div()
                .id(("attachment", id.0 as usize))
                .group(group.clone())
                .relative()
                .flex_none()
                .size(px(STRIP_THUMB))
                .rounded(crate::agent::ui::ITEM_RADIUS)
                .overflow_hidden()
                .bg(theme.colors.ctl)
                .child(
                    img(image.clone())
                        .size_full()
                        .object_fit(gpui::ObjectFit::Cover),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(METRICS.space_1))
                        .right(px(METRICS.space_1))
                        .invisible()
                        .group_hover(group, |style| style.visible())
                        .child(remove),
                )
                .into_any_element()
        }
        _ => file_chip(id, label, theme, on_remove).into_any_element(),
    }
}

/// A file chip: the paperclip, the name (truncated, the full name as tooltip) and a 24 px
/// remove target that is always shown, on the `ctl` fill behind a hairline.
fn file_chip(
    id: AttachmentId,
    label: SharedString,
    theme: &Theme,
    on_remove: RemoveHandler,
) -> impl IntoElement {
    let colors = theme.colors;
    let remove = IconButton::new(
        theme,
        ElementId::Name(format!("attachment-{}-remove", id.0).into()),
        Icon::X.path(),
    )
    .size(IconButtonSize::Sm)
    .emphasis(Emphasis::Ghost)
    .label(format!("Remove {label}"))
    .tooltip("Remove")
    .on_click(move |_, window, cx| on_remove(id, window, cx));
    let (tip_theme, tip) = (*theme, label.clone());
    div()
        .id(("attachment", id.0 as usize))
        .flex()
        .flex_none()
        .items_center()
        .gap(px(FILE_CHIP_GAP))
        .h(px(METRICS.size.chip_h))
        .pl(px(FILE_CHIP_PAD_LEFT))
        .pr(px(FILE_CHIP_PAD_RIGHT))
        .rounded(CONTROL_CORNER)
        .border_1()
        .border_color(colors.line)
        .bg(colors.ctl)
        .child(icon(Icon::Paperclip, METRICS.size.icon_sm, colors.text_2))
        .child(
            text_style(
                div().id(("attachment-name", id.0 as usize)),
                TextStyle::Label,
            )
            .max_w(px(FILE_CHIP_NAME_MAX_W))
            .truncate()
            .text_color(colors.text_1)
            .tooltip_show_delay(crate::kit::TOOLTIP_SHOW_DELAY)
            .tooltip(move |_, cx| crate::kit::Tooltip::new(&tip_theme, tip.clone()).view(cx))
            .child(label),
        )
        .child(remove)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_only_grow_even_after_removal() {
        let mut chips = Attachments::default();
        let a = chips.add(AttachmentDraft::file("a.txt"));
        let b = chips.add(AttachmentDraft::file("b.txt"));
        assert!(chips.remove(b));
        let c = chips.add(AttachmentDraft::file("c.txt"));
        assert!(a < b && b < c);
        assert!(!chips.remove(b), "a removed id is gone");
        assert_eq!(chips.len(), 2);
    }

    #[test]
    fn restore_keeps_ids_and_order_and_never_reuses_them() {
        let mut chips = Attachments::default();
        let a = chips.add(AttachmentDraft::file("a.txt"));
        let b = chips.add(AttachmentDraft::file("b.txt"));
        let taken = chips.take();
        assert!(chips.is_empty());
        let fresh = chips.add(AttachmentDraft::file("fresh.txt"));
        assert!(fresh > b);
        chips.restore(taken);
        let ids: Vec<_> = chips.items().iter().map(|item| item.id).collect();
        assert_eq!(ids, [a, b, fresh]);
        assert!(chips.add(AttachmentDraft::file("later.txt")) > fresh);
    }
}
