//! The settings frame's Agent page: providers, the adapter description and writer
//! containment. The fields, their state and their telemetry stay the panel's;
//! [`AgentSetupPage`] only renders them where the settings frame puts the page.

use super::*;

/// The settings frame's Agent page, drawn by the panel it belongs to.
pub struct AgentSetupPage {
    panel: Entity<ConversationPanel>,
    _panel_changed: gpui::Subscription,
}

impl AgentSetupPage {
    pub fn new(panel: &Entity<ConversationPanel>, cx: &mut Context<Self>) -> Self {
        Self {
            panel: panel.clone(),
            _panel_changed: cx.observe(panel, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for AgentSetupPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.panel
            .update(cx, |panel, cx| panel.render_setup_page(window, cx))
    }
}

impl ConversationPanel {
    fn render_setup_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.theme.refresh(window, cx);
        let Some(snapshot) = self.snapshot.clone() else {
            return div()
                .flex()
                .flex_col()
                .gap(px(METRICS.space_2))
                .child(self.line("Open or create a project to set up its agent."))
                .into_any_element();
        };
        let controls = derive_controls(&snapshot, self.prompt_is_empty(cx), self.sdk_ready);
        self.setup_page(snapshot, &controls, cx)
    }

    fn setup_page(
        &mut self,
        snapshot: Arc<WorkflowSnapshot>,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let configured = snapshot.adapter.provider.is_some();
        let g = guidance(&snapshot.adapter.readiness, configured);
        let mut column = div()
            .id("agent-setup-page")
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2));
        column = column.child(
            self.section(&g.headline)
                .children(g.steps.iter().map(|s| self.line(s.clone()))),
        );
        // The panel header, where notices usually show, is out of view behind the frame.
        if let Some(notice) = &self.notice {
            column = column.child(Notice::new(
                "agent-setup-notice",
                UiTone::Warning,
                notice.clone(),
            ));
        }
        if let Some(error) = &self.settings_error {
            column = column.child(Notice::new(
                "agent-settings-error",
                UiTone::Danger,
                error.clone(),
            ));
        }
        let switch_pending = snapshot.switch_pending.is_some();
        let selection_busy = snapshot
            .task
            .as_ref()
            .is_some_and(|task| !task.phase.is_terminal())
            || matches!(&snapshot.undo, UndoView::InProgress { .. });
        column = column.child(self.providers_section(switch_pending, selection_busy, cx));
        column = column.child(self.detect_section(
            &snapshot.adapter.readiness,
            controls.settings_editable,
            cx,
        ));

