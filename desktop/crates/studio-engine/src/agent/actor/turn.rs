//! Adapter probe and starting a task: tools, the writer process and the task phase.

use super::*;

impl Actor {
    // ---- adapter probe ----------------------------------------------------------------------------

    /// Probes the configured adapter. An explicit check always launches it; the automatic
    /// re-check reuses a remembered report of the same executable.
    pub(super) fn check_adapter(&mut self, explicit: bool) {
        let Some(adapter) = self.shared.settings.lock().adapter.clone() else {
            return;
        };
        if self.probing {
            return;
        }
        self.probing = true;
        self.adapter.readiness = AdapterReadiness::Checking;
        self.touch();
        let scratch = self
            .shared
            .paths
            .agent(&self.shared.project)
            .join(format!("probe-{}", uuid::Uuid::new_v4().simple()));
        let search = self.shared.search.clone();
        let timeout = self.shared.probe_timeout;
        let processes = self.shared.root_scope.sub_manager();
        let tx = self.shared.tx.clone();
        let generation = self.probe_generation;
        let payload = ProbeInput {
            adapter: adapter.adapter,
            wire: adapter.wire,
            search,
            auth_method: adapter.auth_method,
            scratch,
            timeout,
            processes,
        };
        let refused = self.admit(JobKind::Probe, payload, move |input: ProbeInput| {
            let key = ProbeKey::of(
                &input.adapter,
                input.wire,
                &input.search,
                input.auth_method.as_deref(),
            );
            let probe = || run_probe(input);
            let cache = AdapterProbeCache::global();
            let report = if explicit {
                cache.refresh(key, probe)
            } else {
                cache.get_or_probe(key, probe)
            };
            let _ = tx.send(Msg::Probe { generation, report });
        });
        if let Err((message, _)) = refused {
            self.probing = false;
            self.adapter.readiness = AdapterReadiness::Unchecked;
            self.error_row(present::simple_error(
                "thread_failed",
                "A background worker could not be started",
                &message,
                Some("Close other applications to free system resources, then check again."),
                false,
            ));
        }
    }

    // ---- starting tasks -------------------------------------------------------------------------------

    pub(super) fn submit(
        &mut self,
        id: u64,
        brief: String,
        scope: Option<TaskScope>,
        attachments: Vec<PreparedAttachment>,
    ) {
        if self.active.is_some()
            || self.undo.is_some()
            || !self.queue.is_empty()
            || self.switch_pending.is_some()
            || self.switch_target.is_some()
        {
            let scope_label = scope
                .as_ref()
                .map(TaskScope::label)
                .map(|label| format!(" · {label}"))
                .unwrap_or_default();
            self.queue.push_back(Queued {
                id,
                brief: brief.clone(),
                queued_unix: now_unix(),
                scope,
                attachments,
            });
            self.notice(format!(
                "Queued: {}{scope_label}",
                present::brief_summary(&brief)
            ));
            return;
        }
        self.begin_task(brief, scope, attachments);
        self.sync_registry(1);
    }

    /// Starts queued briefs until one actually starts a task (or the queue is empty).
    pub(super) fn start_next(&mut self) {
        if self.switch_pending.is_some() || self.switch_target.is_some() {
            return;
        }
        while self.active.is_none() && self.undo.is_none() && !self.closing {
            let Some(next) = self.queue.pop_front() else {
                break;
            };
            let started = self.begin_task(next.brief, next.scope, next.attachments);
            self.sync_registry(1);
            if started {
                break;
            }
        }
    }

