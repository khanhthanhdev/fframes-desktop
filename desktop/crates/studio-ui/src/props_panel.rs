//! The props panel: the editable fields of the component selected on the canvas, after
//! the preview worker's answer to `PropsInfo`, with a control per kind and "reset to
//! code" per field.
//!
//! The panel never writes `props.json` itself: it emits [`PropsEvent`]s, and the shell
//! runs the commands through the engine, which writes the file and hands the document to
//! the preview. A slider drag is one gesture, so it is one undo step and one write.
use std::{cell::Cell, collections::HashMap, rc::Rc};

use fframes_studio_protocol::{EditorObjectIdentity, PropField, PropKind, PropsInfo};
use gpui::{
    AppContext, Bounds, Context, DispatchPhase, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, ParentElement,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div,
    px,
};
use serde_json::Value;
use studio_engine::props::{PropsCommand, PropsEditing, PropsSnapshot};
use studio_theme::{METRICS, Theme};

use crate::kit::{
    ColorPicker, ColorPickerSnapshot, EmptyState, FormField, Icon, IconButton, IconButtonSize,
    Menu, MenuItem, MenuState, Notice, PanelSection, SegmentOption, SegmentedControl, Select,
    Slider, TextInput, TextInputEvent, TextInputState, Toggle, Tone, bounds_probe, color_swatch,
    hsv_to_rgb, hue_from_point, sv_from_point,
};

#[derive(Debug, Clone, PartialEq)]
pub enum PropsEvent {
    Run(PropsCommand),
    /// Escape in a field: focus returns to the shell.
    Leave,
}

impl EventEmitter<PropsEvent> for PropsPanel {}

pub struct PropsPanel {
    snapshot: Option<PropsSnapshot>,
    selected: Option<EditorObjectIdentity>,
    /// The worker's answer for `selected`; `None` until it arrives.
    info: Option<Result<Option<PropsInfo>, String>>,
    /// Text fields of the shown schema, by field name, with the value they were last set
    /// to from the preview.
    inputs: HashMap<String, (Entity<TextInputState>, String)>,
    /// What the inputs were made for: the object and its component.
    inputs_for: Option<(EditorObjectIdentity, String)>,
    /// The field a slider drag changes; the drag is one gesture.
    dragging: Option<String>,
    /// Where each colour field's swatch was last painted, to anchor its picker.
    swatches: HashMap<String, Rc<Cell<Option<Bounds<Pixels>>>>>,
    picker: Option<Picker>,
    /// The open choice menu and the field it chooses for.
    menu: Option<(PropField, MenuState)>,
    menu_focus: FocusHandle,
    menu_return_focus: Option<FocusHandle>,
    /// Why the last entry could not be applied.
    error: Option<String>,
    /// Refreshed at the top of every render.
    theme: Theme,
}

