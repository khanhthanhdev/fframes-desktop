//! The native agent panel: setup and readiness, a virtualized conversation, review,
//! Undo and recovery. It renders immutable [`WorkflowSnapshot`]s and sends commands; it
//! never touches the filesystem, the adapter, hashing or compilation (the workflow's
//! threads do), and it never holds a lock across a frame.
//!
//! * Rows live in a `gpui::list` (visible rows plus [`rows::OVERDRAW_PX`] of overscan are
//!   measured and painted). [`rows`] decides which rows the list owns and how a new
//!   snapshot becomes list operations, so streaming text and tool-card updates re-measure
//!   in place while the reader's position holds.
//! * [`controls`] decides which controls exist in which state (pure, unit-tested).
//! * [`host`] holds the app-local adapter description and the workflow inbox.
pub mod controls;
pub mod host;
pub mod qualification;
pub mod rows;

/// Redacted observation of an evidence set: state, revision and frame labels only.
fn evidence_telemetry(view: &crate::agent_workflow::EvidenceView) -> serde_json::Value {
    serde_json::json!({
        "state": format!("{:?}", view.state),
        "revision": view.revision,
        "artifacts": view.artifacts.iter().map(|a| serde_json::json!({
            "label": a.label, "frames": a.frames, "bytes": a.bytes, "sha256": a.sha256[..12.min(a.sha256.len())],
        })).collect::<Vec<_>>(),
    })
}

/// One line per evidence set: its state, the immutable revision it was rendered from and
/// the actual frames rendered. Never predicted pixels.
fn evidence_summary(name: &str, view: &crate::agent_workflow::EvidenceView) -> String {
    use crate::agent_workflow::EvidenceState;
    let state = match view.state {
        EvidenceState::Pending => "rendering…".to_owned(),
        EvidenceState::Ready => {
            let frames: Vec<String> = view
                .artifacts
                .iter()
                .map(|a| format!("{} {:?}", a.label, a.frames))
                .collect();
            format!("{} · rendered {}", view.artifacts.len(), frames.join("; "))
        }
        EvidenceState::Unavailable => format!(
            "unavailable: {}",
            view.note.as_deref().unwrap_or("not rendered")
        ),
        EvidenceState::Released if !view.artifacts.is_empty() => {
            let frames: Vec<String> = view
                .artifacts
                .iter()
                .map(|a| format!("{} {:?}", a.label, a.frames))
                .collect();
            format!(
                "{} · rendered {} · artifact files released with task",
                view.artifacts.len(),
                frames.join("; ")
            )
        }
        EvidenceState::Released => "released with its task".to_owned(),
    };
    format!("{name} evidence (revision {}): {state}", view.revision)
}

fn evidence_image_key(task: &studio_engine::AgentTaskId, artifact_id: &str) -> String {
    format!("{}:{artifact_id}", task.0)
}

use crate::{
    agent_workflow::{
        AdapterReadiness, AgentWorkflow, ChangeCard, ConflictView, HandoffState, HistoryEntry,
        NoticeLevel, OutcomeKind, PermissionAnswer, PermissionCard, PermissionRef, PermissionState,
        Row, RowKind, StructuredError, TaskPhase, ToolCard, UndoView, UserSource, ValidationCard,
        WorkflowSnapshot,
    },
    studio_shell::{ACCENT, BORDER, MUTED, PANEL, TEXT},
    text_input::TextInput,
};
use controls::{
    Controls, InputKey, KeyMods, ReplyTracker, ReviewControls, SaveTracker, SubmitKind,
    UndoControl, derive_controls, guidance, route_input_key,
};
use gpui::{
    AnyElement, AppContext, Context, ElementId, Entity, EventEmitter, FollowMode,
    InteractiveElement, IntoElement, ListAlignment, ListState, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, list, px, rgb,
};
use host::{AdapterFields, AdapterFile, McpChoice};
use parking_lot::Mutex;
use qualification::{OwnershipPolicy, Resolution};
use rows::{ListOp, OVERDRAW_PX, ResidentLimits, RowKey, Viewport};
use std::{
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use studio_agent_spike::driver::{ConfigKind, OptionValue, ToolStatus};
use studio_engine::{DraftState, ReviewPolicy, TaskScope, app_paths::AppPaths};

const DANGER: u32 = 0x442c27;
const WARNING: u32 = 0x3a3420;
const SUCCESS: u32 = 0x1f3a2c;
const BUBBLE: u32 = 0x223048;
const CARD: u32 = 0x1d2736;

/// Events the shell reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEvent {
    /// Escape in an input: focus returns to the shell.
    Leave,
}

