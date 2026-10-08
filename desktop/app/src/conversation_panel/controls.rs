//! Pure presentation rules of the agent panel: which controls exist in which state,
//! provider-owned setup guidance, reply correlation and focus/shortcut routing.
//!
//! Nothing here touches GPUI or the filesystem, so every rule is unit-testable.
use crate::agent_workflow::{
    AdapterReadiness, MAX_QUEUED_BRIEFS, PermissionCard, PermissionRef, PermissionState, TaskPhase,
    TaskView, UndoView, WorkflowSnapshot,
};
use std::collections::VecDeque;
use studio_agent_spike::AdapterStatus;
use studio_engine::{AgentTaskId, DraftState};

// ---- setup guidance --------------------------------------------------------------------------

/// Provider-owned setup guidance for the adapter's current readiness. The app does not
/// install adapters or sign anyone in: it states what is missing and where the user acts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guidance {
    pub headline: String,
    pub steps: Vec<String>,
    /// A task may start (the adapter initialized, or nothing says it cannot).
    pub usable: bool,
}

pub const NO_AGENT_LAUNCHED: &str =
    "Opening a project never installs or starts an agent; only Check adapter and tasks do.";

pub fn guidance(readiness: &AdapterReadiness, configured: bool) -> Guidance {
    if !configured {
        return Guidance {
            headline: "No agent adapter configured".into(),
            steps: vec![
                "Install an Agent Client Protocol (ACP v1) adapter by following its provider's instructions.".into(),
                "In Setup, enter its executable (an absolute path), arguments and the NAMES of the environment variables it reads for sign-in, then Save.".into(),
                NO_AGENT_LAUNCHED.into(),
            ],
            usable: false,
        };
    }
    match readiness {
        AdapterReadiness::NotConfigured => guidance(readiness, false),
        AdapterReadiness::Unchecked => Guidance {
            headline: "Configured · not checked yet".into(),
            steps: vec![
                "Check adapter starts it once to verify the executable, runtime, protocol version and sign-in.".into(),
                NO_AGENT_LAUNCHED.into(),
            ],
            usable: true,
        },
        AdapterReadiness::Checking => Guidance {
            headline: "Checking the adapter…".into(),
            steps: vec!["The adapter is starting in a scratch folder, not in your project.".into()],
            usable: true,
        },
        AdapterReadiness::Checked { report, .. } => match &report.status {
            AdapterStatus::Ready { agent, version } => Guidance {
                headline: format!("Ready · {agent} {version}"),
                steps: vec!["Sign-in was proven by creating a session.".into()],
                usable: true,
            },
            AdapterStatus::AuthUnknown { methods } => Guidance {
                headline: "Adapter starts · sign-in not verified".into(),
                steps: vec![
                    "The adapter speaks ACP v1 but sign-in is only proven by the first task.".into(),
                    auth_methods_line(methods),
                ],
                usable: true,
            },
            AdapterStatus::AuthRequired { methods } => Guidance {
                headline: "Sign-in required".into(),
                steps: vec![
                    "Sign in with the adapter provider's own tool, then set the environment variables the provider documents.".into(),
                    "Add those variable NAMES to auth_env_names in Setup and Save (values are read from the app's environment and never stored).".into(),
                    auth_methods_line(methods),
                ],
                usable: false,
            },
            AdapterStatus::AuthRejected { detail } => Guidance {
                headline: "Sign-in was rejected".into(),
                steps: vec![
                    format!("The provider refused the credentials: {detail}"),
                    "Refresh the credentials with the provider's own tool, then Check adapter again.".into(),
                ],
                usable: false,
            },
            AdapterStatus::MissingExecutable { searched } => Guidance {
                headline: "Adapter executable not found".into(),
                steps: vec![
                    "Install the adapter as its provider documents, then set its absolute path in Setup.".into(),
                    format!(
                        "Searched: {}",
                        if searched.is_empty() {
                            "(nothing: set an absolute path)".to_owned()
                        } else {
                            searched.join(", ")
                        }
                    ),
                ],
                usable: false,
            },
            AdapterStatus::MissingRuntime { detail } => Guidance {
                headline: "Adapter runtime missing".into(),
                steps: vec![
                    format!("The adapter exists but cannot run: {detail}"),
                    "Install the runtime the adapter's provider requires (for example Node.js or Python), then Check adapter again.".into(),
                ],
                usable: false,
            },
            AdapterStatus::ProtocolMismatch { detail } => Guidance {
                headline: "Not an ACP v1 adapter".into(),
                steps: vec![
                    format!("The program did not negotiate ACP v1: {detail}"),
                    "Use an adapter that implements the Agent Client Protocol version 1.".into(),
                ],
                usable: false,
            },
            AdapterStatus::Failed { failure } => Guidance {
                headline: "The adapter check failed".into(),
                steps: vec![
                    failure.message.clone(),
                    "Fix the reported problem, then Check adapter again.".into(),
                ],
                usable: false,
            },
        },
    }
}

fn auth_methods_line(methods: &[String]) -> String {
    if methods.is_empty() {
        "The adapter advertised no authentication methods.".into()
    } else {
        format!("Advertised sign-in methods: {}", methods.join(", "))
    }
}

