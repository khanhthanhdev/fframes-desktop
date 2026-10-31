//! Native style-preset controls: bundled and imported preset selection, the CSS import
//! report, export, basic project override editing and an explicit reset.
//!
//! The panel never touches project files itself: it renders a
//! [`studio_engine::presets::PresetView`] and emits [`PresetEvent`]s, and the shell runs
//! the commands through the engine. The video preset's tokens never reach the Studio
//! chrome: the panel uses the application design system, separate from project styling.
use gpui::{
    AppContext, Context, ElementId, Entity, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use studio_engine::PresetMutation;
use studio_engine::presets::{
    CssView, EntryView, MAX_CSS_LINES, Outcome, PresetCommand, PresetReport, PresetView,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use crate::kit::{
    Badge, Button, ButtonSize, CollapsibleSection, Emphasis, EmptyState, FormField, Icon,
    IconButton, IconButtonSize, List, ListRow, Notice, PanelSection, TextInput, TextInputEvent,
    TextInputState, Tone, selectable,
};

// ---- GPUI panel ------------------------------------------------------------------------

/// Native pickers the panel asks the shell to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetPick {
    ImportFolder,
    ImportCss,
    Export { key: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetEvent {
    Run(PresetCommand),
    Pick(PresetPick),
    /// Escape in an input: focus returns to the shell.
    Leave,
}

impl EventEmitter<PresetEvent> for PresetPanel {}

pub struct PresetPanel {
    view: Option<PresetView>,
    selected: Option<String>,
    last: Option<PresetReport>,
    css: Option<CssView>,
    css_open: bool,
    token: Entity<TextInputState>,
    value: Entity<TextInputState>,
    /// Refreshed at the top of every render.
    theme: Theme,
}

/// An override field: Return sets the override, Escape hands the keyboard back to the
/// shell.
fn input(
    placeholder: &str,
    window: &mut Window,
    cx: &mut Context<PresetPanel>,
) -> Entity<TextInputState> {
    let placeholder = placeholder.to_owned();
    let input = cx.new(|cx| {
        let mut input = TextInputState::single_line(window, cx);
        input.set_placeholder(placeholder);
        input
    });
    cx.subscribe(&input, |panel, _, event: &TextInputEvent, cx| match event {
        TextInputEvent::Confirmed => panel.set_override(cx),
        TextInputEvent::Cancelled => cx.emit(PresetEvent::Leave),
        _ => (),
    })
    .detach();
    input
}

impl PresetPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            view: None,
            selected: None,
            last: None,
            css: None,
            css_open: true,
            token: input("Token, for example color.accent", window, cx),
            value: input("Value, for example #ff5533 or 120", window, cx),
            theme: Theme::for_window(window, cx),
        }
    }

    // ---- shell interface ---------------------------------------------------------------

    /// The latest presentation of the open project (`None` when none is open).
    pub fn set_view(&mut self, view: Option<PresetView>, cx: &mut Context<Self>) {
        match &view {
            None => {
                self.last = None;
                self.css = None;
                self.selected = None;
            }
            Some(view) => {
                let still = self
                    .selected
                    .as_ref()
                    .is_some_and(|key| view.entries.iter().any(|e| &e.key == key));
                if !still {
                    // Start on what the project uses, else the first bundled preset.
                    self.selected = view
                        .entries
                        .iter()
                        .find(|e| e.applied)
                        .or_else(|| view.entries.first())
                        .map(|e| e.key.clone());
                }
            }
        }
        self.view = view;
        cx.notify();
    }

    /// The outcome of the command the shell just ran.
    pub fn report(&mut self, report: PresetReport, cx: &mut Context<Self>) {
        if let Some(css) = &report.css {
            self.css = Some(css.clone());
        }
        self.last = Some(report);
        cx.notify();
    }

    pub fn selected_key(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The catalog, the applied preset, why mutation is blocked (if it is) and the last
    /// command's status lines; preset ids and token names only, never values.
    pub fn telemetry(&self) -> serde_json::Value {
        let Some(view) = &self.view else {
            return serde_json::json!({ "entries": [] });
        };
        serde_json::json!({
            "entries": view.entries.iter().map(|entry| serde_json::json!({
                "key": entry.key,
                "id": entry.id,
                "applied": entry.applied,
            })).collect::<Vec<_>>(),
            "selected": self.selected,
            "applied": view.applied.as_ref().map(|applied| serde_json::json!({
                "id": applied.id,
                "overrides": applied.overridden.len(),
            })),
            "blocked": match &view.mutation {
                PresetMutation::Enabled => None,
                PresetMutation::Disabled(reason) => Some(reason.clone()),
            },
            "status": self.last.as_ref().map(PresetReport::status_lines),
        })
    }

    /// True while an override field holds keyboard focus.
    pub fn has_text_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        use gpui::Focusable;
        self.token.focus_handle(cx).is_focused(window)
            || self.value.focus_handle(cx).is_focused(window)
    }

    // ---- building blocks ---------------------------------------------------------------

    /// A button at its natural width that runs `action` on this panel.
    #[allow(clippy::too_many_arguments)]
    fn action_button(
        &self,
        id: impl Into<ElementId>,
        label: &'static str,
        enabled: bool,
        emphasis: Emphasis,
        glyph: Option<Icon>,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Button {
        let entity = cx.entity();
        let mut button = Button::new(&self.theme, id)
            .label(label)
            .emphasis(emphasis)
            .disabled(!enabled)
            .on_click(move |_, _, cx| {
                entity.update(cx, |panel, cx| action(panel, cx));
            });
        if let Some(glyph) = glyph {
            button = button.left_icon(glyph.path());
        }
        button
    }

    /// A row of buttons that wraps only when the page is too narrow for it.
    fn button_row() -> gpui::Div {
        div().flex().flex_wrap().gap(px(METRICS.space_2))
    }

    /// One preset in the list; Enter, Space or a click selects it.
    fn preset_row(&self, entry: &EntryView, selected: bool, cx: &mut Context<Self>) -> ListRow {
        let entity = cx.entity();
        let key = entry.key.clone();
        let origin = if entry.bundled { "Bundled" } else { "Imported" };
        let mut row = ListRow::new(&self.theme, SharedString::from(format!("preset-{key}")))
            .icon(Icon::Palette)
            .title(format!("{} {}", entry.name, entry.version))
            .meta(format!("{origin} · {}", entry.description))
            .tooltip(format!("{} — {}", entry.id, entry.description))
            .selected(selected)
            .on_click(move |_, cx| {
                entity.update(cx, |panel, cx| {
                    panel.selected = Some(key.clone());
                    cx.notify();
                });
            });
        if entry.applied {
            row = row.trailing(Badge::new("Applied").tone(Tone::Success));
        }
        row
    }

    fn small(text: impl Into<SharedString>, color: gpui::Rgba) -> gpui::Div {
        text_style(div(), TextStyle::Small)
            .text_color(color)
            .child(selectable(text))
    }

    fn run(&mut self, command: PresetCommand, cx: &mut Context<Self>) {
        cx.emit(PresetEvent::Run(command));
    }

    fn set_override(&mut self, cx: &mut Context<Self>) {
        let token = self.token.read(cx).text().trim().to_owned();
        let value = self.value.read(cx).text().trim().to_owned();
        if !token.is_empty() && !value.is_empty() {
            self.run(PresetCommand::SetOverride { token, value }, cx);
        }
    }

    fn field(
        &self,
        id: &'static str,
        input: Entity<TextInputState>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        // Return and Escape are the field's own key bindings; Tab is routed here.
        let field = input.clone();
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .on_key_down(
                cx.listener(move |_, event: &gpui::KeyDownEvent, window, cx| {
                    if field.read(cx).is_composing() {
                        cx.stop_propagation();
                        return;
                    }
                    if event.keystroke.key == "tab" {
                        if event.keystroke.modifiers.shift {
                            window.focus_prev(cx);
                        } else {
                            window.focus_next(cx);
                        }
                        cx.stop_propagation();
                    }
                }),
            )
            .child(TextInput::new(
                &self.theme,
                SharedString::from(format!("{id}-input")),
                &input,
            ))
    }

    /// The preset list, the selected preset and what to do with it.
    fn presets_section(
        &self,
        view: &PresetView,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> PanelSection {
        let theme = self.theme;
        let palette = theme.colors;
        let mut section = PanelSection::new(&theme, "Style presets")
            .description("Presets style the video only; Studio's own interface never changes.");
        if let PresetMutation::Disabled(reason) = &view.mutation {
            section = section.child(
                Notice::new(
                    "preset-disabled",
                    Tone::Warning,
                    "Preset changes are disabled",
                )
                .body(reason.clone()),
            );
        }
        let applied = match &view.applied {
            Some(applied) => FormField::row(
                &theme,
                "Applied",
                Self::small(
                    format!(
                        "{} {} ({})",
                        applied.name,
                        applied.version,
                        applied.hash.get(..12).unwrap_or(&applied.hash)
                    ),
                    palette.text_2,
                ),
            ),
            None => FormField::row(&theme, "Applied", Self::small("None", palette.text_3))
                .hint("This project keeps its own styles until you choose a preset."),
        };
        section = section.child(applied);
        let rows = view
            .entries
            .iter()
            .map(|entry| {
                let selected = self.selected.as_deref() == Some(entry.key.as_str());
                self.preset_row(entry, selected, cx)
            })
            .collect::<Vec<_>>();
        section = section.child(List::new().children(rows).empty(
            EmptyState::new(&theme, "No presets").body("Import a preset folder to add one."),
        ));
        // The skipped folders follow the actions, which belong to the list's selection.
        let skipped = view
            .skipped
            .iter()
            .enumerate()
            .map(|(index, skipped)| {
                Notice::new(
                    ElementId::from(("preset-skipped", index)),
                    Tone::Warning,
                    format!("Skipped: {skipped}"),
                )
            })
            .collect::<Vec<_>>();
        let selected = self
            .selected
            .as_ref()
            .and_then(|key| view.entries.iter().find(|e| &e.key == key))
            .cloned();
        let key = selected.as_ref().map(|e| e.key.clone());
        let can_select = enabled && key.is_some();
        let reapply = enabled && selected.as_ref().is_some_and(|e| e.same_id_applied);
        let reset = enabled && view.applied.is_some() && key.is_some();
        let (apply_key, reapply_key, reset_key) = (key.clone(), key.clone(), key);
        section
            .child(
                Self::button_row()
                    .child(self.action_button(
                        "preset-apply",
                        "Apply",
                        can_select,
                        Emphasis::Primary,
                        Some(Icon::Check),
                        cx,
                        move |this, cx| {
                            if let Some(key) = apply_key.clone() {
                                this.run(PresetCommand::Apply { key }, cx);
                            }
                        },
                    ))
                    .child(
                        self.action_button(
                            "preset-reapply",
                            "Reapply",
                            reapply,
                            Emphasis::Neutral,
                            Some(Icon::RefreshCw),
                            cx,
                            move |this, cx| {
                                if let Some(key) = reapply_key.clone() {
                                    this.run(PresetCommand::Reapply { key }, cx);
                                }
                            },
                        )
                        .tooltip("Reapply, keeping the project overrides"),
                    )
                    .child(self.action_button(
                        "preset-reset",
                        "Reset overrides",
                        reset,
                        Emphasis::Neutral,
                        Some(Icon::History),
                        cx,
                        move |this, cx| {
                            if let Some(key) = reset_key.clone() {
                                this.run(PresetCommand::Reset { key }, cx);
                            }
                        },
                    )),
            )
            .children(skipped)
    }

    /// Project overrides: the user's layer, kept across Reapply and reopen.
    fn overrides_section(
        &self,
        view: &PresetView,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> PanelSection {
        let theme = self.theme;
        let section = PanelSection::new(&theme, "Project overrides");
        let Some(applied) = &view.applied else {
            return section.child(
                EmptyState::new(&theme, "No preset applied")
                    .body("Apply a preset to override its tokens for this project."),
            );
        };
        let rows = applied
            .overridden
            .iter()
            .map(|(token, layer, value)| {
                let clear = token.clone();
                let entity = cx.entity();
                ListRow::new(&theme, SharedString::from(format!("override-{token}")))
                    .title(token.clone())
                    .meta(format!("{layer} · {value}"))
                    .tooltip(format!("{token} = {value} ({layer})"))
                    .trailing(
                        IconButton::new(&theme, format!("clear-{token}"), Icon::X.path())
                            .size(IconButtonSize::Sm)
                            .label("Clear")
                            .tooltip("Clear")
                            .disabled(!enabled)
                            .on_click(move |_, _, cx| {
                                entity.update(cx, |this, cx| {
                                    this.run(
                                        PresetCommand::ClearOverride {
                                            token: clear.clone(),
                                        },
                                        cx,
                                    );
                                });
                            }),
                    )
            })
            .collect::<Vec<_>>();
        let mut section = section.child(
            List::new()
                .children(rows)
                .empty(EmptyState::new(&theme, "No project overrides")),
        );
        for (index, diagnostic) in applied.diagnostics.iter().enumerate() {
            section = section.child(Notice::new(
                ElementId::from(("preset-diagnostic", index)),
                Tone::Warning,
                format!("Kept, not applied: {diagnostic}"),
            ));
        }
        let token = self.field("override-token", self.token.clone(), cx);
        let value = self.field("override-value", self.value.clone(), cx);
        // Stacked, so the fields keep the whole width of a narrow sidebar.
        section
            .child(FormField::stacked(&theme, "Token", token))
            .child(FormField::stacked(&theme, "Value", value))
            .child(Self::button_row().child(self.action_button(
                "override-set",
                "Set override",
                enabled,
                Emphasis::Neutral,
                Some(Icon::Plus),
                cx,
                |this, cx| this.set_override(cx),
            )))
    }

    /// The CSS import report, collapsible, its declarations scrolling.
    fn css_report(&self, css: &CssView, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = self.theme;
        let palette = theme.colors;
        let mut lines = div()
            .id("css-lines")
            .max_h(px(METRICS.layout.css_report_max_h))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .pt(px(METRICS.space_1))
            .child(Self::small(
                format!(
                    "CSS report · {}: {} accepted · {} unsupported · {} rejected",
                    css.file, css.accepted, css.unsupported, css.rejected
                ),
                palette.text_1,
            ));
        for line in &css.lines {
            lines = lines.child(Self::small(line.clone(), palette.text_2));
        }
        if css.truncated {
            lines = lines.child(Self::small(
                format!("Showing the first {MAX_CSS_LINES} declarations."),
                palette.text_2,
            ));
        }
        let entity = cx.entity();
        div()
            .id("css-report")
            .child(
                CollapsibleSection::new("css-report-section", self.css_open)
                    .header(
                        text_style(div(), TextStyle::Label)
                            .text_color(palette.text_2)
                            .child("CSS import report"),
                    )
                    .content(lines)
                    .on_toggle(move |open, _, cx| {
                        entity.update(cx, |panel, cx| {
                            panel.css_open = *open;
                            cx.notify();
                        });
                    }),
            )
            .into_any_element()
    }
}

impl Render for PresetPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme.refresh(window, cx);
        let theme = self.theme;
        let root = PanelSection::column().id("preset-panel");
        let Some(view) = self.view.clone() else {
            return root.child(
                PanelSection::new(&theme, "Style presets")
                    .description(
                        "Presets style the video only; Studio's own interface never changes.",
                    )
                    .child(
                        EmptyState::new(&theme, "No project open")
                            .body("Open a project to choose a preset."),
                    ),
            );
        };
        let enabled = view.mutation.is_enabled();
        let export_key = self
            .selected
            .as_ref()
            .filter(|key| view.entries.iter().any(|e| &e.key == *key))
            .cloned();
        let can_export = export_key.is_some();
        let files = PanelSection::new(&theme, "Import and export").child(
            Self::button_row()
                .child(
                    self.action_button(
                        "preset-import",
                        "Import preset folder…",
                        true,
                        Emphasis::Neutral,
                        Some(Icon::Upload),
                        cx,
                        |_, cx| cx.emit(PresetEvent::Pick(PresetPick::ImportFolder)),
                    )
                    .size(ButtonSize::Sm),
                )
                .child(
                    self.action_button(
                        "preset-export",
                        "Export selected…",
                        can_export,
                        Emphasis::Neutral,
                        Some(Icon::Download),
                        cx,
                        move |_, cx| {
                            if let Some(key) = export_key.clone() {
                                cx.emit(PresetEvent::Pick(PresetPick::Export { key }));
                            }
                        },
                    )
                    .size(ButtonSize::Sm),
                )
                .child(
                    self.action_button(
                        "preset-css",
                        "CSS import report…",
                        true,
                        Emphasis::Neutral,
                        Some(Icon::File),
                        cx,
                        |_, cx| cx.emit(PresetEvent::Pick(PresetPick::ImportCss)),
                    )
                    .size(ButtonSize::Sm),
                ),
        );
        let mut root = root
            .child(self.presets_section(&view, enabled, cx))
            .child(files)
            .child(self.overrides_section(&view, enabled, cx));
        if let Some(last) = &self.last {
            let tone = match &last.outcome {
                Outcome::Committed { .. } | Outcome::Unchanged { .. } | Outcome::Done(_) => {
                    Tone::Success
                }
                Outcome::Refused(_) => Tone::Danger,
            };
            let (title, body) = status_text(last.status_lines());
            let mut status = Notice::new("preset-status", tone, title);
            if let Some(body) = body {
                status = status.body(body);
            }
            root = root.child(status);
        }
        if let Some(css) = &self.css {
            root = root.child(self.css_report(css, cx));
        }
        root
    }
}

/// The first status line as the notice title and the remaining lines as its body.
fn status_text(lines: Vec<String>) -> (String, Option<String>) {
    let mut lines = lines.into_iter();
    let title = lines.next().unwrap_or_default();
    let rest = lines.collect::<Vec<_>>().join("\n");
    (title, (!rest.is_empty()).then_some(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_splits_title_from_body() {
        assert_eq!(status_text(vec![]), (String::new(), None));
        assert_eq!(status_text(vec!["a".into()]), ("a".into(), None));
        assert_eq!(
            status_text(vec!["a".into(), "b".into(), "c".into()]),
            (
                "a".into(),
                Some(
                    "b
c"
                    .into()
                )
            )
        );
    }
}

/// Choosing and applying a preset from the keyboard.
#[cfg(test)]
mod keyboard {
    use std::{cell::RefCell, rc::Rc};

    use gpui::TestAppContext;
    use studio_engine::PresetMutation;
    use studio_engine::presets::{EntryView, PresetCommand, PresetView};

    use super::{PresetEvent, PresetPanel};

    #[gpui::test]
    fn enter_selects_a_preset_and_applies_it_once(cx: &mut TestAppContext) {
        let view = PresetView {
            entries: vec![EntryView {
                key: "k1".into(),
                id: "studio.basic".into(),
                name: "Basic".into(),
                version: "1.0.0".into(),
                description: "The basic look".into(),
                bundled: true,
                applied: false,
                same_id_applied: false,
            }],
            skipped: Vec::new(),
            applied: None,
            mutation: PresetMutation::Enabled,
        };
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = PresetPanel::new(window, cx);
            panel.set_view(Some(view), cx);
            panel
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let log = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &PresetEvent, _| {
                log.borrow_mut().push(event.clone())
            })
            .detach()
        });
        cx.run_until_parked();
        let tab = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| window.focus_next(cx));
            cx.run_until_parked();
        };
        // A whole press: the click a focused control gets from Enter comes on key-up.
        let enter = |cx: &mut gpui::VisualTestContext| {
            let keystroke = gpui::Keystroke::parse("enter").unwrap();
            cx.simulate_event(gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            });
            cx.simulate_event(gpui::KeyUpEvent { keystroke });
            cx.run_until_parked();
        };
        // The preset row is the first tab stop, Apply the next.
        tab(cx);
        enter(cx);
        assert_eq!(
            panel.read_with(cx, |panel, _| panel.selected_key().map(str::to_owned)),
            Some("k1".into())
        );
        tab(cx);
        enter(cx);
        assert_eq!(
            *events.borrow(),
            [PresetEvent::Run(PresetCommand::Apply { key: "k1".into() })]
        );
    }
}

