//! Create in the shell: the panel that asks what the project starts from, then the folder
//! picker, then the engine's `Create` with the chosen template.

use gpui::{Context, IntoElement};
use studio_project::ProjectTemplate;

use super::{Picker, StudioShell};
use crate::create_panel::CreatePanel;

impl StudioShell {
    /// Shows the Create panel in place of the editor, with Blank chosen.
    pub(super) fn open_create(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.overflow_open = false;
        self.create_choice = Some(ProjectTemplate::default());
        cx.notify();
    }

    fn close_create(&mut self, cx: &mut Context<Self>) {
        self.create_choice = None;
        cx.notify();
    }

    /// Asks for the new project's folder; the panel closes, the template goes with it.
    fn continue_create(&mut self, cx: &mut Context<Self>) {
        let Some(template) = self.create_choice.take() else {
            return;
        };
        self.pick(Picker::Create(template), cx);
    }

    pub(super) fn render_create(
        &self,
        template: ProjectTemplate,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let entity = cx.entity();
        CreatePanel::new(&self.theme, template)
            .on_choose({
                let entity = entity.clone();
                move |template, _, cx| {
                    entity.update(cx, |shell, cx| {
                        shell.create_choice = Some(template);
                        cx.notify();
                    });
                }
            })
            .on_continue({
                let entity = entity.clone();
                move |_, cx| entity.update(cx, |shell, cx| shell.continue_create(cx))
            })
            .on_back(move |_, cx| entity.update(cx, |shell, cx| shell.close_create(cx)))
            .into_any_element()
    }
}
