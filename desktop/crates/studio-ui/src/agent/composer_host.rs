//! The input side of the agent panel: the composer, the plan list and the session list,
//! and everything that moves between them and the workflow.
//!
//! The components own what the user is typing and nothing else. This module pushes the
//! snapshot's composer state into them ([`ConversationPanel::sync_composer`]), turns their
//! events into workflow calls and puts a refused message back, so a refusal never costs
//! the user their text or chips. Attachments are prepared in [`super::attachment_flow`],
//! agent questions answered in [`super::question_card`], sessions listed and opened in
//! [`super::history_host`].

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use gpui::{App, Focusable, KeyDownEvent, prelude::FluentBuilder as _};
use studio_engine::agent::{
    CommandInfo, OpenQuestion, PreparedAttachment, StagedAttachment, SteerSupport, WorkflowError,
};

use super::composer::{
    AttachKind, AttachmentId, Composer, ComposerControl, ComposerEvent, SlashCommand, SteerMode,
    Submission,
};
use super::composer_tools::{
    ApprovalPopover, ToolsPopover, approval_menu, policy_name, policy_short_label, tools_menu,
    tools_rows,
};
use super::history::{HistoryEvent, HistoryList};
use super::question_card::ItemDraft;
use super::todo_panel::{TodoItem, TodoPanel};
use super::ui::{ActionCard, STATUS_MARK, TrayHeader, TrayRow, tray_frame};
use super::*;
use crate::kit::icon;
use crate::probe::BoundsSink;

/// Why image chips are off while the agent has not said it takes images.
pub(super) const IMAGES_OFF: &str =
    "This agent has not advertised image prompts (check the agent in setup).";
/// Why file chips are off while the agent takes no files.
pub(super) const FILES_OFF: &str = "This agent cannot take file attachments.";

/// An outside file the engine checked and the user has not confirmed yet.
pub(super) struct Staged {
    pub staged: StagedAttachment,
    /// When the engine issued the token; it expires, so an old one is staged again.
    pub at: Instant,
}

/// The entities and the bookkeeping the composer side of the panel keeps.
pub(super) struct ComposerHost {
    pub composer: Entity<Composer>,
    pub todo: Entity<TodoPanel>,
    pub history: Entity<HistoryList>,
    /// What the engine accepted for each chip. A send takes copies, so a refused send
    /// still has them when the chips come back.
    pub prepared: HashMap<AttachmentId, PreparedAttachment>,
    /// Counts conversations; an attachment answer from an older one is dropped.
    pub generation: u64,
    /// Attachment checks in flight.
    pub preparing: usize,
    pub stage: VecDeque<Staged>,
    /// The last `unsent` the engine showed, so only a change acts.
    pub last_unsent: Option<String>,
    /// A message the agent refused while the composer held something new; the user
    /// decides what to do with it.
    pub refused: Option<String>,
    pub last_commands: Vec<CommandInfo>,
    /// The engine's last reason for refusing an attachment.
    pub last_refusal: Option<String>,
    pub questions: HashMap<studio_engine::agent::PermissionRef, Vec<ItemDraft>>,
    /// Counts session list requests; only the newest answer is shown.
    pub sessions_serial: u64,
    /// The model option the composer shows beside Send: its id and the current value's name.
    pub last_model: Option<(String, String)>,
    /// What the Tools and approval triggers last showed, so only a change rebuilds them.
    pub last_controls: Option<ControlsShown>,
}

/// The composer popover that is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ComposerPopover {
    Tools,
    Approval,
}

/// The Tools trigger's label, the policy and the open popover.
pub(super) type ControlsShown = (
    Option<String>,
    studio_proto::PermissionPolicy,
    Option<ComposerPopover>,
);

