//! Hosting of one [`AgentWorkflow`] inside the GPUI shell, without any GPUI types.
//!
//! * The adapter description is app-local (`<data>/agent-adapter.json`): executable,
//!   arguments and the NAMES of the environment variables carrying sign-in. Values are
//!   never stored, and a value pasted where a name belongs is refused.
//! * [`HostInbox`] is the only channel from workflow threads to the UI thread: the open
//!   result, post-commit preview hand-offs and a wake flag. The UI polls it from its
//!   existing frame loop, so no workflow thread ever touches an entity.
use super::qualification::{OwnershipPolicy, Resolution, probe, resolve_ownership};
use crate::agent_workflow::{
    AdapterSettings, AgentWorkflow, BuildSettings, PreviewHandoff, PromotionHandoff, ToolSettings,
    WorkflowConfig, WorkflowError, short_runtime_dir,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use studio_agent_spike::{AdapterConfig, ExecutableSearch, McpStdioSupport};
use studio_engine::{Controller, app_paths::AppPaths};

/// File name of the app-local adapter description inside the app data directory.
pub const SETTINGS_FILE: &str = "agent-adapter.json";
/// File name of the app-local provider registry inside the app data directory.
pub const REGISTRY_FILE: &str = "provider-registry.json";
/// Suffix for migration backup rollback files.
pub const REGISTRY_BACKUP_SUFFIX: &str = ".migration-bak";

/// How an adapter's stdio MCP support is treated (host policy; ACP v1 has no flag).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpChoice {
    /// Offer the project tools as a stdio MCP server (and the command-line route).
    #[default]
    Baseline,
    /// Command-line route only.
    Unsupported,
}

/// The app-local adapter description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterFile {
    /// Label shown on rows and readiness; not an identity or a credential.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Absolute path of the adapter executable, or a file name inside `<data>/adapters`
    /// (the global `PATH` is never consulted).
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Names of environment variables forwarded to the adapter. Values are read from the
    /// app's environment at task start and are never stored or shown.
    #[serde(default)]
    pub auth_env_names: Vec<String>,
    /// Advertised authentication method to select explicitly, if the adapter needs one.
    #[serde(default)]
    pub auth_method: Option<String>,
    #[serde(default)]
    pub mcp: McpChoice,
}

fn default_provider() -> String {
    "agent".into()
}

fn looks_like_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 128
}

impl AdapterFile {
    /// Parses the Setup text. `Ok(None)` = blank (no adapter).
    pub fn parse(text: &str) -> Result<Option<Self>, String> {
        if text.trim().is_empty() {
            return Ok(None);
        }
        let file: AdapterFile = serde_json::from_str(text).map_err(|e| {
            let message = e.to_string();
            if message.contains("`writer_qualification`") {
                // Containment is derived from validated evidence now, never configured.
                "\"writer_qualification\" is no longer a setting: writer containment is derived from the installed qualification ledger for this exact adapter. Remove it.".to_owned()
            } else {
                format!("Invalid adapter JSON: {message}")
            }
        })?;
        file.validate()?;
        Ok(Some(file))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.executable.trim().is_empty() {
            return Err("Set \"executable\" to the adapter's path first.".into());
        }
        if self.provider.trim().is_empty() || self.provider.len() > 64 {
            return Err("\"provider\" is a short label (1-64 characters).".into());
        }
        for name in &self.auth_env_names {
            if !looks_like_variable_name(name) {
                return Err(
                    "auth_env_names takes environment variable NAMES (letters, digits, '_'), never values; remove anything that looks like a secret."
                        .into(),
                );
            }
        }
        Ok(())
    }

