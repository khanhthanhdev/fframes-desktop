mod agent_tools;
mod create;
mod engine_events;
mod export_panel;
mod fixture;
mod header;
mod home;
mod keymap;
mod layout;
mod left_column;
mod observation;
mod onboarding;
mod overflow;
mod player;
mod present;
mod props_edits;
mod review;
mod sections;
mod settings;
mod settings_dialog;
mod status_strip;
mod timeline_edits;
mod transport;

pub use keymap::bind_keys;
pub use settings::AgentTab;

use gpui::{AppContext, Context, Window};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use studio_engine::agent::AgentWorkflow;
use studio_engine::containment::OwnershipPolicy;
use studio_engine::engine::backend::StudioEnvironment;
use studio_engine::engine::backend::{Command, Presentation};
use studio_engine::engine::handle::EngineHandle;
use studio_engine::providers::host::{HostInbox, WorkflowWake};
use studio_engine::services::audio_service::{AudioService, OutputDevice, OutputHandle};
use studio_engine::services::build_service::BuildService;
use studio_engine::services::export_service::{ExportControl, ExportProgress};
use studio_engine::services::preview_coordinator::PreviewCoordinator;
use studio_engine::services::thumbnail_cache::ThumbnailKey;
use studio_project::ProjectId;
use studio_proto::CommandId;
use studio_theme::Theme;

use crate::agent::{AgentSetupPage, Attention, ConversationPanel, PanelEvent};
use crate::app_state::{AppEvent, AppState};
use crate::kit::{Button, Emphasis, Icon, IconButton, IconButtonSize, MenuState};
use crate::{
    canvas_view::CanvasViewState,
    frame_image::ImagePresentationManager,
    preset_panel::PresetPanel,
    props_panel::{PropsEvent, PropsPanel},
    slides_panel::{SlidesEvent, SlidesPanel},
    timeline::{TimelineControls, TimelineEvent, TimelineView},
};
use left_column::SidebarTab;

enum Picker {
    /// The folder of a new project from the template.
    Create(studio_project::ProjectTemplate),
    Restore,
    Open,
    Import,
    Asset,
    Sdk,
    SdkBundle,
    Locate(ProjectId),
    PresetImport,
    PresetCss,
    PresetExport(String),
    Export,
}

struct PreparedInstall {
    ready: Arc<studio_engine::ReadyPreview>,
    clock: studio_engine::PlaybackClock,
    audio: Option<OutputHandle>,
    audio_ready: bool,
}

