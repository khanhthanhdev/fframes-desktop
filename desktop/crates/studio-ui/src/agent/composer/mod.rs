//! The agent prompt composer: a multiline input, slash command completion,
//! attachment chips and the send, steer and stop controls.
//!
//! The component owns what the user is typing and nothing else. It reads no
//! file, calls no agent and decides no policy. Every action leaves as a
//! [`ComposerEvent`] for its owner, and the owner pushes state back in:
//! the command list, whether a turn is running, how the agent takes a message
//! mid-turn ([`SteerMode`]), and the chips it accepted.
//!
//! ## Confinement is the owner's job
//!
//! Pressing attach, pasting an image or pasting copied files emits
//! [`ComposerEvent::AttachRequested`]. The component checks nothing on disk. The
//! owner decides whether a path is inside the project, whether the deny list
//! applies, and whether an image is within the size cap, then calls
//! [`Composer::add_attachment`] with what it accepted. A chip is a label and
//! an optional thumbnail; it carries no path.
//!
//! The owner calls [`keymap::init`] once at start so the input's key bindings
//! exist.

pub mod attachments;
mod input;
pub mod keymap;
pub mod metrics;
pub mod slash;
pub mod steer;
#[cfg(test)]
mod tests;

use std::{path::PathBuf, rc::Rc, sync::Arc};

use gpui::{
    App, Bounds, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    IntoElement, ParentElement, Pixels, Render, SharedString, Styled, Subscription, Window, div,
    prelude::*, px,
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

pub use attachments::{
    Attachment, AttachmentDraft, AttachmentId, AttachmentKind, Attachments, ContextChip,
};
pub use input::{ComposerInput, ComposerInputEvent};
pub use slash::SlashCommand;
pub use steer::{SendAction, SteerMode};

use crate::kit::{
    Emphasis, Icon, IconButton, IconButtonSize, Popover, Select, dim_disabled, focusable,
};
use crate::probe::{self, BoundsSink};
use slash::SlashState;
use steer::steer_actions;

/// The tallest the slash menu grows before it scrolls.
const SLASH_MAX_H: f32 = 240.;
/// The composer's border while its input has focus: `accent` at this alpha.
const FOCUS_BORDER_ALPHA: f32 = 0.6;

/// Opens the agent's model menu under the trigger's bounds.
pub type ModelMenuHandler = Rc<dyn Fn(&Bounds<Pixels>, &mut Window, &mut App)>;

/// The model the next message goes to, shown as a quiet select beside Send when the agent
/// advertises a model option.
#[derive(Clone)]
pub struct ModelChoice {
    pub label: SharedString,
    pub on_open: ModelMenuHandler,
}

/// What the user wants to send: the text and the chips that went with it.
#[derive(Clone)]
pub struct Submission {
    pub text: String,
    pub attachments: Vec<Attachment>,
}

/// What the user asked to attach. Nothing has been read or checked.
#[derive(Clone)]
pub enum AttachRequest {
    /// The attach-file button: open a file picker.
    PickFiles,
    /// The attach-image button: open an image picker.
    PickImages,
    /// Files copied in a file manager and pasted.
    Paths(Vec<PathBuf>),
    /// Image data on the clipboard, such as a screenshot.
    Images(Vec<Arc<gpui::Image>>),
}

/// The kinds of attachment the owner can switch off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachKind {
    File,
    Image,
}

/// What the composer tells its owner.
#[derive(Clone)]
pub enum ComposerEvent {
    /// Send, with no turn running. The draft is already cleared; call
    /// [`Composer::restore`] if the send is refused.
    Submit(Submission),
    /// Send while a turn runs, to be delivered at the next boundary.
    SendAtBoundary(Submission),
    /// Send while a turn runs, to be handed to the running turn.
    Steer(Submission),
    /// Cancel the running turn, then send.
    SendNow(Submission),
    /// The stop button.
    Stop,
    AttachRequested(AttachRequest),
    /// A chip's remove button was pressed and the chip is gone, or the owner switched
    /// its kind off and the composer dropped it.
    AttachmentRemoved(AttachmentId),
    /// Something of a switched-off kind was pasted, or chips of a kind that was just
    /// switched off were dropped: the owner shows the reason.
    AttachBlocked {
        kind: AttachKind,
        reason: SharedString,
    },
}

