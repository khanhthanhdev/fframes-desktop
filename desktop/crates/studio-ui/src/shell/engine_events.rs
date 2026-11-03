//! Commands to the engine and what comes back: engine events, presets, the agent workflow, opening
//! projects, file pickers and export.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use gpui::{Context, Window};
use studio_engine::agent::{AgentWorkflow, BuildSettings};
use studio_engine::engine::backend::{AgentSdk, Command};
use studio_engine::engine::fence::{PresentationVerdict, presentation_verdict};
use studio_engine::engine::handle::{CommandFinished, EngineEvent, WorkflowOpen};
use studio_engine::presets::{PresetCommand, PresetReport};
use studio_engine::services::build_service::BuildService;
use studio_engine::services::export_service::{ExportControl, ExportProgress};
use studio_engine::services::preview_coordinator::{
    PreviewCoordinator, PreviewProps, PreviewTiming, SeekIntent,
};

use super::{Picker, StudioShell};
use crate::agent::Attachment;
use crate::export::settings::{ExportTarget, deck_settings};
use crate::preset_panel::{PresetEvent, PresetPick};

impl StudioShell {
    /// The text shown while the accepted source has no matching preview yet.
    pub fn awaiting_preview(&self) -> Option<&str> {
        self.awaiting_preview.as_deref()
    }

    /// Guarded handoff of a just-published task revision to playback.
    ///
    /// `staged` is the preview Stage 2's validation prepared for the candidate; it is
    /// adopted under the engine's fresh authorization and re-primed at the latest
    /// playhead before the usual matching video/audio commit (with a new audio epoch)
    /// installs it. If there is no staged preview or authorization, or adoption fails,
    /// the old preview keeps playing, the accepted source is labelled as awaiting its
    /// preview, and an ordinary build of the accepted source is queued. Committed
    /// source is never undone implicitly.
    ///
    /// Returns `Ok(())` when the coordinator adopted the staged preview, otherwise the
    /// reason the accepted revision is left awaiting an ordinary preview build.
    pub fn adopt_promotion(
        &mut self,
        staged: Option<studio_engine::services::preview_coordinator::StagedPreview>,
        promotion: &studio_engine::Promotion,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let published = promotion.record.published.as_str();
        self.awaiting_preview = Some(format!(
            "Accepted source {} is awaiting its preview; the preview still shows the previous revision.",
            &published[..12]
        ));
        self.watch_acceptance(cx);
        let handoff = match (
            staged,
            promotion.authorization.as_ref(),
            self.preview.as_ref(),
        ) {
            (Some(staged), Some(authorization), Some(preview)) => {
                let latest = SeekIntent {
                    identity: self
                        .preview_state
                        .displayed()
                        .cloned()
                        .unwrap_or_else(|| staged.ready().identity().clone()),
                    serial: self.preview_state.serial(),
                    position: self.preview_state.position(),
                    scale: self.preview_state.scale(),
                };
                preview
                    .adopt(staged, authorization, latest)
                    .map(|()| authorization.clone())
                    .map_err(|e| e.to_string())
            }
            (_, None, _) => {
                Err("the source changed again before the preview could be handed off".into())
            }
            _ => Err("no staged preview is available".into()),
        };
        // The published bytes changed the source: the presentation catches up once idle.
        self.refresh_presentation();
        let outcome = match handoff {
            Ok(authorization) => {
                // Preview-state bookkeeping follows the adoption so a rejected adoption
                // leaves nothing half-begun.
                self.preview_state.begin_promotion(&authorization);
                Ok(())
            }
            Err(reason) => {
                self.error = Some(format!(
                    "Preview handoff failed ({reason}); rebuilding the accepted source."
                ));
                self.dispatch(Command::Build, cx);
                Err(reason)
            }
        };
        cx.notify();
        outcome
    }

    pub(super) fn discard_install(&mut self, cx: &mut Context<Self>) {
        if self.pending_ready.take().is_some() {
            self.audio.stop(self.transport.epoch());
            self.output = None;
            self.audio_ready_epoch = None;
        }
        if self.resume_install {
            self.resume_install = false;
            self.toggle_playback(cx);
        }
    }

