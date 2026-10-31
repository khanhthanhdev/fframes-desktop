//! App-local provider registry and profile management.
//!
//! Provides first-party profiles for Claude, Codex, Pi, and Antigravity, and validation
//! preventing credentials in configuration. Only the current registry schema is read: a
//! registry made by another Studio version is refused and left as it is.

use super::host::{AdapterFile, McpChoice};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use studio_harness::{
    AgentWire, Detected, DetectionEnv, DetectionSpec,
    adapter_install::{InstallTools, InstalledAdapter, find_installed, package_for},
    detect_agents, find_executable,
};

/// Current schema version of the provider registry file.
pub const REGISTRY_SCHEMA_VERSION: u32 = 2;

/// Stable identifier for a provider profile.
pub const BUILTIN_CLAUDE_ID: &str = "claude";
pub const BUILTIN_CODEX_ID: &str = "codex";
pub const BUILTIN_PI_ID: &str = "pi";
pub const BUILTIN_ANTIGRAVITY_ID: &str = "antigravity";

/// Identifiers of the four first-party provider profiles.
pub const BUILTIN_IDS: [&str; 4] = [
    BUILTIN_CLAUDE_ID,
    BUILTIN_CODEX_ID,
    BUILTIN_PI_ID,
    BUILTIN_ANTIGRAVITY_ID,
];

/// Checks whether an ID belongs to a first-party provider profile.
pub fn is_builtin_id(id: &str) -> bool {
    BUILTIN_IDS.contains(&id)
}

/// Where Install adapter's runtime comes from when Node.js or npm is missing.
pub const NODE_DOWNLOAD_URL: &str = "https://nodejs.org/en/download";

/// The official install page of a built-in agent, for the Install guide button. Custom
/// profiles have none.
///
/// Checked on 2026-10-12. Antigravity has no official Google page documenting its ACP
/// binary, so it gets no guide rather than a third-party one.
pub fn install_guide_url(profile_id: &str) -> Option<&'static str> {
    match profile_id {
        BUILTIN_CLAUDE_ID => Some("https://code.claude.com/docs/en/setup"),
        BUILTIN_CODEX_ID => Some("https://learn.chatgpt.com/docs/codex/cli"),
        // The adapter's README lists pi and Node.js as prerequisites.
        BUILTIN_PI_ID => Some("https://github.com/svkozak/pi-acp"),
        _ => None,
    }
}

/// The command that signs a built-in agent in with its own tool, shown on its card to copy
/// into a terminal (the same commands Oleafly's agent catalog uses). Custom profiles and
/// Antigravity, which signs in through Google's environment variables, have none.
pub fn sign_in_command(profile_id: &str) -> Option<&'static str> {
    match profile_id {
        BUILTIN_CLAUDE_ID => Some("claude auth login"),
        BUILTIN_CODEX_ID => Some("codex login"),
        BUILTIN_PI_ID => Some("pi"),
        _ => None,
    }
}

/// Whether a guide link may be handed to the browser: `https://` only, exactly as written.
pub fn is_openable(url: &str) -> bool {
    url.strip_prefix("https://")
        .is_some_and(|rest| !rest.is_empty() && !url.chars().any(char::is_whitespace))
}

/// The action a Detect agents row offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectRowAction {
    /// Select: save the adapter and test it once.
    TurnOn,
    InstallAdapter,
    /// Install adapter needs Node.js and npm first.
    GetNode,
    InstallGuide(&'static str),
    None,
}

/// What a Detect row offers for its state. `installable` says Studio can install the
/// profile's adapter, `tools_present` that Node.js and npm were found.
pub fn row_action(
    state: DetectionState,
    installable: bool,
    tools_present: bool,
    guide: Option<&'static str>,
) -> DetectRowAction {
    let guide = || guide.map_or(DetectRowAction::None, DetectRowAction::InstallGuide);
    match state {
        DetectionState::Ready => DetectRowAction::TurnOn,
        DetectionState::AdapterMissing if installable && tools_present => {
            DetectRowAction::InstallAdapter
        }
        DetectionState::AdapterMissing if installable => DetectRowAction::GetNode,
        DetectionState::AdapterMissing | DetectionState::NotFound => guide(),
    }
}