impl PropsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            snapshot: None,
            selected: None,
            info: None,
            inputs: HashMap::new(),
            inputs_for: None,
            dragging: None,
            swatches: HashMap::new(),
            picker: None,
            menu: None,
            menu_focus: cx.focus_handle(),
            menu_return_focus: None,
            error: None,
            theme: Theme::for_window(window, cx),
        }
    }

    // ---- shell interface ---------------------------------------------------------------

    /// The latest props of the open project (`None` when none is open).
    pub fn set_snapshot(&mut self, snapshot: Option<PropsSnapshot>, cx: &mut Context<Self>) {
        if self.snapshot != snapshot {
            self.snapshot = snapshot;
            cx.notify();
        }
    }

    /// The object selected on the canvas; its props arrive with [`Self::set_info`].
    pub fn set_selection(
        &mut self,
        identity: Option<EditorObjectIdentity>,
        cx: &mut Context<Self>,
    ) {
        if self.selected != identity {
            self.selected = identity;
            self.info = None;
            self.error = None;
            self.end_drag(cx);
            self.picker = None;
            self.menu = None;
            cx.notify();
        }
    }

    /// The preview's answer for `identity`; ignored when another object is selected now.
    pub fn set_info(
        &mut self,
        identity: EditorObjectIdentity,
        info: Result<Option<PropsInfo>, String>,
        cx: &mut Context<Self>,
    ) {
        if self.selected.as_ref() == Some(&identity) {
            self.info = Some(info);
            cx.notify();
        }
    }

    /// The selection and what the panel shows for it; field names only, never values.
    pub fn telemetry(&self) -> serde_json::Value {
        let shown = match &self.info {
            Some(Ok(Some(info))) => serde_json::json!({
                "component": info.schema.component,
                "fields": info.schema.fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>(),
            }),
            Some(Ok(None)) => serde_json::json!("not editable"),
            Some(Err(error)) => serde_json::json!({ "error": error }),
            None => serde_json::Value::Null,
        };
        serde_json::json!({
            "selected": self.selected.as_ref().map(|id| id.object_key.clone()),
            "shown": shown,
            "dragging": self.dragging,
        })
    }

    // ---- edits -------------------------------------------------------------------------

    fn run(&mut self, command: PropsCommand, cx: &mut Context<Self>) {
        cx.emit(PropsEvent::Run(command));
    }

    fn set_field(&mut self, field: &str, value: Option<Value>, cx: &mut Context<Self>) {
        let Some(identity) = self.selected.clone() else {
            return;
        };
        self.error = None;
        self.run(
            PropsCommand::Set {
                identity,
                field: field.to_owned(),
                value,
            },
            cx,
        );
    }

    /// Return in a text field: the entry, read as the field's kind.
    fn commit(&mut self, field: &PropField, cx: &mut Context<Self>) {
        let Some((input, _)) = self.inputs.get(&field.name) else {
            return;
        };
        let text = input.read(cx).text().to_owned();
        match parse_entry(field, &text) {
            Ok(value) => self.set_field(&field.name, Some(value), cx),
            Err(error) => {
                self.error = Some(format!("{}: {error}", field.name));
                cx.notify();
            }
        }
    }

    fn drag(&mut self, field: &PropField, fraction: f32, cx: &mut Context<Self>) {
        let (Some(min), Some(max)) = (field.min, field.max) else {
            return;
        };
        self.begin_drag(&field.name, cx);
        let value = slider_value(field, min, max, fraction);
        self.set_field(&field.name, Some(value), cx);
    }

    /// Starts a gesture on `name` unless one already runs on it.
    fn begin_drag(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.dragging.as_deref() != Some(name) {
            self.end_drag(cx);
            self.dragging = Some(name.to_owned());
            self.run(PropsCommand::BeginGesture, cx);
        }
    }

    /// Ends a slider or picker drag as one gesture; nothing happens when no drag runs.
    pub fn end_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(picker) = &mut self.picker {
            picker.part = None;
        }
        if self.dragging.take().is_some() {
            self.run(PropsCommand::EndGesture, cx);
            cx.notify();
        }
    }

    /// Makes the text fields of `info`'s schema and puts the drawn values in the ones
    /// that are not being typed in.
    fn sync_inputs(
        &mut self,
        info: &PropsInfo,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(identity) = self.selected.clone() else {
            return;
        };
        let target = (identity, info.schema.component.clone());
        if self.inputs_for.as_ref() != Some(&target) {
            self.inputs.clear();
            self.swatches.clear();
            self.picker = None;
            self.menu = None;
            self.inputs_for = Some(target);
        }
        for field in &info.schema.fields {
            if field.kind == PropKind::Color {
                self.swatches.entry(field.name.clone()).or_default();
            }
        }
        for field in info.schema.fields.iter().filter(|f| typed(&f.kind)) {
            let shown = info
                .values
                .get(&field.name)
                .map(display_value)
                .unwrap_or_default();
            if !self.inputs.contains_key(&field.name) {
                let input = cx.new(|cx| TextInputState::single_line(window, cx));
                let committed = field.clone();
                cx.subscribe(
                    &input,
                    move |panel, _, event: &TextInputEvent, cx| match event {
                        TextInputEvent::Confirmed => panel.commit(&committed, cx),
                        TextInputEvent::Cancelled => cx.emit(PropsEvent::Leave),
                        _ => (),
                    },
                )
                .detach();
                self.inputs
                    .insert(field.name.clone(), (input, String::new()));
            }
            let (input, synced) = self.inputs.get_mut(&field.name).expect("just inserted");
            if *synced != shown && !input.focus_handle(cx).is_focused(window) {
                input.update(cx, |input, cx| input.set_text(shown.clone(), cx));
                *synced = shown;
            }
            input.update(cx, |input, cx| input.set_disabled(!enabled, cx));
        }
    }

    // ---- colour picker and choice menu -------------------------------------------------

    /// The value the preview drew for `name`.
    fn value(&self, name: &str) -> Option<Value> {
        match &self.info {
            Some(Ok(Some(info))) => info.values.get(name).cloned(),
            _ => None,
        }
    }

    /// Opens the colour picker under the field's swatch.
    fn open_picker(&mut self, field: &PropField, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bounds) = self.swatches.get(&field.name).and_then(|cell| cell.get()) else {
            return;
        };
        let (rgb, alpha) = self
            .value(&field.name)
            .as_ref()
            .and_then(Value::as_str)
            .and_then(parse_hex)
            .unwrap_or(([0, 0, 0], None));
        self.menu = None;
        self.picker = Some(Picker {
            field: field.name.clone(),
            snapshot: ColorPickerSnapshot::place(bounds.bottom_left(), window.viewport_size(), rgb),
            alpha,
            part: None,
        });
        cx.notify();
    }

    fn close_picker(&mut self, cx: &mut Context<Self>) {
        self.end_drag(cx);
        if self.picker.take().is_some() {
            cx.notify();
        }
    }

    /// A press or drag on the picker's field or hue rail: the colour under the pointer,
    /// one gesture until the button is released.
    fn pick(&mut self, part: PickerPart, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(picker) = &self.picker else {
            return;
        };
        let mut snapshot = picker.snapshot;
        match part {
            PickerPart::Field => {
                (snapshot.sat, snapshot.val) = sv_from_point(snapshot.origin, position);
            }
            PickerPart::Hue => snapshot.hue = hue_from_point(snapshot.origin, position),
        }
        let (name, alpha) = (picker.field.clone(), picker.alpha);
        self.begin_drag(&name, cx);
        if let Some(picker) = &mut self.picker {
            picker.snapshot = snapshot;
            picker.part = Some(part);
        }
        self.set_field(
            &name,
            Some(Value::String(hex(
                hsv_to_rgb(snapshot.hue, snapshot.sat, snapshot.val),
                alpha,
            ))),
            cx,
        );
        cx.notify();
    }

    /// The choice menu of a field with more options than a segmented control holds.
    fn open_menu(
        &mut self,
        field: &PropField,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker = None;
        let items = self.menu_items(field);
        self.menu = Some((field.clone(), MenuState::anchored(bounds, &items)));
        self.menu_return_focus = window.focused(cx);
        window.focus(&self.menu_focus, cx);
        cx.notify();
    }

    fn menu_items(&self, field: &PropField) -> Vec<MenuItem> {
        let PropKind::Choice { options } = &field.kind else {
            return Vec::new();
        };
        let current = self.value(&field.name);
        let current = current.as_ref().and_then(Value::as_str);
        options
            .iter()
            .map(|option| {
                MenuItem::new(option.clone(), current == Some(option.as_str()))
                    .id(format!("prop-{}-{option}", field.name))
            })
            .collect()
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            if let Some(focus) = self.menu_return_focus.take() {
                window.focus(&focus, cx);
            }
            cx.notify();
        }
    }

    // ---- rendering ---------------------------------------------------------------------

    /// One field: its label and control, a reset while it overrides the code, and a
    /// slider under numbers with a range.
    fn field_row(
        &self,
        field: &PropField,
        info: &PropsInfo,
        overridden: bool,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let value = info.values.get(&field.name);
        let id = format!("prop-{}", field.name);
        let mut label = field.name.clone();
        if let Some(unit) = &field.unit {
            label.push_str(&format!(" ({unit})"));
        }
        let (control, stacked) = match &field.kind {
            PropKind::Toggle => {
                let entity = cx.entity();
                let name = field.name.clone();
                let toggle = Toggle::new(
                    &theme,
                    id.clone(),
                    value.and_then(Value::as_bool).unwrap_or(false),
                )
                .label(field.name.clone())
                .disabled(!enabled)
                .on_change(move |checked, _, cx| {
                    entity.update(cx, |panel, cx| {
                        panel.set_field(&name, Some(Value::Bool(checked)), cx)
                    });
                });
                (toggle.into_any_element(), false)
            }
            PropKind::Choice { options } => (self.choice(field, options, value, enabled, cx), true),
            PropKind::Color => (self.color_entry(field, value, enabled, cx), false),
            PropKind::Text => (self.entry(field, false), true),
            PropKind::Number | PropKind::Integer => (self.entry(field, true), false),
        };
        let mut form = if stacked {
            FormField::stacked(&theme, label, control)
        } else {
            FormField::row(&theme, label, control)
        };
        if overridden && enabled {
            let entity = cx.entity();
            let name = field.name.clone();
            form = form.reset_id(format!("{id}-reset")).reset(move |_, cx| {
                entity.update(cx, |panel, cx| panel.set_field(&name, None, cx));
            });
        }
        div()
            .id(SharedString::from(id))
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .child(form)
            .children(self.slider(field, value, enabled, cx))
            .into_any_element()
    }

    /// The field's text input; `fill` makes it take the rest of a row.
    fn entry(&self, field: &PropField, fill: bool) -> gpui::AnyElement {
        match self.inputs.get(&field.name) {
            Some((input, _)) => TextInput::new(
                &self.theme,
                SharedString::from(format!("prop-{}-input", field.name)),
                input,
            )
            .flex(fill)
            .into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// A segmented control for a few options, a select for more.
    fn choice(
        &self,
        field: &PropField,
        options: &[String],
        value: Option<&Value>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let current = value.and_then(Value::as_str);
        let entity = cx.entity();
        if options.len() <= MAX_SEGMENTS {
            let segments = options
                .iter()
                .map(|option| {
                    SegmentOption::new(option.clone(), current == Some(option.as_str()))
                        .id(format!("prop-{}-{option}", field.name))
                        .disabled(!enabled)
                })
                .collect();
            let name = field.name.clone();
            let options = options.to_vec();
            SegmentedControl::new(&theme, format!("prop-{}-choice", field.name), segments)
                .stretch()
                .on_select(move |index, _, cx| {
                    if let Some(option) = options.get(*index) {
                        entity.update(cx, |panel, cx| {
                            panel.set_field(&name, Some(Value::String(option.clone())), cx)
                        });
                    }
                })
                .into_any_element()
        } else {
            let opened = field.clone();
            Select::new(
                &theme,
                format!("prop-{}-select", field.name),
                current.unwrap_or("Choose…").to_owned(),
            )
            .stretch_label()
            .disabled(!enabled)
            .on_open(move |bounds, window, cx| {
                entity.update(cx, |panel, cx| {
                    panel.open_menu(&opened, *bounds, window, cx)
                });
            })
            .into_any_element()
        }
    }

    /// The swatch that opens the colour picker, beside the hex entry.
    fn color_entry(
        &self,
        field: &PropField,
        value: Option<&Value>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let fill = value
            .and_then(Value::as_str)
            .and_then(parse_hex)
            .map(|(rgb, alpha)| swatch_color(rgb, alpha));
        let bounds = self.swatches.get(&field.name).cloned().unwrap_or_default();
        let entity = cx.entity();
        let opened = field.clone();
        let swatch = color_swatch(
            &self.theme,
            SharedString::from(format!("prop-{}-swatch", field.name)),
            fill,
            !enabled,
            Rc::new(move |window, cx| {
                entity.update(cx, |panel, cx| panel.open_picker(&opened, window, cx));
            }),
        )
        .child(bounds_probe(Rc::new(move |measured, _| {
            bounds.set(Some(measured))
        })));
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(px(METRICS.space_2))
            .child(swatch)
            .child(self.entry(field, true))
            .into_any_element()
    }

    /// A slider for a number with a range.
    fn slider(
        &self,
        field: &PropField,
        value: Option<&Value>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Option<Slider> {
        let (PropKind::Number | PropKind::Integer, Some(min), Some(max)) =
            (&field.kind, field.min, field.max)
        else {
            return None;
        };
        if max <= min {
            return None;
        }
        let fraction = value
            .and_then(Value::as_f64)
            .map_or(0., |v| ((v - min) / (max - min)).clamp(0., 1.)) as f32;
        let entity = cx.entity();
        let dragged = field.clone();
        Some(
            Slider::new(&self.theme, format!("prop-{}-slider", field.name), fraction)
                .label(field.name.clone())
                .disabled(!enabled)
                .on_change(0., move |fraction, _, cx| {
                    entity.update(cx, |panel, cx| panel.drag(&dragged, *fraction, cx));
                }),
        )
    }

    /// What the panel shows under its title for the current selection, and the line
    /// that names the selected component.
    fn body(
        &mut self,
        snapshot: &PropsSnapshot,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Option<String>, Vec<gpui::AnyElement>) {
        let theme = self.theme;
        let empty = |title: &str, body: String| {
            vec![
                EmptyState::new(&theme, title.to_owned())
                    .body(body)
                    .into_any_element(),
            ]
        };
        let Some(selected) = self.selected.clone() else {
            return (
                None,
                empty(
                    "Nothing selected",
                    "Select a component on the canvas to edit its props.".into(),
                ),
            );
        };
        let info = match self.info.clone() {
            None => {
                return (
                    None,
                    vec![
                        EmptyState::new(&theme, "Reading the selection's props…")
                            .into_any_element(),
                    ],
                );
            }
            Some(Err(error)) => {
                return (
                    None,
                    vec![
                        Notice::new(
                            "props-unavailable",
                            Tone::Warning,
                            "Could not read this component's props",
                        )
                        .body(error)
                        .into_any_element(),
                    ],
                );
            }
            Some(Ok(None)) => {
                return (
                    None,
                    empty(
                        "No editable props",
                        format!(
                            "{} \"{}\" has no editable props. Mark it editable in the code (`.editable(\"key\")`).",
                            selected.component_key, selected.object_key
                        ),
                    ),
                );
            }
            Some(Ok(Some(info))) => info,
        };
        self.sync_inputs(&info, enabled, window, cx);
        let description = format!(
            "{} \"{}\" in {}",
            info.schema.component, selected.object_key, selected.scene_instance_key
        );
        if info.schema.fields.is_empty() {
            return (
                Some(description),
                empty(
                    "No editable fields",
                    "This component has no editable fields.".into(),
                ),
            );
        }
        let overrides = snapshot
            .doc
            .as_ref()
            .and_then(|doc| doc.values(&selected))
            .cloned()
            .unwrap_or_default();
        let rows = info
            .schema
            .fields
            .iter()
            .map(|field| {
                let overridden = overrides.contains_key(&field.name);
                self.field_row(field, &info, overridden, enabled, cx)
            })
            .collect();
        (Some(description), rows)
    }

    /// The open colour picker over the whole window, closed by a press outside it.
    fn picker_layer(&self, window: &Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let picker = self.picker.as_ref()?;
        let viewport = window.viewport_size();
        let (close, field, hue) = (cx.entity(), cx.entity(), cx.entity());
        let layer = div()
            .relative()
            .w(viewport.width)
            .h(viewport.height)
            .child(
                div()
                    .id("prop-color-backdrop")
                    .occlude()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        close.update(cx, |panel, cx| panel.close_picker(cx));
                    }),
            )
            .child(
                ColorPicker::new(&self.theme, picker.snapshot)
                    .on_sv_down(move |event, _, cx| {
                        field.update(cx, |panel, cx| {
                            panel.pick(PickerPart::Field, event.position, cx)
                        });
                    })
                    .on_hue_down(move |event, _, cx| {
                        hue.update(cx, |panel, cx| {
                            panel.pick(PickerPart::Hue, event.position, cx)
                        });
                    }),
            );
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position_mode(gpui::AnchoredPositionMode::Window)
                    .position(Point::default())
                    .child(layer),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// The open choice menu of a field with many options.
    fn menu_layer(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (field, state) = self.menu.as_ref()?;
        let PropKind::Choice { options } = &field.kind else {
            return None;
        };
        let (highlight, select, dismiss) = (cx.entity(), cx.entity(), cx.entity());
        let name = field.name.clone();
        let options = options.clone();
        Some(
            Menu::new(
                &self.theme,
                format!("prop-{}-menu", field.name),
                self.menu_items(field),
                state,
            )
            .focus(&self.menu_focus)
            .on_highlight(move |index, _, cx| {
                highlight.update(cx, |panel, cx| {
                    if let Some((_, state)) = &mut panel.menu {
                        state.set_highlighted(*index);
                        cx.notify();
                    }
                });
            })
            .on_select(move |index, window, cx| {
                select.update(cx, |panel, cx| {
                    panel.close_menu(window, cx);
                    if let Some(option) = options.get(*index) {
                        panel.set_field(&name, Some(Value::String(option.clone())), cx);
                    }
                });
            })
            .on_dismiss(move |window, cx| {
                dismiss.update(cx, |panel, cx| panel.close_menu(window, cx))
            })
            .into_any_element(),
        )
    }
}

impl Render for PropsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme.refresh(window, cx);
        let theme = self.theme;
        let root = div()
            .id("props-panel")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll();
        let Some(snapshot) = self.snapshot.clone() else {
            return root.child(PanelSection::new(&theme, "Props").child(
                EmptyState::new(&theme, "No project open").body("Open a project to edit props."),
            ));
        };
        let enabled = snapshot.editing == PropsEditing::Enabled;
        let mut history = div().flex().gap(px(METRICS.space_1));
        for (id, glyph, label, possible, command) in [
            (
                "props-undo",
                Icon::Undo,
                "Undo props edit",
                snapshot.can_undo,
                PropsCommand::Undo,
            ),
            (
                "props-redo",
                Icon::Redo,
                "Redo props edit",
                snapshot.can_redo,
                PropsCommand::Redo,
            ),
        ] {
            let entity = cx.entity();
            history = history.child(
                IconButton::new(&theme, id, glyph.path())
                    .size(IconButtonSize::Sm)
                    .label(label)
                    .tooltip(label)
                    .disabled(!(enabled && possible))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |panel, cx| panel.run(command.clone(), cx));
                    }),
            );
        }
        let mut section = PanelSection::new(&theme, "Props").trailing(history);
        if let PropsEditing::Disabled(reason) = &snapshot.editing {
            section = section.child(
                Notice::new("props-disabled", Tone::Warning, "Props edits are disabled")
                    .body(reason.clone()),
            );
        }
        if let Some(notice) = &snapshot.notice {
            section = section.child(Notice::new("props-notice", Tone::Info, notice.clone()));
        }
        if let Some(error) = &self.error {
            section = section.child(Notice::new("props-error", Tone::Warning, error.clone()));
        }
        let (description, rows) = self.body(&snapshot, enabled, window, cx);
        if let Some(description) = description {
            section = section.description(description);
        }
        let mut root = root.child(section.children(rows));
        if self.dragging.is_some() {
            // The slider's or picker's release can land anywhere in the window.
            let entity = cx.entity();
            root = root.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let up = entity.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture && event.button == MouseButton::Left
                            {
                                up.update(cx, |panel, cx| panel.end_drag(cx));
                            }
                        });
                        // A release outside the window arrives as a move without the button;
                        // a move with it held drags the picker.
                        let moved = entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            moved.update(cx, |panel, cx| {
                                if event.pressed_button != Some(MouseButton::Left) {
                                    panel.end_drag(cx);
                                } else if let Some(part) =
                                    panel.picker.as_ref().and_then(|picker| picker.part)
                                {
                                    panel.pick(part, event.position, cx);
                                }
                            });
                        });
                    },
                )
                .absolute()
                .size_0(),
            );
        }
        root.children(self.picker_layer(window, cx))
            .children(self.menu_layer(cx))
    }
}