    pub(super) fn dispatch(&mut self, command: Command, cx: &mut Context<Self>) {
        if self.busy || self.closed.load(Ordering::Acquire) {
            return;
        }
        if matches!(
            &command,
            Command::Close
                | Command::Create(..)
                | Command::Import(_)
                | Command::Open(_)
                | Command::Locate(_, _)
        ) {
            self.cancel_export(cx);
        }
        if let Command::Open(path) | Command::Locate(path, _) = &command {
            self.failed_open = Some(path.clone());
        }
        if matches!(command, Command::CancelBuild | Command::Build) {
            if let Some(preview) = &self.preview {
                preview.cancel_build();
            }
            self.preview_state.cancel_build();
            self.discard_install(cx);
        }
        if matches!(command, Command::Close) {
            self.detach_preview(cx);
        }
        // A command that replaces the open project closes the project's agent workflow
        // first (cancelling its task and reaping its scopes), on the background thread and
        // before the controller it shares is closed.
        let closing = self.take_workflow_for(&command, cx);
        let leaves_session = closing.is_some() || self.leaves_session(&command);
        let refresh = matches!(command, Command::Refresh);
        self.busy = true;
        let id = self.engine.send(command, closing);
        self.in_flight = Some((id, refresh));
        self.app.update(cx, |app, _| app.sent(id, leaves_session));
        cx.notify();
    }

    /// Installs the SDK an updated app ships, once per launch, unless that is turned off
    /// in Settings → Help & About. Only while no project is open, so no build or agent
    /// task uses the SDK it replaces.
    fn install_sdk_update(&mut self, cx: &mut Context<Self>) {
        if self.sdk_auto_update_tried
            || self.sdk_auto_update_disabled
            || !self.setup_checked
            || self.busy
            || self.presentation.project.is_some()
            || self.presentation.sdk_update.is_none()
        {
            return;
        }
        self.sdk_auto_update_tried = true;
        self.dispatch(Command::InstallSdk, cx);
    }

    /// Whether `command` closes or replaces the open project.
    fn leaves_session(&self, command: &Command) -> bool {
        let current = self.presentation.project.as_ref().map(|p| &p.root);
        match command {
            Command::Close | Command::Import(_) | Command::Create(..) => true,
            Command::Open(path) | Command::Locate(path, _) | Command::Independent(path) => {
                current != Some(path)
            }
            _ => false,
        }
    }