/// A single configured provider profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfile {
    /// Stable profile identifier (alphanumeric, hyphens, underscores).
    pub id: String,
    /// Display label shown in the UI.
    pub label: String,
    /// Descriptive summary of distribution, package, or upstream ownership.
    #[serde(default)]
    pub description: String,
    /// The underlying adapter configuration for process launch.
    pub adapter: AdapterFile,
    /// The agent's own command-line tool, looked for by Detect agents. Empty means the
    /// agent speaks ACP natively and the adapter executable is the only thing to find.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cli_executable: String,
    /// Whether this is one of the four first-party profiles (runtime property, not serialized).
    #[serde(skip)]
    pub builtin: bool,
    /// Experimental status until passing qualification (runtime property, not serialized).
    #[serde(skip)]
    pub experimental: bool,
}

impl ProviderProfile {
    /// Validates the profile fields.
    /// Whether this is one of the four first-party profiles.
    pub fn is_builtin(&self) -> bool {
        is_builtin_id(&self.id)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || self.id.len() > 64 {
            return Err("Profile \"id\" must be 1 to 64 characters.".into());
        }
        if !self
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(
                "Profile \"id\" may only contain alphanumeric characters, hyphens, and underscores."
                    .into(),
            );
        }
        if self.label.trim().is_empty() || self.label.len() > 64 {
            return Err("Profile \"label\" must be 1 to 64 characters.".into());
        }
        self.adapter.validate()
    }
}

/// The versioned app-local provider registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderRegistry {
    /// Schema version for forward-compatibility detection.
    pub schema_version: u32,
    /// Currently selected profile ID for tasks and workflow execution (None = no selection).
    pub selected_profile_id: Option<String>,
    /// Available provider profiles.
    pub profiles: Vec<ProviderProfile>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            selected_profile_id: None,
            profiles: default_builtin_profiles(),
        }
    }
}

/// Default built-in provider profiles for Claude, Codex, Pi, and Antigravity.
pub fn default_builtin_profiles() -> Vec<ProviderProfile> {
    let mut profiles = vec![
        ProviderProfile {
            id: BUILTIN_CLAUDE_ID.into(),
            label: "Claude".into(),
            description: "Claude Code via its native stream-json protocol".into(),
            adapter: AdapterFile {
                provider: "Claude".into(),
                executable: String::new(),
                args: Vec::new(),
                auth_env_names: vec!["ANTHROPIC_API_KEY".into()],
                auth_method: None,
                mcp: McpChoice::Baseline,
                wire: AgentWire::ClaudeStreamJson,
            },
            cli_executable: String::new(),
            builtin: true,
            experimental: true,
        },
        ProviderProfile {
            id: BUILTIN_CODEX_ID.into(),
            label: "Codex".into(),
            description: "Codex via its native app-server protocol".into(),
            adapter: AdapterFile {
                provider: "Codex".into(),
                executable: String::new(),
                args: Vec::new(),
                auth_env_names: vec!["OPENAI_API_KEY".into(), "CODEX_API_KEY".into()],
                auth_method: None,
                mcp: McpChoice::Baseline,
                wire: AgentWire::CodexAppServer,
            },
            cli_executable: String::new(),
            builtin: true,
            experimental: true,
        },
        ProviderProfile {
            id: BUILTIN_PI_ID.into(),
            label: "Pi".into(),
            description: "Pi via community ACP adapter by svkozak (pi-acp)".into(),
            adapter: AdapterFile {
                provider: "Pi".into(),
                executable: String::new(),
                args: Vec::new(),
                auth_env_names: Vec::new(),
                auth_method: None,
                mcp: McpChoice::Unsupported,
                wire: AgentWire::Acp,
            },
            cli_executable: String::new(),
            builtin: true,
            experimental: true,
        },
        ProviderProfile {
            id: BUILTIN_ANTIGRAVITY_ID.into(),
            label: "Antigravity".into(),
            description: "Google Antigravity via official ACP binary (antigravity-acp)".into(),
            adapter: AdapterFile {
                provider: "Antigravity".into(),
                executable: String::new(),
                args: Vec::new(),
                auth_env_names: vec!["GEMINI_API_KEY".into(), "GOOGLE_API_KEY".into()],
                auth_method: None,
                mcp: McpChoice::Baseline,
                wire: AgentWire::Acp,
            },
            cli_executable: String::new(),
            builtin: true,
            experimental: true,
        },
    ];
    for profile in &mut profiles {
        profile.adapter.executable = match profile.adapter.wire {
            AgentWire::Acp => builtin_adapter(&profile.id).unwrap_or_default(),
            _ => builtin_cli(&profile.id),
        }
        .into();
        profile.cli_executable = builtin_cli(&profile.id).into();
    }
    profiles
}

