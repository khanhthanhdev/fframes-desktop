//! The native agent panel: readiness, a virtualized conversation, review, Undo and
//! recovery, and the settings frame's Agent page ([`AgentSetupPage`]). It renders immutable [`WorkflowSnapshot`]s and sends commands; it
//! never touches the filesystem, the adapter, hashing or compilation (the workflow's
//! threads do), and it never holds a lock across a frame.
//!
//! * Rows live in a `gpui::list` (visible rows plus [`transcript::scroll::OVERDRAW_PX`] of overscan are
//!   measured and painted). [`rows`] decides which rows the list owns and how a new
//!   snapshot becomes list operations, so streaming text and tool-card updates re-measure
//!   in place while the reader's position holds.
//! * [`controls`] decides which controls exist in which state (pure, unit-tested).
//! * [`studio_engine::providers::host`] holds the app-local adapter description and the workflow inbox.
//! * [`provider_setup`] selects providers, detects and installs adapters and turns them on.
mod attachment_flow;
mod cards;
pub mod changes;
mod chat_tab;
mod commands;
pub mod composer;
mod composer_host;
pub mod controls;
pub mod history;
mod history_host;
pub mod markdown;
mod project_tab;
pub mod provider_setup;
mod question_card;
mod review_entry;
mod setup_page;
pub mod syntax;
pub mod todo_panel;
pub mod transcript;
pub mod ui;
mod widgets;

pub use setup_page::AgentSetupPage;

use transcript::rows;

/// Redacted observation of an evidence set: state, revision and frame labels only.
fn evidence_telemetry(view: &studio_engine::agent::EvidenceView) -> serde_json::Value {
    serde_json::json!({
        "state": format!("{:?}", view.state),
        "revision": view.revision,
        "artifacts": view.artifacts.iter().map(|a| serde_json::json!({
            "label": a.label, "frames": a.frames, "bytes": a.bytes, "sha256": a.sha256[..12.min(a.sha256.len())],
        })).collect::<Vec<_>>(),
    })
}

/// Redacted observation of a transcript row: its kind and size, never its text.
fn row_telemetry(kind: &RowKind) -> serde_json::Value {
    use serde_json::json;
    match kind {
        RowKind::User { text, .. } => json!({"kind": "user", "chars": text.chars().count()}),
        RowKind::Agent {
            text, streaming, ..
        } => json!({"kind": "agent", "chars": text.chars().count(), "streaming": streaming}),
        RowKind::Thought { .. } => json!({"kind": "thought"}),
        RowKind::Tool(_) => json!({"kind": "tool"}),
        RowKind::Permission(card) => {
            json!({"kind": "permission", "state": format!("{:?}", card.state)})
        }
        RowKind::Notice { .. } => json!({"kind": "notice"}),
        RowKind::Error(error) => json!({"kind": "error", "code": error.code}),
        RowKind::Validation(_) => json!({"kind": "validation"}),
        RowKind::Changes(_) => json!({"kind": "changes"}),
        RowKind::Outcome(card) => json!({"kind": "outcome", "outcome": format!("{:?}", card.kind)}),
    }
}

