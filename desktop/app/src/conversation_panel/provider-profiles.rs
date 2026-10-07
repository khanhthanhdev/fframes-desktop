//! App-local provider registry and profile management.
//!
//! Provides first-party profiles for Claude, Codex, Pi, and Antigravity,
//! migration from legacy `agent-adapter.json`, and validation preventing
//! credentials in configuration.

use super::host::{AdapterFile, McpChoice};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Current schema version of the provider registry file.
pub const REGISTRY_SCHEMA_VERSION: u32 = 1;

/// Stable identifier for a provider profile.
pub const BUILTIN_CLAUDE_ID: &str = "claude";
pub const BUILTIN_CODEX_ID: &str = "codex";
pub const BUILTIN_PI_ID: &str = "pi";
pub const BUILTIN_ANTIGRAVITY_ID: &str = "antigravity";

/// Identifiers of the four first-party ACP adapter profiles.
pub const BUILTIN_IDS: [&str; 4] = [
    BUILTIN_CLAUDE_ID,
    BUILTIN_CODEX_ID,
    BUILTIN_PI_ID,
    BUILTIN_ANTIGRAVITY_ID,
];

/// The status reported for a provider by the installed M6 evidence ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderQualificationStatus {
    Qualified,
    Experimental,
    Blocked,
    #[default]
    NotRun,
    Invalid,
}

impl ProviderQualificationStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Qualified => "Ledger reports qualified",
            Self::Experimental => "Ledger reports experimental",
            Self::Blocked => "Ledger reports blocked",
            Self::NotRun => "Qualification not run",
            Self::Invalid => "Qualification ledger invalid",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QualificationRankingStatus {
    Recommended,
    #[default]
    InsufficientEvidence,
    Invalid,
}

/// Small, display-only summary read off the UI thread. Qualification enforcement continues
/// to use the launch-bound evidence resolver, not this presentation snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QualificationSnapshot {
    pub providers: HashMap<String, ProviderQualificationStatus>,
    pub ranking: QualificationRankingStatus,
    pub recommended: Vec<String>,
    pub notice: Option<String>,
}