impl EventEmitter<PanelEvent> for ConversationPanel {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Chat,
    Project,
    Setup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Normal,
    Primary,
    Danger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Export {
    Candidate,
    Draft,
}

struct Attached {
    workflow: Arc<AgentWorkflow>,
    #[allow(dead_code)]
    paths: AppPaths,
    serial: u64,
}

/// Everything the shell hands the panel when a project's workflow opens.
pub struct Attachment {
    pub workflow: Arc<AgentWorkflow>,
    pub paths: AppPaths,
    /// The shell's open serial (hand-offs and history pages carry it).
    pub serial: u64,
    /// The saved adapter description (what the Setup tab starts with).
    pub adapter: Option<AdapterFile>,
    pub settings_error: Option<String>,
    /// How the saved adapter's writer containment was derived.
    pub resolution: Option<Resolution>,
    pub policy: OwnershipPolicy,
}

type PageResult = (u64, Result<Vec<Arc<Row>>, String>);

#[derive(Default)]
struct PageSlot {
    done: Mutex<Option<PageResult>>,
    loading: AtomicBool,
}

struct SetupForm {
    provider: Entity<TextInput>,
    executable: Entity<TextInput>,
    args: Entity<TextInput>,
    env_names: Entity<TextInput>,
}

pub struct ConversationPanel {
    attached: Option<Attached>,
    snapshot: Option<Arc<WorkflowSnapshot>>,
    tab: Tab,
    prompt: Entity<TextInput>,
    prompt_kind: Option<SubmitKind>,
    setup: SetupForm,
    saved_adapter: Option<AdapterFile>,
    settings_error: Option<String>,
    /// How the saved adapter's writer containment was derived (shown read-only).
    containment: Option<Resolution>,
    /// Who may say a writer is contained (production: validated evidence only).
    policy: OwnershipPolicy,
    saves: SaveTracker,
    mcp_choice: McpChoice,
    list: ListState,
    view_rows: Vec<Arc<Row>>,
    view_keys: Vec<RowKey>,
    older: Vec<Arc<Row>>,
    older_exhausted: bool,
    hidden_newer: usize,
    page: Arc<PageSlot>,
    viewport: Rc<Cell<Option<usize>>>,
    replies: ReplyTracker,
    notice: Option<String>,
    sdk_ready: bool,
    task_scope: Option<TaskScope>,
    scope_error: Option<String>,
    /// Measured bounds of the buttons a native driver must click (qualification only).
    button_bounds: std::collections::HashMap<String, [f32; 4]>,
    /// Current prompt input bounds, used by the native shell qualification harness.
    prompt_bounds: Option<[f32; 4]>,
    /// Current-task thumbnails are local presentation state, never workflow snapshot data.
    evidence_images: HashMap<String, Arc<gpui::RenderImage>>,
    /// Images removed from the current task; released from GPUI on the next render.
    retired_evidence_images: Vec<Arc<gpui::RenderImage>>,
}

fn input(placeholder: &str, cx: &mut Context<ConversationPanel>) -> Entity<TextInput> {
    let placeholder = placeholder.to_owned();
    cx.new(move |cx| {
        let mut input = TextInput::new(cx);
        input.placeholder = placeholder.into();
        input.compact = true;
        input
    })
}

impl ConversationPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(OVERDRAW_PX));
        list.set_follow_mode(FollowMode::Tail);
        let viewport = Rc::new(Cell::new(None));
        let seen = viewport.clone();
        list.set_scroll_handler(move |event, _, _| seen.set(Some(event.visible_range.start)));
        Self {
            attached: None,
            snapshot: None,
            tab: Tab::Chat,
            prompt: input("Describe the change you want", cx),
            prompt_kind: None,
            setup: SetupForm {
                provider: input("Label, for example my-adapter", cx),
                executable: input("Absolute path of the ACP adapter", cx),
                args: input("Arguments, for example --acp", cx),
                env_names: input("Sign-in variable NAMES, for example PROVIDER_API_KEY", cx),
            },
            saved_adapter: None,
            settings_error: None,
            containment: None,
            policy: OwnershipPolicy::Validated,
            saves: SaveTracker::default(),
            mcp_choice: McpChoice::Baseline,
            list,
            view_rows: Vec::new(),
            view_keys: Vec::new(),
            older: Vec::new(),
            older_exhausted: false,
            hidden_newer: 0,
            page: Arc::new(PageSlot::default()),
            viewport,
            replies: ReplyTracker::default(),
            notice: None,
            sdk_ready: false,
            task_scope: None,
            scope_error: None,
            button_bounds: std::collections::HashMap::new(),
            prompt_bounds: None,
            evidence_images: HashMap::new(),
            retired_evidence_images: Vec::new(),
        }
    }

    // ---- shell interface ---------------------------------------------------------------------

    /// Binds the panel to a freshly opened workflow.
    pub fn attach(&mut self, attachment: Attachment, cx: &mut Context<Self>) {
        let Attachment {
            workflow,
            paths,
            serial,
            adapter,
            settings_error,
            resolution,
            policy,
        } = attachment;
        self.detach(cx);
        self.attached = Some(Attached {
            workflow,
            paths,
            serial,
        });
        self.mcp_choice = adapter.as_ref().map_or(McpChoice::Baseline, |a| a.mcp);
        self.fill_setup(adapter.as_ref(), cx);
        self.saved_adapter = adapter;
        self.settings_error = settings_error;
        self.containment = resolution;
        self.policy = policy;
        self.tab = Tab::Chat;
        self.poll(cx);
        cx.notify();
    }

    /// Forgets the workflow (the shell closes it).
    pub fn detach(&mut self, cx: &mut Context<Self>) {
        self.attached = None;
        self.snapshot = None;
        self.task_scope = None;
        self.scope_error = None;
        self.older.clear();
        self.older_exhausted = false;
        self.hidden_newer = 0;
        self.view_rows.clear();
        self.view_keys.clear();
        self.list.reset(0);
        self.viewport.set(None);
        self.page.loading.store(false, Ordering::Release);
        self.page.done.lock().take();
        self.replies = ReplyTracker::default();
        self.notice = None;
        self.prompt_kind = None;
        self.saves.cancel();
        cx.notify();
    }

    pub fn workflow(&self) -> Option<&Arc<AgentWorkflow>> {
        self.attached.as_ref().map(|a| &a.workflow)
    }

    pub fn attached_serial(&self) -> Option<u64> {
        self.attached.as_ref().map(|a| a.serial)
    }

    pub fn set_sdk_ready(&mut self, ready: bool, cx: &mut Context<Self>) {
        if self.sdk_ready != ready {
            self.sdk_ready = ready;
            cx.notify();
        }
    }

    /// Receives the latest immutable selection snapshot from the compiled timeline.
    pub fn set_task_scope(
        &mut self,
        scope: Result<Option<TaskScope>, String>,
        cx: &mut Context<Self>,
    ) {
        match scope {
            Ok(scope) => {
                self.task_scope = scope;
                self.scope_error = None;
            }
            Err(error) => {
                self.task_scope = None;
                self.scope_error = Some(error);
            }
        }
        cx.notify();
    }

    /// True while the prompt (or a setup field) holds keyboard focus.
    pub fn has_text_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        use gpui::Focusable;
        let focused = |input: &Entity<TextInput>| input.focus_handle(cx).is_focused(window);
        focused(&self.prompt)
            || focused(&self.setup.provider)
            || focused(&self.setup.executable)
            || focused(&self.setup.args)
            || focused(&self.setup.env_names)
    }

    /// Redacted observation of the panel for the M3 telemetry file: counts and states,
    /// never prompt, brief or message text.
    pub fn telemetry(&self) -> serde_json::Value {
        let Some(snapshot) = &self.snapshot else {
            return serde_json::json!({"attached": false});
        };
        // Unique `Arc<Row>` allocations the panel itself holds (paged history plus what it
        // lays out); the workflow's own live window is shared with it and bounded by the
        // workflow, so it is reported separately instead of being counted twice.
        let (retained_rows, retained_bytes) =
            rows::unique_retained(&[&self.older, &self.view_rows]);
        let bytes: usize = self.view_rows.iter().map(|r| r.estimated_bytes()).sum();
        serde_json::json!({
            "attached": true,
            "revision": snapshot.revision,
            "phase": snapshot.task.as_ref().map(|t| t.phase.label()),
            "repair_used": snapshot.task.as_ref().map(|t| t.repair.used),
            "queue": snapshot.queue.len(),
            "queue_stale": snapshot.queue.iter().filter(|q| q.stale_scope).count(),
            "error_codes": snapshot.rows.iter().filter_map(|r| match &r.kind {
                RowKind::Error(e) => Some(e.code.clone()),
                _ => None,
            }).take(20).collect::<Vec<_>>(),
            "review_policy": format!("{:?}", snapshot.review_policy),
            "list_items": self.list.item_count(),
            "view_rows": self.view_rows.len(),
            "view_bytes": bytes,
            "retained_rows": retained_rows,
            "retained_bytes": retained_bytes,
            "live_rows": snapshot.rows.len(),
            "older_loaded": self.older.len(),
            "hidden_newer": self.hidden_newer,
            "open_permissions": snapshot.resources.open_permissions,
            "owned_processes": snapshot.resources.owned_processes,
            "broker_grants": snapshot.resources.broker_grants,
            "handoff": snapshot.handoff.as_ref().map(|h| format!("{:?}", h.state)),
            "buttons": self.button_bounds,
            "prompt_bounds": self.prompt_bounds,
            "scope_error": self.scope_error,
            "submit_scope": self.task_scope.as_ref().map(|scope| scope.label()),
            "task_scope": snapshot.task.as_ref().map(|t| t.scope.label()),
            "image_limitation": snapshot.task.as_ref().and_then(|t| t.image_limitation.clone()),
            "evidence_preview_count": self.evidence_images.len(),
            "before_evidence": snapshot.task.as_ref().and_then(|t| t.before.as_ref()).map(evidence_telemetry),
            "after_evidence": snapshot.task.as_ref().and_then(|t| t.after.as_ref()).map(evidence_telemetry),
            "undo": match &snapshot.undo {
                UndoView::Available { .. } => "available",
                UndoView::Unavailable { .. } => "unavailable",
                UndoView::InProgress { .. } => "in_progress",
            },
        })
    }

    /// Takes the newest snapshot and finished history pages. Cheap; the shell calls it from
    /// its frame loop. Returns whether anything changed.
    pub fn poll(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(attached) = &self.attached else {
            return false;
        };
        let mut changed = false;
        let snapshot = attached.workflow.snapshot();
        let serial = attached.serial;
        if self
            .snapshot
            .as_ref()
            .is_none_or(|current| current.revision != snapshot.revision)
        {
            self.snapshot = Some(snapshot);
            self.after_snapshot(cx);
            changed = true;
        }
        changed |= self.sync_evidence_images();
        let finished = self.page.done.lock().take();
        if let Some((page_serial, result)) = finished {
            self.page.loading.store(false, Ordering::Release);
            if page_serial == serial {
                match result {
                    Ok(page) => {
                        let wanted = rows::PAGE_ROWS;
                        self.older_exhausted = page.len() < wanted;
                        rows::prepend_page(&mut self.older, page, ResidentLimits::default());
                        self.recompose(cx);
                        // The next page waits for the reader to scroll again; a stale
                        // "at the top" would otherwise page the whole log in.
                        self.viewport.set(None);
                    }
                    Err(error) => {
                        self.notice = Some(format!("Earlier messages unavailable: {error}"))
                    }
                }
                changed = true;
            }
        }
        self.maybe_request_page();
        if changed {
            cx.notify();
        }
        changed
    }

    fn after_snapshot(&mut self, cx: &mut Context<Self>) {
        self.recompose(cx);
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let controls = derive_controls(&snapshot, self.prompt_is_empty(cx), self.sdk_ready);
        if self.prompt_kind.as_ref() != Some(&controls.submit.kind) {
            let placeholder = match controls.submit.kind {
                SubmitKind::Brief => "Describe the change you want",
                SubmitKind::Queue => "Queue a follow-up; it runs after the current task",
                SubmitKind::Clarify(_) => "Answer the agent's question",
            };
            self.prompt.update(cx, |input, cx| {
                input.placeholder = placeholder.into();
                cx.notify();
            });
            self.prompt_kind = Some(controls.submit.kind);
        }
    }

    /// Copies only decoded UI handles for the current task out of the workflow's bounded
    /// presentation cache, and retires no-longer-visible images for GPUI cleanup.
    fn sync_evidence_images(&mut self) -> bool {
        let mut desired = HashMap::new();
        if let (Some(attached), Some(snapshot)) = (&self.attached, &self.snapshot)
            && let Some(task) = &snapshot.task
        {
            for view in [task.before.as_ref(), task.after.as_ref()]
                .into_iter()
                .flatten()
            {
                for artifact in &view.artifacts {
                    if let Some(image) = attached.workflow.evidence_image(&task.id, &artifact.id) {
                        desired.insert(evidence_image_key(&task.id, &artifact.id), image);
                    }
                }
            }
        }

        let retired: Vec<String> = self
            .evidence_images
            .keys()
            .filter(|key| !desired.contains_key(*key))
            .cloned()
            .collect();
        let mut changed = false;
        for key in retired {
            if let Some(image) = self.evidence_images.remove(&key) {
                self.retired_evidence_images.push(image);
                changed = true;
            }
        }
        for (key, image) in desired {
            if self
                .evidence_images
                .get(&key)
                .is_none_or(|current| !Arc::ptr_eq(current, &image))
            {
                self.evidence_images.insert(key, image);
                changed = true;
            }
        }
        changed
    }

    fn recompose(&mut self, _cx: &mut Context<Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let view = rows::compose_view(&self.older, &snapshot.rows, ResidentLimits::default());
        let keys: Vec<RowKey> = view.rows.iter().map(RowKey::of).collect();
        for op in rows::diff_rows(&self.view_keys, &keys) {
            match op {
                ListOp::Splice { at, old, new } => self.list.splice(at..at + old, new),
                ListOp::Remeasure { at, len } => self.list.remeasure_items(at..at + len),
                ListOp::Reset { len } => self.list.reset(len),
            }
        }
        self.view_rows = view.rows;
        self.view_keys = keys;
        self.hidden_newer = view.hidden_newer;
    }

    fn log_has_older(&self) -> bool {
        if self.older.is_empty() {
            self.snapshot.as_ref().is_some_and(|s| s.older_rows) && !self.older_exhausted
        } else {
            !self.older_exhausted
        }
    }

    fn maybe_request_page(&mut self) {
        let Some(first_visible) = self.viewport.get() else {
            return;
        };
        if let Some((before, limit)) = rows::page_request(
            &self.view_rows,
            Viewport { first_visible },
            self.log_has_older(),
            self.page.loading.load(Ordering::Acquire),
        ) {
            self.request_page(before, limit);
        }
    }

    fn request_page(&mut self, before: u64, limit: usize) {
        let Some(attached) = &self.attached else {
            return;
        };
        if self.page.loading.swap(true, Ordering::AcqRel) {
            return;
        }
        let slot = self.page.clone();
        let serial = attached.serial;
        let started = attached.workflow.history_page_with(
            crate::agent_workflow::RowId(before),
            limit,
            move |result| {
                *slot.done.lock() = Some((serial, result.map_err(|e| e.to_string())));
            },
        );
        if let Err(error) = started {
            self.page.loading.store(false, Ordering::Release);
            self.notice = Some(error.to_string());
        }
    }

    fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        self.older.clear();
        self.older_exhausted = false;
        self.recompose(cx);
        self.list.set_follow_mode(FollowMode::Tail);
        self.list.scroll_to_end();
        cx.notify();
    }

    fn prompt_is_empty(&self, cx: &gpui::App) -> bool {
        self.prompt.read(cx).content().trim().is_empty()
    }

    fn fill_setup(&mut self, adapter: Option<&AdapterFile>, cx: &mut Context<Self>) {
        let fields = adapter.map(AdapterFile::fields).unwrap_or_default();
        let set = |input: &Entity<TextInput>, text: String, cx: &mut Context<Self>| {
            input.update(cx, |input, cx| input.set_text(text, cx));
        };
        set(&self.setup.provider, fields.provider, cx);
        set(&self.setup.executable, fields.executable, cx);
        set(&self.setup.args, fields.args, cx);
        set(&self.setup.env_names, fields.env_names, cx);
    }

    fn setup_fields(&self, cx: &gpui::App) -> AdapterFields {
        let text = |input: &Entity<TextInput>| input.read(cx).content().to_owned();
        AdapterFields {
            provider: text(&self.setup.provider),
            executable: text(&self.setup.executable),
            args: text(&self.setup.args),
            env_names: text(&self.setup.env_names),
        }
    }

    // ---- commands ------------------------------------------------------------------------------

    fn say(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.notice = Some(message.into());
        cx.notify();
    }

    fn command<T>(
        &mut self,
        result: Result<T, crate::agent_workflow::WorkflowError>,
        cx: &mut Context<Self>,
    ) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                self.say(error.to_string(), cx);
                None
            }
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let (Some(attached), Some(snapshot)) = (&self.attached, &self.snapshot) else {
            return;
        };
        let controls = derive_controls(snapshot, self.prompt_is_empty(cx), self.sdk_ready);
        if let Some(reason) = controls.submit.blocked {
            self.say(reason, cx);
            return;
        }
        if let Some(error) = &self.scope_error {
            self.say(
                format!("The selected timeline scope is unavailable: {error}"),
                cx,
            );
            return;
        }
        let text = self.prompt.read(cx).content().to_owned();
        let workflow = attached.workflow.clone();
        let scope = self.task_scope.clone();
        let sent = match &controls.submit.kind {
            SubmitKind::Clarify(task) => workflow.reply_clarification(task, &text),
            SubmitKind::Brief | SubmitKind::Queue => match scope {
                Some(scope) => workflow.submit_scoped(&text, scope).map(|_| ()),
                None => workflow.submit(&text).map(|_| ()),
            },
        };
        if self.command(sent, cx).is_some() {
            self.notice = None;
            self.prompt.update(cx, |input, cx| input.set_text("", cx));
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(workflow) = self.workflow().cloned() {
            let result = workflow.stop();
            self.command(result, cx);
        }
    }

    fn answer_permission(
        &mut self,
        reference: PermissionRef,
        answer: PermissionAnswer,
        cx: &mut Context<Self>,
    ) {
        let (Some(attached), Some(snapshot)) = (&self.attached, &self.snapshot) else {
            return;
        };
        let workflow = attached.workflow.clone();
        let open = snapshot.open_permissions();
        if let Err(refusal) = self.replies.begin(&reference, &open) {
            self.say(refusal.to_string(), cx);
            return;
        }
        let result = workflow.reply_permission(&reference, answer);
        self.command(result, cx);
        cx.notify();
    }

    /// Saves the Setup fields. The qualification of the adapter (hashing its executable and
    /// reading the ledger) and the file write both run on background threads; at most one
    /// save is in flight (a second click is refused until it completes), the running
    /// workflow is only reconfigured while idle, and only the in-flight save's completion
    /// is reported.
    fn save_adapter(&mut self, cx: &mut Context<Self>) {
        let Some(attached) = &self.attached else {
            return;
        };
        let paths = attached.paths.clone();
        let built = AdapterFile::from_fields(
            self.saved_adapter.as_ref(),
            &self.setup_fields(cx),
            self.mcp_choice,
        );
        let file = match built {
            Ok(file) => file,
            Err(error) => {
                self.say(error, cx);
                return;
            }
        };
        if let Some(snapshot) = &self.snapshot
            && !derive_controls(snapshot, self.prompt_is_empty(cx), self.sdk_ready)
                .settings_editable
        {
            self.say(
                "Settings can change after the running task finishes or is stopped.",
                cx,
            );
            return;
        }
        let ticket = match self.saves.begin() {
            Ok(ticket) => ticket,
            Err(refusal) => {
                self.say(refusal, cx);
                return;
            }
        };
        let policy = self.policy.clone();
        let resolving = {
            let (file, paths) = (file.clone(), paths.clone());
            cx.background_executor()
                .spawn(async move { file.map(|file| file.resolve(&paths, &policy)) })
        };
        cx.spawn(async move |this, cx| {
            let resolved = resolving.await;
            let _ = this.update(cx, |panel, cx| {
                panel.apply_saved(ticket, file, resolved, paths, cx)
            });
        })
        .detach();
        self.say("Saving the adapter settings…", cx);
    }

    /// Second half of a save, back on the UI thread: reconfigure the workflow, then persist.
    fn apply_saved(
        &mut self,
        ticket: u64,
        file: Option<AdapterFile>,
        resolved: Option<(crate::agent_workflow::AdapterSettings, Resolution)>,
        paths: AppPaths,
        cx: &mut Context<Self>,
    ) {
        if !self.saves.is_current(ticket) {
            return;
        }
        let Some(attached) = &self.attached else {
            self.saves.finish(ticket);
            return;
        };
        let (settings, resolution) = match resolved {
            Some((settings, resolution)) => (Some(settings), Some(resolution)),
            None => (None, None),
        };
        let applied = attached.workflow.set_adapter(settings);
        if self.command(applied, cx).is_none() {
            self.saves.finish(ticket);
            return;
        }
        self.saved_adapter = file.clone();
        self.containment = resolution;
        self.settings_error = None;
        let task = cx.background_executor().spawn(async move {
            match &file {
                Some(file) => host::save_settings(&paths, file),
                None => host::clear_settings(&paths),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |panel, cx| {
                if !panel.saves.finish(ticket) {
                    return;
                }
                panel.notice = Some(match result {
                    Ok(()) => {
                        "Adapter settings saved on this computer. Use Check adapter to verify them."
                            .into()
                    }
                    Err(error) => error,
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn export(&mut self, kind: Export, cx: &mut Context<Self>) {
        let Some(workflow) = self.workflow().cloned() else {
            return;
        };
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let picker = cx.prompt_for_new_path(
            &home,
            Some(match kind {
                Export::Candidate => "Candidate video",
                Export::Draft => "Retained draft",
            }),
        );
        cx.spawn(async move |this, cx| {
            let chosen = picker.await;
            let _ = this.update(cx, |panel, cx| match chosen {
                Ok(Ok(Some(path))) => {
                    let sent = match kind {
                        Export::Candidate => workflow.export_candidate(&path),
                        Export::Draft => workflow.export_draft(&path),
                    };
                    if panel.command(sent, cx).is_some() {
                        panel.say(format!("Exporting to {}…", path.display()), cx);
                    }
                }
                Ok(Ok(None)) => (),
                other => panel.say(
                    format!("Native picker failed: {other:?}; check the desktop portal/file chooser and retry"),
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Key routing for one text field. The composition state is the field's own, read at
    /// the moment of the key: the prompt and every Setup field share this one path.
    fn key_in_field(
        &mut self,
        field: &Entity<TextInput>,
        event: &gpui::KeyDownEvent,
        submit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let composing = field.read(cx).is_composing();
        self.key_in_input(event, composing, submit, window, cx);
    }

    fn key_in_input(
        &mut self,
        event: &gpui::KeyDownEvent,
        composing: bool,
        submit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = &event.keystroke.modifiers;
        let mods = KeyMods {
            shift: modifiers.shift,
            alt: modifiers.alt,
            secondary: modifiers.control || modifiers.platform,
        };
        match route_input_key(event.keystroke.key.as_str(), mods, composing) {
            InputKey::Submit => {
                if submit {
                    self.submit(cx);
                }
                cx.stop_propagation();
            }
            InputKey::FocusNext => {
                window.focus_next(cx);
                cx.stop_propagation();
            }
            InputKey::FocusPrev => {
                window.focus_prev(cx);
                cx.stop_propagation();
            }
            InputKey::Leave => {
                cx.emit(PanelEvent::Leave);
                cx.stop_propagation();
            }
            InputKey::Composition => cx.stop_propagation(),
            // Typing, IME text, Space and the arrows belong to the input.
            InputKey::Text => (),
        }
    }

    // ---- building blocks -----------------------------------------------------------------------

    fn button(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        enabled: bool,
        tone: Tone,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let label: SharedString = label.into();
        let action = Rc::new(action);
        let on_click = action.clone();
        let id: ElementId = id.into();
        let measured = match &id {
            ElementId::Name(name) if matches!(name.as_ref(), "agent-policy" | "agent-apply") => {
                Some(name.to_string())
            }
            _ => None,
        };
        let entity = cx.entity();
        let (background, border) = match tone {
            Tone::Normal => (PANEL, BORDER),
            Tone::Primary => (0x24476f, ACCENT),
            Tone::Danger => (0x5a2f2b, 0xc8736a),
        };
        div()
            .id(id)
            .relative()
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .tab_index(0)
            .tab_stop(enabled)
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(background))
            .text_xs()
            .text_color(rgb(if enabled { TEXT } else { MUTED }))
            .opacity(if enabled { 1.0 } else { 0.45 })
            .focus_visible(|s| s.border_color(rgb(0xffffff)))
            .on_click(cx.listener(move |this, _, window, cx| {
                if enabled {
                    on_click(this, window, cx);
                }
            }))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if enabled && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        action(this, window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .child(label)
            .children(measured.map(|name| {
                canvas(
                    move |bounds, _, cx| {
                        entity.update(cx, |panel, _| {
                            panel.button_bounds.insert(
                                name,
                                [
                                    f32::from(bounds.left()),
                                    f32::from(bounds.top()),
                                    f32::from(bounds.size.width),
                                    f32::from(bounds.size.height),
                                ],
                            );
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            }))
    }

    fn tab_button(
        &self,
        tab: Tab,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.tab == tab;
        div()
            .id(format!("agent-tab-{label}"))
            .role(gpui::Role::Tab)
            .aria_label(label)
            .tab_index(0)
            .px_2()
            .py_1()
            .text_sm()
            .border_b_2()
            .border_color(rgb(if active { ACCENT } else { PANEL }))
            .text_color(rgb(if active { TEXT } else { MUTED }))
            .focus_visible(|s| s.border_color(rgb(0xffffff)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
            .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    this.tab = tab;
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            .child(label)
    }

    fn section(title: &str) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .bg(rgb(CARD))
            .border_1()
            .border_color(rgb(BORDER))
            .child(div().text_sm().child(title.to_owned()))
    }

    fn line(text: impl Into<SharedString>) -> gpui::Div {
        div().text_xs().text_color(rgb(MUTED)).child(text.into())
    }

    // ---- rows ------------------------------------------------------------------------------------

    fn row_element(&mut self, row: &Arc<Row>, cx: &mut Context<Self>) -> AnyElement {
        let frame = |tint: u32| {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .mb_1()
                .rounded_md()
                .bg(rgb(tint))
                .text_sm()
                .text_color(rgb(TEXT))
        };
        let label = |text: &str| {
            div()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(text.to_owned())
        };
        match &row.kind {
            RowKind::User { text, source } => frame(BUBBLE)
                .child(label(match source {
                    UserSource::Brief => "You",
                    UserSource::Clarification => "You (answer)",
                    UserSource::Repair => "Automatic repair context (sent by Studio)",
                }))
                .child(text.clone())
                .into_any_element(),
            RowKind::Agent {
                text, streaming, ..
            } => frame(CARD)
                .child(label(if *streaming {
                    "Agent · writing…"
                } else {
                    "Agent"
                }))
                .child(text.clone())
                .into_any_element(),
            RowKind::Thought {
                text, streaming, ..
            } => frame(PANEL)
                .text_color(rgb(MUTED))
                .child(label(if *streaming {
                    "Reasoning · writing…"
                } else {
                    "Reasoning"
                }))
                .child(text.clone())
                .into_any_element(),
            RowKind::Tool(card) => self.tool_card(card, frame(CARD)),
            RowKind::Permission(card) => self.permission_card(card, frame(WARNING), cx),
            RowKind::Notice { level, text } => frame(match level {
                NoticeLevel::Info => PANEL,
                NoticeLevel::Warning => WARNING,
            })
            .text_xs()
            .child(text.clone())
            .into_any_element(),
            RowKind::Error(error) => self.error_card(error, frame(DANGER)),
            RowKind::Validation(card) => self.validation_card(card, frame(CARD)),
            RowKind::Changes(card) => self.changes_card(card, frame(CARD)),
            RowKind::Outcome(card) => {
                let (tint, title) = match card.kind {
                    OutcomeKind::Accepted => (SUCCESS, "Accepted"),
                    OutcomeKind::Undone => (SUCCESS, "Undone"),
                    OutcomeKind::AwaitingReview => (WARNING, "Waiting for your review"),
                    OutcomeKind::Conflict => (DANGER, "Conflict"),
                    OutcomeKind::Failed => (DANGER, "Failed"),
                    OutcomeKind::Cancelled => (PANEL, "Stopped"),
                    OutcomeKind::Interrupted => (WARNING, "Interrupted"),
                    OutcomeKind::Discarded => (PANEL, "Discarded"),
                };
                frame(tint)
                    .child(div().child(title))
                    .child(card.summary.clone())
                    .children(
                        card.published
                            .as_ref()
                            .map(|p| Self::line(format!("Revision {}", &p[..p.len().min(12)]))),
                    )
                    .children(
                        card.draft_retained
                            .then(|| Self::line("The agent's working copy was kept.")),
                    )
                    .into_any_element()
            }
        }
    }

    fn tool_card(&self, card: &ToolCard, frame: gpui::Div) -> AnyElement {
        let status = match card.status {
            Some(ToolStatus::Pending) => "pending",
            Some(ToolStatus::InProgress) => "running",
            Some(ToolStatus::Completed) => "done",
            Some(ToolStatus::Failed) => "failed",
            None => "…",
        };
        frame
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(format!("Tool · {}", card.kind.as_deref().unwrap_or("call"))),
                    )
                    .child(div().text_xs().child(status)),
            )
            .child(card.title.clone().unwrap_or_else(|| card.call_id.clone()))
            .children((card.updates > 1).then(|| Self::line(format!("{} updates", card.updates))))
            .into_any_element()
    }

    fn permission_card(
        &mut self,
        card: &PermissionCard,
        frame: gpui::Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let pending = self.replies.is_pending(&card.reference);
        let open = card.state == PermissionState::Open && !pending;
        let state = match &card.state {
            PermissionState::Open if pending => "Answer sent…".to_owned(),
            PermissionState::Open => "Waiting for your choice".to_owned(),
            PermissionState::Selected { option_id } => format!(
                "You chose {}",
                card.options
                    .iter()
                    .find(|o| &o.option_id == option_id)
                    .map_or(option_id.as_str(), |o| o.name.as_str())
            ),
            PermissionState::Cancelled => "Closed without an answer".to_owned(),
        };
        let mut actions = div().flex().flex_wrap().gap_1();
        let key = format!(
            "{}-{}-{}",
            &card.reference.task.0[..card.reference.task.0.len().min(8)],
            card.reference.writer,
            card.reference.request
        );
        if open {
            for option in &card.options {
                let reference = card.reference.clone();
                let id = option.option_id.clone();
                actions = actions.child(self.button(
                    format!("perm-{key}-{}", option.option_id),
                    option.name.clone(),
                    true,
                    if option.kind.starts_with("allow") {
                        Tone::Primary
                    } else {
                        Tone::Normal
                    },
                    cx,
                    move |this, _, cx| {
                        this.answer_permission(
                            reference.clone(),
                            PermissionAnswer::Select(id.clone()),
                            cx,
                        )
                    },
                ));
            }
            let reference = card.reference.clone();
            actions = actions.child(self.button(
                format!("perm-{key}-decline"),
                "Decline",
                true,
                Tone::Normal,
                cx,
                move |this, _, cx| {
                    this.answer_permission(reference.clone(), PermissionAnswer::Cancel, cx)
                },
            ));
        }
        frame
            .child(Self::line("Permission requested"))
            .child(card.title.clone())
            .child(Self::line(state))
            .child(actions)
            .into_any_element()
    }

    fn error_card(&self, error: &StructuredError, frame: gpui::Div) -> AnyElement {
        frame
            .child(div().child(error.title.clone()))
            .child(error.detail.clone())
            .children(
                error
                    .action
                    .as_ref()
                    .map(|a| Self::line(format!("Next: {a}"))),
            )
            .children(
                error
                    .phase
                    .as_ref()
                    .map(|p| Self::line(format!("During {p} · code {}", error.code))),
            )
            .children(
                error
                    .retained
                    .then(|| Self::line("The working copy and candidate were kept.")),
            )
            .into_any_element()
    }

    fn validation_card(&self, card: &ValidationCard, frame: gpui::Div) -> AnyElement {
        let mut body = frame.child(div().child(format!(
            "{} · {}",
            if card.passed {
                "Validation passed"
            } else {
                "Validation failed"
            },
            card.summary
        )));
        if let Some(c) = &card.coverage {
            body = body.child(Self::line(format!(
                "Coverage: {} of {} frames rendered, {} inspected, {} boundary frames, playhead {}{}{}",
                c.rendered_frames,
                c.total_frames,
                c.inspected_frames,
                c.boundary_frames,
                c.playhead,
                c.broadened
                    .as_ref()
                    .map(|b| format!(", broadened: {b}"))
                    .unwrap_or_default(),
                if c.complete { "" } else { " · PARTIAL" }
            )));
        }
        if let Some(a) = &card.audio {
            body = body.child(Self::line(format!(
                "Audio: {}, peak {:.2}, {} Hz, placement {}",
                if a.silent { "silent" } else { "present" },
                a.peak,
                a.sample_rate,
                if a.placement_verified {
                    "verified"
                } else {
                    "not verified"
                }
            )));
        }
        for error in card.errors.iter().take(6) {
            body = body.child(
                div()
                    .text_xs()
                    .child(format!("{:?} · {}", error.stage, error.message)),
            );
        }
        if card.errors_total > card.errors.len().min(6) {
            body = body.child(Self::line(format!(
                "{} more error{}",
                card.errors_total - card.errors.len().min(6),
                if card.errors_total - card.errors.len().min(6) == 1 {
                    ""
                } else {
                    "s"
                }
            )));
        }
        if !card.warnings.is_empty() {
            body = body.child(Self::line(format!(
                "{} warning{}: {}",
                card.warnings.len(),
                if card.warnings.len() == 1 { "" } else { "s" },
                card.warnings
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" · ")
            )));
        }
        if !card.capability_gaps.is_empty() {
            body = body.child(Self::line(format!(
                "Not verified by this preview: {}",
                card.capability_gaps.join(", ")
            )));
        }
        if let Some(f) = &card.failure {
            body = body.child(Self::line(format!(
                "{:?} failure in {:?}: {}",
                f.kind, f.stage, f.summary
            )));
        }
        body.child(Self::line(format!(
            "Candidate {}{}{} · repair {}",
            &card.candidate[..card.candidate.len().min(12)],
            card.build_key
                .as_ref()
                .map(|k| format!(" · build {}", &k[..k.len().min(12)]))
                .unwrap_or_default(),
            format_args!(" · {} diagnostics", card.diagnostics_total),
            card.repair_count
        )))
        .into_any_element()
    }

    fn changes_card(&self, card: &ChangeCard, frame: gpui::Div) -> AnyElement {
        let mut body = frame.child(div().child(format!(
            "{} changed file{}",
            card.total,
            if card.total == 1 { "" } else { "s" }
        )));
        for entry in &card.entries {
            let mark = match entry.change {
                studio_engine::candidate_validation::ChangeKind::Added => "A",
                studio_engine::candidate_validation::ChangeKind::Modified => "M",
                studio_engine::candidate_validation::ChangeKind::Deleted => "D",
                studio_engine::candidate_validation::ChangeKind::Mode => "X",
            };
            body = body.child(div().text_xs().child(format!("{mark}  {}", entry.path)));
        }
        if card.truncated || card.total > card.entries.len() {
            body = body.child(Self::line(format!(
                "…and {} more",
                card.total.saturating_sub(card.entries.len())
            )));
        }
        body.into_any_element()
    }

    // ---- tabs ----------------------------------------------------------------------------------------

    fn banner(
        &mut self,
        snapshot: &WorkflowSnapshot,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let task = snapshot.task.as_ref()?;
        let tint = match task.phase {
            TaskPhase::Accepted => SUCCESS,
            TaskPhase::Failed | TaskPhase::Conflict => DANGER,
            TaskPhase::AwaitingReview
            | TaskPhase::AwaitingEvidence
            | TaskPhase::WaitingPermission
            | TaskPhase::WaitingClarification
            | TaskPhase::Interrupted => WARNING,
            _ => CARD,
        };
        let mut banner = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_md()
            .bg(rgb(tint))
            .border_1()
            .border_color(rgb(BORDER))
            .child(div().text_sm().child(format!(
                "{}{}",
                task.phase.label(),
                if task.stop_requested {
                    " · stopping"
                } else {
                    ""
                }
            )));
        let id = task.id.0.as_str();
        banner = banner.child(Self::line(format!(
            "Task {} · generation {} · base {}",
            &id[..id.len().min(8)],
            task.generation,
            task.source_base
        )));
        if !matches!(
            task.phase,
            TaskPhase::AwaitingReview | TaskPhase::AwaitingEvidence
        ) {
            banner = banner.child(Self::line(format!(
                "{} · {} of {} automatic repairs",
                match task.review_policy {
                    ReviewPolicy::AutoApply => "Apply automatically",
                    ReviewPolicy::ManualReview => "Manual review",
                },
                task.repair.used,
                task.repair.max
            )));
        }
        if let Some(reason) = &task.reason {
            banner = banner.child(Self::line(reason.clone()));
        }
        if let Some(note) = &controls.stop
            && !task.phase.is_terminal()
        {
            banner = banner.child(Self::line(note.clone()));
        }
        if let Some(conflict) = &task.conflict {
            banner = banner.child(self.conflict_block(conflict));
        }
        if let Some(review) = &controls.review {
            banner = banner.child(self.review_buttons(review, cx));
        }
        Some(banner)
    }

    fn review_buttons(&mut self, review: &ReviewControls, cx: &mut Context<Self>) -> gpui::Div {
        let mut row = div().flex().flex_col().gap_1();
        if let Some(note) = &review.apply_note {
            row = row.child(Self::line(format!(
                "Apply is blocked: {note}. The candidate is kept; retry after fixing it."
            )));
        }
        row.child(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .child(self.button(
                    "agent-apply",
                    if review.apply_note.is_some() {
                        "Retry Apply"
                    } else {
                        "Apply"
                    },
                    true,
                    Tone::Primary,
                    cx,
                    |this, _, cx| {
                        if let Some(w) = this.workflow().cloned() {
                            let r = w.apply();
                            this.command(r, cx);
                        }
                    },
                ))
                .child(self.button(
                    "agent-discard",
                    "Discard",
                    true,
                    Tone::Danger,
                    cx,
                    |this, _, cx| {
                        if let Some(w) = this.workflow().cloned() {
                            let r = w.discard();
                            this.command(r, cx);
                        }
                    },
                ))
                .child(self.button(
                    "agent-export-candidate",
                    "Export candidate…",
                    true,
                    Tone::Normal,
                    cx,
                    |this, _, cx| this.export(Export::Candidate, cx),
                )),
        )
    }

    fn conflict_block(&self, conflict: &ConflictView) -> gpui::Div {
        let list = |title: &str, paths: &[String]| {
            let mut block = div().flex().flex_col();
            if !paths.is_empty() {
                block = block.child(Self::line(title.to_owned()));
                for path in paths.iter().take(8) {
                    block = block.child(div().text_xs().child(format!("  {path}")));
                }
                if paths.len() > 8 {
                    block = block.child(Self::line(format!("  …and {} more", paths.len() - 8)));
                }
            }
            block
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .child(format!("Conflict: {}", conflict.reason)),
            )
            .child(list(
                "Changed in the project meanwhile:",
                &conflict.external_paths,
            ))
            .child(list("The candidate changes:", &conflict.candidate_paths))
            .child(list("Changed on both sides:", &conflict.overlapping))
            .children((!conflict.retained_variants.is_empty()).then(|| {
                let mut block = div().flex().flex_col();
                block = block.child(Self::line("Preserved variants (nothing was overwritten):"));
                for variant in conflict.retained_variants.iter().take(8) {
                    block = block.child(
                        div()
                            .text_xs()
                            .child(format!("  {} · {}", variant.path, variant.role)),
                    );
                }
                block
            }))
    }

    fn options_bar(
        &mut self,
        snapshot: &WorkflowSnapshot,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        if !controls.modes && !controls.config_options {
            return controls
                .options_note
                .as_ref()
                .map(|note| div().child(Self::line(note.clone())));
        }
        let mut bar = div().flex().flex_col().items_start().gap_1();
        if controls.modes {
            let mut row = div()
                .flex()
                .flex_wrap()
                .gap_1()
                .items_center()
                .child(Self::line("Mode"));
            for mode in &snapshot.options.modes {
                let selected = snapshot.options.current_mode.as_deref() == Some(mode.id.as_str());
                let id = mode.id.clone();
                row = row.child(self.button(
                    format!("mode-{}", mode.id),
                    if selected {
                        format!("● {}", mode.name)
                    } else {
                        mode.name.clone()
                    },
                    !selected,
                    Tone::Normal,
                    cx,
                    move |this, _, cx| {
                        if let Some(w) = this.workflow().cloned() {
                            let r = w.set_mode(&id);
                            this.command(r, cx);
                        }
                    },
                ));
            }
            bar = bar.child(row);
        }
        if controls.config_options {
            for option in &snapshot.options.config {
                let option_id = option.id.clone();
                let (label, next) = match &option.kind {
                    ConfigKind::Boolean { current } => (
                        format!("{}: {}", option.name, if *current { "on" } else { "off" }),
                        OptionValue::Boolean(!current),
                    ),
                    ConfigKind::Select { current, values } => {
                        let at = values.iter().position(|v| &v.value == current).unwrap_or(0);
                        let name = values.get(at).map_or(current.as_str(), |v| v.name.as_str());
                        let next = values
                            .get((at + 1) % values.len().max(1))
                            .map_or(current.clone(), |v| v.value.clone());
                        (format!("{}: {name}", option.name), OptionValue::Value(next))
                    }
                };
                bar = bar.child(self.button(
                    format!("config-{}", option.id),
                    label,
                    true,
                    Tone::Normal,
                    cx,
                    move |this, _, cx| {
                        if let Some(w) = this.workflow().cloned() {
                            let r = w.set_config_option(&option_id, next.clone());
                            this.command(r, cx);
                        }
                    },
                ));
            }
        }
        Some(bar)
    }

    fn chat_tab(
        &mut self,
        snapshot: Arc<WorkflowSnapshot>,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut column = div().flex_1().min_h_0().flex().flex_col().gap_2().p_2();
        let g = guidance(
            &snapshot.adapter.readiness,
            snapshot.adapter.provider.is_some(),
        );
        if let Some(banner) = self.banner(&snapshot, controls, cx) {
            column = column.child(banner);
        } else if !g.usable || snapshot.rows.is_empty() {
            column = column.child(
                Self::section(&g.headline)
                    .children(g.steps.iter().take(3).map(|s| Self::line(s.clone()))),
            );
        }
        if let Some(bar) = self.options_bar(&snapshot, controls, cx) {
            column = column.child(bar);
        }
        if let Some(error) = &self.scope_error {
            column = column.child(Self::line(format!("Timeline scope unavailable: {error}")));
        } else if let Some(scope) = &self.task_scope {
            let same_as_task = snapshot
                .task
                .as_ref()
                .filter(|task| !task.phase.is_terminal())
                .is_some_and(|task| task.scope.label() == scope.label());
            if !same_as_task {
                column = column.child(Self::line(format!("Submit scope: {}", scope.label())));
            }
        }
        if let Some(task) = &snapshot.task {
            let label = if task.phase.is_terminal() {
                "Last task scope"
            } else {
                "Frozen task scope"
            };
            column = column.child(Self::line(format!("{label}: {}", task.scope.label())));
        }
        if let Some(task) = &snapshot.task {
            let mut evidence_cards = div().flex().gap_1();
            let mut has_evidence_card = false;
            for (name, view) in [("Before", &task.before), ("After", &task.after)] {
                if let Some(view) = view {
                    let selected = view
                        .artifacts
                        .iter()
                        .find(|artifact| artifact.label == "selected")
                        .or_else(|| view.artifacts.first());
                    let image = selected.and_then(|artifact| {
                        self.evidence_images
                            .get(&evidence_image_key(&task.id, &artifact.id))
                    });
                    if let (Some(artifact), Some(image)) = (selected, image) {
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
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .child(format!("{name} · revision {}", view.revision)),
                                )
                                .child(gpui::img(image.clone()).w(px(128.)).h(px(72.)))
                                .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                                    "{} · {} frames{}",
                                    artifact.label,
                                    artifact.frames.len(),
                                    if boundaries.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" · edge {boundaries}")
                                    }
                                ))),
                        );
                    } else {
                        column = column.child(Self::line(evidence_summary(name, view)));
                    }
                }
            }
            if has_evidence_card {
                column = column.child(evidence_cards);
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
                column = column.child(Self::line(summary.to_owned()));
            }
        }
        if self.log_has_older() {
            let loading = self.page.loading.load(Ordering::Acquire);
            column = column.child(self.button(
                "agent-load-earlier",
                if loading {
                    "Loading earlier messages…"
                } else {
                    "Load earlier messages"
                },
                !loading,
                Tone::Normal,
                cx,
                |this, _, cx| {
                    if let Some(first) = this.view_rows.first() {
                        let before = first.id.0;
                        this.request_page(before, rows::PAGE_ROWS);
                        cx.notify();
                    }
                },
            ));
        }
        let list = list(
            self.list.clone(),
            cx.processor(
                |this, ix: usize, _window, cx| match this.view_rows.get(ix).cloned() {
                    Some(row) => this.row_element(&row, cx),
                    None => div().into_any_element(),
                },
            ),
        )
        .size_full();
        column = column.child(
            div()
                .id("agent-transcript")
                .role(gpui::Role::List)
                .aria_label("Agent conversation")
                .flex_1()
                .min_h(px(80.))
                .child(list),
        );
        if self.hidden_newer > 0 || !self.older.is_empty() {
            column = column.child(self.button(
                "agent-jump-latest",
                if self.hidden_newer > 0 {
                    format!(
                        "Jump to latest · {} newer messages hidden",
                        self.hidden_newer
                    )
                } else {
                    "Jump to latest".to_owned()
                },
                true,
                Tone::Primary,
                cx,
                |this, _, cx| this.jump_to_latest(cx),
            ));
        }
        if !snapshot.queue.is_empty() {
            let mut queue = Self::section(&format!("Queued ({})", snapshot.queue.len()));
            for item in &snapshot.queue {
                let id = item.id;
                queue = queue.child(
                    div()
                        .flex()
                        .justify_between()
                        .items_center()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .child(div().text_xs().child(item.summary.clone()))
                                .children(item.scope.as_ref().map(|scope| {
                                    Self::line(format!(
                                        "{scope}{}",
                                        if item.stale_scope {
                                            " · stale — will be refused"
                                        } else {
                                            ""
                                        }
                                    ))
                                })),
                        )
                        .child(self.button(
                            format!("queue-remove-{id}"),
                            "Remove",
                            true,
                            Tone::Normal,
                            cx,
                            move |this, _, cx| {
                                if let Some(w) = this.workflow().cloned() {
                                    let r = w.cancel_queued(id);
                                    this.command(r, cx);
                                }
                            },
                        )),
                );
            }
            column = column.child(queue);
        }
        column
            .child(self.prompt_form(controls, cx))
            .into_any_element()
    }

    fn prompt_form(&mut self, controls: &Controls, cx: &mut Context<Self>) -> gpui::Div {
        let enabled = controls.submit.blocked.is_none();
        let label = controls.submit.label();
        let hint = controls.submit.blocked.clone();
        let entity = cx.entity();
        let mut buttons = div().flex().gap_1().items_center();
        buttons = buttons.child(self.button(
            "agent-send",
            label,
            enabled,
            Tone::Primary,
            cx,
            |this, _, cx| this.submit(cx),
        ));
        if controls.stop.is_some() {
            buttons = buttons.child(self.stop_button(cx));
        }
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .id("agent-prompt")
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                        let prompt = this.prompt.clone();
                        this.key_in_field(&prompt, event, true, window, cx);
                    }))
                    .child(
                        canvas(
                            move |bounds, _, cx| {
                                entity.update(cx, |panel, _| {
                                    panel.prompt_bounds = Some([
                                        f32::from(bounds.left()),
                                        f32::from(bounds.top()),
                                        f32::from(bounds.size.width),
                                        f32::from(bounds.size.height),
                                    ]);
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    )
                    .child(self.prompt.clone()),
            )
            .children(hint.map(Self::line))
            .child(buttons)
    }

    fn stop_button(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        self.button(
            "agent-stop",
            "Stop",
            true,
            Tone::Danger,
            cx,
            |this, _, cx| this.stop(cx),
        )
    }

    fn project_tab(
        &mut self,
        snapshot: Arc<WorkflowSnapshot>,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut column = div()
            .id("agent-project-tab")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .p_2();

        // Review policy: app-local, affects subsequent tasks only.
        let policy = snapshot.review_policy;
        let next = match policy {
            ReviewPolicy::AutoApply => ReviewPolicy::ManualReview,
            ReviewPolicy::ManualReview => ReviewPolicy::AutoApply,
        };
        let mut review = Self::section("Review")
            .child(div().text_xs().child(match policy {
                ReviewPolicy::AutoApply => "Validated changes are applied automatically.",
                ReviewPolicy::ManualReview => "Validated changes wait for your Apply or Discard.",
            }))
            .child(Self::line(WorkflowSnapshot::REVIEW_POLICY_NOTICE))
            .child(Self::line(
                "A change affects tasks started afterwards; a running task keeps the policy it started with.",
            ));
        if let Some(task) = snapshot.task.as_ref().filter(|t| !t.phase.is_terminal()) {
            review = review.child(Self::line(format!(
                "The running task uses: {}",
                match task.review_policy {
                    ReviewPolicy::AutoApply => "automatic Apply",
                    ReviewPolicy::ManualReview => "manual review",
                }
            )));
        }
        review = review.child(self.button(
            "agent-policy",
            match next {
                ReviewPolicy::AutoApply => "Apply automatically (later tasks)",
                ReviewPolicy::ManualReview => "Review before Apply (later tasks)",
            },
            true,
            Tone::Normal,
            cx,
            move |this, _, cx| {
                if let Some(w) = this.workflow().cloned() {
                    let r = w.set_review_policy(next);
                    this.command(r, cx);
                }
            },
        ));
        column = column.child(review);

        // Undo and the accepted-awaiting-preview state.
        let mut undo = Self::section("Undo");
        match &controls.undo {
            UndoControl::Enabled { summary, .. } => {
                undo = undo
                    .child(Self::line(format!("Newest agent edit: {summary}")))
                    .child(self.button(
                        "agent-undo",
                        "Undo last agent edit",
                        true,
                        Tone::Normal,
                        cx,
                        |this, _, cx| {
                            if let Some(w) = this.workflow().cloned() {
                                let r = w.undo(None);
                                this.command(r, cx);
                            }
                        },
                    ));
            }
            UndoControl::Disabled(reason) => {
                undo = undo.child(Self::line(reason.clone())).child(self.button(
                    "agent-undo",
                    "Undo last agent edit",
                    false,
                    Tone::Normal,
                    cx,
                    |_, _, _| (),
                ));
            }
        }
        if let Some(handoff) = &snapshot.handoff {
            undo = undo.child(div().text_xs().child(match &handoff.state {
                HandoffState::AwaitingPreview { reason } => format!(
                    "Accepted {} is awaiting its preview{}. The preview still shows the previous revision.",
                    &handoff.published[..handoff.published.len().min(12)],
                    reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()
                ),
                HandoffState::Adopted => format!(
                    "Accepted {} · the preview is switching to it.",
                    &handoff.published[..handoff.published.len().min(12)]
                ),
                HandoffState::Displayed => format!(
                    "Accepted {} · displayed in the preview.",
                    &handoff.published[..handoff.published.len().min(12)]
                ),
            }));
        }
        column = column.child(undo);

        // Recovery.
        let recovery = &snapshot.recovery;
        if !recovery.rolled_back.is_empty()
            || !recovery.unresolved_conflicts.is_empty()
            || recovery.suspended.is_some()
            || recovery.draft.as_ref().is_some_and(draft_needs_attention)
            || recovery.notice.is_some()
            || controls.export_draft
        {
            let mut section = Self::section("Recovery");
            if let Some(reason) = &recovery.suspended {
                section = section.child(div().text_xs().child(format!(
                    "Source changes are suspended until the project is reopened: {reason}"
                )));
            }
            if let Some(notice) = &recovery.notice {
                section = section.child(Self::line(notice.clone()));
            }
            for id in recovery.rolled_back.iter().take(5) {
                section = section.child(Self::line(format!(
                    "An unfinished edit ({}) was rolled back when the project opened; your files are as they were.",
                    &id[..id.len().min(8)]
                )));
            }
            for conflict in &recovery.unresolved_conflicts {
                section = section.child(self.conflict_block(conflict));
            }
            if let Some(draft) = recovery.draft.as_ref().filter(|d| draft_needs_attention(d)) {
                section = section.child(div().text_xs().child(match &draft.state {
                    _ if draft.interrupted_by_restart => {
                        "A task was interrupted when the app last closed. Its working copy was kept and is locked until you confirm nothing is writing there.".to_owned()
                    }
                    DraftState::UnsafeWriter { reason, .. } => format!(
                        "The agent's working copy is locked because a writer may still run: {reason}"
                    ),
                    DraftState::Retained { reason, .. } => format!(
                        "The last task's working copy was kept ({reason}). It is archived before the next task starts."
                    ),
                    _ => "The agent's working copy is retained.".to_owned(),
                }));
                section = section.child(Self::line(draft.draft.display().to_string()));
                if controls.acknowledge_writer_gone {
                    section = section
                        .child(Self::line(
                            "Confirm only if you are sure no agent process is still writing there.",
                        ))
                        .child(self.button(
                            "agent-ack-writer",
                            "Nothing is writing; unlock the working copy",
                            true,
                            Tone::Normal,
                            cx,
                            |this, _, cx| {
                                if let Some(w) = this.workflow().cloned() {
                                    let r = w.acknowledge_writer_gone();
                                    this.command(r, cx);
                                }
                            },
                        ));
                }
            }
            if controls.export_draft {
                section = section.child(self.button(
                    "agent-export-draft",
                    "Export the retained draft…",
                    true,
                    Tone::Normal,
                    cx,
                    |this, _, cx| this.export(Export::Draft, cx),
                ));
            }
            column = column.child(section);
        }

        // History.
        let mut history = Self::section(&format!(
            "Accepted agent edits ({})",
            snapshot.history.len()
        ));
        if snapshot.history.is_empty() {
            history = history.child(Self::line(
                "No agent edit has been accepted in this project yet.",
            ));
        }
        for entry in snapshot.history.iter().rev().take(10) {
            history = history.child(history_line(entry));
        }
        column = column.child(history);

        // Resources.
        let r = snapshot.resources;
        column = column.child(Self::line(format!(
            "Resident messages {}/{} · {} KiB of {} KiB · {} processes · {} tool grants · {} tool workers",
            r.resident_rows,
            r.max_resident_rows,
            r.resident_bytes / 1024,
            r.max_resident_bytes / 1024,
            r.owned_processes,
            r.broker_grants,
            r.tool_workers
        )));
        column.into_any_element()
    }

    fn setup_tab(
        &mut self,
        snapshot: Arc<WorkflowSnapshot>,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let configured = snapshot.adapter.provider.is_some();
        let g = guidance(&snapshot.adapter.readiness, configured);
        let mut column = div()
            .id("agent-setup-tab")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .p_2();
        column = column.child(
            Self::section(&g.headline).children(g.steps.iter().map(|s| Self::line(s.clone()))),
        );
        if let Some(error) = &self.settings_error {
            column = column.child(
                div()
                    .p_2()
                    .rounded_md()
                    .bg(rgb(DANGER))
                    .text_xs()
                    .child(error.clone()),
            );
        }
        let field = |this: &Self,
                     label: &'static str,
                     input: &Entity<TextInput>,
                     cx: &mut Context<Self>| {
            let _ = this;
            // The handler reads THIS field's composition state: an active IME composition
            // keeps Enter/Tab/Escape (candidate selection and cancellation) in the field.
            let field = input.clone();
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(Self::line(label))
                .child(
                    div()
                        .on_key_down(cx.listener(
                            move |this, event: &gpui::KeyDownEvent, window, cx| {
                                this.key_in_field(&field, event, false, window, cx);
                            },
                        ))
                        .child(input.clone()),
                )
        };
        let editable = controls.settings_editable;
        let mut form = Self::section("Adapter");
        form = form
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
            .child(Self::line(
                "Writer containment is not a setting: it is derived from the qualification ledger installed in the app data folder, and only for this exact adapter and platform.",
            ))
            .child(Self::line(
                self.containment.as_ref().map_or_else(
                    || "Writer containment: no adapter configured.".to_owned(),
                    Resolution::summary,
                ),
            ))
            .child(self.button(
                "agent-mcp-toggle",
                match self.mcp_choice {
                    McpChoice::Baseline => "Project tools: MCP and command line",
                    McpChoice::Unsupported => "Project tools: command line only",
                },
                editable,
                Tone::Normal,
                cx,
                |this, _, cx| {
                    this.mcp_choice = match this.mcp_choice {
                        McpChoice::Baseline => McpChoice::Unsupported,
                        McpChoice::Unsupported => McpChoice::Baseline,
                    };
                    cx.notify();
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(self.button(
                        "agent-save-adapter",
                        "Save",
                        editable && !self.saves.is_pending(),
                        Tone::Primary,
                        cx,
                        |this, _, cx| this.save_adapter(cx),
                    ))
                    .child(self.button(
                        "agent-check-adapter",
                        "Check adapter",
                        controls.check_adapter,
                        Tone::Normal,
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
            form = form.child(Self::line(
                "Settings can change after the running task finishes or is stopped.",
            ));
        }
        column = column.child(form);

        let mut status = Self::section("Status");
        status = status
            .child(Self::line(format!(
                "Adapter: {}",
                snapshot
                    .adapter
                    .executable
                    .clone()
                    .unwrap_or_else(|| "not configured".into())
            )))
            .child(Self::line(format!(
                "Writer process model: {}",
                snapshot
                    .adapter
                    .writer_ownership
                    .clone()
                    .unwrap_or_else(|| "n/a".into())
            )));
        if snapshot
            .adapter
            .writer_ownership
            .as_deref()
            .is_some_and(|label| label != "process-group-contained")
        {
            status = status.child(Self::line(
                "This adapter is not qualified for this exact launch and platform, so its candidates cannot be captured or applied until a passed writer-containment qualification of it is installed.",
            ));
        }
        if let AdapterReadiness::Checked { report, .. } = &snapshot.adapter.readiness
            && let Some(info) = &report.initialized
        {
            status = status.child(Self::line(format!(
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
                status = status.child(Self::line(format!(
                    "Not supported by this agent (prompts are text only): {}",
                    unsupported.join(", ")
                )));
            }
        }
        let mcp = &snapshot.mcp;
        status = status.child(Self::line(format!(
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

/// Only a locked or retained working copy needs the user's attention; one whose bytes
/// were published (or that a running task owns) is routine.
fn draft_needs_attention(draft: &crate::agent_workflow::DraftNotice) -> bool {
    matches!(
        draft.state,
        DraftState::UnsafeWriter { .. } | DraftState::Retained { .. }
    )
}

fn history_line(entry: &HistoryEntry) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .child(div().text_xs().child(format!(
            "{} · {} file{} · {}{}",
            entry.kind,
            entry.files,
            if entry.files == 1 { "" } else { "s" },
            &entry.published[..entry.published.len().min(12)],
            entry
                .undoes
                .as_ref()
                .map(|u| format!(" · undoes {}", &u[..u.len().min(8)]))
                .unwrap_or_default()
        )))
        .child(ConversationPanel::line(entry.summary.clone()))
}

impl Render for ConversationPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_evidence_images();
        for image in self.retired_evidence_images.drain(..) {
            let _ = window.drop_image(image);
        }
        let mut root = div()
            .id("agent-panel")
            .role(gpui::Role::Group)
            .aria_label("Agent")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_l_1()
            .border_color(rgb(BORDER))
            .text_color(rgb(TEXT));
        let Some(snapshot) = self.snapshot.clone() else {
            return root
                .p_4()
                .gap_2()
                .child("Agent")
                .child(Self::line(
                    "Open or create a project to work with an agent.",
                ))
                .child(Self::line(controls::NO_AGENT_LAUNCHED))
                .into_any_element();
        };
        let controls = derive_controls(&snapshot, self.prompt_is_empty(cx), self.sdk_ready);
        let g = guidance(
            &snapshot.adapter.readiness,
            snapshot.adapter.provider.is_some(),
        );
        let mut header = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .pt_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(div().child("Agent"))
                    .children(controls.stop.as_ref().map(|_| self.stop_button(cx))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(if g.usable { MUTED } else { ACCENT }))
                    .child(g.headline.clone()),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(self.tab_button(Tab::Chat, "Chat", cx))
                    .child(self.tab_button(Tab::Project, "Project", cx))
                    .child(self.tab_button(Tab::Setup, "Setup", cx)),
            );
        if let Some(notice) = self.notice.clone() {
            header = header.child(
                div()
                    .flex()
                    .justify_between()
                    .items_start()
                    .gap_2()
                    .p_1()
                    .rounded_md()
                    .bg(rgb(WARNING))
                    .child(div().text_xs().child(notice))
                    .child(self.button(
                        "agent-dismiss-notice",
                        "×",
                        true,
                        Tone::Normal,
                        cx,
                        |this, _, cx| {
                            this.notice = None;
                            cx.notify();
                        },
                    )),
            );
        }
        root = root.child(header);
        let body = match self.tab {
            Tab::Chat => self.chat_tab(snapshot, &controls, cx),
            Tab::Project => self.project_tab(snapshot, &controls, cx),
            Tab::Setup => self.setup_tab(snapshot, &controls, cx),
        };
        root.child(body).into_any_element()
    }
}