    /// The workflow settings for this description. Ownership is derived from validated
    /// qualification evidence under `policy` (this hashes the executable and reads the
    /// ledger: call it off the UI thread) and re-derived at every task launch.
    pub fn resolve(
        &self,
        paths: &AppPaths,
        policy: &OwnershipPolicy,
    ) -> (AdapterSettings, Resolution) {
        let resolution = resolve_ownership(self, paths, policy);
        let settings = AdapterSettings {
            provider: self.provider.clone(),
            adapter: AdapterConfig {
                executable: self.executable.clone(),
                args: self.args.clone(),
                auth_env_names: self.auth_env_names.clone(),
            },
            writer_ownership: resolution.ownership.clone(),
            ownership_probe: Some(probe(self.clone(), paths.clone(), policy.clone())),
            mcp: match self.mcp {
                McpChoice::Baseline => McpStdioSupport::Baseline,
                McpChoice::Unsupported => McpStdioSupport::Unsupported,
            },
            auth_method: self.auth_method.clone(),
        };
        (settings, resolution)
    }

    /// The text fields of the Setup tab for this description.
    pub fn fields(&self) -> AdapterFields {
        AdapterFields {
            provider: self.provider.clone(),
            executable: self.executable.clone(),
            args: join_args(&self.args),
            env_names: self.auth_env_names.join(", "),
        }
    }

    /// The description the Setup fields describe, keeping the settings the fields do not
    /// show (`auth_method`) from `base`. `Ok(None)` = every field blank (no adapter).
    pub fn from_fields(
        base: Option<&AdapterFile>,
        fields: &AdapterFields,
        mcp: McpChoice,
    ) -> Result<Option<Self>, String> {
        let blank = fields.executable.trim().is_empty()
            && fields.args.trim().is_empty()
            && fields.env_names.trim().is_empty();
        if blank {
            return Ok(None);
        }
        let file = AdapterFile {
            provider: if fields.provider.trim().is_empty() {
                default_provider()
            } else {
                fields.provider.trim().to_owned()
            },
            executable: fields.executable.trim().to_owned(),
            args: split_args(&fields.args)?,
            auth_env_names: fields
                .env_names
                .split([',', ' ', '\n'])
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect(),
            auth_method: base.and_then(|b| b.auth_method.clone()),
            mcp,
        };
        file.validate()?;
        Ok(Some(file))
    }

    pub fn to_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("plain data serializes")
    }
}

/// The editable text of the Setup tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdapterFields {
    pub provider: String,
    pub executable: String,
    /// Shell-style words: whitespace separated, `'single'` or `"double"` quotes group.
    pub args: String,
    /// Variable NAMES separated by commas or spaces.
    pub env_names: String,
}