/// One line per evidence set: its state, the immutable revision it was rendered from and
/// the actual frames rendered. Never predicted pixels.
fn evidence_summary(name: &str, view: &studio_engine::agent::EvidenceView) -> String {
    use studio_engine::agent::EvidenceState;
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

use controls::{
    Controls, InputKey, KeyMods, ReplyTracker, ReviewControls, SaveTracker, SubmitKind,
    UndoControl, derive_controls, guidance, route_input_key,
};
use gpui::{
    AnyElement, AppContext, Context, ElementId, Entity, EventEmitter, FocusHandle, FollowMode,
    InteractiveElement, IntoElement, ListAlignment, ListState, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, list, px,
};
use parking_lot::Mutex;
use rows::{ListOp, ResidentLimits, RowKey, Viewport};
use std::{
    cell::Cell,
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use studio_engine::agent::LaunchContainment;
use studio_engine::agent::{
    AdapterReadiness, AgentWorkflow, ChangeCard, ConflictView, HandoffState, NoticeLevel,
    PermissionAnswer, PermissionCard, PermissionRef, PermissionState, Row, RowKind,
    StructuredError, TaskPhase, ToolCard, UndoView, UserSource, ValidationCard, WorkflowSnapshot,
};
use studio_engine::containment::OwnershipPolicy;
use studio_engine::providers::host::{self, AdapterFields, AdapterFile, McpChoice};
use studio_engine::providers::provider_profiles;
use studio_engine::{DraftState, ReviewPolicy, TaskScope, app_paths::AppPaths};
use studio_harness::AgentWire;
use studio_harness::driver::{ConfigKind, OptionValue, ToolStatus};
use studio_theme::{METRICS, TextStyle, Theme, text_style};
use transcript::scroll::{self, OVERDRAW_PX};

use crate::kit::{
    Badge, Button, Card, Chip, Emphasis, EmptyState, FormField, Icon, IconButton, IconButtonSize,
    Menu, MenuItem, MenuState, Notice, SegmentOption, SegmentedControl, Select, StatusDot,
    TextInput, TextInputEvent, TextInputState, Toggle, Tone as UiTone, label, selectable,
};
use crate::probe::BoundsProbe;

/// The least the transcript is given, however many cards wait above the composer.
const TRANSCRIPT_MIN_H: f32 = 140.;
/// The least while a question or permission card waits: the card needs the room more.
const TRANSCRIPT_MIN_ANSWERING_H: f32 = 72.;
/// The before and after evidence thumbnails in the task details.
const EVIDENCE_THUMB_W: f32 = 128.;
const EVIDENCE_THUMB_H: f32 = 72.;

/// Events the shell reacts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEvent {
    /// Escape in an input: focus returns to the shell.
    Leave,
    /// The settings button: the shell opens the settings frame's Agent page.
    OpenSetup,
    /// What waits for the user changed; the sidebar marks its Agent tab with it.
    Attention(Attention),
}

/// What waits for the user in the panel, for the sidebar's Agent tab while another tab is
/// selected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Attention {
    /// Permission cards that wait for an answer.
    pub permissions: usize,
    /// A permission card or a question waits, or a turn is running.
    pub waiting: bool,
}

impl Attention {
    fn of(snapshot: Option<&WorkflowSnapshot>) -> Self {
        let Some(snapshot) = snapshot else {
            return Self::default();
        };
        let permissions = snapshot.resources.open_permissions;
        let running = snapshot.composer.turn_live
            || snapshot.task.as_ref().is_some_and(|t| t.phase.stoppable());
        Self {
            permissions,
            waiting: permissions > 0 || !snapshot.composer.questions.is_empty() || running,
        }
    }
}

impl EventEmitter<PanelEvent> for ConversationPanel {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Chat,
    History,
    Project,
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
    /// The saved adapter description (what the Agent settings page starts with).
    pub adapter: Option<AdapterFile>,
    /// The app-local provider registry; defaults are shown when no registry exists yet.
    pub registry: Option<provider_profiles::ProviderRegistry>,
    pub settings_error: Option<String>,
    pub policy: OwnershipPolicy,
}

/// (attachment serial, chat generation at request time, the page).
type PageResult = (u64, u64, Result<Vec<Arc<Row>>, String>);

/// The open menu of a select-type agent option: the rows it shows and, row for row, the
/// values they commit. Both are built together, so a click commits the value whose name the
/// reader saw even when the agent changed its options since.
struct ConfigMenu {
    items: Vec<MenuItem>,
    state: MenuState,
    values: Vec<String>,
}

impl ConfigMenu {
    /// A menu listing the values of the select-type option `id`, the current one checked.
    /// It opens at the window origin until [`MenuState::anchored`] places it at its trigger.
    fn of(config: &[studio_harness::driver::ConfigOption], id: &str) -> Option<Self> {
        let option = config.iter().find(|o| o.id == id)?;
        let ConfigKind::Select { current, values } = &option.kind else {
            return None;
        };
        let items: Vec<MenuItem> = values
            .iter()
            .map(|v| MenuItem::new(v.name.clone(), &v.value == current))
            .collect();
        Some(Self {
            state: MenuState::new(gpui::Point::default(), &items),
            items,
            values: values.iter().map(|v| v.value.clone()).collect(),
        })
    }

    /// Takes the rows of `fresh` when they changed (`items` carries the check mark, so a new
    /// current value counts), keeping the place the menu opened at. Unchanged rows keep
    /// everything, including a highlight moved with the keys.
    fn refresh(&mut self, mut fresh: ConfigMenu) {
        if self.values != fresh.values || self.items != fresh.items {
            fresh.state.origin = self.state.origin;
            fresh.state.trigger_bounds = self.state.trigger_bounds;
            *self = fresh;
        }
    }
}

