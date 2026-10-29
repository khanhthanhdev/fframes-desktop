//! The card for an agent's open question: one answer per item, an option or free text,
//! sent together only once every item has one. Nothing is chosen for the user and
//! nothing is sent on their behalf; Cancel declines the question.

use gpui::KeyDownEvent;
use studio_engine::agent::{OpenQuestion, QuestionAnswer};

use super::ui::ActionCard;
use super::*;
use crate::kit::{RadioCard, RadioCards};

/// What the user has answered for one item so far.
pub(super) struct ItemDraft {
    /// The labels of the chosen options.
    pub selected: Vec<String>,
    /// The free-text answer.
    pub text: Entity<TextInputState>,
}

impl ConversationPanel {
    /// Keeps one draft per item of every open question: drafts of closed questions go,
    /// new questions start empty. Runs in render, where fields can be created.
    pub(super) fn sync_questions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let open: Vec<OpenQuestion> = self
            .snapshot
            .as_ref()
            .map(|s| s.composer.questions.clone())
            .unwrap_or_default();
        self.host
            .questions
            .retain(|reference, _| open.iter().any(|q| &q.reference == reference));
        for question in open {
            if self.host.questions.contains_key(&question.reference) {
                continue;
            }
            let drafts = question
                .items
                .iter()
                .map(|_| ItemDraft {
                    selected: Vec::new(),
                    text: input("Or type your own answer", window, cx),
                })
                .collect();
            self.host.questions.insert(question.reference, drafts);
        }
    }

    /// The answers once every item has one, in item order.
    fn collect_answers(
        &self,
        reference: &PermissionRef,
        cx: &gpui::App,
    ) -> Option<Vec<QuestionAnswer>> {
        let drafts = self.host.questions.get(reference)?;
        drafts
            .iter()
            .map(|draft| {
                let text = draft.text.read(cx).text().trim().to_owned();
                (!draft.selected.is_empty() || !text.is_empty()).then(|| QuestionAnswer {
                    selected: draft.selected.clone(),
                    text: (!text.is_empty()).then_some(text),
                })
            })
            .collect()
    }

    fn toggle_option(
        &mut self,
        reference: &PermissionRef,
        item: usize,
        label: &str,
        multi: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self
            .host
            .questions
            .get_mut(reference)
            .and_then(|drafts| drafts.get_mut(item))
        else {
            return;
        };
        let at = draft.selected.iter().position(|chosen| chosen == label);
        match (at, multi) {
            (Some(at), _) => {
                draft.selected.remove(at);
            }
            (None, true) => draft.selected.push(label.to_owned()),
            (None, false) => draft.selected = vec![label.to_owned()],
        }
        cx.notify();
    }

    fn send_answers(&mut self, reference: &PermissionRef, cx: &mut Context<Self>) {
        let Some(answers) = self.collect_answers(reference, cx) else {
            self.say("Answer every question first.", cx);
            return;
        };
        if let Some(workflow) = self.workflow().cloned() {
            let result = workflow.answer_questions(reference, answers);
            self.command(result, cx);
        }
    }

    fn decline_question(&mut self, reference: &PermissionRef, cx: &mut Context<Self>) {
        if let Some(workflow) = self.workflow().cloned() {
            let result = workflow.reply_permission(reference, PermissionAnswer::Cancel);
            self.command(result, cx);
        }
    }

    pub(super) fn question_card(
        &mut self,
        index: usize,
        question: &OpenQuestion,
        cx: &mut Context<Self>,
    ) -> ActionCard {
        let theme = self.theme;
        let ready = self.collect_answers(&question.reference, cx).is_some();
        let mut card = ActionCard::new(
            &theme,
            SharedString::from(format!("question-{index}")),
            UiTone::Warning,
            if question.title.is_empty() {
                "The agent has a question"
            } else {
                question.title.as_str()
            },
        );
        for (item_index, item) in question.items.iter().enumerate() {
            let Some(draft) = self
                .host
                .questions
                .get(&question.reference)
                .and_then(|drafts| drafts.get(item_index))
            else {
                continue;
            };
            let field = draft.text.clone();
            let chosen = draft.selected.clone();
            let mut block = div().flex().flex_col().gap(px(METRICS.space_2));
            if !item.header.is_empty() {
                block = block.child(self.caption(&item.header));
            }
            block = block.child(
                text_style(div(), TextStyle::Label)
                    .text_color(theme.colors.text_1)
                    .child(selectable(item.question.clone())),
            );
            for (option_index, option) in item.options.iter().enumerate() {
                // One card per option, so several can be chosen when the item allows it.
                let id: ElementId =
                    format!("question-{index}-{item_index}-opt-{option_index}").into();
                let probe = Self::bounds_probe(&id, cx);
                let reference = question.reference.clone();
                let label = option.label.clone();
                let multi = item.multi_select;
                let entity = cx.entity();
                let description =
                    (!option.description.is_empty()).then_some(option.description.as_str());
                let mut cards = RadioCards::new(
                    &theme,
                    id,
                    vec![RadioCard::new(option.label.clone(), description)],
                    chosen.contains(&option.label).then_some(0),
                )
                .on_select(move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.toggle_option(&reference, item_index, &label, multi, cx)
                    });
                });
                if let Some(probe) = probe {
                    let report = probe.into_bounds_handler();
                    cards = cards.on_option_bounds(move |_, bounds, cx| report(bounds, cx));
                }
                block = block.child(cards);
            }
            let id: ElementId = format!("question-{index}-{item_index}-text").into();
            let mut wrapper = div().relative().on_key_down(cx.listener({
                let field = field.clone();
                move |this, event: &KeyDownEvent, window, cx| {
                    this.key_in_field(&field, event, false, window, cx);
                }
            }));
            if let Some(probe) = Self::bounds_probe(&id, cx) {
                wrapper = wrapper.child(crate::kit::bounds_probe(Rc::new(
                    probe.into_bounds_handler(),
                )));
            }
            block = block.child(wrapper.child(TextInput::new(&theme, id, &field)));
            card = card.child(block);
        }
        let send_reference = question.reference.clone();
        let decline_reference = question.reference.clone();
        card.action(self.action_button(
            format!("question-{index}-cancel"),
            "Cancel",
            true,
            Emphasis::Neutral,
            cx,
            move |this, _, cx| this.decline_question(&decline_reference, cx),
        ))
        .action(self.action_button(
            format!("question-{index}-send"),
            "Send answers",
            ready,
            Emphasis::Primary,
            cx,
            move |this, _, cx| this.send_answers(&send_reference, cx),
        ))
    }
}