/// The most options a segmented control shows; more go into a select.
const MAX_SEGMENTS: usize = 4;

/// Which part of the colour picker a drag moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerPart {
    Field,
    Hue,
}

/// The open colour picker: the field it edits, where it is, and the alpha the value
/// had, which the picker keeps.
struct Picker {
    field: String,
    snapshot: ColorPickerSnapshot,
    alpha: Option<u8>,
    /// The part a drag that began on the picker moves.
    part: Option<PickerPart>,
}

/// Fields edited by typing: numbers, text and colours.
fn typed(kind: &PropKind) -> bool {
    matches!(
        kind,
        PropKind::Number | PropKind::Integer | PropKind::Text | PropKind::Color
    )
}

/// How a drawn value reads in its text field.
pub fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Reads a text entry as the field's kind, checking its range.
pub fn parse_entry(field: &PropField, text: &str) -> Result<Value, String> {
    let text = text.trim();
    let number = |value: f64| {
        if field.min.is_some_and(|min| value < min) || field.max.is_some_and(|max| value > max) {
            Err(format!(
                "must be between {} and {}",
                field.min.map_or("-∞".into(), |v| v.to_string()),
                field.max.map_or("∞".into(), |v| v.to_string())
            ))
        } else {
            Ok(value)
        }
    };
    match &field.kind {
        PropKind::Number => {
            let value = text
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or("expected a number")?;
            let value = number(value)?;
            serde_json::Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| "expected a number".into())
        }
        PropKind::Integer => {
            let value = text.parse::<i64>().map_err(|_| "expected a whole number")?;
            number(value as f64)?;
            Ok(Value::from(value))
        }
        PropKind::Text => Ok(Value::String(text.to_owned())),
        PropKind::Color => parse_hex(text)
            .map(|_| Value::String(text.to_ascii_lowercase()))
            .ok_or_else(|| "expected #rrggbb or #rrggbbaa".into()),
        PropKind::Toggle | PropKind::Choice { .. } => Err("not a text field".into()),
    }
}

