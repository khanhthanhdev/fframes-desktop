//! Provider selection, Detect agents, Install adapter and turn-on: the Agent settings page's
//! "Providers" and "Detect agents" sections.
//!
//! * [`TurnOn`] is the one state a Select or Install adapter click moves through (save, then
//!   one connection test); a Detect row's status is derived from it ([`TurnOn::row_status`]).
//! * [`InstallTracker`] and [`ScanTracker`] hand out tickets: a result whose ticket is no
//!   longer current (the install was replaced, the panel left its project) is dropped.
//! * Detect agents and Install adapter never run at once, so a scan that started before an
//!   install finished can never overwrite the adapter it installed.
//! * [`AgentState`] is the one vocabulary every card's badge uses.
//! * Find more agents lists the ACP registry on request; an entry only fills in the Register
//!   a custom agent form or opens its page.
//!
//! The types are GPUI-free and unit-tested; the handlers below only wire them to the panel.
use super::{
    ConversationPanel, Emphasis,
    controls::{self, Controls, derive_controls},
};
use gpui::{Context, Entity, IntoElement, ParentElement, Styled, Window, div, px};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use studio_engine::agent::{AdapterReadiness, UndoView};
use studio_engine::app_paths::AppPaths;
use studio_engine::providers::{
    acp_registry::{self, RegistryAgent},
    host::{self, AdapterFile},
    provider_profiles::{self, DetectRowAction, DetectionScan, DetectionState},
};
use studio_harness::{
    NodeEntry,
    adapter_install::{self, InstallError, InstalledAdapter},
};
use studio_theme::{METRICS, TextStyle, Theme, text_style};

use super::ui::{AgentCard, AgentFact};
use crate::kit::{
    Badge, ButtonSize, EmptyState, Notice, Spinner, TextInput, TextInputState, Tone as UiTone,
    selectable, setting_card,
};

// ---- state -------------------------------------------------------------------------------------

/// Where a turn-on (Select on a Detect row or a provider, or a finished Install adapter)
/// stands. Every change of the configured adapter that is not this turn-on resets it, so a
/// row only ever shows the readiness of the adapter it turned on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TurnOn {
    #[default]
    Idle,
    /// The save with this ticket is in flight; the snapshot still shows the previous adapter.
    Saving { ticket: u64, profile: String },
    /// The save completed: the workflow's adapter is this profile's, and its readiness (the
    /// connection test) is shown on the profile's row.
    Testing { profile: String },
}

/// What a Detect row shows as its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStatus {
    /// What Detect agents found.
    Detected(DetectionState),
    /// The turn-on save is in flight.
    TurningOn,
    /// The row's adapter is the configured one: the workflow's readiness.
    Readiness,
}

impl TurnOn {
    /// The profile the in-flight save `ticket` turns on, if that save is a turn-on.
    pub fn profile_of(&self, ticket: u64) -> Option<&str> {
        match self {
            Self::Saving { ticket: t, profile } if *t == ticket => Some(profile),
            _ => None,
        }
    }

    /// The save `ticket` completed; `saved` says whether the adapter is now configured and
    /// written. Returns whether that save was a turn-on that may now run its one test.
    pub fn finish(&mut self, ticket: u64, saved: bool) -> bool {
        let Some(profile) = self.profile_of(ticket).map(str::to_owned) else {
            return false;
        };
        *self = if saved {
            Self::Testing { profile }
        } else {
            Self::Idle
        };
        saved
    }

    /// The configured adapter changed some other way (a Save, a provider switch, leaving
    /// the project): no row shows a readiness any more.
    pub fn reset(&mut self) {
        *self = Self::Idle;
    }

    pub fn row_status(&self, profile: &str, detected: DetectionState) -> RowStatus {
        match self {
            Self::Saving { profile: p, .. } if p == profile => RowStatus::TurningOn,
            Self::Testing { profile: p } if p == profile => RowStatus::Readiness,
            _ => RowStatus::Detected(detected),
        }
    }
}

/// At most one Detect agents scan runs; only the current scan's result is applied.
#[derive(Debug, Default)]
pub struct ScanTracker {
    running: Option<u64>,
    last: u64,
}

impl ScanTracker {
    /// Claims the scan slot; `None` while a scan runs.
    pub fn begin(&mut self) -> Option<u64> {
        if self.running.is_some() {
            return None;
        }
        self.last += 1;
        self.running = Some(self.last);
        Some(self.last)
    }

    /// Ends scan `ticket`; `false` when it is stale and its result must be dropped.
    pub fn finish(&mut self, ticket: u64) -> bool {
        let current = self.running == Some(ticket);
        if current {
            self.running = None;
        }
        current
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Forgets the running scan (the panel left its project).
    pub fn abandon(&mut self) {
        self.running = None;
    }
}

#[derive(Debug)]
struct Install {
    ticket: u64,
    profile: String,
    cancel: Arc<AtomicBool>,
}

/// At most one adapter install runs. Cancelling sets the flag that kills npm's process
/// tree; leaving the project cancels it and drops its result.
#[derive(Debug, Default)]
pub struct InstallTracker {
    current: Option<Install>,
    last: u64,
}

/// A finished install that is still current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallFinished {
    pub profile: String,
    /// Cancel was pressed, possibly after npm had already finished.
    pub cancelled: bool,
}

impl InstallTracker {
    /// Claims the install slot for `profile`: the ticket and the cancel flag to hand to the
    /// installer. `None` while an install runs.
    pub fn begin(&mut self, profile: &str) -> Option<(u64, Arc<AtomicBool>)> {
        if self.current.is_some() {
            return None;
        }
        self.last += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.current = Some(Install {
            ticket: self.last,
            profile: profile.to_owned(),
            cancel: cancel.clone(),
        });
        Some((self.last, cancel))
    }

    /// The profile whose adapter is installing.
    pub fn installing(&self) -> Option<&str> {
        self.current
            .as_ref()
            .map(|install| install.profile.as_str())
    }

    pub fn is_running(&self) -> bool {
        self.current.is_some()
    }