impl QualificationSnapshot {
    pub fn invalid() -> Self {
        Self {
            ranking: QualificationRankingStatus::Invalid,
            notice: Some(
                "The M6 qualification ledger is invalid; no recommendation is shown.".into(),
            ),
            ..Self::default()
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let ledger: serde_json::Value =
            serde_json::from_str(text).map_err(|_| "invalid M6 ledger JSON".to_string())?;
        if ledger.get("kind").and_then(serde_json::Value::as_str) != Some("m6")
            || ledger
                .get("schema_version")
                .and_then(serde_json::Value::as_u64)
                != Some(1)
        {
            return Err("unsupported M6 ledger schema".into());
        }
        let providers = ledger
            .get("providers")
            .and_then(serde_json::Value::as_object)
            .ok_or("M6 ledger has no providers")?;
        if providers.len() != BUILTIN_IDS.len()
            || BUILTIN_IDS.iter().any(|id| !providers.contains_key(*id))
        {
            return Err("M6 ledger provider set is incomplete".into());
        }
        let mut statuses = HashMap::new();
        for id in BUILTIN_IDS {
            let provider = &providers[id];
            if provider.get("id").and_then(serde_json::Value::as_str) != Some(id) {
                return Err("M6 ledger provider identity mismatch".into());
            }
            let status = match provider.get("status").and_then(serde_json::Value::as_str) {
                Some("qualified") => ProviderQualificationStatus::Qualified,
                Some("experimental") => ProviderQualificationStatus::Experimental,
                Some("blocked") => ProviderQualificationStatus::Blocked,
                Some("not_run") => ProviderQualificationStatus::NotRun,
                _ => return Err("M6 ledger contains an unknown provider status".into()),
            };
            statuses.insert(id.to_string(), status);
        }

        let ranking = ledger.get("ranking").ok_or("M6 ledger has no ranking")?;
        let recommended = ranking
            .get("recommended")
            .and_then(serde_json::Value::as_array)
            .ok_or("M6 ledger recommendation is invalid")?
            .iter()
            .map(|id| {
                id.as_str()
                    .map(str::to_owned)
                    .ok_or("M6 ledger recommendation is invalid")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if recommended.len()
            != recommended
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
        {
            return Err("M6 ledger recommendation contains duplicates".into());
        }
        let qualified_count = statuses
            .values()
            .filter(|status| **status == ProviderQualificationStatus::Qualified)
            .count();
        let ranking_status = match ranking.get("status").and_then(serde_json::Value::as_str) {
            Some("recommended") if recommended.len() == 2 && qualified_count >= 2 => {
                if recommended
                    .iter()
                    .any(|id| statuses.get(id) != Some(&ProviderQualificationStatus::Qualified))
                {
                    return Err("M6 ledger recommends an unqualified provider".into());
                }
                QualificationRankingStatus::Recommended
            }
            Some("insufficient_evidence") if recommended.is_empty() && qualified_count < 2 => {
                QualificationRankingStatus::InsufficientEvidence
            }
            _ => return Err("M6 ledger ranking state is inconsistent".into()),
        };

        Ok(Self {
            providers: statuses,
            ranking: ranking_status,
            recommended,
            notice: None,
        })
    }

    pub fn status_for(&self, profile_id: &str) -> ProviderQualificationStatus {
        if self.ranking == QualificationRankingStatus::Invalid {
            return ProviderQualificationStatus::Invalid;
        }
        self.providers
            .get(profile_id)
            .copied()
            .unwrap_or(ProviderQualificationStatus::NotRun)
    }
}

/// Checks whether an ID belongs to a first-party provider profile.
pub fn is_builtin_id(id: &str) -> bool {
    BUILTIN_IDS.contains(&id)
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
    vec![
        ProviderProfile {
            id: BUILTIN_CLAUDE_ID.into(),
            label: "Claude".into(),
            description: "Anthropic Claude via ACP adapter (@agentclientprotocol/claude-agent-acp)"
                .into(),
            adapter: AdapterFile {
                provider: "Claude".into(),
                executable: "claude-agent-acp".into(),
                args: Vec::new(),
                auth_env_names: vec!["ANTHROPIC_API_KEY".into()],
                auth_method: None,
                mcp: McpChoice::Baseline,
            },
            builtin: true,
            experimental: true,
        },
        ProviderProfile {
            id: BUILTIN_CODEX_ID.into(),
            label: "Codex".into(),
            description: "Codex App Server via ACP adapter (@agentclientprotocol/codex-acp)".into(),
            adapter: AdapterFile {
                provider: "Codex".into(),
                executable: "codex-acp".into(),
                args: Vec::new(),
                auth_env_names: vec!["OPENAI_API_KEY".into(), "CODEX_API_KEY".into()],
                auth_method: None,
                mcp: McpChoice::Baseline,
            },
            builtin: true,
            experimental: true,
        },
        ProviderProfile {
            id: BUILTIN_PI_ID.into(),
            label: "Pi".into(),
            description: "Pi via community ACP adapter by svkozak (pi-acp)".into(),
            adapter: AdapterFile {
                provider: "Pi".into(),
                executable: "pi-acp".into(),
                args: Vec::new(),
                auth_env_names: Vec::new(),
                auth_method: None,
                mcp: McpChoice::Unsupported,
            },
            builtin: true,
            experimental: true,
        },
        ProviderProfile {
            id: BUILTIN_ANTIGRAVITY_ID.into(),
            label: "Antigravity".into(),
            description: "Google Antigravity via official ACP binary (antigravity-acp)".into(),
            adapter: AdapterFile {
                provider: "Antigravity".into(),
                executable: "antigravity-acp".into(),
                args: Vec::new(),
                auth_env_names: vec!["GEMINI_API_KEY".into(), "GOOGLE_API_KEY".into()],
                auth_method: None,
                mcp: McpChoice::Baseline,
            },
            builtin: true,
            experimental: true,
        },
    ]
}

impl ProviderRegistry {
    /// Parses registry JSON from a string.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut registry: Self = serde_json::from_str(text)
            .map_err(|e| format!("Invalid provider registry JSON: {e}"))?;
        for profile in &mut registry.profiles {
            profile.builtin = is_builtin_id(&profile.id);
            profile.experimental = true;
        }
        registry.validate()?;
        Ok(registry)
    }

    /// Validates the registry structure, schema version, and profile contents.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version == 0 {
            return Err("Registry schema_version cannot be 0.".into());
        }
        if self.schema_version > REGISTRY_SCHEMA_VERSION {
            return Err(format!(
                "Registry schema version {} is newer than supported version {}. It is diagnostic-only and cannot be launched.",
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

    /// Migrates an existing `agent-adapter.json` description into a full `ProviderRegistry`.
    ///
    /// Preserves all existing fields, arguments, and custom labels.
    pub fn migrate_from_adapter_file(legacy: &AdapterFile) -> Self {
        let mut profiles = default_builtin_profiles();
        let legacy_norm = legacy.provider.trim().to_lowercase();

        // Check if legacy matches one of the builtin providers strictly by name or id
        let matched_builtin = profiles
            .iter_mut()
            .find(|p| p.id == legacy_norm || p.label.to_lowercase() == legacy_norm);

        let selected_id = if let Some(builtin) = matched_builtin {
            builtin.adapter = legacy.clone();
            builtin.id.clone()
        } else {
            // Create a custom profile preserving all fields and labels
            let custom_id = slugify(&legacy.provider);
            let profile = ProviderProfile {
                id: custom_id.clone(),
                label: legacy.provider.clone(),
                description: format!("Imported configuration for {}", legacy.provider),
                adapter: legacy.clone(),
                builtin: false,
                experimental: true,
            };
            profiles.push(profile);
            custom_id
        };

        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            selected_profile_id: Some(selected_id),
            profiles,
        }
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
            builtin: false,
            experimental: true,
        };
        self.profiles.push(profile);
        self.selected_profile_id = Some(id);
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