/// Splits an argument line into words (quotes group, a backslash escapes inside double
/// quotes and outside quotes).
pub fn split_args(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, c) if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (Some(q), c) if c == q => quote = None,
            (Some('"') | None, '\\') => {
                word.push(
                    chars
                        .next()
                        .ok_or("A trailing backslash escapes nothing.")?,
                );
                started = true;
            }
            (_, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err("An argument has an unclosed quote.".into());
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

/// The inverse of [`split_args`] for display.
pub fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            if !arg.is_empty()
                && !arg
                    .chars()
                    .any(|c| c.is_whitespace() || "'\"\\".contains(c))
            {
                arg.clone()
            } else {
                format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn settings_path(paths: &AppPaths) -> PathBuf {
    paths.data.join(SETTINGS_FILE)
}

pub fn registry_path(paths: &AppPaths) -> PathBuf {
    paths.data.join(REGISTRY_FILE)
}

fn load_qualification_snapshot(
    paths: &AppPaths,
) -> super::provider_profiles::QualificationSnapshot {
    let path = paths
        .data
        .join(super::qualification::LEDGER_DIR)
        .join(super::qualification::LEDGER_FILE_M6);
    match std::fs::read_to_string(path) {
        Ok(text) => super::provider_profiles::QualificationSnapshot::parse(&text)
            .unwrap_or_else(|_| super::provider_profiles::QualificationSnapshot::invalid()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(_) => super::provider_profiles::QualificationSnapshot::invalid(),
    }
}

/// Reads the provider registry; migrates from `agent-adapter.json` if needed.
/// Runs on a background thread.
pub fn load_registry(
    paths: &AppPaths,
) -> Result<Option<super::provider_profiles::ProviderRegistry>, String> {
    let reg_path = registry_path(paths);
    if reg_path.exists() {
        let text = std::fs::read_to_string(&reg_path)
            .map_err(|e| format!("Cannot read the provider registry: {e}"))?;
        let registry = super::provider_profiles::ProviderRegistry::parse(&text)?;
        return Ok(Some(registry));
    }

    // If provider-registry.json does not exist, check for legacy agent-adapter.json to migrate
    let legacy_path = settings_path(paths);
    if legacy_path.exists() {
        let text = std::fs::read_to_string(&legacy_path)
            .map_err(|e| format!("Cannot read legacy adapter settings for migration: {e}"))?;
        let legacy = match AdapterFile::parse(&text)? {
            Some(f) => f,
            None => return Ok(None),
        };
        // Save rollback backup copy before writing new registry, keeping old file intact
        let backup_path = legacy_path.with_extension(format!("json{REGISTRY_BACKUP_SUFFIX}"));
        let _serialized = SETTINGS_WRITES.lock();
        backup_before_write(&legacy_path, &backup_path)?;
        let registry =
            super::provider_profiles::ProviderRegistry::migrate_from_adapter_file(&legacy);
        // Save migrated registry atomically
        write_atomic(&reg_path, registry.to_pretty().as_bytes())?;
        return Ok(Some(registry));
    }

    Ok(None)
}

/// Saves the provider registry atomically with owner-only permissions.
/// Legacy `agent-adapter.json` remains intact and is never overwritten.
pub fn save_registry(
    paths: &AppPaths,
    registry: &super::provider_profiles::ProviderRegistry,
) -> Result<(), String> {
    registry.validate()?;
    let _serialized = SETTINGS_WRITES.lock();
    let reg_path = registry_path(paths);
    if reg_path.exists() {
        let backup_path = reg_path.with_extension(format!("json{REGISTRY_BACKUP_SUFFIX}"));
        backup_before_write(&reg_path, &backup_path)?;
    }
    write_atomic(&reg_path, registry.to_pretty().as_bytes())
}

fn backup_before_write(source: &Path, backup: &Path) -> Result<(), String> {
    std::fs::copy(source, backup).map(|_| ()).map_err(|error| {
        format!(
            "Cannot create rollback backup {}: {error}",
            backup.display()
        )
    })
}

/// Removes the provider registry and legacy settings.
pub fn clear_registry(paths: &AppPaths) -> Result<(), String> {
    let _serialized = SETTINGS_WRITES.lock();
    let _ = std::fs::remove_file(registry_path(paths));
    match std::fs::remove_file(settings_path(paths)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Cannot remove the adapter settings: {e}")),
    }
}

/// Reads the saved description; `Ok(None)` when none was ever saved. Runs on a
/// background thread.
pub fn load_settings(paths: &AppPaths) -> Result<Option<AdapterFile>, String> {
    if registry_path(paths).exists() {
        match load_registry(paths)? {
            Some(reg) => Ok(reg.selected_adapter().cloned()),
            None => Ok(None),
        }
    } else {
        match std::fs::read_to_string(settings_path(paths)) {
            Ok(text) => AdapterFile::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("Cannot read the adapter settings: {e}")),
        }
    }
}

/// Every settings mutation (save or clear) of this process runs under this lock, in call
/// order per thread: two overlapping writers can never interleave their files or leave a
/// stale temporary behind.
static SETTINGS_WRITES: Mutex<()> = Mutex::new(());
static TEMPORARY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Writes the description atomically with owner-only permissions. Runs off the UI thread.
/// When the provider registry exists, saves as custom profile configuration without corrupting
/// builtin defaults or mutating legacy files.
pub fn save_settings(paths: &AppPaths, file: &AdapterFile) -> Result<(), String> {
    file.validate()?;
    let _serialized = SETTINGS_WRITES.lock();
    let reg_path = registry_path(paths);
    if reg_path.exists() {
        let text = std::fs::read_to_string(&reg_path)
            .map_err(|e| format!("Cannot read the provider registry: {e}"))?;
        let mut registry = super::provider_profiles::ProviderRegistry::parse(&text)
            .map_err(|e| format!("Cannot parse the provider registry: {e}"))?;
        registry.save_custom_adapter(file.clone());
        return write_atomic(&reg_path, registry.to_pretty().as_bytes());
    }
    write_atomic(&settings_path(paths), file.to_pretty().as_bytes())
}

/// Writes through a unique, exclusively created temporary that is renamed over `path` (and
/// removed again if anything fails): a write never shares a file with another.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let temporary = path.with_extension(format!(
        "json.{}.{}.tmp",
        std::process::id(),
        TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|e| format!("Cannot write the adapter settings: {e}"))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("Cannot write the adapter settings: {e}"))?;
        drop(file);
        std::fs::rename(&temporary, path)
            .map_err(|e| format!("Cannot save the adapter settings: {e}"))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// Removes the saved description (blank Setup text).