#[derive(Default)]
struct PageSlot {
    done: Mutex<Option<PageResult>>,
    loading: AtomicBool,
    /// Tells the shell a page finished.
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

struct SetupForm {
    provider: Entity<TextInputState>,
    executable: Entity<TextInputState>,
    args: Entity<TextInputState>,
    env_names: Entity<TextInputState>,
}

pub struct ConversationPanel {
    attached: Option<Attached>,
    snapshot: Option<Arc<WorkflowSnapshot>>,
    /// The attention last reported to the shell.
    attention: Attention,
    tab: Tab,
    /// The composer, the plan list and the session list.
    host: composer_host::ComposerHost,
    /// Refreshed at the top of every render.
    theme: Theme,
    prompt_kind: Option<SubmitKind>,
    setup: SetupForm,
    saved_adapter: Option<AdapterFile>,
    registry: provider_profiles::ProviderRegistry,
    /// The writer containment each provider (by its adapter label) established at its
    /// last launch while the studio runs; never stored.
    launches: HashMap<String, LaunchContainment>,
    settings_error: Option<String>,
    /// Who may say a writer is contained (production: the containment verified at launch).
    policy: OwnershipPolicy,
    saves: SaveTracker,
    mcp_choice: McpChoice,
    /// The protocol chosen on the setup page; loaded with the form's fields.
    wire_choice: AgentWire,
    /// Detect agents, Install adapter and the turn-on of a Select or Install click.
    agent_setup: provider_setup::AgentSetup,
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
    /// How permission requests are answered (shown in the header).
    permission_policy: studio_proto::PermissionPolicy,
    task_scope: Option<TaskScope>,
    scope_error: Option<String>,
    /// The status strip shows the task's ids, repairs, scope and evidence.
    task_details_open: bool,
    /// Measured bounds of the buttons a native driver must click (qualification only).
    button_bounds: std::collections::HashMap<String, [f32; 4]>,
    /// Current-task thumbnails are local presentation state, never workflow snapshot data.
    /// Each entry keeps the workflow's decoded thumbnail it was converted from.
    evidence_images: HashMap<String, (Arc<image::RgbaImage>, Arc<gpui::RenderImage>)>,
    /// Images removed from the current task; released from GPUI on the next render.
    retired_evidence_images: Vec<Arc<gpui::RenderImage>>,
    /// Reasoning rows the reader expanded (presentation state only).
    thoughts_open: std::collections::HashSet<u64>,
    /// Long user messages the reader expanded.
    messages_open: std::collections::HashSet<u64>,
    /// The runs of activity among the shown rows, and the runs the reader opened or closed
    /// by hand (by the run's first row id).
    activity: Vec<cards::ActivitySpan>,
    activity_open: HashMap<u64, bool>,
    /// The reader is far enough from the newest rows for the jump pill.
    far_from_end: Rc<Cell<bool>>,
    /// The workflow's chat generation the shown rows belong to.
    chat: u64,
    /// One menu per select-type agent option, kept across frames so each trigger
    /// reports its own bounds.
    config_menus: HashMap<String, ConfigMenu>,
    /// The option whose menu is open.
    open_config: Option<String>,
    config_focus: FocusHandle,
    /// The open project's folder; links in agent text to files inside it reveal them. Set on
    /// every workflow open and left alone by `detach`, which clears the rows that could link.
    project_root: Option<std::path::PathBuf>,
}

/// A single-line field. Escape in any field hands the keyboard back to the shell.
fn input(
    placeholder: &str,
    window: &mut Window,
    cx: &mut Context<ConversationPanel>,
) -> Entity<TextInputState> {
    let placeholder = placeholder.to_owned();
    let input = cx.new(|cx| {
        let mut input = TextInputState::single_line(window, cx);
        input.set_placeholder(placeholder);
        input
    });
    cx.subscribe(&input, |_, _, event: &TextInputEvent, cx| {
        if matches!(event, TextInputEvent::Cancelled) {
            cx.emit(PanelEvent::Leave);
        }
    })
    .detach();
    input
}

impl ConversationPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(OVERDRAW_PX));
        list.set_follow_mode(FollowMode::Tail);
        let viewport = Rc::new(Cell::new(None));
        let seen = viewport.clone();
        let far_from_end = Rc::new(Cell::new(false));
        let panel = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            seen.set(Some(event.visible_range.start));
            // Scrolled towards the top: page in earlier history. The jump pill shows or
            // hides when the reader crosses its distance from the newest rows.
            let panel = panel.clone();
            cx.defer(move |cx| {
                let _ = panel.update(cx, |panel, cx| {
                    panel.maybe_request_page();
                    let far = scroll::distance_from_bottom(&panel.list) > scroll::JUMP_DISTANCE_PX;
                    if far != panel.far_from_end.get() {
                        cx.notify();
                    }
                });
            });
        });
        // The composer and the session list report their parts' bounds into the same map
        // the panel's own buttons use.
        let weak = cx.entity().downgrade();
        let sink: crate::probe::BoundsSink = Rc::new(move |name, bounds, cx| {
            let _ = weak.update(cx, |panel, _| {
                panel.button_bounds.insert(name.to_owned(), bounds);
            });
        });
        let theme = Theme::for_window(window, cx);
        Self {
            attached: None,
            snapshot: None,
            attention: Attention::default(),
            tab: Tab::Chat,
            host: composer_host::ComposerHost::new(window, theme, sink, cx),
            theme,
            prompt_kind: None,
            setup: SetupForm {
                provider: input("Label, for example my-adapter", window, cx),
                executable: input("Absolute path of the ACP adapter", window, cx),
                args: input("Arguments, for example --acp", window, cx),
                env_names: input("e.g. ANTHROPIC_API_KEY", window, cx),
            },
            saved_adapter: None,
            registry: provider_profiles::ProviderRegistry::default(),
            launches: HashMap::new(),
            settings_error: None,
            policy: OwnershipPolicy::Validated,
            saves: SaveTracker::default(),
            mcp_choice: McpChoice::Baseline,
            wire_choice: AgentWire::Acp,
            agent_setup: provider_setup::AgentSetup::default(),
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
            permission_policy: studio_proto::PermissionPolicy::default(),
            task_scope: None,
            scope_error: None,
            task_details_open: false,
            button_bounds: std::collections::HashMap::new(),
            evidence_images: HashMap::new(),
            retired_evidence_images: Vec::new(),
            thoughts_open: std::collections::HashSet::new(),
            messages_open: std::collections::HashSet::new(),
            activity: Vec::new(),
            activity_open: HashMap::new(),
            far_from_end,
            chat: 0,
            config_menus: HashMap::new(),
            open_config: None,
            config_focus: cx.focus_handle(),
            project_root: None,
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
            registry,
            settings_error,
            policy,
        } = attachment;
        self.detach(cx);
        self.attached = Some(Attached {
            workflow,
            paths,
            serial,
        });
        self.mcp_choice = adapter.as_ref().map_or(McpChoice::Baseline, |a| a.mcp);
        self.registry = registry.unwrap_or_default();
        self.fill_setup(adapter.as_ref(), cx);
        self.saved_adapter = adapter;
        self.settings_error = settings_error;
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
        self.reset_transcript();
        self.open_config = None;
        self.notice = None;
        self.prompt_kind = None;
        self.clear_composer(cx);
        self.saves.cancel();
        // Stops a running install and drops every in-flight scan, install and turn-on.
        self.agent_setup.reset();
        self.report_attention(cx);
        cx.notify();
    }

    /// Tells the shell when what waits for the user changed.
    fn report_attention(&mut self, cx: &mut Context<Self>) {
        let attention = Attention::of(self.snapshot.as_deref());
        if attention != self.attention {
            self.attention = attention;
            cx.emit(PanelEvent::Attention(attention));
        }
    }

    /// Forgets every shown and paged-in row and the state keyed by their ids.
    fn reset_transcript(&mut self) {
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
        self.thoughts_open.clear();
        self.messages_open.clear();
        self.activity.clear();
        self.activity_open.clear();
    }

    pub fn workflow(&self) -> Option<&Arc<AgentWorkflow>> {
        self.attached.as_ref().map(|a| &a.workflow)
    }

    /// The project folder that file links in agent text resolve against; `None` makes them
    /// plain text.
    pub fn set_project_root(&mut self, root: Option<std::path::PathBuf>, cx: &mut Context<Self>) {
        if self.project_root != root {
            self.project_root = root;
            cx.notify();
        }
    }

    pub fn attached_serial(&self) -> Option<u64> {
        self.attached.as_ref().map(|a| a.serial)
    }

    pub fn set_permission_policy(
        &mut self,
        policy: studio_proto::PermissionPolicy,
        cx: &mut Context<Self>,
    ) {
        if self.permission_policy != policy {
            self.permission_policy = policy;
            cx.notify();
        }
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
        let (task_scope, scope_error) = match scope {
            Ok(scope) => (scope, None),
            Err(error) => (None, Some(error)),
        };
        // Set again with every presented frame; only a change renders the panel.
        if self.task_scope != task_scope || self.scope_error != scope_error {
            // The selection rides with the next message, so it shows as a chip in the composer.
            let chip = match (&scope_error, &task_scope) {
                (Some(error), _) => Some(composer::ContextChip {
                    label: format!("Selection unavailable: {error}").into(),
                    problem: true,
                }),
                (None, Some(scope)) => Some(composer::ContextChip {
                    label: scope.label().into(),
                    problem: false,
                }),
                (None, None) => None,
            };
            self.host
                .composer
                .update(cx, |composer, cx| composer.set_context(chip, cx));
            self.task_scope = task_scope;
            self.scope_error = scope_error;
            cx.notify();
        }
    }

    /// True while the prompt (or a setup field) holds keyboard focus.
    pub fn has_text_focus(&self, window: &Window, cx: &gpui::App) -> bool {
        use gpui::Focusable;
        let focused = |input: &Entity<TextInputState>| input.focus_handle(cx).is_focused(window);
        self.composer_has_focus(window, cx)
            || focused(&self.setup.provider)
            || focused(&self.setup.executable)
            || focused(&self.setup.args)
            || focused(&self.setup.env_names)
    }

    /// Redacted observation of the panel for the M3 telemetry file: counts and states,
    /// never prompt, brief or message text.
    pub fn telemetry(&self, cx: &gpui::App) -> serde_json::Value {
        let Some(snapshot) = &self.snapshot else {
            return serde_json::json!({"attached": false, "setup": self.setup_telemetry()});
        };
        // Unique `Arc<Row>` allocations the panel itself holds (paged history plus what it
        // lays out); the workflow's own live window is shared with it and bounded by the
        // workflow, so it is reported separately instead of being counted twice.
        let (retained_rows, retained_bytes) =
            rows::unique_retained(&[&self.older, &self.view_rows]);
        let bytes: usize = self.view_rows.iter().map(|r| r.estimated_bytes()).sum();
        serde_json::json!({
            "attached": true,
            "tab": format!("{:?}", self.tab),
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
            "scroll": {
                "following": self.list.is_following_tail(),
                "distance_from_bottom": scroll::distance_from_bottom(&self.list),
            },
            "open_permissions": snapshot.resources.open_permissions,
            "owned_processes": snapshot.resources.owned_processes,
            "broker_grants": snapshot.resources.broker_grants,
            "handoff": snapshot.handoff.as_ref().map(|h| format!("{:?}", h.state)),
            "buttons": self.button_bounds,
            "prompt_bounds": self.button_bounds.get("composer-input"),
            "composer": self.composer_telemetry(cx),
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
            "setup": self.setup_telemetry(),
            "apply_blocked": snapshot.task.as_ref().and_then(|t| t.review.as_ref()).and_then(|r| r.apply_blocked.clone()),
            "rows": snapshot.rows.iter().skip(snapshot.rows.len().saturating_sub(64)).map(|row| row_telemetry(&row.kind)).collect::<Vec<_>>(),
        })
    }

    /// Called on a workflow thread when a history page finished; the shell then calls
    /// [`Self::poll`].
    pub fn set_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        *self.page.wake.lock() = Some(Arc::new(wake));
    }

    /// Takes the newest snapshot and finished history pages. Cheap; the shell calls it
    /// whenever the workflow changed or a page finished. Returns whether anything changed.
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
            if snapshot.chat != self.chat {
                self.chat = snapshot.chat;
                self.reset_transcript();
                self.reset_composer_state(cx);
            }
            if let (Some(provider), Some(launch)) =
                (&snapshot.adapter.provider, &snapshot.adapter.last_launch)
            {
                self.launches.insert(provider.clone(), launch.clone());
            }
            self.snapshot = Some(snapshot);
            self.after_snapshot(cx);
            self.report_attention(cx);
            changed = true;
        }
        changed |= self.sync_evidence_images();
        let finished = self.page.done.lock().take();
        if let Some((page_serial, page_chat, result)) = finished {
            self.page.loading.store(false, Ordering::Release);
            if page_serial == serial && page_chat == self.chat {
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
        self.sync_composer(cx);
        let controls = derive_controls(&snapshot, self.prompt_is_empty(cx), self.sdk_ready);
        if self.prompt_kind.as_ref() != Some(&controls.submit.kind) {
            let placeholder = match controls.submit.kind {
                SubmitKind::Brief => "Describe the change you want",
                SubmitKind::Queue => "Queue a follow-up; it runs after the current task",
                SubmitKind::Clarify(_) => "Answer the agent's question",
            };
            self.host
                .composer
                .update(cx, |composer, cx| composer.set_placeholder(placeholder, cx));
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
            if let Some((_, image)) = self.evidence_images.remove(&key) {
                self.retired_evidence_images.push(image);
                changed = true;
            }
        }
        for (key, thumbnail) in desired {
            if self
                .evidence_images
                .get(&key)
                .is_none_or(|(current, _)| !Arc::ptr_eq(current, &thumbnail))
            {
                let image = Arc::new(gpui::RenderImage::new(smallvec::SmallVec::from_const([
                    image::Frame::new(thumbnail.as_ref().clone()),
                ])));
                self.evidence_images.insert(key, (thumbnail, image));
                changed = true;
            }
        }
        changed
    }
}