/// The override fields' keyboard contract: Tab walks from token to value, Enter sets
/// the override, Escape leaves, and an IME composition keeps Enter and Escape.
#[cfg(test)]
mod field_keys {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{Entity, EntityInputHandler, Focusable, TestAppContext, VisualTestContext};
    use studio_engine::PresetMutation;
    use studio_engine::presets::{AppliedView, PresetCommand, PresetView};

    use super::{PresetEvent, PresetPanel};

    type Events = Rc<RefCell<Vec<PresetEvent>>>;

    fn applied() -> PresetView {
        PresetView {
            entries: Vec::new(),
            skipped: Vec::new(),
            applied: Some(AppliedView {
                id: "studio.basic".into(),
                name: "Basic".into(),
                version: "1.0.0".into(),
                hash: "0123456789abcdef".repeat(4),
                overridden: Vec::new(),
                diagnostics: Vec::new(),
            }),
            mutation: PresetMutation::Enabled,
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<PresetPanel>, Events, &mut VisualTestContext) {
        cx.update(crate::kit::bind_text_input_keys);
        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = PresetPanel::new(window, cx);
            panel.set_view(Some(applied()), cx);
            panel
        });
        let events = Events::default();
        let log = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &PresetEvent, _| {
                log.borrow_mut().push(event.clone())
            })
            .detach()
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let token = panel.read(cx).token.focus_handle(cx);
            window.focus(&token, cx);
        });
        (panel, events, cx)
    }

    #[gpui::test]
    fn tab_then_enter_sets_the_override(cx: &mut TestAppContext) {
        let (_, events, cx) = open(cx);
        cx.simulate_input("color.accent");
        cx.simulate_keystrokes("tab");
        cx.simulate_input("#ff5533");
        cx.simulate_keystrokes("enter");
        assert_eq!(
            *events.borrow(),
            [PresetEvent::Run(PresetCommand::SetOverride {
                token: "color.accent".into(),
                value: "#ff5533".into(),
            })]
        );
    }

    #[gpui::test]
    fn escape_leaves_the_field(cx: &mut TestAppContext) {
        let (_, events, cx) = open(cx);
        cx.simulate_keystrokes("escape");
        assert_eq!(*events.borrow(), [PresetEvent::Leave]);
    }

    #[gpui::test]
    fn a_composition_keeps_enter_and_escape(cx: &mut TestAppContext) {
        let (panel, events, cx) = open(cx);
        cx.simulate_input("color.accent");
        cx.update(|window, cx| {
            let token = panel.read(cx).token.clone();
            token.update(cx, |input, cx| {
                input.replace_and_mark_text_in_range(None, "\u{304b}", None, window, cx)
            });
        });
        // Escape first: a Return the IME takes ends the composition.
        cx.simulate_keystrokes("escape");
        cx.simulate_keystrokes("enter");
        assert!(events.borrow().is_empty(), "{:?}", events.borrow());
    }
}