    /// Asks the running install to stop.
    pub fn cancel(&self) {
        if let Some(install) = &self.current {
            install.cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Ends install `ticket`; `None` when it is stale and nothing may be applied or reported.
    pub fn finish(&mut self, ticket: u64) -> Option<InstallFinished> {
        if self.current.as_ref()?.ticket != ticket {
            return None;
        }
        let install = self.current.take()?;
        Some(InstallFinished {
            profile: install.profile,
            cancelled: install.cancel.load(Ordering::SeqCst),
        })
    }

    /// Stops the running install and forgets it (the panel left its project).
    pub fn abandon(&mut self) {
        self.cancel();
        self.current = None;
    }
}

/// What the panel does with a finished install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Save the new adapter and test it once.
    TurnOn,
    /// Installed, but settings cannot change now: the user selects it later.
    Installed,
    /// Cancel was pressed. `installed` when npm had finished first: the adapter is on disk
    /// but is not turned on.
    Cancelled {
        installed: bool,
    },
    Failed(String),
}

impl InstallOutcome {
    /// `can_turn_on`: settings are editable and no other save is in flight.
    pub fn of(
        result: &Result<InstalledAdapter, InstallError>,
        cancelled: bool,
        can_turn_on: bool,
    ) -> Self {
        match result {
            _ if cancelled => Self::Cancelled {
                installed: result.is_ok(),
            },
            Err(InstallError::Cancelled) => Self::Cancelled { installed: false },
            Err(error) => Self::Failed(error.to_string()),
            Ok(_) if can_turn_on => Self::TurnOn,
            Ok(_) => Self::Installed,
        }
    }
}

/// The launch Select saves for a detected adapter file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectedLaunch {
    /// `node <entry>`.
    Node {
        node: std::path::PathBuf,
        entry: std::path::PathBuf,
    },
    /// The file itself (a native binary, or a launcher started with Node's folder first on
    /// its `PATH`).
    Direct(std::path::PathBuf),
}

/// How a detected adapter file starts. A script needs Node.js, which `node` names when
/// Detect agents found it; the error says what is missing.
pub fn detected_launch(
    adapter: &std::path::Path,
    node: Option<&std::path::Path>,
) -> Result<DetectedLaunch, String> {
    match studio_harness::node_entry(adapter)? {
        NodeEntry::Script(entry) => match node {
            Some(node) => Ok(DetectedLaunch::Node {
                node: node.to_owned(),
                entry,
            }),
            None => Err("This adapter needs Node.js to start, but node was not found.".into()),
        },
        NodeEntry::Native | NodeEntry::Shim => Ok(DetectedLaunch::Direct(adapter.to_owned())),
    }
}

/// The panel's Detect agents, Install adapter and turn-on state.
#[derive(Debug, Default)]
pub(super) struct AgentSetup {
    /// The last Detect agents result, one entry per profile.
    pub(super) detected: DetectionScan,
    scans: ScanTracker,
    install: InstallTracker,
    pub(super) turn_on: TurnOn,
    /// Profiles whose Install guide was opened since the last Detect agents.
    guides_opened: HashSet<String>,
    /// Find more agents: the ACP registry, fetched only when asked. Kept across projects.
    pub(super) registry_list: RegistryList,
    registry_search: Option<Entity<TextInputState>>,
    /// The Register a custom agent card is open.
    pub(super) custom_open: bool,
}

impl AgentSetup {
    /// Leaving the project: stops a running install and forgets every result.
    pub(super) fn reset(&mut self) {
        self.install.abandon();
        self.scans.abandon();
        self.turn_on.reset();
        self.detected = DetectionScan::default();
        self.guides_opened.clear();
    }
}

// ---- handlers ----------------------------------------------------------------------------------