/// The buttons whose bounds native qualification reads.
fn probed_button(id: &ElementId) -> Option<&str> {
    match id {
        ElementId::Name(name)
            if matches!(
                name.as_ref(),
                "agent-apply"
                    | "agent-undo"
                    | "agent-review-candidate"
                    | "agent-review-banner"
                    | "agent-tab-Chat"
                    | "agent-tab-History"
                    | "agent-tab-Project"
                    | "agent-stop"
                    | "agent-held-cancel"
                    | "agent-refused-restore"
                    | "agent-refused-dismiss"
                    | "agent-stage-confirm"
                    | "agent-stage-cancel"
                    | "agent-open-setup"
                    | "agent-save-adapter"
                    | "setup-Label"
            ) || name.starts_with("question-") =>
        {
            Some(name.as_ref())
        }
        _ => None,
    }
}

/// The header's badge for the permission policy: allowing everything is a warning.
fn policy_badge(policy: studio_proto::PermissionPolicy) -> Badge {
    use studio_proto::PermissionPolicy::*;
    match policy {
        AskEveryTime => Badge::new("Asks every time"),
        AllowInWorkingCopy => Badge::new("Auto: working copy").tone(UiTone::Info),
        AllowEverything => Badge::new("Auto: everything").tone(UiTone::Warning),
    }
}

