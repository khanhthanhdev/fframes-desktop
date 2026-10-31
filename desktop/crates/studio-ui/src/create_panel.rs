//! The first step of Create: what the new project starts from (Blank or a Slide deck).
//! It is shown in the window in place of the editor, like the export panel; its Choose
//! folder… asks for the new project's folder.

use std::rc::Rc;

use gpui::{
    App, InteractiveElement, IntoElement, ParentElement, RenderOnce, Styled, Window, div, px,
};
use studio_project::ProjectTemplate;
use studio_theme::{METRICS, Theme};

use crate::kit::{
    Button, Card, Emphasis, PanelSection, RadioCard, RadioCards, workflow_column, workflow_title,
};

type Choose = Rc<dyn Fn(ProjectTemplate, &mut Window, &mut App)>;
type Click = Rc<dyn Fn(&mut Window, &mut App)>;

/// The templates in the order the panel lists them, with their descriptions.
pub const TEMPLATES: [(ProjectTemplate, &str, &str); 2] = [
    (
        ProjectTemplate::Blank,
        "Blank",
        "Three title scenes to start a video from.",
    ),
    (
        ProjectTemplate::SlideDeck,
        "Slide deck",
        "Six fframes-kit slides with bullets that come in one by one, a stop after each and \
         speaker notes, ready to present or export as an HTML deck.",
    ),
];

#[derive(IntoElement)]
pub struct CreatePanel {
    theme: Theme,
    selected: ProjectTemplate,
    on_choose: Option<Choose>,
    on_continue: Option<Click>,
    on_back: Option<Click>,
}

impl CreatePanel {
    pub fn new(theme: &Theme, selected: ProjectTemplate) -> Self {
        Self {
            theme: *theme,
            selected,
            on_choose: None,
            on_continue: None,
            on_back: None,
        }
    }

    pub fn on_choose(
        mut self,
        handler: impl Fn(ProjectTemplate, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_choose = Some(Rc::new(handler));
        self
    }

    /// Choose folder…: asks for the folder of the project.
    pub fn on_continue(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_continue = Some(Rc::new(handler));
        self
    }

    pub fn on_back(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_back = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for CreatePanel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let on_choose = self.on_choose.clone();
        let choice = RadioCards::new(
            &theme,
            "create-template",
            TEMPLATES
                .iter()
                .map(|(_, label, text)| RadioCard::new(*label, Some(text)))
                .collect(),
            TEMPLATES
                .iter()
                .position(|(template, _, _)| *template == self.selected),
        )
        .on_select(move |index, window, cx| {
            if let (Some(choose), Some((template, _, _))) = (&on_choose, TEMPLATES.get(*index)) {
                choose(*template, window, cx);
            }
        });
        let on_continue = self.on_continue.clone();
        let on_back = self.on_back.clone();
        let actions = div()
            .flex()
            .items_center()
            .gap(px(METRICS.space_2))
            .child(
                Button::new(&theme, "create-back")
                    .emphasis(Emphasis::Neutral)
                    .label("Back")
                    .on_click(move |_, window, cx| {
                        if let Some(back) = &on_back {
                            back(window, cx);
                        }
                    }),
            )
            .child(div().flex_1())
            .child(
                Button::new(&theme, "create-continue")
                    .emphasis(Emphasis::Primary)
                    .label("Choose folder…")
                    .on_click(move |_, window, cx| {
                        if let Some(next) = &on_continue {
                            next(window, cx);
                        }
                    }),
            );
        // Title, the choice under a section heading, and the actions, Back left and the
        // one primary action right, as on the export panel.
        Card::raised(&theme)
            .id("create-panel")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .p(px(METRICS.space_6))
            .flex()
            .flex_col()
            .child(
                workflow_column()
                    .gap(px(METRICS.space_6))
                    .child(workflow_title(
                        &theme,
                        "Create a project",
                        "Pick what the new video starts from; Studio asks for its folder next.",
                    ))
                    .child(PanelSection::new(&theme, "Start from").child(choice))
                    .child(actions),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_template_is_offered_once_and_blank_is_first() {
        let offered: Vec<_> = TEMPLATES.iter().map(|(template, _, _)| *template).collect();
        assert_eq!(offered, ProjectTemplate::ALL);
        assert_eq!(offered[0], ProjectTemplate::default());
    }
}