pub fn clear_settings(paths: &AppPaths) -> Result<(), String> {
    let _serialized = SETTINGS_WRITES.lock();
    let _ = std::fs::remove_file(registry_path(paths));
    match std::fs::remove_file(settings_path(paths)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Cannot remove the adapter settings: {e}")),
    }
}

// ---- inbox -------------------------------------------------------------------------------------

/// The result of opening a workflow on its own thread.
pub struct Opened {
    /// The shell's open serial: a result for a project the shell has left is closed.
    pub serial: u64,
    pub result: Result<AgentWorkflow, WorkflowError>,
    /// The saved adapter description (what the Setup tab starts with).
    pub adapter: Option<AdapterFile>,
    /// The provider registry with first-party profiles.
    pub registry: Option<super::provider_profiles::ProviderRegistry>,
    /// Display-only qualification states read from the installed M6 ledger.
    pub qualification: super::provider_profiles::QualificationSnapshot,
    /// The saved description could not be read or is invalid.
    pub settings_error: Option<String>,
    /// How the saved adapter's writer containment was resolved (`None` without an adapter).
    pub resolution: Option<Resolution>,
    /// The ownership policy this workflow was opened under (saving reuses it).
    pub policy: OwnershipPolicy,
}
/// A post-commit hand-off tagged with the workflow it came from.
pub struct QueuedHandoff {
    pub serial: u64,
    pub handoff: PromotionHandoff,
}

/// Everything workflow threads hand to the UI thread.
#[derive(Default)]
pub struct HostInbox {
    opened: Mutex<Vec<Opened>>,
    handoffs: Mutex<VecDeque<QueuedHandoff>>,
    dirty: AtomicBool,
}

impl HostInbox {
    pub fn wake(&self) {
        self.dirty.store(true, Ordering::Release);
    }
    /// True once per wake.
    pub fn take_wake(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }
    pub fn push_opened(&self, opened: Opened) {
        self.opened.lock().push(opened);
        self.wake();
    }
    pub fn take_opened(&self) -> Vec<Opened> {
        std::mem::take(&mut *self.opened.lock())
    }
    pub fn take_handoffs(&self) -> Vec<QueuedHandoff> {
        self.handoffs.lock().drain(..).collect()
    }
    pub fn pending_handoffs(&self) -> usize {
        self.handoffs.lock().len()
    }
}

/// The preview sink: forwards a committed promotion to the UI thread, which adopts it and
/// answers with `report_handoff`.
pub struct UiHandoff {
    serial: u64,
    inbox: Arc<HostInbox>,
}

impl UiHandoff {
    pub fn new(serial: u64, inbox: Arc<HostInbox>) -> Self {
        Self { serial, inbox }
    }
}