    pub(super) fn begin_task(
        &mut self,
        brief: String,
        scope: Option<TaskScope>,
        attachments: Vec<PreparedAttachment>,
    ) -> bool {
        let display_brief = brief;
        if let Some(scope) = &scope {
            if let Err(error) = scope.validate() {
                self.error_row(present::simple_error(
                    "invalid_task_scope",
                    "The selected scope is invalid",
                    &error.to_string(),
                    Some("Reselect the scene or frame range, then send the brief again."),
                    false,
                ));
                return false;
            }
            if scope.compiled.is_some()
                && !self
                    .displayed_preview_identity
                    .as_ref()
                    .is_some_and(|identity| scope.is_current_preview(identity))
            {
                self.error_row(present::simple_error(
                    "stale_task_scope",
                    "The selected scope is stale",
                    "The displayed preview changed after this brief was submitted. The saved scene/range was not remapped to the new timeline.",
                    Some("Reselect the scene or frame range and send the brief again."),
                    false,
                ));
                return false;
            }
        }
        let (adapter, build, tools_settings) = {
            let settings = self.shared.settings.lock();
            (
                settings.adapter.clone(),
                settings.build.clone(),
                settings.tools.clone(),
            )
        };
        let Some(adapter) = adapter else {
            self.error_row(present::simple_error(
                "adapter_not_configured",
                "No agent is configured",
                "Configure an ACP adapter in the agent setup first.",
                Some("Open the agent setup and choose an adapter."),
                false,
            ));
            return false;
        };
        let Some(build) = build else {
            self.error_row(present::simple_error(
                "sdk_unavailable",
                "No compatible SDK is installed",
                "Validation compiles the edit against the SDK; none is selected.",
                Some("Install or select the SDK in setup."),
                false,
            ));
            return false;
        };
        // Everything that can fail without side effects comes before the task exists.
        let auth_env = self.shared.auth_env.clone();
        let launch =
            match AdapterLaunch::resolve_with_env(&adapter.adapter, &self.shared.search, |name| {
                auth_env(name)
            }) {
                Ok(launch) => match &adapter.auth_method {
                    Some(method) => launch.with_auth_method(method.clone()),
                    None => launch,
                },
                Err(error) => {
                    self.error_row(present::simple_error(
                        "adapter_missing",
                        "The agent executable was not found",
                        &error.to_string(),
                        Some("Check the adapter configuration, then check readiness again."),
                        false,
                    ));
                    return false;
                }
            };
        let draft_choice = self
            .next_draft_choice
            .clone()
            .unwrap_or(crate::DraftPreparationChoice::FreshFromBase);
        let handoff_context = self.next_handoff_context.clone();
        let task_brief = match handoff_context.as_ref() {
            Some(context) => match context.to_json_envelope() {
                Ok(envelope) => format!(
                    "{display_brief}\n\nStudio handoff context (bounded; provider-native conversation memory was not transferred):\n{envelope}"
                ),
                Err(error) => {
                    self.error_row(present::simple_error(
                        "handoff_context_failed",
                        "Could not prepare provider handoff context",
                        &error.to_string(),
                        Some("Retry the task without relying on prior provider context."),
                        false,
                    ));
                    return false;
                }
            },
            None => display_brief.clone(),
        };
        let begun = self
            .shared
            .controller
            .lock()
            .begin_agent_task_scoped_observed_with_choice(
                &task_brief,
                scope,
                draft_choice.clone(),
                |_| {},
            );
        let context = match begun {
            Ok(context) => {
                self.next_draft_choice = None;
                self.next_handoff_context = None;
                context
            }
            Err(error) => {
                let selected_handoff_revision = matches!(
                    draft_choice,
                    crate::DraftPreparationChoice::FromHandoff { .. }
                );
                if selected_handoff_revision {
                    self.next_draft_choice = None;
                    self.next_handoff_context = None;
                    self.notice("The selected handoff revision became stale before launch; choose the current source again before continuing.");
                }
                self.error_row(present::engine_error(&error, false));
                self.refresh_idle();
                return false;
            }
        };
        let identity = context.identity.clone();
        self.shared.live.set(Some(identity.clone()));
        let policy = self.shared.controller.lock().review_policy();
        self.policy = policy;
        let agent_tools = self.agent_tools.clone();
        let (user_mcp, mut user_notes) =
            agent_tools.launch_servers(|name| std::env::var(name).ok());
        // Which servers the session takes is known only once it starts (an ACP agent says
        // at `initialize` whether it takes HTTP servers): see `on_session_ready`.
        if !user_mcp.is_empty() && adapter.mcp != McpStdioSupport::Baseline {
            user_notes.push(ToolNote::new(
                ToolNoteKind::NoMcp {
                    count: user_mcp.len(),
                },
                format!(
                    "This agent takes no MCP servers, so your {} server(s) are not used.",
                    user_mcp.len()
                ),
            ));
        }
        self.secrets = launch
            .secrets
            .iter()
            .map(String::as_str)
            .chain(
                user_mcp
                    .iter()
                    .flat_map(studio_harness::UserMcpLaunch::secrets),
            )
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        self.secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        self.secrets.dedup();
        let offer = self.offer_tools(
            &build,
            &adapter,
            tools_settings.as_ref(),
            &context,
            &agent_tools.enabled_tools(),
        );
        self.tool_notes.0.extend(user_notes);
        self.session_notes.clear();
        self.shared
            .evidence_images
            .lock()
            .begin_task(identity.task.clone());
        self.task = Some(TaskView {
            id: identity.task.clone(),
            generation: identity.generation,
            phase: TaskPhase::Starting,
            engine_state: TaskState::ContextReady,
            brief: present::brief_summary(&display_brief),
            source_base: present::short(context.source_base.revision().as_str()),
            scope: context.scope.clone(),
            before: None,
            after: None,
            // Image delivery is not known until the adapter finishes negotiating ACP
            // capabilities. Do not present the text-only fallback before then.
            image_limitation: None,
            draft: context.draft.clone(),
            review_policy: policy,
            permission_policy: self.permission_policy,
            repair: RepairView {
                used: 0,
                max: crate::MAX_AUTOMATIC_REPAIRS,
                in_progress: false,
                context_summary: None,
            },
            turns: 0,
            writer: None,
            changes: None,
            validation: None,
            review: None,
            conflict: None,
            error: None,
            stop_requested: false,
            reason: None,
            started_unix: now_unix(),
            ended_unix: None,
            draft_state: None,
        });
        if let Some(previous) = &context.archived_previous {
            self.notice(format!(
                "The previous retained working copy was archived to {}",
                previous.display()
            ));
        }
        let route = offer.route();
        let prompt = present::first_prompt(&context, &route);
        self.active = Some(Active {
            ctx: context,
            phase: TaskPhase::Starting,
            policy,
            permission_policy: self.permission_policy,
            approvals: self.shared.approvals.lock().clone(),
            agent_tools,
            user_mcp,
            adapter,
            launch,
            gate: Gate::new(),
            driver: None,
            writer: None,
            session: None,
            mcp: offer.mcp,
            cli: offer.cli,
            capability: offer.capability,
            pipeline_scopes: None,
            retained: None,
            rejecting: false,
            in_repair: false,
            pending_prompt: Some((prompt, PromptKind::Brief)),
            first_attachments: attachments,
            brief_row: None,
            before_pending: false,
            prompt_held: false,
            evidence_cancel: Arc::new(AtomicBool::new(false)),
            turn: None,
            open_permissions: HashSet::new(),
            open_questions: HashMap::new(),
            engine_waiting: false,
            background: (0, false),
            capture_deferred: false,
            failed: false,
            stop_requested: false,
            promoting: false,
            promote_after_evidence: false,
        });
        // The attachment labels join this row once the first prompt actually carries them.
        let brief_row = self.push_row(RowKind::User {
            text: present::bounded(&display_brief, MAX_ROW_TEXT_BYTES),
            source: UserSource::Brief,
        });
        if let Some(active) = &mut self.active {
            active.brief_row = Some(brief_row);
        }
        // The accepted-awaiting-preview state belongs to the accepted source, not to the
        // task that follows it: only a new durable promotion replaces it.
        self.options = Default::default();
        // A new task starts a new plan; the commands belong to the agent, not the task.
        self.composer.plan.clear();
        self.start_before_evidence();
        if let Err(error) = self.spawn_writer() {
            self.fail_with(*error);
        }
        self.touch();
        true
    }