fn chip_kind(chip: &Attachment) -> AttachKind {
    match chip.draft.kind {
        AttachmentKind::File => AttachKind::File,
        AttachmentKind::Image => AttachKind::Image,
    }
}

/// The composer view.
pub struct Composer {
    theme: Theme,
    input: Entity<ComposerInput>,
    commands: Vec<SlashCommand>,
    slash: SlashState,
    attachments: Attachments,
    context: Option<ContextChip>,
    steer_mode: SteerMode,
    running: bool,
    /// Why each kind of attachment is off, when it is.
    file_disabled: Option<SharedString>,
    image_disabled: Option<SharedString>,
    bounds_sink: Option<BoundsSink>,
    /// The model select beside Send, when the agent advertises one.
    model: Option<ModelChoice>,
    /// The send button's menu (Send now) is open.
    send_menu_open: bool,
    send_menu_focus: FocusHandle,
    _input_events: Subscription,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Focusable for Composer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Composer {
    pub fn new(placeholder: impl Into<SharedString>, theme: Theme, cx: &mut Context<Self>) -> Self {
        let placeholder = placeholder.into();
        let input = cx.new(|cx| ComposerInput::new(placeholder, theme, cx));
        let subscription = cx.subscribe(&input, |this, _, event: &ComposerInputEvent, cx| {
            this.on_input_event(event, cx);
        });
        Self {
            theme,
            input,
            commands: Vec::new(),
            slash: SlashState::default(),
            attachments: Attachments::default(),
            context: None,
            steer_mode: SteerMode::None,
            running: false,
            file_disabled: None,
            image_disabled: None,
            bounds_sink: None,
            model: None,
            send_menu_open: false,
            send_menu_focus: cx.focus_handle(),
            _input_events: subscription,
        }
    }

    /// Shows the model select beside Send, or hides it with `None`.
    pub fn set_model(&mut self, model: Option<ModelChoice>, cx: &mut Context<Self>) {
        self.model = model;
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        self.input
            .update(cx, |input, cx| input.set_theme(theme, cx));
        cx.notify();
    }

    /// The commands the agent advertises, shown when the user types `/`.
    pub fn set_commands(&mut self, commands: Vec<SlashCommand>, cx: &mut Context<Self>) {
        self.commands = commands;
        self.refresh_slash(cx);
    }

    /// Whether a turn is running. While it is, sending follows the steer mode
    /// and the stop button shows.
    pub fn set_running(&mut self, running: bool, cx: &mut Context<Self>) {
        if self.running != running {
            self.running = running;
            cx.notify();
        }
    }

    pub fn set_steer_mode(&mut self, mode: SteerMode, cx: &mut Context<Self>) {
        if self.steer_mode != mode {
            self.steer_mode = mode;
            cx.notify();
        }
    }

    /// Switches an attachment kind off with the reason to show, or back on
    /// with `None`. A disabled kind neither shows an enabled button nor emits
    /// [`ComposerEvent::AttachRequested`] from a paste.
    pub fn set_attach_disabled(
        &mut self,
        kind: AttachKind,
        reason: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let slot = match kind {
            AttachKind::File => &mut self.file_disabled,
            AttachKind::Image => &mut self.image_disabled,
        };
        if *slot == reason {
            return;
        }
        slot.clone_from(&reason);
        // Chips of a kind the agent cannot take must not ride along with the next send.
        if let Some(reason) = reason {
            let dropped: Vec<AttachmentId> = self
                .attachments
                .items()
                .iter()
                .filter(|chip| chip_kind(chip) == kind)
                .map(|chip| chip.id)
                .collect();
            for id in &dropped {
                self.attachments.remove(*id);
                cx.emit(ComposerEvent::AttachmentRemoved(*id));
            }
            if !dropped.is_empty() {
                cx.emit(ComposerEvent::AttachBlocked { kind, reason });
            }
        }
        cx.notify();
    }

    /// Shows the timeline selection that goes with the next message as the first chip,
    /// or none.
    pub fn set_context(&mut self, context: Option<ContextChip>, cx: &mut Context<Self>) {
        if self.context != context {
            self.context = context;
            cx.notify();
        }
    }

    /// The selection chip shown, if any.
    pub fn context(&self) -> Option<&ContextChip> {
        self.context.as_ref()
    }

    /// Shows a chip the owner accepted.
    pub fn add_attachment(
        &mut self,
        draft: AttachmentDraft,
        cx: &mut Context<Self>,
    ) -> AttachmentId {
        let id = self.attachments.add(draft);
        cx.notify();
        id
    }

    /// Removes a chip without telling the owner, as when the owner drops it.
    pub fn remove_attachment(&mut self, id: AttachmentId, cx: &mut Context<Self>) -> bool {
        let removed = self.attachments.remove(id);
        if removed {
            cx.notify();
        }
        removed
    }

    /// Drops every chip without telling the owner, as when the conversation changed.
    pub fn clear_attachments(&mut self, cx: &mut Context<Self>) {
        if !self.attachments.is_empty() {
            self.attachments.take();
            cx.notify();
        }
    }

    pub fn attachments(&self) -> &[Attachment] {
        self.attachments.items()
    }

    /// The draft text.
    pub fn text(&self, cx: &App) -> String {
        self.input.read(cx).text().to_string()
    }

    /// Replaces the draft text. Attachments stay.
    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        self.input.update(cx, |input, cx| input.set_text(text, cx));
    }