impl PreviewHandoff for UiHandoff {
    fn handoff(&self, handoff: PromotionHandoff) -> Result<(), String> {
        self.inbox.handoffs.lock().push_back(QueuedHandoff {
            serial: self.serial,
            handoff,
        });
        self.inbox.wake();
        // Queued for the UI thread; the adoption outcome arrives through report_handoff.
        Ok(())
    }
}

// ---- configuration -----------------------------------------------------------------------------

/// Everything the workflow needs from the shell for one project.
pub struct HostParams {
    pub paths: AppPaths,
    pub build: Option<BuildSettings>,
    pub adapter: Option<AdapterSettings>,
    pub serial: u64,
    pub inbox: Arc<HostInbox>,
}

pub fn workflow_config(params: HostParams) -> WorkflowConfig {
    let mut config = WorkflowConfig::new(params.paths.clone());
    config.adapter = params.adapter;
    // The global PATH is never consulted: an absolute executable, or a name inside the
    // app-managed adapter directory.
    config.search = ExecutableSearch {
        managed_dirs: vec![params.paths.data.join("adapters")],
        gui_path: None,
    };
    config.build = params.build;
    config.tools = Some(ToolSettings {
        runtime_dir: short_runtime_dir(),
        studio_mcp: crate::agent_tools::sibling_binary("studio-mcp"),
        studio_tools: crate::agent_tools::sibling_binary("studio-tools"),
    });
    config.handoff = Some(Arc::new(UiHandoff::new(
        params.serial,
        params.inbox.clone(),
    )));
    let wake = params.inbox.clone();
    config.notify = Some(Arc::new(move || wake.wake()));
    config
}