pub struct StudioShell {
    /// The theme of the window, refreshed at the start of every render.
    theme: Theme,
    /// The running app's version, which SDKs must be compatible with.
    app_version: &'static str,
    engine: EngineHandle,
    app: gpui::Entity<AppState>,
    _app_subscription: gpui::Subscription,
    /// The environment's build service, read here so the UI thread never waits on the
    /// engine for it.
    builds: BuildService,
    closed: Arc<AtomicBool>,
    focus: gpui::FocusHandle,
    presentation: Presentation,
    /// A command is in flight or a native picker is open.
    busy: bool,
    /// The command in flight and whether it is a refresh (which keeps the error shown).
    in_flight: Option<(CommandId, bool)>,
    error: Option<String>,
    failed_open: Option<PathBuf>,
    /// The sidebar tab on show.
    sidebar_tab: SidebarTab,
    /// What waits in the agent panel, for the Agent tab's dot.
    agent_attention: Attention,
    recents_open: bool,
    /// The Project and Assets tabs' folder trees, per project, for this launch.
    explorers: left_column::Explorers,
    status_open: bool,
    preview: Option<Arc<PreviewCoordinator>>,
    preview_session: Option<studio_engine::OpenSession>,
    preview_state: studio_engine::PreviewState,
    displayed: Option<Arc<studio_engine::ReadyPreview>>,
    pending_ready: Option<PreparedInstall>,
    pending_frame: Option<studio_engine::PreviewFrame>,
    canvas: CanvasViewState,
    /// The preview surface's focus, which its keys need.
    preview_focus: gpui::FocusHandle,
    canvas_origin: (f64, f64),
    canvas_extent: (f64, f64),
    canvas_pan_position: Option<(f64, f64)>,
    images: ImagePresentationManager,
    preview_extent: (u32, u32),
    clear_images: bool,
    preview_gate_pending: bool,
    /// Woken by the preview coordinator when it has news.
    preview_wake: async_channel::Sender<()>,
    /// The thumbnails last asked of the coordinator; `None` asks again.
    thumbnails_sent: Option<Vec<ThumbnailKey>>,
    /// The timing generation last handed to the preview coordinator.
    timing_sent: Option<u64>,
    timeline: gpui::Entity<TimelineView>,
    _timeline_subscription: gpui::Subscription,
    /// The timeline's controls the transport and the header draw, as it last reported.
    timeline_controls: TimelineControls,
    /// Timeline and slide stop edits waiting for the engine, oldest first.
    timeline_queue: VecDeque<timeline_edits::DocEdit>,
    /// The timeline or slide stop edit the engine is running.
    timeline_in_flight: Option<(CommandId, timeline_edits::DocEdit)>,
    /// The props generation last handed to the preview coordinator.
    props_sent: Option<u64>,
    /// The selection the coordinator was last asked the props of.
    props_selection: Option<fframes_studio_protocol::EditorObjectIdentity>,
    /// Props edits waiting for the engine, oldest first.
    props_queue: VecDeque<studio_engine::props::PropsCommand>,
    /// The props edit the engine is running.
    props_in_flight: Option<(CommandId, studio_engine::props::PropsCommand)>,
    _activation_subscription: gpui::Subscription,
    transport: studio_engine::PlaybackClock,
    /// The one-shot wait for the next frame boundary while playing; replaced on every pump.
    playback_timer: Option<gpui::Task<()>>,
    /// Whether the stranded-acceptance watch is running.
    acceptance_watch: bool,
    audio: Arc<AudioService>,
    output: Option<OutputHandle>,
    audio_ready_epoch: Option<u64>,
    audio_note: String,
    output_device: OutputDevice,
    muted: bool,
    resume_install: bool,
    requested_clock_epoch: u64,
    presented_serial: u64,
    clock_start: Instant,
    resume_scrub: bool,
    qualifying: bool,
    button_bounds: HashMap<String, [f32; 4]>,
    player_width: f32,
    /// The workspace's height (the player and sidebar row, gap and timeline), measured at
    /// paint.
    editor_height: f32,
    /// The timeline height the grip asked for; `None` keeps the default.
    timeline_height: Option<f32>,
    /// The grip being dragged, with the pointer and the size at the press.
    resize: Option<layout::Resize>,
    /// The sidebar width its grip asked for; `None` keeps the default.
    sidebar_width: Option<f32>,
    /// Where the grip was last painted, `[left, top, width, height]`.
    grip_bounds: Option<[f32; 4]>,
    /// Where the sidebar's grip was last painted.
    agent_grip_bounds: Option<[f32; 4]>,
    /// The bounds each cached section was laid out in, for the qualification telemetry.
    section_bounds: HashMap<&'static str, [f32; 4]>,
    /// The window's viewport, measured each render.
    viewport: [f32; 2],
    /// Whether the settings dialog is open; the main window's render builds the dialog.
    settings_open: bool,
    /// The settings dialog over the main window, while it is open.
    settings_dialog: Option<gpui::Entity<settings_dialog::SettingsDialog>>,
    /// What gets the focus back once the settings dialog has closed.
    settings_return_focus: Option<gpui::FocusHandle>,
    settings_page: settings::SettingsPage,
    /// The settings page's scrolling, which a deep link moves to its target.
    settings_scroll: gpui::ScrollHandle,
    settings_writer: Arc<settings::SettingsWriter>,
    /// The first-run setup, while it shows in place of the workspace.
    onboarding: Option<onboarding::Onboarding>,
    /// Whether the saved setup choices were read (once the app's storage is known).
    setup_checked: bool,
    /// Whether the setup was finished or skipped (`None`: never recorded).
    onboarding_completed: Option<bool>,
    /// How agent permission requests are answered, for every project.
    permission_policy: studio_proto::PermissionPolicy,
    /// The gallery's saved rules for Settings → Agent → Permissions, in place of the open
    /// project's; `None` outside the gallery.
    approvals_fixture: Option<Vec<studio_engine::agent::ApprovalRule>>,
    /// Whether the launch check for a newer Studio is turned off.
    update_check_disabled: bool,
    /// The last check for a newer Studio and its answer.
    update: settings::UpdateCheck,
    /// The "update available" notice was closed for this launch.
    update_dismissed: bool,
    /// The appearance the cached header last rendered with, for the telemetry.
    header_appearance: std::cell::Cell<Option<studio_theme::Appearance>>,
    /// The canvas message's toast at the foot of the stage; the player renders it.
    canvas_toast: std::cell::RefCell<player::CanvasToast>,
    /// The Create panel is open with this template chosen.
    create_choice: Option<studio_project::ProjectTemplate>,
    overflow_open: bool,
    /// Which menu `overflow_open` and `overflow_menu` belong to: the transport's or Export's.
    overflow_kind: overflow::OverflowMenu,
    overflow_menu: MenuState,
    /// Where the overflow trigger was last painted; the menu opens against it.
    overflow_anchor: std::rc::Rc<std::cell::Cell<Option<gpui::Bounds<gpui::Pixels>>>>,
    overflow_focus: gpui::FocusHandle,
    painted_frame: Option<usize>,
    /// The presentation under way (Present mode).
    presenting: Option<present::PresentState>,
    /// The full screen state the shell asked the window for.
    fullscreen: present::Fullscreen,
    /// What the next render restores after a presentation ended.
    after_present: Option<present::AfterPresent>,
    /// The window's section views, created on the first render.
    sections: Option<sections::Sections>,
    /// The start screen's search field and layout.
    home: home::Home,
    /// Set while the accepted source is ahead of what the preview shows: a handoff that
    /// failed (or has not finished) keeps the old preview playing under this label.
    awaiting_preview: Option<String>,
    panel: gpui::Entity<ConversationPanel>,
    /// The settings frame's Agent page, drawn by `panel`.
    setup_page: gpui::Entity<AgentSetupPage>,
    /// Settings → Agent → MCP servers and Tools.
    agent_tools: gpui::Entity<agent_tools::AgentToolsPage>,
    _agent_tools_subscription: gpui::Subscription,
    _panel_subscription: gpui::Subscription,
    preset_panel: gpui::Entity<PresetPanel>,
    _preset_subscription: gpui::Subscription,
    props_panel: gpui::Entity<PropsPanel>,
    _props_subscription: gpui::Subscription,
    slides_panel: gpui::Entity<SlidesPanel>,
    _slides_subscription: gpui::Subscription,
    agent: AgentHost,
    /// A promotion changed the source: take a fresh presentation (without cancelling the
    /// staged preview that was just adopted for exactly that source).
    refresh_wanted: bool,
    presentation_refresh_pending: bool,
    /// When the accepted source started waiting for a preview that nothing is preparing.
    awaiting_since: Option<Instant>,
    export_status: Option<ExportProgress>,
    export_panel: export_panel::ExportPanelState,
    review: review::ReviewState,
    export_cancelled: Option<Arc<ExportControl>>,
    export_processes: Option<studio_bootstrap::ProcessTreeManager>,
}

