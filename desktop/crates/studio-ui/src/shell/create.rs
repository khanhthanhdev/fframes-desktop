//! Create in the shell: the panel that asks what the project starts from, its style and
//! its frame, then the folder picker, then the engine's `Create` with that choice.

use gpui::{Context, IntoElement};
use studio_engine::engine::backend::NewProject;

use super::{Picker, StudioShell};
use crate::create_panel::CreatePanel;

impl StudioShell {
    /// Shows the Create panel in place of the editor, with a Blank 1080p 30 fps project in
    /// the starter style chosen.
    pub(super) fn open_create(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.overflow_open = false;
        self.create_choice = Some(NewProject::default());
        cx.notify();
    }

    fn close_create(&mut self, cx: &mut Context<Self>) {
        self.create_choice = None;
        cx.notify();
    }

    /// Asks for the new project's folder; the panel closes, the choice goes with it.
    fn continue_create(&mut self, cx: &mut Context<Self>) {
        let Some(choice) = self.create_choice.take() else {
            return;
        };
        self.pick(Picker::Create(choice), cx);
    }

    pub(super) fn render_create(
        &self,
        choice: &NewProject,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let entity = cx.entity();
        CreatePanel::new(&self.theme, choice, &self.presentation.styles)
            .on_change({
                let entity = entity.clone();
                move |choice, _, cx| {
                    entity.update(cx, |shell, cx| {
                        shell.create_choice = Some(choice);
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