fn banner_tone(phase: TaskPhase) -> UiTone {
    match phase {
        TaskPhase::Accepted => UiTone::Success,
        TaskPhase::Failed | TaskPhase::Conflict => UiTone::Danger,
        TaskPhase::AwaitingReview
        | TaskPhase::AwaitingEvidence
        | TaskPhase::WaitingPermission
        | TaskPhase::WaitingClarification
        | TaskPhase::Interrupted => UiTone::Warning,
        _ => UiTone::Info,
    }
}

/// The verb and glyph of a tool call in the activity log, from the ACP tool kind the
/// drivers report (`read`, `edit`, `search`, `execute`, ...).
fn tool_verb(kind: Option<&str>) -> (&'static str, Icon) {
    match kind {
        Some("read") => ("Read", Icon::Eye),
        Some("edit") => ("Edit", Icon::File),
        Some("delete") => ("Delete", Icon::Trash),
        Some("move") => ("Move", Icon::File),
        Some("search") => ("Search", Icon::Search),
        Some("execute") => ("Run", Icon::Hammer),
        Some("fetch") => ("Fetch", Icon::Download),
        Some("think") => ("Think", Icon::Bot),
        Some("switch_mode") => ("Mode", Icon::Settings),
        _ => ("Tool", Icon::Package),
    }
}

