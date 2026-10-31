//! Transcript rows and the published snapshot.

use super::*;

impl Actor {
    // ---- rows ---------------------------------------------------------------------------------

    pub(super) fn touch(&mut self) {
        self.dirty = true;
    }

    pub(super) fn task_id(&self) -> Option<AgentTaskId> {
        self.active
            .as_ref()
            .map(|a| a.ctx.identity.task.clone())
            .or_else(|| self.task.as_ref().map(|t| t.id.clone()))
    }

    /// Replaces every configured credential value in `text`.
    pub(super) fn scrub_str(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "[REDACTED]");
            }
        }
        out
    }

    pub(super) fn scrub_json(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => {
                if self.secrets.iter().any(|s| text.contains(s.as_str())) {
                    *text = self.scrub_str(text);
                }
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(|v| self.scrub_json(v)),
            serde_json::Value::Object(map) => map.values_mut().for_each(|v| self.scrub_json(v)),
            _ => {}
        }
    }

    /// `value` with every nested string scrubbed of the configured credential values
    /// (`None` only if it cannot be re-read, in which case nothing may be shown).
    pub(super) fn scrub<T: serde::Serialize + serde::de::DeserializeOwned>(
        &self,
        value: T,
    ) -> Option<T> {
        if self.secrets.is_empty() {
            return Some(value);
        }
        let mut json = serde_json::to_value(&value).ok()?;
        self.scrub_json(&mut json);
        serde_json::from_value(json).ok()
    }

    pub(super) fn scrub_kind(&self, kind: RowKind) -> RowKind {
        // Streamed agent text was scrubbed by the driver with the same secrets.
        if kind.is_message() {
            return kind;
        }
        self.scrub(kind).unwrap_or_else(|| RowKind::Notice {
            level: NoticeLevel::Warning,
            text: "[REDACTED]".into(),
        })
    }

    /// Persisting dropped unsaved rows to keep the memory bound: remember it so the
    /// running task is stopped visibly instead of streaming into the void.
    pub(super) fn note_log_loss(&mut self) {
        if self.store.take_lost() > 0 {
            self.log_lost = true;
        }
    }

    pub(super) fn push_row(&mut self, kind: RowKind) -> RowId {
        if !kind.is_message() {
            // Anything but more text ends the message being streamed.
            self.finish_streaming();
        }
        let kind = self.scrub_kind(kind);
        let id = self.store.next_id();
        let row = Row {
            id,
            task: self.task_id(),
            at_unix: now_unix(),
            kind,
        };
        self.store.upsert(Arc::new(row));
        self.note_log_loss();
        self.touch();
        id
    }

    /// Rewrites one row, wherever its latest version lives: a row that already left the
    /// resident window is read back from the log and its new version is persisted under
    /// the same id (never a second card).
    pub(super) fn update_row(&mut self, id: RowId, change: impl FnOnce(&mut RowKind)) -> bool {
        let Some(row) = self.store.latest(id) else {
            return false;
        };
        let mut next = (*row).clone();
        change(&mut next.kind);
        next.kind = self.scrub_kind(next.kind);
        self.store.upsert(Arc::new(next));
        self.note_log_loss();
        self.touch();
        true
    }

    pub(super) fn notice(&mut self, text: impl AsRef<str>) {
        let text = present::message(text.as_ref());
        self.push_row(RowKind::Notice {
            level: NoticeLevel::Info,
            text,
        });
    }

    pub(super) fn warn(&mut self, text: impl AsRef<str>) {
        let text = present::message(text.as_ref());
        self.push_row(RowKind::Notice {
            level: NoticeLevel::Warning,
            text,
        });
    }

    pub(super) fn error_row(&mut self, error: StructuredError) {
        if let Some(task) = &mut self.task
            && self.active.is_some()
        {
            task.error = Some(error.clone());
        }
        self.push_row(RowKind::Error(error));
    }

    pub(super) fn append_message(&mut self, role: MessageRole, text: &str) {
        if role == MessageRole::User || text.is_empty() {
            return;
        }
        let turn = self.active.as_ref().and_then(|a| a.turn).unwrap_or(0);
        let mut rest = text;
        while !rest.is_empty() {
            if let Some(stream) = &self.stream
                && stream.role == role
                && stream.turn == turn
                && let Some(row) = self.store.get(stream.row)
            {
                let len = match &row.kind {
                    RowKind::Agent { text, .. } | RowKind::Thought { text, .. } => text.len(),
                    _ => usize::MAX,
                };
                if len < MAX_ROW_TEXT_BYTES {
                    let (head, tail) = split_at_boundary(rest, MAX_ROW_TEXT_BYTES - len);
                    if !head.is_empty() {
                        let id = stream.row;
                        let head = head.to_owned();
                        self.update_row(id, |kind| {
                            if let RowKind::Agent { text, .. } | RowKind::Thought { text, .. } =
                                kind
                            {
                                text.push_str(&head);
                            }
                        });
                        rest = tail;
                        continue;
                    }
                }
            }
            let (head, tail) = split_at_boundary(rest, MAX_ROW_TEXT_BYTES);
            let text = head.to_owned();
            let kind = match role {
                MessageRole::Thought => RowKind::Thought {
                    text,
                    turn,
                    streaming: true,
                },
                _ => RowKind::Agent {
                    text,
                    turn,
                    streaming: true,
                },
            };
            let id = self.push_row(kind);
            self.stream = Some(Stream {
                role,
                turn,
                row: id,
            });
            rest = tail;
        }
    }

    pub(super) fn finish_streaming(&mut self) {
        if let Some(stream) = self.stream.take() {
            self.update_row(stream.row, |kind| {
                if let RowKind::Agent { streaming, .. } | RowKind::Thought { streaming, .. } = kind
                {
                    *streaming = false;
                }
            });
        }
    }

    pub(super) fn upsert_tool(&mut self, event: ToolEvent) {
        self.composer_track_tool(&event);
        if let Some(&row) = self.tool_rows.get(&event.tool_call_id) {
            let event = event.clone();
            if self.update_row(row, |kind| {
                if let RowKind::Tool(card) = kind {
                    if event.title.is_some() {
                        card.title = event.title.clone();
                    }
                    if event.kind.is_some() {
                        card.kind = event.kind.clone();
                    }
                    if event.status.is_some() {
                        card.status = event.status;
                    }
                    card.updates += 1;
                }
            }) {
                return;
            }
        }
        while self.tool_rows.len() >= MAX_TOOL_INDEX {
            match self.tool_order.pop_front() {
                Some(oldest) => {
                    self.tool_rows.remove(&oldest);
                }
                None => {
                    self.tool_rows.clear();
                    break;
                }
            }
        }
        let id = self.push_row(RowKind::Tool(ToolCard {
            call_id: event.tool_call_id.clone(),
            title: event.title,
            kind: event.kind,
            status: event.status,
            updates: 0,
        }));
        self.tool_order.push_back(event.tool_call_id.clone());
        self.tool_rows.insert(event.tool_call_id, id);
    }

    // ---- snapshot ---------------------------------------------------------------------------------

    pub(super) fn publish(&mut self, force: bool) {
        if !force && !self.dirty {
            return;
        }
        if let Err(error) = self.store.flush() {
            if !self.log_failed {
                self.log_failed = true;
                self.warn(format!(
                    "The conversation could not be saved ({error}); it stays visible until the app closes."
                ));
            }
        } else {
            self.log_failed = false;
        }
        self.revision += 1;
        let rows = self.store.resident();
        // Open requests are tracked outside the transcript window: an evicted card is
        // still open (and answerable).
        let open_permissions = self.permission_rows.len();
        self.published_owned = self.shared.root_scope.active_count();
        let (settings_adapter, tools_settings) = {
            let settings = self.shared.settings.lock();
            (settings.adapter.clone(), settings.tools.clone())
        };
        let policy_enabled = settings_adapter
            .as_ref()
            .is_some_and(|a| a.mcp == McpStdioSupport::Baseline);
        let binary_available = tools_settings
            .as_ref()
            .and_then(|t| t.studio_mcp.as_ref())
            .is_some_and(|p| p.is_file());
        let live = self.active.as_ref().filter(|a| a.driver.is_some());
        let mcp_active = live.is_some_and(|a| a.mcp.is_some());
        let cli_active = live.is_some_and(|a| a.cli.is_some());
        let capability_file = live.and_then(|a| a.capability.clone());
        let task = self.task.clone().and_then(|task| self.scrub(task));
        let history: Vec<HistoryEntry> = self
            .history
            .iter()
            .cloned()
            .map(|mut entry| {
                entry.summary = self.scrub_str(&entry.summary);
                entry
            })
            .collect();
        let snapshot = WorkflowSnapshot {
            revision: self.revision,
            project: self.shared.project.clone(),
            adapter: self.adapter.clone(),
            capabilities: self.capabilities.clone(),
            options: self.options.clone(),
            mcp: McpView {
                policy_enabled,
                binary_available,
                active: mcp_active,
                cli_active,
                capability_file,
                note: self.tool_notes_view(),
            },
            task,
            queue: self
                .queue
                .iter()
                .map(|q| QueuedBrief {
                    id: q.id,
                    summary: self.scrub_str(&present::brief_summary(&q.brief)),
                    queued_unix: q.queued_unix,
                    scope: q.scope.as_ref().map(TaskScope::label),
                    stale_scope: q.scope.as_ref().is_some_and(|scope| {
                        scope.compiled.is_some()
                            && !self
                                .displayed_preview_identity
                                .as_ref()
                                .is_some_and(|identity| scope.is_current_preview(identity))
                    }),
                })
                .collect(),
            retained_queue: self
                .retained_queue
                .iter()
                .map(|q| QueuedBrief {
                    id: q.id,
                    summary: self.scrub_str(&present::brief_summary(&q.brief)),
                    queued_unix: q.queued_unix,
                    scope: q.scope.as_ref().map(TaskScope::label),
                    stale_scope: q.scope.as_ref().is_some_and(|scope| {
                        scope.compiled.is_some()
                            && !self
                                .displayed_preview_identity
                                .as_ref()
                                .is_some_and(|identity| scope.is_current_preview(identity))
                    }),
                })
                .collect(),
            older_rows: self.store.has_older(),
            chat: self.chat,
            resources: ResourceView {
                resident_rows: self.store.resident_len(),
                resident_bytes: self.store.resident_bytes(),
                max_resident_rows: MAX_RESIDENT_ROWS,
                max_resident_bytes: MAX_RESIDENT_ROW_BYTES,
                queue_len: self.queue.len(),
                open_permissions,
                owned_processes: self.published_owned,
                broker_grants: self.tools.as_ref().map_or(0, ToolRuntime::grants),
                tool_workers: self.tools.as_ref().map_or(0, ToolRuntime::workers),
            },
            rows,
            composer: self.publish_composer(),
            review_policy: self.policy,
            undo: match &self.undo {
                Some(_) => match &self.undo_view {
                    UndoView::InProgress { .. } => self.undo_view.clone(),
                    _ => UndoView::InProgress {
                        phase: "Undo".into(),
                    },
                },
                None => self.undo_view.clone(),
            },
            recovery: self.recovery.clone(),
            handoff: self.handoff.clone().and_then(|h| self.scrub(h)),
            history,
            closed: self.closing,
            switch_pending: self.switch_pending.clone(),
            session_restore: self.session_restore.clone(),
        };
        *self.shared.snapshot.lock() = Arc::new(snapshot);
        self.shared.changed.notify_all();
        if let Some(notify) = &self.shared.notify {
            notify();
        }
        self.dirty = false;
        self.last_publish = Instant::now();
    }

    pub(super) fn settings_changed(&mut self) {
        let settings = self.shared.settings.lock().adapter.clone();
        self.adapter = match &settings {
            None => AdapterView {
                provider: None,
                executable: None,
                readiness: AdapterReadiness::NotConfigured,
                last_launch: None,
            },
            Some(adapter) => AdapterView {
                provider: Some(adapter.provider.clone()),
                executable: Some(adapter.adapter.executable.clone()),
                // A check still running belongs to the old settings; when it ends the
                // check runs again for these (see `Msg::Probe`).
                readiness: if self.probing {
                    AdapterReadiness::Checking
                } else {
                    AdapterReadiness::Unchecked
                },
                last_launch: None,
            },
        };
        self.probe_generation += 1;
        if let Some(tools) = self.tools.take() {
            tools.shutdown();
        }
        self.touch();
    }

    /// Recomputes everything derived from the controller. Only called while no job may
    /// hold the controller for long.
    pub(super) fn refresh_idle(&mut self) {
        let controller = self.shared.controller.clone();
        let mut c = controller.lock();
        self.policy = c.review_policy();
        if self.undo.is_none() {
            self.undo_view = match c.undo_status() {
                crate::UndoStatus::Available { target, summary } => {
                    UndoView::Available { target, summary }
                }
                crate::UndoStatus::Unavailable { reason } => UndoView::Unavailable { reason },
            };
        }
        let entries = c.state().task_history().entries();
        let skip = entries.len().saturating_sub(MAX_HISTORY_ENTRIES);
        let mut reverted = c.state().task_history().all_reverted_paths();
        self.history = entries[skip..]
            .iter()
            .map(|r| HistoryEntry {
                id: r.id.clone(),
                kind: format!("{:?}", r.kind),
                summary: present::message(&r.prompt_summary),
                files: r.changes.len(),
                published: present::short(r.published.as_str()),
                undoes: r.undoes.clone(),
                undoes_paths: r.undoes_paths.clone(),
                reverted_paths: reverted.remove(&r.id).unwrap_or_default(),
                committed_unix: r.committed_unix,
            })
            .collect();
        let status = c.recovery_status();
        let unresolved = status
            .unresolved
            .iter()
            .map(conflict_of_report)
            .collect::<Vec<_>>();
        let rolled_back = status.report.rolled_back.clone();
        let suspended = status.suspended.clone();
        let notice = c.recovery_notice.clone();
        let draft_path = c.agent_draft_store().path().to_path_buf();
        let draft = c.agent_draft_state().ok().flatten();
        drop(c);
        self.recovery = RecoveryView {
            rolled_back,
            unresolved_conflicts: unresolved,
            suspended,
            notice,
            draft: draft.map(|state| DraftNotice {
                interrupted_by_restart: self.interrupted_task.is_some()
                    && matches!(&state, DraftState::UnsafeWriter { task, .. }
                        if Some(task) == self.interrupted_task.as_ref().map(|t| &t.0)),
                can_acknowledge: matches!(state, DraftState::UnsafeWriter { .. }),
                state,
                draft: draft_path,
            }),
        };
        self.touch();
    }

    /// An `UnsafeWriter` draft left by a previous session means a task was running when
    /// the app ended: surface it once as an interrupted task.
    pub(super) fn detect_interrupted(&mut self) {
        let state = self
            .shared
            .controller
            .lock()
            .agent_draft_state()
            .ok()
            .flatten();
        let Some(DraftState::UnsafeWriter { task, reason }) = state else {
            return;
        };
        if !reason.contains("previous session ended") {
            return;
        }
        let id = AgentTaskId(task);
        let already = self.store.resident().iter().any(|row| {
            row.task.as_ref() == Some(&id)
                && matches!(&row.kind, RowKind::Outcome(card) if card.kind == OutcomeKind::Interrupted)
        });
        self.interrupted_task = Some(id.clone());
        if already {
            return;
        }
        let task = Some(id);
        let row_id = self.store.next_id();
        self.store.upsert(Arc::new(Row {
            id: row_id,
            task,
            at_unix: now_unix(),
            kind: RowKind::Outcome(OutcomeCard {
                kind: OutcomeKind::Interrupted,
                summary: "The previous session ended while an agent task was running. Its working copy was kept and is locked until you confirm that no agent is still writing to it.".into(),
                published: None,
                draft_retained: true,
            }),
        }));
    }
}