impl ConversationPanel {
    pub(super) fn select_provider(&mut self, profile_id: &str, cx: &mut Context<Self>) {
        if self.attached.is_none() {
            return;
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.switch_pending.is_some())
        {
            self.say("Resolve or cancel the pending provider handoff first.", cx);
            return;
        }
        let Some(profile) = self
            .registry
            .profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .cloned()
        else {
            self.say("That provider profile is no longer available.", cx);
            return;
        };
        let busy = self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .task
                .as_ref()
                .is_some_and(|task| !task.phase.is_terminal())
                || matches!(snapshot.undo, UndoView::InProgress { .. })
        });
        if busy {
            // The switch replaces the adapter once the task stops; no row keeps showing
            // the outgoing adapter's readiness.
            self.agent_setup.turn_on.reset();
            if let Some(workflow) = self.workflow().cloned() {
                self.command(workflow.initiate_switch(&profile.id), cx);
            }
            return;
        }
        if self.registry.selected_profile_id.as_deref() == Some(profile_id) {
            self.say(format!("{} is already selected.", profile.label), cx);
            return;
        }

        let mut registry = self.registry.clone();
        if let Err(error) = registry.select_profile(profile_id) {
            self.say(error, cx);
            return;
        }
        let Some(attached) = &self.attached else {
            return;
        };
        let paths = attached.paths.clone();
        let ticket = match self.saves.begin() {
            Ok(ticket) => ticket,
            Err(refusal) => {
                self.say(refusal, cx);
                return;
            }
        };
        self.agent_setup.turn_on = TurnOn::Saving {
            ticket,
            profile: profile.id.clone(),
        };
        let adapter = profile.adapter.clone();
        let policy = self.policy.clone();
        let resolving = {
            let adapter = adapter.clone();
            cx.background_executor()
                .spawn(async move { adapter.resolve(&policy) })
        };
        cx.spawn(async move |this, cx| {
            let resolved = resolving.await;
            let _ = this.update(cx, |panel, cx| {
                panel.apply_selected_provider(ticket, registry, adapter, resolved, paths, cx)
            });
        })
        .detach();
        self.say(format!("Selecting {}…", profile.label), cx);
    }

    fn apply_selected_provider(
        &mut self,
        ticket: u64,
        registry: provider_profiles::ProviderRegistry,
        adapter: AdapterFile,
        resolved: studio_engine::agent::AdapterSettings,
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
        if let Err(error) = attached.workflow.set_adapter(Some(resolved)) {
            self.saves.finish(ticket);
            self.agent_setup.turn_on.finish(ticket, false);
            self.say(error.to_string(), cx);
            return;
        }
        self.registry = registry.clone();
        self.fill_setup(Some(&adapter), cx);
        self.mcp_choice = adapter.mcp;
        self.saved_adapter = Some(adapter);
        let save = cx
            .background_executor()
            .spawn(async move { host::save_registry(&paths, &registry) });
        cx.spawn(async move |this, cx| {
            let result = save.await;
            let _ = this.update(cx, |panel, cx| {
                if panel.saves.finish(ticket) {
                    panel.settings_error = result.as_ref().err().cloned();
                    let turn_on = panel.agent_setup.turn_on.finish(ticket, result.is_ok());
                    panel.notice = Some(match result {
                        Ok(()) if turn_on => panel.test_turned_on(
                            "Provider selected. Its writer containment is verified when it starts.",
                            cx,
                        ),
                        Ok(()) => "Provider selected. Its writer containment is verified when it starts.".into(),
                        Err(error) => format!("Provider selected for this session, but its registry could not be saved: {error}"),
                    });
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Runs the one connection test of a completed turn-on save, when the current snapshot
    /// allows Check adapter, and returns what to tell the user after `saved`.
    pub(super) fn test_turned_on(&mut self, saved: &str, cx: &mut Context<Self>) -> String {
        let (Some(workflow), Some(snapshot)) = (self.workflow().cloned(), self.snapshot.clone())
        else {
            return format!("{saved} Use Check adapter to test it.");
        };
        let controls = derive_controls(&snapshot, self.prompt_is_empty(cx), self.sdk_ready);
        if controls.check_adapter {
            match workflow.check_adapter() {
                Ok(()) => format!("{saved} Testing the connection…"),
                Err(error) => {
                    format!("{saved} The connection test could not start: {error}")
                }
            }
        } else if !controls.settings_editable {
            format!(
                "{saved} The connection test waits until the running task finishes; use Check adapter then."
            )
        } else {
            format!("{saved} Use Check adapter to test it once the current check finishes.")
        }
    }

    /// Looks for installed agents on a background thread. Detection only checks that files
    /// exist; nothing is started. Refused while an adapter installs.
    pub fn detect_agents(&mut self, cx: &mut Context<Self>) {
        let Some(attached) = &self.attached else {
            return;
        };
        if self.agent_setup.install.is_running() {
            self.say(
                "Wait for the adapter install to finish, then detect again.",
                cx,
            );
            return;
        }
        let Some(ticket) = self.agent_setup.scans.begin() else {
            return;
        };
        let registry = self.registry.clone();
        let adapters = attached.paths.data.join("adapters");
        let env = studio_harness::DetectionEnv::from_process(vec![adapters.clone()]);
        let task = cx
            .background_executor()
            .spawn(async move { provider_profiles::scan_agents(&registry, &env, &adapters) });
        cx.spawn(async move |this, cx| {
            let scan = task.await;
            let _ = this.update(cx, |panel, cx| {
                if panel.agent_setup.scans.finish(ticket) {
                    panel.agent_setup.detected = scan;
                    panel.agent_setup.guides_opened.clear();
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Fills the setup fields with a profile whose adapter was detected or installed, using
    /// the absolute path that was found, and saves them through the normal save path as a
    /// turn-on of that profile.
    pub fn use_detected(&mut self, profile_id: &str, cx: &mut Context<Self>) {
        let Some(profile) = self.registry.profiles.iter().find(|p| p.id == profile_id) else {
            self.say("That provider profile is no longer available.", cx);
            return;
        };
        let mut file = profile.launch_template();
        let detected = &self.agent_setup.detected;
        let launch = if let Some(installed) = detected.installed.get(profile_id) {
            // An adapter the app installed starts as `node <entry>`.
            Ok(DetectedLaunch::Node {
                node: installed.node.clone(),
                entry: installed.entry.clone(),
            })
        } else if let Some(path) = detected
            .found
            .iter()
            .find(|found| found.id == profile_id)
            .and_then(|found| found.adapter.as_deref())
        {
            detected_launch(path, detected.node.as_deref())
        } else {
            Err("That agent's adapter was not detected.".to_owned())
        };
        match launch {
            Ok(DetectedLaunch::Node { node, entry }) => {
                file.executable = node.to_string_lossy().into_owned();
                file.args.insert(0, entry.to_string_lossy().into_owned());
            }
            Ok(DetectedLaunch::Direct(path)) => {
                file.executable = path.to_string_lossy().into_owned();
            }
            Err(reason) => {
                self.say(reason, cx);
                return;
            }
        }
        self.mcp_choice = file.mcp;
        self.fill_setup(Some(&file), cx);
        self.save_adapter_with(Some((profile_id.to_owned(), &file)), cx);
    }

    /// Opens an install page in the default browser. Only `https://` links are handed over;
    /// anything else is refused and reported. Returns whether the link was opened.
    /// Redacted Detect agents and turn-on state: what each profile's scan found (the
    /// adapter's file name, never its folder) and the selected profile.
    pub(super) fn setup_telemetry(&self) -> serde_json::Value {
        let setup = &self.agent_setup;
        serde_json::json!({
            "scanning": setup.scans.is_running(),
            "detected": setup.detected.found.iter().map(|found| serde_json::json!({
                "profile": found.id,
                "state": setup.detected.state(found).label(),
                "adapter": found.adapter.as_ref().and_then(|path| path.file_name()).map(|name| name.to_string_lossy().into_owned()),
            })).collect::<Vec<_>>(),
            "turn_on": format!("{:?}", setup.turn_on),
            "selected_profile": self.registry.selected_profile_id,
        })
    }

    fn open_link(&mut self, url: &str, cx: &mut Context<Self>) -> bool {
        if !provider_profiles::is_openable(url) {
            self.say(
                format!("Refused to open {url:?}: only https:// links are opened."),
                cx,
            );
            return false;
        }
        cx.open_url(url);
        self.say(
            format!("Opened {url} in your browser. After installing, press Detect agents again."),
            cx,
        );
        true
    }

    /// Installs the pinned ACP adapter of a profile on a background thread, under the
    /// project's process scope, which closing the project shuts down (quitting closes the
    /// project when the shell can take its controller in time). Runs only from the Install
    /// adapter button and never during a Detect agents scan; Cancel install and leaving the
    /// project kill npm's process tree.
    fn install_adapter(&mut self, profile_id: &str, cx: &mut Context<Self>) {
        let (Some(attached), Some(package)) =
            (&self.attached, adapter_install::package_for(profile_id))
        else {
            return;
        };
        if self.agent_setup.scans.is_running() {
            self.say("Wait for Detect agents to finish, then install.", cx);
            return;
        }
        let Some(tools) = self.agent_setup.detected.tools() else {
            let missing = self.agent_setup.detected.missing_tools().join(" and ");
            self.say(format!("Install adapter needs {missing}."), cx);
            return;
        };
        let Some((ticket, cancel)) = self.agent_setup.install.begin(profile_id) else {
            return;
        };
        let adapters = attached.paths.data.join("adapters");
        let processes = attached.workflow.process_scope().sub_manager();
        let task = cx.background_executor().spawn(async move {
            adapter_install::install_adapter(
                &package,
                &adapters,
                &tools,
                &processes,
                &cancel,
                adapter_install::INSTALL_TIMEOUT,
            )
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |panel, cx| panel.finish_install(ticket, result, cx));
        })
        .detach();
        self.say(
            format!("Installing {} {}…", package.name, package.version),
            cx,
        );
    }

    fn finish_install(
        &mut self,
        ticket: u64,
        result: Result<InstalledAdapter, InstallError>,
        cx: &mut Context<Self>,
    ) {
        let Some(finished) = self.agent_setup.install.finish(ticket) else {
            return;
        };
        let editable = self.snapshot.as_ref().is_none_or(|snapshot| {
            derive_controls(snapshot, self.prompt_is_empty(cx), self.sdk_ready).settings_editable
        });
        let outcome = InstallOutcome::of(
            &result,
            finished.cancelled,
            editable && !self.saves.is_pending(),
        );
        if let Ok(installed) = result {
            // On disk either way, so its row shows it; only a turn-on configures it.
            self.agent_setup
                .detected
                .installed
                .insert(finished.profile.clone(), installed);
        }
        match outcome {
            // Saves the new adapter, then tests it once.
            InstallOutcome::TurnOn => self.use_detected(&finished.profile, cx),
            InstallOutcome::Installed => {
                self.say("Adapter installed. Select it to turn it on.", cx)
            }
            InstallOutcome::Cancelled { installed: true } => self.say(
                "Install cancelled after the adapter was already installed. Select it to turn it on.",
                cx,
            ),
            InstallOutcome::Cancelled { installed: false } => {
                self.say("Adapter install cancelled.", cx)
            }
            InstallOutcome::Failed(error) => {
                self.say(format!("Adapter install failed: {error}"), cx)
            }
        }
        cx.notify();
    }

    // ---- render ----------------------------------------------------------------------------------

    /// What the provider's last launch in this session proved about its writers.
    fn launch_status(&self, provider: &str) -> Option<String> {
        self.launches.get(provider).map(|launch| {
            if launch.contained {
                format!("Contained at its last launch ({}).", launch.ownership)
            } else {
                format!(
                    "Not contained at its last launch ({}): its changes are not captured.",
                    launch.ownership
                )
            }
        })
    }

    /// Settings → Agent → CLI agents: one card per registry profile, after Oleafly's agent
    /// cards, then Register a custom agent and Find more agents. A card's header says
    /// whether the agent is in use and its [`AgentState`]; its details show the command line
    /// tool, the adapter, the sign-in command and the one next step.
    /// `switch_pending`: a provider handoff waits; `selection_busy`: a task or Undo runs, so
    /// choosing another agent becomes a stopped-writer switch.
    pub(super) fn agents_section(
        &mut self,
        readiness: &AdapterReadiness,
        controls: &Controls,
        switch_pending: bool,
        selection_busy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = self.theme;
        let setup = &self.agent_setup;
        let installing = setup.install.is_running();
        let scanned = !setup.detected.found.is_empty();
        let check = self.action_button(
            "agent-detect",
            if setup.scans.is_running() {
                "Checking…"
            } else if scanned {
                "Check again"
            } else {
                "Check installed agents"
            },
            !setup.scans.is_running() && !installing,
            Emphasis::Neutral,
            cx,
            |this, _, cx| this.detect_agents(cx),
        );
        let intro = "Claude Code, Codex and other CLI agents edit your video in a private \
                     working copy. Each keeps its own sign-in, models and memory; Studio never \
                     stores an API key. Checking only looks for installed files and starts \
                     nothing.";
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .child(self.section_header("CLI AGENTS", intro, check.into_any_element()));
        let profiles = self.registry.profiles.clone();
        for profile in &profiles {
            section = section.child(self.agent_card(
                profile,
                readiness,
                controls.settings_editable,
                switch_pending,
                selection_busy,
                cx,
            ));
        }
        section = section.child(self.custom_agent_card(controls, cx));
        section.child(
            div()
                .pt(px(METRICS.space_4))
                .child(self.registry_section(&theme, cx)),
        )
    }

    /// A section's title (`Overline`), its one paragraph and the button on the right.
    fn section_header(
        &self,
        title: &'static str,
        intro: &'static str,
        action: gpui::AnyElement,
    ) -> gpui::Div {
        div()
            .flex()
            .items_start()
            .gap(px(METRICS.space_3))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(METRICS.space_1))
                    .child(
                        studio_theme::text_style(div(), studio_theme::TextStyle::Overline)
                            .text_color(self.theme.colors.text_2)
                            .child(title),
                    )
                    .child(self.line(intro)),
            )
            .child(action)
    }

    /// Register a custom agent: the custom adapter form folded into a card at the end of the
    /// agent list.
    fn custom_agent_card(
        &mut self,
        controls: &Controls,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let open = self.agent_setup.custom_open;
        let toggle = cx.entity();
        let mut card = AgentCard::new(
            &theme,
            CUSTOM_CARD_ID,
            "Register a custom agent",
            "Any ACP agent, or Claude Code or Codex at a path you choose.",
            ("", UiTone::Neutral),
        )
        .without_badge()
        .open(open)
        .on_toggle(move |_, app| {
            toggle.update(app, |this, cx| {
                this.agent_setup.custom_open = !this.agent_setup.custom_open;
                cx.notify();
            });
        });
        if open {
            card = card.content(self.custom_agent_form(controls, cx));
        }
        card.into_any_element()
    }

    /// One agent's card; see [`Self::agents_section`].
    fn agent_card(
        &mut self,
        profile: &provider_profiles::ProviderProfile,
        readiness: &AdapterReadiness,
        settings_editable: bool,
        switch_pending: bool,
        selection_busy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = self.theme;
        let setup = &self.agent_setup;
        let id = profile.id.clone();
        let selected = self.registry.selected_profile_id.as_deref() == Some(profile.id.as_str());
        let found = setup
            .detected
            .found
            .iter()
            .find(|found| found.id == profile.id);
        let detected = found.map(|found| setup.detected.state(found));
        let installed = setup.detected.installed.get(&profile.id);
        let installing_this = setup.install.installing() == Some(profile.id.as_str());
        let row_status = detected.map(|state| setup.turn_on.row_status(&profile.id, state));
        let acp = profile.adapter.wire == studio_harness::AgentWire::Acp;
        let installable = acp && adapter_install::package_for(&profile.id).is_some();
        let installing = setup.install.is_running();
        let state = AgentState::of(installing_this, row_status, selected, readiness, detected);
        let guide = controls::guidance(readiness, true);
        let sign_in = provider_profiles::sign_in_command(&profile.id);

        // The header's one line: what the badge does not already say.
        let summary = match state {
            AgentState::Installing => "Installing its adapter…".to_owned(),
            AgentState::Checking => "Testing the connection once…".to_owned(),
            AgentState::Ready if selected => guide.headline.strip_prefix("Ready · ").map_or_else(
                || format!("{} · new tasks use it", guide.headline),
                |rest| format!("{rest} · new tasks use it"),
            ),
            AgentState::Ready => "The connection check passed.".to_owned(),
            AgentState::NeedsSignIn => "Sign in with its own tool, then test it again.".to_owned(),
            AgentState::CheckFailed => format!("{}.", guide.headline),
            AgentState::Installed if selected => "New tasks use it. Not tested yet.".to_owned(),
            AgentState::Installed => "Installed on this computer.".to_owned(),
            AgentState::AdapterMissing => {
                "The command line tool is installed; its ACP adapter is not.".to_owned()
            }
            AgentState::NotInstalled => "Not found on this computer.".to_owned(),
            AgentState::NotChecked if selected => "New tasks use it. Not tested yet.".to_owned(),
            AgentState::NotChecked => profile.description.clone(),
        };

        // The details: what was found, where, and how it talks to Studio.
        let external =
            |path: &std::path::Path| studio_bootstrap::external_path(path).display().to_string();
        let not_checked = AgentState::NotChecked.label();
        // A path is monospaced; "Not found" and the like are not.
        let fact = |label: &'static str, path: Option<&std::path::Path>, missing: &str| match path {
            Some(path) => AgentFact::code(label, external(path)),
            None => AgentFact::text(label, missing.to_owned()),
        };
        let mut facts = vec![match found {
            Some(found) => fact("Command line tool", found.cli.as_deref(), "Not installed"),
            None => fact("Command line tool", None, not_checked),
        }];
        if acp {
            facts.push(match (installed, found) {
                (Some(installed), _) => fact("ACP adapter", Some(&installed.entry), ""),
                (None, Some(found)) => {
                    fact("ACP adapter", found.adapter.as_deref(), "Not installed")
                }
                (None, None) => fact("ACP adapter", None, not_checked),
            });
        }
        facts.push(AgentFact::text("Protocol", profile.adapter.wire.label()));
        facts.push(AgentFact::text(
            "Source",
            if installed.is_some() {
                "Installed by Studio"
            } else if profile.builtin {
                "Built in, found automatically"
            } else {
                "Added by you"
            },
        ));

        // The next step for this state and its actions. Ready and in use needs none.
        let mut actions: Vec<gpui::AnyElement> = Vec::new();
        let test_enabled = settings_editable
            && !matches!(
                readiness,
                AdapterReadiness::Checking | AdapterReadiness::NotConfigured
            );
        let test =
            |this: &Self, label: &'static str, emphasis: Emphasis, cx: &mut Context<Self>| {
                this.action_button(
                    format!("agent-check-{id}"),
                    label,
                    test_enabled,
                    emphasis,
                    cx,
                    |this, _, cx| {
                        if let Some(workflow) = this.workflow().cloned() {
                            let result = workflow.check_adapter();
                            this.command(result, cx);
                        }
                    },
                )
                .into_any_element()
            };
        let next_step: Option<String> = match state {
            AgentState::Installing => {
                actions.push(
                    self.action_button(
                        format!("agent-install-cancel-{id}"),
                        "Cancel install",
                        true,
                        Emphasis::Neutral,
                        cx,
                        |this, _, cx| {
                            this.agent_setup.install.cancel();
                            this.say("Cancelling the install…", cx);
                        },
                    )
                    .into_any_element(),
                );
                Some("Studio is installing the pinned adapter package.".into())
            }
            AgentState::Checking => {
                Some("Studio starts it once in a scratch folder, not in your project.".into())
            }
            AgentState::Ready if selected => {
                Some("Nothing to do. New tasks use this agent.".into())
            }
            AgentState::NeedsSignIn if selected => {
                if let Some(command) = sign_in {
                    actions.push(
                        self.action_button(
                            format!("agent-sign-in-{id}"),
                            "Copy sign-in command",
                            true,
                            Emphasis::Primary,
                            cx,
                            move |this, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    command.to_owned(),
                                ));
                                this.say(
                                    format!("Copied {command}. Run it in a terminal, then test the connection."),
                                    cx,
                                );
                            },
                        )
                        .into_any_element(),
                    );
                }
                let emphasis = if sign_in.is_some() {
                    Emphasis::Neutral
                } else {
                    Emphasis::Primary
                };
                actions.push(test(self, "Test connection", emphasis, cx));
                Some(match sign_in {
                    Some(_) => {
                        "Run the sign-in command in a terminal, then test the connection.".into()
                    }
                    None => "Sign in with the agent's own tool, then test the connection.".into(),
                })
            }
            AgentState::CheckFailed if selected => {
                actions.push(test(self, "Test again", Emphasis::Neutral, cx));
                Some(guide.steps.first().cloned().unwrap_or_default())
            }
            AgentState::Installed | AgentState::NotChecked if selected => {
                actions.push(test(self, "Test connection", Emphasis::Primary, cx));
                Some(
                    "Test it once: Studio starts it in a scratch folder and checks the sign-in."
                        .into(),
                )
            }
            _ => {
                let action = detected.map(|state| {
                    provider_profiles::row_action(
                        state,
                        installable,
                        setup.detected.tools().is_some(),
                        provider_profiles::install_guide_url(&profile.id),
                    )
                });
                let busy_switch = selection_busy && !switch_pending;
                match action {
                    Some(DetectRowAction::TurnOn) if !selection_busy => {
                        let select = id.clone();
                        actions.push(
                            self.action_button(
                                format!("agent-detected-{id}"),
                                "Use this agent",
                                settings_editable && !self.saves.is_pending() && !switch_pending,
                                Emphasis::Primary,
                                cx,
                                move |this, _, cx| this.use_detected(&select, cx),
                            )
                            .into_any_element(),
                        );
                        Some("Use it for new tasks: Studio saves it and tests it once.".into())
                    }
                    Some(DetectRowAction::InstallAdapter) => {
                        let install = id.clone();
                        actions.push(
                            self.action_button(
                                format!("agent-install-{id}"),
                                "Install adapter",
                                !installing && !setup.scans.is_running(),
                                Emphasis::Primary,
                                cx,
                                move |this, _, cx| this.install_adapter(&install, cx),
                            )
                            .into_any_element(),
                        );
                        Some(
                            "Install its ACP adapter, a small pinned package Studio keeps in its own folder. Then sign in with the command line tool."
                                .into(),
                        )
                    }
                    Some(DetectRowAction::GetNode) => {
                        actions.push(
                            self.action_button(
                                format!("agent-get-node-{id}"),
                                "Get Node.js",
                                true,
                                Emphasis::Neutral,
                                cx,
                                |this, _, cx| {
                                    this.open_link(provider_profiles::NODE_DOWNLOAD_URL, cx);
                                },
                            )
                            .into_any_element(),
                        );
                        Some(format!(
                            "Installing its adapter needs {}. Install it, then check again.",
                            setup.detected.missing_tools().join(" and ")
                        ))
                    }
                    Some(DetectRowAction::InstallGuide(url)) => {
                        let step = if setup.guides_opened.contains(&profile.id) {
                            format!("After installing {}, check again.", profile.label)
                        } else {
                            format!(
                                "Install {} with its own installer, then check again.",
                                profile.label
                            )
                        };
                        let opened = id.clone();
                        actions.push(
                            self.action_button(
                                format!("agent-install-guide-{id}"),
                                "Install guide",
                                true,
                                Emphasis::Neutral,
                                cx,
                                move |this, _, cx| {
                                    if this.open_link(url, cx) {
                                        this.agent_setup.guides_opened.insert(opened.clone());
                                    }
                                },
                            )
                            .into_any_element(),
                        );
                        Some(step)
                    }
                    Some(DetectRowAction::None) => {
                        Some(format!("Install {}, then check again.", profile.label))
                    }
                    Some(DetectRowAction::TurnOn) | None => {
                        // Not checked yet, or a task runs: choosing it uses the saved profile
                        // (a running task is stopped first and handed over).
                        let select = id.clone();
                        actions.push(
                            self.action_button(
                                format!("agent-provider-{id}"),
                                if busy_switch {
                                    "Switch & stop"
                                } else {
                                    "Use this agent"
                                },
                                !switch_pending && !self.saves.is_pending(),
                                Emphasis::Neutral,
                                cx,
                                move |this, _, cx| this.select_provider(&select, cx),
                            )
                            .into_any_element(),
                        );
                        Some(if busy_switch {
                            "A task is running: switching stops it and hands its work over.".into()
                        } else if switch_pending {
                            "Resolve the pending switch first.".into()
                        } else {
                            "Check installed agents to see whether it is here, or use its saved setup."
                                .into()
                        })
                    }
                }
            }
        };

        let open = self.expanded_agents.contains(&profile.id);
        let toggle = cx.entity();
        let toggled = id.clone();
        let mut card = AgentCard::new(
            &theme,
            id.clone(),
            profile.label.clone(),
            summary,
            (state.label(), state.tone()),
        )
        .in_use(selected)
        .open(open)
        .on_toggle(move |_, app| {
            toggle.update(app, |this, cx| {
                if !this.expanded_agents.remove(&toggled) {
                    this.expanded_agents.insert(toggled.clone());
                }
                cx.notify();
            });
        })
        .facts(facts);
        if let Some(command) = sign_in {
            card = card.sign_in(command);
        }
        if let Some(step) = next_step {
            card = card.next_step(step);
        }
        for action in actions {
            card = card.action(action);
        }
        if let Some(status) = self.launch_status(&profile.adapter.provider) {
            card = card.footer(status);
        }
        if profile.experimental {
            card = card.footer("Experimental: not yet qualified against every Studio workflow.");
        }
        match &self.reveal_agent {
            Some((reveal, scroll)) if *reveal == profile.id => {
                let panel = cx.entity().downgrade();
                div()
                    .relative()
                    .child(card)
                    .child(crate::kit::reveal_probe(scroll.clone(), move |cx| {
                        let _ = panel.update(cx, |panel, _| panel.reveal_agent = None);
                    }))
                    .into_any_element()
            }
            _ => card.into_any_element(),
        }
    }

    // ---- Find more agents ------------------------------------------------------------------------

    /// The search field of Find more agents, made the first time it is needed.
    pub(super) fn registry_search(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInputState> {
        if let Some(input) = &self.agent_setup.registry_search {
            return input.clone();
        }
        let input = super::input("Search by name or description", window, cx);
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        self.agent_setup.registry_search = Some(input.clone());
        input
    }

    /// Fetches the ACP registry on a background thread. Only on the user's click; the list
    /// is shown, and nothing in it is installed or started.
    fn find_more_agents(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.agent_setup.registry_list == RegistryList::Loading {
            return;
        }
        self.registry_search(window, cx);
        self.agent_setup.registry_list = RegistryList::Loading;
        let task = cx
            .background_executor()
            .spawn(async { acp_registry::fetch_registry() });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |panel, cx| {
                panel.agent_setup.registry_list = match result {
                    Ok(agents) => RegistryList::Loaded(agents),
                    Err(error) => RegistryList::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Fills the custom agent form from a registry entry and opens it. The program's path is
    /// filled only when a file of the binary's name is found where Detect agents looks (an
    /// existence check); nothing is downloaded, installed or started.
    fn use_registry_entry(&mut self, registry_id: &str, cx: &mut Context<Self>) {
        let RegistryList::Loaded(agents) = &self.agent_setup.registry_list else {
            return;
        };
        let Some(agent) = agents.iter().find(|agent| agent.id == registry_id).cloned() else {
            return;
        };
        let draft = agent.custom_draft();
        let file = AdapterFile {
            provider: draft.label,
            executable: String::new(),
            args: draft.args,
            auth_env_names: Vec::new(),
            auth_method: None,
            mcp: self.mcp_choice,
            wire: studio_harness::AgentWire::Acp,
        };
        self.fill_setup(Some(&file), cx);
        self.agent_setup.custom_open = true;
        let missing = format!(
            "Filled in {}. Install it from its page, enter the absolute path of its program, then Save.",
            agent.name
        );
        let Some(name) = draft.executable_name else {
            self.say(missing, cx);
            return;
        };
        let managed = self
            .attached
            .as_ref()
            .map(|attached| vec![attached.paths.data.join("adapters")])
            .unwrap_or_default();
        let env = studio_harness::DetectionEnv::from_process(managed);
        let task = cx
            .background_executor()
            .spawn(async move { studio_harness::find_executable(&name, &env) });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |panel, cx| {
                let empty = panel.setup.executable.read(cx).text().is_empty();
                match found {
                    Some(path) if empty => {
                        let path = path.to_string_lossy().into_owned();
                        panel
                            .setup
                            .executable
                            .update(cx, |input, cx| input.set_text(path.clone(), cx));
                        panel.say(
                            format!(
                                "Filled in {} with {path}. Check the fields, then Save.",
                                agent.name
                            ),
                            cx,
                        );
                    }
                    _ => panel.say(missing, cx),
                }
            });
        })
        .detach();
    }

    /// Find more agents: the ACP registry's agents, searchable, after Oleafly's registry
    /// browser. Each entry fills in the custom agent form or opens its page.
    fn registry_section(&mut self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let list = self.agent_setup.registry_list.clone();
        let loading = list == RegistryList::Loading;
        let find = self
            .action_button(
                "agent-registry-find",
                match list {
                    RegistryList::Loading => "Loading…",
                    RegistryList::Loaded(_) => "Reload",
                    _ => "Find more agents",
                },
                !loading,
                Emphasis::Neutral,
                cx,
                |this, window, cx| this.find_more_agents(window, cx),
            )
            .into_any_element();
        let intro = "Agents listed in the public ACP registry. Studio only lists them: an \
                     agent fills in the custom agent form or opens its page, and nothing is \
                     installed or started from here.";
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_2))
            .child(self.section_header("MORE AGENTS", intro, find));
        match list {
            RegistryList::Closed => {}
            RegistryList::Loading => {
                section = section.child(
                    setting_card(theme)
                        .flex()
                        .items_center()
                        .gap(px(METRICS.space_2))
                        .child(Spinner::new(theme, "agent-registry-loading"))
                        .child(self.line("Loading the agent registry…")),
                );
            }
            RegistryList::Failed(error) => {
                let retry = self.action_button(
                    "agent-registry-retry",
                    "Retry",
                    true,
                    Emphasis::Neutral,
                    cx,
                    |this, window, cx| this.find_more_agents(window, cx),
                );
                section = section.child(
                    Notice::new(
                        "agent-registry-error",
                        UiTone::Danger,
                        "The agent registry could not be loaded",
                    )
                    .body("Check your connection, then retry. You can still register an agent yourself.")
                    .details(error)
                    .action(retry),
                );
            }
            RegistryList::Loaded(agents) => {
                let Some(search) = self.agent_setup.registry_search.clone() else {
                    return section;
                };
                let query = search.read(cx).text().trim().to_owned();
                let shown: Vec<&RegistryAgent> = agents
                    .iter()
                    .filter(|agent| agent.matches(&query))
                    .collect();
                section = section
                    .child(TextInput::new(theme, "agent-registry-search", &search))
                    .child(self.line(format!("{} of {} agents", shown.len(), agents.len())));
                if shown.is_empty() {
                    section = section.child(
                        setting_card(theme).child(
                            EmptyState::new(
                                theme,
                                if agents.is_empty() {
                                    "The registry lists no agents".to_owned()
                                } else {
                                    format!("No agents match \"{query}\"")
                                },
                            )
                            .body("Try another name, or register the agent yourself above."),
                        ),
                    );
                } else {
                    let mut rows = setting_card(theme).p_0().flex().flex_col();
                    for (index, agent) in shown.into_iter().enumerate() {
                        rows = rows.child(self.registry_row(theme, agent, index > 0, cx));
                    }
                    section = section.child(rows);
                }
            }
        }
        section
    }

    /// One registry agent: name, version, whether Studio lists it already, description,
    /// distribution, and its actions.
    fn registry_row(
        &mut self,
        theme: &Theme,
        agent: &RegistryAgent,
        divided: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let colors = theme.colors;
        let builtin = agent
            .builtin_profile()
            .filter(|profile| self.registry.profiles.iter().any(|p| p.id == *profile));
        let mut title = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(METRICS.space_2))
            .child(
                text_style(div(), TextStyle::BodyMedium)
                    .text_color(colors.text_1)
                    .child(agent.name.clone()),
            );
        if !agent.version.is_empty() {
            title = title.child(
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_3)
                    .child(format!("v{}", agent.version)),
            );
        }
        if builtin.is_some() {
            title = title.child(Badge::new("Built in").tone(UiTone::Info));
        }
        let mut text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(METRICS.space_1))
            .child(title);
        if !agent.description.is_empty() {
            text = text.child(
                text_style(div(), TextStyle::Small)
                    .text_color(colors.text_2)
                    .child(selectable(agent.description.clone())),
            );
        }
        text = text.child(
            text_style(div(), TextStyle::Small)
                .text_color(colors.text_3)
                .child(selectable(agent.distribution_label())),
        );
        let id = agent.id.clone();
        let mut actions = div().flex().items_center().gap(px(METRICS.space_2));
        if let Some(profile) = builtin {
            actions = actions.child(
                self.action_button(
                    format!("agent-registry-show-{id}"),
                    "Show",
                    true,
                    Emphasis::Neutral,
                    cx,
                    move |this, _, cx| {
                        this.expanded_agents.insert(profile.to_owned());
                        cx.notify();
                    },
                )
                .size(ButtonSize::Sm),
            );
        } else {
            let fill = id.clone();
            actions = actions.child(
                self.action_button(
                    format!("agent-registry-use-{id}"),
                    "Fill in form",
                    true,
                    Emphasis::Neutral,
                    cx,
                    move |this, _, cx| this.use_registry_entry(&fill, cx),
                )
                .size(ButtonSize::Sm),
            );
        }
        if let Some(url) = agent.page_url() {
            actions = actions.child(
                self.action_button(
                    format!("agent-registry-page-{id}"),
                    "Open page",
                    true,
                    Emphasis::Ghost,
                    cx,
                    move |this, _, cx| {
                        this.open_link(&url, cx);
                    },
                )
                .size(ButtonSize::Sm),
            );
        }
        let row = div()
            .flex()
            .items_start()
            .gap(px(METRICS.space_3))
            .p(px(METRICS.space_3))
            .child(text)
            .child(actions);
        if divided {
            row.border_t(px(METRICS.size.divider))
                .border_color(colors.line)
        } else {
            row
        }
    }
}