/// Loads the saved adapter description and opens the workflow, on the calling (background)
/// thread: replaying the conversation log must never run on the UI thread.
pub fn open_workflow(
    controller: Arc<Mutex<Controller>>,
    paths: AppPaths,
    build: Option<BuildSettings>,
    serial: u64,
    inbox: Arc<HostInbox>,
    policy: OwnershipPolicy,
) -> Opened {
    let qualification = load_qualification_snapshot(&paths);
    let (registry, adapter, settings_error) = match load_registry(&paths) {
        Ok(Some(reg)) => {
            let adapter = reg.selected_adapter().cloned();
            (Some(reg), adapter, None)
        }
        Ok(None) => match load_settings(&paths) {
            Ok(adapter) => (None, adapter, None),
            Err(e) => (None, None, Some(e)),
        },
        Err(e) => (None, None, Some(e)),
    };
    // Hashing the adapter executable and reading the qualification ledger happen here, on
    // the opening thread.
    let resolved = adapter.as_ref().map(|a| a.resolve(&paths, &policy));
    let (settings, resolution) = match resolved {
        Some((settings, resolution)) => (Some(settings), Some(resolution)),
        None => (None, None),
    };
    let config = workflow_config(HostParams {
        paths,
        build,
        adapter: settings,
        serial,
        inbox,
    });
    Opened {
        serial,
        result: AgentWorkflow::open(config, controller),
        adapter,
        registry,
        qualification,
        settings_error,
        resolution,
        policy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_bootstrap::WriterOwnership;

    fn file(json: &str) -> Result<Option<AdapterFile>, String> {
        AdapterFile::parse(json)
    }

    #[test]
    fn blank_text_means_no_adapter_and_an_empty_executable_is_refused() {
        assert_eq!(file("  \n"), Ok(None));
        let error = file(r#"{"executable":""}"#).unwrap_err();
        assert!(error.contains("executable"), "{error}");
    }

    fn data() -> (tempfile::TempDir, AppPaths) {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        (temp, paths)
    }

    #[test]
    fn migration_stops_before_writing_when_rollback_backup_fails() {
        let (_temp, paths) = data();
        let legacy_path = settings_path(&paths);
        std::fs::write(
            &legacy_path,
            r#"{"provider":"Custom","executable":"/opt/agent"}"#,
        )
        .unwrap();
        let backup_path = legacy_path.with_extension(format!("json{REGISTRY_BACKUP_SUFFIX}"));
        std::fs::create_dir(&backup_path).unwrap();

        let error = load_registry(&paths).unwrap_err();

        assert!(error.contains("rollback backup"), "{error}");
        assert!(!registry_path(&paths).exists());
        assert!(legacy_path.is_file());
    }

    #[test]
    fn registry_save_preserves_the_current_file_when_rollback_backup_fails() {
        let (_temp, paths) = data();
        let registry_path = registry_path(&paths);
        let original = super::super::provider_profiles::ProviderRegistry::default().to_pretty();
        std::fs::write(&registry_path, &original).unwrap();
        let backup_path = registry_path.with_extension(format!("json{REGISTRY_BACKUP_SUFFIX}"));
        std::fs::create_dir(&backup_path).unwrap();
        let mut updated = super::super::provider_profiles::ProviderRegistry::default();
        updated.select_profile("claude").unwrap();

        let error = save_registry(&paths, &updated).unwrap_err();

        assert!(error.contains("rollback backup"), "{error}");
        assert_eq!(std::fs::read_to_string(registry_path).unwrap(), original);
    }

    #[test]
    fn a_minimal_description_defaults_to_baseline_mcp_and_unknown_ownership() {
        let (_temp, paths) = data();
        let parsed = file(r#"{"executable":"/opt/adapter"}"#).unwrap().unwrap();
        let (resolved, resolution) = parsed.resolve(&paths, &OwnershipPolicy::Validated);
        assert_eq!(resolved.provider, "agent");
        assert_eq!(resolved.mcp, McpStdioSupport::Baseline);
        assert_eq!(resolved.writer_ownership, WriterOwnership::Unknown);
        assert!(matches!(
            resolution.containment,
            super::super::qualification::Containment::Unknown { .. }
        ));
        assert!(resolved.adapter.auth_env_names.is_empty());
        // Every launch derives ownership again.
        assert_eq!(
            (resolved.ownership_probe.as_ref().unwrap())(),
            WriterOwnership::Unknown
        );
    }

    #[test]
    fn a_writer_qualification_setting_is_no_longer_accepted_anywhere() {
        for key in [
            "m3.writer_group.linux",
            "development-fixture-NOT-A-QUALIFICATION",
        ] {
            let json = serde_json::json!({"executable": "/a", "writer_qualification": key});
            let error = file(&json.to_string()).unwrap_err();
            assert!(error.contains("no longer a setting"), "{error}");
            assert!(!error.contains(key));
        }
        // And the Setup fields cannot carry one: containment is derived, not typed.
        let fields = AdapterFields {
            executable: "/a".into(),
            ..Default::default()
        };
        let built = AdapterFile::from_fields(None, &fields, McpChoice::Baseline)
            .unwrap()
            .unwrap();
        assert!(!built.to_pretty().contains("qualification"));
    }

    #[test]
    fn test_injection_is_explicit_and_labelled_and_the_default_policy_never_injects() {
        let (_temp, paths) = data();
        let parsed = file(r#"{"executable":"/opt/adapter"}"#).unwrap().unwrap();
        let injected = WriterOwnership::ProcessGroupContained {
            qualification: "test-fixture".into(),
        };
        let (settings, resolution) =
            parsed.resolve(&paths, &OwnershipPolicy::TestInjected(injected.clone()));
        assert_eq!(settings.writer_ownership, injected);
        assert!(resolution.summary().contains("not a qualification"));
        let (settings, _) = parsed.resolve(&paths, &OwnershipPolicy::Validated);
        assert_eq!(settings.writer_ownership, WriterOwnership::Unknown);
    }

    #[test]
    fn a_secret_value_pasted_as_a_variable_name_is_refused_and_never_echoed() {
        for bad in ["sk-live-abc123", "TOKEN=hunter2", "my token", "1ABC", "a.b"] {
            let json = serde_json::json!({"executable": "/a", "auth_env_names": [bad]});
            let error = file(&json.to_string()).unwrap_err();
            assert!(error.contains("NAMES"), "{bad}: {error}");
            assert!(!error.contains(bad), "the value is not echoed");
        }
        let ok =
            serde_json::json!({"executable": "/a", "auth_env_names": ["PROVIDER_API_KEY", "_X1"]});
        assert!(file(&ok.to_string()).unwrap().is_some());
    }

    #[test]
    fn unknown_fields_are_refused_so_a_typo_cannot_silently_drop_a_setting() {
        assert!(file(r#"{"executable":"/a","auth_env":["X"]}"#).is_err());
        assert!(file("not json").is_err());
        assert!(file(r#"{"executable":"/a","mcp":"sometimes"}"#).is_err());
    }

    #[test]
    fn argument_lines_round_trip_through_quotes() {
        assert_eq!(
            split_args(r#"--acp  "two words" 'x y' plain a\ b "q\"uote" """#).unwrap(),
            vec!["--acp", "two words", "x y", "plain", "a b", "q\"uote", ""]
        );
        assert!(split_args("\"unclosed").is_err());
        assert!(split_args("trailing\\").is_err());
        for args in [
            vec![
                "--acp".to_owned(),
                "a b".into(),
                "q\"uote".into(),
                String::new(),
                "c\\d".into(),
            ],
            vec![],
        ] {
            assert_eq!(
                split_args(&join_args(&args)).unwrap(),
                args,
                "{}",
                join_args(&args)
            );
        }
    }

    #[test]
    fn setup_fields_build_a_description_and_keep_what_they_do_not_show() {
        let base = AdapterFile {
            provider: "old".into(),
            executable: "/old".into(),
            args: vec![],
            auth_env_names: vec![],
            auth_method: Some("oauth".into()),
            mcp: McpChoice::Baseline,
        };
        let fields = AdapterFields {
            provider: " tool ".into(),
            executable: " /opt/a ".into(),
            args: "--acp --flag \"v w\"".into(),
            env_names: "KEY_A, KEY_B  KEY_C".into(),
        };
        let built = AdapterFile::from_fields(Some(&base), &fields, McpChoice::Unsupported)
            .unwrap()
            .unwrap();
        assert_eq!(built.provider, "tool");
        assert_eq!(built.executable, "/opt/a");
        assert_eq!(built.args, vec!["--acp", "--flag", "v w"]);
        assert_eq!(built.auth_env_names, vec!["KEY_A", "KEY_B", "KEY_C"]);
        assert_eq!(built.auth_method.as_deref(), Some("oauth"));
        assert_eq!(built.mcp, McpChoice::Unsupported);
        assert_eq!(built.fields().args, "--acp --flag \"v w\"");
        // Blank fields mean "no adapter"; a value in an environment field is refused.
        assert_eq!(
            AdapterFile::from_fields(None, &AdapterFields::default(), McpChoice::Baseline),
            Ok(None)
        );
        let mut secret = fields.clone();
        secret.env_names = "KEY=value".into();
        assert!(AdapterFile::from_fields(None, &secret, McpChoice::Baseline).is_err());
        let mut no_exe = fields;
        no_exe.executable = " ".into();
        assert!(AdapterFile::from_fields(None, &no_exe, McpChoice::Baseline).is_err());
    }

    #[test]
    fn settings_round_trip_atomically_with_owner_only_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        assert_eq!(load_settings(&paths), Ok(None));
        let saved = AdapterFile {
            provider: "p".into(),
            executable: "/opt/a".into(),
            args: vec!["--acp".into()],
            auth_env_names: vec!["API_KEY".into()],
            auth_method: None,
            mcp: McpChoice::Unsupported,
        };
        save_settings(&paths, &saved).unwrap();
        assert_eq!(load_settings(&paths).unwrap().unwrap(), saved);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(settings_path(&paths))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "not readable by other users");
        }
        assert!(no_temporaries(&paths));
        std::fs::write(settings_path(&paths), "{broken").unwrap();
        assert!(
            load_settings(&paths).is_err(),
            "a damaged file is reported, not ignored"
        );
        clear_settings(&paths).unwrap();
        assert_eq!(load_settings(&paths), Ok(None));
        clear_settings(&paths).unwrap();
    }

    fn no_temporaries(paths: &AppPaths) -> bool {
        std::fs::read_dir(&paths.data)
            .unwrap()
            .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
    }

    fn described(provider: &str, args: usize) -> AdapterFile {
        AdapterFile {
            provider: provider.into(),
            executable: "/opt/adapter".into(),
            // Differing lengths: a shared, truncated temporary would corrupt the shorter.
            args: (0..args)
                .map(|i| format!("--argument-number-{i}"))
                .collect(),
            auth_env_names: vec![],
            auth_method: None,
            mcp: McpChoice::Baseline,
        }
    }

    #[test]
    fn overlapping_saves_never_corrupt_the_settings_file_or_leave_temporaries() {
        let (_temp, paths) = data();
        let variants: Vec<AdapterFile> = (0..8)
            .map(|i| described(&format!("p{i}"), i * 40))
            .collect();
        for round in 0..20 {
            std::thread::scope(|scope| {
                for variant in &variants {
                    let paths = &paths;
                    scope.spawn(move || save_settings(paths, variant).unwrap());
                }
            });
            let loaded = load_settings(&paths)
                .unwrap_or_else(|e| panic!("round {round}: the file is damaged: {e}"))
                .expect("a saved description exists");
            assert!(
                variants.contains(&loaded),
                "round {round}: not one of the saves"
            );
            assert!(no_temporaries(&paths), "round {round}");
        }
    }

    #[test]
    fn a_save_then_clear_then_save_ends_in_the_last_operation() {
        let (_temp, paths) = data();
        let (long, short) = (described("long", 120), described("short", 1));
        save_settings(&paths, &long).unwrap();
        clear_settings(&paths).unwrap();
        assert_eq!(load_settings(&paths), Ok(None));
        save_settings(&paths, &short).unwrap();
        assert_eq!(load_settings(&paths).unwrap().unwrap(), short);
        // Racing a clear against saves still leaves a valid state (absent or one save).
        std::thread::scope(|scope| {
            scope.spawn(|| clear_settings(&paths).unwrap());
            scope.spawn(|| save_settings(&paths, &long).unwrap());
            scope.spawn(|| save_settings(&paths, &short).unwrap());
        });
        match load_settings(&paths).unwrap() {
            None => (),
            Some(file) => assert!(file == long || file == short),
        }
        assert!(no_temporaries(&paths));
    }

    #[test]
    fn the_inbox_wakes_once_and_holds_results_until_taken() {
        let inbox = HostInbox::default();
        assert!(!inbox.take_wake());
        inbox.wake();
        inbox.wake();
        assert!(inbox.take_wake());
        assert!(!inbox.take_wake());
        assert!(inbox.take_opened().is_empty());
        assert_eq!(inbox.pending_handoffs(), 0);
    }

    #[test]
    fn the_workflow_config_never_consults_the_global_path_and_names_the_managed_dir() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let inbox = Arc::new(HostInbox::default());
        let config = workflow_config(HostParams {
            paths: paths.clone(),
            build: None,
            adapter: None,
            serial: 7,
            inbox: inbox.clone(),
        });
        assert!(config.search.gui_path.is_none());
        assert_eq!(
            config.search.managed_dirs,
            vec![paths.data.join("adapters")]
        );
        assert!(config.adapter.is_none());
        assert!(config.handoff.is_some());
        (config.notify.as_ref().unwrap())();
        assert!(inbox.take_wake(), "a published snapshot wakes the UI");
    }
}