/// The shell's side of the per-project agent workflow: which project it belongs to, how
/// it was opened and what must still be reported to it. Workflow threads reach the UI
/// through [`HostInbox`] (open results, hand-offs), which the frame loop drains, and
/// through the engine's `WorkflowChanged` events (published snapshots).
struct AgentHost {
    inbox: Arc<HostInbox>,
    workflow: Option<Arc<AgentWorkflow>>,
    /// The attached workflow's snapshot wake, taken before each poll so the next publish
    /// signals again.
    wake: Option<Arc<WorkflowWake>>,
    session: Option<studio_engine::OpenSession>,
    /// Serial of the open in flight, if any; a result with another serial is closed.
    opening: Option<u64>,
    /// Serial of the workflow currently attached (hand-offs carry it).
    attached: Option<u64>,
    serial: u64,
    /// The SDK directory the workflow's build settings were last set to.
    applied_sdk: Option<PathBuf>,
    playhead: usize,
    /// A revision the preview now displays, still to be reported.
    displayed: Option<String>,
    /// Who may say a writer is contained: the containment verified at each launch,
    /// except for the explicit test injection of the studio's observation entry.
    ownership: OwnershipPolicy,
}

impl AgentHost {
    fn new(ownership: OwnershipPolicy) -> Self {
        Self {
            inbox: Arc::new(HostInbox::default()),
            workflow: None,
            wake: None,
            session: None,
            opening: None,
            attached: None,
            serial: 0,
            applied_sdk: None,
            playhead: 0,
            displayed: None,
            ownership,
        }
    }
}