/// The id of the Register a custom agent card. Profile ids are letters, digits, `-` and `_`,
/// so it never names an agent's card.
const CUSTOM_CARD_ID: &str = "+custom";

/// Where Find more agents stands. Not fetched until the user asks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RegistryList {
    #[default]
    Closed,
    Loading,
    /// Offline, refused or not the registry: the reason.
    Failed(String),
    Loaded(Vec<RegistryAgent>),
}

/// What a CLI agent card says about its agent: the one vocabulary every card's badge and
/// summary use, from what Studio knows (Detect agents, an install or turn-on in flight and
/// the last connection test of the agent in use).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Studio is installing its adapter.
    Installing,
    /// The connection test is running.
    Checking,
    /// The connection test passed.
    Ready,
    /// The connection test reached the agent but it is not signed in.
    NeedsSignIn,
    /// The connection test failed for another reason.
    CheckFailed,
    /// Its adapter was found, but it has not been tested.
    Installed,
    /// Its command line tool was found but not its ACP adapter.
    AdapterMissing,
    /// Neither was found.
    NotInstalled,
    /// Detect agents has not looked for it, and it has not been tested.
    NotChecked,
}

impl AgentState {
    /// The state of one card. `row` is its Detect row's status, `in_use` whether it is the
    /// selected agent, `readiness` the workflow's last connection test (it belongs to the
    /// card in use or turned on), `detected` what Detect agents found.
    pub fn of(
        installing: bool,
        row: Option<RowStatus>,
        in_use: bool,
        readiness: &AdapterReadiness,
        detected: Option<DetectionState>,
    ) -> Self {
        use studio_harness::AdapterStatus;
        if installing {
            return Self::Installing;
        }
        if row == Some(RowStatus::TurningOn) {
            return Self::Checking;
        }
        let found = match detected {
            Some(DetectionState::Ready) => Self::Installed,
            Some(DetectionState::AdapterMissing) => Self::AdapterMissing,
            Some(DetectionState::NotFound) => Self::NotInstalled,
            None => Self::NotChecked,
        };
        if !in_use && row != Some(RowStatus::Readiness) {
            return found;
        }
        match readiness {
            AdapterReadiness::Checking => Self::Checking,
            AdapterReadiness::Checked { report, .. } => match report.status {
                AdapterStatus::Ready { .. } | AdapterStatus::AuthUnknown { .. } => Self::Ready,
                AdapterStatus::AuthRequired { .. } | AdapterStatus::AuthRejected { .. } => {
                    Self::NeedsSignIn
                }
                _ => Self::CheckFailed,
            },
            AdapterReadiness::Unchecked | AdapterReadiness::NotConfigured => found,
        }
    }