/// The ACP adapter executable a first-party agent is known by. Detect agents looks for this
/// name whatever launch was saved for the profile, so a saved `node <entry>` launch can
/// never make every Node.js installation look like the agent's adapter.
pub fn builtin_adapter(id: &str) -> Option<&'static str> {
    match id {
        BUILTIN_CLAUDE_ID => Some("claude-agent-acp"),
        BUILTIN_CODEX_ID => Some("codex-acp"),
        BUILTIN_PI_ID => Some("pi-acp"),
        BUILTIN_ANTIGRAVITY_ID => Some("antigravity-acp"),
        _ => None,
    }
}

/// The CLI of a first-party agent. Empty when the adapter is the agent's own ACP binary.
fn builtin_cli(id: &str) -> &'static str {
    match id {
        BUILTIN_CLAUDE_ID => "claude",
        BUILTIN_CODEX_ID => "codex",
        BUILTIN_PI_ID => "pi",
        _ => "",
    }
}

/// Refuses registry text that is valid JSON but not the current format: another `schema_version`
/// (older or newer) or a shape this Studio does not read. `source` names the file in the message.
/// Text that is not JSON at all passes (the parser reports it) and is the only kind a save may
/// replace. The file itself is never changed.
pub fn check_registry_version(text: &str, source: &str) -> Result<(), String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Ok(());
    };
    match value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    {
        Some(version) if version < u64::from(REGISTRY_SCHEMA_VERSION) => Err(format!(
            "The provider registry {source} was made by an older Studio (schema version {version}, this Studio reads version {REGISTRY_SCHEMA_VERSION}). It is left as it is; set up the agent again after moving it aside."
        )),
        Some(version) if version > u64::from(REGISTRY_SCHEMA_VERSION) => Err(format!(
            "The provider registry {source} is schema version {version}, newer than this Studio reads ({REGISTRY_SCHEMA_VERSION}). It is left as it is and cannot be used."
        )),
        _ => serde_json::from_value::<ProviderRegistry>(value)
            .map(|_| ())
            .map_err(|e| {
                format!(
                    "The provider registry {source} is not in the format this Studio reads ({e}). It is left as it is; set up the agent again after moving it aside."
                )
            }),
    }
}

impl ProviderRegistry {
    /// Parses registry JSON from a string. A registry of any schema version but the current
    /// one is refused.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut registry: Self = serde_json::from_str(text)
            .map_err(|e| format!("Invalid provider registry JSON: {e}"))?;
        registry.validate()?;
        for profile in &mut registry.profiles {
            profile.builtin = is_builtin_id(&profile.id);
            profile.experimental = true;
        }
        Ok(registry)
    }
    /// Validates the registry structure, schema version, and profile contents.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != REGISTRY_SCHEMA_VERSION {
            return Err(format!(
                "Registry schema version {} is not the version this Studio reads ({}).",
                self.schema_version, REGISTRY_SCHEMA_VERSION
            ));
        }
        if self.profiles.is_empty() {
            return Err("Registry must contain at least one profile.".into());
        }

        let mut seen_ids = std::collections::HashSet::new();
        for profile in &self.profiles {
            if !seen_ids.insert(&profile.id) {
                return Err(format!("Duplicate profile ID: \"{}\".", profile.id));
            }
            profile.validate()?;
        }

        if let Some(selected) = &self.selected_profile_id
            && !seen_ids.contains(selected)
        {
            return Err(format!(
                "Selected profile \"{selected}\" does not exist in registry."
            ));
        }

        Ok(())
    }

    /// Returns the currently selected provider profile.
    pub fn selected_profile(&self) -> Option<&ProviderProfile> {
        let id = self.selected_profile_id.as_deref()?;
        self.profiles.iter().find(|p| p.id == id)
    }

    /// Returns a mutable reference to the currently selected provider profile.
    pub fn selected_profile_mut(&mut self) -> Option<&mut ProviderProfile> {
        let id = self.selected_profile_id.as_deref()?;
        self.profiles.iter_mut().find(|p| p.id == id)
    }

    /// Returns the active adapter configuration of the currently selected profile.
    pub fn selected_adapter(&self) -> Option<&AdapterFile> {
        self.selected_profile().map(|p| &p.adapter)
    }

    /// Selects a profile by ID.
    pub fn select_profile(&mut self, id: &str) -> Result<(), String> {
        if self.profiles.iter().any(|p| p.id == id) {
            self.selected_profile_id = Some(id.to_owned());
            Ok(())
        } else {
            Err(format!("Profile with ID \"{id}\" not found in registry."))
        }
    }
    /// Saves or updates an adapter configuration without mutating builtin profile defaults.
    ///
    /// If modifying a custom profile, updates that profile. If saving a custom configuration
    /// while a builtin is selected, creates/selects a custom profile rather than mutating the
    /// builtin profile in place.
    pub fn save_custom_adapter(&mut self, file: AdapterFile) {
        let norm = file.provider.trim().to_lowercase();
        // If the file explicitly targets one of the builtins by label or id, update that builtin's settings:
        if let Some(builtin) = self
            .profiles
            .iter_mut()
            .find(|p| p.builtin && (p.id == norm || p.label.to_lowercase() == norm))
        {
            builtin.adapter = file;
            self.selected_profile_id = Some(builtin.id.clone());
            return;
        }
        // If the currently selected profile is already a custom profile, update it:
        let custom_selected = self.selected_profile_mut().filter(|p| !p.builtin);
        if let Some(selected) = custom_selected {
            selected.adapter = file.clone();
            selected.label = file.provider;
            return;
        }
        // Otherwise, create a new custom profile:
        let slug = slugify(&file.provider);
        let id = if self.profiles.iter().any(|p| p.id == slug) {
            format!("{slug}-custom")
        } else {
            slug
        };
        let profile = ProviderProfile {
            id: id.clone(),
            label: file.provider.clone(),
            description: format!("Custom configuration for {}", file.provider),
            adapter: file,
            cli_executable: String::new(),
            builtin: false,
            experimental: true,
        };
        self.profiles.push(profile);
        self.selected_profile_id = Some(id);
    }

    /// Saves the launch Detect agents filled in for profile `id` and selects it. Only the
    /// profile's adapter changes: its id, label, description and CLI stay, and no other
    /// profile is touched, whatever label the adapter carries.
    pub fn save_profile_adapter(&mut self, id: &str, file: AdapterFile) -> Result<(), String> {
        file.validate()?;
        let profile = self
            .profiles
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("Profile with ID \"{id}\" not found in registry."))?;
        profile.adapter = file;
        self.selected_profile_id = Some(id.to_owned());
        Ok(())
    }

    /// Formats the registry as formatted JSON.
    pub fn to_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned())
    }
}

