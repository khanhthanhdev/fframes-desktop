//! The panel's small building blocks: key routing in fields, buttons, tabs and text lines.

use super::*;

/// Reports a tab's bounds to its probe.
type BoundsReport = Rc<dyn Fn(gpui::Bounds<gpui::Pixels>, &mut gpui::App)>;

impl ConversationPanel {
    /// Key routing for one text field. The composition state is the field's own, read at
    /// the moment of the key: the prompt and every setup field share this one path.
    pub(super) fn key_in_field(
        &mut self,
        field: &Entity<TextInputState>,
        event: &gpui::KeyDownEvent,
        submit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let composing = field.read(cx).is_composing();
        self.key_in_input(event, composing, submit, window, cx);
    }

    pub(super) fn key_in_input(
        &mut self,
        event: &gpui::KeyDownEvent,
        composing: bool,
        submit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = &event.keystroke.modifiers;
        let mods = KeyMods {
            shift: modifiers.shift,
            alt: modifiers.alt,
            secondary: modifiers.control || modifiers.platform,
        };
        match route_input_key(event.keystroke.key.as_str(), mods, composing) {
            InputKey::Submit => {
                if submit {
                    self.submit(cx);
                }
                cx.stop_propagation();
            }
            InputKey::FocusNext => {
                window.focus_next(cx);
                cx.stop_propagation();
            }
            InputKey::FocusPrev => {
                window.focus_prev(cx);
                cx.stop_propagation();
            }
            InputKey::Leave => {
                cx.emit(PanelEvent::Leave);
                cx.stop_propagation();
            }
            InputKey::Composition => cx.stop_propagation(),
            // Typing, IME text, Space and the arrows belong to the input.
            InputKey::Text => (),
        }
    }

    // ---- building blocks -----------------------------------------------------------------------

    pub(super) fn activate(
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut gpui::App) + 'static {
        let entity = cx.entity();
        move |window, app| {
            entity.update(app, |this, cx| action(this, window, cx));
        }
    }

    /// Reports the bounds of the buttons and tabs native qualification clicks.
    pub(super) fn bounds_probe(id: &ElementId, cx: &mut Context<Self>) -> Option<BoundsProbe> {
        let name = probed_button(id)?.to_owned();
        let entity = cx.entity();
        Some(BoundsProbe::new(move |bounds, cx| {
            entity.update(cx, |panel, _| {
                panel.button_bounds.insert(name.clone(), bounds);
            });
        }))
    }

    /// [`Self::activate`] in the shape of a click handler.
    pub(super) fn click(
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static {
        let activate = Self::activate(cx, action);
        move |_, window, app| activate(window, app)
    }

    pub(super) fn action_button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        enabled: bool,
        emphasis: Emphasis,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Button {
        let theme = self.theme;
        let id: ElementId = id.into();
        let probe = Self::bounds_probe(&id, cx);
        let button = Button::new(&theme, id)
            .label(label)
            .emphasis(emphasis)
            .disabled(!enabled)
            .on_click(Self::click(cx, action));
        match probe {
            Some(probe) => button.on_bounds(probe.into_bounds_handler()),
            None => button,
        }
    }

    /// An icon-only button; `label` is its tooltip and accessible name. `Normal` has no fill.
    pub(super) fn icon_button(
        &self,
        id: impl Into<ElementId>,
        glyph: Icon,
        label: &'static str,
        emphasis: Emphasis,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> IconButton {
        let theme = self.theme;
        let id: ElementId = id.into();
        let probe = Self::bounds_probe(&id, cx);
        let button = IconButton::new(&theme, id, glyph.path())
            .size(IconButtonSize::Sm)
            .emphasis(emphasis)
            .label(label)
            .tooltip(label)
            .on_click(Self::click(cx, action));
        match probe {
            Some(probe) => button.on_bounds(probe.into_bounds_handler()),
            None => button,
        }
    }

    /// The panel's tabs on a segmented control. Each option keeps the id and the probed
    /// bounds it had as a button (the Changes tab is still `agent-tab-Project`).
    pub(super) fn tabs(&mut self, cx: &mut Context<Self>) -> SegmentedControl {
        const TABS: [(Tab, &str, &str); 3] = [
            (Tab::Chat, "Chat", "agent-tab-Chat"),
            (Tab::History, "History", "agent-tab-History"),
            (Tab::Project, "Changes", "agent-tab-Project"),
        ];
        let options = TABS
            .iter()
            .map(|(tab, label, id)| SegmentOption::new(*label, self.tab == *tab).id(*id))
            .collect();
        let reports: Vec<Option<BoundsReport>> = TABS
            .iter()
            .map(|(_, _, id)| {
                Self::bounds_probe(&ElementId::from(*id), cx)
                    .map(|probe| Rc::new(probe.into_bounds_handler()) as BoundsReport)
            })
            .collect();
        let entity = cx.entity();
        SegmentedControl::new(&self.theme, "agent-tab", options)
            .on_select(move |index, _, cx| {
                let Some((tab, ..)) = TABS.get(*index).copied() else {
                    return;
                };
                entity.update(cx, |this, cx| {
                    if this.tab != tab {
                        this.tab = tab;
                        if tab == Tab::History {
                            this.refresh_sessions(cx);
                        }
                        cx.notify();
                    }
                });
            })
            .on_option_bounds(move |index, bounds, cx| {
                if let Some(Some(report)) = reports.get(index) {
                    report(bounds, cx);
                }
            })
    }

    /// A titled group on a nested card.
    pub(super) fn section(&self, title: &str) -> gpui::Div {
        let theme = self.theme;
        Card::nested(&theme)
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .p(px(METRICS.space_3))
            .child(
                text_style(div(), TextStyle::Label)
                    .text_color(theme.colors.text_1)
                    .child(selectable(title.to_owned())),
            )
    }

    pub(super) fn line(&self, text: impl Into<SharedString>) -> gpui::Div {
        line(&self.theme, text)
    }

    // ---- rows ------------------------------------------------------------------------------------

    /// Rows get their spacing as padding inside the measured element, so the list's
    /// height measurement includes it and stays stable while a row streams.
    pub(super) fn row_shell() -> gpui::Div {
        div().w_full().pb(px(METRICS.space_2))
    }

    pub(super) fn caption(&self, text: &str) -> gpui::Div {
        text_style(div(), TextStyle::Small)
            .text_color(self.theme.colors.text_3)
            .child(selectable(text.to_owned()))
    }
}