        if let Some(choice) = &controls.switch_choice {
            let mut handoff = self
                .section("Stopped-writer handoff")
                .child(self.line(choice.note.clone()))
                .child(self.line(format!(
                    "Outgoing task: {}",
                    snapshot
                        .task
                        .as_ref()
                        .map(|task| task.brief.as_str())
                        .unwrap_or("no active task")
                )))
                .child(self.line(format!(
                    "Draft revision: {} · current source: {} · accepted: {}",
                    choice.draft_revision.as_deref().unwrap_or("unavailable"),
                    choice.source_revision,
                    choice.accepted_revision
                )));
            if choice.retained_queue_count > 0 {
                handoff = handoff.child(self.line(format!(
                    "{} queued brief(s) remain with the outgoing provider until you explicitly transfer or clear them.",
                    choice.retained_queue_count
                )));
            }
            handoff = handoff.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(self.action_button(
                        "agent-handoff-continue-draft",
                        "Continue captured draft",
                        true,
                        Emphasis::Primary,
                        cx,
                        |this, _, cx| {
                            if let Some(workflow) = this.workflow().cloned() {
                                this.command(
                                    workflow.choose_handoff_continuation(
                                        studio_engine::agent::model::HandoffChoice::ContinueDraft,
                                    ),
                                    cx,
                                );
                            }
                        },
                    ))
                    .child(self.action_button(
                        "agent-handoff-restart-accepted",
                        "Restart from accepted",
                        true,
                        Emphasis::Neutral,
                        cx,
                        |this, _, cx| {
                            if let Some(workflow) = this.workflow().cloned() {
                                this.command(
                                    workflow.choose_handoff_continuation(
                                        studio_engine::agent::model::HandoffChoice::RestartFromAccepted,
                                    ),
                                    cx,
                                );
                            }
                        },
                    ))
                    .child(self.action_button(
                        "agent-handoff-cancel",
                        "Cancel switch",
                        true,
                        Emphasis::Neutral,
                        cx,
                        |this, _, cx| {
                            if let Some(workflow) = this.workflow().cloned() {
                                this.command(workflow.cancel_switch(), cx);
                            }
                        },
                    )),
            );
            column = column.child(handoff);
        }
        if controls.retained_queue.is_some() {
            let mut retained = self.section("Queued work held for the previous provider");
            for brief in &snapshot.retained_queue {
                retained = retained.child(self.line(format!("• {}", brief.summary)));
            }
            retained = retained.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(self.action_button(
                        "agent-transfer-retained-queue",
                        "Explicitly transfer queued briefs",
                        !selection_busy && !switch_pending,
                        Emphasis::Neutral,
                        cx,
                        |this, _, cx| {
                            if let Some(workflow) = this.workflow().cloned() {
                                this.command(workflow.transfer_retained_queue(), cx);
                            }
                        },
                    ))
                    .child(self.action_button(
                        "agent-clear-retained-queue",
                        "Clear held briefs",
                        !selection_busy && !switch_pending,
                        Emphasis::Neutral,
                        cx,
                        |this, _, cx| {
                            if let Some(workflow) = this.workflow().cloned() {
                                this.command(workflow.clear_retained_queue(), cx);
                            }
                        },
                    )),
            );
            column = column.child(retained);
        }
        if let Some(restore) = &controls.session_restore {
            let mut session = self
                .section("Previous provider session")
                .child(self.line(format!(
                    "{} · session {} · {}",
                    restore.provider_id,
                    restore.redacted_session_id,
                    restore
                        .notice
                        .as_deref()
                        .unwrap_or("Native conversation memory stays with its provider.")
                )));
            if restore.is_resumable {
                session = session.child(self.action_button(
                    "agent-restore-session",
                    "Use restored session for next task",
                    !selection_busy && !switch_pending,
                    Emphasis::Neutral,
                    cx,
                    |this, _, cx| {
                        if let Some(workflow) = this.workflow().cloned() {
                            this.command(workflow.restore_session(), cx);
                        }
                    },
                ));
            }
            session = session.child(self.action_button(
                "agent-dismiss-session",
                "Dismiss saved session",
                !selection_busy && !switch_pending,
                Emphasis::Neutral,
                cx,
                |this, _, cx| {
                    if let Some(workflow) = this.workflow().cloned() {
                        this.command(workflow.dismiss_restoration(), cx);
                    }
                },
            ));
            column = column.child(session);
        }
        let field = |this: &Self,
                     label: &'static str,
                     input: &Entity<TextInputState>,
                     cx: &mut Context<Self>| {
            // The handler reads THIS field's composition state: an active IME composition
            // keeps Enter/Tab/Escape (candidate selection and cancellation) in the field.
            let field = input.clone();
            let probe = Self::bounds_probe(&ElementId::Name(format!("setup-{label}").into()), cx)
                .map(|probe| {
                    let report = probe.into_bounds_handler();
                    canvas(move |bounds, _, cx| report(bounds, cx), |_, _, _, _| {})
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                });
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(self.line(label))
                .child(
                    div()
                        .relative()
                        .children(probe)
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                this.key_in_field(&field, event, false, window, cx);
                            },
                        ))
                        .child(TextInput::new(
                            &this.theme,
                            SharedString::from(format!("setup-{label}")),
                            input,
                        )),
                )
        };
        let editable = controls.settings_editable;
        let mut form = self.section("Adapter");
        form = form
            .child({
                let entity = cx.entity();
                let options = AgentWire::ALL
                    .iter()
                    .map(|wire| {
                        SegmentOption::new(wire.label(), *wire == self.wire_choice)
                            .disabled(!editable)
                    })
                    .collect();
                FormField::row(
                    &self.theme,
                    "Protocol",
                    SegmentedControl::new(&self.theme, "agent-wire", options).on_select(
                        move |index, _, app| {
                            if let Some(&wire) = AgentWire::ALL.get(*index) {
                                entity.update(app, |this, cx| {
                                    this.wire_choice = wire;
                                    cx.notify();
                                });
                            }
                        },
                    ),
                )
            })
            .child(field(self, "Label", &self.setup.provider.clone(), cx))
            .child(field(
                self,
                "Executable",
                &self.setup.executable.clone(),
                cx,
            ))
            .child(field(self, "Arguments", &self.setup.args.clone(), cx))
            .child(field(
                self,
                "Sign-in variable names (values are never stored)",
                &self.setup.env_names.clone(),
                cx,
            ));
        form = form
            .child(self.line(if self.saved_adapter.is_some() {
                self.policy.summary()
            } else {
                "Writer containment: no adapter configured.".to_owned()
            }))
            .child({
                let entity = cx.entity();
                let label = match self.mcp_choice {
                    McpChoice::Baseline => "Project tools: MCP and command line",
                    McpChoice::Unsupported => "Project tools: command line only",
                };
                FormField::row(
                    &self.theme,
                    label,
                    Toggle::new(
                        &self.theme,
                        "agent-mcp-toggle",
                        self.mcp_choice == McpChoice::Baseline,
                    )
                    .label(label)
                    .disabled(!editable)
                    .on_change(move |_, _, app| {
                        entity.update(app, |this, cx| {
                            this.mcp_choice = match this.mcp_choice {
                                McpChoice::Baseline => McpChoice::Unsupported,
                                McpChoice::Unsupported => McpChoice::Baseline,
                            };
                            cx.notify();
                        });
                    }),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(self.action_button(
                        "agent-save-adapter",
                        "Save",
                        editable && !self.saves.is_pending(),
                        Emphasis::Primary,
                        cx,
                        |this, _, cx| this.save_adapter(cx),
                    ))
                    .child(self.action_button(
                        "agent-check-adapter",
                        "Check adapter",
                        controls.check_adapter,
                        Emphasis::Neutral,
                        cx,
                        |this, _, cx| {
                            if let Some(w) = this.workflow().cloned() {
                                let r = w.check_adapter();
                                this.command(r, cx);
                            }
                        },
                    )),
            );
        if !editable {
            form = form.child(
                self.line("Settings can change after the running task finishes or is stopped."),
            );
        }
        column = column.child(form);

        let mut status = self.section("Status");
        status = status
            .child(self.line(format!(
                "Adapter: {}",
                snapshot
                    .adapter
                    .executable
                    .clone()
                    .unwrap_or_else(|| "not configured".into())
            )))
            .child(self.line(format!(
                "Writer containment at the last launch: {}",
                snapshot
                    .adapter
                    .last_launch
                    .as_ref()
                    .map_or("not launched yet", |launch| launch.ownership.as_str())
            )));
        if snapshot
            .adapter
            .last_launch
            .as_ref()
            .is_some_and(|launch| !launch.contained)
        {
            status = status.child(self.line(
                "The last launch of this adapter could not verify that its writer processes stay contained, so its candidates are not captured.",
            ));
        }
        if let AdapterReadiness::Checked { report, .. } = &snapshot.adapter.readiness
            && let Some(info) = &report.initialized
        {
            status = status.child(self.line(format!(
                "Adapter reports {} {} · ACP v{}",
                info.agent_name, info.agent_version, info.protocol_version
            )));
        }
        if let Some(caps) = &snapshot.capabilities {
            let mut unsupported = Vec::new();
            if !caps.prompt_image {
                unsupported.push("images");
            }
            if !caps.prompt_audio {
                unsupported.push("audio");
            }
            if !caps.load_session && !caps.resume_session {
                unsupported.push("resuming a session");
            }
            if !unsupported.is_empty() {
                status = status.child(self.line(format!(
                    "Not supported by this agent (prompts are text only): {}",
                    unsupported.join(", ")
                )));
            }
        }
        let mcp = &snapshot.mcp;
        status = status.child(self.line(format!(
            "Project tools: MCP {} · command line {}{}",
            if mcp.policy_enabled && mcp.binary_available {
                "offered"
            } else {
                "not offered"
            },
            if mcp.cli_active {
                "offered"
            } else {
                "when a task runs"
            },
            mcp.note
                .as_ref()
                .map(|n| format!(" · {n}"))
                .unwrap_or_default()
        )));
        column = column.child(status);
        column.into_any_element()
    }
}