    /// The badge's words.
    pub fn label(self) -> &'static str {
        match self {
            Self::Installing => "Installing",
            Self::Checking => "Checking",
            Self::Ready => "Ready",
            Self::NeedsSignIn => "Needs sign-in",
            Self::CheckFailed => "Check failed",
            Self::Installed => "Installed · not checked",
            Self::AdapterMissing => "Adapter missing",
            Self::NotInstalled => "Not installed",
            Self::NotChecked => "Not checked yet",
        }
    }

    /// The badge's tone: green works, amber needs you, red failed, blue in progress, grey
    /// not proven either way.
    pub fn tone(self) -> UiTone {
        match self {
            Self::Ready => UiTone::Success,
            Self::NeedsSignIn | Self::AdapterMissing => UiTone::Warning,
            Self::CheckFailed => UiTone::Danger,
            Self::Installing | Self::Checking => UiTone::Info,
            Self::Installed | Self::NotInstalled | Self::NotChecked => UiTone::Neutral,
        }
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;
    use studio_harness::{AdapterStatus, DiscoveryReport};

    fn checked(status: AdapterStatus) -> AdapterReadiness {
        AdapterReadiness::Checked {
            report: Box::new(DiscoveryReport {
                status,
                executable: None,
                initialized: None,
            }),
            at_unix: 0,
        }
    }

    #[test]
    fn the_connection_test_speaks_only_for_the_agent_in_use() {
        let ready = checked(AdapterStatus::Ready {
            agent: "Claude Code".into(),
            version: "2".into(),
        });
        let found = Some(DetectionState::Ready);
        let row = Some(RowStatus::Detected(DetectionState::Ready));
        assert_eq!(
            AgentState::of(false, row, true, &ready, found),
            AgentState::Ready
        );
        // Another agent found on disk is installed, not ready: it was never tested.
        assert_eq!(
            AgentState::of(false, row, false, &ready, found),
            AgentState::Installed
        );
        let sign_in = checked(AdapterStatus::AuthRequired { methods: vec![] });
        assert_eq!(
            AgentState::of(false, row, true, &sign_in, found),
            AgentState::NeedsSignIn
        );
        let failed = checked(AdapterStatus::ProtocolMismatch {
            detail: "v2".into(),
        });
        assert_eq!(
            AgentState::of(false, row, true, &failed, found),
            AgentState::CheckFailed
        );
        // Turned on but not the selected profile yet: its own test counts.
        assert_eq!(
            AgentState::of(false, Some(RowStatus::Readiness), false, &sign_in, found),
            AgentState::NeedsSignIn
        );
    }

    #[test]
    fn detection_names_the_rest_and_work_in_flight_comes_first() {
        let unchecked = AdapterReadiness::Unchecked;
        let of = |detected| AgentState::of(false, None, false, &unchecked, detected);
        assert_eq!(of(None), AgentState::NotChecked);
        assert_eq!(
            of(Some(DetectionState::AdapterMissing)),
            AgentState::AdapterMissing
        );
        assert_eq!(of(Some(DetectionState::NotFound)), AgentState::NotInstalled);
        // In use but never tested: what detection found.
        assert_eq!(
            AgentState::of(false, None, true, &unchecked, Some(DetectionState::Ready)),
            AgentState::Installed
        );
        assert_eq!(
            AgentState::of(true, None, true, &AdapterReadiness::Checking, None),
            AgentState::Installing
        );
        assert_eq!(
            AgentState::of(false, Some(RowStatus::TurningOn), false, &unchecked, None),
            AgentState::Checking
        );
    }
}