impl ComposerHost {
    pub(super) fn new(
        window: &mut Window,
        theme: Theme,
        sink: BoundsSink,
        cx: &mut Context<ConversationPanel>,
    ) -> Self {
        let composer = cx.new(|cx| {
            let mut composer = Composer::new("Describe the change you want", theme, cx);
            composer.set_bounds_sink(sink.clone());
            composer
        });
        cx.subscribe(&composer, |panel, _, event: &ComposerEvent, cx| {
            panel.on_composer_event(event.clone(), cx)
        })
        .detach();
        let history = cx.new(|cx| {
            let mut history = HistoryList::new(window, cx);
            history.set_bounds_sink(sink);
            history
        });
        cx.subscribe_in(
            &history,
            window,
            |panel, _, event: &HistoryEvent, window, cx| {
                panel.on_history_event(event.clone(), window, cx)
            },
        )
        .detach();
        Self {
            composer,
            todo: cx.new(|_| TodoPanel::new()),
            history,
            prepared: HashMap::new(),
            generation: 0,
            preparing: 0,
            stage: VecDeque::new(),
            last_unsent: None,
            refused: None,
            last_commands: Vec::new(),
            last_refusal: None,
            questions: HashMap::new(),
            sessions_serial: 0,
            last_model: None,
            last_controls: None,
        }
    }
}

impl ConversationPanel {
    pub(super) fn prompt_is_empty(&self, cx: &App) -> bool {
        self.host.composer.read(cx).text(cx).trim().is_empty()
    }