fn slugify(text: &str) -> String {
    let slug: String = text
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "custom-adapter".into()
    } else {
        trimmed.into()
    }
}

/// What Detect agents found for one profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionState {
    /// The adapter executable was found and can be selected.
    Ready,
    /// The agent's CLI is installed but its ACP adapter is not.
    AdapterMissing,
    NotFound,
}

impl DetectionState {
    pub fn of(found: &Detected) -> Self {
        match (&found.adapter, &found.cli) {
            (Some(_), _) => Self::Ready,
            (None, Some(_)) => Self::AdapterMissing,
            (None, None) => Self::NotFound,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::AdapterMissing => "CLI found, adapter missing",
            Self::NotFound => "Not found",
        }
    }
}

/// What tells Detect agents that a profile's adapter is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterIdentity {
    /// An executable looked up by file name in the search roots.
    Name(String),
    /// A custom adapter saved as `node <entry>`: the entry file itself. `None` when the
    /// arguments name no entry; such an adapter is never found.
    Entry(Option<PathBuf>),
}

/// Whether `executable` is the Node.js runtime (`node`, `node.exe`, any case).
fn is_node(executable: &str) -> bool {
    Path::new(executable)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case("node"))
}

/// The index of the script argument of a `node <entry>` launch: the first argument that is
/// not a Node.js flag. `None` when the adapter is not started through Node.js.
fn node_entry_index(file: &AdapterFile) -> Option<usize> {
    if !is_node(&file.executable) {
        return None;
    }
    file.args.iter().position(|arg| !arg.starts_with('-'))
}

impl ProviderProfile {
    /// How Detect agents recognises this profile's adapter. A built-in is known by its own
    /// adapter name, never by the launch saved for it; a custom adapter by the file name of
    /// its executable, or by its entry when it starts as `node <entry>`.
    pub fn adapter_identity(&self) -> AdapterIdentity {
        if is_builtin_id(&self.id) && self.adapter.wire != AgentWire::Acp {
            return AdapterIdentity::Name(builtin_cli(&self.id).to_owned());
        }
        if let Some(name) = builtin_adapter(&self.id) {
            return AdapterIdentity::Name(name.to_owned());
        }
        if is_node(&self.adapter.executable) {
            return AdapterIdentity::Entry(
                node_entry_index(&self.adapter).map(|i| PathBuf::from(&self.adapter.args[i])),
            );
        }
        let configured = Path::new(&self.adapter.executable);
        AdapterIdentity::Name(
            configured
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&self.adapter.executable)
                .to_owned(),
        )
    }

    /// The profile's adapter without the launch Detect agents fills in: a saved
    /// `node <entry>` launch keeps only the arguments after the entry, so selecting a
    /// different file never carries the old entry along.
    pub fn launch_template(&self) -> AdapterFile {
        let mut file = self.adapter.clone();
        if let Some(index) = node_entry_index(&file) {
            file.args.drain(..=index);
        }
        file
    }
}

