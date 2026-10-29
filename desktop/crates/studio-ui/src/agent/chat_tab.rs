//! The Chat tab: the status line and its details, the transcript with its empty state,
//! working trailer and jump pill, then the dock and the options bar.

use super::ui::{ActionCard, JumpPill, StatusLine, WorkingTrailer, elapsed_label, panel_empty};
use super::*;
use studio_harness::AdapterStatus;
use studio_harness::driver::ConfigOption;

/// What a new chat suggests when the agent is ready.
const SUGGESTIONS: [&str; 3] = [
    "Make the intro faster",
    "Add a title card",
    "Tighten the pacing",
];

/// Whether `option` is the agent's model choice, which the composer shows beside Send.
pub(super) fn is_model_option(option: &ConfigOption) -> bool {
    matches!(option.kind, ConfigKind::Select { .. })
        && (option.id.eq_ignore_ascii_case("model") || option.name.eq_ignore_ascii_case("model"))
}

/// The status line without a task: who the agent is and whether it can take a task.
fn adapter_status(
    provider: Option<&str>,
    readiness: &AdapterReadiness,
    g: &controls::Guidance,
) -> (String, UiTone) {
    let Some(provider) = provider else {
        return ("No agent set up".to_owned(), UiTone::Warning);
    };
    match readiness {
        AdapterReadiness::NotConfigured => ("No agent set up".to_owned(), UiTone::Warning),
        AdapterReadiness::Unchecked => (format!("{provider} · not checked"), UiTone::Neutral),
        AdapterReadiness::Checking => (format!("{provider} · checking…"), UiTone::Info),
        AdapterReadiness::Checked { report, .. } => match &report.status {
            AdapterStatus::Ready { agent, version } => {
                (format!("{agent} {version} · ready"), UiTone::Success)
            }
            AdapterStatus::AuthUnknown { .. } => (
                format!("{provider} · ready, sign-in not verified"),
                UiTone::Success,
            ),
            _ => (format!("{provider} · {}", g.headline), UiTone::Danger),
        },
    }
}

/// What the working trailer says in a phase, or `None` when nothing runs (the agent waits
/// for the user, or the task ended).
fn working_label(phase: TaskPhase) -> Option<&'static str> {
    match phase {
        TaskPhase::Queued | TaskPhase::Starting => Some("Sending"),
        TaskPhase::Editing | TaskPhase::Repairing => Some("Working"),
        TaskPhase::WaitingBackground
        | TaskPhase::Quiescing
        | TaskPhase::Capturing
        | TaskPhase::Validating
        | TaskPhase::AwaitingEvidence
        | TaskPhase::Promoting => Some(phase.label()),
        _ => None,
    }
}