    /// Moves the keyboard focus into the composer.
    pub(super) fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        let handle = self.host.composer.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    /// Whether the composer or a question's text field holds the keyboard focus.
    pub(crate) fn composer_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.host
            .composer
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
            || self
                .host
                .questions
                .values()
                .flatten()
                .any(|draft| draft.text.focus_handle(cx).is_focused(window))
    }

    // ---- state from the snapshot -------------------------------------------------------------

    /// Pushes the snapshot's composer state into the components. Cheap and idempotent: each
    /// setter does nothing when the value did not change.
    pub(super) fn sync_composer(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let view = &snapshot.composer;
        if self.host.last_commands != view.commands {
            self.host.last_commands = view.commands.clone();
            let commands = view
                .commands
                .iter()
                .map(|c| SlashCommand::new(c.name.clone(), c.description.clone()))
                .collect();
            self.host
                .composer
                .update(cx, |composer, cx| composer.set_commands(commands, cx));
        }
        self.sync_model(&snapshot, cx);
        self.sync_composer_controls(cx);
        let mode = match view.steer {
            SteerSupport::None => SteerMode::None,
            SteerSupport::AtBoundary => SteerMode::AtBoundary,
            SteerSupport::Native => SteerMode::Native,
        };
        let off = |off: bool, why: &'static str| off.then(|| SharedString::from(why));
        self.host.composer.update(cx, |composer, cx| {
            composer.set_running(view.turn_live, cx);
            composer.set_steer_mode(mode, cx);
            composer.set_attach_disabled(AttachKind::Image, off(!view.images, IMAGES_OFF), cx);
            composer.set_attach_disabled(AttachKind::File, off(!view.files, FILES_OFF), cx);
        });
        let items: Vec<TodoItem> = view.plan.iter().cloned().map(TodoItem::from).collect();
        let chat = snapshot.chat.to_string();
        self.host.todo.update(cx, |todo, cx| {
            todo.set_plan(chat, items, cx);
            todo.set_live(view.turn_live, cx);
        });
        if self.host.last_unsent != view.unsent {
            self.host.last_unsent = view.unsent.clone();
            if let Some(text) = view.unsent.clone() {
                if self.prompt_is_empty(cx) {
                    self.host
                        .composer
                        .update(cx, |composer, cx| composer.set_text(text, cx));
                } else {
                    // The user has typed something new since: never overwrite it.
                    self.host.refused = Some(text);
                }
            }
        }
    }

    /// Shows the agent's model option beside Send, or none when it advertises none.
    fn sync_model(&mut self, snapshot: &WorkflowSnapshot, cx: &mut Context<Self>) {
        let advertised = derive_controls(snapshot, true, self.sdk_ready).config_options;
        let model = snapshot
            .options
            .config
            .iter()
            .filter(|_| advertised)
            .find(|option| super::chat_tab::is_model_option(option))
            .and_then(|option| match &option.kind {
                ConfigKind::Select { current, values } => Some((
                    option.id.clone(),
                    values
                        .iter()
                        .find(|v| &v.value == current)
                        .map_or_else(|| current.clone(), |v| v.name.clone()),
                )),
                ConfigKind::Boolean { .. } => None,
            });
        if self.host.last_model == model {
            return;
        }
        self.host.last_model = model.clone();
        let panel = cx.entity().downgrade();
        let choice = model.map(|(id, label)| super::composer::ModelChoice {
            label: label.into(),
            on_open: Rc::new(move |bounds, window, cx| {
                let _ = panel.update(cx, |this, cx| {
                    this.toggle_config_menu(&id, *bounds, window, cx)
                });
            }),
        });
        self.host
            .composer
            .update(cx, |composer, cx| composer.set_model(choice, cx));
    }

    // ---- Tools and approval ------------------------------------------------------------------

    /// Takes the MCP servers and tool choices Settings → Agent holds now. The shell calls it
    /// after every change, whichever side made it.
    pub fn set_agent_tools(
        &mut self,
        settings: studio_engine::agent_settings::AgentToolSettings,
        cx: &mut Context<Self>,
    ) {
        if self.agent_tools == settings {
            return;
        }
        // A server added or removed on the Settings page while the list is open.
        if let Some((ComposerPopover::Tools, state)) = &mut self.composer_popover
            && state.len != tools_rows(&settings).len()
        {
            *state = tools_menu(&settings, state.trigger_bounds);
        }
        self.agent_tools = settings;
        self.sync_composer_controls(cx);
        cx.notify();
    }

    /// A task is under way: it keeps the tools and the policy it started with.
    pub(super) fn task_running(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.composer.turn_live
                || snapshot
                    .task
                    .as_ref()
                    .is_some_and(|task| task.phase.stoppable())
        })
    }

    /// Shows the Tools and approval triggers after the model select.
    pub(super) fn sync_composer_controls(&mut self, cx: &mut Context<Self>) {
        let total = studio_engine::agent_tools::TOOL_NAMES.len();
        let on = self.agent_tools.enabled_tools().len();
        let tools_label = (on < total).then(|| format!("{on}/{total}"));
        let policy = self.permission_policy;
        let open = self.composer_popover.as_ref().map(|(kind, _)| *kind);
        let shown = (tools_label.clone(), policy, open);
        if self.host.last_controls.as_ref() == Some(&shown) {
            return;
        }
        self.host.last_controls = Some(shown);
        let opener = |kind: ComposerPopover, cx: &mut Context<Self>| {
            let panel = cx.entity().downgrade();
            Rc::new(
                move |bounds: &gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut App| {
                    let _ = panel.update(cx, |this, cx| {
                        this.toggle_composer_popover(kind, *bounds, window, cx)
                    });
                },
            )
        };
        let controls = vec![
            ComposerControl {
                id: "composer-tools".into(),
                icon: Icon::Hammer,
                label: tools_label.map(Into::into),
                name: format!("Tools: {on} of {total} on").into(),
                open: open == Some(ComposerPopover::Tools),
                on_open: opener(ComposerPopover::Tools, cx),
            },
            ComposerControl {
                id: "composer-approval".into(),
                icon: Icon::ShieldQuestion,
                label: Some(policy_short_label(policy).into()),
                name: policy_name(policy).into(),
                open: open == Some(ComposerPopover::Approval),
                on_open: opener(ComposerPopover::Approval, cx),
            },
        ];
        self.host
            .composer
            .update(cx, |composer, cx| composer.set_controls(controls, cx));
    }

    /// Opens the Tools popover or the approval selector against its trigger, or closes it
    /// when it is open.
    pub(super) fn toggle_composer_popover(
        &mut self,
        kind: ComposerPopover,
        trigger: gpui::Bounds<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.composer_popover.as_ref().map(|(open, _)| *open) == Some(kind) {
            self.close_composer_popover(window, cx);
            return;
        }
        self.open_config = None;
        let state = match kind {
            ComposerPopover::Tools => tools_menu(&self.agent_tools, Some(trigger)),
            ComposerPopover::Approval => approval_menu(self.permission_policy, Some(trigger)),
        };
        self.composer_popover = Some((kind, state));
        window.focus(&self.popover_focus, cx);
        self.sync_composer_controls(cx);
        cx.notify();
    }

    pub(super) fn close_composer_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer_popover.take().is_some() {
            self.focus_composer(window, cx);
            self.sync_composer_controls(cx);
            cx.notify();
        }
    }

    /// A switch in the Tools popover: shown at once, saved by the shell.
    fn change_agent_tools(
        &mut self,
        settings: studio_engine::agent_settings::AgentToolSettings,
        cx: &mut Context<Self>,
    ) {
        self.agent_tools = settings.clone();
        self.sync_composer_controls(cx);
        cx.emit(PanelEvent::SetAgentTools(settings));
        cx.notify();
    }

    /// A policy in the approval selector; the shell saves it and hands it back.
    fn choose_policy(
        &mut self,
        policy: studio_proto::PermissionPolicy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_composer_popover(window, cx);
        if policy != self.permission_policy {
            cx.emit(PanelEvent::SetPermissionPolicy(policy));
        }
    }

    /// A popover's link to Settings → Agent.
    fn manage_in_settings(&mut self, tab: AgentTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_composer_popover(window, cx);
        cx.emit(PanelEvent::OpenSettings(SettingsLink::new(tab)));
    }

    pub(super) fn render_composer_popover(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (kind, state) = self.composer_popover.as_ref()?;
        let running = self.task_running();
        let entity = cx.entity();
        let (highlight, manage, dismiss) = (entity.clone(), entity.clone(), entity.clone());
        let on_highlight = move |index: usize, _: &mut Window, cx: &mut App| {
            highlight.update(cx, |this, cx| {
                if let Some((_, state)) = &mut this.composer_popover {
                    state.set_highlighted(index);
                }
                cx.notify();
            });
        };
        let on_manage = move |tab: AgentTab, window: &mut Window, cx: &mut App| {
            manage.update(cx, |this, cx| this.manage_in_settings(tab, window, cx));
        };
        let on_dismiss = move |window: &mut Window, cx: &mut App| {
            dismiss.update(cx, |this, cx| this.close_composer_popover(window, cx));
        };
        Some(match kind {
            ComposerPopover::Tools => {
                ToolsPopover::new(&self.theme, self.agent_tools.clone(), state)
                    .running(running)
                    .focus(&self.popover_focus)
                    .on_change(move |settings, _, cx| {
                        entity.update(cx, |this, cx| this.change_agent_tools(settings, cx));
                    })
                    .on_manage(on_manage)
                    .on_highlight(on_highlight)
                    .on_dismiss(on_dismiss)
                    .into_any_element()
            }
            ComposerPopover::Approval => {
                ApprovalPopover::new(&self.theme, self.permission_policy, state)
                    .running(running)
                    .focus(&self.popover_focus)
                    .on_choose(move |policy, window, cx| {
                        entity.update(cx, |this, cx| this.choose_policy(policy, window, cx));
                    })
                    .on_manage(on_manage)
                    .on_highlight(on_highlight)
                    .on_dismiss(on_dismiss)
                    .into_any_element()
            }
        })
    }

    /// A different conversation, or none: nothing prepared for the old one may travel.
    pub(super) fn reset_composer_state(&mut self, cx: &mut Context<Self>) {
        let host = &mut self.host;
        host.generation += 1;
        host.preparing = 0;
        host.prepared.clear();
        host.stage.clear();
        host.last_unsent = None;
        host.refused = None;
        host.last_refusal = None;
        host.questions.clear();
        host.composer
            .update(cx, |composer, cx| composer.clear_attachments(cx));
    }

    /// The composer side of [`Self::detach`].
    pub(super) fn clear_composer(&mut self, cx: &mut Context<Self>) {
        self.reset_composer_state(cx);
        self.host.last_commands.clear();
        self.host.last_model = None;
        self.host
            .composer
            .update(cx, |composer, cx| composer.set_model(None, cx));
        self.host
            .composer
            .update(cx, |composer, cx| composer.set_commands(Vec::new(), cx));
        self.host
            .todo
            .update(cx, |todo, cx| todo.set_plan("", Vec::new(), cx));
    }

    // ---- sending -----------------------------------------------------------------------------

    pub(super) fn on_composer_event(&mut self, event: ComposerEvent, cx: &mut Context<Self>) {
        match event {
            ComposerEvent::Submit(sub) => self.send_submission(sub, cx),
            ComposerEvent::SendAtBoundary(sub) => self.send_follow_up(sub, false, cx),
            ComposerEvent::SendNow(sub) => self.send_follow_up(sub, true, cx),
            ComposerEvent::Steer(sub) => self.steer_with(sub, cx),
            ComposerEvent::Stop => self.stop(cx),
            ComposerEvent::AttachRequested(request) => self.attach_requested(request, cx),
            ComposerEvent::AttachmentRemoved(id) => {
                self.host.prepared.remove(&id);
            }
            ComposerEvent::AttachBlocked { reason, .. } => {
                self.host.last_refusal = Some(reason.to_string());
                self.say(reason.to_string(), cx);
            }
        }
    }

    /// The Enter gesture from outside the composer (the shell's submit action). An empty
    /// draft says why nothing was sent.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        let (Some(_), Some(snapshot)) = (&self.attached, &self.snapshot) else {
            return;
        };
        if !self.host.composer.read(cx).has_draft(cx) {
            let controls = derive_controls(snapshot, true, self.sdk_ready);
            if let Some(reason) = controls.submit.blocked {
                self.say(reason, cx);
            }
            return;
        }
        self.host
            .composer
            .update(cx, |composer, cx| composer.send_primary(cx));
    }

    /// Puts a refused submission back and says why.
    fn refuse(&mut self, sub: Submission, reason: impl Into<String>, cx: &mut Context<Self>) {
        self.host
            .composer
            .update(cx, |composer, cx| composer.restore(sub, cx));
        self.say(reason, cx);
    }

    /// Copies of what the engine accepted for each chip of `sub`.
    fn prepared_for(&self, sub: &Submission) -> Vec<PreparedAttachment> {
        sub.attachments
            .iter()
            .filter_map(|chip| self.host.prepared.get(&chip.id).cloned())
            .collect()
    }

    /// The send went through: the chips' prepared copies are no longer needed.
    fn sent(&mut self, sub: &Submission, cx: &mut Context<Self>) {
        for chip in &sub.attachments {
            self.host.prepared.remove(&chip.id);
        }
        self.notice = None;
        cx.notify();
    }

    fn send_submission(&mut self, sub: Submission, cx: &mut Context<Self>) {
        let (Some(workflow), Some(snapshot)) = (self.workflow().cloned(), self.snapshot.clone())
        else {
            return self.refuse(sub, "No project is open.", cx);
        };
        let controls = derive_controls(&snapshot, sub.text.trim().is_empty(), self.sdk_ready);
        if let Some(reason) = controls.submit.blocked {
            return self.refuse(sub, reason, cx);
        }
        if let Some(error) = self.scope_error.clone() {
            let reason = format!("The selected timeline scope is unavailable: {error}");
            return self.refuse(sub, reason, cx);
        }
        let attachments = self.prepared_for(&sub);
        let scope = self.task_scope.clone();
        let sent = match &controls.submit.kind {
            SubmitKind::Clarify(_) if !attachments.is_empty() => Err(WorkflowError::Refused(
                "Attachments cannot go with the answer to the agent's question.".into(),
            )),
            SubmitKind::Clarify(task) => workflow.reply_clarification(task, &sub.text),
            SubmitKind::Brief | SubmitKind::Queue if attachments.is_empty() => match scope {
                Some(scope) => workflow.submit_scoped(&sub.text, scope).map(|_| ()),
                None => workflow.submit(&sub.text).map(|_| ()),
            },
            SubmitKind::Brief | SubmitKind::Queue => workflow
                .submit_with_attachments(&sub.text, scope, attachments)
                .map(|_| ()),
        };
        match sent {
            Ok(()) => self.sent(&sub, cx),
            Err(error) => self.refuse(sub, error.to_string(), cx),
        }
    }

    fn steer_with(&mut self, sub: Submission, cx: &mut Context<Self>) {
        let Some(workflow) = self.workflow().cloned() else {
            return self.refuse(sub, "No project is open.", cx);
        };
        if !sub.attachments.is_empty() {
            let reason = "Steering takes text only; use Send now or Send at boundary to include attachments.";
            return self.refuse(sub, reason, cx);
        }
        match workflow.steer(&sub.text) {
            Ok(()) => self.sent(&sub, cx),
            Err(error) => self.refuse(sub, error.to_string(), cx),
        }
    }

    /// Send at the next boundary, or `now`: cancel the turn first.
    fn send_follow_up(&mut self, sub: Submission, now: bool, cx: &mut Context<Self>) {
        let Some(workflow) = self.workflow().cloned() else {
            return self.refuse(sub, "No project is open.", cx);
        };
        let attachments = self.prepared_for(&sub);
        let sent = if now {
            workflow.send_now(&sub.text, attachments)
        } else {
            workflow.send_at_boundary(&sub.text, attachments)
        };
        match sent {
            Ok(()) => self.sent(&sub, cx),
            Err(error) => self.refuse(sub, error.to_string(), cx),
        }
    }

    // ---- what the panel shows above the composer ---------------------------------------------

    /// The bottom of the chat tab: the cards that wait for the user (questions, permission
    /// requests, the outside-file prompt, held and refused messages) in one scrolling area that
    /// gives way to the transcript, the tray (the plan and the queue) tucked under the
    /// composer's top edge, the composer and its toolbar.
    pub(super) fn composer_dock(
        &mut self,
        snapshot: &WorkflowSnapshot,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme;
        let mut cards: Vec<AnyElement> = Vec::new();
        if let Some(restore) = controls
            .session_restore
            .as_ref()
            .filter(|r| !r.is_resumable)
        {
            cards.push(
                ActionCard::new(&theme, "agent-readonly-session", UiTone::Info, "Read-only conversation")
                    .body(restore.notice.clone().unwrap_or_else(|| {
                        "The provider cannot resume this session; its stored messages are shown as they were."
                            .to_owned()
                    }))
                    .into_any_element(),
            );
        }
        for (index, question) in snapshot.composer.questions.iter().enumerate() {
            let card = self.question_card(index, question, cx);
            cards.push(card.into_any_element());
        }
        let requests: Vec<PermissionCard> = snapshot
            .open_permissions()
            .into_iter()
            .filter(|card| !self.is_question(card))
            .cloned()
            .collect();
        for card in &requests {
            let request = self.permission_request(card, cx);
            cards.push(request.into_any_element());
        }
        if let Some(held) = snapshot.composer.held.clone() {
            let card = ActionCard::new(
                &theme,
                "agent-held",
                UiTone::Info,
                "Waiting for the next step",
            )
            .body(held)
            .action(self.action_button(
                "agent-held-cancel",
                "Cancel message",
                true,
                Emphasis::Neutral,
                cx,
                |this, _, cx| {
                    if let Some(workflow) = this.workflow().cloned() {
                        let result = workflow.cancel_held();
                        this.command(result, cx);
                    }
                },
            ));
            cards.push(card.into_any_element());
        }
        if let Some(text) = self.host.refused.clone() {
            let card = ActionCard::new(
                &theme,
                "agent-refused",
                UiTone::Warning,
                "The agent did not take this message",
            )
            .body(text)
            .action(self.action_button(
                "agent-refused-dismiss",
                "Dismiss",
                true,
                Emphasis::Neutral,
                cx,
                |this, _, cx| {
                    this.host.refused = None;
                    cx.notify();
                },
            ))
            .action(self.action_button(
                "agent-refused-restore",
                "Put back",
                true,
                Emphasis::Primary,
                cx,
                |this, _, cx| this.restore_refused(cx),
            ));
            cards.push(card.into_any_element());
        }
        if let Some(card) = self.stage_card(cx) {
            cards.push(card);
        }
        let mut dock = div()
            .flex()
            .flex_col()
            .flex_shrink(1.)
            .min_h_0()
            .gap(px(METRICS.space_2))
            // Oleafly's composer wrapper: 12 px at the sides and bottom, 4 px above.
            .px(px(METRICS.space_3))
            .pt(px(METRICS.space_1))
            .pb(px(METRICS.space_3));
        if !cards.is_empty() {
            // A short window scrolls the cards; the choice stays reachable and the
            // composer stays in view.
            dock = dock.child(
                div()
                    .id("agent-dock-scroll")
                    .flex_shrink(1.)
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(METRICS.space_2))
                            .children(cards),
                    ),
            );
        }
        let todo = self.host.todo.read(cx).is_visible();
        let queue = self.queue_tray(snapshot, cx);
        let tray = (todo || queue.is_some()).then(|| {
            tray_frame(&theme)
                .children(todo.then(|| self.host.todo.clone()))
                .children(queue.map(|queue| {
                    div()
                        .when(todo, |queue| {
                            queue.border_t_1().border_color(theme.colors.line)
                        })
                        .child(queue)
                }))
        });
        dock = dock.child(
            div()
                .id("agent-prompt")
                .flex_none()
                .flex()
                .flex_col()
                .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                    let m = &event.keystroke.modifiers;
                    if event.keystroke.key == "escape" && !m.shift && !m.alt && !m.control {
                        cx.emit(PanelEvent::Leave);
                        cx.stop_propagation();
                    }
                }))
                .children(tray)
                .child(self.host.composer.clone()),
        );
        let blocked = controls
            .submit
            .blocked
            .clone()
            .filter(|_| !self.host.composer.read(cx).has_draft(cx));
        let options = self.options_bar(snapshot, controls, cx);
        if options.is_some() || blocked.is_some() {
            dock = dock.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_1))
                    .children(options)
                    .children(blocked.map(|reason| self.caption(&reason))),
            );
        }
        dock
    }

    /// Briefs waiting for the running task, as the tray's second list: each with a remove
    /// button shown on hover, and a stale one marked as refused.
    fn queue_tray(
        &mut self,
        snapshot: &WorkflowSnapshot,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        if snapshot.queue.is_empty() {
            return None;
        }
        let theme = self.theme;
        let mut tray = div().flex().flex_col().child(
            TrayHeader::new(&theme, "agent-queue", Icon::Clock, "Queued")
                .count(snapshot.queue.len().to_string()),
        );
        for item in &snapshot.queue {
            let id = item.id;
            // The first line of the message, on one line: the whole text is in the transcript
            // once it is sent.
            let first_line = item.summary.lines().next().unwrap_or_default().to_owned();
            let mut row = TrayRow::new(&theme, ("queue-item", id as usize), first_line)
                .single_line()
                .mark(icon(Icon::Clock, STATUS_MARK, theme.colors.text_3))
                .action(self.icon_button(
                    format!("queue-remove-{id}"),
                    Icon::X,
                    "Remove",
                    Emphasis::Ghost,
                    cx,
                    move |this, _, cx| {
                        if let Some(w) = this.workflow().cloned() {
                            let r = w.cancel_queued(id);
                            this.command(r, cx);
                        }
                    },
                ));
            if let Some(scope) = &item.scope {
                row = if item.stale_scope {
                    row.note(format!("{scope} · will be refused"), theme.colors.warning)
                } else {
                    row.note(scope.clone(), theme.colors.text_3)
                };
            }
            tray = tray.child(row);
        }
        Some(tray)
    }

    /// Puts the refused text back into the composer after whatever is there.
    fn restore_refused(&mut self, cx: &mut Context<Self>) {
        let Some(refused) = self.host.refused.take() else {
            return;
        };
        let current = self.host.composer.read(cx).text(cx);
        let text = if current.trim().is_empty() {
            refused
        } else {
            format!("{current}\n\n{refused}")
        };
        self.host
            .composer
            .update(cx, |composer, cx| composer.set_text(text, cx));
        cx.notify();
    }

    /// Redacted observation of the composer side: counts, states and agent-supplied names,
    /// never what the user typed.
    pub(super) fn composer_telemetry(&self, cx: &App) -> serde_json::Value {
        use serde_json::json;
        let composer = self.host.composer.read(cx);
        let todo = self.host.todo.read(cx);
        let history = self.host.history.read(cx);
        let view = self.snapshot.as_ref().map(|s| &s.composer);
        let chip_kinds: Vec<&str> = composer
            .attachments()
            .iter()
            .map(|chip| match chip.draft.kind {
                super::composer::AttachmentKind::File => "file",
                super::composer::AttachmentKind::Image => "image",
            })
            .collect();
        json!({
            "text_chars": composer.text(cx).chars().count(),
            "chips": chip_kinds,
            "context": composer.context().map(|chip| chip.label.as_ref()),
            "commands": self.host.last_commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            "slash_rows": composer.slash_rows(),
            "images_off": composer.attach_disabled(AttachKind::Image),
            "files_off": composer.attach_disabled(AttachKind::File),
            "turn_live": view.map(|v| v.turn_live),
            "steer": view.map(|v| format!("{:?}", v.steer)),
            "held": view.is_some_and(|v| v.held.is_some()),
            "unsent": view.is_some_and(|v| v.unsent.is_some()),
            "refused_shown": self.host.refused.is_some(),
            "preparing": self.host.preparing,
            "last_refusal": self.host.last_refusal,
            "stage": self.host.stage.front().map(|s| json!({
                "path": s.staged.path,
                "bytes": s.staged.size,
                "waiting": self.host.stage.len(),
            })),
            "questions": view.map(|v| v.questions.iter().map(|q| self.question_telemetry(q, cx)).collect::<Vec<_>>()),
            "todo": {
                "visible": todo.is_visible(),
                "label": todo.summary().label(),
                "items": todo.items().iter().map(|i| json!({"text": i.text, "status": format!("{:?}", i.status)})).collect::<Vec<_>>(),
            },
            "history": {
                "shown": history.shown().iter().map(|e| json!({
                    "id": e.id, "current": e.current, "title_chars": e.title.chars().count(),
                })).collect::<Vec<_>>(),
                "query_chars": history.query(cx).chars().count(),
            },
        })
    }

    fn question_telemetry(&self, question: &OpenQuestion, cx: &App) -> serde_json::Value {
        let drafts = self.host.questions.get(&question.reference);
        serde_json::json!({
            "items": question.items.len(),
            "selected": drafts.map(|d| d.iter().map(|i| i.selected.len()).collect::<Vec<_>>()),
            "typed": drafts.map(|d| d.iter().map(|i| !i.text.read(cx).text().trim().is_empty()).collect::<Vec<_>>()),
        })
    }
}