impl ProviderRegistry {
    /// One detection request per profile known by an executable name (see
    /// [`ProviderProfile::adapter_identity`]); a `node <entry>` profile is checked by its
    /// entry in [`scan_agents`].
    pub fn detection_specs(&self) -> Vec<DetectionSpec> {
        self.profiles
            .iter()
            .filter_map(|profile| {
                let AdapterIdentity::Name(adapter) = profile.adapter_identity() else {
                    return None;
                };
                Some(DetectionSpec {
                    id: profile.id.clone(),
                    cli_executable: if profile.cli_executable.is_empty() {
                        adapter.clone()
                    } else {
                        profile.cli_executable.clone()
                    },
                    adapter_executable: adapter,
                    extra_dirs: Vec::new(),
                })
            })
            .collect()
    }
}

/// Everything Detect agents learns in one pass.
#[derive(Debug, Clone, Default)]
pub struct DetectionScan {
    pub found: Vec<Detected>,
    /// Adapters installed by the app, by profile id.
    pub installed: HashMap<String, InstalledAdapter>,
    /// The Node.js runtime a script adapter starts with.
    pub node: Option<PathBuf>,
    /// npm, which Install adapter needs besides Node.js.
    pub npm: Option<PathBuf>,
}

impl DetectionScan {
    /// Whether a profile can be selected: its adapter was found or the app installed it.
    pub fn is_ready(&self, profile_id: &str) -> bool {
        self.installed.contains_key(profile_id)
            || self
                .found
                .iter()
                .any(|found| found.id == profile_id && found.adapter.is_some())
    }

    /// The row state for a profile, counting an app-installed adapter as ready.
    pub fn state(&self, found: &Detected) -> DetectionState {
        if self.installed.contains_key(&found.id) {
            DetectionState::Ready
        } else {
            DetectionState::of(found)
        }
    }

    /// The runtimes an adapter install needs, when both were found.
    pub fn tools(&self) -> Option<InstallTools> {
        self.node
            .clone()
            .zip(self.npm.clone())
            .map(|(node, npm)| InstallTools { node, npm })
    }

    /// The install runtimes that were not found, for example `["npm"]`.
    pub fn missing_tools(&self) -> Vec<&'static str> {
        [
            ("Node.js", self.node.is_none()),
            ("npm", self.npm.is_none()),
        ]
        .into_iter()
        .filter_map(|(name, missing)| missing.then_some(name))
        .collect()
    }
}

/// Runs detection for every profile of `registry`. Existence checks only.
pub fn scan_agents(
    registry: &ProviderRegistry,
    env: &DetectionEnv,
    adapters_dir: &Path,
) -> DetectionScan {
    let node = find_executable("node", env);
    let npm = find_executable("npm", env);
    let installed = node
        .iter()
        .flat_map(|node| {
            registry.profiles.iter().filter_map(move |profile| {
                if profile.adapter.wire != AgentWire::Acp {
                    return None;
                }
                let package = package_for(&profile.id)?;
                let adapter = find_installed(adapters_dir, &package, node)?;
                Some((profile.id.clone(), adapter))
            })
        })
        .collect();
    // `detection_specs` lists the name-identified profiles in registry order.
    let mut by_name = detect_agents(&registry.detection_specs(), env).into_iter();
    let found = registry
        .profiles
        .iter()
        .filter_map(|profile| match profile.adapter_identity() {
            AdapterIdentity::Name(_) => by_name.next(),
            AdapterIdentity::Entry(entry) => Some(Detected {
                id: profile.id.clone(),
                cli: (!profile.cli_executable.is_empty())
                    .then(|| find_executable(&profile.cli_executable, env))
                    .flatten(),
                adapter: entry.filter(|entry| entry.is_absolute() && entry.is_file()),
            }),
        })
        .collect();
    DetectionScan {
        found,
        installed,
        node,
        npm,
    }
}