    pub(super) fn ensure_tools(
        &mut self,
        build: &BuildSettings,
        runtime_dir: PathBuf,
    ) -> Result<(), String> {
        if self.tools.is_some() {
            return Ok(());
        }
        let project = self.shared.project.clone();
        let processes = self.shared.root_scope.sub_manager();
        let runtime = ToolRuntime::start(
            ToolPlacement {
                history: self.shared.paths.project(&project),
                artifacts: self.shared.paths.agent(&project).join("artifacts"),
                builds: self.shared.paths.builds(),
                runtime_dir,
                project,
            },
            build,
            processes,
            self.shared.live.clone(),
        )?;
        self.tools = Some(runtime);
        Ok(())
    }

    /// Registers the task with the tool broker and decides which routes the agent gets.
    /// One capability (a file path; the secret stays inside it) serves both the MCP
    /// server and the `studio-tools` command, so it is granted whenever any route is
    /// offered, whatever host policy says about MCP, and dies with the task.
    pub(super) fn offer_tools(
        &mut self,
        build: &BuildSettings,
        adapter: &AdapterSettings,
        tools: Option<&ToolSettings>,
        context: &AgentTaskContext,
        enabled: &[&'static str],
    ) -> ToolOffer {
        let mut offer = ToolOffer::default();
        self.tool_notes = ToolNotes::default();
        let mut note = |kind, text: String| self.tool_notes.0.push(ToolNote::new(kind, text));
        if enabled.is_empty() {
            note(
                ToolNoteKind::ToolsOff,
                "Every project tool is turned off in Settings → Agent → Tools; the agent has no project tools in this task."
                    .into(),
            );
            return offer;
        }
        let Some(tools) = tools else {
            note(
                ToolNoteKind::Other,
                "No project tool helpers are configured; the agent has no project tools in this task."
                    .into(),
            );
            return offer;
        };
        let policy_allows = adapter.mcp == McpStdioSupport::Baseline;
        let mcp_binary = tools.studio_mcp.clone().filter(|p| p.is_file());
        let cli_binary = tools.studio_tools.clone().filter(|p| p.is_file());
        let want_mcp = policy_allows && mcp_binary.is_some();
        if !want_mcp && cli_binary.is_none() {
            note(
                ToolNoteKind::Other,
                if policy_allows {
                    "Neither the MCP helper nor the `studio-tools` command was found; the agent has no project tools in this task.".to_owned()
                } else {
                    "Project tools over MCP are disabled by host policy and no `studio-tools` command was found; the agent has no project tools in this task.".to_owned()
                },
            );
            return offer;
        }
        if let Err(error) = self.ensure_tools(build, tools.runtime_dir.clone()) {
            self.tool_notes = ToolNotes(vec![ToolNote::other(format!(
                "Project tools are unavailable ({error}); the agent has no project tools in this task."
            ))]);
            return offer;
        }
        let runtime = self.tools.as_ref().expect("tools started");
        let grant = match runtime.grant(context, enabled) {
            Ok(grant) => grant,
            Err(error) => {
                self.tool_notes = ToolNotes(vec![ToolNote::other(format!(
                    "Project tools are unavailable for this task ({error})."
                ))]);
                return offer;
            }
        };
        let mut notes: Vec<String> = Vec::new();
        if let (true, Some(binary)) = (want_mcp, &mcp_binary) {
            match ToolRuntime::mcp_server(&grant, binary) {
                Ok(server) => offer.mcp = Some(server),
                Err(error) => notes.push(format!("The MCP route could not be prepared ({error}).")),
            }
        }
        if let Some(command) = cli_binary {
            offer.cli = Some(CliRoute {
                command,
                capability_file: grant.capability_file.clone(),
            });
        }
        if offer.mcp.is_none() && offer.cli.is_some() {
            if !policy_allows {
                notes.push("Project tools over MCP are disabled by host policy; the `studio-tools` command is offered instead.".into());
            } else if mcp_binary.is_none() {
                notes.push(
                    "The MCP helper was not found; the `studio-tools` command is offered instead."
                        .into(),
                );
            }
        }
        if offer.mcp.is_none() && offer.cli.is_none() {
            runtime.end_task(&context.identity);
            notes.push("The agent has no project tools in this task.".into());
        } else {
            offer.capability = Some(grant.capability_file);
            offer.tools = enabled.to_vec();
        }
        self.tool_notes = ToolNotes(notes.into_iter().map(ToolNote::other).collect());
        offer
    }

    /// Starts (or, for the repair, restarts) the adapter in the task's own scope.
    pub(super) fn spawn_writer(&mut self) -> Result<(), Box<StructuredError>> {
        let controller = self.shared.controller.clone();
        let limits = self.shared.limits.clone();
        let active = self.active.as_mut().expect("active task");
        let identity = active.ctx.identity.clone();
        let provider = active.adapter.provider.clone();
        let scope = controller.lock().agent_processes(&identity);
        let scope = scope.map_err(|e| Box::new(present::engine_error(&e, true)))?;
        let (resume_session, closure_changed) = if let Some(pending) =
            self.pending_resume_session.take()
        {
            let launch_digest = session_launch_digest(active).ok();
            let accepted_revision = self.shared.controller.lock().state().accepted().to_string();
            if pending.provider_id == active.adapter.provider
                && Some(pending.launch_digest) == launch_digest
                && pending.draft_path == active.ctx.draft
                && pending.last_source_revision == active.ctx.source_base.revision().to_string()
                && pending.last_accepted_revision == accepted_revision
            {
                (Some(pending.wire_session_id), false)
            } else {
                (None, true)
            }
        } else {
            (None, false)
        };
        self.resume_attempt = resume_session.is_some();
        let config = DriverConfig {
            provider: active.adapter.provider.clone(),
            task: identity.task.0.clone(),
            cwd: active.ctx.draft.clone(),
            launch: active.launch.clone(),
            limits,
            resume_session,
            ownership: active.adapter.ownership.clone(),
            mode: DriverMode::Full,
            mcp_servers: active.mcp.iter().cloned().collect(),
            user_mcp_servers: active.user_mcp.clone(),
            mcp_stdio: active.adapter.mcp,
        };
        let driver = studio_harness::start_driver(active.adapter.wire, config, &scope)
            .map_err(|f| Box::new(present::provider_error(&f, true)))?;
        // What the supervisor verified for the process it just spawned, not a claim made
        // before the launch; observations during the task only demote it.
        let ownership = driver.ownership();
        self.adapter.last_launch = Some(LaunchContainment {
            ownership: ownership.label().to_owned(),
            contained: ownership.is_qualified(),
        });
        let started = controller
            .lock()
            .agent_writer_started(&identity, ownership.clone());
        let generation = match started {
            Ok(generation) => generation,
            Err(error) => {
                let _ = driver.shutdown();
                return Err(Box::new(present::engine_error(&error, true)));
            }
        };
        active.driver = Some(driver);
        active.writer = Some(generation);
        active.session = None;
        active.turn = None;
        active.failed = false;
        active.open_permissions.clear();
        active.open_questions.clear();
        active.engine_waiting = false;
        // A pump of an earlier driver must not act on this one.
        self.driver_epoch += 1;
        let dropped = self.composer.retire();
        self.composer.driver_started(&provider);
        let ownership = ownership.label().to_owned();
        let phase = if active.in_repair {
            TaskPhase::Repairing
        } else {
            TaskPhase::Starting
        };
        if let Some(task) = &mut self.task {
            task.writer = Some(WriterView {
                generation: generation.value(),
                ownership,
                provider_session: None,
            });
        }
        self.set_phase(phase);
        if dropped {
            self.warn(
                "A message waiting to be sent was dropped because the agent session was replaced.",
            );
        }
        if closure_changed {
            self.notice("Saved session provider, adapter closure, or draft location changed; starting fresh session.");
        }
        Ok(())
    }

    pub(super) fn set_phase(&mut self, phase: TaskPhase) {
        if let Some(active) = &mut self.active {
            active.phase = phase;
        }
        let repairing = self
            .active
            .as_ref()
            .is_some_and(|a| a.in_repair && a.driver.is_some());
        if let Some(task) = &mut self.task {
            task.phase = phase;
            task.engine_state = engine_state_of(phase);
            task.repair.in_progress = repairing && phase == TaskPhase::Repairing;
        }
        self.touch();
    }
}