/// `#rrggbb` or `#rrggbbaa`: the colour and the alpha when it is written.
fn parse_hex(text: &str) -> Option<([u8; 3], Option<u8>)> {
    let hex = text.strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let alpha = if hex.len() == 8 { Some(byte(6)?) } else { None };
    Some(([byte(0)?, byte(2)?, byte(4)?], alpha))
}

/// The value a picked colour is written as: `#rrggbb`, or `#rrggbbaa` keeping the
/// alpha the value had.
fn hex([r, g, b]: [u8; 3], alpha: Option<u8>) -> String {
    match alpha {
        Some(a) => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
        None => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

/// The swatch's fill for a parsed value.
fn swatch_color([r, g, b]: [u8; 3], alpha: Option<u8>) -> gpui::Rgba {
    let channel = |c: u8| f32::from(c) / 255.;
    gpui::Rgba {
        r: channel(r),
        g: channel(g),
        b: channel(b),
        a: channel(alpha.unwrap_or(u8::MAX)),
    }
}

/// The value a slider of `field` at `fraction` stands for: snapped to the step, whole
/// for an integer.
pub fn slider_value(field: &PropField, min: f64, max: f64, fraction: f32) -> Value {
    let mut value = min + f64::from(fraction.clamp(0., 1.)) * (max - min);
    if let Some(step) = field.step.filter(|step| *step > 0.) {
        value = (min + ((value - min) / step).round() * step).clamp(min, max);
    }
    if field.kind == PropKind::Integer {
        Value::from(value.round() as i64)
    } else {
        // Two decimals are what a slider can aim at.
        let value = (value * 100.).round() / 100.;
        serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[gpui::test]
    fn choice_menu_restores_its_trigger_after_escape_and_commit(cx: &mut gpui::TestAppContext) {
        use fframes_studio_protocol::PropsSchema;

        use crate::kit::test_support::{log, press, tab};

        let (panel, cx) = cx.add_window_view(|window, cx| {
            let mut panel = PropsPanel::new(window, cx);
            panel.snapshot = Some(PropsSnapshot {
                generation: 0,
                props_json: None,
                doc: None,
                can_undo: false,
                can_redo: false,
                editing: PropsEditing::Enabled,
                notice: None,
            });
            panel.selected = Some(EditorObjectIdentity {
                scene_instance_key: "scene".into(),
                component_key: "Card".into(),
                object_key: "card".into(),
                repeat_key: String::new(),
            });
            panel.info = Some(Ok(Some(PropsInfo {
                schema: PropsSchema {
                    component: "Card".into(),
                    fields: vec![field(
                        PropKind::Choice {
                            options: ["a", "b", "c", "d", "e"].map(String::from).to_vec(),
                        },
                        None,
                        None,
                    )],
                },
                code_values: Default::default(),
                values: Default::default(),
            })));
            panel
        });
        let events = log();
        let observed = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&panel, move |_, event: &PropsEvent, _| {
                observed.borrow_mut().push(event.clone());
            })
            .detach();
        });
        tab(cx);
        let trigger = cx.update(|window, cx| window.focused(cx).expect("choice trigger focused"));
        press(cx, "enter");
        assert!(panel.read_with(cx, |panel, _| panel.menu.is_some()));
        press(cx, "escape");
        assert!(cx.update(|window, _| trigger.is_focused(window)));
        assert!(events.borrow().is_empty(), "Escape must not edit props");

        press(cx, "enter");
        assert!(panel.read_with(cx, |panel, _| panel.menu.is_some()));
        press(cx, "down");
        press(cx, "enter");
        assert!(panel.read_with(cx, |panel, _| panel.menu.is_none()));
        assert!(cx.update(|window, _| trigger.is_focused(window)));
        assert!(matches!(
            events.borrow().as_slice(),
            [PropsEvent::Run(PropsCommand::Set { field, value: Some(Value::String(_)), .. })]
                if field == "size"
        ));
        press(cx, "enter");
        assert!(panel.read_with(cx, |panel, _| panel.menu.is_some()));
    }

    fn field(kind: PropKind, min: Option<f64>, max: Option<f64>) -> PropField {
        PropField {
            name: "size".into(),
            kind,
            min,
            max,
            step: None,
            unit: None,
        }
    }

    #[test]
    fn entries_are_read_as_their_kind_and_range() {
        let number = field(PropKind::Number, Some(0.), Some(100.));
        assert_eq!(parse_entry(&number, " 42.5 ").unwrap(), json!(42.5));
        assert!(
            parse_entry(&number, "120")
                .unwrap_err()
                .contains("between 0 and 100")
        );
        assert!(parse_entry(&number, "big").is_err());
        let integer = field(PropKind::Integer, None, Some(10.));
        assert_eq!(parse_entry(&integer, "7").unwrap(), json!(7));
        assert!(parse_entry(&integer, "7.5").is_err());
        assert!(parse_entry(&integer, "11").is_err());
        let text = field(PropKind::Text, None, None);
        assert_eq!(parse_entry(&text, "Hello").unwrap(), json!("Hello"));
        let color = field(PropKind::Color, None, None);
        assert_eq!(parse_entry(&color, "#FF5500").unwrap(), json!("#ff5500"));
        assert_eq!(
            parse_entry(&color, "#ff550080").unwrap(),
            json!("#ff550080")
        );
        assert!(parse_entry(&color, "ff5500").is_err());
        assert!(parse_entry(&color, "#ff55").is_err());
    }

    #[test]
    fn values_read_back_into_their_fields() {
        assert_eq!(display_value(&json!("Hi")), "Hi");
        assert_eq!(display_value(&json!(3.5)), "3.5");
        assert_eq!(display_value(&json!(true)), "true");
        assert_eq!(display_value(&Value::Null), "");
    }

    #[test]
    fn sliders_snap_to_the_step_and_stay_whole_for_integers() {
        let mut stepped = field(PropKind::Number, Some(0.), Some(10.));
        stepped.step = Some(2.5);
        assert_eq!(slider_value(&stepped, 0., 10., 0.3), json!(2.5));
        assert_eq!(slider_value(&stepped, 0., 10., 1.), json!(10.0));
        let integer = field(PropKind::Integer, Some(0.), Some(9.));
        assert_eq!(slider_value(&integer, 0., 9., 0.5), json!(5));
        let plain = field(PropKind::Number, Some(0.), Some(1.));
        assert_eq!(slider_value(&plain, 0., 1., 0.333_333), json!(0.33));
    }

    #[test]
    fn picked_colours_keep_the_alpha_the_value_had() {
        assert_eq!(parse_hex("#ff5500"), Some(([255, 85, 0], None)));
        assert_eq!(parse_hex("#FF550080"), Some(([255, 85, 0], Some(0x80))));
        assert_eq!(parse_hex("#ff55"), None);
        assert_eq!(parse_hex("#gg5500"), None);
        assert_eq!(hex([255, 85, 0], None), "#ff5500");
        assert_eq!(hex([255, 85, 0], Some(0x80)), "#ff550080");
    }
}