impl StudioShell {
    /// `app_version` is the running app's version, which SDKs must be compatible with.
    pub fn new(app_version: &'static str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_environment(StudioEnvironment::system(), app_version, window, cx)
    }

    pub fn with_environment(
        environment: StudioEnvironment,
        app_version: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle().tab_stop(false);
        window.focus(&focus, cx);
        let timeline = cx.new(TimelineView::new);
        let timeline_subscription =
            cx.subscribe(&timeline, |shell, _, event, cx| match event.clone() {
                TimelineEvent::Seek(frame) => shell.seek_preview(frame, cx),
                TimelineEvent::BeginScrub => {
                    shell.resume_scrub = shell.transport.playing() || shell.resume_install;
                    shell.resume_install = false;
                    shell.pause_playback(cx);
                }
                TimelineEvent::EndScrub => {
                    if shell.resume_scrub
                        && !shell.transport.playing()
                        && shell
                            .displayed
                            .as_ref()
                            .is_some_and(|d| shell.transport.position() < d.timeline.total_frames)
                    {
                        shell.toggle_playback(cx);
                    }
                    shell.resume_scrub = false;
                }
                TimelineEvent::Edit(command) => shell.timeline_edit(command, cx),
                TimelineEvent::Stop(command) => shell.slides_edit(command, cx),
                TimelineEvent::ThumbnailsWanted => shell.request_thumbnails(cx),
                TimelineEvent::Controls(controls) => shell.timeline_controls_changed(controls, cx),
                TimelineEvent::ScopeChanged(scope) => {
                    let scope = scope.map(|scope| scope.map(|scope| *scope));
                    shell.install_task_scope(scope, cx);
                }
            });
        // A drag the pointer left the window during never sees its release.
        let activation_subscription = cx.observe_window_activation(window, |shell, window, cx| {
            if !window.is_window_active() {
                shell
                    .timeline
                    .update(cx, |timeline, cx| timeline.end_edit_drag(cx));
                shell.props_panel.update(cx, |panel, cx| panel.end_drag(cx));
            }
        });
        let panel = cx.new(|cx| ConversationPanel::new(window, cx));
        let panel_subscription =
            cx.subscribe_in(&panel, window, |shell, _, event, window, cx| match event {
                PanelEvent::Leave => window.focus(&shell.focus, cx),
                PanelEvent::OpenSettings(link) => {
                    shell.open_agent_settings(link.tab, link.target.clone(), window, cx)
                }
                // The composer's Tools popover and approval selector save through the same
                // paths as Settings → Agent, so both stay in step.
                // The page saves and announces the change (AgentToolsChanged).
                PanelEvent::SetAgentTools(settings) => shell
                    .agent_tools
                    .update(cx, |page, cx| page.set_from_composer(settings, cx)),
                PanelEvent::SetPermissionPolicy(policy) => {
                    shell.choose_permission_policy(*policy, cx)
                }
                PanelEvent::Attention(attention) => {
                    // A new permission card brings the panel forward; one already waiting
                    // does not keep the user from other tabs.
                    if attention.permissions > shell.agent_attention.permissions {
                        shell.sidebar_tab = SidebarTab::Agent;
                    }
                    shell.agent_attention = *attention;
                    cx.notify();
                }
            });
        let review = review::ReviewState::new(&panel, cx);
        let setup_page = cx.new(|cx| AgentSetupPage::new(&panel, cx));
        let settings_scroll = gpui::ScrollHandle::new();
        let agent_tools = cx
            .new(|_| agent_tools::AgentToolsPage::new(Default::default(), settings_scroll.clone()));
        let agent_tools_subscription = cx.subscribe(&agent_tools, |shell, _, event, cx| {
            let agent_tools::AgentToolsChanged(settings) = event;
            shell.apply_agent_tools(settings.clone(), cx);
        });
        // The composer's Tools popover shows what the agent gets (the open project's
        // override included), after every load or change.
        cx.observe(&agent_tools, |shell, page, cx| {
            let settings = page.read(cx).effective();
            shell
                .panel
                .update(cx, |panel, cx| panel.set_agent_tools(settings, cx));
        })
        .detach();
        let preset_panel = cx.new(|cx| PresetPanel::new(window, cx));
        let preset_subscription =
            cx.subscribe_in(&preset_panel, window, |shell, _, event, window, cx| {
                shell.preset_event(event.clone(), window, cx)
            });
        let props_panel = cx.new(|cx| PropsPanel::new(window, cx));
        let props_subscription = cx.subscribe_in(
            &props_panel,
            window,
            |shell, _, event, window, cx| match event {
                PropsEvent::Run(command) => shell.props_edit(command.clone(), cx),
                PropsEvent::Leave => window.focus(&shell.focus, cx),
            },
        );
        let slides_panel = cx.new(|cx| SlidesPanel::new(window, cx));
        let slides_subscription = cx.subscribe_in(
            &slides_panel,
            window,
            |shell, _, event, window, cx| match event {
                SlidesEvent::Run(command) => shell.slides_edit(command.clone(), cx),
                SlidesEvent::ScanStops => {
                    if let Some(preview) = &shell.preview {
                        preview.scan_stops();
                    }
                    cx.notify();
                }
                SlidesEvent::CancelScan => {
                    if let Some(preview) = &shell.preview {
                        preview.cancel_stop_scan();
                    }
                    cx.notify();
                }
                SlidesEvent::Seek(frame) => shell.seek_preview(*frame, cx),
                SlidesEvent::AddAtPlayhead => shell
                    .timeline
                    .update(cx, |timeline, cx| timeline.add_stop_at_playhead(cx)),
                SlidesEvent::PresentFrom(frame) => shell.present(Some(*frame), cx),
                SlidesEvent::Leave => window.focus(&shell.focus, cx),
            },
        );
        let (engine, events) = EngineHandle::new(environment.clone(), app_version);
        let app = cx.new(|cx| AppState::new(engine.clone(), events, cx));
        let app_subscription = cx.subscribe(&app, |shell, _, event: &AppEvent, cx| {
            if let Some(event) = event.take() {
                shell.engine_event(event, cx);
            }
        });
        let closed = engine.closed().clone();
        let owner = engine.clone();
        let shutdown = closed.clone();
        let clock_start = Instant::now();
        let audio = Arc::new(AudioService::new(clock_start));
        let quit_audio = audio.clone();
        let builds = environment.builds.clone();
        cx.on_app_quit(move |shell, cx| {
            shutdown.store(true, Ordering::Release);
            shell.cancel_export(cx);
            quit_audio.shutdown();
            let audio = quit_audio.clone();
            let preview = shell.preview.take();
            // The workflow reaps its task, broker, compilers and workers before the
            // controller they use is closed.
            let workflow = shell.agent.workflow.take();
            let displayed = shell.displayed.take();
            let pending = shell.pending_ready.take();
            shell.output = None;
            let owner = owner.clone();
            let builds = builds.clone();
            cx.background_executor().spawn(async move {
                if let Some(workflow) = workflow {
                    workflow.close();
                }
                owner.processes().shutdown(Duration::ZERO);
                // Compiles are owned by the build service, not the app process scope.
                builds.close();
                audio.join();
                if let Some(preview) = preview {
                    preview.close();
                    drop(preview);
                }
                drop((displayed, pending));
                // A hash/copy may still own the serialized queue. Do not wait
                // unboundedly on it during quit; durable jobs recover as interrupted.
                owner.shutdown(Duration::from_millis(50));
            })
        })
        .detach();
        let agent = AgentHost::new(environment.ownership.clone());
        let (agent_wake, agent_wakes) = async_channel::bounded(1);
        agent.inbox.set_listener(move || {
            let _ = agent_wake.try_send(());
        });
        let inbox = agent.inbox.clone();
        panel.update(cx, |panel, _| panel.set_wake(move || inbox.wake()));
        cx.spawn(async move |this, cx| {
            while agent_wakes.recv().await.is_ok() {
                if this.update(cx, |shell, cx| shell.agent_woken(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let (audio_wake, audio_wakes) = async_channel::bounded(1);
        let (preview_wake, preview_wakes) = async_channel::bounded(1);
        cx.spawn(async move |this, cx| {
            while preview_wakes.recv().await.is_ok() {
                if this
                    .update(cx, |shell, cx| shell.drain_preview(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let mut shell = Self {
            theme: Theme::for_window(window, cx),
            app_version,
            engine,
            app,
            _app_subscription: app_subscription,
            builds: environment.builds.clone(),
            closed,
            focus,
            presentation: Presentation::default(),
            busy: false,
            in_flight: None,
            error: None,
            failed_open: None,
            sidebar_tab: SidebarTab::Agent,
            agent_attention: Attention::default(),
            recents_open: true,
            explorers: left_column::Explorers::default(),
            status_open: false,
            preview: None,
            preview_session: None,
            preview_state: studio_engine::PreviewState::default(),
            displayed: None,
            pending_ready: None,
            pending_frame: None,
            canvas: CanvasViewState::default(),
            preview_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            canvas_origin: (0.0, 0.0),
            canvas_extent: (0.0, 0.0),
            canvas_pan_position: None,
            images: ImagePresentationManager::new(),
            preview_extent: (1280, 720),
            clear_images: false,
            preview_gate_pending: false,
            preview_wake,
            thumbnails_sent: None,
            timing_sent: None,
            timeline,
            _timeline_subscription: timeline_subscription,
            timeline_controls: TimelineControls::default(),
            timeline_queue: VecDeque::new(),
            timeline_in_flight: None,
            props_sent: None,
            props_selection: None,
            props_queue: VecDeque::new(),
            props_in_flight: None,
            _activation_subscription: activation_subscription,
            transport: studio_engine::PlaybackClock::install(30, 0, 0, 0.)
                .expect("valid empty clock"),
            audio,
            output: None,
            audio_ready_epoch: None,
            audio_note: "Audio output not prepared".into(),
            awaiting_preview: None,
            panel,
            setup_page,
            agent_tools,
            _agent_tools_subscription: agent_tools_subscription,
            _panel_subscription: panel_subscription,
            preset_panel,
            _preset_subscription: preset_subscription,
            props_panel,
            _props_subscription: props_subscription,
            slides_panel,
            _slides_subscription: slides_subscription,
            agent,
            refresh_wanted: false,
            presentation_refresh_pending: false,
            awaiting_since: None,
            export_status: None,
            export_panel: Default::default(),
            review,
            export_cancelled: None,
            export_processes: None,
            output_device: OutputDevice::Default,
            muted: false,
            resume_install: false,
            requested_clock_epoch: 0,
            presented_serial: 0,
            clock_start,
            playback_timer: None,
            acceptance_watch: false,
            resume_scrub: false,
            qualifying: false,
            button_bounds: HashMap::new(),
            player_width: f32::INFINITY,
            editor_height: 0.,
            timeline_height: None,
            resize: None,
            sidebar_width: None,
            grip_bounds: None,
            agent_grip_bounds: None,
            section_bounds: HashMap::new(),
            viewport: [0., 0.],
            settings_open: false,
            settings_dialog: None,
            settings_return_focus: None,
            settings_page: settings::SettingsPage::default(),
            settings_scroll,
            settings_writer: Arc::default(),
            onboarding: None,
            setup_checked: false,
            onboarding_completed: None,
            permission_policy: studio_proto::PermissionPolicy::default(),
            approvals_fixture: None,
            update_check_disabled: false,
            update: settings::UpdateCheck::NotChecked,
            update_dismissed: false,
            header_appearance: std::cell::Cell::new(None),
            canvas_toast: Default::default(),
            create_choice: None,
            overflow_open: false,
            overflow_kind: overflow::OverflowMenu::Transport,
            overflow_menu: MenuState::new(gpui::Point::default(), &[]),
            overflow_anchor: Default::default(),
            overflow_focus: cx.focus_handle(),
            painted_frame: None,
            presenting: None,
            fullscreen: present::Fullscreen::default(),
            after_present: None,
            sections: None,
            home: home::Home::new(window, cx),
        };
        shell.dispatch(Command::Initialize, cx);
        shell.audio.set_wake(move || {
            let _ = audio_wake.try_send(());
        });
        cx.spawn(async move |this, cx| {
            while audio_wakes.recv().await.is_ok() {
                if this
                    .update(cx, |shell, cx| shell.pump_playback(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        shell
    }
}

impl StudioShell {
    /// An editor button wired to a shell action, with the qualification bounds probe.
    /// `primary` paints it as the view's main action.
    #[allow(clippy::too_many_arguments)]
    fn action_button(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        primary: bool,
        glyph: Option<Icon>,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Button {
        let theme = self.theme;
        let entity = cx.entity();
        let probe = self.bounds_probe(&gpui::ElementId::Name(id.into()), cx);
        let mut button = Button::new(&theme, id)
            .label(label)
            .disabled(!enabled)
            .on_click(move |_, _, cx| {
                entity.update(cx, |shell, cx| action(shell, cx));
            });
        button = if primary {
            button.emphasis(Emphasis::Primary)
        } else {
            button.emphasis(Emphasis::Neutral)
        };
        if let Some(glyph) = glyph {
            button = button.left_icon(glyph.path());
        }
        if let Some(probe) = probe {
            button = button.on_bounds(probe.into_bounds_handler());
        }
        button
    }

    /// An icon button wired to a shell action, with the qualification bounds probe.
    /// The label is the accessible name and the tooltip.
    #[allow(clippy::too_many_arguments)]
    fn action_icon_button(
        &self,
        id: &'static str,
        label: &'static str,
        glyph: Icon,
        size: IconButtonSize,
        enabled: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> IconButton {
        let theme = self.theme;
        let entity = cx.entity();
        let probe = self.bounds_probe(&gpui::ElementId::Name(id.into()), cx);
        let button = IconButton::new(&theme, id, glyph.path())
            .size(size)
            .label(label)
            .tooltip(label)
            .disabled(!enabled)
            .on_click(move |_, _, cx| {
                entity.update(cx, |shell, cx| action(shell, cx));
            });
        match probe {
            Some(probe) => button.on_bounds(probe.into_bounds_handler()),
            None => button,
        }
    }
}