/// Only a locked or retained working copy needs the user's attention; one whose bytes
/// were published (or that a running task owns) is routine.
fn draft_needs_attention(draft: &studio_engine::agent::DraftNotice) -> bool {
    matches!(
        draft.state,
        DraftState::UnsafeWriter { .. } | DraftState::Retained { .. }
    )
}

fn line(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    text_style(div(), TextStyle::Small)
        .text_color(theme.colors.text_2)
        .child(selectable(text))
}

/// `Label` style text the reader can select and copy.
fn label_text(text: impl Into<SharedString>) -> gpui::Div {
    text_style(div(), TextStyle::Label).child(selectable(text))
}

impl Render for ConversationPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.theme.refresh(window, cx) {
            let theme = self.theme;
            self.host
                .composer
                .update(cx, |composer, cx| composer.set_theme(theme, cx));
        }
        self.sync_questions(window, cx);
        self.sync_evidence_images();
        for image in self.retired_evidence_images.drain(..) {
            let _ = window.drop_image(image);
        }
        let theme = self.theme;
        let root = div()
            .id("agent-panel")
            .role(gpui::Role::Group)
            .aria_label("Agent")
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme.colors.text_1);
        let Some(snapshot) = self.snapshot.clone() else {
            return root
                .items_center()
                .justify_center()
                .p(px(METRICS.space_4))
                .child(ui::panel_empty(
                    &theme,
                    TextStyle::Heading,
                    "Agent",
                    "Open or create a project to work with an agent.",
                ))
                .into_any_element();
        };
        let controls = derive_controls(&snapshot, self.prompt_is_empty(cx), self.sdk_ready);
        self.sync_config_menu();
        let open_setup = self.icon_button(
            "agent-open-setup",
            Icon::Settings,
            "Agent settings",
            Emphasis::Ghost,
            cx,
            |_, _, cx| cx.emit(PanelEvent::OpenSetup),
        );
        let new_chat = self
            .icon_button(
                "agent-new-chat",
                Icon::Plus,
                "New chat",
                Emphasis::Ghost,
                cx,
                |this, _, cx| {
                    if let Some(w) = this.workflow().cloned() {
                        let r = w.new_chat();
                        this.command(r, cx);
                    }
                    this.tab = Tab::Chat;
                    cx.notify();
                },
            )
            .disabled(!controls.new_chat);
        // The sidebar's Agent tab names the panel, so the header holds the sub-tabs and the
        // panel's own buttons.
        let header = ui::PanelHeader::new(&theme, self.tabs(cx))
            .action(new_chat)
            .action(open_setup);
        let notice = self.notice.clone().map(|notice| {
            let entity = cx.entity();
            div()
                .flex_none()
                .px(px(METRICS.space_3))
                .pt(px(METRICS.space_2))
                .child(
                    Notice::new("agent-notice", UiTone::Warning, notice).on_dismiss(
                        move |_, cx| {
                            entity.update(cx, |this, cx| {
                                this.notice = None;
                                cx.notify();
                            });
                        },
                    ),
                )
        });
        let body = match self.tab {
            Tab::Chat => self.chat_tab(snapshot, &controls, cx),
            Tab::History => self.history_tab(),
            Tab::Project => self.project_tab(snapshot, &controls, cx),
        };
        root.child(header)
            .children(notice)
            .child(body)
            .children(self.render_config_menu(cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;

    #[test]
    fn only_the_listed_buttons_report_bounds() {
        assert_eq!(probed_button(&"agent-apply".into()), Some("agent-apply"));
        assert_eq!(probed_button(&"agent-policy".into()), None);
        assert_eq!(probed_button(&"agent-undo".into()), Some("agent-undo"));
        assert_eq!(
            probed_button(&"agent-tab-Project".into()),
            Some("agent-tab-Project")
        );
        assert_eq!(
            probed_button(&"agent-open-setup".into()),
            Some("agent-open-setup")
        );
        assert_eq!(probed_button(&"setup-Label".into()), Some("setup-Label"));
        assert_eq!(
            probed_button(&"question-0-1-opt-2".into()),
            Some("question-0-1-opt-2")
        );
        assert_eq!(probed_button(&"agent-tab-Setup".into()), None);
        assert_eq!(probed_button(&"agent-discard".into()), None);
        assert_eq!(
            probed_button(&ElementId::from(("agent-apply", 1usize))),
            None
        );
    }

    #[test]
    fn a_config_menu_lists_the_values_and_commits_the_chosen_one() {
        use studio_harness::driver::{ConfigOption, ConfigValue};
        let value = |value: &str, name: &str| ConfigValue {
            value: value.into(),
            name: name.into(),
            group: None,
        };
        let config = vec![
            ConfigOption {
                id: "model".into(),
                name: "Model".into(),
                description: None,
                kind: ConfigKind::Select {
                    current: "sonnet".into(),
                    values: vec![value("opus", "Opus"), value("sonnet", "Sonnet")],
                },
            },
            ConfigOption {
                id: "fast".into(),
                name: "Fast".into(),
                description: None,
                kind: ConfigKind::Boolean { current: false },
            },
        ];
        let menu = ConfigMenu::of(&config, "model").unwrap();
        let items: Vec<&str> = menu.items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(items, ["Opus", "Sonnet"]);
        // The current value carries the check mark and starts highlighted.
        let checked: Vec<bool> = menu.items.iter().map(|i| i.checked).collect();
        assert_eq!(checked, [false, true]);
        assert_eq!(menu.state.highlighted, Some(1));
        assert_eq!(menu.values, ["opus", "sonnet"]);
        assert!(ConfigMenu::of(&config, "fast").is_none());
        assert!(ConfigMenu::of(&config, "gone").is_none());
    }

    #[test]
    fn a_refreshed_config_menu_stays_where_it_opened() {
        use studio_harness::driver::{ConfigOption, ConfigValue};
        let config = |current: &str| {
            vec![ConfigOption {
                id: "model".into(),
                name: "Model".into(),
                description: None,
                kind: ConfigKind::Select {
                    current: current.into(),
                    values: [("opus", "Opus"), ("sonnet", "Sonnet")]
                        .iter()
                        .map(|(value, name)| ConfigValue {
                            value: (*value).into(),
                            name: (*name).into(),
                            group: None,
                        })
                        .collect(),
                },
            }]
        };
        let trigger = gpui::Bounds::new(
            gpui::point(px(40.), px(300.)),
            gpui::size(px(160.), px(30.)),
        );
        let mut open = ConfigMenu::of(&config("opus"), "model").unwrap();
        open.state = MenuState::anchored(trigger, &open.items);
        // The agent switched the current value: the check mark moves, the menu stays put.
        open.refresh(ConfigMenu::of(&config("sonnet"), "model").unwrap());
        let checked: Vec<bool> = open.items.iter().map(|i| i.checked).collect();
        assert_eq!(checked, [false, true]);
        assert_eq!(open.state.trigger_bounds, Some(trigger));
        assert_eq!(open.state.origin, trigger.bottom_left());
        // The same options again: nothing is rebuilt, so a keyboard highlight survives.
        open.state.set_highlighted(0);
        open.refresh(ConfigMenu::of(&config("sonnet"), "model").unwrap());
        assert_eq!(open.state.highlighted, Some(0));
        assert!(open.state.highlight_visible);
    }

    #[test]
    fn a_config_menu_commits_the_value_it_showed_after_the_options_change() {
        use studio_harness::driver::{ConfigOption, ConfigValue};
        let select = |values: &[(&str, &str)]| {
            vec![ConfigOption {
                id: "model".into(),
                name: "Model".into(),
                description: None,
                kind: ConfigKind::Select {
                    current: values[0].0.into(),
                    values: values
                        .iter()
                        .map(|(value, name)| ConfigValue {
                            value: (*value).into(),
                            name: (*name).into(),
                            group: None,
                        })
                        .collect(),
                },
            }]
        };
        let shown =
            ConfigMenu::of(&select(&[("opus", "Opus"), ("sonnet", "Sonnet")]), "model").unwrap();
        // The agent reorders its options between the paint and the click.
        let now =
            ConfigMenu::of(&select(&[("sonnet", "Sonnet"), ("opus", "Opus")]), "model").unwrap();
        // Each menu pairs its names with their values row for row, so the row the reader
        // clicked commits the value they saw.
        assert_eq!(shown.items[1].label.as_ref(), "Sonnet");
        assert_eq!(shown.values[1], "sonnet");
        assert_eq!(now.items[1].label.as_ref(), "Opus");
        assert_eq!(now.values[1], "opus");
    }

    #[test]
    fn banner_tone_follows_the_task_phase() {
        assert_eq!(banner_tone(TaskPhase::Accepted), UiTone::Success);
        assert_eq!(banner_tone(TaskPhase::Failed), UiTone::Danger);
        assert_eq!(banner_tone(TaskPhase::Conflict), UiTone::Danger);
        assert_eq!(banner_tone(TaskPhase::AwaitingReview), UiTone::Warning);
        assert_eq!(banner_tone(TaskPhase::WaitingPermission), UiTone::Warning);
    }

    #[test]
    fn outcomes_keep_their_titles_and_tones() {
        use studio_engine::agent::OutcomeKind;
        use ui::outcome_style;
        assert_eq!(
            outcome_style(OutcomeKind::Accepted),
            ("Accepted", UiTone::Success)
        );
        assert_eq!(
            outcome_style(OutcomeKind::Conflict),
            ("Conflict", UiTone::Danger)
        );
        assert_eq!(
            outcome_style(OutcomeKind::Cancelled),
            ("Stopped", UiTone::Neutral)
        );
        assert_eq!(
            outcome_style(OutcomeKind::AwaitingReview),
            ("In review", UiTone::Warning)
        );
    }
}
