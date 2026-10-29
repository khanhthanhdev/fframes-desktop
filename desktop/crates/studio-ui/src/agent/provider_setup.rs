//! Provider selection, Detect agents, Install adapter and turn-on: the Agent settings page's
//! "Providers" and "Detect agents" sections.
//!
//! * [`TurnOn`] is the one state a Select or Install adapter click moves through (save, then
//!   one connection test); a Detect row's status is derived from it ([`TurnOn::row_status`]).
//! * [`InstallTracker`] and [`ScanTracker`] hand out tickets: a result whose ticket is no
//!   longer current (the install was replaced, the panel left its project) is dropped.
//! * Detect agents and Install adapter never run at once, so a scan that started before an
//!   install finished can never overwrite the adapter it installed.
//!
//! The types are GPUI-free and unit-tested; the handlers below only wire them to the panel.
use super::{
    ConversationPanel, Emphasis,
    controls::{self, derive_controls},
};
use gpui::{
    Context, ElementId, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled,
    Window, div, px,
};
use std::{
    collections::HashSet,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use studio_engine::agent::{AdapterReadiness, UndoView};
use studio_engine::app_paths::AppPaths;
use studio_engine::providers::{
    host::{self, AdapterFile},
    provider_profiles::{self, DetectRowAction, DetectionScan, DetectionState},
};
use studio_harness::{
    NodeEntry,
    adapter_install::{self, InstallError, InstalledAdapter},
};
use studio_theme::METRICS;

use crate::kit::{Badge, Tone as UiTone, focusable, label, on_activate};

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
    detected: DetectionScan,
    scans: ScanTracker,
    install: InstallTracker,
    pub(super) turn_on: TurnOn,
    /// Profiles whose Install guide was opened since the last Detect agents.
    guides_opened: HashSet<String>,
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

    /// The "Providers" section: one radio row per registry profile.
    /// `switch_pending`: a provider handoff waits; `selection_busy`: a task or Undo runs, so a
    /// selection becomes a stopped-writer switch.
    /// What the provider's last launch in this session proved about its writers.
    fn launch_status(&self, provider: &str) -> String {
        match self.launches.get(provider) {
            None => "Not launched yet".into(),
            Some(launch) if launch.contained => {
                format!("Contained at last launch ({})", launch.ownership)
            }
            Some(launch) => format!("Not contained at last launch ({})", launch.ownership),
        }
    }

    pub(super) fn providers_section(
        &mut self,
        switch_pending: bool,
        selection_busy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut providers = self
            .section("Providers")
            .child(
                self.line("Select a configured ACP provider, or start a stopped-writer handoff."),
            )
            .child(self.line(
                "Writer containment is verified each time a provider starts; the result of its last launch is shown below.",
            ));
        for profile in &self.registry.profiles {
            let selected =
                self.registry.selected_profile_id.as_deref() == Some(profile.id.as_str());
            let button_label = if switch_pending {
                "Handoff pending"
            } else if selected {
                "Selected"
            } else if selection_busy {
                "Switch & stop"
            } else {
                "Use provider"
            };
            let enabled =
                !switch_pending && !self.saves.is_pending() && !(selected && !selection_busy);
            let profile_id = profile.id.clone();
            let description = format!(
                "{} · {} · {}",
                self.launch_status(&profile.adapter.provider),
                profile.id,
                profile.description,
            );
            providers = providers.child(self.provider_row(
                format!("agent-provider-{}", profile.id),
                &profile.label,
                description,
                button_label,
                selected,
                enabled,
                cx,
                move |this, _, cx| this.select_provider(&profile_id, cx),
            ));
        }
        providers
    }

    /// A radio row: the whole row selects the provider; `action_label` is the old button
    /// label and stays as the accessible name and the trailing badge.
    #[allow(clippy::too_many_arguments)]
    fn provider_row(
        &self,
        id: impl Into<ElementId>,
        name: &str,
        description: String,
        action_label: &'static str,
        selected: bool,
        enabled: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme;
        let palette = theme.colors;
        let radius = px(METRICS.radius.control);
        let radio = div()
            .flex_none()
            .size(px(METRICS.size.icon_sm))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border_1()
            .border_color(if selected {
                palette.accent
            } else {
                palette.line_strong
            })
            .children(selected.then(|| {
                div()
                    .size(px(METRICS.space_2))
                    .rounded_full()
                    .bg(palette.accent)
            }));
        let row = div()
            .id(id.into())
            .role(gpui::Role::RadioButton)
            .aria_label(action_label)
            .flex()
            .items_center()
            .gap(px(METRICS.space_2))
            .p(px(METRICS.space_2))
            .rounded(radius)
            .bg(if selected {
                palette.ctl
            } else {
                palette.card_2
            })
            .child(radio)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    // Plain text: the whole row is a radio button.
                    .child(
                        studio_theme::text_style(div(), studio_theme::TextStyle::Label)
                            .text_color(palette.text_1)
                            .child(name.to_owned()),
                    )
                    .child(
                        studio_theme::text_style(div(), studio_theme::TextStyle::Small)
                            .text_color(palette.text_2)
                            .child(description),
                    ),
            )
            .child(Badge::new(action_label).tone(if selected {
                UiTone::Info
            } else {
                UiTone::Neutral
            }));
        let row = focusable(row, &theme, !enabled);
        on_activate(row, !enabled, Some(Rc::new(Self::activate(cx, action))))
    }

    /// The "Detect agents" section: the Detect button and one row per profile it found.
    pub(super) fn detect_section(
        &mut self,
        readiness: &AdapterReadiness,
        settings_editable: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let setup = &self.agent_setup;
        let installing = setup.install.is_running();
        let mut detect = self.section("Detect agents").child(self.line(
            "Looks for installed agents without starting them. No API key is needed; each agent uses its own sign-in.",
        ));
        detect = detect.child(self.action_button(
            "agent-detect",
            if setup.scans.is_running() {
                "Detecting…"
            } else {
                "Detect agents"
            },
            !setup.scans.is_running() && !installing,
            Emphasis::Neutral,
            cx,
            |this, _, cx| this.detect_agents(cx),
        ));
        for found in &setup.detected.found {
            let Some(profile) = self.registry.profiles.iter().find(|p| p.id == found.id) else {
                continue;
            };
            let state = setup.detected.state(found);
            let location = match setup.detected.installed.get(&found.id) {
                Some(installed) => {
                    format!(
                        " · installed by Studio · {}",
                        studio_bootstrap::external_path(&installed.entry).display()
                    )
                }
                None => found
                    .adapter
                    .as_ref()
                    .or(found.cli.as_ref())
                    .map_or_else(String::new, |path| {
                        format!(" · {}", studio_bootstrap::external_path(path).display())
                    }),
            };
            let installable = profile.adapter.wire == studio_harness::AgentWire::Acp
                && adapter_install::package_for(&profile.id).is_some();
            let action = provider_profiles::row_action(
                state,
                installable,
                setup.detected.tools().is_some(),
                provider_profiles::install_guide_url(&profile.id),
            );
            let installing_this = setup.install.installing() == Some(profile.id.as_str());
            let hint = match action {
                DetectRowAction::GetNode => format!(
                    " · Install adapter needs {}",
                    setup.detected.missing_tools().join(" and ")
                ),
                DetectRowAction::InstallGuide(_) if setup.guides_opened.contains(&profile.id) => {
                    " · After installing, press Detect agents again.".to_owned()
                }
                _ => String::new(),
            };
            let status = match setup.turn_on.row_status(&profile.id, state) {
                // The save is still in flight: the snapshot shows the previous adapter.
                RowStatus::TurningOn => "Turning on…".to_owned(),
                RowStatus::Readiness => controls::turned_on_status(readiness),
                RowStatus::Detected(state) => state.label().to_owned(),
            };
            let (select_id, install_id) = (profile.id.clone(), profile.id.clone());
            let mut actions = div().flex().gap_1();
            if installing_this {
                actions = actions.child(self.action_button(
                    format!("agent-install-cancel-{}", profile.id),
                    "Cancel install",
                    true,
                    Emphasis::Neutral,
                    cx,
                    |this, _, cx| {
                        this.agent_setup.install.cancel();
                        this.say("Cancelling the install…", cx);
                    },
                ));
            } else {
                match action {
                    DetectRowAction::TurnOn => {
                        actions = actions.child(self.action_button(
                            format!("agent-detected-{}", profile.id),
                            "Select",
                            settings_editable && !self.saves.is_pending(),
                            Emphasis::Primary,
                            cx,
                            move |this, _, cx| this.use_detected(&select_id, cx),
                        ));
                    }
                    DetectRowAction::InstallAdapter => {
                        actions = actions.child(self.action_button(
                            format!("agent-install-{}", profile.id),
                            "Install adapter",
                            !installing && !setup.scans.is_running(),
                            Emphasis::Neutral,
                            cx,
                            move |this, _, cx| this.install_adapter(&install_id, cx),
                        ));
                    }
                    DetectRowAction::GetNode => {
                        actions = actions.child(self.action_button(
                            format!("agent-get-node-{}", profile.id),
                            "Get Node.js",
                            true,
                            Emphasis::Neutral,
                            cx,
                            |this, _, cx| {
                                this.open_link(provider_profiles::NODE_DOWNLOAD_URL, cx);
                            },
                        ));
                    }
                    DetectRowAction::InstallGuide(url) => {
                        actions = actions.child(self.action_button(
                            format!("agent-install-guide-{}", profile.id),
                            "Install guide",
                            true,
                            Emphasis::Neutral,
                            cx,
                            move |this, _, cx| {
                                if this.open_link(url, cx) {
                                    this.agent_setup.guides_opened.insert(install_id.clone());
                                }
                            },
                        ));
                    }
                    DetectRowAction::None => {}
                }
            }
            detect = detect.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(label(&self.theme, profile.label.clone()))
                            .child(self.line(format!("{status}{location}{hint}"))),
                    )
                    .child(actions),
            );
        }
        detect
    }
}