    /// Puts a refused submission back: its text and chips under their old ids.
    pub fn restore(&mut self, submission: Submission, cx: &mut Context<Self>) {
        self.attachments.restore(submission.attachments);
        self.set_text(submission.text, cx);
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.input.focus_handle(cx), cx);
    }

    /// The placeholder shown while the draft is empty.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        self.input
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
    }

    /// Where the composer reports the bounds of its buttons, the slash rows and the
    /// input (composer-input), for owners that drive the window with real pointer events.
    pub fn set_bounds_sink(&mut self, sink: BoundsSink) {
        self.bounds_sink = Some(sink);
    }

    /// The Enter gesture: sends as the primary action of the current state. Does nothing
    /// when the agent cannot take a message now or the draft is empty.
    pub fn send_primary(&mut self, cx: &mut Context<Self>) {
        if let Some(action) = steer_actions(self.steer_mode, self.running).primary {
            self.trigger(action, cx);
        }
    }

    /// Whether the draft has any text or chip to send.
    pub fn has_draft(&self, cx: &App) -> bool {
        self.can_send(cx)
    }

    /// The reason each attachment kind is off, if it is.
    pub fn attach_disabled(&self, kind: AttachKind) -> Option<&SharedString> {
        match kind {
            AttachKind::File => self.file_disabled.as_ref(),
            AttachKind::Image => self.image_disabled.as_ref(),
        }
    }

    /// The names of the commands the slash menu lists now, in row order; empty while it is
    /// closed.
    pub fn slash_rows(&self) -> Vec<SharedString> {
        if !self.slash.is_open() {
            return Vec::new();
        }
        self.slash
            .rows()
            .iter()
            .filter_map(|index| self.commands.get(*index))
            .map(|command| command.name.clone())
            .collect()
    }

    fn on_input_event(&mut self, event: &ComposerInputEvent, cx: &mut Context<Self>) {
        match event {
            ComposerInputEvent::Submitted => self.send_primary(cx),
            ComposerInputEvent::SubmittedNow => {
                let action = steer_actions(self.steer_mode, self.running).now;
                self.trigger(action, cx);
            }
            ComposerInputEvent::Edited | ComposerInputEvent::CursorMoved => {
                self.refresh_slash(cx);
                cx.notify();
            }
            ComposerInputEvent::ViewportChanged => cx.notify(),
            ComposerInputEvent::CompletionNavigate(delta) => {
                self.slash.navigate(*delta);
                self.sync_completion_controls(cx);
                cx.notify();
            }
            ComposerInputEvent::CompletionAccept => self.accept_selected(cx),
            ComposerInputEvent::CompletionDismiss => {
                let text = self.text(cx);
                self.slash.dismiss(&text);
                self.sync_completion_controls(cx);
                cx.notify();
            }
            ComposerInputEvent::PastedImages(images) => match self.image_disabled.clone() {
                None => {
                    let images = images.iter().cloned().map(Arc::new).collect();
                    cx.emit(ComposerEvent::AttachRequested(AttachRequest::Images(
                        images,
                    )));
                }
                Some(reason) => cx.emit(ComposerEvent::AttachBlocked {
                    kind: AttachKind::Image,
                    reason,
                }),
            },
            ComposerInputEvent::PastedPaths(paths) => match self.file_disabled.clone() {
                None => {
                    cx.emit(ComposerEvent::AttachRequested(AttachRequest::Paths(
                        paths.clone(),
                    )));
                }
                Some(reason) => cx.emit(ComposerEvent::AttachBlocked {
                    kind: AttachKind::File,
                    reason,
                }),
            },
        }
    }

    /// Whether there is anything to send.
    fn can_send(&self, cx: &App) -> bool {
        !self.input.read(cx).text().trim().is_empty() || !self.attachments.is_empty()
    }

    /// Sends the draft as `action`. Does nothing for an empty draft.
    fn trigger(&mut self, action: SendAction, cx: &mut Context<Self>) {
        if !self.can_send(cx) {
            return;
        }
        self.send_menu_open = false;
        let text = self.text(cx).trim().to_string();
        self.input.update(cx, |input, cx| input.set_text("", cx));
        // A chip of a switched-off kind never travels, whatever put it there.
        let (file_off, image_off) = (self.file_disabled.is_some(), self.image_disabled.is_some());
        let mut chips = self.attachments.take();
        chips.retain(|chip| match chip_kind(chip) {
            AttachKind::File => !file_off,
            AttachKind::Image => !image_off,
        });
        let submission = Submission {
            text,
            attachments: chips,
        };
        cx.emit(match action {
            SendAction::Submit => ComposerEvent::Submit(submission),
            SendAction::SendAtBoundary => ComposerEvent::SendAtBoundary(submission),
            SendAction::Steer => ComposerEvent::Steer(submission),
            SendAction::SendNow => ComposerEvent::SendNow(submission),
        });
        cx.notify();
    }

    fn refresh_slash(&mut self, cx: &mut Context<Self>) {
        let (text, cursor) = {
            let input = self.input.read(cx);
            (input.text().to_string(), input.cursor_offset())
        };
        self.slash.refresh(&text, cursor, &self.commands);
        self.sync_completion_controls(cx);
    }

    /// Tells the input whether the menu is open, so Enter, Tab and the arrows
    /// go to it.
    fn sync_completion_controls(&mut self, cx: &mut Context<Self>) {
        let open = self.slash.is_open();
        let selected = open && self.slash.selected_command().is_some();
        self.input.update(cx, |input, cx| {
            input.set_completion_controls(open, selected, cx)
        });
    }

    fn accept_selected(&mut self, cx: &mut Context<Self>) {
        let Some(command) = self.slash.selected_command() else {
            return;
        };
        let text = self.text(cx);
        let Some((range, replacement)) = self.slash.insertion(&text, &self.commands, command)
        else {
            return;
        };
        self.input.update(cx, |input, cx| {
            input.replace_range_as_step(range, &replacement, cx)
        });
    }

    fn remove_from_strip(&mut self, id: AttachmentId, cx: &mut Context<Self>) {
        if self.attachments.remove(id) {
            cx.emit(ComposerEvent::AttachmentRemoved(id));
            cx.notify();
        }
    }

    /// The stop button. Only meaningful while a turn runs.
    fn stop(&mut self, cx: &mut Context<Self>) {
        if self.running {
            cx.emit(ComposerEvent::Stop);
        }
    }

    fn request_attach(&mut self, request: AttachRequest, cx: &mut Context<Self>) {
        cx.emit(ComposerEvent::AttachRequested(request));
    }

    fn slash_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.slash.is_open() {
            return None;
        }
        let theme = self.theme;
        let colors = theme.colors;
        let active = self.slash.active_row();
        let rows = self
            .slash
            .rows()
            .iter()
            .enumerate()
            .filter_map(|(row, index)| self.commands.get(*index).map(|command| (row, command)))
            .map(|(row, command)| {
                div()
                    .id(("slash-row", row))
                    .relative()
                    .flex_none()
                    .h(px(METRICS.size.control_h))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(METRICS.space_2))
                    .px(px(METRICS.space_2))
                    .rounded(crate::agent::ui::ITEM_RADIUS)
                    .cursor_pointer()
                    .when(active == Some(row), |row| row.bg(colors.ctl_hover))
                    .hover(move |style| style.bg(colors.ctl_hover))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.slash.set_active_row(row);
                        this.accept_selected(cx);
                    }))
                    .child(
                        text_style(div().flex_none(), TextStyle::Label)
                            .text_color(colors.text_1)
                            .child(format!("/{}", command.name)),
                    )
                    .child(
                        text_style(div().min_w_0().truncate(), TextStyle::Small)
                            .text_color(colors.text_3)
                            .child(command.description.clone()),
                    )
                    .children(probe::probe_element(
                        &self.bounds_sink,
                        format!("composer-slash-row-{row}"),
                    ))
            })
            .collect::<Vec<_>>();
        // The kit `Menu`'s panel and rows. The menu itself takes the keyboard and closes on a
        // click anywhere, which would steal the caret from the input that drives this one.
        Some(
            Popover::new(&theme)
                .id("composer-slash-menu")
                .left_0()
                .right_0()
                .bottom_full()
                .mb(px(METRICS.space_2))
                .max_h(px(SLASH_MAX_H))
                .overflow_y_scroll()
                .p(px(METRICS.space_1))
                .children(rows),
        )
    }

    fn close_send_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.send_menu_open = false;
        self.focus(window, cx);
        cx.notify();
    }

    /// The menu of the send button while a turn runs: Send now, which cancels the turn.
    fn send_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !(self.send_menu_open && self.running) {
            return None;
        }
        let theme = self.theme;
        let colors = theme.colors;
        let now = steer_actions(self.steer_mode, self.running).now;
        let sendable = self.can_send(cx);
        let row = focusable(div().id("composer-send-now"), &theme, !sendable)
            .track_focus(&self.send_menu_focus)
            .relative()
            .h(px(METRICS.size.control_h))
            .flex()
            .items_center()
            .px(px(METRICS.space_2))
            .rounded(crate::agent::ui::ITEM_RADIUS)
            .map(|row| {
                if sendable {
                    row.cursor_pointer()
                        .hover(move |style| style.bg(colors.ctl_hover))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_send_menu(window, cx);
                            this.trigger(now, cx);
                        }))
                } else {
                    dim_disabled(row, true)
                }
            })
            .child(
                text_style(div(), TextStyle::Label)
                    .text_color(colors.text_1)
                    .child(now.label()),
            )
            .children(probe::probe_element(&self.bounds_sink, "composer-send-now"));
        Some(
            Popover::new(&theme)
                .id("composer-send-menu")
                .right(px(METRICS.space_2))
                .bottom_full()
                .mb(px(METRICS.space_1))
                .p(px(METRICS.space_1))
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_send_menu(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(row),
        )
    }

    fn controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let actions = steer_actions(self.steer_mode, self.running);
        let sendable = self.can_send(cx);
        let file_button = IconButton::new(&theme, "composer-attach-file", Icon::File.path())
            .size(IconButtonSize::Sm)
            .emphasis(Emphasis::Ghost)
            .label("Attach file")
            .tooltip(
                self.file_disabled
                    .clone()
                    .unwrap_or_else(|| "Attach file".into()),
            )
            .disabled(self.file_disabled.is_some())
            .on_click(
                cx.listener(|this, _, _, cx| this.request_attach(AttachRequest::PickFiles, cx)),
            );
        let file_button = match probe::report_to(&self.bounds_sink, "composer-attach-file") {
            Some(report) => file_button.on_bounds(report),
            None => file_button,
        };
        let image_button = IconButton::new(&theme, "composer-attach-image", Icon::Image.path())
            .size(IconButtonSize::Sm)
            .emphasis(Emphasis::Ghost)
            .label("Attach image")
            .tooltip(
                self.image_disabled
                    .clone()
                    .unwrap_or_else(|| "Attach image".into()),
            )
            .disabled(self.image_disabled.is_some())
            .on_click(
                cx.listener(|this, _, _, cx| this.request_attach(AttachRequest::PickImages, cx)),
            );
        let image_button = match probe::report_to(&self.bounds_sink, "composer-attach-image") {
            Some(report) => image_button.on_bounds(report),
            None => image_button,
        };

        let mut right = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(METRICS.space_1));
        if let Some(model) = &self.model {
            let open = model.on_open.clone();
            right = right.child(
                Select::new(&theme, "composer-model", model.label.clone())
                    .ghost()
                    .on_open(move |bounds, window, cx| open(bounds, window, cx)),
            );
        }
        if self.running {
            right = right.child(
                IconButton::new(
                    &theme,
                    "composer-send-menu-toggle",
                    Icon::ChevronDown.path(),
                )
                .size(IconButtonSize::Sm)
                .emphasis(Emphasis::Ghost)
                .pressed(self.send_menu_open)
                .label("More ways to send")
                .tooltip("More ways to send")
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.send_menu_open {
                        this.close_send_menu(window, cx);
                    } else {
                        this.send_menu_open = true;
                        window.focus(&this.send_menu_focus, cx);
                        cx.notify();
                    }
                })),
            );
        }
        if let Some(primary) = actions.primary {
            let send = IconButton::new(&theme, "composer-send", Icon::ArrowUp.path())
                .size(IconButtonSize::Md)
                .round()
                .emphasis(Emphasis::Primary)
                .label(primary.label())
                .tooltip(primary.label())
                .disabled(!sendable)
                .on_click(cx.listener(move |this, _, _, cx| this.trigger(primary, cx)));
            right = right.child(match probe::report_to(&self.bounds_sink, "composer-send") {
                Some(report) => send.on_bounds(report),
                None => send,
            });
        }
        if self.running {
            let stop = IconButton::new(&theme, "composer-stop", Icon::Square.path())
                .size(IconButtonSize::Md)
                .inverted()
                .label("Stop")
                .tooltip("Stop")
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)));
            right = right.child(match probe::report_to(&self.bounds_sink, "composer-stop") {
                Some(report) => stop.on_bounds(report),
                None => stop,
            });
        }

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px(px(METRICS.space_2))
            .pb(px(METRICS.space_2))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(METRICS.space_1))
                    .child(file_button)
                    .child(image_button),
            )
            .child(right)
    }

    /// What Send does while a turn runs, under the composer: only while there is something
    /// to send, or when the agent cannot take a message at all.
    fn running_hint(&self, cx: &App) -> Option<&'static str> {
        if !self.running {
            return None;
        }
        let actions = steer_actions(self.steer_mode, self.running);
        match actions.primary {
            None => actions.hint,
            Some(_) if !self.can_send(cx) => None,
            Some(SendAction::SendAtBoundary) => Some("Sends after this step"),
            Some(SendAction::Steer) => Some("Steers the current step"),
            Some(_) => None,
        }
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let colors = theme.colors;
        if !self.running && self.send_menu_open {
            if self.send_menu_focus.is_focused(window) {
                self.close_send_menu(window, cx);
            } else {
                self.send_menu_open = false;
            }
        }
        let weak = cx.entity().downgrade();
        let on_remove: attachments::RemoveHandler = Rc::new(move |id, _, cx| {
            weak.update(cx, |this, cx| this.remove_from_strip(id, cx))
                .ok();
        });
        let focused = self.input.focus_handle(cx).is_focused(window);
        let border = if focused {
            Hsla::from(colors.accent).opacity(FOCUS_BORDER_ALPHA)
        } else {
            colors.line.into()
        };
        let hint = self.running_hint(cx);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .child(
                div()
                    .id("composer")
                    .relative()
                    .flex()
                    .flex_col()
                    .w_full()
                    .rounded(crate::agent::ui::CARD_RADIUS)
                    .border_1()
                    .border_color(border)
                    .bg(colors.card)
                    .children(self.slash_menu(cx))
                    .children(self.send_menu(cx))
                    .children(attachments::strip(
                        &self.attachments,
                        self.context.as_ref(),
                        &theme,
                        on_remove,
                    ))
                    .child(
                        div()
                            .relative()
                            .px(px(METRICS.space_3))
                            .py(px(METRICS.space_2))
                            .child(self.input.clone())
                            .children(probe::probe_element(&self.bounds_sink, "composer-input")),
                    )
                    .child(self.controls(cx)),
            )
            .children(hint.map(|hint| {
                text_style(div().px(px(METRICS.space_3)), TextStyle::Small)
                    .text_color(colors.text_3)
                    .child(hint)
            }))
    }
}