impl ConversationPanel {
    pub(super) fn options_bar(
        &mut self,
        snapshot: &WorkflowSnapshot,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        // The model sits beside Send; the bar holds what is left.
        let others: Vec<&ConfigOption> = if controls.config_options {
            snapshot
                .options
                .config
                .iter()
                .filter(|option| !is_model_option(option))
                .collect()
        } else {
            Vec::new()
        };
        if !controls.modes && others.is_empty() {
            return controls
                .options_note
                .as_ref()
                .map(|note| div().child(self.caption(note)));
        }
        let mut bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(METRICS.space_2));
        if controls.modes {
            let modes = &snapshot.options.modes;
            let options = modes
                .iter()
                .map(|mode| {
                    SegmentOption::new(
                        mode.name.clone(),
                        snapshot.options.current_mode.as_deref() == Some(mode.id.as_str()),
                    )
                    .id(format!("mode-{}", mode.id))
                })
                .collect();
            let ids: Vec<String> = modes.iter().map(|mode| mode.id.clone()).collect();
            let entity = cx.entity();
            bar = bar.child(
                SegmentedControl::new(&self.theme, "agent-modes", options).on_select(
                    move |index, _, cx| {
                        let Some(id) = ids.get(*index).cloned() else {
                            return;
                        };
                        entity.update(cx, |this, cx| {
                            if let Some(w) = this.workflow().cloned() {
                                let r = w.set_mode(&id);
                                this.command(r, cx);
                            }
                        });
                    },
                ),
            );
        }
        for option in others {
            let option_id = option.id.clone();
            let id = format!("config-{}", option.id);
            match &option.kind {
                ConfigKind::Boolean { current } => {
                    let next = OptionValue::Boolean(!current);
                    let label = format!("{}: {}", option.name, if *current { "on" } else { "off" });
                    let entity = cx.entity();
                    bar = bar.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(METRICS.space_1))
                            .child(self.caption(&option.name))
                            .child(
                                Toggle::new(&self.theme, id, *current)
                                    .label(label)
                                    .on_change(move |_, _, app| {
                                        entity.update(app, |this, cx| {
                                            if let Some(w) = this.workflow().cloned() {
                                                let r =
                                                    w.set_config_option(&option_id, next.clone());
                                                this.command(r, cx);
                                            }
                                        });
                                    }),
                            ),
                    );
                }
                ConfigKind::Select { current, values } => {
                    let name = values
                        .iter()
                        .find(|v| &v.value == current)
                        .map_or(current.as_str(), |v| v.name.as_str());
                    let entity = cx.entity();
                    bar = bar.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(METRICS.space_1))
                            .child(self.caption(&option.name))
                            .child(Select::new(&self.theme, id, name.to_owned()).on_open(
                                move |bounds, window, app| {
                                    entity.update(app, |this, cx| {
                                        this.toggle_config_menu(&option_id, *bounds, window, cx);
                                    });
                                },
                            )),
                    );
                }
            }
        }
        Some(bar)
    }

    pub(super) fn toggle_config_menu(
        &mut self,
        option_id: &str,
        trigger: gpui::Bounds<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open_config.as_deref() == Some(option_id) {
            self.close_config_menu(window, cx);
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let Some(mut menu) = ConfigMenu::of(&snapshot.options.config, option_id) else {
            return;
        };
        menu.state = MenuState::anchored(trigger, &menu.items);
        self.config_menus.insert(option_id.to_owned(), menu);
        self.open_config = Some(option_id.to_owned());
        window.focus(&self.config_focus, cx);
        cx.notify();
    }

    pub(super) fn close_config_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_config.take().is_some() {
            self.focus_prompt(window, cx);
            cx.notify();
        }
    }

    /// Shows the chat and moves the keyboard focus to its prompt.
    pub fn focus_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = Tab::Chat;
        self.focus_composer(window, cx);
    }

    /// Sets option `option_id` to `value`, the value of the row the reader clicked in the
    /// menu as it was painted.
    pub(super) fn commit_config(
        &mut self,
        option_id: &str,
        value: Option<OptionValue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.open_config.as_deref() != Some(option_id) {
            return;
        }
        self.open_config = None;
        if let (Some(value), Some(w)) = (value, self.workflow().cloned()) {
            let r = w.set_config_option(option_id, value);
            self.command(r, cx);
        }
        self.focus_prompt(window, cx);
        cx.notify();
    }

    /// Keeps an open menu in step with the options the agent advertises now: changed
    /// values rebuild it, a withdrawn option closes it.
    pub(super) fn sync_config_menu(&mut self) {
        let Some(option_id) = self.open_config.clone() else {
            return;
        };
        let fresh = self
            .snapshot
            .as_ref()
            .and_then(|s| ConfigMenu::of(&s.options.config, &option_id));
        let Some(fresh) = fresh else {
            self.open_config = None;
            return;
        };
        let Some(menu) = self.config_menus.get_mut(&option_id) else {
            self.open_config = None;
            return;
        };
        menu.refresh(fresh);
    }

    pub(super) fn render_config_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let option_id = self.open_config.as_ref()?;
        let menu = self.config_menus.get(option_id)?;
        let highlight = cx.entity();
        let select = cx.entity();
        let dismiss = cx.entity();
        let key = option_id.clone();
        let chosen = option_id.clone();
        // The values of the rows as painted now: a click commits the one it hit.
        let shown = menu.values.clone();
        Some(
            Menu::new(
                &self.theme,
                "agent-config-menu",
                menu.items.clone(),
                &menu.state,
            )
            .focus(&self.config_focus)
            .on_highlight(move |index, _, cx| {
                highlight.update(cx, |this, cx| {
                    if let Some(menu) = this.config_menus.get_mut(&key) {
                        menu.state.set_highlighted(*index);
                    }
                    cx.notify();
                });
            })
            .on_select(move |index, window, cx| {
                let value = shown.get(*index).cloned().map(OptionValue::Value);
                select.update(cx, |this, cx| {
                    this.commit_config(&chosen, value, window, cx)
                });
            })
            .on_dismiss(move |window, cx| {
                dismiss.update(cx, |this, cx| this.close_config_menu(window, cx));
            })
            .into_any_element(),
        )
    }

    pub(super) fn chat_tab(
        &mut self,
        snapshot: Arc<WorkflowSnapshot>,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Top to bottom: the status line (and the task's details), the transcript, then the
        // dock (cards that wait for an answer, the trays, the composer and its toolbar). The
        // transcript keeps a minimum height; the dock's cards scroll instead of pushing it out
        // of the panel.
        let configured = snapshot.adapter.provider.is_some();
        let g = guidance(&snapshot.adapter.readiness, configured);
        let mut top = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .px(px(METRICS.space_3))
            .pt(px(METRICS.space_2))
            .child(self.status_line(&snapshot, controls, &g, cx));
        top = top.children(self.task_well(&snapshot, controls, cx));
        let setup = self.setup_notice(&snapshot, controls, &g, cx);
        let needs_setup = setup.is_some();
        top = top.children(setup);
        let answering =
            !snapshot.composer.questions.is_empty() || snapshot.resources.open_permissions > 0;
        let empty = snapshot.rows.is_empty() && !self.log_has_older();
        let mut transcript = div()
            .flex_1()
            .min_h(px(if answering || empty {
                TRANSCRIPT_MIN_ANSWERING_H
            } else {
                TRANSCRIPT_MIN_H
            }))
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .px(px(METRICS.space_3))
            .pt(px(METRICS.space_2));
        if self.log_has_older() {
            let loading = self.page.loading.load(Ordering::Acquire);
            transcript = transcript.child(div().flex_none().child(self.action_button(
                "agent-load-earlier",
                if loading {
                    "Loading earlier messages…"
                } else {
                    "Load earlier messages"
                },
                !loading,
                Emphasis::Ghost,
                cx,
                |this, _, cx| {
                    if let Some(first) = this.view_rows.first() {
                        let before = first.id.0;
                        this.request_page(before, rows::PAGE_ROWS);
                        cx.notify();
                    }
                },
            )));
        }
        if empty && !needs_setup {
            transcript = transcript.child(self.new_chat_state(cx));
        }
        let list = list(
            self.list.clone(),
            cx.processor(
                |this, ix: usize, _window, cx| match this.view_rows.get(ix).cloned() {
                    Some(row) => this.row_element(ix, &row, cx),
                    None => div().into_any_element(),
                },
            ),
        )
        .size_full();
        let mut rows = div()
            .id("agent-transcript")
            .role(gpui::Role::List)
            .aria_label("Agent conversation")
            .relative()
            .flex_1()
            .min_h_0()
            .child(list);
        let far = scroll::distance_from_bottom(&self.list) > scroll::JUMP_DISTANCE_PX;
        self.far_from_end.set(far);
        if self.hidden_newer > 0 || !self.older.is_empty() || far {
            // Floats over the newest rows while the reader is away from them.
            let entity = cx.entity();
            rows = rows.child(
                div()
                    .absolute()
                    .bottom(px(METRICS.space_2))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        JumpPill::new(&self.theme, "agent-jump-latest", self.hidden_newer)
                            .on_click(move |_, cx| {
                                entity.update(cx, |this, cx| this.jump_to_latest(cx));
                            }),
                    ),
            );
        }
        transcript = transcript.child(rows);
        transcript = transcript.children(self.working_trailer(&snapshot));
        // A sidebar shorter than the transcript's minimum and the composer scrolls rather
        // than cutting off Send.
        div()
            .id("agent-chat")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .child(top)
            .child(transcript)
            .child(self.composer_dock(&snapshot, controls, cx))
            .into_any_element()
    }

    /// The task, or the agent when there is none, in one line: a status dot, a summary,
    /// the permission policy, Stop while a task waits, and the details chevron.
    fn status_line(
        &mut self,
        snapshot: &WorkflowSnapshot,
        controls: &Controls,
        g: &controls::Guidance,
        cx: &mut Context<Self>,
    ) -> StatusLine {
        let (summary, tone) = match &snapshot.task {
            Some(task) => {
                let what = task.reason.clone().unwrap_or_else(|| task.scope.label());
                (
                    format!(
                        "{}{}: {what}",
                        task.phase.label(),
                        if task.stop_requested {
                            " · stopping"
                        } else {
                            ""
                        }
                    ),
                    banner_tone(task.phase),
                )
            }
            None => adapter_status(
                snapshot.adapter.provider.as_deref(),
                &snapshot.adapter.readiness,
                g,
            ),
        };
        let mut line = StatusLine::new(&self.theme, tone, summary)
            .trailing(policy_badge(self.permission_policy));
        // The composer's own Stop covers a live turn; this one every other state a task can
        // be in (waiting, reviewing, queued briefs).
        if !snapshot.composer.turn_live
            && let Some(note) = controls.stop.clone()
        {
            line = line.trailing(self.stop_button(note, cx));
        }
        if snapshot.task.is_some() {
            let open = self.task_details_open;
            line = line.trailing(self.icon_button(
                "agent-task-details",
                if open {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                if open { "Hide details" } else { "Task details" },
                Emphasis::Ghost,
                cx,
                |this, _, cx| {
                    this.task_details_open = !this.task_details_open;
                    cx.notify();
                },
            ));
        }
        line
    }

    /// The task's well under the status line: its details while open, a blocked Apply, a
    /// conflict, and the review buttons, which stay in view whenever a candidate waits.
    fn task_well(
        &mut self,
        snapshot: &WorkflowSnapshot,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let task = snapshot.task.as_ref()?;
        let open = self.task_details_open;
        if !open && controls.review.is_none() && task.conflict.is_none() {
            return None;
        }
        let mut well = Card::nested(&self.theme)
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .p(px(METRICS.space_3));
        if open {
            well = well.child(self.task_details(snapshot, controls));
        }
        if let Some(conflict) = &task.conflict {
            well = well.child(self.conflict_block(conflict));
        }
        if let Some(review) = &controls.review {
            if let Some(note) = &review.apply_note {
                well = well.child(self.line(format!(
                    "Apply is blocked: {note}. The candidate is kept; retry after fixing it."
                )));
            }
            let mut buttons = div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(px(METRICS.space_2))
                .child(self.banner_review_button(cx));
            for button in self.review_buttons(review, cx) {
                buttons = buttons.child(button);
            }
            well = well.child(buttons);
        }
        Some(well)
    }

    /// One notice while the agent cannot take a task yet (or was never checked): what is
    /// wrong in a line, Set up agent and Check now. The setup steps live on the settings page.
    fn setup_notice(
        &mut self,
        snapshot: &WorkflowSnapshot,
        controls: &Controls,
        g: &controls::Guidance,
        cx: &mut Context<Self>,
    ) -> Option<ActionCard> {
        let configured = snapshot.adapter.provider.is_some();
        let (title, body) = match &snapshot.adapter.readiness {
            _ if !configured => (
                "Agent not set up",
                "Choose an agent in Settings › Agent.".to_owned(),
            ),
            AdapterReadiness::NotConfigured => (
                "Agent not set up",
                "Choose an agent in Settings › Agent.".to_owned(),
            ),
            AdapterReadiness::Unchecked => (
                "Agent not checked yet",
                "Check it once to confirm it starts and is signed in.".to_owned(),
            ),
            _ if !g.usable => (
                "Agent not ready",
                g.steps.first().map_or_else(
                    || g.headline.clone(),
                    |step| format!("{}. {step}", g.headline),
                ),
            ),
            _ => return None,
        };
        let mut card = ActionCard::new(&self.theme, "agent-setup-guide", UiTone::Warning, title)
            .body(body)
            .action(self.action_button(
                "agent-setup-start",
                "Set up agent",
                true,
                if configured {
                    Emphasis::Neutral
                } else {
                    Emphasis::Primary
                },
                cx,
                |_, _, cx| cx.emit(PanelEvent::OpenSetup),
            ));
        if configured {
            card = card.action(self.action_button(
                "agent-check-now",
                "Check now",
                controls.check_adapter,
                Emphasis::Primary,
                cx,
                |this, _, cx| {
                    if let Some(w) = this.workflow().cloned() {
                        let r = w.check_adapter();
                        this.command(r, cx);
                    }
                },
            ));
        }
        Some(card)
    }

    /// A new chat with a ready agent: what to ask, and a few suggestions to start from.
    fn new_chat_state(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = self.theme;
        let chips = SUGGESTIONS.iter().enumerate().map(|(ix, text)| {
            let entity = cx.entity();
            div()
                .id(("agent-suggestion", ix))
                .cursor_pointer()
                .on_click(move |_, window, cx| {
                    entity.update(cx, |this, cx| {
                        this.host
                            .composer
                            .update(cx, |composer, cx| composer.set_text(*text, cx));
                        this.focus_composer(window, cx);
                    });
                })
                .child(Chip::new(&theme, ("agent-suggestion-chip", ix), *text))
        });
        div()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(METRICS.space_3))
            .pt(px(METRICS.space_8))
            .child(panel_empty(
                &theme,
                TextStyle::Title,
                "What should we change?",
                "Describe an edit; the agent changes the project and you review it.",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(METRICS.space_2))
                    .children(chips),
            )
    }

    /// Under the last row while the agent works: what it does and for how long.
    fn working_trailer(&self, snapshot: &WorkflowSnapshot) -> Option<WorkingTrailer> {
        let task = snapshot.task.as_ref()?;
        let label = working_label(task.phase)?;
        let started = self
            .view_rows
            .iter()
            .rev()
            .find(|row| matches!(row.kind, RowKind::User { .. }))
            .map(|row| row.at_unix)
            .filter(|at| *at > 0);
        let mut trailer = WorkingTrailer::new(&self.theme, label);
        if let Some(started) = started {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            trailer = trailer.elapsed(elapsed_label(now.saturating_sub(started)));
        }
        Some(trailer)
    }

    /// What the status line keeps behind its chevron: the task's identity, its repair budget,
    /// what Stop does now, the frozen scope and the before/after evidence.
    fn task_details(&self, snapshot: &WorkflowSnapshot, controls: &Controls) -> gpui::Div {
        let mut details = div().flex().flex_col().gap(px(METRICS.space_1));
        let Some(task) = &snapshot.task else {
            return details;
        };
        let id = task.id.0.as_str();
        details = details.child(self.line(format!(
            "Task {} · generation {} · base {}",
            &id[..id.len().min(8)],
            task.generation,
            task.source_base
        )));
        if !matches!(
            task.phase,
            TaskPhase::AwaitingReview | TaskPhase::AwaitingEvidence
        ) {
            details = details.child(self.line(format!(
                "{} · {} of {} automatic repairs",
                match task.review_policy {
                    ReviewPolicy::AutoApply => "Apply automatically",
                    ReviewPolicy::ManualReview => "Manual review",
                },
                task.repair.used,
                task.repair.max
            )));
        }
        if let Some(note) = &controls.stop
            && !task.phase.is_terminal()
        {
            details = details.child(self.line(note.clone()));
        }
        details = details.child(self.line(format!(
            "{}: {}",
            if task.phase.is_terminal() {
                "Last task scope"
            } else {
                "Frozen task scope"
            },
            task.scope.label()
        )));
        let thumb_radius = px(METRICS.radius.item);
        let mut evidence_cards = div().flex().gap(px(METRICS.space_2));
        let mut has_evidence_card = false;
        for (name, view) in [("Before", &task.before), ("After", &task.after)] {
            let Some(view) = view else {
                continue;
            };
            let selected = view
                .artifacts
                .iter()
                .find(|artifact| artifact.label == "selected")
                .or_else(|| view.artifacts.first());
            let image = selected.and_then(|artifact| {
                self.evidence_images
                    .get(&evidence_image_key(&task.id, &artifact.id))
                    .map(|(_, image)| image)
            });
            let (Some(artifact), Some(image)) = (selected, image) else {
                details = details.child(self.line(evidence_summary(name, view)));
                continue;
            };
            has_evidence_card = true;
            let boundaries = view
                .artifacts
                .iter()
                .filter(|other| other.id != artifact.id)
                .flat_map(|other| other.frames.iter().copied())
                .map(|frame| frame.to_string())
                .collect::<Vec<_>>()
                .join(",");
            evidence_cards = evidence_cards.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_1))
                    .child(label_text(format!("{name} · revision {}", view.revision)))
                    .child(
                        gpui::img(image.clone())
                            .w(px(EVIDENCE_THUMB_W))
                            .h(px(EVIDENCE_THUMB_H))
                            .rounded(thumb_radius),
                    )
                    .child(
                        text_style(div(), TextStyle::Small)
                            .text_color(self.theme.colors.text_2)
                            .child(selectable(format!(
                                "{} · {} frames{}",
                                artifact.label,
                                artifact.frames.len(),
                                if boundaries.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · edge {boundaries}")
                                }
                            ))),
                    ),
            );
        }
        if has_evidence_card {
            details = details.child(evidence_cards);
        }
        if let Some(limit) = task
            .image_limitation
            .as_ref()
            .filter(|_| task.before.is_some())
        {
            let summary = if limit.contains("Text-only") {
                "Image context is text-only; the agent receives artifact references."
            } else {
                limit
            };
            details = details.child(self.line(summary.to_owned()));
        }
        details
    }

    /// Stop for a task that waits (no live turn): a round solid button; what Stop does now
    /// is its tooltip.
    pub(super) fn stop_button(&mut self, note: String, cx: &mut Context<Self>) -> IconButton {
        let id: ElementId = "agent-stop".into();
        let probe = Self::bounds_probe(&id, cx);
        let button = IconButton::new(&self.theme, id, Icon::Square.path())
            .size(IconButtonSize::Sm)
            .inverted()
            .label("Stop")
            .tooltip(note)
            .on_click(Self::click(cx, |this, _, cx| this.stop(cx)));
        match probe {
            Some(probe) => button.on_bounds(probe.into_bounds_handler()),
            None => button,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_trailer_shows_only_while_the_agent_works() {
        assert_eq!(working_label(TaskPhase::Starting), Some("Sending"));
        assert_eq!(working_label(TaskPhase::Editing), Some("Working"));
        assert_eq!(
            working_label(TaskPhase::Validating),
            Some(TaskPhase::Validating.label())
        );
        assert_eq!(working_label(TaskPhase::WaitingPermission), None);
        assert_eq!(working_label(TaskPhase::AwaitingReview), None);
        assert_eq!(working_label(TaskPhase::Accepted), None);
    }
}