// ---- controls --------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitKind {
    /// Start a task now.
    Brief,
    /// A task runs: the brief waits behind it (refreshed context when it starts).
    Queue,
    /// The agent asked a question: the text is the answer, sent into the same session.
    Clarify(AgentTaskId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitControl {
    pub kind: SubmitKind,
    /// Why the button is disabled (also shown as the hint under the prompt).
    pub blocked: Option<String>,
}

impl SubmitControl {
    pub fn label(&self) -> &'static str {
        match self.kind {
            SubmitKind::Brief => "Send",
            SubmitKind::Queue => "Queue",
            SubmitKind::Clarify(_) => "Answer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoControl {
    Enabled { target: String, summary: String },
    Disabled(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchChoiceControl {
    pub target_provider: String,
    pub draft_revision: Option<String>,
    pub source_revision: String,
    pub accepted_revision: String,
    pub retained_queue_count: usize,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRestoreControl {
    pub provider_id: String,
    pub redacted_session_id: String,
    pub is_resumable: bool,
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedQueueControl {
    pub count: usize,
    pub summaries: Vec<String>,
}

/// Which controls exist and are enabled for one snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controls {
    pub submit: SubmitControl,
    /// Stop is offered in every non-terminal state (including queued briefs and Undo);
    /// the note explains what it does in this state.
    pub stop: Option<String>,
    pub review: Option<ReviewControls>,
    pub export_draft: bool,
    pub undo: UndoControl,
    pub settings_editable: bool,
    pub check_adapter: bool,
    pub acknowledge_writer_gone: bool,
    pub modes: bool,
    pub config_options: bool,
    /// Controls the adapter did not advertise are not shown; this explains why when a
    /// session is live.
    pub options_note: Option<String>,
    pub switch_choice: Option<SwitchChoiceControl>,
    pub session_restore: Option<SessionRestoreControl>,
    pub retained_queue: Option<RetainedQueueControl>,
    pub can_switch_provider: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewControls {
    /// Apply can be retried even while blocked; the reason is shown next to it.
    pub apply_note: Option<String>,
}

fn live(task: &TaskView) -> bool {
    !task.phase.is_terminal()
}

/// Something that excludes a new task: editing is suspended or a writer may still run.
fn blocked_reason(snapshot: &WorkflowSnapshot, sdk_ready: bool) -> Option<String> {
    if snapshot.switch_pending.is_some() {
        return Some("A provider switch is pending. Choose continuation or cancel before sending a new brief.".into());
    }
    if matches!(snapshot.adapter.readiness, AdapterReadiness::NotConfigured)
        && snapshot.adapter.provider.is_none()
    {
        return Some("Configure an agent adapter in Setup first.".into());
    }
    if !sdk_ready {
        return Some(
            "No compatible SDK is selected. SDK readiness is separate from agent readiness: install or select an SDK first."
                .into(),
        );
    }
    if let Some(reason) = &snapshot.recovery.suspended {
        return Some(format!("Source changes are suspended: {reason}"));
    }
    if let Some(draft) = &snapshot.recovery.draft
        && matches!(draft.state, DraftState::UnsafeWriter { .. })
    {
        return Some(
            "A previous agent may still be writing to its working copy. Confirm in Project that nothing is writing before starting another task."
                .into(),
        );
    }
    if matches!(snapshot.adapter.readiness, AdapterReadiness::Checked { .. }) {
        let g = guidance(&snapshot.adapter.readiness, true);
        if !g.usable {
            return Some(match g.steps.first() {
                Some(step) => format!("{} — {step}", g.headline),
                None => g.headline,
            });
        }
    }
    None
}

/// Derives every control from the snapshot, the prompt contents and the host's SDK state.
pub fn derive_controls(
    snapshot: &WorkflowSnapshot,
    input_empty: bool,
    sdk_ready: bool,
) -> Controls {
    let task = snapshot.task.as_ref();
    let active_task = task.filter(|t| live(t));
    let undo_running = matches!(snapshot.undo, UndoView::InProgress { .. });
    let busy = active_task.is_some() || undo_running;

    let mut submit = SubmitControl {
        kind: SubmitKind::Brief,
        blocked: None,
    };
    if let Some(t) = active_task {
        submit.kind = if t.phase == TaskPhase::WaitingClarification {
            SubmitKind::Clarify(t.id.clone())
        } else {
            SubmitKind::Queue
        };
    } else if undo_running || !snapshot.queue.is_empty() {
        submit.kind = SubmitKind::Queue;
    }
    submit.blocked = if submit.kind != SubmitKind::Brief && snapshot.recovery.suspended.is_none() {
        // A running or queued task already passed the start checks; only the queue bound
        // and emptiness matter now.
        None
    } else {
        blocked_reason(snapshot, sdk_ready)
    };
    if submit.blocked.is_none() {
        if matches!(submit.kind, SubmitKind::Queue) && snapshot.queue.len() >= MAX_QUEUED_BRIEFS {
            submit.blocked = Some(format!("{MAX_QUEUED_BRIEFS} briefs are already queued."));
        } else if input_empty {
            submit.blocked = Some(match submit.kind {
                SubmitKind::Clarify(_) => "Type your answer.".into(),
                SubmitKind::Queue => "Type a follow-up to queue.".into(),
                SubmitKind::Brief => "Describe the change you want.".into(),
            });
        }
    }

    let stop = if let Some(t) = active_task {
        Some(stop_note(t.phase))
    } else if undo_running {
        Some("Stops the Undo before it publishes; a publication already under way finishes and stays recoverable.".into())
    } else if !snapshot.queue.is_empty() {
        Some("Clears the queued briefs.".into())
    } else {
        None
    };

    let review = active_task
        .filter(|t| t.phase == TaskPhase::AwaitingReview)
        .and_then(|t| t.review.as_ref())
        .map(|review| ReviewControls {
            apply_note: review.apply_blocked.clone(),
        });

    // A retained working copy can be exported once no task owns it: the ended task's own
    // view, or the project's recovery state after a restart.
    let retained = |state: &DraftState| matches!(state, DraftState::Retained { .. });
    let export_draft = active_task.is_none()
        && (task.is_some_and(|t| t.draft_state.as_ref().is_some_and(retained))
            || snapshot
                .recovery
                .draft
                .as_ref()
                .is_some_and(|d| retained(&d.state)));

    let undo = match &snapshot.undo {
        UndoView::Available { target, summary } => {
            if busy {
                UndoControl::Disabled("Wait for the running task to finish.".into())
            } else if let Some(reason) = &snapshot.recovery.suspended {
                UndoControl::Disabled(format!("Source changes are suspended: {reason}"))
            } else {
                UndoControl::Enabled {
                    target: target.clone(),
                    summary: summary.clone(),
                }
            }
        }
        UndoView::Unavailable { reason } => UndoControl::Disabled(reason.clone()),
        UndoView::InProgress { phase } => {
            UndoControl::Disabled(format!("Undo in progress: {phase}"))
        }
    };

    let modes = !snapshot.options.modes.is_empty();
    let config_options = !snapshot.options.config.is_empty();
    let options_note = (active_task.is_some() && !modes && !config_options).then(|| {
        "This agent did not advertise model or mode options, so none are offered.".to_owned()
    });
    let switch_choice = snapshot.switch_pending.as_ref().map(|p| SwitchChoiceControl {
        target_provider: p.target_provider.clone(),
        draft_revision: p.draft_revision.clone(),
        source_revision: p.source_revision.clone(),
        accepted_revision: p.accepted_revision.clone(),
        retained_queue_count: p.retained_queue_count,
        note: format!(
            "Provider switch ready for {}. Choose whether to continue draft or restart from accepted.",
            p.target_provider
        ),
    });

    let session_restore = snapshot
        .session_restore
        .as_ref()
        .map(|r| SessionRestoreControl {
            provider_id: r.provider_id.clone(),
            redacted_session_id: r.redacted_session_id.clone(),
            is_resumable: r.is_resumable,
            notice: r.notice.clone(),
        });

    let retained_queue = if !snapshot.retained_queue.is_empty() {
        Some(RetainedQueueControl {
            count: snapshot.retained_queue.len(),
            summaries: snapshot
                .retained_queue
                .iter()
                .map(|q| q.summary.clone())
                .collect(),
        })
    } else {
        None
    };

    let can_switch_provider = !busy && snapshot.switch_pending.is_none();

    Controls {
        submit,
        stop,
        review,
        export_draft,
        undo,
        settings_editable: !busy && snapshot.queue.is_empty(),
        check_adapter: !busy
            && snapshot.adapter.provider.is_some()
            && !matches!(snapshot.adapter.readiness, AdapterReadiness::Checking),
        acknowledge_writer_gone: snapshot
            .recovery
            .draft
            .as_ref()
            .is_some_and(|d| d.can_acknowledge),
        modes,
        config_options,
        options_note,
        switch_choice,
        session_restore,
        retained_queue,
        can_switch_provider,
    }
}

/// What Stop does in each phase (shown beside the button).
pub fn stop_note(phase: TaskPhase) -> String {
    match phase {
        TaskPhase::Queued => "Removes the waiting task and clears the queue.",
        TaskPhase::Starting | TaskPhase::Editing | TaskPhase::WaitingPermission => {
            "Cancels the agent, closes open requests and keeps its working copy."
        }
        TaskPhase::WaitingClarification => "Ends the task without an answer and keeps the working copy.",
        TaskPhase::Quiescing | TaskPhase::Capturing | TaskPhase::Validating | TaskPhase::Repairing => {
            "Cancels validation or repair; the working copy and candidate are kept."
        }
        TaskPhase::AwaitingEvidence => {
            "Cancels the candidate evidence render before automatic Apply."
        }
        TaskPhase::AwaitingReview => {
            "Stop does not discard the candidate. Use Discard, or Apply to publish it."
        }
        TaskPhase::Promoting => {
            "Applying has a short commit that cannot be cancelled. Stop reports it; the result stays recoverable with Undo."
        }
        _ => "",
    }
    .to_owned()
}

// ---- task banner -----------------------------------------------------------------------------

/// The frozen identities of the running task, as short labelled lines.
pub fn identity_lines(task: &TaskView) -> Vec<(&'static str, String)> {
    let id = task.id.0.as_str();
    vec![
        (
            "Task",
            format!(
                "{} · generation {}",
                &id[..id.len().min(8)],
                task.generation
            ),
        ),
        ("Base", task.source_base.clone()),
        ("Working copy", task.draft.display().to_string()),
        (
            "Review",
            match task.review_policy {
                studio_engine::ReviewPolicy::AutoApply => "Apply automatically after validation",
                studio_engine::ReviewPolicy::ManualReview => "Manual review before Apply",
            }
            .to_owned(),
        ),
        (
            "Repair",
            format!(
                "{} of {} automatic repair{}{}",
                task.repair.used,
                task.repair.max,
                if task.repair.max == 1 { "" } else { "s" },
                if task.repair.in_progress {
                    " · in progress"
                } else {
                    ""
                }
            ),
        ),
    ]
}

// ---- reply correlation -----------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReplyRefusal {
    #[error("that request is no longer open")]
    NotOpen,
    #[error("that request already has an answer on its way")]
    AlreadySent,
}

/// UI-side half of reply correlation. The workflow rejects stale and duplicate replies
/// itself; this tracker stops them before they are even sent, so a double click between
/// two snapshots is one command and a click on a card that closed is a visible refusal.
#[derive(Debug, Default)]
pub struct ReplyTracker {
    sent: VecDeque<PermissionRef>,
}

const TRACKED_REPLIES: usize = 64;

impl ReplyTracker {
    /// Claims the right to answer `target`. `open` is the set of requests the CURRENT
    /// snapshot still lists as open.
    pub fn begin(
        &mut self,
        target: &PermissionRef,
        open: &[&PermissionCard],
    ) -> Result<(), ReplyRefusal> {
        if self.sent.contains(target) {
            return Err(ReplyRefusal::AlreadySent);
        }
        if !open
            .iter()
            .any(|card| &card.reference == target && card.state == PermissionState::Open)
        {
            return Err(ReplyRefusal::NotOpen);
        }
        self.sent.push_back(target.clone());
        while self.sent.len() > TRACKED_REPLIES {
            self.sent.pop_front();
        }
        Ok(())
    }

    /// The answer is on its way (or already answered): the card must not offer choices.
    pub fn is_pending(&self, target: &PermissionRef) -> bool {
        self.sent.contains(target)
    }
}

// ---- settings saves ----------------------------------------------------------------------------

/// At most one adapter-settings save is in flight (resolving the qualification, applying it
/// to the workflow, writing the file). A second Save while one is pending is refused and
/// only the completion of the in-flight save is ever reported, so overlapping saves can
/// neither interleave their files nor mask each other's failure.
#[derive(Debug, Default)]
pub struct SaveTracker {
    pending: Option<u64>,
    last: u64,
}

impl SaveTracker {
    /// Claims the save slot; the returned ticket must be passed to [`Self::finish`].
    pub fn begin(&mut self) -> Result<u64, &'static str> {
        if self.pending.is_some() {
            return Err("The previous Save is still being applied; wait for it to finish.");
        }
        self.last += 1;
        self.pending = Some(self.last);
        Ok(self.last)
    }

    /// Whether `ticket` is still the in-flight save (not cancelled by a detach).
    pub fn is_current(&self, ticket: u64) -> bool {
        self.pending == Some(ticket)
    }

    /// Completes `ticket`; `false` when it is stale (cancelled), in which case nothing may
    /// be reported for it.
    pub fn finish(&mut self, ticket: u64) -> bool {
        if self.pending == Some(ticket) {
            self.pending = None;
            true
        } else {
            false
        }
    }

    /// Abandons the in-flight save (the workflow it belonged to is gone).
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

// ---- focus and shortcuts ---------------------------------------------------------------------

/// Modifier state the routing needs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyMods {
    pub shift: bool,
    pub alt: bool,
    pub secondary: bool,
}

/// What a key pressed while the prompt (or another text input) has focus does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKey {
    /// Submit the prompt.
    Submit,
    /// Hand focus to the next / previous tab stop.
    FocusNext,
    FocusPrev,
    /// Leave the input (back to the shell).
    Leave,
    /// The key is text or editing: the input handler (typing, IME, key bindings) owns it
    /// and nothing above the input may act on it. Space, arrows, Home/End and letters
    /// land here, so the timeline's transport shortcuts can never fire from a prompt.
    Text,
    /// The key belongs to an IME composition in progress; neither the form nor the shell
    /// may act on it.
    Composition,
}

pub fn route_input_key(key: &str, mods: KeyMods, composing: bool) -> InputKey {
    if composing && matches!(key, "enter" | "tab" | "escape") {
        return InputKey::Composition;
    }
    match key {
        "enter" if !mods.shift && !mods.alt && !mods.secondary => InputKey::Submit,
        "tab" if mods.shift => InputKey::FocusPrev,
        "tab" => InputKey::FocusNext,
        "escape" => InputKey::Leave,
        _ => InputKey::Text,
    }
}

/// The transport shortcuts of the preview surface. They exist only while the preview
/// surface (or the timeline) is the focused element; a text input never forwards them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    TogglePlayback,
    StepBack,
    StepForward,
    ToStart,
    ToEnd,
}

pub fn transport_for_key(key: &str) -> Option<Transport> {
    Some(match key {
        "space" => Transport::TogglePlayback,
        "left" => Transport::StepBack,
        "right" => Transport::StepForward,
        "home" => Transport::ToStart,
        "end" => Transport::ToEnd,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_workflow::{
        AdapterView, ChangeCard, McpView, RecoveryView, RepairView, ResourceView, ReviewView,
    };
    use std::{path::PathBuf, sync::Arc};
    use studio_agent_spike::{DiscoveryReport, driver::PermissionChoice};
    use studio_engine::{ReviewPolicy, TaskScope, TaskState};
    use studio_project::ProjectId;

    fn base_snapshot() -> WorkflowSnapshot {
        WorkflowSnapshot {
            revision: 1,
            project: ProjectId::try_from("p1".to_owned()).unwrap(),
            adapter: AdapterView {
                provider: Some("fixture".into()),
                executable: Some("/opt/adapter".into()),
                readiness: AdapterReadiness::Unchecked,
                writer_ownership: Some("unknown".into()),
            },
            capabilities: None,
            options: Default::default(),
            mcp: McpView {
                policy_enabled: true,
                binary_available: true,
                active: false,
                cli_active: false,
                capability_file: None,
                note: None,
            },
            task: None,
            queue: Vec::new(),
            rows: Vec::new(),
            older_rows: false,
            review_policy: ReviewPolicy::AutoApply,
            undo: UndoView::Unavailable {
                reason: "No accepted agent edit yet.".into(),
            },
            recovery: RecoveryView::default(),
            handoff: None,
            history: Vec::new(),
            resources: ResourceView::default(),
            switch_pending: None,
            session_restore: None,
            retained_queue: Vec::new(),
            closed: false,
        }
    }

    fn task(phase: TaskPhase) -> TaskView {
        TaskView {
            id: AgentTaskId("0123456789abcdef".into()),
            generation: 3,
            phase,
            engine_state: TaskState::Editing,
            brief: "make it blue".into(),
            source_base: "abcdef012345".into(),
            scope: TaskScope::whole_project("p1", "a".repeat(64)),
            before: None,
            after: None,
            image_limitation: None,
            draft: PathBuf::from("/data/projects/p/agent/draft"),
            review_policy: ReviewPolicy::AutoApply,
            repair: RepairView {
                used: 0,
                max: 1,
                in_progress: false,
                context_summary: None,
            },
            turns: 1,
            writer: None,
            changes: None::<ChangeCard>,
            validation: None,
            review: None,
            conflict: None,
            error: None,
            stop_requested: false,
            reason: None,
            started_unix: 0,
            ended_unix: None,
            draft_state: None,
        }
    }

    fn with_task(phase: TaskPhase) -> WorkflowSnapshot {
        let mut snapshot = base_snapshot();
        snapshot.task = Some(task(phase));
        snapshot
    }

    #[test]
    fn without_an_adapter_nothing_can_be_sent_and_the_reason_names_setup() {
        let mut snapshot = base_snapshot();
        snapshot.adapter.provider = None;
        snapshot.adapter.executable = None;
        snapshot.adapter.readiness = AdapterReadiness::NotConfigured;
        let controls = derive_controls(&snapshot, false, true);
        assert!(
            controls
                .submit
                .blocked
                .as_deref()
                .unwrap()
                .contains("Setup")
        );
        assert!(!controls.check_adapter);
        assert!(controls.stop.is_none());
    }

    #[test]
    fn sdk_readiness_is_separate_from_agent_readiness() {
        let snapshot = base_snapshot();
        let controls = derive_controls(&snapshot, false, false);
        let reason = controls.submit.blocked.unwrap();
        assert!(reason.contains("SDK"), "{reason}");
        assert!(
            derive_controls(&snapshot, false, true)
                .submit
                .blocked
                .is_none()
        );
    }

    #[test]
    fn an_empty_prompt_is_a_blocked_send_with_a_hint() {
        let controls = derive_controls(&base_snapshot(), true, true);
        assert_eq!(controls.submit.kind, SubmitKind::Brief);
        assert!(controls.submit.blocked.is_some());
    }

    #[test]
    fn a_running_task_turns_send_into_queue_and_stop_is_always_offered() {
        for phase in [
            TaskPhase::Queued,
            TaskPhase::Starting,
            TaskPhase::Editing,
            TaskPhase::WaitingPermission,
            TaskPhase::Quiescing,
            TaskPhase::Capturing,
            TaskPhase::Validating,
            TaskPhase::Repairing,
            TaskPhase::AwaitingReview,
            TaskPhase::AwaitingEvidence,
            TaskPhase::Promoting,
        ] {
            let controls = derive_controls(&with_task(phase), false, true);
            assert_eq!(controls.submit.kind, SubmitKind::Queue, "{phase:?}");
            assert!(controls.submit.blocked.is_none(), "{phase:?}");
            assert!(
                controls.stop.as_deref().is_some_and(|n| !n.is_empty()),
                "Stop is offered (and explained) in {phase:?}"
            );
            assert!(!controls.settings_editable, "{phase:?}");
        }
        for phase in [
            TaskPhase::Accepted,
            TaskPhase::Conflict,
            TaskPhase::Failed,
            TaskPhase::Cancelled,
            TaskPhase::Interrupted,
        ] {
            let controls = derive_controls(&with_task(phase), false, true);
            assert_eq!(controls.submit.kind, SubmitKind::Brief, "{phase:?}");
            assert!(controls.stop.is_none(), "{phase:?}");
            assert!(controls.settings_editable, "{phase:?}");
        }
    }

    #[test]
    fn the_promotion_stop_note_states_the_uncancellable_commit() {
        let note = stop_note(TaskPhase::Promoting);
        assert!(note.contains("cannot be cancelled"));
        assert!(stop_note(TaskPhase::AwaitingReview).contains("Discard"));
    }

    #[test]
    fn a_clarification_turns_the_prompt_into_an_answer_for_exactly_that_task() {
        let controls = derive_controls(&with_task(TaskPhase::WaitingClarification), false, true);
        assert_eq!(
            controls.submit.kind,
            SubmitKind::Clarify(AgentTaskId("0123456789abcdef".into()))
        );
        assert_eq!(controls.submit.label(), "Answer");
    }

    #[test]
    fn a_full_queue_blocks_further_briefs() {
        let mut snapshot = with_task(TaskPhase::Editing);
        snapshot.queue = (0..MAX_QUEUED_BRIEFS as u64)
            .map(|id| crate::agent_workflow::QueuedBrief {
                id,
                summary: "x".into(),
                queued_unix: 0,
                scope: None,
                stale_scope: false,
            })
            .collect();
        let controls = derive_controls(&snapshot, false, true);
        assert!(controls.submit.blocked.unwrap().contains("queued"));
    }

    #[test]
    fn review_controls_exist_only_while_a_candidate_awaits_review() {
        let mut snapshot = with_task(TaskPhase::AwaitingReview);
        snapshot.task.as_mut().unwrap().review = Some(ReviewView {
            candidate: "ab".into(),
            apply_blocked: Some("publication gate blocked".into()),
        });
        let controls = derive_controls(&snapshot, false, true);
        assert_eq!(
            controls.review,
            Some(ReviewControls {
                apply_note: Some("publication gate blocked".into())
            })
        );
        assert!(
            derive_controls(&with_task(TaskPhase::Editing), false, true)
                .review
                .is_none()
        );
    }

    #[test]
    fn export_draft_is_offered_only_for_a_retained_draft_of_an_ended_task() {
        let mut snapshot = with_task(TaskPhase::Failed);
        snapshot.task.as_mut().unwrap().draft_state = Some(DraftState::Retained {
            task: "t".into(),
            reason: "failed".into(),
        });
        assert!(derive_controls(&snapshot, false, true).export_draft);
        snapshot.task.as_mut().unwrap().draft_state =
            Some(DraftState::Accepted { task: "t".into() });
        assert!(!derive_controls(&snapshot, false, true).export_draft);
        // After a restart there is no task view, only the recovery state.
        let mut restarted = base_snapshot();
        restarted.recovery.draft = Some(crate::agent_workflow::DraftNotice {
            state: DraftState::Retained {
                task: "t".into(),
                reason: "stopped".into(),
            },
            interrupted_by_restart: false,
            can_acknowledge: false,
            draft: PathBuf::from("/d"),
        });
        assert!(derive_controls(&restarted, false, true).export_draft);
        let mut running = with_task(TaskPhase::Editing);
        running.task.as_mut().unwrap().draft_state = Some(DraftState::Retained {
            task: "t".into(),
            reason: "x".into(),
        });
        assert!(!derive_controls(&running, false, true).export_draft);
    }

    #[test]
    fn undo_is_available_only_when_idle_and_explains_otherwise() {
        let mut snapshot = base_snapshot();
        snapshot.undo = UndoView::Available {
            target: "tx1".into(),
            summary: "Changed 2 files".into(),
        };
        assert!(matches!(
            derive_controls(&snapshot, false, true).undo,
            UndoControl::Enabled { .. }
        ));
        snapshot.task = Some(task(TaskPhase::Editing));
        assert!(matches!(
            derive_controls(&snapshot, false, true).undo,
            UndoControl::Disabled(_)
        ));
        snapshot.task = None;
        snapshot.recovery.suspended = Some("journal write failed".into());
        let UndoControl::Disabled(reason) = derive_controls(&snapshot, false, true).undo else {
            panic!("suspended source disables Undo")
        };
        assert!(reason.contains("journal write failed"));
        snapshot.recovery.suspended = None;
        snapshot.undo = UndoView::InProgress {
            phase: "validating".into(),
        };
        let controls = derive_controls(&snapshot, false, true);
        assert!(matches!(controls.undo, UndoControl::Disabled(_)));
        assert!(controls.stop.is_some(), "Undo can be stopped");
    }

    #[test]
    fn unadvertised_options_are_hidden_and_a_live_session_explains_why() {
        let mut snapshot = with_task(TaskPhase::Editing);
        let controls = derive_controls(&snapshot, false, true);
        assert!(!controls.modes && !controls.config_options);
        assert!(controls.options_note.is_some());
        snapshot
            .options
            .modes
            .push(studio_agent_spike::driver::ModeOption {
                id: "plan".into(),
                name: "Plan".into(),
                description: None,
            });
        let controls = derive_controls(&snapshot, false, true);
        assert!(controls.modes && !controls.config_options);
        assert!(controls.options_note.is_none());
        assert!(
            derive_controls(&base_snapshot(), false, true)
                .options_note
                .is_none()
        );
    }

    #[test]
    fn an_unsafe_working_copy_blocks_new_tasks_until_acknowledged() {
        let mut snapshot = base_snapshot();
        snapshot.recovery.draft = Some(crate::agent_workflow::DraftNotice {
            state: DraftState::UnsafeWriter {
                task: "t".into(),
                reason: "previous session ended".into(),
            },
            interrupted_by_restart: true,
            can_acknowledge: true,
            draft: PathBuf::from("/d"),
        });
        let controls = derive_controls(&snapshot, false, true);
        assert!(
            controls
                .submit
                .blocked
                .unwrap()
                .contains("still be writing")
        );
        assert!(controls.acknowledge_writer_gone);
    }

    fn checked(status: AdapterStatus) -> AdapterReadiness {
        AdapterReadiness::Checked {
            report: Box::new(DiscoveryReport {
                status,
                executable: None,
                initialized: None,
            }),
            at_unix: 1,
        }
    }

    #[test]
    fn every_adapter_failure_has_provider_owned_guidance_and_blocks_tasks() {
        let cases = [
            (
                AdapterStatus::MissingExecutable {
                    searched: vec!["/opt/a".into()],
                },
                "not found",
                "/opt/a",
            ),
            (
                AdapterStatus::MissingRuntime {
                    detail: "node: not found".into(),
                },
                "runtime",
                "node: not found",
            ),
            (
                AdapterStatus::ProtocolMismatch {
                    detail: "v2".into(),
                },
                "ACP v1",
                "v2",
            ),
            (
                AdapterStatus::AuthRequired {
                    methods: vec!["oauth".into()],
                },
                "Sign-in required",
                "oauth",
            ),
            (
                AdapterStatus::AuthRejected {
                    detail: "expired".into(),
                },
                "rejected",
                "expired",
            ),
        ];
        for (status, headline, step) in cases {
            let readiness = checked(status);
            let g = guidance(&readiness, true);
            assert!(!g.usable);
            assert!(g.headline.contains(headline), "{}", g.headline);
            assert!(g.steps.join(" ").contains(step), "{:?}", g.steps);
            let mut snapshot = base_snapshot();
            snapshot.adapter.readiness = readiness;
            assert!(
                derive_controls(&snapshot, false, true)
                    .submit
                    .blocked
                    .is_some(),
                "{headline} blocks Send"
            );
        }
    }

    #[test]
    fn usable_statuses_do_not_block_tasks_and_unchecked_launches_nothing() {
        let unchecked = guidance(&AdapterReadiness::Unchecked, true);
        assert!(unchecked.usable);
        assert!(
            unchecked
                .steps
                .join(" ")
                .contains("never installs or starts")
        );
        let unknown = checked(AdapterStatus::AuthUnknown { methods: vec![] });
        assert!(guidance(&unknown, true).usable);
        let mut snapshot = base_snapshot();
        snapshot.adapter.readiness = unknown;
        assert!(
            derive_controls(&snapshot, false, true)
                .submit
                .blocked
                .is_none()
        );
        assert!(!guidance(&AdapterReadiness::NotConfigured, false).usable);
    }

    fn card(request: u64, state: PermissionState) -> PermissionCard {
        PermissionCard {
            reference: PermissionRef {
                task: AgentTaskId("t1".into()),
                writer: 1,
                request,
            },
            turn: 1,
            tool_call_id: "c".into(),
            title: "Edit src/lib.rs".into(),
            options: vec![PermissionChoice {
                option_id: "allow".into(),
                name: "Allow".into(),
                kind: "allow_once".into(),
            }],
            state,
        }
    }

    #[test]
    fn a_double_click_between_snapshots_sends_one_reply() {
        let open = card(1, PermissionState::Open);
        let mut tracker = ReplyTracker::default();
        assert_eq!(tracker.begin(&open.reference, &[&open]), Ok(()));
        assert!(tracker.is_pending(&open.reference));
        assert_eq!(
            tracker.begin(&open.reference, &[&open]),
            Err(ReplyRefusal::AlreadySent)
        );
    }

    #[test]
    fn a_click_on_a_card_the_current_snapshot_closed_is_refused() {
        let closed = card(2, PermissionState::Cancelled);
        let mut tracker = ReplyTracker::default();
        assert_eq!(
            tracker.begin(&closed.reference, &[&closed]),
            Err(ReplyRefusal::NotOpen)
        );
        assert_eq!(
            tracker.begin(&closed.reference, &[]),
            Err(ReplyRefusal::NotOpen)
        );
        assert!(
            !tracker.is_pending(&closed.reference),
            "a refusal claims nothing"
        );
    }

    #[test]
    fn a_reply_for_another_writer_epoch_or_task_is_stale() {
        let open = card(3, PermissionState::Open);
        let mut foreign = open.reference.clone();
        foreign.writer = 2;
        let mut other_task = open.reference.clone();
        other_task.task = AgentTaskId("t2".into());
        let mut tracker = ReplyTracker::default();
        assert_eq!(
            tracker.begin(&foreign, &[&open]),
            Err(ReplyRefusal::NotOpen)
        );
        assert_eq!(
            tracker.begin(&other_task, &[&open]),
            Err(ReplyRefusal::NotOpen)
        );
        assert_eq!(tracker.begin(&open.reference, &[&open]), Ok(()));
    }

    #[test]
    fn the_tracker_is_bounded() {
        let mut tracker = ReplyTracker::default();
        for request in 0..(TRACKED_REPLIES as u64 + 20) {
            let open = card(request, PermissionState::Open);
            tracker.begin(&open.reference, &[&open]).unwrap();
        }
        assert_eq!(tracker.sent.len(), TRACKED_REPLIES);
        assert!(!tracker.is_pending(&card(0, PermissionState::Open).reference));
    }

    #[test]
    fn space_and_navigation_keys_are_text_in_an_input_never_transport_commands() {
        for key in [
            "space",
            "left",
            "right",
            "home",
            "end",
            "m",
            "a",
            "backspace",
        ] {
            assert_eq!(
                route_input_key(key, KeyMods::default(), false),
                InputKey::Text,
                "{key} belongs to the focused input"
            );
        }
        // The same keys are transport commands only on the preview surface.
        assert_eq!(transport_for_key("space"), Some(Transport::TogglePlayback));
        assert_eq!(transport_for_key("end"), Some(Transport::ToEnd));
        assert_eq!(transport_for_key("m"), None);
    }

    #[test]
    fn enter_submits_but_never_during_a_composition_or_with_modifiers() {
        assert_eq!(
            route_input_key("enter", KeyMods::default(), false),
            InputKey::Submit
        );
        assert_eq!(
            route_input_key("enter", KeyMods::default(), true),
            InputKey::Composition
        );
        let shift = KeyMods {
            shift: true,
            ..Default::default()
        };
        assert_eq!(route_input_key("enter", shift, false), InputKey::Text);
    }

    #[test]
    fn tab_order_moves_focus_and_is_kept_by_a_composition() {
        assert_eq!(
            route_input_key("tab", KeyMods::default(), false),
            InputKey::FocusNext
        );
        let shift = KeyMods {
            shift: true,
            ..Default::default()
        };
        assert_eq!(route_input_key("tab", shift, false), InputKey::FocusPrev);
        assert_eq!(
            route_input_key("tab", KeyMods::default(), true),
            InputKey::Composition
        );
        assert_eq!(
            route_input_key("escape", KeyMods::default(), false),
            InputKey::Leave
        );
        assert_eq!(
            route_input_key("escape", KeyMods::default(), true),
            InputKey::Composition
        );
    }

    #[test]
    fn identity_lines_name_the_frozen_task_base_draft_policy_and_repair_count() {
        let mut view = task(TaskPhase::Repairing);
        view.repair.used = 1;
        view.repair.in_progress = true;
        view.review_policy = ReviewPolicy::ManualReview;
        let lines = identity_lines(&view);
        let text = lines
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Task: 01234567 · generation 3"), "{text}");
        assert!(text.contains("Base: abcdef012345"));
        assert!(text.contains("/data/projects/p/agent/draft"));
        assert!(text.contains("Manual review before Apply"));
        assert!(text.contains("1 of 1 automatic repair · in progress"));
    }

    #[test]
    fn only_one_settings_save_is_in_flight_and_only_its_completion_counts() {
        let mut saves = SaveTracker::default();
        let first = saves.begin().unwrap();
        assert!(saves.is_pending() && saves.is_current(first));
        // A second Save (differently sized JSON, or a clear) is refused while one runs.
        assert!(saves.begin().unwrap_err().contains("still being applied"));
        assert!(saves.is_current(first), "the refused click changed nothing");
        assert!(saves.finish(first));
        assert!(!saves.finish(first), "a completion is reported once");
        let second = saves.begin().unwrap();
        assert_ne!(first, second);
        // A save abandoned by a detach is stale: its late completion reports nothing, and
        // it does not mask the next save.
        saves.cancel();
        assert!(!saves.is_current(second) && !saves.finish(second));
        let third = saves.begin().unwrap();
        assert!(
            !saves.finish(second),
            "the stale completion cannot finish the new save"
        );
        assert!(saves.finish(third));
    }

    #[test]
    fn a_setup_field_in_composition_keeps_enter_tab_and_escape() {
        // Every field (prompt and Setup alike) routes through one path that passes THAT
        // field's composition state: while composing, nothing submits, moves focus or leaves.
        let plain = KeyMods::default();
        for key in ["enter", "tab", "escape"] {
            assert_eq!(
                route_input_key(key, plain, true),
                InputKey::Composition,
                "{key}"
            );
        }
        // And the same keys act normally once the composition ends.
        assert_eq!(route_input_key("tab", plain, false), InputKey::FocusNext);
        assert_eq!(route_input_key("escape", plain, false), InputKey::Leave);
        assert_eq!(route_input_key("enter", plain, false), InputKey::Submit);
    }

    #[test]
    fn nothing_arc_shared_is_needed_to_derive_controls() {
        // Controls are a pure function of the immutable snapshot.
        let snapshot = Arc::new(base_snapshot());
        assert_eq!(
            derive_controls(&snapshot, false, true),
            derive_controls(&snapshot, false, true)
        );
    }
}