    pub(super) fn engine_event(&mut self, event: EngineEvent, cx: &mut Context<Self>) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        match event {
            EngineEvent::CommandFinished(finished) => {
                self.command_finished(*finished, cx);
                self.refresh_if_wanted();
                // Preview news held back while the command ran.
                self.drain_preview(cx);
                self.install_sdk_update(cx);
            }
            EngineEvent::PreviewGated(gated) => {
                self.preview_gated(*gated, cx);
                self.drain_preview(cx);
            }
            EngineEvent::Refreshed { presentation, .. } => {
                self.presentation_refresh_pending = false;
                if let Some(presentation) = presentation {
                    self.presentation = *presentation;
                    self.push_presentation_views(cx);
                }
                self.refresh_if_wanted();
                cx.notify();
            }
            EngineEvent::SourceChanged { .. } => {
                // A native picker or a command defers it; the engine says it again.
                if !self.busy {
                    self.dispatch(Command::Refresh, cx);
                }
            }
            EngineEvent::WorkflowChanged { serial, .. } => {
                if self.agent.attached == Some(serial) {
                    self.agent_refreshed(cx);
                }
            }
            EngineEvent::CleanupProblem(report) => {
                // Background teardown only ever reports problems; show the newest one.
                self.error = Some(format!(
                    "Background cleanup ({}): {}",
                    report.label, report.problem
                ));
                cx.notify();
            }
        }
    }

    fn command_finished(&mut self, finished: CommandFinished, cx: &mut Context<Self>) {
        let CommandFinished {
            id,
            result,
            presentation,
            build: spec,
            preset: report,
            ..
        } = finished;
        let Some((_, refresh)) = self.in_flight.take_if(|(sent, _)| *sent == id) else {
            return;
        };
        // Jobs run in order: a refresh or preview news sent before this command has been
        // answered, or dropped with the session this command left.
        self.presentation_refresh_pending = false;
        self.preview_gate_pending = false;
        self.busy = false;
        let edit = self.timeline_in_flight.take_if(|(sent, _)| *sent == id);
        let props_edit = self.props_in_flight.take_if(|(sent, _)| *sent == id);
        if let Some(presentation) = presentation {
            let changed_session =
                self.preview_session.as_ref() != presentation.state.as_ref().map(|s| s.session());
            if changed_session {
                self.detach_preview(cx);
                if let (Some(state), Some(owner)) = (&presentation.state, &presentation.owner) {
                    let preview = PreviewCoordinator::new(owner.sub_manager());
                    let wake = self.preview_wake.clone();
                    preview.set_listener(move || {
                        let _ = wake.try_send(());
                    });
                    self.preview = Some(Arc::new(preview));
                    self.thumbnails_sent = None;
                    self.preview_session = Some(state.session().clone());
                    self.preview_state = studio_engine::PreviewState::default();
                }
            }
            let verdict = if changed_session {
                PresentationVerdict::CancelPreparation
            } else {
                presentation_verdict(
                    &self.preview_state,
                    self.presentation.state.as_ref().map(|s| s.generation()),
                    presentation.state.as_ref(),
                )
            };
            match verdict {
                PresentationVerdict::Older => {
                    // Captured before a publication this shell already knows: it
                    // replaces nothing and cancels nothing; a fresh one is taken.
                    self.refresh_presentation();
                }
                verdict => {
                    if verdict == PresentationVerdict::CancelPreparation {
                        if let Some(preview) = &self.preview {
                            preview.cancel_build();
                        }
                        self.preview_state.cancel_build();
                        self.discard_install(cx);
                    }
                    self.presentation = presentation;
                    self.check_onboarding(cx);
                    self.sync_agent(cx);
                    self.push_presentation_views(cx);
                }
            }
        }
        if let Some(spec) = spec {
            self.preview_state.begin(spec.tag.clone());
            if let Some(preview) = &self.preview {
                preview.build(spec);
            }
        }
        let refused = result.is_err();
        match result {
            Ok(()) => {
                if !refresh {
                    self.error = None;
                    self.failed_open = None;
                }
            }
            Err(error) => self.error = Some(error),
        }
        if let Some(report) = report {
            self.adopt_preset_report(report, cx);
        }
        if let Some((_, command)) = edit {
            self.timeline_edit_finished(command, refused, cx);
        }
        if let Some((_, command)) = props_edit {
            self.props_edit_finished(command, refused);
        }
        self.flush_timeline_edits(cx);
        self.flush_props_edits(cx);
        cx.notify();
    }

    /// Shows a preset command's outcome. Only a committed revision asks for a matching
    /// preview: the previous playable preview stays displayed under the awaiting label
    /// until it is ready (or for good if its build fails), and the source is never
    /// rolled back implicitly.
    fn adopt_preset_report(&mut self, report: PresetReport, cx: &mut Context<Self>) {
        let request = report.preview_request();
        let catalog_changed = report.catalog_changed;
        self.preset_panel
            .update(cx, |panel, cx| panel.report(report, cx));
        if catalog_changed {
            self.refresh_presentation();
        }
        if let Some(request) = request {
            self.awaiting_preview = Some(request.label);
            self.watch_acceptance(cx);
            self.refresh_presentation();
            self.dispatch(Command::Build, cx);
        }
    }

    /// Hands what a new presentation changed to the views and the preview: the preset
    /// panel's state and the timing document the preview plays.
    pub(super) fn push_presentation_views(&mut self, cx: &mut Context<Self>) {
        self.sync_export_support(cx);
        let view = self.presentation.preset.clone();
        self.preset_panel
            .update(cx, |panel, cx| panel.set_view(view, cx));
        let snapshot = self.presentation.timeline.clone();
        self.timeline
            .update(cx, |timeline, cx| timeline.set_snapshot(snapshot, cx));
        if let (Some(preview), Some(timeline)) = (&self.preview, &self.presentation.timeline)
            && self.timing_sent != Some(timeline.generation)
        {
            preview.set_timing(PreviewTiming {
                json: timeline.timing_json.clone(),
                generation: timeline.generation,
            });
            self.timing_sent = Some(timeline.generation);
        }
        let props = self.presentation.props.clone();
        if let (Some(preview), Some(props)) = (&self.preview, &props)
            && self.props_sent != Some(props.generation)
        {
            preview.set_props(PreviewProps {
                json: props.props_json.clone(),
                generation: props.generation,
            });
            self.props_sent = Some(props.generation);
        }
        self.props_panel
            .update(cx, |panel, cx| panel.set_snapshot(props, cx));
    }

    pub(super) fn run_preset(&mut self, command: PresetCommand, cx: &mut Context<Self>) {
        // The fence is the revision this window last showed.
        let expected = self
            .presentation
            .state
            .as_ref()
            .map(|state| state.source().clone());
        self.dispatch(Command::Preset(command, expected), cx);
    }

    pub(super) fn preset_event(
        &mut self,
        event: PresetEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            PresetEvent::Leave => window.focus(&self.focus, cx),
            PresetEvent::Run(command) => self.run_preset(command, cx),
            PresetEvent::Pick(PresetPick::ImportFolder) => self.pick(Picker::PresetImport, cx),
            PresetEvent::Pick(PresetPick::ImportCss) => self.pick(Picker::PresetCss, cx),
            PresetEvent::Pick(PresetPick::Source) => self.pick(Picker::PresetSource, cx),
            PresetEvent::Pick(PresetPick::Export { key }) => {
                self.pick(Picker::PresetExport(key), cx)
            }
        }
    }

    /// The workflow a command must close before it may run: every command that closes or
    /// replaces the open project. Re-selecting the same folder keeps the workflow.
    fn take_workflow_for(
        &mut self,
        command: &Command,
        cx: &mut Context<Self>,
    ) -> Option<Arc<AgentWorkflow>> {
        if !self.leaves_session(command) {
            return None;
        }
        self.agent.opening = None;
        self.agent.session = None;
        self.agent.attached = None;
        self.agent.applied_sdk = None;
        self.close_review(cx);
        self.panel.update(cx, |panel, cx| panel.detach(cx));
        self.agent.workflow.take()
    }

    /// Keeps the agent workflow in step with the open project: one workflow per open
    /// session, opened on its own thread (it replays the conversation log), never
    /// launching an agent. Also forwards SDK readiness, which stays separate from the
    /// agent's.
    fn sync_agent(&mut self, cx: &mut Context<Self>) {
        let session = self
            .presentation
            .state
            .as_ref()
            .map(|state| state.session().clone());
        let sdk_ready = self.presentation.agent_sdk.is_some();
        self.panel
            .update(cx, |panel, cx| panel.set_sdk_ready(sdk_ready, cx));
        let Some(session) = session else {
            if let Some(workflow) = self.agent.workflow.take() {
                close_workflow_detached(workflow);
            }
            self.agent.session = None;
            self.agent.opening = None;
            self.agent.attached = None;
            self.close_review(cx);
            self.panel.update(cx, |panel, cx| panel.detach(cx));
            return;
        };
        if self.agent.session.as_ref() != Some(&session) {
            // A different open session (the project was replaced some other way).
            if let Some(workflow) = self.agent.workflow.take() {
                close_workflow_detached(workflow);
            }
            self.agent.attached = None;
            self.agent.opening = None;
            self.close_review(cx);
            self.panel.update(cx, |panel, cx| panel.detach(cx));
            self.agent.session = Some(session);
        }
        if self.agent.workflow.is_none() && self.agent.opening.is_none() {
            self.start_agent_open();
        }
        self.sync_agent_build();
    }

    fn start_agent_open(&mut self) {
        let Some(session) = self.presentation.session else {
            return;
        };
        self.agent.serial += 1;
        let serial = self.agent.serial;
        self.agent.opening = Some(serial);
        let service = self.builds.clone();
        let build = self
            .presentation
            .agent_sdk
            .as_ref()
            .map(|sdk| agent_build(sdk, &service));
        self.agent.applied_sdk = self.presentation.agent_sdk.as_ref().map(|s| s.dir.clone());
        self.engine.open_workflow(WorkflowOpen {
            session,
            serial,
            build,
            inbox: self.agent.inbox.clone(),
            policy: self.agent.ownership.clone(),
        });
    }

    /// Hands a changed SDK selection to the workflow (it refuses while a task runs; the
    /// task ending wakes the agent and it is tried again).
    fn sync_agent_build(&mut self) {
        let Some(workflow) = &self.agent.workflow else {
            return;
        };
        let desired = self.presentation.agent_sdk.as_ref().map(|s| s.dir.clone());
        if desired == self.agent.applied_sdk {
            return;
        }
        let service = self.builds.clone();
        if workflow
            .set_build(
                self.presentation
                    .agent_sdk
                    .as_ref()
                    .map(|sdk| agent_build(sdk, &service)),
            )
            .is_ok()
        {
            self.agent.applied_sdk = desired;
        }
    }

    /// The agent half of the window, run whenever a hand-off, an open or a history page
    /// woke it: adopt a finished open, adopt committed promotions on this thread (the only
    /// place the preview coordinator is touched), then [`Self::agent_refreshed`].
    pub(super) fn agent_woken(&mut self, cx: &mut Context<Self>) {
        let inbox = self.agent.inbox.clone();
        inbox.take_wake();
        for opened in inbox.take_opened() {
            self.adopt_opened(opened, cx);
        }
        for queued in inbox.take_handoffs() {
            if self.agent.attached != Some(queued.serial) {
                // Not this project's workflow any more: the staged worker is reaped by
                // dropping it; nothing is adopted.
                continue;
            }
            let published = queued
                .handoff
                .promotion
                .record
                .published
                .as_str()
                .to_owned();
            let outcome =
                self.adopt_promotion(queued.handoff.staged, &queued.handoff.promotion, cx);
            if let Some(workflow) = &self.agent.workflow {
                let _ = workflow.report_handoff(&published, outcome);
            }
        }
        self.agent_refreshed(cx);
    }

    /// After the workflow changed: report the displayed revision, retry a refused build
    /// selection (a task ending publishes a snapshot) and refresh the panel. Takes the
    /// attached workflow's wake first, so the next publish wakes the window again.
    fn agent_refreshed(&mut self, cx: &mut Context<Self>) {
        if self.agent.attached.is_some()
            && let Some(wake) = &self.agent.wake
        {
            wake.take();
        }
        self.report_displayed();
        self.sync_agent_build();
        self.panel.update(cx, |panel, cx| {
            panel.poll(cx);
        });
        self.sync_review(cx);
    }

    /// Tells the workflow which revision the preview now displays.
    pub(super) fn report_displayed(&mut self) {
        if let Some(workflow) = &self.agent.workflow
            && let Some(revision) = self.agent.displayed.take()
        {
            let _ = workflow.preview_displayed(&revision);
        }
    }

    /// Forwards the playhead to the workflow when it moved.
    pub(super) fn forward_playhead(&mut self) {
        let Some(workflow) = &self.agent.workflow else {
            return;
        };
        let position = self.preview_state.position();
        if position != self.agent.playhead {
            self.agent.playhead = position;
            workflow.set_playhead(position);
        }
    }

    /// Replaces the presentation with the controller's current one. Unlike a command's
    /// completion this never cancels preparation: a promotion advances the generation on
    /// purpose and the staged preview adopted for it must survive.
    fn refresh_presentation(&mut self) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if self.presentation_refresh_pending {
            self.refresh_wanted = true;
            return;
        }
        self.presentation_refresh_pending = true;
        self.engine.refresh();
    }

    /// Sends the refresh wanted while another was pending.
    fn refresh_if_wanted(&mut self) {
        if std::mem::take(&mut self.refresh_wanted) {
            self.refresh_presentation();
        }
    }

    /// Checks once a second, only while a source is awaiting its preview, whether it is
    /// stranded; stops by itself when nothing awaits.
    pub(super) fn watch_acceptance(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.acceptance_watch, true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(again) = this.update(cx, |shell, cx| {
                    shell.rebuild_stranded_acceptance(cx);
                    let again =
                        shell.awaiting_preview.is_some() && !shell.closed.load(Ordering::Acquire);
                    shell.acceptance_watch = again;
                    again
                }) else {
                    break;
                };
                if !again {
                    break;
                }
            }
        })
        .detach();
    }

    /// Safety net for an accepted source whose handoff never produced a preview (a
    /// refresh or a failed staging cancelled it): after a grace period with nothing being
    /// prepared, build the accepted source the ordinary way. Never undoes anything.
    pub(super) fn rebuild_stranded_acceptance(&mut self, cx: &mut Context<Self>) {
        if self.awaiting_preview.is_none() {
            self.awaiting_since = None;
            return;
        }
        let since = *self.awaiting_since.get_or_insert_with(Instant::now);
        let preparing = matches!(
            self.preview_state.status,
            studio_engine::PreviewStatus::Building | studio_engine::PreviewStatus::Preparing
        ) || self.pending_ready.is_some();
        if preparing {
            self.awaiting_since = Some(Instant::now());
        } else if since.elapsed() > Duration::from_secs(8)
            && !self.busy
            && self.preview.is_some()
            && self
                .presentation
                .project
                .as_ref()
                .is_some_and(|p| p.worker_available)
        {
            self.awaiting_since = Some(Instant::now());
            self.dispatch(Command::Build, cx);
        }
    }

    fn adopt_opened(
        &mut self,
        opened: studio_engine::providers::host::Opened,
        cx: &mut Context<Self>,
    ) {
        let wanted = self.agent.opening == Some(opened.serial);
        let workflow = match opened.result {
            Ok(workflow) => workflow,
            Err(error) => {
                if wanted {
                    self.agent.opening = None;
                    self.error = Some(format!("Agent workflow unavailable: {error}"));
                    cx.notify();
                }
                return;
            }
        };
        if !wanted {
            // Opened for a project the shell has left.
            std::thread::spawn(move || workflow.close());
            return;
        }
        self.agent.opening = None;
        self.agent.wake = Some(opened.wake);
        let workflow = Arc::new(workflow);
        let preview_identity = self
            .displayed
            .as_ref()
            .map(|preview| preview.identity().clone());
        let _ = workflow.set_displayed_preview_identity(preview_identity);
        let _ = workflow.set_permission_policy(self.permission_policy);
        self.follow_agent_tools_project(cx);
        let _ = workflow.set_agent_tools(self.agent_tools.read(cx).effective());
        self.agent.workflow = Some(workflow.clone());
        self.agent.attached = Some(opened.serial);
        self.agent.playhead = usize::MAX;
        if let Some(paths) = self.presentation.paths.clone() {
            let scope = self.timeline.read(cx).selected_scope();
            let root = self.presentation.project.as_ref().map(|p| p.root.clone());
            self.panel.update(cx, |panel, cx| {
                panel.attach(
                    Attachment {
                        workflow,
                        paths,
                        serial: opened.serial,
                        adapter: opened.adapter,
                        registry: opened.registry,
                        settings_error: opened.settings_error,
                        policy: opened.policy,
                    },
                    cx,
                );
                panel.set_project_root(root, cx);
            });
            self.install_task_scope(scope, cx);
        }
        self.sync_agent_build();
        cx.notify();
    }

    pub(super) fn pick(&mut self, kind: Picker, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        // A watcher refresh must not discard the result of an active native dialog.
        self.busy = true;
        cx.notify();
        if matches!(
            kind,
            Picker::Create(_) | Picker::Restore | Picker::PresetExport(_) | Picker::Export
        ) {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            // A deck asks for a new folder name; the folder must not exist.
            let deck = self.export_panel.target == ExportTarget::Deck;
            let export_file_name = match self.export_settings().container {
                _ if deck => "Slides",
                fframes_studio_protocol::export::ExportContainer::Mp4 => "Exported video.mp4",
                fframes_studio_protocol::export::ExportContainer::Webm => "Exported video.webm",
            };
            let picker = cx.prompt_for_new_path(
                &home,
                Some(match &kind {
                    Picker::Create(choice)
                        if choice.template == studio_project::ProjectTemplate::SlideDeck =>
                    {
                        "New slide deck"
                    }
                    Picker::Create(_) => "New video",
                    Picker::PresetExport(_) => "Exported preset",
                    Picker::Export => export_file_name,
                    _ => "Recovered video",
                }),
            );
            // Export the checkpoint selected before the picker opened, even if
            // current source is degraded. No source installation occurs here.
            let accepted = self
                .presentation
                .project
                .as_ref()
                .map(|p| p.accepted.clone());
            cx.spawn(async move |this, cx| {
                let result = picker.await;
                let _ = this.update(cx, |shell, cx| {
                    if shell.closed.load(Ordering::Acquire) {
                        return;
                    }
                    shell.busy = false;
                    // Preview news held back while the dialog was open.
                    shell.drain_preview(cx);
                    match result {
                        Ok(Ok(Some(path))) => {
                            if let Picker::PresetExport(key) = kind {
                                shell.run_preset(PresetCommand::Export { key, dest: path }, cx);
                            } else if let Picker::Create(choice) = kind {
                                shell.dispatch(Command::Create(path, choice), cx);
                            } else if matches!(kind, Picker::Export) {
                                shell.start_export(path, deck, cx);
                            } else if let Some(accepted) = accepted {
                                shell.dispatch(Command::Restore(path, accepted), cx);
                            }
                        }
                        Ok(Ok(None)) => (),
                        error => shell.error = Some(format!("Native picker failed: {error:?}; check the desktop portal/file chooser and retry")),
                    }
                    cx.notify();
                });
            }).detach();
        } else {
            let label = match &kind {
                Picker::Asset => "Asset",
                Picker::Import => "Import",
                Picker::Sdk => "SDK",
                Picker::SdkBundle => "Offline SDK bundle folder",
                Picker::Locate(_) => "Locate",
                Picker::PresetImport => "Import preset folder",
                Picker::PresetCss => "CSS report",
                Picker::PresetSource => "Make a preset from",
                _ => "Open",
            };
            let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: matches!(
                    kind,
                    Picker::Asset | Picker::Import | Picker::PresetCss | Picker::PresetSource
                ),
                directories: !matches!(
                    kind,
                    Picker::Asset | Picker::Import | Picker::PresetCss | Picker::PresetSource
                ),
                multiple: false,
                prompt: Some(format!("{label}…").into()),
            });
            cx.spawn(async move |this, cx| {
                let result = picker.await;
                let _ = this.update(cx, |shell, cx| {
                    if shell.closed.load(Ordering::Acquire) {
                        return;
                    }
                    shell.busy = false;
                    // Preview news held back while the dialog was open.
                    shell.drain_preview(cx);
                    match result {
                        Ok(Ok(Some(paths))) => if let Some(path) = paths.into_iter().next() {
                            let command = match kind {
                                Picker::Asset => Command::Asset(path),
                                Picker::Import => Command::Import(path),
                                Picker::Sdk => Command::SelectSdk(path),
                                Picker::SdkBundle => Command::ImportSdkBundle(path),
                                Picker::Locate(id) => Command::Locate(path, id),
                                Picker::PresetImport => {
                                    return shell
                                        .run_preset(PresetCommand::ImportFolder(path), cx);
                                }
                                Picker::PresetCss => {
                                    return shell.run_preset(PresetCommand::ImportCss(path), cx);
                                }
                                Picker::PresetSource => {
                                    return shell.run_preset(
                                        PresetCommand::FromSource(path.to_string_lossy().into()),
                                        cx,
                                    );
                                }
                                _ => Command::Open(path),
                            };
                            shell.dispatch(command, cx);
                        },
                        Ok(Ok(None)) => (),
                        error => shell.error = Some(format!("Native picker failed: {error:?}; check the desktop portal/file chooser and retry")),
                    }
                    cx.notify();
                });
            }).detach();
        }
    }

    /// Starts a video export to the file `destination`, or with `deck` a slide deck
    /// export into the new folder `destination`.
    fn start_export(&mut self, destination: PathBuf, deck: bool, cx: &mut Context<Self>) {
        if self.export_cancelled.is_some() {
            return;
        }
        let (Some(session), Some(sdk)) = (
            self.presentation.session,
            self.presentation
                .agent_sdk
                .as_ref()
                .map(|sdk| sdk.dir.clone()),
        ) else {
            self.export_status = Some(ExportProgress::Failed(
                "Select a compatible installed SDK and open a project before exporting".into(),
            ));
            cx.notify();
            return;
        };
        let settings = if deck {
            deck_settings(self.export_panel.choice.unwrap_or_default())
        } else {
            self.export_settings()
        };
        // The container decides the extension: the worker refuses any other.
        let extension = settings.container.extension();
        let destination = if deck
            || destination.extension().and_then(|value| value.to_str()) == Some(extension)
        {
            destination
        } else {
            destination.with_extension(extension)
        };
        let started = if deck {
            self.engine
                .start_deck_export(session, destination, settings, sdk, self.builds.clone())
        } else {
            self.engine
                .start_export(session, destination, settings, sdk, self.builds.clone())
        };
        let job = match started {
            Ok(job) => job,
            Err(error) => {
                self.export_status = Some(ExportProgress::Failed(error));
                cx.notify();
                return;
            }
        };
        let control = job.control.clone();
        self.export_cancelled = Some(control.clone());
        self.export_processes = Some(job.processes.clone());
        self.export_status = Some(ExportProgress::Started {
            revision: "capturing validated revision".into(),
            label: "Preparing immutable project snapshot".into(),
        });
        cx.spawn(async move |this, cx| {
            while let Ok(event) = job.progress.recv().await {
                let terminal = is_terminal(&event);
                let shown = this.update(cx, |shell, cx| {
                    shell.export_progress(&control, Some(event), cx)
                });
                if terminal || shown.is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |shell, cx| shell.export_progress(&control, None, cx));
        })
        .detach();
        cx.notify();
    }

    /// Shows the progress of the export `control` runs; `None` when its thread stopped
    /// without a terminal event. A replaced export's progress is ignored.
    fn export_progress(
        &mut self,
        control: &Arc<ExportControl>,
        event: Option<ExportProgress>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .export_cancelled
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, control))
        {
            return;
        }
        let event = event.unwrap_or_else(|| {
            if control.is_cancelled() {
                ExportProgress::Cancelled
            } else {
                ExportProgress::Failed("Export worker stopped without a completion result".into())
            }
        });
        if is_terminal(&event) {
            self.export_cancelled = None;
            self.export_processes = None;
        }
        self.export_status = Some(event);
        cx.notify();
    }

    pub(super) fn cancel_export(&mut self, cx: &mut Context<Self>) {
        let Some(cancelled) = &self.export_cancelled else {
            return;
        };
        if !cancelled.request_cancel() {
            if cancelled.is_publishing() {
                self.export_status = Some(ExportProgress::Verifying);
                cx.notify();
            }
            return;
        }
        if let Some(processes) = self.export_processes.clone() {
            cx.background_executor()
                .spawn(async move {
                    processes.shutdown(Duration::ZERO);
                })
                .detach();
        }
        self.export_status = Some(ExportProgress::Cancelled);
        cx.notify();
    }
}

fn agent_build(sdk: &AgentSdk, service: &BuildService) -> BuildSettings {
    BuildSettings {
        service: service.clone(),
        sdk: sdk.dir.clone(),
        compatibility: sdk.compatibility.clone(),
    }
}

/// Closes a workflow away from the UI thread (cancels its task, reaps its scopes).
fn close_workflow_detached(workflow: Arc<AgentWorkflow>) {
    std::thread::spawn(move || workflow.close());
}

fn is_terminal(event: &ExportProgress) -> bool {
    matches!(
        event,
        ExportProgress::Complete { .. } | ExportProgress::Failed(_) | ExportProgress::Cancelled
    )
}
